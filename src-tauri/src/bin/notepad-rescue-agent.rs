// Prevents console window on Windows across all builds
#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
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

#[cfg(windows)]
extern "system" {
    fn AttachConsole(dw_process_id: u32) -> i32;
}

fn attach_parent_console_if_available() {
    #[cfg(windows)]
    unsafe {
        AttachConsole(!0u32);
    }
}

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

fn log_agent(storage_root: &Path, is_background: bool, msg: &str) {
    if !is_background {
        println!("{msg}");
    }
    let log_path = storage_root.join("agent.log");
    let ts = now_rfc3339();
    let line = format!("[{ts}] {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(line.as_bytes())
        });
}

fn log_agent_err(storage_root: &Path, is_background: bool, msg: &str) {
    if !is_background {
        eprintln!("{msg}");
    }
    let log_path = storage_root.join("agent.log");
    let ts = now_rfc3339();
    let line = format!("[{ts}] ERROR: {msg}\n");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(line.as_bytes())
        });
}

fn main() {
    let storage_root = resolve_storage_root();
    let root_clone = storage_root.clone();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("PANIC: {info}\n");
        let _ = std::fs::create_dir_all(&root_clone);
        let _ = std::fs::write(root_clone.join("crash.log"), msg);
    }));
    tauri::async_runtime::block_on(async_main());
}

async fn async_main() {
    let args: Vec<String> = std::env::args().collect();
    let storage_root = resolve_storage_root();
    let db_path = resolve_db_path();
    let is_background = args.iter().any(|a| a == "--background");

    if args.iter().any(|a| a == "--stop") {
        attach_parent_console_if_available();
        let _ = request_agent_stop(&storage_root);
        println!("Notepad Rescue Agent: Stop signal requested.");
        return;
    }

    if args.iter().any(|a| a == "--status") {
        attach_parent_console_if_available();
        if is_agent_instance_running(&storage_root) {
            if let Ok(Some(s)) = read_heartbeat(&storage_root) {
                println!(
                    "Notepad Rescue Agent: RUNNING (PID: {}, Protocol: {}, Uptime since: {})",
                    s.pid, s.protocol_version, s.started_at
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
            log_agent_err(
                &storage_root,
                is_background,
                "Notepad Rescue Agent is already running in another process. Exiting.",
            );
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
            log_agent_err(
                &storage_root,
                is_background,
                &format!("Failed to connect to SQLite database: {e}"),
            );
            return;
        }
    };

    // 3. 스키마 검증 및 설정 확인
    let _ = repository::ensure_schema(&pool).await;
    let settings = repository::get_settings(&pool).await.unwrap_or_default();
    if !settings.enabled || !settings.watch_enabled {
        log_agent(
            &storage_root,
            is_background,
            "Notepad Rescue is disabled in settings. Agent exiting.",
        );
        return;
    }

    // 4. 감시 대상 디렉터리 확인
    let local_state = match default_notepad_local_state_dir() {
        Some(dir) => dir,
        None => {
            log_agent_err(
                &storage_root,
                is_background,
                "Windows Notepad LocalState directory not found.",
            );
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
            log_agent_err(
                &storage_root,
                is_background,
                &format!("Failed to start Notepad watcher: {e}"),
            );
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
        protocol_version: AGENT_PROTOCOL_VERSION,
    };
    let _ = write_heartbeat(&storage_root, &status);

    log_agent(
        &storage_root,
        is_background,
        &format!(
            "Notepad Rescue Agent background worker started (PID: {})",
            status.pid
        ),
    );

    // 7. 하트비트 및 라이프사이클 루프
    while running.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(1500)).await;

        if has_stop_request(&storage_root) {
            log_agent(
                &storage_root,
                is_background,
                "Stop flag received. Initiating graceful shutdown.",
            );
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
    log_agent(
        &storage_root,
        is_background,
        "Notepad Rescue Agent shutting down...",
    );
    status.status = "STOPPED".to_string();
    status.watcher_active = false;
    status.last_heartbeat = now_rfc3339();
    let _ = write_heartbeat(&storage_root, &status);
    clear_stop_request(&storage_root);

    watcher.stop();
    drop(instance_lock);
    log_agent(
        &storage_root,
        is_background,
        "Notepad Rescue Agent terminated cleanly.",
    );
}
