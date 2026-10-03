use std::path::PathBuf;
use std::sync::atomic::Ordering;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, State};

use super::models::{
    NoteFilter, NoteSummaryDto, NotepadNote, NotepadRescueSettings, NotepadSnapshot,
    RescueStatusDto, StorageStatistics, UpdateRescueSettingsDto,
};
use super::repository;
use super::watcher;
use super::NotepadRescueState;
use crate::error::AppError;

/// SQLite DB 커넥션 풀을 획득합니다.
pub async fn open_db_pool(app: &AppHandle) -> Result<SqlitePool, AppError> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Io(format!("앱 설정 경로 조회 실패: {e}")))?;
    let db_file = config_dir.join("apidb.db");
    SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db_file)
                .read_only(false),
        )
        .await
        .map_err(|e| AppError::Database(format!("DB 연결 실패: {e}")))
}

/// 백엔드 감시 상태 및 스토리지 현황을 조회합니다.
#[tauri::command]
pub async fn rescue_get_status(
    app: AppHandle,
    state: State<'_, NotepadRescueState>,
) -> Result<RescueStatusDto, AppError> {
    let pool = open_db_pool(&app).await?;
    let settings = repository::get_settings(&pool).await?;
    let storage_stats = repository::get_storage_statistics(&pool).await?;

    let local_state = watcher::default_notepad_local_state_dir();
    let (status, message, local_state_path) = match &local_state {
        None => (
            "NOTEPAD_NOT_FOUND".to_string(),
            Some("Windows Notepad 패키지 로컬 상태 디렉터리를 찾을 수 없습니다".to_string()),
            None,
        ),
        Some(dir) => {
            let path_str = dir.to_string_lossy().to_string();
            let tab_dir = dir.join("TabState");
            if !settings.enabled || !settings.watch_enabled {
                ("DISABLED".to_string(), None, Some(path_str))
            } else if !tab_dir.is_dir() {
                (
                    "WATCH_PATH_MISSING".to_string(),
                    Some("TabState 디렉터리가 아직 생성되지 않았습니다".to_string()),
                    Some(path_str),
                )
            } else if state.is_watching.load(Ordering::Relaxed) {
                ("ACTIVE".to_string(), None, Some(path_str))
            } else {
                (
                    "WATCHER_ERROR".to_string(),
                    Some("감시 워커가 시작되지 않았거나 오류로 중지되었습니다".to_string()),
                    Some(path_str),
                )
            }
        }
    };

    let last_snapshot_at = storage_stats.newest_snapshot_at.clone();

    Ok(RescueStatusDto {
        enabled: settings.enabled,
        watch_enabled: settings.watch_enabled,
        is_watching: state.is_watching.load(Ordering::Relaxed),
        status,
        message,
        storage_stats,
        last_snapshot_at,
        local_state_path,
    })
}

/// 노트 목록을 요약 정보와 함께 조회합니다.
#[tauri::command]
pub async fn rescue_list_notes(
    app: AppHandle,
    category: Option<String>,
    query: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<NoteSummaryDto>, AppError> {
    let pool = open_db_pool(&app).await?;
    let is_deleted = match category.as_deref() {
        Some("active") => Some(false),
        Some("deleted") => Some(true),
        _ => None,
    };
    let filter = NoteFilter {
        is_deleted,
        query,
        limit,
        offset,
    };
    repository::list_notes_summary(&pool, &filter).await
}

/// 특정 노트의 상세 정보를 조회합니다.
#[tauri::command]
pub async fn rescue_get_note(
    app: AppHandle,
    note_id: String,
) -> Result<Option<NoteSummaryDto>, AppError> {
    let pool = open_db_pool(&app).await?;
    repository::get_note_summary(&pool, &note_id).await
}

/// 특정 노트에 연결된 스냅샷 목록을 최신순으로 조회합니다.
#[tauri::command]
pub async fn rescue_list_snapshots(
    app: AppHandle,
    note_id: String,
) -> Result<Vec<NotepadSnapshot>, AppError> {
    let pool = open_db_pool(&app).await?;
    repository::list_note_snapshots(&pool, &note_id).await
}

/// 특정 스냅샷의 전체 메타데이터 및 복구된 본문을 조회합니다.
#[tauri::command]
pub async fn rescue_get_snapshot(
    app: AppHandle,
    snapshot_id: String,
) -> Result<Option<NotepadSnapshot>, AppError> {
    let pool = open_db_pool(&app).await?;
    repository::get_snapshot(&pool, &snapshot_id).await
}

/// 현재 Notepad Rescue 설정을 조회합니다.
#[tauri::command]
pub async fn rescue_get_settings(app: AppHandle) -> Result<NotepadRescueSettings, AppError> {
    let pool = open_db_pool(&app).await?;
    repository::get_settings(&pool).await
}

/// Notepad Rescue 설정을 검증 후 갱신합니다.
#[tauri::command]
pub async fn rescue_update_settings(
    app: AppHandle,
    settings: UpdateRescueSettingsDto,
) -> Result<NotepadRescueSettings, AppError> {
    let pool = open_db_pool(&app).await?;
    let current = repository::get_settings(&pool).await?;

    let retention = settings.retention_days.unwrap_or(current.retention_days);
    if !(1..=3650).contains(&retention) {
        return Err(AppError::InvalidRequest(
            "보관 일수는 1일에서 3650일 사이여야 합니다".into(),
        ));
    }

    let deleted_retention = settings
        .deleted_retention_days
        .unwrap_or(current.deleted_retention_days);
    if !(1..=3650).contains(&deleted_retention) {
        return Err(AppError::InvalidRequest(
            "삭제 노트 보관 일수는 1일에서 3650일 사이여야 합니다".into(),
        ));
    }

    let max_storage = settings
        .max_storage_bytes
        .unwrap_or(current.max_storage_bytes);
    if !(10_000_000..=1_000_000_000_000).contains(&max_storage) {
        return Err(AppError::InvalidRequest(
            "최대 스토리지 용량은 10MB에서 1TB 사이여야 합니다".into(),
        ));
    }

    let new_settings = NotepadRescueSettings {
        enabled: settings.enabled.unwrap_or(current.enabled),
        watch_enabled: settings.watch_enabled.unwrap_or(current.watch_enabled),
        retention_days: retention,
        deleted_retention_days: deleted_retention,
        max_storage_bytes: max_storage,
        updated_at: super::pipeline::now_rfc3339(),
    };

    repository::update_settings(&pool, &new_settings).await?;

    if !new_settings.enabled || !new_settings.watch_enabled {
        super::shutdown(&app);
    } else {
        super::start_worker(&app);
    }

    Ok(new_settings)
}

/// 복구된 텍스트를 외부 TXT 파일로 안전하게 내보냅니다.
#[tauri::command]
pub async fn rescue_export_text(
    app: AppHandle,
    snapshot_id: String,
    destination_path: Option<String>,
) -> Result<String, AppError> {
    let pool = open_db_pool(&app).await?;
    let snapshot = repository::get_snapshot(&pool, &snapshot_id)
        .await?
        .ok_or_else(|| AppError::InvalidRequest("스냅샷을 찾을 수 없습니다".into()))?;

    let text = snapshot
        .recovered_text
        .ok_or_else(|| AppError::InvalidRequest("복구된 텍스트가 없습니다".into()))?;

    let target_file = if let Some(dest) = destination_path {
        PathBuf::from(dest)
    } else {
        let downloads_dir = app
            .path()
            .download_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("notepad_rescue_exports"));
        let _ = std::fs::create_dir_all(&downloads_dir);

        let note = repository::get_note(&pool, &snapshot.note_id)
            .await?
            .unwrap_or_else(|| NotepadNote {
                id: snapshot.note_id.clone(),
                source_id: "unknown".to_string(),
                display_title: "Untitled".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: String::new(),
                last_seen_at: String::new(),
                deleted_at: None,
                created_at: String::new(),
                updated_at: String::new(),
            });

        let sanitized_title: String = note
            .display_title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .take(30)
            .collect();
        let title = if sanitized_title.trim().is_empty() {
            "Untitled".to_string()
        } else {
            sanitized_title.trim().to_string()
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        downloads_dir.join(format!("{title}_{ts}.txt"))
    };

    if let Some(parent) = target_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    std::fs::write(&target_file, text.as_bytes())
        .map_err(|e| AppError::Io(format!("텍스트 내보내기 실패: {e}")))?;

    Ok(target_file.to_string_lossy().to_string())
}

/// 임시 잔여 파일 정리 및 최신 스토리지 통계를 반환합니다.
#[tauri::command]
pub async fn rescue_cleanup_stale(app: AppHandle) -> Result<StorageStatistics, AppError> {
    let pool = open_db_pool(&app).await?;
    if let Ok(store_root) = super::storage_root(&app) {
        let _ = super::snapshot::cleanup_orphaned_tmp(
            &store_root,
            std::time::Duration::from_secs(3600),
        );
    }
    repository::get_storage_statistics(&pool).await
}

/// 내보낸 텍스트 파일을 기본 편집기(메모장 등)로 안전하게 실행합니다.
#[tauri::command]
pub async fn rescue_open_exported_note(_app: AppHandle, path: String) -> Result<(), AppError> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(AppError::InvalidRequest("파일이 존재하지 않습니다".into()));
    }
    #[cfg(target_os = "windows")]
    {
        crate::process::hidden_command("cmd.exe")
            .arg("/c")
            .arg("start")
            .arg("")
            .arg(&path)
            .spawn()
            .map_err(|e| AppError::Io(format!("파일 열기 실패: {e}")))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notepad_rescue::models::{ParseStatus, SourceType};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn create_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE notepad_notes (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL UNIQUE,
                display_title TEXT NOT NULL,
                original_path TEXT,
                is_saved INTEGER NOT NULL DEFAULT 0,
                is_deleted INTEGER NOT NULL DEFAULT 0,
                first_seen_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                deleted_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE notepad_snapshots (
                id TEXT PRIMARY KEY,
                note_id TEXT NOT NULL REFERENCES notepad_notes(id) ON DELETE CASCADE,
                content_hash TEXT NOT NULL,
                raw_snapshot_path TEXT NOT NULL,
                recovered_text TEXT,
                source_type TEXT NOT NULL,
                source_filename TEXT NOT NULL,
                source_size INTEGER NOT NULL,
                parse_status TEXT NOT NULL,
                notepad_format_version INTEGER,
                captured_at TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE notepad_sessions (
                id TEXT PRIMARY KEY,
                source_filename TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                raw_snapshot_path TEXT NOT NULL,
                captured_at TEXT NOT NULL,
                parse_status TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE notepad_rescue_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO notepad_rescue_settings (key, value, updated_at) VALUES
            ('enabled', 'true', '2026-01-01T00:00:00Z'),
            ('watch_enabled', 'true', '2026-01-01T00:00:00Z'),
            ('retention_days', '30', '2026-01-01T00:00:00Z'),
            ('deleted_retention_days', '30', '2026-01-01T00:00:00Z'),
            ('max_storage_bytes', '1073741824', '2026-01-01T00:00:00Z');",
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[test]
    fn test_list_notes_summary_empty_and_populated() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let empty_list = repository::list_notes_summary(&pool, &NoteFilter::default())
                .await
                .unwrap();
            assert!(empty_list.is_empty());

            let note = NotepadNote {
                id: "note-1".to_string(),
                source_id: "src-1".to_string(),
                display_title: "My Note".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-01-01T10:00:00Z".to_string(),
                last_seen_at: "2026-01-01T10:05:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-01-01T10:00:00Z".to_string(),
                updated_at: "2026-01-01T10:05:00Z".to_string(),
            };
            repository::upsert_note(&pool, &note).await.unwrap();

            let snap = NotepadSnapshot {
                id: "snap-1".to_string(),
                note_id: "note-1".to_string(),
                content_hash: "hash-1".to_string(),
                raw_snapshot_path: "snapshots/ab/hash.bin".to_string(),
                recovered_text: Some("Recovered line 1\nRecovered line 2".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab.bin".to_string(),
                source_size: 100,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-01-01T10:05:00Z".to_string(),
                created_at: "2026-01-01T10:05:00Z".to_string(),
            };
            repository::insert_snapshot_metadata(&pool, &snap)
                .await
                .unwrap();

            let list = repository::list_notes_summary(&pool, &NoteFilter::default())
                .await
                .unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].id, "note-1");
            assert_eq!(list[0].version_count, 1);
            assert_eq!(list[0].display_title, "My Note");
            assert!(list[0]
                .preview_text
                .as_ref()
                .unwrap()
                .contains("Recovered line 1"));
        });
    }

    #[test]
    fn test_settings_validation_limits() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let valid = NotepadRescueSettings {
                enabled: true,
                watch_enabled: true,
                retention_days: 60,
                deleted_retention_days: 15,
                max_storage_bytes: 500_000_000,
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            };
            repository::update_settings(&pool, &valid).await.unwrap();
            let loaded = repository::get_settings(&pool).await.unwrap();
            assert_eq!(loaded.retention_days, 60);
            assert_eq!(loaded.deleted_retention_days, 15);
            assert_eq!(loaded.max_storage_bytes, 500_000_000);
        });
    }
}
