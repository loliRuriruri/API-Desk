// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let log_dir = std::path::PathBuf::from(local).join("com.apidesk.desktop");
            let _ = std::fs::create_dir_all(&log_dir);
            let log_file = log_dir.join("panic.log");
            let now = std::time::SystemTime::now();
            let msg = format!("[{now:?}] PANIC: {info}\n");
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)
                .and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(msg.as_bytes())
                });
        }
    }));
    api_desk_lib::run()
}
