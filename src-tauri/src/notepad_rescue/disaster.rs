#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::notepad_rescue::gc::{execute_gc, plan_gc};
    use crate::notepad_rescue::models::*;
    use crate::notepad_rescue::parser::{generate_synthetic_saved, generate_synthetic_unsaved};
    use crate::notepad_rescue::pipeline::{capture_state_file, handle_file_removed};
    use crate::notepad_rescue::recovery::*;
    use crate::notepad_rescue::repository;
    use crate::notepad_rescue::snapshot;
    use crate::notepad_rescue::watcher::scan_and_reconcile;

    async fn create_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
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

    fn create_test_env(test_name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("nr5-disaster-{test_name}"));
        let _ = fs::remove_dir_all(&base);
        let source_dir = base.join("source_tabs");
        let storage_root = base.join("storage_root");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&storage_root).unwrap();
        (source_dir, storage_root)
    }

    fn cleanup_test_env(test_name: &str) {
        let base = std::env::temp_dir().join(format!("nr5-disaster-{test_name}"));
        let _ = fs::remove_dir_all(&base);
    }

    // =========================================================================
    // 25. Disaster Scenario A: Single TabState Deletion
    // =========================================================================
    #[test]
    fn test_disaster_25_single_tabstate_disappears() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("25");

            let tab_file = source_dir.join("test-25-guid.bin");
            let data1 = generate_synthetic_unsaved("Version 1 content", false);
            fs::write(&tab_file, &data1).unwrap();

            // v1 캡처
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            // v2 변경 및 캡처
            let data2 = generate_synthetic_unsaved("Version 2 content updated", false);
            fs::write(&tab_file, &data2).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            let note = repository::get_note_by_source_id(&pool, "test-25-guid")
                .await
                .unwrap()
                .unwrap();
            let snaps = repository::list_note_snapshots(&pool, &note.id)
                .await
                .unwrap();
            assert_eq!(snaps.len(), 2);

            // 시뮬레이션: 원본 TabState 파일 돌연 삭제
            fs::remove_file(&tab_file).unwrap();
            handle_file_removed(&tab_file, SourceType::TabState, &pool, None)
                .await
                .unwrap();

            // 검증:
            // 1. 노트는 is_deleted = true 로 전환
            let deleted_note = repository::get_note(&pool, &note.id)
                .await
                .unwrap()
                .unwrap();
            assert!(
                deleted_note.is_deleted,
                "원본 삭제 시 is_deleted = 1 이어야 함"
            );

            // 2. 과거 스냅샷들은 보존되며 최신 텍스트 미리보기 및 내보내기 가능
            let detail = get_note_recovery_detail(&pool, &store_root, &note.id)
                .await
                .unwrap();
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .as_ref()
                    .unwrap()
                    .recovered_text
                    .as_deref()
                    .unwrap(),
                "Version 2 content updated"
            );
            assert_eq!(detail.latest_confidence, RecoveryConfidence::Full);

            // 3. TXT 복구 내보내기 정상 동작
            let export_target = std::env::temp_dir().join("nr5-test-25-recovered.txt");
            let exported = write_atomic_txt(
                &export_target,
                detail
                    .latest_valid_snapshot
                    .as_ref()
                    .unwrap()
                    .recovered_text
                    .as_ref()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(&exported).unwrap(),
                "Version 2 content updated"
            );

            let _ = fs::remove_file(&exported);
            cleanup_test_env("25");
        });
    }

    // =========================================================================
    // 26. Disaster Scenario B: Complete TabState Directory Loss
    // =========================================================================
    #[test]
    fn test_disaster_26_complete_tabstate_directory_loss() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("26");

            // 3개의 독립된 탭 생성 및 캡처
            for i in 1..=3 {
                let file = source_dir.join(format!("bulk-guid-{i}.bin"));
                let content = format!("Important Note {i}");
                let bytes = generate_synthetic_unsaved(&content, false);
                fs::write(&file, &bytes).unwrap();
                capture_state_file(&file, SourceType::TabState, &pool, &store_root, None)
                    .await
                    .unwrap();
            }

            let count_active = repository::list_notes(
                &pool,
                &NoteFilter {
                    is_deleted: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .len();
            assert_eq!(count_active, 3);

            // 시뮬레이션: 메모장 비정상 종료/데이터 증발로 source 디렉터리 내 모든 파일 삭제
            for i in 1..=3 {
                let file = source_dir.join(format!("bulk-guid-{i}.bin"));
                let _ = fs::remove_file(file);
            }

            // 재조정 실행 (스캔 시 디스크에 없는 노트는 soft-delete 처리)
            scan_and_reconcile(
                &source_dir,
                &source_dir.join("window"),
                &pool,
                &store_root,
                None,
            )
            .await;

            // 검증: 스냅샷 히스토리는 절대 연쇄 삭제되지 않고 100% 보존됨
            let all_notes = repository::list_notes(&pool, &NoteFilter::default())
                .await
                .unwrap();
            assert_eq!(all_notes.len(), 3);
            for n in all_notes {
                assert!(n.is_deleted, "소스 파일 증발 시 삭제 상태로 마킹됨");
                let detail = get_note_recovery_detail(&pool, &store_root, &n.id)
                    .await
                    .unwrap();
                assert!(
                    detail.latest_valid_snapshot.is_some(),
                    "히스토리 데이터는 유지되어야 함"
                );
            }

            cleanup_test_env("26");
        });
    }

    // =========================================================================
    // 27. Disaster Scenario C: Corrupt Incoming State
    // =========================================================================
    #[test]
    fn test_disaster_27_corrupt_incoming_state() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("27");

            let tab_file = source_dir.join("corrupt-flow.bin");

            // 1. 정상 v1
            let v1_bytes = generate_synthetic_unsaved("Good Note Version 1", false);
            fs::write(&tab_file, &v1_bytes).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            // 2. 정상 v2
            let v2_bytes = generate_synthetic_unsaved("Good Note Version 2", false);
            fs::write(&tab_file, &v2_bytes).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            // 3. 손상된 v3 (CRC 위조 / 구조 깨짐)
            let v3_bytes = generate_synthetic_unsaved("Corrupt Note Version 3", true);
            fs::write(&tab_file, &v3_bytes).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            let note = repository::get_note_by_source_id(&pool, "corrupt-flow")
                .await
                .unwrap()
                .unwrap();
            let detail = get_note_recovery_detail(&pool, &store_root, &note.id)
                .await
                .unwrap();

            // 검증:
            // 최신 스냅샷(v3)은 손상(CORRUPT)으로 기록됨
            assert!(detail.is_latest_corrupt);
            assert_eq!(
                detail.latest_snapshot.as_ref().unwrap().parse_status,
                ParseStatus::Corrupt
            );
            // 원시 블롭은 안전 격리 보관됨 (RAW_ONLY)
            assert_eq!(detail.latest_confidence, RecoveryConfidence::RawOnly);

            // 최신 유효(Latest Valid) 버전은 손상되지 않은 v2를 정확히 가리킴
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .as_ref()
                    .unwrap()
                    .recovered_text
                    .as_deref()
                    .unwrap(),
                "Good Note Version 2"
            );
            assert_eq!(detail.valid_confidence, Some(RecoveryConfidence::Full));

            cleanup_test_env("27");
        });
    }

    // =========================================================================
    // 28. Disaster Scenario D: Unsupported Future Format
    // =========================================================================
    #[test]
    fn test_disaster_28_unsupported_future_format() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("28");

            let tab_file = source_dir.join("future-fmt.bin");

            // 정상 v1
            let v1 = generate_synthetic_unsaved("V1 content", false);
            fs::write(&tab_file, &v1).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            // 미지원 포맷 v2 (매직 바이트가 "NP\0"이 아닌 미래 포맷)
            let mut v2 = generate_synthetic_unsaved("Future unknown format", false);
            v2[0] = b'X';
            v2[1] = b'Y';
            v2[2] = b'Z';
            fs::write(&tab_file, &v2).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            let note = repository::get_note_by_source_id(&pool, "future-fmt")
                .await
                .unwrap()
                .unwrap();
            let detail = get_note_recovery_detail(&pool, &store_root, &note.id)
                .await
                .unwrap();

            // RAW 블롭 보존 + UNSUPPORTED_FORMAT 판정
            assert_eq!(
                detail.latest_snapshot.as_ref().unwrap().parse_status,
                ParseStatus::UnsupportedFormat
            );
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .as_ref()
                    .unwrap()
                    .recovered_text
                    .as_deref()
                    .unwrap(),
                "V1 content"
            );

            cleanup_test_env("28");
        });
    }

    // =========================================================================
    // 29. Disaster Scenario E: DB Write Failure Preserves Raw Blob
    // =========================================================================
    #[test]
    fn test_disaster_29_db_write_failure_preserves_raw_blob() {
        tauri::async_runtime::block_on(async {
            let (_source_dir, store_root) = create_test_env("29");

            let tab_data =
                generate_synthetic_unsaved("Critical memo captured before DB crash", false);
            let hash = snapshot::compute_sha256(&tab_data);

            // CAS에 원자적 물리 저장
            let rel_path = snapshot::store_blob(&store_root, &hash, &tab_data).unwrap();
            let abs_path = snapshot::resolve_blob_path(&store_root, &rel_path);

            assert!(abs_path.is_file(), "CAS 블롭 저장 성공");

            // DB 기록 전 크래시 모의: DB가 비어있는 상태
            let pool = create_test_db().await;

            // GC 실행 시 24시간 고아 유예기간에 의해 절대 즉시 삭제되지 않음
            let settings = NotepadRescueSettings::default();
            let plan = plan_gc(&pool, &store_root, &settings, 1000).await.unwrap();
            assert!(!plan.unreferenced_blob_hashes.contains(&hash));
            assert!(
                abs_path.is_file(),
                "미등록 신규 블롭은 유예기간 동안 안전 보호됨"
            );

            cleanup_test_env("29");
        });
    }

    // =========================================================================
    // 30. Disaster Scenario F: Raw Blob Missing (TextOnly Recovery)
    // =========================================================================
    #[test]
    fn test_disaster_30_raw_blob_missing() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("30");

            let tab_file = source_dir.join("missing-raw.bin");
            let bytes = generate_synthetic_unsaved("Recoverable DB Text", false);
            fs::write(&tab_file, &bytes).unwrap();
            capture_state_file(&tab_file, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            let note = repository::get_note_by_source_id(&pool, "missing-raw")
                .await
                .unwrap()
                .unwrap();
            let snap = repository::get_latest_snapshot_for_note(&pool, &note.id)
                .await
                .unwrap()
                .unwrap();

            // 물리 블롭 파일 의도적 제거
            let blob_abs = store_root.join(&snap.raw_snapshot_path);
            assert!(blob_abs.is_file());
            fs::remove_file(&blob_abs).unwrap();

            // 복구 판정 확인: 패닉 없이 TextOnly 판정
            let detail = get_note_recovery_detail(&pool, &store_root, &note.id)
                .await
                .unwrap();
            assert_eq!(detail.latest_confidence, RecoveryConfidence::TextOnly);
            // DB에 보관된 텍스트로 복구 가능
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .unwrap()
                    .recovered_text
                    .unwrap(),
                "Recoverable DB Text"
            );

            cleanup_test_env("30");
        });
    }

    // =========================================================================
    // 31. Disaster Scenario G: Database Lost, Raw CAS Blobs Preserved
    // =========================================================================
    #[test]
    fn test_disaster_31_db_lost_raw_exists() {
        let (_source_dir, store_root) = create_test_env("31");

        // SQLite 없이 CAS 디렉터리에 직접 블롭 생성
        let content1 = "Saved Document via TabState";
        let bytes1 = generate_synthetic_saved("C:\\docs\\doc.txt", content1, false);
        let hash1 = snapshot::compute_sha256(&bytes1);
        snapshot::store_blob(&store_root, &hash1, &bytes1).unwrap();

        // 읽기 전용 스토리지 스캔 실행
        let scanned = scan_and_recover_raw_blobs(&store_root).unwrap();
        assert_eq!(scanned.len(), 1);
        assert!(scanned[0].is_valid_tabstate);
        assert_eq!(scanned[0].hash, hash1);
        assert_eq!(
            scanned[0].recovered_title.as_deref(),
            Some("C:\\docs\\doc.txt")
        );
        assert!(scanned[0]
            .recovered_text_preview
            .as_ref()
            .unwrap()
            .contains("Saved Document"));

        cleanup_test_env("31");
    }

    // =========================================================================
    // 32. Disaster Scenario H: Interrupted GC
    // =========================================================================
    #[test]
    fn test_disaster_32_interrupted_gc() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (_source_dir, store_root) = create_test_env("32");

            // 1. 활성 노트 등록
            sqlx::query(
                "INSERT INTO notepad_notes (id, source_id, display_title, is_saved, is_deleted, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES ('n_gc', 's_gc', 'GC Protected Note', 0, 0, '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            let bytes = generate_synthetic_unsaved("Active Protected Text", false);
            let hash = snapshot::compute_sha256(&bytes);
            let rel_path = snapshot::store_blob(&store_root, &hash, &bytes).unwrap();

            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('snap_gc', 'n_gc', ?1, ?2, 'Active Protected Text', 'tab_state', 't.bin', 10, 'PARSED', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z')"
            ).bind(&hash).bind(&rel_path).execute(&pool).await.unwrap();

            let mut settings = NotepadRescueSettings::default();
            settings.retention_days = 1; // 강제 만료 유도

            // GC 계획 수립 (Dry-Run)
            let plan = plan_gc(&pool, &store_root, &settings, 2000000000)
                .await
                .unwrap();
            // 최신 활성 스냅샷 보존 보장 규칙에 의해 만료 대상에 포함되지 않아야 함
            assert!(!plan.expired_snapshot_ids.contains(&"snap_gc".to_string()));
            assert!(!plan.unreferenced_blob_hashes.contains(&hash));

            // GC 실행 후에도 안전 보존
            let report = execute_gc(&pool, &store_root, &settings, 2000000000)
                .await
                .unwrap();
            assert_eq!(report.deleted_snapshots_count, 0);

            let check = repository::get_snapshot(&pool, "snap_gc").await.unwrap();
            assert!(
                check.is_some(),
                "활성 메모 최신 스냅샷은 GC 중단/실행 후에도 보존되어야 함"
            );

            cleanup_test_env("32");
        });
    }

    // =========================================================================
    // 33. Disaster Scenario I: Concurrent GC and Capture Stress
    // =========================================================================
    #[test]
    fn test_disaster_33_concurrent_gc_and_capture() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("33");

            let pool_clone = pool.clone();
            let store_root_clone = store_root.clone();
            let source_dir_clone = source_dir.clone();

            // 캡처 워커 스레드
            let capture_handle = tauri::async_runtime::spawn(async move {
                for i in 0..10 {
                    let tab = source_dir_clone.join(format!("conc-{i}.bin"));
                    let content = format!("Concurrent Content Iteration {i}");
                    let bytes = generate_synthetic_unsaved(&content, false);
                    fs::write(&tab, &bytes).unwrap();
                    let _ = capture_state_file(
                        &tab,
                        SourceType::TabState,
                        &pool_clone,
                        &store_root_clone,
                        None,
                    )
                    .await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });

            // GC 워커 스레드
            let pool_gc = pool.clone();
            let store_gc = store_root.clone();
            let gc_handle = tauri::async_runtime::spawn(async move {
                let settings = NotepadRescueSettings::default();
                for _ in 0..10 {
                    let _ = execute_gc(&pool_gc, &store_gc, &settings, 2000000000).await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });

            let r1 = capture_handle.await;
            let r2 = gc_handle.await;
            assert!(r1.is_ok());
            assert!(r2.is_ok());

            // 무결성 검증: 데드락이나 DB 손상 없이 모든 캡처 완료 확인
            let count = repository::list_notes(&pool, &NoteFilter::default())
                .await
                .unwrap()
                .len();
            assert_eq!(
                count, 10,
                "동시 GC 중에도 10개의 노트가 무결하게 캡처되어야 함"
            );

            cleanup_test_env("33");
        });
    }

    // =========================================================================
    // 34. Disaster Scenario J: Process Restart Reconciliation
    // =========================================================================
    #[test]
    fn test_disaster_34_process_restart_reconciliation() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("34");

            // 1. 세션 파일 생성
            let tab1 = source_dir.join("reconcile-1.bin");
            let tab2 = source_dir.join("reconcile-2.bin");
            fs::write(&tab1, generate_synthetic_unsaved("Tab 1", false)).unwrap();
            fs::write(&tab2, generate_synthetic_unsaved("Tab 2", false)).unwrap();

            capture_state_file(&tab1, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();
            capture_state_file(&tab2, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            // 2. 프로세스 종료 중 tab2가 수동 삭제되었다고 가정
            fs::remove_file(&tab2).unwrap();

            // 3. 재시작 시 시동 조정(Reconciliation) 실행
            scan_and_reconcile(
                &source_dir,
                &source_dir.join("window"),
                &pool,
                &store_root,
                None,
            )
            .await;

            let n1 = repository::get_note_by_source_id(&pool, "reconcile-1")
                .await
                .unwrap()
                .unwrap();
            let n2 = repository::get_note_by_source_id(&pool, "reconcile-2")
                .await
                .unwrap()
                .unwrap();

            assert!(!n1.is_deleted, "tab1은 계속 존재하므로 활성 상태");
            assert!(
                n2.is_deleted,
                "tab2는 재조정에 의해 안전하게 삭제 상태로 전환됨"
            );

            cleanup_test_env("34");
        });
    }

    // =========================================================================
    // 35. Shared CAS Blob Disaster Path
    // =========================================================================
    #[test]
    fn test_disaster_35_shared_cas_blob_disaster_path() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (source_dir, store_root) = create_test_env("35");

            // 내용이 완전히 동일한 서로 다른 두 개의 탭
            let shared_text = "Shared Secret Content Across Tabs";
            let bytes = generate_synthetic_unsaved(shared_text, false);
            let hash = snapshot::compute_sha256(&bytes);

            let t1 = source_dir.join("shared-1.bin");
            let t2 = source_dir.join("shared-2.bin");
            fs::write(&t1, &bytes).unwrap();
            fs::write(&t2, &bytes).unwrap();

            capture_state_file(&t1, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();
            capture_state_file(&t2, SourceType::TabState, &pool, &store_root, None)
                .await
                .unwrap();

            let n1 = repository::get_note_by_source_id(&pool, "shared-1")
                .await
                .unwrap()
                .unwrap();
            let n2 = repository::get_note_by_source_id(&pool, "shared-2")
                .await
                .unwrap()
                .unwrap();

            // 탭 1 소멸 및 만료 정리
            sqlx::query("UPDATE notepad_notes SET is_deleted = 1, deleted_at = '2026-08-01T00:00:00Z' WHERE id = ?1")
                .bind(&n1.id)
                .execute(&pool)
                .await
                .unwrap();

            let mut settings = NotepadRescueSettings::default();
            settings.deleted_retention_days = 1;

            execute_gc(&pool, &store_root, &settings, 2000000000)
                .await
                .unwrap();

            // 검증: n1의 논리적 삭제 및 정리에도 불구하고 n2가 참조하고 있으므로 물리 블롭은 절대 보존됨
            let blob_path =
                snapshot::resolve_blob_path(&store_root, &snapshot::relative_blob_path(&hash));
            assert!(
                blob_path.is_file(),
                "공유 물리 블롭은 참조가 1개라도 남아있으면 보존되어야 함"
            );

            let detail_n2 = get_note_recovery_detail(&pool, &store_root, &n2.id)
                .await
                .unwrap();
            assert_eq!(detail_n2.latest_confidence, RecoveryConfidence::Full);
            assert_eq!(
                detail_n2
                    .latest_valid_snapshot
                    .unwrap()
                    .recovered_text
                    .unwrap(),
                shared_text
            );

            cleanup_test_env("35");
        });
    }

    // =========================================================================
    // 36. Latest Active Snapshot Protected Under Quota Pressure
    // =========================================================================
    #[test]
    fn test_disaster_36_latest_active_snapshot_protected() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let (_source_dir, store_root) = create_test_env("36");

            // 1. 활성 메모 생성
            sqlx::query(
                "INSERT INTO notepad_notes (id, source_id, display_title, is_saved, is_deleted, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES ('note_protect', 'src_p', 'Must Survive Note', 0, 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            let bytes = generate_synthetic_unsaved("Indestructible User Note", false);
            let hash = snapshot::compute_sha256(&bytes);
            let rel = snapshot::store_blob(&store_root, &hash, &bytes).unwrap();

            // 200일 전 생성된 스냅샷
            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('snap_old', 'note_protect', ?1, ?2, 'Indestructible User Note', 'tab_state', 't.bin', 1000, 'PARSED', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"
            ).bind(&hash).bind(&rel).execute(&pool).await.unwrap();

            // 극단적인 스토리지 할당 압박 (한도: 10 바이트, 보관 기한: 1일)
            let mut settings = NotepadRescueSettings::default();
            settings.max_storage_bytes = 10;
            settings.retention_days = 1;

            let report = execute_gc(&pool, &store_root, &settings, 2000000000)
                .await
                .unwrap();

            // 데이터 안전 정책에 따라 활성 노트의 최신 스냅샷은 쿼터 압박 하에서도 절대 삭제되지 않음
            assert_eq!(report.deleted_snapshots_count, 0);
            assert!(
                report.has_quota_pressure,
                "쿼터 초과 상태가 감지 및 보고되어야 함"
            );

            let detail = get_note_recovery_detail(&pool, &store_root, "note_protect")
                .await
                .unwrap();
            assert_eq!(detail.latest_confidence, RecoveryConfidence::Full);
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .unwrap()
                    .recovered_text
                    .unwrap(),
                "Indestructible User Note"
            );

            cleanup_test_env("36");
        });
    }
}
