// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use api_desk_lib::notepad_rescue::coordination::*;
use api_desk_lib::notepad_rescue::models::NoteFilter;
use api_desk_lib::notepad_rescue::repository;
use api_desk_lib::notepad_rescue::watcher::{
    default_notepad_local_state_dir, scan_and_reconcile, start_notepad_watcher,
    DEFAULT_DEBOUNCE_DURATION,
};

fn resolve_storage_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data)
                .join("com.apidesk.desktop")
                .join("notepad_rescue");
        }
    }
    std::env::temp_dir().join("apidesk_notepad_rescue")
}

fn resolve_db_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            return PathBuf::from(app_data)
                .join("com.apidesk.desktop")
                .join("apidb.db");
        }
    }
    std::env::temp_dir().join("apidb.db")
}

fn main() {
    tauri::async_runtime::block_on(async_main());
}

async fn async_main() {
    let args: Vec<String> = std::env::args().collect();
    let storage_root = resolve_storage_root();
    let db_path = resolve_db_path();

    if args.iter().any(|a| a == "--stop") {
        let _ = request_agent_stop(&storage_root);
        println!("Notepad Rescue Agent: Stop signal requested.");
        return;
    }

    if args.iter().any(|a| a == "--status") {
        if is_agent_alive(&storage_root) {
            if let Ok(Some(s)) = read_heartbeat(&storage_root) {
                println!(
                    "Notepad Rescue Agent: RUNNING (PID: {}, Uptime since: {})",
                    s.pid, s.started_at
                );
            } else {
                println!("Notepad Rescue Agent: RUNNING");
            }
        } else {
            println!("Notepad Rescue Agent: STOPPED");
        }
        return;
    }

    // 1. 단일 인스턴스 상호 배제 락 획득
    let instance_lock = match AgentInstanceLock::try_acquire(&storage_root) {
        Some(lock) => lock,
        None => {
            eprintln!("Notepad Rescue Agent is already running in another process. Exiting.");
            return;
        }
    };

    let _ = std::fs::create_dir_all(&storage_root);
    clear_stop_request(&storage_root);

    // 2. DB 연결
    if !db_path.exists() {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let pool = match SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true),
        )
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to connect to SQLite database at {:?}: {e}", db_path);
            return;
        }
    };

    // 3. 설정 확인
    let settings = repository::get_settings(&pool).await.unwrap_or_default();
    if !settings.enabled || !settings.watch_enabled {
        eprintln!("Notepad Rescue is disabled in settings. Agent exiting.");
        return;
    }

    // 4. 감시 대상 디렉터리 확인
    let local_state = match default_notepad_local_state_dir() {
        Some(dir) => dir,
        None => {
            eprintln!("Windows Notepad LocalState directory not found.");
            return;
        }
    };

    let tab_dir = local_state.join("TabState");
    let win_dir = local_state.join("WindowState");

    // 5. 초기 스캔 및 시동 동기화 (Startup Reconciliation)
    let initial_scanned = scan_and_reconcile(&tab_dir, &win_dir, &pool, &storage_root, None).await;

    // 6. 감시 워커 가동
    let watcher = match start_notepad_watcher(
        tab_dir,
        win_dir,
        pool.clone(),
        storage_root.clone(),
        DEFAULT_DEBOUNCE_DURATION,
        None,
    )
    .await
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Failed to start Notepad watcher: {e}");
            return;
        }
    };

    let running = Arc::new(AtomicBool::new(true));

    let mut status = AgentStatus {
        pid: std::process::id(),
        started_at: now_rfc3339(),
        last_heartbeat: now_rfc3339(),
        status: "RUNNING".to_string(),
        version: AGENT_VERSION.to_string(),
        watcher_active: true,
        captured_count: initial_scanned,
        last_captured_at: None,
        provider: "background_agent".to_string(),
    };
    let _ = write_heartbeat(&storage_root, &status);

    println!(
        "Notepad Rescue Agent started (PID: {}, storage: {})",
        status.pid,
        storage_root.display()
    );

    // 7. 하트비트 및 라이프사이클 루프
    while running.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(1500)).await;

        if has_stop_request(&storage_root) {
            println!("Stop flag received. Initiating graceful shutdown.");
            break;
        }

        // DB에서 통계 정보 갱신
        if let Ok(notes) = repository::list_notes(&pool, &NoteFilter::default()).await {
            status.captured_count = notes.len();
        }

        status.last_heartbeat = now_rfc3339();
        let _ = write_heartbeat(&storage_root, &status);
    }

    // 8. 안전 종료
    println!("Notepad Rescue Agent shutting down...");
    status.status = "STOPPED".to_string();
    status.watcher_active = false;
    status.last_heartbeat = now_rfc3339();
    let _ = write_heartbeat(&storage_root, &status);
    clear_stop_request(&storage_root);

    watcher.stop();
    drop(instance_lock);
    println!("Notepad Rescue Agent terminated cleanly.");
}
