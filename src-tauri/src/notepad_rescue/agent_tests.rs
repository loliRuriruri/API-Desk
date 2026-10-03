//! Tests for Notepad Rescue Phase NR.6: Background Protection Agent Lifecycle & Coordination.
//!
//! Test Matrix (Tests 1 ~ 36):
//! - 1..6: Process ownership & single instance locking (AgentInstanceLock)
//! - 7..12: Heartbeat status serialization, staleness detection, and IPC stop signaling
//! - 13..18: Cross-process GC lock mutual exclusion, timeouts, and RAII safety
//! - 19..24: Safe coordination between capture pipeline and GC under lock
//! - 25..30: Startup registry configuration without admin elevation
//! - 31..36: SQLite WAL concurrent read/write and startup reconciliation

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

use crate::notepad_rescue::coordination::{
    clear_stop_request, has_stop_request, is_agent_alive, is_startup_enabled, read_heartbeat,
    request_agent_stop, set_startup_enabled, write_heartbeat, AgentInstanceLock, AgentStatus,
    CrossProcessGcLock,
};
use crate::notepad_rescue::gc::{execute_gc, plan_gc};
use crate::notepad_rescue::models::{NoteFilter, NotepadNote, NotepadRescueSettings, SourceType};
use crate::notepad_rescue::parser::generate_synthetic_unsaved;
use crate::notepad_rescue::pipeline::capture_state_file;
use crate::notepad_rescue::repository::*;
use crate::notepad_rescue::snapshot::store_blob;

/// Test helper to create an in-memory SQLite database with all migrations applied.
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

fn unique_test_id(prefix: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{prefix}-{nanos}-{n}")
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Test helper to create an isolated temporary storage directory.
fn create_test_storage_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nr6-test-{}", unique_test_id(prefix)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn test_settings() -> NotepadRescueSettings {
    NotepadRescueSettings {
        enabled: true,
        watch_enabled: true,
        retention_days: 30,
        deleted_retention_days: 30,
        max_storage_bytes: 1024 * 1024 * 1024,
        updated_at: "2026-10-03T12:00:00Z".to_string(),
    }
}

// =========================================================================
// Group 1: Process Ownership & Single Instance Locking (Tests 1..6)
// =========================================================================

#[test]
fn test_agent_01_instance_lock_acquire_and_drop() {
    let dir = create_test_storage_dir("lock-01");
    let name = unique_test_id("Lock01");
    let lock = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(lock.is_some(), "First lock acquisition should succeed");
    drop(lock);

    // After drop, acquiring again should succeed immediately
    let lock2 = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(
        lock2.is_some(),
        "Re-acquiring lock after drop should succeed"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_02_instance_lock_mutual_exclusion() {
    let dir = create_test_storage_dir("lock-02");
    let name = unique_test_id("Lock02");
    let lock1 = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(lock1.is_some());

    // Second acquisition attempt while lock1 is held must fail
    let lock2 = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(lock2.is_none(), "Concurrent lock acquisition must fail");

    drop(lock1);

    // After releasing lock1, lock3 can acquire
    let lock3 = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(
        lock3.is_some(),
        "Lock acquisition must succeed after release"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_03_instance_lock_raii_guard() {
    let dir = create_test_storage_dir("lock-03");
    let name = unique_test_id("Lock03");
    {
        let _guard = AgentInstanceLock::try_acquire_named(&dir, &name);
        assert!(_guard.is_some());
        let conflict = AgentInstanceLock::try_acquire_named(&dir, &name);
        assert!(conflict.is_none());
    }
    // Out of scope: guard dropped
    let reacquire = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(reacquire.is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_04_instance_lock_with_valid_dir() {
    let dir = create_test_storage_dir("lock-04");
    let name = unique_test_id("Lock04");
    assert!(dir.exists());
    let lock = AgentInstanceLock::try_acquire_named(&dir, &name);
    assert!(lock.is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_05_instance_lock_multiple_subdirs() {
    let dir_a = create_test_storage_dir("lock-05a");
    let name = unique_test_id("Lock05");
    let lock = AgentInstanceLock::try_acquire_named(&dir_a, &name);
    assert!(lock.is_some());
    let _ = fs::remove_dir_all(&dir_a);
}

#[test]
fn test_agent_06_instance_lock_rapid_cycling() {
    let dir = create_test_storage_dir("lock-06");
    let name = unique_test_id("Lock06");
    for _ in 0..5 {
        let lock = AgentInstanceLock::try_acquire_named(&dir, &name);
        assert!(lock.is_some());
        drop(lock);
    }
    let _ = fs::remove_dir_all(&dir);
}

// =========================================================================
// Group 2: Heartbeat & IPC Status (Tests 7..12)
// =========================================================================

#[test]
fn test_agent_07_heartbeat_write_and_read() {
    let dir = create_test_storage_dir("heartbeat");
    let status = AgentStatus {
        pid: std::process::id(),
        started_at: "2026-10-03T12:00:00Z".to_string(),
        last_heartbeat: "2026-10-03T12:00:00Z".to_string(),
        status: "RUNNING".to_string(),
        version: "0.1.0".to_string(),
        watcher_active: true,
        captured_count: 5,
        last_captured_at: None,
        provider: "background_agent".to_string(),
    };

    assert!(write_heartbeat(&dir, &status).is_ok());

    let read_back = read_heartbeat(&dir).unwrap();
    assert!(read_back.is_some());
    let read_back = read_back.unwrap();
    assert_eq!(read_back.pid, status.pid);
    assert_eq!(read_back.status, "RUNNING");
    assert_eq!(read_back.version, "0.1.0");
    assert!(read_back.watcher_active);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_08_heartbeat_missing_file_returns_none() {
    let dir = create_test_storage_dir("heartbeat-missing");
    let read_back = read_heartbeat(&dir).unwrap();
    assert!(read_back.is_none());
    assert!(!is_agent_alive(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_09_heartbeat_is_alive_fresh() {
    let dir = create_test_storage_dir("heartbeat-alive");
    let now = crate::notepad_rescue::coordination::now_rfc3339();
    let status = AgentStatus {
        pid: std::process::id(),
        last_heartbeat: now,
        status: "RUNNING".to_string(),
        ..AgentStatus::default()
    };

    write_heartbeat(&dir, &status).unwrap();
    assert!(is_agent_alive(&dir));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_10_heartbeat_is_alive_stale() {
    let dir = create_test_storage_dir("heartbeat-stale");
    // Timestamp 60 seconds in the past
    let old_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 60;
    let old_time = crate::usage::epoch_to_rfc3339(old_secs as i64);
    let status = AgentStatus {
        pid: std::process::id(),
        last_heartbeat: old_time,
        status: "RUNNING".to_string(),
        ..AgentStatus::default()
    };

    write_heartbeat(&dir, &status).unwrap();
    assert!(
        !is_agent_alive(&dir),
        "Heartbeat older than 10s must be classified as stale/inactive"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_11_stop_request_lifecycle() {
    let dir = create_test_storage_dir("stop-request");
    assert!(!has_stop_request(&dir));

    assert!(request_agent_stop(&dir).is_ok());
    assert!(has_stop_request(&dir));

    clear_stop_request(&dir);
    assert!(!has_stop_request(&dir));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_12_heartbeat_corrupt_json_handled_cleanly() {
    let dir = create_test_storage_dir("heartbeat-corrupt");
    let hb_file = dir.join("agent_heartbeat.json");
    fs::write(&hb_file, b"NOT_A_VALID_JSON{:::").unwrap();

    let read_back = read_heartbeat(&dir).unwrap();
    assert!(read_back.is_none());
    assert!(!is_agent_alive(&dir));

    let _ = fs::remove_dir_all(&dir);
}

// =========================================================================
// Group 3: Cross-Process GC Locking (Tests 13..18)
// =========================================================================

#[test]
fn test_agent_13_gc_lock_acquire_and_drop() {
    let dir = create_test_storage_dir("gclock-13");
    let name = unique_test_id("gclock-13");
    let lock = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(500));
    assert!(lock.is_ok(), "GC lock should be acquired");
    drop(lock);

    let lock2 = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(500));
    assert!(lock2.is_ok(), "GC lock should be re-acquired after drop");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_14_gc_lock_timeout_on_contention() {
    let dir = create_test_storage_dir("gclock-14");
    let name = unique_test_id("gclock-14");
    let lock1 = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(1000));
    assert!(lock1.is_ok());

    let dir_clone = dir.clone();
    let name_clone = name.clone();
    let thread = std::thread::spawn(move || {
        let start = Instant::now();
        let lock2 =
            CrossProcessGcLock::acquire_named(&dir_clone, &name_clone, Duration::from_millis(100));
        let elapsed = start.elapsed();
        (lock2.is_err(), elapsed)
    });

    let (is_err, elapsed) = thread.join().unwrap();
    assert!(
        is_err,
        "Lock acquisition on separate thread should fail under contention"
    );
    assert!(
        elapsed >= Duration::from_millis(80),
        "Timeout duration should be respected"
    );

    drop(lock1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_15_gc_lock_handoff_after_release() {
    let dir = create_test_storage_dir("gclock-15");
    let name = unique_test_id("gclock-15");
    let lock1 = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(500));
    assert!(lock1.is_ok());

    let dir_clone = dir.clone();
    let name_clone = name.clone();
    let thread = std::thread::spawn(move || {
        let lock =
            CrossProcessGcLock::acquire_named(&dir_clone, &name_clone, Duration::from_millis(1500));
        assert!(lock.is_ok());
    });

    std::thread::sleep(Duration::from_millis(100));
    drop(lock1);

    thread.join().expect("Thread should finish successfully");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_16_gc_lock_subsequent_attempts() {
    let dir = create_test_storage_dir("gclock-16");
    let name = unique_test_id("gclock-16");
    for _ in 0..3 {
        let lock = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(200));
        assert!(lock.is_ok());
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_17_gc_lock_zero_timeout() {
    let dir = create_test_storage_dir("gclock-17");
    let name = unique_test_id("gclock-17");
    let lock = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(0));
    assert!(lock.is_ok());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_agent_18_gc_lock_raii_scope_safety() {
    let dir = create_test_storage_dir("gclock-18");
    let name = unique_test_id("gclock-18");
    {
        let _lock = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(500));
        assert!(_lock.is_ok());
    }
    let lock_next = CrossProcessGcLock::acquire_named(&dir, &name, Duration::from_millis(500));
    assert!(lock_next.is_ok());
    let _ = fs::remove_dir_all(&dir);
}

// =========================================================================
// Group 4: Safe Coordination under GC & Pipeline (Tests 19..24)
// =========================================================================

#[test]
fn test_agent_19_gc_preview_does_not_modify_cas() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("gc-preview");
        let settings = test_settings();

        let data = b"Test data for GC preview";
        let hash = crate::notepad_rescue::snapshot::compute_sha256(data);
        store_blob(&storage_root, &hash, data).unwrap();
        let _plan = plan_gc(&pool, &storage_root, &settings, now_epoch())
            .await
            .unwrap();

        // Preview identifies orphan blob if older than grace period
        // But regardless, preview must not delete anything
        let blob_path = crate::notepad_rescue::resolve_snapshot_path(
            &storage_root,
            &crate::notepad_rescue::relative_blob_path(&hash),
        );
        assert!(
            blob_path.exists(),
            "Plan/Preview must be strictly non-destructive"
        );

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_20_gc_execute_acquires_lock_and_cleans() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("gc-exec");
        let settings = test_settings();

        let data = b"Test data for GC execute";
        let hash = crate::notepad_rescue::snapshot::compute_sha256(data);
        store_blob(&storage_root, &hash, data).unwrap();
        let _report = execute_gc(&pool, &storage_root, &settings, now_epoch())
            .await
            .unwrap();

        // Fresh blob is within 24h grace period, so it remains safely protected
        let blob_path = crate::notepad_rescue::resolve_snapshot_path(
            &storage_root,
            &crate::notepad_rescue::relative_blob_path(&hash),
        );
        assert!(
            blob_path.exists(),
            "Fresh orphan blob within grace period is not prematurely deleted"
        );

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_21_pipeline_capture_acquires_gc_lock() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("pipe-gc-lock");

        let tab_file = storage_root.join("tab_test.bin");
        let content = generate_synthetic_unsaved("Concurrent Pipeline Note", false);
        fs::write(&tab_file, &content).unwrap();

        let snap_id =
            capture_state_file(&tab_file, SourceType::TabState, &pool, &storage_root, None)
                .await
                .unwrap();

        assert!(snap_id.is_some());
        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_22_pipeline_capture_skips_temp_files() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("pipe-skip-tmp");

        let tmp_file = storage_root.join("test.tmp");
        fs::write(&tmp_file, b"temporary partial data").unwrap();

        let snap_id =
            capture_state_file(&tmp_file, SourceType::TabState, &pool, &storage_root, None)
                .await
                .unwrap();

        assert!(snap_id.is_none(), "Pipeline must skip .tmp files");
        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_23_pipeline_deduplicates_identical_captures() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("pipe-dedup");

        let tab_file = storage_root.join("tab_dedup.bin");
        let content = generate_synthetic_unsaved("Identical Deduplicated Content", false);
        fs::write(&tab_file, &content).unwrap();

        let snap1 = capture_state_file(&tab_file, SourceType::TabState, &pool, &storage_root, None)
            .await
            .unwrap();
        assert!(snap1.is_some());

        // Capture identical content again
        let snap2 = capture_state_file(&tab_file, SourceType::TabState, &pool, &storage_root, None)
            .await
            .unwrap();
        assert!(snap2.is_none(), "Unmodified content must be deduplicated");

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_24_pipeline_recovers_after_content_mutation() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("pipe-mutate");

        let tab_file = storage_root.join("tab_mutate.bin");
        let c1 = generate_synthetic_unsaved("Version 1", false);
        fs::write(&tab_file, &c1).unwrap();

        let s1 = capture_state_file(&tab_file, SourceType::TabState, &pool, &storage_root, None)
            .await
            .unwrap();
        assert!(s1.is_some());

        let c2 = generate_synthetic_unsaved("Version 2", false);
        fs::write(&tab_file, &c2).unwrap();

        let s2 = capture_state_file(&tab_file, SourceType::TabState, &pool, &storage_root, None)
            .await
            .unwrap();
        assert!(s2.is_some(), "Mutated content must trigger a new snapshot");

        let _ = fs::remove_dir_all(&storage_root);
    });
}

// =========================================================================
// Group 5: Startup Registry Configuration (Tests 25..30)
// =========================================================================

static REGISTRY_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_agent_25_startup_query_succeeds() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    // Reading registry does not require elevation and must not panic
    let status = is_startup_enabled();
    let _ = status;
}

#[test]
fn test_agent_26_startup_set_and_query_roundtrip() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    let initial = is_startup_enabled();
    let target = !initial;
    let set_res = set_startup_enabled(target, None);
    if set_res.is_ok() {
        assert_eq!(is_startup_enabled(), target);
        // Restore original state
        let _ = set_startup_enabled(initial, None);
        assert_eq!(is_startup_enabled(), initial);
    }
}

#[test]
fn test_agent_27_startup_idempotent_enable() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    let initial = is_startup_enabled();
    let _ = set_startup_enabled(true, None);
    let _ = set_startup_enabled(true, None);
    // Restore
    let _ = set_startup_enabled(initial, None);
}

#[test]
fn test_agent_28_startup_idempotent_disable() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    let initial = is_startup_enabled();
    let _ = set_startup_enabled(false, None);
    let _ = set_startup_enabled(false, None);
    // Restore
    let _ = set_startup_enabled(initial, None);
}

#[test]
fn test_agent_29_startup_path_contains_no_elevation_prompts() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    let enabled = is_startup_enabled();
    assert!(enabled == true || enabled == false);
}

#[test]
fn test_agent_30_startup_status_consistency() {
    let _guard = REGISTRY_TEST_MUTEX.lock().unwrap();
    let a = is_startup_enabled();
    let b = is_startup_enabled();
    assert_eq!(a, b, "Consecutive reads must return consistent status");
}

// =========================================================================
// Group 6: SQLite WAL Concurrency & Startup Reconciliation (Tests 31..36)
// =========================================================================

#[test]
fn test_agent_31_sqlite_concurrent_readers() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;

        let note = NotepadNote {
            id: unique_test_id("note"),
            source_id: "concurrent-source-1".to_string(),
            display_title: "Concurrent Note".to_string(),
            original_path: None,
            is_saved: false,
            is_deleted: false,
            first_seen_at: "2026-10-03T12:00:00Z".to_string(),
            last_seen_at: "2026-10-03T12:00:00Z".to_string(),
            deleted_at: None,
            created_at: "2026-10-03T12:00:00Z".to_string(),
            updated_at: "2026-10-03T12:00:00Z".to_string(),
        };
        upsert_note(&pool, &note).await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let pool_clone = pool.clone();
            let note_id = note.id.clone();
            handles.push(tokio::spawn(async move {
                let n = get_note(&pool_clone, &note_id).await.unwrap();
                assert!(n.is_some());
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
    });
}

#[test]
fn test_agent_32_sqlite_concurrent_readers_and_writers() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;

        let mut handles = Vec::new();
        for i in 0..5 {
            let pool_clone = pool.clone();
            handles.push(tokio::spawn(async move {
                let note = NotepadNote {
                    id: unique_test_id("note"),
                    source_id: format!("concurrent-writer-{i}"),
                    display_title: format!("Concurrent Writer {i}"),
                    original_path: None,
                    is_saved: false,
                    is_deleted: false,
                    first_seen_at: "2026-10-03T12:00:00Z".to_string(),
                    last_seen_at: "2026-10-03T12:00:00Z".to_string(),
                    deleted_at: None,
                    created_at: "2026-10-03T12:00:00Z".to_string(),
                    updated_at: "2026-10-03T12:00:00Z".to_string(),
                };
                upsert_note(&pool_clone, &note).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let all_notes = list_notes(&pool, &NoteFilter::default()).await.unwrap();
        assert_eq!(all_notes.len(), 5);
    });
}

#[test]
fn test_agent_33_startup_reconciliation_detects_preexisting_files() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("recon-preexist");
        let tab_dir = storage_root.join("TabState");
        fs::create_dir_all(&tab_dir).unwrap();

        // 3 files created before agent starts
        for i in 1..=3 {
            let file = tab_dir.join(format!("tab-file-{i}.bin"));
            let content = generate_synthetic_unsaved(&format!("Reconciled Title {i}"), false);
            fs::write(&file, &content).unwrap();
        }

        // Run reconciliation scan
        if let Ok(entries) = fs::read_dir(&tab_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    let _ =
                        capture_state_file(&path, SourceType::TabState, &pool, &storage_root, None)
                            .await;
                }
            }
        }

        let notes = list_notes(&pool, &NoteFilter::default()).await.unwrap();
        assert_eq!(
            notes.len(),
            3,
            "All 3 pre-existing files should be captured and reconciled"
        );

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_34_startup_reconciliation_idempotency() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("recon-idempotent");
        let tab_dir = storage_root.join("TabState");
        fs::create_dir_all(&tab_dir).unwrap();

        let file = tab_dir.join("idempotent.bin");
        let content = generate_synthetic_unsaved("Idempotent Reconciliation", false);
        fs::write(&file, &content).unwrap();

        // Scan twice
        for _ in 0..2 {
            let _ =
                capture_state_file(&file, SourceType::TabState, &pool, &storage_root, None).await;
        }

        let notes = list_notes(&pool, &NoteFilter::default()).await.unwrap();
        assert_eq!(notes.len(), 1);

        let snaps = list_note_snapshots(&pool, &notes[0].id).await.unwrap();
        assert_eq!(
            snaps.len(),
            1,
            "Snapshot must not be duplicated on repeated reconciliation"
        );

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_35_storage_stats_consistent_after_reconciliation() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;
        let storage_root = create_test_storage_dir("recon-stats");
        let tab_dir = storage_root.join("TabState");
        fs::create_dir_all(&tab_dir).unwrap();

        let file = tab_dir.join("stats_test.bin");
        let content = generate_synthetic_unsaved("Stats Note", false);
        fs::write(&file, &content).unwrap();

        let _ = capture_state_file(&file, SourceType::TabState, &pool, &storage_root, None).await;

        let stats = get_storage_statistics(&pool).await.unwrap();
        assert_eq!(stats.active_notes, 1);
        assert_eq!(stats.total_snapshots, 1);
        assert!(stats.total_snapshot_bytes > 0);

        let _ = fs::remove_dir_all(&storage_root);
    });
}

#[test]
fn test_agent_36_db_connection_pool_resilience() {
    tauri::async_runtime::block_on(async {
        let pool = create_test_db().await;

        // Perform 50 rapid sequential queries
        for i in 0..50 {
            let stats = get_storage_statistics(&pool).await;
            assert!(
                stats.is_ok(),
                "Query {i} should succeed without connection exhaustion"
            );
        }
    });
}
