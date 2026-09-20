use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_stronghold::{kdf::KeyDerivation, stronghold::Stronghold};
use zeroize::{Zeroize, Zeroizing};

use crate::error::AppError;

const CLIENT_PATH: &[u8] = b"api-desk";
const MIN_PASSWORD_LEN: usize = 4;

#[derive(Default)]
pub struct VaultState(Mutex<Option<Stronghold>>);

impl VaultState {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock_guard(&self) -> Result<std::sync::MutexGuard<'_, Option<Stronghold>>, AppError> {
        self.0
            .lock()
            .map_err(|_| AppError::Vault("Vault 상태 잠금이 손상되었습니다".into()))
    }

    pub fn is_unlocked(&self) -> bool {
        match self.0.lock() {
            Ok(guard) => guard.is_some(),
            Err(_) => false,
        }
    }

    pub fn with_stronghold<T>(
        &self,
        operation: impl FnOnce(&Stronghold) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let guard = self.lock_guard()?;
        let stronghold = guard.as_ref().ok_or(AppError::VaultLocked)?;
        operation(stronghold)
    }
}

pub struct VaultPaths {
    pub vault: PathBuf,
    pub salt: PathBuf,
}

pub fn vault_paths(app: &AppHandle) -> Result<VaultPaths, AppError> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::Io(format!("앱 데이터 폴더를 확인할 수 없습니다: {e}")))?;
    std::fs::create_dir_all(&dir)?;
    Ok(VaultPaths {
        vault: dir.join("vault.hold"),
        salt: dir.join("vault.salt"),
    })
}

pub fn open_at(paths: &VaultPaths, password: &str, create: bool) -> Result<Stronghold, AppError> {
    if create {
        if paths.vault.exists() {
            return Err(AppError::InvalidRequest(
                "Vault 파일이 이미 존재합니다".into(),
            ));
        }
        let key = Zeroizing::new(KeyDerivation::argon2(password, &paths.salt));
        let stronghold = Stronghold::new(&paths.vault, key.to_vec())
            .map_err(|e| AppError::Vault(e.to_string()))?;
        stronghold
            .create_client(CLIENT_PATH)
            .map_err(|e| AppError::Vault(e.to_string()))?;
        save(&stronghold)?;
        return Ok(stronghold);
    }
    if !paths.vault.exists() {
        return Err(AppError::VaultNotInitialized);
    }
    let key = Zeroizing::new(KeyDerivation::argon2(password, &paths.salt));
    let stronghold = Stronghold::new(&paths.vault, key.to_vec())
        .map_err(|_| AppError::VaultUnlockFailed)?;
    stronghold
        .load_client(CLIENT_PATH)
        .or_else(|err| match err {
            iota_stronghold::ClientError::ClientDataNotPresent => stronghold
                .create_client(CLIENT_PATH)
                .map_err(|e| AppError::Vault(e.to_string())),
            other => Err(AppError::Vault(other.to_string())),
        })?;
    Ok(stronghold)
}

fn open_stronghold(app: &AppHandle, password: &str) -> Result<Stronghold, AppError> {
    let paths = vault_paths(app)?;
    open_at(&paths, password, false)
}

fn store_of(
    stronghold: &Stronghold,
) -> Result<iota_stronghold::Store, AppError> {
    stronghold
        .get_client(CLIENT_PATH)
        .map(|client| client.store())
        .map_err(|e| match e {
            iota_stronghold::ClientError::ClientDataNotPresent => AppError::VaultLocked,
            other => AppError::Vault(other.to_string()),
        })
}

fn save(stronghold: &Stronghold) -> Result<(), AppError> {
    stronghold
        .save()
        .map_err(|e| AppError::Vault(format!("Vault 저장에 실패했습니다: {e}")))
}

pub fn read_secret_from(
    stronghold: &Stronghold,
    secret_id: &str,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let store = store_of(stronghold)?;
    let value = store
        .get(secret_id.as_bytes())
        .map_err(|e| AppError::Vault(e.to_string()))?
        .ok_or(AppError::SecretNotFound)?;
    Ok(Zeroizing::new(value))
}

pub fn read_secret(state: &VaultState, secret_id: &str) -> Result<Zeroizing<Vec<u8>>, AppError> {
    state.with_stronghold(|stronghold| read_secret_from(stronghold, secret_id))
}

pub fn read_secret_string(state: &VaultState, secret_id: &str) -> Result<Zeroizing<String>, AppError> {
    let bytes = read_secret(state, secret_id)?;
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|_| AppError::Vault("저장된 비밀 값이 올바른 UTF-8이 아닙니다".into()))?;
    Ok(Zeroizing::new(text))
}

pub fn write_secret_to(
    stronghold: &Stronghold,
    secret_id: &str,
    value: &str,
) -> Result<(), AppError> {
    let store = store_of(stronghold)?;
    store
        .insert(
            secret_id.as_bytes().to_vec(),
            value.as_bytes().to_vec(),
            None,
        )
        .map_err(|e| AppError::Vault(e.to_string()))?;
    save(stronghold)
}

pub fn write_secret(
    state: &VaultState,
    secret_id: &str,
    value: &str,
) -> Result<(), AppError> {
    state.with_stronghold(|stronghold| write_secret_to(stronghold, secret_id, value))
}

pub fn delete_secret(state: &VaultState, secret_id: &str) -> Result<(), AppError> {
    state.with_stronghold(|stronghold| {
        let store = store_of(stronghold)?;
        store
            .delete(secret_id.as_bytes())
            .map_err(|e| AppError::Vault(e.to_string()))?;
        save(stronghold)
    })
}

pub fn has_secret(state: &VaultState, secret_id: &str) -> bool {
    state
        .with_stronghold(|stronghold| {
            let store = store_of(stronghold)?;
            store
                .contains_key(secret_id.as_bytes())
                .map_err(|e| AppError::Vault(e.to_string()))
        })
        .unwrap_or(false)
}

pub fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let len = chars.len();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "••••".into();
    }
    let mut prefix_len = 0;
    for (index, c) in chars.iter().enumerate().take(12) {
        if *c == '-' {
            prefix_len = index + 1;
        }
    }
    let tail: String = chars[len - 4..].iter().collect();
    let dots = "•".repeat(((len.saturating_sub(prefix_len + 4)).min(12)).max(4));
    let prefix: String = chars[..prefix_len].iter().collect();
    format!("{prefix}{dots}{tail}")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub state: String,
    pub vault_path: String,
    pub salt_path: String,
}

#[tauri::command]
pub fn vault_status(app: AppHandle, state: State<'_, VaultState>) -> Result<VaultStatus, AppError> {
    let paths = vault_paths(&app)?;
    let state_name = if state.is_unlocked() {
        "unlocked"
    } else if paths.vault.exists() {
        "locked"
    } else {
        "uninitialized"
    };
    Ok(VaultStatus {
        state: state_name.into(),
        vault_path: paths.vault.to_string_lossy().to_string(),
        salt_path: paths.salt.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn vault_init(
    app: AppHandle,
    state: State<'_, VaultState>,
    password: String,
) -> Result<(), AppError> {
    let paths = vault_paths(&app)?;
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::InvalidRequest(format!(
            "마스터 비밀번호는 최소 {MIN_PASSWORD_LEN}자 이상이어야 합니다"
        )));
    }
    let mut password = password;
    let result = open_at(&paths, &password, true);
    password.zeroize();
    let stronghold = result?;
    *state.lock_guard()? = Some(stronghold);
    Ok(())
}

#[tauri::command]
pub fn vault_unlock(
    app: AppHandle,
    state: State<'_, VaultState>,
    password: String,
) -> Result<(), AppError> {
    let mut password = password;
    let result = open_stronghold(&app, &password);
    password.zeroize();
    let stronghold = result?;
    *state.lock_guard()? = Some(stronghold);
    Ok(())
}

#[tauri::command]
pub fn vault_lock(state: State<'_, VaultState>) -> Result<(), AppError> {
    *state.lock_guard()? = None;
    Ok(())
}

#[tauri::command]
pub fn vault_reset(app: AppHandle, state: State<'_, VaultState>, confirm: String) -> Result<(), AppError> {
    if confirm != "RESET" {
        return Err(AppError::InvalidRequest(
            "Vault 초기화를 확인하려면 RESET을 입력하세요".into(),
        ));
    }
    let paths = vault_paths(&app)?;
    *state.lock_guard()? = None;
    if paths.vault.exists() {
        std::fs::remove_file(&paths.vault)?;
    }
    if paths.salt.exists() {
        std::fs::remove_file(&paths.salt)?;
    }
    Ok(())
}

#[tauri::command]
pub fn secret_store(
    state: State<'_, VaultState>,
    secret_id: String,
    value: String,
) -> Result<(), AppError> {
    if secret_id.trim().is_empty() {
        return Err(AppError::InvalidRequest("secret id가 필요합니다".into()));
    }
    if value.is_empty() {
        return Err(AppError::InvalidRequest("비밀 값이 비어 있습니다".into()));
    }
    write_secret(&state, &secret_id, &value)
}

#[tauri::command]
pub fn secret_delete(state: State<'_, VaultState>, secret_id: String) -> Result<(), AppError> {
    delete_secret(&state, &secret_id)
}

#[tauri::command]
pub fn secret_reveal(state: State<'_, VaultState>, secret_id: String) -> Result<String, AppError> {
    let value = read_secret_string(&state, &secret_id)?;
    Ok(value.to_string())
}

#[tauri::command]
pub fn secret_mask(state: State<'_, VaultState>, secret_id: String) -> Result<String, AppError> {
    let value = read_secret_string(&state, &secret_id)?;
    Ok(mask_secret(value.as_str()))
}

#[tauri::command]
pub fn secret_masks(
    state: State<'_, VaultState>,
    secret_ids: Vec<String>,
) -> Result<HashMap<String, String>, AppError> {
    let mut result = HashMap::new();
    state.with_stronghold(|stronghold| {
        for secret_id in &secret_ids {
            if let Ok(value) = read_secret_from(stronghold, secret_id) {
                if let Ok(text) = std::str::from_utf8(value.as_slice()) {
                    result.insert(secret_id.clone(), mask_secret(text));
                }
            }
        }
        Ok(())
    })?;
    Ok(result)
}

#[tauri::command]
pub fn secret_exists(state: State<'_, VaultState>, secret_id: String) -> bool {
    has_secret(&state, &secret_id)
}

pub fn should_clear_clipboard(current: &str, copied: &str) -> bool {
    !copied.is_empty() && current == copied
}

#[tauri::command]
pub async fn secret_copy(
    app: AppHandle,
    state: State<'_, VaultState>,
    secret_id: String,
    clear_after_secs: u64,
) -> Result<(), AppError> {
    let mut value = read_secret_string(&state, &secret_id)?.to_string();
    app.clipboard()
        .write_text(value.clone())
        .map_err(|e| AppError::Io(format!("클립보드 저장에 실패했습니다: {e}")))?;
    if clear_after_secs > 0 {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(clear_after_secs)).await;
            let clipboard = app_handle.clipboard();
            if let Ok(current) = clipboard.read_text() {
                if should_clear_clipboard(&current, &value) {
                    let _ = clipboard.write_text(String::new());
                }
            }
            value.zeroize();
        });
    } else {
        value.zeroize();
    }
    Ok(())
}

#[cfg(test)]
impl VaultState {
    pub fn with_test_stronghold(stronghold: Stronghold) -> Self {
        Self(Mutex::new(Some(stronghold)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_long_keys() {
        assert_eq!(mask_secret("sk-proj-testfakeabcdefghA82F"), "sk-proj-••••••••••••A82F");
        assert_eq!(mask_secret("sk-1234567890abcd4821"), "sk-••••••••••••4821");
        assert_eq!(mask_secret("abcdefgh"), "••••efgh");
    }

    #[test]
    fn masks_short_or_empty_keys() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("ab"), "••••");
        assert_eq!(mask_secret("abcd"), "••••");
    }

    #[test]
    fn clipboard_is_only_cleared_when_it_still_holds_the_copied_value() {
        assert!(should_clear_clipboard("test-secret-not-a-real-key", "test-secret-not-a-real-key"));
        assert!(!should_clear_clipboard("something the user copied later", "test-secret-not-a-real-key"));
        assert!(!should_clear_clipboard("", "test-secret-not-a-real-key"));
        assert!(!should_clear_clipboard("anything", ""));
    }

    #[test]
    fn vault_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("api-desk-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let vault_path = dir.join("vault.hold");
        let salt_path = dir.join("vault.salt");
        let _ = std::fs::remove_file(&vault_path);
        let _ = std::fs::remove_file(&salt_path);

        let key = Zeroizing::new(KeyDerivation::argon2("correct horse battery", &salt_path));
        let stronghold = Stronghold::new(&vault_path, key.to_vec()).unwrap();
        stronghold.create_client(CLIENT_PATH).unwrap();
        let store = stronghold.get_client(CLIENT_PATH).unwrap().store();
        store
            .insert(
                b"secret-1".to_vec(),
                b"test-secret-not-a-real-api-key".to_vec(),
                None,
            )
            .unwrap();
        stronghold.save().unwrap();

        let reopened = Stronghold::new(
            &vault_path,
            Zeroizing::new(KeyDerivation::argon2("correct horse battery", &salt_path)).to_vec(),
        )
        .unwrap();
        reopened.load_client(CLIENT_PATH).unwrap();
        let value = reopened
            .get_client(CLIENT_PATH)
            .unwrap()
            .store()
            .get(b"secret-1")
            .unwrap()
            .unwrap();
        assert_eq!(value, b"test-secret-not-a-real-api-key".to_vec());

        let wrong_password = Stronghold::new(
            &vault_path,
            Zeroizing::new(KeyDerivation::argon2("wrong password", &salt_path)).to_vec(),
        );
        assert!(wrong_password.is_err());

        let _ = std::fs::remove_file(&vault_path);
        let _ = std::fs::remove_file(&salt_path);
    }
}
