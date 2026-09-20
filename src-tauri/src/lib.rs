mod envfile;
mod error;
mod export;
mod http_test;
mod migrations;
mod model_import;
mod monitors;
mod usage;
mod vault;

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;

use error::AppError;
use vault::VaultState;

#[derive(Default)]
pub struct TrayState {
    resident: AtomicBool,
}

#[derive(Default)]
pub struct LaunchState {
    autostart: AtomicBool,
}

impl LaunchState {
    fn new() -> Self {
        Self::default()
    }
}

fn launched_from_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == "--autostart")
}

#[tauri::command]
fn launch_startup(state: tauri::State<'_, LaunchState>) -> bool {
    state.autostart.load(Ordering::Relaxed)
}

impl TrayState {
    fn new() -> Self {
        Self {
            resident: AtomicBool::new(true),
        }
    }
}

#[tauri::command]
fn set_tray_resident(state: tauri::State<'_, TrayState>, resident: bool) {
    state.resident.store(resident, Ordering::Relaxed);
}

fn open_mini_window(app: &tauri::AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("mini") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "mini", WebviewUrl::App("index.html?window=mini".into()))
        .title("API Desk Mini")
        .inner_size(430.0, 720.0)
        .min_inner_size(340.0, 340.0)
        .always_on_top(true)
        .skip_taskbar(true)
        .build()
        .map_err(|e| AppError::Io(format!("미니 창을 만들 수 없습니다: {e}")))?;
    Ok(())
}

#[tauri::command]
fn open_mini_window_command(app: tauri::AppHandle) -> Result<(), AppError> {
    // Creating a second webview synchronously inside an IPC handler can come up
    // blank on Windows, so defer the creation onto the main thread.
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let main_thread_handle = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = open_mini_window(&main_thread_handle);
        });
    });
    Ok(())
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppPaths {
    db_path: String,
    vault_path: String,
    salt_path: String,
    app_config_dir: String,
    app_local_data_dir: String,
}

#[tauri::command]
fn app_paths(app: tauri::AppHandle) -> Result<AppPaths, AppError> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Io(e.to_string()))?;
    let local_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::Io(e.to_string()))?;
    let paths = vault::vault_paths(&app)?;
    Ok(AppPaths {
        db_path: config_dir.join("apidb.db").to_string_lossy().to_string(),
        vault_path: paths.vault.to_string_lossy().to_string(),
        salt_path: paths.salt.to_string_lossy().to_string(),
        app_config_dir: config_dir.to_string_lossy().to_string(),
        app_local_data_dir: local_dir.to_string_lossy().to_string(),
    })
}

fn quit_app(app: &tauri::AppHandle) {
    // Exiting directly inside the tray menu event hangs on Windows because the
    // menu's modal message loop swallows the exit request. Defer it to a worker
    // thread and hard-exit as a watchdog if the graceful exit never completes.
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        handle.exit(0);
        std::thread::sleep(std::time::Duration::from_millis(4_000));
        std::process::exit(0);
    });
}

fn build_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let open_item = MenuItemBuilder::with_id("open", "API Desk 열기").build(app)?;
    let mini_item = MenuItemBuilder::with_id("mini", "미니 창").build(app)?;
    let refresh_item = MenuItemBuilder::with_id("refresh", "사용량 새로고침").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "종료").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&open_item, &mini_item, &refresh_item, &quit_item])
        .build()?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "mini" => {
                let _ = open_mini_window(app);
            }
            "refresh" => {
                let _ = app.emit("usage-refresh", ());
            }
            "quit" => {
                quit_app(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                let _ = open_mini_window(app);
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(
            tauri_plugin_sql::Builder::default()
                .add_migrations("sqlite:apidb.db", migrations::migrations())
                .build(),
        )
        .manage(VaultState::new())
        .manage(TrayState::new())
        .manage(LaunchState::new())
        .setup(|app| {
            let autostart = launched_from_autostart(std::env::args());
            app.state::<LaunchState>()
                .autostart
                .store(autostart, Ordering::Relaxed);
            build_tray(app.handle())?;
            if !autostart {
                // Refresh the autostart registration so it carries the
                // --autostart flag (keeps boot starts window-less).
                let auto = app.autolaunch();
                if auto.is_enabled().unwrap_or(false) {
                    let _ = auto.enable();
                }
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            if std::env::var("API_DESK_OPEN_MINI").is_ok() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
                    let _ = open_mini_window(&handle);
                });
            }
            if let Ok(secs) = std::env::var("API_DESK_QUIT_AFTER") {
                if let Ok(secs) = secs.parse::<u64>() {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                        quit_app(&handle);
                    });
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let resident = window
                        .app_handle()
                        .state::<TrayState>()
                        .resident
                        .load(Ordering::Relaxed);
                    if resident {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_paths,
            launch_startup,
            set_tray_resident,
            open_mini_window_command,
            show_main_window,
            vault::vault_status,
            vault::vault_init,
            vault::vault_unlock,
            vault::vault_lock,
            vault::vault_reset,
            vault::secret_store,
            vault::secret_delete,
            vault::secret_reveal,
            vault::secret_mask,
            vault::secret_masks,
            vault::secret_exists,
            vault::secret_copy,
            envfile::scan_project,
            envfile::import_env_values,
            envfile::env_preview,
            envfile::env_apply,
            envfile::write_text_file,
            export::export_credentials,
            http_test::test_http,
            usage::fetch_usage,
            model_import::fetch_models,
            monitors::monitor_probe,
            monitors::monitor_refresh,
            monitors::monitor_antigravity_accounts,
            monitors::monitor_antigravity_save_current,
            monitors::monitor_antigravity_switch_account,
            monitors::monitor_antigravity_delete_account,
        ])
        .run(tauri::generate_context!())
        .expect("error while running API Desk");
}

#[cfg(test)]
mod tests {
    use super::launched_from_autostart;

    #[test]
    fn detects_autostart_flag() {
        assert!(launched_from_autostart([
            "C:\\api-desk.exe".to_string(),
            "--autostart".to_string()
        ]));
        assert!(!launched_from_autostart(["api-desk.exe".to_string()]));
        assert!(!launched_from_autostart(Vec::<String>::new()));
        assert!(!launched_from_autostart(["--other".to_string()]));
    }
}
