pub mod models;
pub mod parser;
pub mod pipeline;
pub mod reader;
pub mod repository;
pub mod snapshot;
pub mod watcher;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

use crate::error::AppError;

pub use models::*;
pub use parser::{parse_tab_state, parse_window_state};
pub use pipeline::capture_state_file;
pub use reader::safe_read_file;
pub use snapshot::{compute_sha256, relative_blob_path, resolve_blob_path, store_blob};

/// 스냅샷의 상대 파일 경로를 생성합니다.
pub fn relative_snapshot_path(content_hash: &str) -> String {
    relative_blob_path(content_hash)
}

/// 스토리지 루트와 상대 경로를 결합하여 실제 절대 경로를 반환합니다.
pub fn resolve_snapshot_path(storage_root: &Path, relative_path: &str) -> PathBuf {
    resolve_blob_path(storage_root, relative_path)
}

/// 메모장 복구 데이터의 기본 스토리지 루트 디렉터리를 반환합니다.
/// 기본 경로: %LOCALAPPDATA%\com.apidesk.desktop\notepad_rescue
pub fn storage_root(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    let local_data_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::Io(format!("앱 로컬 데이터 디렉터리를 찾을 수 없습니다: {e}")))?;
    Ok(local_data_dir.join("notepad_rescue"))
}

/// 스토리지 루트 디렉터리 및 서브디렉터리 구조를 생성합니다.
pub fn ensure_storage_dirs(storage_root: &Path) -> Result<(), AppError> {
    let snapshots_dir = storage_root.join("snapshots");
    std::fs::create_dir_all(&snapshots_dir)
        .map_err(|e| AppError::Io(format!("스냅샷 디렉터리 생성 실패: {e}")))?;
    Ok(())
}

/// 백엔드 상주 상태 관리자.
pub struct NotepadRescueState {
    pub is_watching: Arc<AtomicBool>,
    watcher_handle: Arc<Mutex<Option<watcher::WatcherHandle>>>,
}

impl Default for NotepadRescueState {
    fn default() -> Self {
        Self {
            is_watching: Arc::new(AtomicBool::new(false)),
            watcher_handle: Arc::new(Mutex::new(None)),
        }
    }
}

/// 백그라운드 워커를 가동합니다.
pub fn start_worker(app: &tauri::AppHandle) {
    let state = app.state::<NotepadRescueState>();
    if state.is_watching.load(Ordering::Relaxed) {
        return;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let config_dir = match handle.path().app_config_dir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let db_file = config_dir.join("apidb.db");
        let pool = match SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&db_file)
                    .read_only(false),
            )
            .await
        {
            Ok(p) => p,
            Err(_) => return,
        };

        let settings = match repository::get_settings(&pool).await {
            Ok(s) => s,
            Err(_) => return,
        };

        if !settings.enabled || !settings.watch_enabled {
            let _ = handle.emit(
                "rescue://status",
                &RescueStatusEvent {
                    enabled: settings.enabled,
                    watch_enabled: settings.watch_enabled,
                    is_watching: false,
                    status: "DISABLED".to_string(),
                    message: None,
                },
            );
            return;
        }

        let store_root = match storage_root(&handle) {
            Ok(r) => r,
            Err(_) => return,
        };
        let _ = ensure_storage_dirs(&store_root);

        let local_state = match watcher::default_notepad_local_state_dir() {
            Some(dir) => dir,
            None => {
                let _ = handle.emit(
                    "rescue://status",
                    &RescueStatusEvent {
                        enabled: settings.enabled,
                        watch_enabled: settings.watch_enabled,
                        is_watching: false,
                        status: "NOTEPAD_NOT_FOUND".to_string(),
                        message: Some("메모장 로컬 상태 폴더를 찾을 수 없습니다".to_string()),
                    },
                );
                return;
            }
        };

        let tab_dir = local_state.join("TabState");
        let win_dir = local_state.join("WindowState");

        match watcher::start_notepad_watcher(
            tab_dir,
            win_dir,
            pool,
            store_root,
            watcher::DEFAULT_DEBOUNCE_DURATION,
            Some(handle.clone()),
        )
        .await
        {
            Ok(w) => {
                let rescue_state = handle.state::<NotepadRescueState>();
                rescue_state.is_watching.store(true, Ordering::Relaxed);
                let mut guard = rescue_state.watcher_handle.lock().await;
                *guard = Some(w);
                crate::resident::log(&handle, "notepad_rescue", "watcher worker started");
                let _ = handle.emit(
                    "rescue://status",
                    &RescueStatusEvent {
                        enabled: true,
                        watch_enabled: true,
                        is_watching: true,
                        status: "ACTIVE".to_string(),
                        message: None,
                    },
                );
            }
            Err(e) => {
                crate::resident::log(
                    &handle,
                    "notepad_rescue",
                    &format!("watcher start failed: {e}"),
                );
                let _ = handle.emit(
                    "rescue://status",
                    &RescueStatusEvent {
                        enabled: true,
                        watch_enabled: true,
                        is_watching: false,
                        status: "WATCHER_ERROR".to_string(),
                        message: Some(e.to_string()),
                    },
                );
            }
        }
    });
}

/// 백그라운드 워커를 안전하게 중지합니다.
pub fn shutdown(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<NotepadRescueState>() {
        state.is_watching.store(false, Ordering::Relaxed);
        let handle = state.watcher_handle.clone();
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut guard = handle.lock().await;
            if let Some(w) = guard.take() {
                w.stop();
            }
            let _ = app_handle.emit(
                "rescue://status",
                &RescueStatusEvent {
                    enabled: true,
                    watch_enabled: true,
                    is_watching: false,
                    status: "DISABLED".to_string(),
                    message: None,
                },
            );
        });
    }
}

// ---------------------------------------------------------------- Tests
#[cfg(test)]
mod tests {
    use super::parser::generate_synthetic_unsaved;
    use super::pipeline::{capture_state_file, extract_source_id, handle_file_removed};
    use super::repository::*;
    use super::watcher::{scan_and_reconcile, start_notepad_watcher};
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;
    use std::time::Duration;

    async fn create_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("인메모리 SQLite 연결 실패");

        sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&pool)
            .await
            .expect("외래 키 활성화 실패");

        for migration in crate::migrations::migrations() {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("마이그레이션 v{} 실행 실패: {e}", migration.version));
        }

        pool
    }

    // =========================================================================
    // Group A: Reader (1 ~ 4) - reader.rs에서 검증됨 + 보조 테스트
    // =========================================================================
    #[test]
    fn test_a1_reader_normal_file() {
        let temp_dir = std::env::temp_dir().join("nr2-test-a1");
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("normal.bin");
        std::fs::write(&path, b"Normal File Content").unwrap();

        let bytes = reader::safe_read_file(&path).unwrap();
        assert_eq!(bytes, b"Normal File Content");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_a2_reader_concurrent_readable() {
        let temp_dir = std::env::temp_dir().join("nr2-test-a2");
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("concurrent.bin");
        std::fs::write(&path, b"Initial Data").unwrap();

        let read = reader::safe_read_file(&path).unwrap();
        assert_eq!(read, b"Initial Data");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_a3_reader_missing_file() {
        let path = std::env::temp_dir().join("nr2-nonexistent-a3.bin");
        assert!(reader::safe_read_file(&path).is_err());
    }

    #[test]
    fn test_a4_reader_empty_file() {
        let temp_dir = std::env::temp_dir().join("nr2-test-a4");
        let _ = std::fs::create_dir_all(&temp_dir);
        let path = temp_dir.join("empty.bin");
        std::fs::write(&path, b"").unwrap();

        let bytes = reader::safe_read_file(&path).unwrap();
        assert!(bytes.is_empty());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // =========================================================================
    // Group B: Snapshot (5 ~ 10)
    // =========================================================================
    #[test]
    fn test_b5_first_blob_capture() {
        let temp_dir = std::env::temp_dir().join("nr2-test-b5");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let data = b"Snapshot Blob Data";
        let hash = compute_sha256(data);
        let rel_path = store_blob(&temp_dir, &hash, data).unwrap();

        let abs_path = resolve_blob_path(&temp_dir, &rel_path);
        assert!(abs_path.exists());
        assert_eq!(std::fs::read(&abs_path).unwrap(), data);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_b6_same_hash_physical_dedupe() {
        let temp_dir = std::env::temp_dir().join("nr2-test-b6");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let data = b"Deduplicated Data";
        let hash = compute_sha256(data);
        let rel1 = store_blob(&temp_dir, &hash, data).unwrap();
        let rel2 = store_blob(&temp_dir, &hash, data).unwrap();

        assert_eq!(rel1, rel2);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_b7_different_notes_same_hash() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-b7");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let _ = std::fs::create_dir_all(&temp_dir);

            let bin = generate_synthetic_unsaved("Shared Note Content", false);

            let path1 = temp_dir.join("tab-b7-1.bin");
            let path2 = temp_dir.join("tab-b7-2.bin");
            std::fs::write(&path1, &bin).unwrap();
            std::fs::write(&path2, &bin).unwrap();

            let snap1 = capture_state_file(&path1, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap();
            let snap2 = capture_state_file(&path2, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap();

            assert!(snap1.is_some());
            assert!(snap2.is_some());

            let s1 = snap1.unwrap();
            let s2 = snap2.unwrap();
            assert_ne!(s1.note_id, s2.note_id);
            assert_eq!(s1.content_hash, s2.content_hash);

            let stats = get_storage_statistics(&pool).await.unwrap();
            assert_eq!(stats.total_notes, 2);
            assert_eq!(stats.total_snapshots, 2);

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_b8_atomic_publish() {
        let temp_dir = std::env::temp_dir().join("nr2-test-b8");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let data = b"Atomic Publish Data";
        let hash = compute_sha256(data);
        let rel = store_blob(&temp_dir, &hash, data).unwrap();
        let abs = resolve_blob_path(&temp_dir, &rel);

        assert!(abs.exists());
        // 임시 파일(.tmp)이 남아있지 않아야 함
        let parent = abs.parent().unwrap();
        for entry in std::fs::read_dir(parent).unwrap().flatten() {
            assert_ne!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some("tmp")
            );
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_b9_relative_path_format() {
        let hash = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let rel = relative_blob_path(hash);
        assert_eq!(
            rel,
            "snapshots/12/1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef.bin"
        );
    }

    #[test]
    fn test_b10_orphan_tmp_behavior() {
        let temp_dir = std::env::temp_dir().join("nr2-test-b10");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let dir = temp_dir.join("snapshots").join("cd");
        std::fs::create_dir_all(&dir).unwrap();

        let tmp = dir.join("cd1234.tmp");
        std::fs::write(&tmp, b"orphan").unwrap();

        let removed = snapshot::cleanup_orphaned_tmp(&temp_dir, Duration::from_millis(0)).unwrap();
        assert_eq!(removed, 1);
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // =========================================================================
    // Group D: Watcher (21 ~ 28)
    // =========================================================================
    #[test]
    fn test_d21_d22_d24_watcher_create_modify_delete_flow() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-d-watcher");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let tab_dir = temp_dir.join("TabState");
            let win_dir = temp_dir.join("WindowState");
            let store_root = temp_dir.join("store");
            std::fs::create_dir_all(&tab_dir).unwrap();
            std::fs::create_dir_all(&win_dir).unwrap();

            let watcher_handle = start_notepad_watcher(
                tab_dir.clone(),
                win_dir.clone(),
                pool.clone(),
                store_root.clone(),
                Duration::from_millis(150),
                None,
            )
            .await
            .unwrap();

            let file_path = tab_dir.join("11112222-3333-4444-5555-666677778888.bin");

            // 21. Create event
            let bin_v1 = generate_synthetic_unsaved("Watcher Version 1", false);
            std::fs::write(&file_path, &bin_v1).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;

            let note1 = get_note_by_source_id(&pool, "11112222-3333-4444-5555-666677778888")
                .await
                .unwrap();
            assert!(note1.is_some());
            let note1 = note1.unwrap();
            assert!(!note1.is_deleted);
            assert_eq!(note1.display_title, "Watcher Version 1");

            // 22. Modify event
            let bin_v2 = generate_synthetic_unsaved("Watcher Version 2 Modified", false);
            std::fs::write(&file_path, &bin_v2).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;

            let snaps = list_note_snapshots(&pool, &note1.id).await.unwrap();
            assert_eq!(snaps.len(), 2);
            assert_eq!(
                snaps[0].recovered_text.as_deref(),
                Some("Watcher Version 2 Modified")
            );

            // 24. Delete event
            std::fs::remove_file(&file_path).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;

            let deleted_note = get_note_by_source_id(&pool, "11112222-3333-4444-5555-666677778888")
                .await
                .unwrap()
                .unwrap();
            assert!(deleted_note.is_deleted);
            assert!(deleted_note.deleted_at.is_some());

            // 삭제 후에도 스냅샷은 그대로 보존되어야 함
            let snaps_after = list_note_snapshots(&pool, &note1.id).await.unwrap();
            assert_eq!(snaps_after.len(), 2);

            watcher_handle.stop();
            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_d25_extract_source_id() {
        assert_eq!(
            extract_source_id(Path::new(
                "C:\\TabState\\1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2.bin"
            )),
            Some("1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2".to_string())
        );
        assert_eq!(
            extract_source_id(Path::new(
                "C:\\TabState\\1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2.0.bin"
            )),
            Some("1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2".to_string())
        );
        assert_eq!(extract_source_id(Path::new("C:\\TabState\\test.tmp")), None);
        assert_eq!(
            extract_source_id(Path::new("C:\\TabState\\.hidden.bin")),
            None
        );
    }

    #[test]
    fn test_d26_startup_scan() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-d26");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let tab_dir = temp_dir.join("TabState");
            let win_dir = temp_dir.join("WindowState");
            let store_root = temp_dir.join("store");
            std::fs::create_dir_all(&tab_dir).unwrap();
            std::fs::create_dir_all(&win_dir).unwrap();

            // API Desk 시작 전에 이미 2개의 탭 파일이 존재
            let bin1 = generate_synthetic_unsaved("Pre-existing Tab 1", false);
            let bin2 = generate_synthetic_unsaved("Pre-existing Tab 2", false);
            std::fs::write(tab_dir.join("tab-pre-1.bin"), &bin1).unwrap();
            std::fs::write(tab_dir.join("tab-pre-2.bin"), &bin2).unwrap();

            let scanned = scan_and_reconcile(&tab_dir, &win_dir, &pool, &store_root, None).await;
            assert_eq!(scanned, 2);

            let notes = list_notes(&pool, &NoteFilter::default()).await.unwrap();
            assert_eq!(notes.len(), 2);

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_d27_watcher_restart() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-d27");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let tab_dir = temp_dir.join("TabState");
            let win_dir = temp_dir.join("WindowState");
            let store_root = temp_dir.join("store");
            std::fs::create_dir_all(&tab_dir).unwrap();
            std::fs::create_dir_all(&win_dir).unwrap();

            // 첫 번째 워커 가동 및 정지
            let w1 = start_notepad_watcher(
                tab_dir.clone(),
                win_dir.clone(),
                pool.clone(),
                store_root.clone(),
                Duration::from_millis(100),
                None,
            )
            .await
            .unwrap();
            w1.stop();

            // 두 번째 워커 재시작
            let w2 = start_notepad_watcher(
                tab_dir.clone(),
                win_dir.clone(),
                pool.clone(),
                store_root.clone(),
                Duration::from_millis(100),
                None,
            )
            .await
            .unwrap();
            assert!(w2.is_running.load(Ordering::Relaxed));
            w2.stop();

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_d28_watcher_directory_missing() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let missing_tab = PathBuf::from("C:\\fake_nonexistent_tab_dir_12345");
            let missing_win = PathBuf::from("C:\\fake_nonexistent_win_dir_12345");
            let store_root = std::env::temp_dir().join("nr2-test-d28-store");

            let res = start_notepad_watcher(
                missing_tab,
                missing_win,
                pool,
                store_root,
                Duration::from_millis(100),
                None,
            )
            .await;

            // 디렉터리가 없어도 패닉하지 않고 안전하게 핸들을 반환
            assert!(res.is_ok());
            let handle = res.unwrap();
            assert!(handle.is_running.load(Ordering::Relaxed));
            handle.stop();
        });
    }

    // =========================================================================
    // Group E: Integration (29 ~ 34)
    // =========================================================================
    #[test]
    fn test_e29_file_capture_to_blob_and_db() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-e29");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let _ = std::fs::create_dir_all(&temp_dir);

            let path = temp_dir.join("test-tab-e29.bin");
            let bin = generate_synthetic_unsaved("E29 Integration Content", false);
            std::fs::write(&path, &bin).unwrap();

            let event_opt = capture_state_file(&path, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap();
            assert!(event_opt.is_some());
            let event = event_opt.unwrap();

            assert_eq!(event.display_title, "E29 Integration Content");

            let note = get_note(&pool, &event.note_id).await.unwrap().unwrap();
            assert_eq!(note.source_id, "test-tab-e29");

            let snaps = list_note_snapshots(&pool, &note.id).await.unwrap();
            assert_eq!(snaps.len(), 1);
            assert_eq!(
                snaps[0].recovered_text.as_deref(),
                Some("E29 Integration Content")
            );

            // 물리 블롭 존재 확인
            let abs_blob = resolve_blob_path(&temp_dir, &snaps[0].raw_snapshot_path);
            assert!(abs_blob.exists());

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_e30_second_modification_second_logical_version() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-e30");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let _ = std::fs::create_dir_all(&temp_dir);

            let path = temp_dir.join("tab-e30.bin");

            // 1차 쓰기
            let bin1 = generate_synthetic_unsaved("Version One", false);
            std::fs::write(&path, &bin1).unwrap();
            let ev1 = capture_state_file(&path, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap()
                .unwrap();

            // 2차 쓰기
            let bin2 = generate_synthetic_unsaved("Version Two", false);
            std::fs::write(&path, &bin2).unwrap();
            let ev2 = capture_state_file(&path, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap()
                .unwrap();

            assert_eq!(ev1.note_id, ev2.note_id);
            assert_ne!(ev1.snapshot_id, ev2.snapshot_id);

            let snaps = list_note_snapshots(&pool, &ev1.note_id).await.unwrap();
            assert_eq!(snaps.len(), 2);
            assert_eq!(snaps[0].recovered_text.as_deref(), Some("Version Two"));
            assert_eq!(snaps[1].recovered_text.as_deref(), Some("Version One"));

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_e31_tab_deletion_soft_deleted() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-e31");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let _ = std::fs::create_dir_all(&temp_dir);

            let path = temp_dir.join("tab-e31.bin");
            let bin = generate_synthetic_unsaved("To Be Deleted", false);
            std::fs::write(&path, &bin).unwrap();

            capture_state_file(&path, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap();

            std::fs::remove_file(&path).unwrap();

            let del_event = handle_file_removed(&path, SourceType::TabState, &pool, None)
                .await
                .unwrap();
            assert!(del_event.is_some());

            let note = get_note_by_source_id(&pool, "tab-e31")
                .await
                .unwrap()
                .unwrap();
            assert!(note.is_deleted);
            assert!(note.deleted_at.is_some());

            let snaps = list_note_snapshots(&pool, &note.id).await.unwrap();
            assert_eq!(snaps.len(), 1); // 스냅샷 보존

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_e32_parser_fail_raw_blob_remains() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr2-test-e32");
            let _ = std::fs::remove_dir_all(&temp_dir);
            let _ = std::fs::create_dir_all(&temp_dir);

            let path = temp_dir.join("tab-e32.bin");
            // 매직이 손상된 쓰레기 데이터
            let corrupt_data = b"NOT_NOTEPAD_HEADER_GARBAGE_BYTES_1234567890";
            std::fs::write(&path, corrupt_data).unwrap();

            let ev = capture_state_file(&path, SourceType::TabState, &pool, &temp_dir, None)
                .await
                .unwrap();
            assert!(ev.is_some());

            let note = get_note_by_source_id(&pool, "tab-e32")
                .await
                .unwrap()
                .unwrap();
            let snaps = list_note_snapshots(&pool, &note.id).await.unwrap();
            assert_eq!(snaps.len(), 1);
            assert_eq!(snaps[0].parse_status, ParseStatus::UnsupportedFormat);

            // 중요: 파싱이 실패했어도 RAW 바이너리 블롭은 물리 스토리지에 무조건 안전 보존됨
            let abs_blob = resolve_blob_path(&temp_dir, &snaps[0].raw_snapshot_path);
            assert!(abs_blob.exists());
            assert_eq!(std::fs::read(&abs_blob).unwrap(), corrupt_data);

            let _ = std::fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_e33_database_failure_does_not_corrupt_raw_snapshot() {
        let temp_dir = std::env::temp_dir().join("nr2-test-e33");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let data = b"Raw Snapshot Independent Of DB";
        let hash = compute_sha256(data);
        let rel = store_blob(&temp_dir, &hash, data).unwrap();

        let abs = resolve_blob_path(&temp_dir, &rel);
        assert!(abs.exists());
        assert_eq!(std::fs::read(&abs).unwrap(), data);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_e34_existing_apidesk_tests_remain_pass() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let cred_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credentials")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(cred_count, 0);

            let stats = get_storage_statistics(&pool).await.unwrap();
            assert_eq!(stats.total_notes, 0);
        });
    }

    // 실제 Windows 11 Notepad 실행 환경 읽기 전용 실시간 연동 스모크 테스트
    #[test]
    fn test_live_notepad_tabstate_scan_readonly() {
        if let Some(local_state) = watcher::default_notepad_local_state_dir() {
            let tab_dir = local_state.join("TabState");
            let win_dir = local_state.join("WindowState");
            if tab_dir.is_dir() {
                tauri::async_runtime::block_on(async {
                    let pool = create_test_db().await;
                    let temp_store = std::env::temp_dir().join("nr2-live-smoke-store");
                    let _ = std::fs::remove_dir_all(&temp_store);

                    // 실제 Notepad가 실행 중인 상태에서 공유 읽기 플래그로 스캔 및 캡처 수행
                    let captured_count =
                        scan_and_reconcile(&tab_dir, &win_dir, &pool, &temp_store, None).await;
                    assert!(
                        captured_count > 0,
                        "실제 활성 메모장 탭이 스캔되어야 합니다"
                    );

                    let stats = get_storage_statistics(&pool).await.unwrap();
                    assert!(stats.total_notes > 0);
                    assert!(stats.total_snapshots > 0);

                    let _ = std::fs::remove_dir_all(&temp_store);
                });
            }
        }
    }

    // 실제 Windows 11 Notepad 탭 파싱 검증을 위한 수동/선택적 라이브 테스트
    #[test]
    #[ignore = "수동 라이브 검증용: 메모장에 'NR3-LIVE-PARSER-CHECK-001' 내용의 탭을 열어둔 후 실행"]
    fn test_live_notepad_tabstate_marker_validation_manual() {
        if let Some(local_state) = watcher::default_notepad_local_state_dir() {
            let tab_dir = local_state.join("TabState");
            let win_dir = local_state.join("WindowState");
            if tab_dir.is_dir() {
                tauri::async_runtime::block_on(async {
                    let pool = create_test_db().await;
                    let temp_store = std::env::temp_dir().join("nr3-marker-validation-store");
                    let _ = std::fs::remove_dir_all(&temp_store);

                    let _ = scan_and_reconcile(&tab_dir, &win_dir, &pool, &temp_store, None).await;

                    let notes =
                        repository::list_notes(&pool, &super::models::NoteFilter::default())
                            .await
                            .unwrap();
                    let mut found_marker = false;
                    for note in notes {
                        let snaps = repository::list_note_snapshots(&pool, &note.id)
                            .await
                            .unwrap();
                        for snap in snaps {
                            if let Some(ref text) = snap.recovered_text {
                                if text.contains("NR3-LIVE-PARSER-CHECK-001") {
                                    found_marker = true;
                                    assert_eq!(snap.parse_status, ParseStatus::Parsed);
                                    assert!(!snap.content_hash.is_empty());
                                    assert!(!note.source_id.is_empty());
                                    let blob_path =
                                        resolve_blob_path(&temp_store, &snap.raw_snapshot_path);
                                    assert!(blob_path.exists());
                                    break;
                                }
                            }
                        }
                        if found_marker {
                            break;
                        }
                    }

                    let _ = std::fs::remove_dir_all(&temp_store);
                    assert!(found_marker, "마커 'NR3-LIVE-PARSER-CHECK-001'가 포함된 메모장 탭이 감지 및 정상 파싱되어야 합니다.");
                });
            }
        }
    }
}
