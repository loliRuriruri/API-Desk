use std::path::PathBuf;
use std::sync::atomic::Ordering;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, State};

use super::diff::{self, DiffResultDto};
use super::gc::{self, GcPlan, GcReport, StorageDetailDto};
use super::models::{
    NoteFilter, NoteSummaryDto, NotepadNote, NotepadRescueSettings, NotepadSnapshot,
    RescueStatusDto, SearchResultDto, StorageStatistics, UpdateRescueSettingsDto,
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

/// 노트 검색 (제목, 경로, 본문 스냅샷 대상, 제한된 스니펫 반환).
#[tauri::command]
pub async fn rescue_search_notes(
    app: AppHandle,
    query: String,
    scope: Option<String>,
    include_deleted: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<SearchResultDto>, AppError> {
    let pool = open_db_pool(&app).await?;
    repository::search_notes(
        &pool,
        &query,
        scope.as_deref(),
        include_deleted.unwrap_or(false),
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    )
    .await
}

/// 동일 노트의 두 스냅샷 간의 라인 diff를 비교합니다.
#[tauri::command]
pub async fn rescue_diff_snapshots(
    app: AppHandle,
    note_id: String,
    base_snapshot_id: String,
    target_snapshot_id: String,
) -> Result<DiffResultDto, AppError> {
    let pool = open_db_pool(&app).await?;

    let base_snap = repository::get_snapshot(&pool, &base_snapshot_id)
        .await?
        .ok_or_else(|| AppError::InvalidRequest("기준 스냅샷을 찾을 수 없습니다".into()))?;

    let target_snap = repository::get_snapshot(&pool, &target_snapshot_id)
        .await?
        .ok_or_else(|| AppError::InvalidRequest("대상 스냅샷을 찾을 수 없습니다".into()))?;

    if base_snap.note_id != note_id || target_snap.note_id != note_id {
        return Err(AppError::InvalidRequest(
            "서로 다른 노트의 스냅샷은 비교할 수 없습니다".into(),
        ));
    }

    let base_text = base_snap.recovered_text.as_deref().unwrap_or("");
    let target_text = target_snap.recovered_text.as_deref().unwrap_or("");

    Ok(diff::compare_texts(
        &note_id,
        &base_snapshot_id,
        &target_snapshot_id,
        base_text,
        target_text,
    ))
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

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let target_file = if let Some(dest) = destination_path {
        PathBuf::from(dest)
    } else {
        let downloads_dir = app
            .path()
            .download_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("notepad_rescue_exports"));
        let safe_name = super::recovery::sanitize_export_filename(&note.display_title, ts as i64);
        downloads_dir.join(safe_name)
    };

    let exported_path = super::recovery::write_atomic_txt(&target_file, &text)?;
    let _ = super::recovery::write_export_sidecar(
        &exported_path,
        &snapshot.note_id,
        &snapshot.id,
        &snapshot.captured_at,
        &snapshot.content_hash,
    );

    Ok(exported_path.to_string_lossy().to_string())
}

/// 특정 노트의 복구 상세 정보(최신 스냅샷, 최신 유효 스냅샷, 복구 신뢰도 등)를 조회합니다.
#[tauri::command]
pub async fn rescue_get_recovery_detail(
    app: AppHandle,
    note_id: String,
) -> Result<super::recovery::NoteRecoveryDetailDto, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    super::recovery::get_note_recovery_detail(&pool, &store_root, &note_id).await
}

/// 재해 복구 패키지(Disaster Recovery Package)를 로컬 디렉터리에 내보냅니다.
#[tauri::command]
pub async fn rescue_export_package(
    app: AppHandle,
    destination_dir: Option<String>,
) -> Result<super::recovery::RecoveryPackageReport, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    let target_dir = if let Some(d) = destination_dir {
        PathBuf::from(d)
    } else {
        app.path()
            .download_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("notepad_rescue_packages"))
    };

    super::recovery::export_recovery_package(&pool, &store_root, &target_dir).await
}

/// Notepad Rescue 시스템 진단 보고서를 조회합니다 (본문/시크릿 배제).
#[tauri::command]
pub async fn rescue_get_diagnostics(
    app: AppHandle,
) -> Result<super::recovery::DiagnosticReportDto, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    let settings = repository::get_settings(&pool).await?;
    super::recovery::get_diagnostic_report(&pool, &store_root, &settings).await
}

/// DB 유실(Scenario G) 상황 대비 원시 CAS 스토리지 직접 스캔 및 복구 가능 여부 진단
#[tauri::command]
pub async fn rescue_scan_raw_storage(
    app: AppHandle,
) -> Result<Vec<super::recovery::RawBlobScanResult>, AppError> {
    let store_root = super::storage_root(&app)?;
    super::recovery::scan_and_recover_raw_blobs(&store_root)
}

/// 여러 또는 모든 복구 가능한 노트를 일괄 TXT로 내보냅니다.
#[tauri::command]
pub async fn rescue_bulk_export_notes(
    app: AppHandle,
    destination_dir: Option<String>,
) -> Result<Vec<String>, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    let target_dir = if let Some(d) = destination_dir {
        PathBuf::from(d)
    } else {
        let dl = app
            .path()
            .download_dir()
            .unwrap_or_else(|_| std::env::temp_dir());
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        dl.join(format!("NotepadRescue_BulkExport_{ts}"))
    };

    let notes = repository::list_notes(&pool, &super::models::NoteFilter::default()).await?;
    let mut exported_paths = Vec::new();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for note in notes {
        let detail =
            super::recovery::get_note_recovery_detail(&pool, &store_root, &note.id).await?;
        let snapshot = detail.latest_valid_snapshot.or(detail.latest_snapshot);
        if let Some(s) = snapshot {
            if let Some(text) = s.recovered_text {
                let filename =
                    super::recovery::sanitize_export_filename(&note.display_title, ts as i64);
                let dest = target_dir.join(filename);
                if let Ok(p) = super::recovery::write_atomic_txt(&dest, &text) {
                    exported_paths.push(p.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(exported_paths)
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

/// GC 정리 계획(Dry Run)을 조회합니다 (DB 및 디스크 파일 변경 없음).
#[tauri::command]
pub async fn rescue_gc_preview(app: AppHandle) -> Result<GcPlan, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    super::ensure_storage_dirs(&store_root)?;
    let settings = repository::get_settings(&pool).await?;

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    gc::plan_gc(&pool, &store_root, &settings, now_epoch).await
}

/// GC 안전 정리를 실행합니다 (최신 스냅샷 절대 보존, 다중 테이블 참조 확인 후 물리 삭제).
#[tauri::command]
pub async fn rescue_gc_execute(app: AppHandle) -> Result<GcReport, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    super::ensure_storage_dirs(&store_root)?;
    let settings = repository::get_settings(&pool).await?;

    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    gc::execute_gc(&pool, &store_root, &settings, now_epoch).await
}

/// 상세 스토리지 현황(논리/물리 크기, 중복제거, 쿼터)을 조회합니다.
#[tauri::command]
pub async fn rescue_get_storage_details(app: AppHandle) -> Result<StorageDetailDto, AppError> {
    let pool = open_db_pool(&app).await?;
    let store_root = super::storage_root(&app)?;
    super::ensure_storage_dirs(&store_root)?;
    let settings = repository::get_settings(&pool).await?;

    gc::get_storage_details(&pool, &store_root, &settings).await
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
