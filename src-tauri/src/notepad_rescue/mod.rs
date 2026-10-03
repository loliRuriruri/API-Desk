pub mod models;
pub mod repository;

use std::path::{Path, PathBuf};
use tauri::Manager;

use crate::error::AppError;

/// 스냅샷의 상대 파일 경로를 생성합니다 (e.g. "snapshots/<content_hash>.bin").
pub fn relative_snapshot_path(content_hash: &str) -> String {
    format!("snapshots/{content_hash}.bin")
}

/// 스토리지 루트와 상대 경로를 결합하여 실제 절대 경로를 반환합니다.
pub fn resolve_snapshot_path(storage_root: &Path, relative_path: &str) -> PathBuf {
    storage_root.join(relative_path)
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

#[cfg(test)]
mod tests {
    use super::models::*;
    use super::repository::*;
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

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

    #[test]
    fn test_relative_and_resolved_paths() {
        let rel = relative_snapshot_path("abc123sha256");
        assert_eq!(rel, "snapshots/abc123sha256.bin");

        let root = PathBuf::from("C:\\AppData\\notepad_rescue");
        let resolved = resolve_snapshot_path(&root, &rel);
        assert_eq!(
            resolved,
            PathBuf::from("C:\\AppData\\notepad_rescue\\snapshots\\abc123sha256.bin")
        );
    }

    // 1. Fresh DB 마이그레이션 적용 및 테이블/인덱스 검증
    #[test]
    fn test_migration_fresh_db() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'notepad_%' ORDER BY name",
            )
            .fetch_all(&pool)
            .await
            .unwrap();

            assert_eq!(
                tables,
                vec![
                    "notepad_notes",
                    "notepad_rescue_settings",
                    "notepad_sessions",
                    "notepad_snapshots"
                ]
            );

            let indexes: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_notepad_%' ORDER BY name",
            )
            .fetch_all(&pool)
            .await
            .unwrap();

            assert!(indexes.contains(&"idx_notepad_notes_source_id".to_string()));
            assert!(indexes.contains(&"idx_notepad_notes_is_deleted".to_string()));
            assert!(indexes.contains(&"idx_notepad_notes_last_seen_at".to_string()));
            assert!(indexes.contains(&"idx_notepad_snapshots_note_id".to_string()));
            assert!(indexes.contains(&"idx_notepad_snapshots_content_hash".to_string()));
            assert!(indexes.contains(&"idx_notepad_snapshots_captured_at".to_string()));
            assert!(indexes.contains(&"idx_notepad_sessions_content_hash".to_string()));
            assert!(indexes.contains(&"idx_notepad_sessions_captured_at".to_string()));
        });
    }

    // 2. 기존 DB (Migration 1~4)에서 Migration 5로 업그레이드
    #[test]
    fn test_migration_upgrade() {
        tauri::async_runtime::block_on(async {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            let all_migrations = crate::migrations::migrations();

            // v1 ~ v4 먼저 적용
            for migration in &all_migrations[0..4] {
                sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();
            }

            // 기존 테이블에 샘플 데이터 적재
            sqlx::query(
                "INSERT INTO providers (id, name, display_name, created_at, updated_at) VALUES ('p1', 'openai', 'OpenAI', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES ('theme', 'dark', '2026-01-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            // v5 (Notepad Rescue) 적용
            let v5 = &all_migrations[4];
            assert_eq!(v5.version, 5);
            sqlx::raw_sql(v5.sql).execute(&pool).await.unwrap();

            // 기존 데이터 보존 검증
            let provider_name: String =
                sqlx::query_scalar("SELECT name FROM providers WHERE id = 'p1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(provider_name, "openai");

            let theme: String =
                sqlx::query_scalar("SELECT value FROM settings WHERE key = 'theme'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(theme, "dark");

            // 새 테이블 사용 가능 검증
            let settings = get_settings(&pool).await.unwrap();
            assert!(settings.enabled);
            assert_eq!(settings.retention_days, 30);
        });
    }

    // 3. Default settings 초기값 및 업데이트 검증
    #[test]
    fn test_default_settings_init() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let settings = get_settings(&pool).await.unwrap();
            assert!(settings.enabled);
            assert!(settings.watch_enabled);
            assert_eq!(settings.retention_days, 30);
            assert_eq!(settings.deleted_retention_days, 30);
            assert_eq!(settings.max_storage_bytes, 1_073_741_824);

            let updated = NotepadRescueSettings {
                enabled: false,
                watch_enabled: false,
                retention_days: 60,
                deleted_retention_days: 14,
                max_storage_bytes: 2_147_483_648,
                updated_at: "2026-10-03T12:00:00Z".to_string(),
            };

            update_settings(&pool, &updated).await.unwrap();

            let fetched = get_settings(&pool).await.unwrap();
            assert!(!fetched.enabled);
            assert!(!fetched.watch_enabled);
            assert_eq!(fetched.retention_days, 60);
            assert_eq!(fetched.deleted_retention_days, 14);
            assert_eq!(fetched.max_storage_bytes, 2_147_483_648);
        });
    }

    // 4. Unsaved note (미저장 탭) 삽입 및 조회
    #[test]
    fn test_unsaved_note_insert() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note = NotepadNote {
                id: "note-unsaved-1".to_string(),
                source_id: "tab-guid-1111".to_string(),
                display_title: "제목 없음 1".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T10:00:00Z".to_string(),
                last_seen_at: "2026-10-03T10:05:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T10:00:00Z".to_string(),
                updated_at: "2026-10-03T10:05:00Z".to_string(),
            };

            let saved = upsert_note(&pool, &note).await.unwrap();
            assert_eq!(saved.id, "note-unsaved-1");
            assert_eq!(saved.source_id, "tab-guid-1111");
            assert_eq!(saved.display_title, "제목 없음 1");
            assert_eq!(saved.original_path, None);
            assert!(!saved.is_saved);
            assert!(!saved.is_deleted);

            let by_id = get_note(&pool, "note-unsaved-1").await.unwrap().unwrap();
            assert_eq!(by_id.source_id, "tab-guid-1111");
            assert!(by_id.original_path.is_none());

            let by_source = get_note_by_source_id(&pool, "tab-guid-1111")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(by_source.id, "note-unsaved-1");
        });
    }

    // 5. Saved note (파일 경로가 있는 저장된 탭) 삽입 및 조회
    #[test]
    fn test_saved_note_insert() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note = NotepadNote {
                id: "note-saved-1".to_string(),
                source_id: "tab-guid-2222".to_string(),
                display_title: "todo.txt".to_string(),
                original_path: Some("C:\\Users\\test\\Documents\\todo.txt".to_string()),
                is_saved: true,
                is_deleted: false,
                first_seen_at: "2026-10-03T09:00:00Z".to_string(),
                last_seen_at: "2026-10-03T09:30:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T09:00:00Z".to_string(),
                updated_at: "2026-10-03T09:30:00Z".to_string(),
            };

            let saved = upsert_note(&pool, &note).await.unwrap();
            assert_eq!(saved.id, "note-saved-1");
            assert_eq!(
                saved.original_path.as_deref(),
                Some("C:\\Users\\test\\Documents\\todo.txt")
            );
            assert!(saved.is_saved);

            let fetched = get_note(&pool, "note-saved-1").await.unwrap().unwrap();
            assert_eq!(
                fetched.original_path.as_deref(),
                Some("C:\\Users\\test\\Documents\\todo.txt")
            );
            assert!(fetched.is_saved);
        });
    }

    // 6. Note upsert: 동일 source_id 시 기존 id 및 first_seen_at 보존, 제목/시각 갱신
    #[test]
    fn test_note_upsert_preserves_id() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let initial = NotepadNote {
                id: "canonical-id-001".to_string(),
                source_id: "persistent-tab-uuid".to_string(),
                display_title: "초기 제목".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-01T00:00:00Z".to_string(),
                last_seen_at: "2026-10-01T00:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-01T00:00:00Z".to_string(),
                updated_at: "2026-10-01T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &initial).await.unwrap();

            // 다른 id를 부여하려 해도 동일 source_id인 경우 원래 id 보존
            let updated = NotepadNote {
                id: "different-id-attempt".to_string(),
                source_id: "persistent-tab-uuid".to_string(),
                display_title: "수정된 제목.txt".to_string(),
                original_path: Some("D:\\memo.txt".to_string()),
                is_saved: true,
                is_deleted: false,
                first_seen_at: "2026-10-02T00:00:00Z".to_string(),
                last_seen_at: "2026-10-02T12:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-02T00:00:00Z".to_string(),
                updated_at: "2026-10-02T12:00:00Z".to_string(),
            };
            let outcome = upsert_note(&pool, &updated).await.unwrap();

            assert_eq!(outcome.id, "canonical-id-001");
            assert_eq!(outcome.first_seen_at, "2026-10-01T00:00:00Z");
            assert_eq!(outcome.display_title, "수정된 제목.txt");
            assert_eq!(outcome.original_path.as_deref(), Some("D:\\memo.txt"));
            assert!(outcome.is_saved);
            assert_eq!(outcome.last_seen_at, "2026-10-02T12:00:00Z");
        });
    }

    // 7. Mark deleted (소프트 삭제 플래그 및 삭제 시각 기록, 멱등성)
    #[test]
    fn test_mark_note_deleted() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note = NotepadNote {
                id: "del-note-1".to_string(),
                source_id: "tab-to-delete".to_string(),
                display_title: "닫힐 탭".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T10:00:00Z".to_string(),
                last_seen_at: "2026-10-03T10:10:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T10:00:00Z".to_string(),
                updated_at: "2026-10-03T10:10:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            let affected = mark_note_deleted(&pool, "tab-to-delete", "2026-10-03T11:00:00Z")
                .await
                .unwrap();
            assert!(affected);

            let fetched = get_note(&pool, "del-note-1").await.unwrap().unwrap();
            assert!(fetched.is_deleted);
            assert_eq!(fetched.deleted_at.as_deref(), Some("2026-10-03T11:00:00Z"));

            // 이미 삭제된 노트에 대해 재호출 시 false 반환
            let affected_again = mark_note_deleted(&pool, "tab-to-delete", "2026-10-03T11:05:00Z")
                .await
                .unwrap();
            assert!(!affected_again);
        });
    }

    // 8. Snapshot metadata 삽입 및 단일 노트 히스토리 조회
    #[test]
    fn test_snapshot_metadata_insert() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note = NotepadNote {
                id: "note-snap-1".to_string(),
                source_id: "tab-snap-1".to_string(),
                display_title: "스냅샷 노트".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T08:00:00Z".to_string(),
                last_seen_at: "2026-10-03T08:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T08:00:00Z".to_string(),
                updated_at: "2026-10-03T08:00:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            let snap1 = NotepadSnapshot {
                id: "snap-1".to_string(),
                note_id: "note-snap-1".to_string(),
                content_hash: "hash-rev-1".to_string(),
                raw_snapshot_path: "snapshots/hash-rev-1.bin".to_string(),
                recovered_text: Some("첫 번째 텍스트 내용".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab-snap-1.bin".to_string(),
                source_size: 150,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-10-03T08:00:00Z".to_string(),
                created_at: "2026-10-03T08:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap1).await.unwrap();

            let snap2 = NotepadSnapshot {
                id: "snap-2".to_string(),
                note_id: "note-snap-1".to_string(),
                content_hash: "hash-rev-2".to_string(),
                raw_snapshot_path: "snapshots/hash-rev-2.bin".to_string(),
                recovered_text: Some("수정된 텍스트 내용".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab-snap-1.bin".to_string(),
                source_size: 210,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-10-03T08:15:00Z".to_string(),
                created_at: "2026-10-03T08:15:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap2).await.unwrap();

            let history = list_note_snapshots(&pool, "note-snap-1").await.unwrap();
            assert_eq!(history.len(), 2);
            assert_eq!(history[0].id, "snap-2"); // captured_at DESC
            assert_eq!(history[1].id, "snap-1");

            let latest = get_latest_snapshot_for_note(&pool, "note-snap-1")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(latest.id, "snap-2");
            assert_eq!(latest.content_hash, "hash-rev-2");
            assert_eq!(latest.recovered_text.as_deref(), Some("수정된 텍스트 내용"));
        });
    }

    // 9. Duplicate content scenario: 다른 노드가 동일 content_hash를 가질 때의 정상 수용
    #[test]
    fn test_duplicate_content_snapshots() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note1 = NotepadNote {
                id: "note-dup-1".to_string(),
                source_id: "tab-dup-1".to_string(),
                display_title: "노트 1".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T08:00:00Z".to_string(),
                last_seen_at: "2026-10-03T08:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T08:00:00Z".to_string(),
                updated_at: "2026-10-03T08:00:00Z".to_string(),
            };
            let note2 = NotepadNote {
                id: "note-dup-2".to_string(),
                source_id: "tab-dup-2".to_string(),
                display_title: "노트 2".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T08:05:00Z".to_string(),
                last_seen_at: "2026-10-03T08:05:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T08:05:00Z".to_string(),
                updated_at: "2026-10-03T08:05:00Z".to_string(),
            };
            upsert_note(&pool, &note1).await.unwrap();
            upsert_note(&pool, &note2).await.unwrap();

            let shared_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
            let snap1 = NotepadSnapshot {
                id: "snap-dup-1".to_string(),
                note_id: "note-dup-1".to_string(),
                content_hash: shared_hash.to_string(),
                raw_snapshot_path: format!("snapshots/{shared_hash}.bin"),
                recovered_text: Some("동일 텍스트".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab-1.bin".to_string(),
                source_size: 50,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-10-03T08:00:00Z".to_string(),
                created_at: "2026-10-03T08:00:00Z".to_string(),
            };
            let snap2 = NotepadSnapshot {
                id: "snap-dup-2".to_string(),
                note_id: "note-dup-2".to_string(),
                content_hash: shared_hash.to_string(),
                raw_snapshot_path: format!("snapshots/{shared_hash}.bin"),
                recovered_text: Some("동일 텍스트".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab-2.bin".to_string(),
                source_size: 50,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-10-03T08:05:00Z".to_string(),
                created_at: "2026-10-03T08:05:00Z".to_string(),
            };

            // 두 스냅샷 모두 정상 삽입되어야 함 (content_hash는 테이블 레벨에서 유니크하지 않음)
            insert_snapshot_metadata(&pool, &snap1).await.unwrap();
            insert_snapshot_metadata(&pool, &snap2).await.unwrap();

            let list1 = list_note_snapshots(&pool, "note-dup-1").await.unwrap();
            let list2 = list_note_snapshots(&pool, "note-dup-2").await.unwrap();
            assert_eq!(list1.len(), 1);
            assert_eq!(list2.len(), 1);
            assert_eq!(list1[0].content_hash, list2[0].content_hash);
        });
    }

    // 10. Session metadata 삽입 검증
    #[test]
    fn test_session_metadata_insert() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let session = NotepadSession {
                id: "session-1".to_string(),
                source_filename: "window-guid.bin".to_string(),
                content_hash: "hash-win-1".to_string(),
                raw_snapshot_path: "snapshots/hash-win-1.bin".to_string(),
                captured_at: "2026-10-03T07:00:00Z".to_string(),
                parse_status: ParseStatus::Pending,
                created_at: "2026-10-03T07:00:00Z".to_string(),
            };

            insert_session_snapshot(&pool, &session).await.unwrap();

            let stats = get_storage_statistics(&pool).await.unwrap();
            assert_eq!(stats.total_sessions, 1);
        });
    }

    // 11. Storage statistics 집계 검증
    #[test]
    fn test_storage_statistics() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            // 노트 3개: 활성 2개, 삭제 1개
            for i in 1..=3 {
                let note = NotepadNote {
                    id: format!("stat-note-{i}"),
                    source_id: format!("stat-source-{i}"),
                    display_title: format!("노트 {i}"),
                    original_path: None,
                    is_saved: false,
                    is_deleted: i == 3,
                    first_seen_at: format!("2026-10-03T0{i}:00:00Z"),
                    last_seen_at: format!("2026-10-03T0{i}:30:00Z"),
                    deleted_at: if i == 3 {
                        Some("2026-10-03T03:30:00Z".to_string())
                    } else {
                        None
                    },
                    created_at: format!("2026-10-03T0{i}:00:00Z"),
                    updated_at: format!("2026-10-03T0{i}:30:00Z"),
                };
                upsert_note(&pool, &note).await.unwrap();
            }

            // 스냅샷 3개: 크기 100, 200, 300 (합계 600)
            let sizes = [100i64, 200i64, 300i64];
            for (idx, size) in sizes.iter().enumerate() {
                let snap = NotepadSnapshot {
                    id: format!("stat-snap-{idx}"),
                    note_id: "stat-note-1".to_string(),
                    content_hash: format!("stat-hash-{idx}"),
                    raw_snapshot_path: format!("snapshots/stat-hash-{idx}.bin"),
                    recovered_text: Some("내용".to_string()),
                    source_type: SourceType::TabState,
                    source_filename: "tab.bin".to_string(),
                    source_size: *size,
                    parse_status: ParseStatus::Parsed,
                    notepad_format_version: Some(1),
                    captured_at: format!("2026-10-03T0{}:00:00Z", idx + 1),
                    created_at: format!("2026-10-03T0{}:00:00Z", idx + 1),
                };
                insert_snapshot_metadata(&pool, &snap).await.unwrap();
            }

            // 세션 2개
            for i in 1..=2 {
                let session = NotepadSession {
                    id: format!("stat-session-{i}"),
                    source_filename: format!("window-{i}.bin"),
                    content_hash: format!("win-hash-{i}"),
                    raw_snapshot_path: format!("snapshots/win-hash-{i}.bin"),
                    captured_at: format!("2026-10-03T0{i}:00:00Z"),
                    parse_status: ParseStatus::Pending,
                    created_at: format!("2026-10-03T0{i}:00:00Z"),
                };
                insert_session_snapshot(&pool, &session).await.unwrap();
            }

            let stats = get_storage_statistics(&pool).await.unwrap();
            assert_eq!(stats.total_notes, 3);
            assert_eq!(stats.active_notes, 2);
            assert_eq!(stats.deleted_notes, 1);
            assert_eq!(stats.total_snapshots, 3);
            assert_eq!(stats.total_sessions, 2);
            assert_eq!(stats.total_snapshot_bytes, 600);
            assert_eq!(
                stats.oldest_snapshot_at.as_deref(),
                Some("2026-10-03T01:00:00Z")
            );
            assert_eq!(
                stats.newest_snapshot_at.as_deref(),
                Some("2026-10-03T03:00:00Z")
            );
        });
    }

    // 12. 외래 키 캐스케이드 삭제 검증 (노트 삭제 시 연계 스냅샷 자동 삭제)
    #[test]
    fn test_foreign_key_cascade_deletion() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let note = NotepadNote {
                id: "cascade-note-1".to_string(),
                source_id: "cascade-source-1".to_string(),
                display_title: "캐스케이드 테스트".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T10:00:00Z".to_string(),
                last_seen_at: "2026-10-03T10:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T10:00:00Z".to_string(),
                updated_at: "2026-10-03T10:00:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            for i in 1..=2 {
                let snap = NotepadSnapshot {
                    id: format!("cascade-snap-{i}"),
                    note_id: "cascade-note-1".to_string(),
                    content_hash: format!("cascade-hash-{i}"),
                    raw_snapshot_path: format!("snapshots/cascade-hash-{i}.bin"),
                    recovered_text: Some("내용".to_string()),
                    source_type: SourceType::TabState,
                    source_filename: "tab.bin".to_string(),
                    source_size: 100,
                    parse_status: ParseStatus::Parsed,
                    notepad_format_version: Some(1),
                    captured_at: format!("2026-10-03T10:0{i}:00Z"),
                    created_at: format!("2026-10-03T10:0{i}:00Z"),
                };
                insert_snapshot_metadata(&pool, &snap).await.unwrap();
            }

            let initial_snaps = list_note_snapshots(&pool, "cascade-note-1").await.unwrap();
            assert_eq!(initial_snaps.len(), 2);

            // notepad_notes에서 부모 row 물리 삭제
            sqlx::query("DELETE FROM notepad_notes WHERE id = 'cascade-note-1'")
                .execute(&pool)
                .await
                .unwrap();

            // notepad_snapshots에서 연계 row도 CASCADE 삭제되었는지 확인
            let remaining_snaps = list_note_snapshots(&pool, "cascade-note-1").await.unwrap();
            assert_eq!(remaining_snaps.len(), 0);

            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM notepad_snapshots WHERE note_id = 'cascade-note-1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 0);
        });
    }

    // 13. API Desk 기존 테이블 및 기능 회귀 검증
    #[test]
    fn test_existing_apidesk_tables_regression() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            // 1) providers & accounts & credentials (기존 마이그레이션 1~2)
            sqlx::query(
                "INSERT INTO providers (id, name, display_name, created_at, updated_at)
                 VALUES ('p-reg-1', 'anthropic', 'Anthropic', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO accounts (id, provider_id, name, created_at, updated_at)
                 VALUES ('acc-reg-1', 'p-reg-1', 'Main Account', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO credentials (id, account_id, name, secret_id, created_at, updated_at)
                 VALUES ('cred-reg-1', 'acc-reg-1', 'API Key', 'sec-1', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            // 2) usage_snapshots (마이그레이션 3)
            sqlx::query(
                "INSERT INTO usage_snapshots (id, credential_id, adapter, status, summary, fetched_at)
                 VALUES ('us-1', 'cred-reg-1', 'anthropic', 'ok', '사용 $10', '2026-10-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            // 3) monitor_snapshots (마이그레이션 4)
            sqlx::query(
                "INSERT INTO monitor_snapshots (id, monitor, status, summary, details_json, fetched_at)
                 VALUES ('ms-1', 'antigravity', 'ok', '73%', '{}', '2026-10-01T00:00:00Z')"
            )
            .execute(&pool)
            .await
            .unwrap();

            // 4) settings
            sqlx::query(
                "INSERT INTO settings (key, value, updated_at)
                 VALUES ('lang', 'ko', '2026-10-01T00:00:00Z')",
            )
            .execute(&pool)
            .await
            .unwrap();

            // 5) 동시에 notepad_notes 추가
            let note = NotepadNote {
                id: "reg-note-1".to_string(),
                source_id: "reg-tab-1".to_string(),
                display_title: "회귀 테스트 노트".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-10-03T10:00:00Z".to_string(),
                last_seen_at: "2026-10-03T10:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T10:00:00Z".to_string(),
                updated_at: "2026-10-03T10:00:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            // 기존 테이블 데이터 검증
            let cred_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credentials")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(cred_count, 1);

            let usage_summary: String =
                sqlx::query_scalar("SELECT summary FROM usage_snapshots WHERE id = 'us-1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(usage_summary, "사용 $10");

            let monitor_summary: String =
                sqlx::query_scalar("SELECT summary FROM monitor_snapshots WHERE id = 'ms-1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(monitor_summary, "73%");

            // 신규 노트 검증
            let note_title: String = sqlx::query_scalar(
                "SELECT display_title FROM notepad_notes WHERE id = 'reg-note-1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(note_title, "회귀 테스트 노트");
        });
    }

    // 14. list_notes 검색 필터링 검증
    #[test]
    fn test_list_notes_filtering() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;

            let n1 = NotepadNote {
                id: "filter-1".to_string(),
                source_id: "tab-f1".to_string(),
                display_title: "프로젝트 계획서".to_string(),
                original_path: Some("C:\\docs\\plan.txt".to_string()),
                is_saved: true,
                is_deleted: false,
                first_seen_at: "2026-10-03T01:00:00Z".to_string(),
                last_seen_at: "2026-10-03T01:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-10-03T01:00:00Z".to_string(),
                updated_at: "2026-10-03T01:00:00Z".to_string(),
            };
            let n2 = NotepadNote {
                id: "filter-2".to_string(),
                source_id: "tab-f2".to_string(),
                display_title: "장보기 목록".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: true,
                first_seen_at: "2026-10-03T02:00:00Z".to_string(),
                last_seen_at: "2026-10-03T02:00:00Z".to_string(),
                deleted_at: Some("2026-10-03T02:10:00Z".to_string()),
                created_at: "2026-10-03T02:00:00Z".to_string(),
                updated_at: "2026-10-03T02:10:00Z".to_string(),
            };
            upsert_note(&pool, &n1).await.unwrap();
            upsert_note(&pool, &n2).await.unwrap();

            // 스냅샷에 검색 텍스트 저장
            let snap = NotepadSnapshot {
                id: "snap-f1".to_string(),
                note_id: "filter-1".to_string(),
                content_hash: "hash-f1".to_string(),
                raw_snapshot_path: "snapshots/hash-f1.bin".to_string(),
                recovered_text: Some("기밀 마일스톤 데이터".to_string()),
                source_type: SourceType::TabState,
                source_filename: "tab-f1.bin".to_string(),
                source_size: 50,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-10-03T01:00:00Z".to_string(),
                created_at: "2026-10-03T01:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap).await.unwrap();

            // 활성 노트만 조회
            let active = list_notes(
                &pool,
                &NoteFilter {
                    is_deleted: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(active.len(), 1);
            assert_eq!(active[0].id, "filter-1");

            // 삭제된 노트만 조회
            let deleted = list_notes(
                &pool,
                &NoteFilter {
                    is_deleted: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(deleted.len(), 1);
            assert_eq!(deleted[0].id, "filter-2");

            // 내용 검색 (스냅샷 recovered_text 검색)
            let searched = list_notes(
                &pool,
                &NoteFilter {
                    query: Some("마일스톤".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(searched.len(), 1);
            assert_eq!(searched[0].id, "filter-1");
        });
    }
}
