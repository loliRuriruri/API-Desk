use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use super::models::SourceType;
use super::pipeline::{capture_state_file, handle_file_removed};
use crate::error::AppError;

/// 기본 디바운스 대기 시간 (500ms).
pub const DEFAULT_DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

/// Windows 11 Notepad의 LocalState 디렉터리 경로를 반환합니다.
pub fn default_notepad_local_state_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(|local_app_data| {
            PathBuf::from(local_app_data)
                .join("Packages")
                .join("Microsoft.WindowsNotepad_8wekyb3d8bbwe")
                .join("LocalState")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 디바운스 대기 중인 파일 이벤트 항목.
#[derive(Debug, Clone)]
struct PendingEvent {
    source_type: SourceType,
    deadline: Instant,
    is_remove: bool,
}

/// 백그라운드 파일 감시자 핸들.
pub struct WatcherHandle {
    pub is_running: Arc<AtomicBool>,
    _watcher: Option<RecommendedWatcher>,
}

impl WatcherHandle {
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::Relaxed);
    }
}

/// 시작 시 기존 탭/세션 디렉터리를 1회 스캔하여 API Desk가 꺼져 있을 때 생성된 파일들을 즉시 백업하고,
/// 디스크상에서 사라진 탭은 안전하게 soft-delete (is_deleted = 1)로 상태를 동기화합니다.
pub async fn scan_and_reconcile(
    tab_state_dir: &Path,
    window_state_dir: &Path,
    pool: &SqlitePool,
    storage_root: &Path,
    app: Option<&tauri::AppHandle>,
) -> usize {
    let mut captured_count = 0;

    if tab_state_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(tab_state_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    if let Ok(Some(_)) =
                        capture_state_file(&path, SourceType::TabState, pool, storage_root, app)
                            .await
                    {
                        captured_count += 1;
                    }
                }
            }
        }

        // DB상 활성(is_deleted = 0) 노트 중 디스크에서 사라진 탭을 찾아 삭제 마킹
        if let Ok(active_notes) = super::repository::list_notes(
            pool,
            &super::models::NoteFilter {
                is_deleted: Some(false),
                ..Default::default()
            },
        )
        .await
        {
            let deleted_at = super::pipeline::now_rfc3339();
            for note in active_notes {
                let tab_path = tab_state_dir.join(format!("{}.bin", note.source_id));
                let tab_0_path = tab_state_dir.join(format!("{}.0.bin", note.source_id));
                if !tab_path.exists() && !tab_0_path.exists() {
                    if let Ok(true) =
                        super::repository::mark_note_deleted(pool, &note.source_id, &deleted_at)
                            .await
                    {
                        if let Some(app_handle) = app {
                            use tauri::Emitter;
                            let _ = app_handle.emit(
                                "rescue://note-deleted",
                                &super::models::NoteDeletedEvent {
                                    note_id: note.id,
                                    source_id: note.source_id,
                                    deleted_at: deleted_at.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    if window_state_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(window_state_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    let _ =
                        capture_state_file(&path, SourceType::WindowState, pool, storage_root, app)
                            .await;
                }
            }
        }
    }

    captured_count
}

/// TabState 및 WindowState 디렉터리를 감시하고 디바운스 처리 후 캡처 파이프라인을 실행합니다.
pub async fn start_notepad_watcher(
    tab_state_dir: PathBuf,
    window_state_dir: PathBuf,
    pool: SqlitePool,
    storage_root: PathBuf,
    debounce_dur: Duration,
    app: Option<tauri::AppHandle>,
) -> Result<WatcherHandle, AppError> {
    let is_running = Arc::new(AtomicBool::new(true));

    // 1. 시작 시 기존 파일 초기 스캔 및 보호 등록 (Startup Reconciliation)
    scan_and_reconcile(
        &tab_state_dir,
        &window_state_dir,
        &pool,
        &storage_root,
        app.as_ref(),
    )
    .await;

    // 감시 대상 디렉터리가 아직 없는 경우 생성하거나 종료
    let has_tab_dir = tab_state_dir.is_dir();
    let has_win_dir = window_state_dir.is_dir();

    if !has_tab_dir && !has_win_dir {
        // Notepad가 설치되지 않았거나 아직 한 번도 실행되지 않은 경우
        return Ok(WatcherHandle {
            is_running,
            _watcher: None,
        });
    }

    let pending: Arc<Mutex<HashMap<PathBuf, PendingEvent>>> = Arc::new(Mutex::new(HashMap::new()));
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    )
    .map_err(|e| AppError::Io(format!("Watcher 생성 실패: {e}")))?;

    if has_tab_dir {
        let _ = watcher.watch(&tab_state_dir, RecursiveMode::NonRecursive);
    }
    if has_win_dir {
        let _ = watcher.watch(&window_state_dir, RecursiveMode::NonRecursive);
    }

    // 2. 이벤트 수신 및 디바운스 맵 갱신 워커 (채널 브리지)
    let pending_clone = Arc::clone(&pending);
    let running_clone = Arc::clone(&is_running);
    let tab_dir_clone = tab_state_dir.clone();
    let win_dir_clone = window_state_dir.clone();

    std::thread::spawn(move || {
        while running_clone.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => {
                    let is_remove = matches!(event.kind, EventKind::Remove(_));
                    let now = Instant::now();
                    let deadline = now + debounce_dur;

                    for path in event.paths {
                        let source_type = if path.starts_with(&tab_dir_clone) {
                            SourceType::TabState
                        } else if path.starts_with(&win_dir_clone) {
                            SourceType::WindowState
                        } else {
                            continue;
                        };

                        // .tmp, .bak 등은 이벤트 무시
                        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                            if ext != "bin" {
                                continue;
                            }
                        } else {
                            continue;
                        }

                        let mut map = tauri::async_runtime::block_on(pending_clone.lock());
                        // Trailing-edge coalescing: 동일 파일에 대해 반복 이벤트가 들어오면 마감 시각을 연장함
                        map.insert(
                            path,
                            PendingEvent {
                                source_type,
                                deadline,
                                is_remove,
                            },
                        );
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    // 3. 디바운스 만료 처리 루프 (Trailing edge 실행)
    let pending_dispatch = Arc::clone(&pending);
    let running_dispatch = Arc::clone(&is_running);

    tauri::async_runtime::spawn(async move {
        while running_dispatch.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;

            let now = Instant::now();
            let mut expired = Vec::new();

            {
                let mut map = pending_dispatch.lock().await;
                let keys: Vec<PathBuf> = map.keys().cloned().collect();
                for key in keys {
                    if let Some(entry) = map.get(&key) {
                        if now >= entry.deadline {
                            if let Some(removed) = map.remove(&key) {
                                expired.push((key, removed));
                            }
                        }
                    }
                }
            }

            for (path, entry) in expired {
                if entry.is_remove || !path.exists() {
                    let _ =
                        handle_file_removed(&path, entry.source_type, &pool, app.as_ref()).await;
                } else {
                    let _ = capture_state_file(
                        &path,
                        entry.source_type,
                        &pool,
                        &storage_root,
                        app.as_ref(),
                    )
                    .await;
                }
            }
        }
    });

    Ok(WatcherHandle {
        is_running,
        _watcher: Some(watcher),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notepad_rescue::parser::generate_synthetic_unsaved;

    #[test]
    fn test_debounce_coalescing() {
        let temp_dir = std::env::temp_dir().join("api-desk-debounce-test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);

        let file_path = temp_dir.join("test-tab.bin");
        let bin_v1 = generate_synthetic_unsaved("Ver 1", false);
        std::fs::write(&file_path, &bin_v1).unwrap();

        let pending: Arc<Mutex<HashMap<PathBuf, PendingEvent>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // 100ms 간격으로 3회 이벤트 유입 (총 200ms 경과 시점)
        for _ in 1..=3 {
            let deadline = Instant::now() + Duration::from_millis(300);
            tauri::async_runtime::block_on(async {
                let mut map = pending.lock().await;
                map.insert(
                    file_path.clone(),
                    PendingEvent {
                        source_type: SourceType::TabState,
                        deadline,
                        is_remove: false,
                    },
                );
            });
            std::thread::sleep(Duration::from_millis(50));
        }

        // 마지막 갱신으로부터 300ms 이전에는 맵에 남아있어야 함 (아직 만료되지 않음)
        tauri::async_runtime::block_on(async {
            let map = pending.lock().await;
            assert_eq!(map.len(), 1);
            let entry = map.get(&file_path).unwrap();
            assert!(Instant::now() < entry.deadline);
        });

        // 350ms 대기 후 만료 확인
        std::thread::sleep(Duration::from_millis(350));
        tauri::async_runtime::block_on(async {
            let map = pending.lock().await;
            let entry = map.get(&file_path).unwrap();
            assert!(Instant::now() >= entry.deadline);
        });

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
