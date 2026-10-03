use std::env;
use std::path::PathBuf;

use api_desk_lib::notepad_rescue::restore::*;
use api_desk_lib::notepad_rescue::watcher::default_notepad_local_state_dir;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

fn main() {
    tauri::async_runtime::block_on(async_main());
}

async fn async_main() {
    let args: Vec<String> = env::args().collect();
    let action = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    let local_data = env::var("LOCALAPPDATA").expect("LOCALAPPDATA not set");
    let appdata = env::var("APPDATA").expect("APPDATA not set");

    let storage_root = PathBuf::from(&local_data)
        .join("com.apidesk.desktop")
        .join("notepad_rescue");
    let db_path = PathBuf::from(&appdata)
        .join("com.apidesk.desktop")
        .join("apidb.db");

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(SqliteConnectOptions::new().filename(&db_path))
        .await
        .expect("DB connect failed");

    let notepad_local_state =
        default_notepad_local_state_dir().expect("Notepad local state not found");

    match action {
        "restore" => {
            let note_id = args.get(2).expect("note_id required");
            let snap_id = args.get(3).filter(|s| !s.is_empty()).map(|s| s.as_str());
            match restore_native_tab(&pool, &storage_root, note_id, snap_id).await {
                Ok(rep) => {
                    println!("{}", serde_json::to_string(&rep).unwrap());
                }
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                }
            }
        }
        "rollback" => match rollback_last_restore(&storage_root, &notepad_local_state) {
            Ok(rep) => {
                println!("{}", serde_json::to_string(&rep).unwrap());
            }
            Err(e) => {
                eprintln!("ERROR: {e}");
                std::process::exit(1);
            }
        },
        "can_rollback" => {
            println!("{}", can_rollback(&storage_root));
        }
        _ => {
            eprintln!(
                "Usage: nr7_restore_cli [restore <note_id> [snap_id] | rollback | can_rollback]"
            );
        }
    }
}
