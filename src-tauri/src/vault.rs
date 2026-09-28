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
use crate::vault_autounlock::{self as autounlock, AutoUnlockState};

const CLIENT_PATH: &[u8] = b"api-desk";
const MIN_PASSWORD_LEN: usize = 4;

#[derive(Clone)]
pub struct AutoUnlockInfo {
    pub state: String,
    pub error: Option<String>,
}

impl Default for AutoUnlockInfo {
    fn default() -> Self {
        Self {
            state: "initializing".into(),
            error: None,
        }
    }
}

pub struct VaultState {
    stronghold: Mutex<Option<Stronghold>>,
    auto: Mutex<AutoUnlockInfo>,
}

impl Default for VaultState {
    fn default() -> Self {
        Self {
            stronghold: Mutex::new(None),
            auto: Mutex::new(AutoUnlockInfo::default()),
        }
    }
}

impl VaultState {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock_guard(&self) -> Result<std::sync::MutexGuard<'_, Option<Stronghold>>, AppError> {
        self.stronghold
            .lock()
            .map_err(|_| AppError::Vault("Vault 상태 잠금이 손상되었습니다".into()))
    }

    fn set_auto(&self, state: &str, error: Option<String>) {
        if let Ok(mut info) = self.auto.lock() {
            info.state = state.to_string();
            info.error = error;
        }
    }

    fn auto_info(&self) -> AutoUnlockInfo {
        self.auto
            .lock()
            .map(|info| info.clone())
            .unwrap_or_default()
    }

    pub fn is_unlocked(&self) -> bool {
        match self.stronghold.lock() {
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

pub enum AutoUnlockOutcome {
    Unlocked(Box<Stronghold>),
    /// (상태, 사용자 안내문) — 안내문에는 어떤 비밀도 포함하지 않는다.
    Pending(AutoUnlockState, String),
}

/// 자동 잠금해제 핵심 로직(AppHandle 비의존, 테스트 가능).
///
/// - 신규 설치: CSPRNG 비밀번호 생성 → Vault 생성 → DPAPI 보호 저장(실패 시 방금 만든
///   Vault를 롤백해 "열 수 없는 Vault"가 남지 않게 한다).
/// - 기존 설치: DPAPI 복호화 → Vault 해제. 어떤 실패에도 기존 파일을 건드리지 않는다.
pub fn auto_unlock_with(paths: &VaultPaths, blob_path: &std::path::Path) -> AutoUnlockOutcome {
    match autounlock::classify(paths.vault.exists(), blob_path.exists()) {
        None => {
            // 신규 설치: 사용자 입력 없이 고엔트로피 로컬 비밀번호를 만든다.
            let password = match autounlock::generate_password() {
                Ok(password) => password,
                Err(error) => {
                    return AutoUnlockOutcome::Pending(AutoUnlockState::Failed, error.to_string())
                }
            };
            let stronghold = match open_at(paths, password.as_str(), true) {
                Ok(stronghold) => stronghold,
                Err(error) => {
                    return AutoUnlockOutcome::Pending(AutoUnlockState::Failed, error.to_string())
                }
            };
            match autounlock::protect(password.as_bytes())
                .and_then(|protected| autounlock::save_blob_atomic(blob_path, &protected))
            {
                Ok(()) => AutoUnlockOutcome::Unlocked(Box::new(stronghold)),
                Err(error) => {
                    // 롤백: 방금 만든 Vault를 지워 다음 실행에서 다시 신규 설치로 시도한다.
                    let _ = std::fs::remove_file(&paths.vault);
                    let _ = std::fs::remove_file(&paths.salt);
                    AutoUnlockOutcome::Pending(AutoUnlockState::Failed, error.to_string())
                }
            }
        }
        Some(AutoUnlockState::Ready) => {
            let Some(blob) = autounlock::read_blob(blob_path) else {
                return AutoUnlockOutcome::Pending(
                    AutoUnlockState::Failed,
                    "자동 잠금해제 파일을 읽을 수 없습니다".into(),
                );
            };
            let password = match autounlock::unprotect(&blob) {
                Ok(password) => password,
                Err(error) => {
                    return AutoUnlockOutcome::Pending(AutoUnlockState::Failed, error.to_string())
                }
            };
            let Ok(text) = std::str::from_utf8(password.as_slice()) else {
                return AutoUnlockOutcome::Pending(
                    AutoUnlockState::Failed,
                    "자동 잠금해제 정보가 손상되었습니다".into(),
                );
            };
            match open_at(paths, text, false) {
                Ok(stronghold) => AutoUnlockOutcome::Unlocked(Box::new(stronghold)),
                Err(_) => AutoUnlockOutcome::Pending(
                    AutoUnlockState::Failed,
                    "저장된 자격 증명으로 Vault를 열 수 없습니다".into(),
                ),
            }
        }
        Some(AutoUnlockState::LegacyEnrollmentRequired) => {
            AutoUnlockOutcome::Pending(AutoUnlockState::LegacyEnrollmentRequired, String::new())
        }
        Some(AutoUnlockState::Inconsistent) => AutoUnlockOutcome::Pending(
            AutoUnlockState::Inconsistent,
            "자동 잠금해제 파일은 있는데 Vault가 없습니다. 자동으로 새로 만들지 않습니다.".into(),
        ),
        Some(AutoUnlockState::Failed) => AutoUnlockOutcome::Pending(
            AutoUnlockState::Failed,
            "자동 잠금해제를 사용할 수 없습니다".into(),
        ),
    }
}

/// 백그라운드 스레드 진입점: 자동 해제 후 상태를 반영하고 UI에 알린다.
pub fn run_auto_unlock(app: AppHandle) {
    let outcome = match vault_paths(&app).and_then(|paths| {
        let blob = autounlock::blob_path(&app)?;
        Ok((paths, blob))
    }) {
        Ok((paths, blob)) => auto_unlock_with(&paths, &blob),
        Err(error) => AutoUnlockOutcome::Pending(AutoUnlockState::Failed, error.to_string()),
    };
    let state = app.state::<VaultState>();
    match outcome {
        AutoUnlockOutcome::Unlocked(stronghold) => match state.lock_guard() {
            Ok(mut guard) => {
                *guard = Some(*stronghold);
                state.set_auto(AutoUnlockState::Ready.as_str(), None);
            }
            Err(error) => state.set_auto(AutoUnlockState::Failed.as_str(), Some(error.to_string())),
        },
        AutoUnlockOutcome::Pending(auto_state, reason) => {
            state.set_auto(
                auto_state.as_str(),
                if reason.is_empty() { None } else { Some(reason) },
            );
        }
    }
    let _ = tauri::Emitter::emit(&app, "vault-status-changed", ());
}

/// 1회 등록: 기존 마스터 비밀번호를 검증한 뒤 DPAPI 보호 블롭을 저장하고 열린 상태로 둔다.
#[tauri::command]
pub fn vault_enroll_auto_unlock(
    app: AppHandle,
    state: State<'_, VaultState>,
    password: String,
) -> Result<(), AppError> {
    let password = Zeroizing::new(password);
    let paths = vault_paths(&app)?;
    let blob_path = autounlock::blob_path(&app)?;
    // 1) 기존 Vault 검증 — 실패하면 어떤 파일도 만들지 않는다.
    let stronghold = open_at(&paths, password.as_str(), false)?;
    // 2) 보호 + 원자 저장
    let protected = autounlock::protect(password.as_bytes())?;
    autounlock::save_blob_atomic(&blob_path, &protected)?;
    // 3) 열린 상태 유지 + 상태 반영
    *state.lock_guard()? = Some(stronghold);
    state.set_auto(AutoUnlockState::Ready.as_str(), None);
    let _ = tauri::Emitter::emit(&app, "vault-status-changed", ());
    Ok(())
}

/// 자동 잠금해제 재시도(실패 화면의 [다시 시도]).
#[tauri::command]
pub fn vault_retry_auto_unlock(app: AppHandle) -> Result<(), AppError> {
    run_auto_unlock(app);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub state: String,
    /// initializing | ready | legacy | failed | inconsistent
    pub auto_unlock: String,
    pub auto_unlock_error: Option<String>,
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
    let auto = state.auto_info();
    Ok(VaultStatus {
        state: state_name.into(),
        auto_unlock: auto.state,
        auto_unlock_error: auto.error,
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
        Self {
            stronghold: Mutex::new(Some(stronghold)),
            auto: Mutex::new(AutoUnlockInfo::default()),
        }
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

    fn temp_paths(name: &str) -> (VaultPaths, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("api-desk-autounlock-{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = VaultPaths {
            vault: dir.join("vault.hold"),
            salt: dir.join("vault.salt"),
        };
        let blob = dir.join("vault.autounlock");
        (paths, blob)
    }

    #[test]
    fn fresh_install_generates_vault_and_autounlock_blob() {
        let (paths, blob) = temp_paths("fresh");
        let outcome = auto_unlock_with(&paths, &blob);
        assert!(matches!(outcome, AutoUnlockOutcome::Unlocked(_)));
        assert!(paths.vault.exists(), "vault 생성");
        assert!(blob.exists(), "자동 잠금해제 블롭 생성(사용자 입력 없음)");
        // 두 번째 호출은 DPAPI 블롭으로 무비밀번호 해제가 되어야 한다.
        let again = auto_unlock_with(&paths, &blob);
        assert!(matches!(again, AutoUnlockOutcome::Unlocked(_)));
        let _ = std::fs::remove_dir_all(paths.vault.parent().unwrap());
    }

    #[test]
    fn legacy_vault_requires_one_time_enrollment() {
        let (paths, blob) = temp_paths("legacy");
        // 기존 설치 재현: 비밀번호로 vault만 생성.
        let stronghold = open_at(&paths, "legacy-master-pass", true).unwrap();
        drop(stronghold);
        assert!(paths.vault.exists());
        assert!(!blob.exists());
        match auto_unlock_with(&paths, &blob) {
            AutoUnlockOutcome::Pending(AutoUnlockState::LegacyEnrollmentRequired, reason) => {
                assert!(reason.is_empty());
            }
            _ => panic!("레거시 등록 필요 상태여야 한다"),
        }
        assert!(!blob.exists(), "자동으로 블롭을 만들면 안 된다");
        let _ = std::fs::remove_dir_all(paths.vault.parent().unwrap());
    }

    #[test]
    fn wrong_legacy_password_does_not_create_blob() {
        let (paths, blob) = temp_paths("wrong-pass");
        let stronghold = open_at(&paths, "legacy-master-pass", true).unwrap();
        drop(stronghold);
        // 등록 커맨드의 핵심 단계: 열기 실패 → 기록 없음.
        assert!(open_at(&paths, "wrong-password", false).is_err());
        assert!(!blob.exists(), "잘못된 비밀번호는 블롭을 만들지 않는다");
        // 올바른 비밀번호 등록 → 자동 해제 가능.
        let stronghold = open_at(&paths, "legacy-master-pass", false).unwrap();
        let protected = autounlock::protect(b"legacy-master-pass").unwrap();
        autounlock::save_blob_atomic(&blob, &protected).unwrap();
        drop(stronghold);
        assert!(matches!(auto_unlock_with(&paths, &blob), AutoUnlockOutcome::Unlocked(_)));
        let _ = std::fs::remove_dir_all(paths.vault.parent().unwrap());
    }

    #[test]
    fn inconsistent_state_fails_closed_without_creating_vault() {
        let (paths, blob) = temp_paths("inconsistent");
        autounlock::save_blob_atomic(&blob, b"not-a-real-dpapi-blob").unwrap();
        match auto_unlock_with(&paths, &blob) {
            AutoUnlockOutcome::Pending(AutoUnlockState::Inconsistent, _) => {}
            _ => panic!("불일치 상태로 실패해야 한다"),
        }
        assert!(!paths.vault.exists(), "vault를 자동 생성하면 안 된다");
        let _ = std::fs::remove_dir_all(paths.vault.parent().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn corrupt_blob_fails_closed_and_keeps_vault() {
        let (paths, blob) = temp_paths("corrupt");
        let stronghold = open_at(&paths, "legacy-master-pass", true).unwrap();
        drop(stronghold);
        let mut protected = autounlock::protect(b"legacy-master-pass").unwrap();
        let middle = protected.len() / 2;
        protected[middle] ^= 0x33;
        autounlock::save_blob_atomic(&blob, &protected).unwrap();
        let before = std::fs::read(&paths.vault).unwrap();
        match auto_unlock_with(&paths, &blob) {
            AutoUnlockOutcome::Pending(AutoUnlockState::Failed, reason) => {
                assert!(!reason.is_empty(), "사용자 안내문 존재");
                assert!(!reason.to_lowercase().contains("password"), "비밀 노출 금지");
            }
            _ => panic!("손상된 블롭은 실패해야 한다"),
        }
        assert_eq!(std::fs::read(&paths.vault).unwrap(), before, "vault 유지");
        assert!(blob.exists(), "블롭 파일은 삭제하지 않는다");
        let _ = std::fs::remove_dir_all(paths.vault.parent().unwrap());
    }

    #[test]
    fn auto_unlock_status_dto_contains_no_secrets() {
        let status = VaultStatus {
            state: "unlocked".into(),
            auto_unlock: "ready".into(),
            auto_unlock_error: None,
            vault_path: "C:/test/vault.hold".into(),
            salt_path: "C:/test/vault.salt".into(),
        };
        let json = serde_json::to_string(&status).unwrap().to_lowercase();
        for forbidden in ["password", "secret", "dpapi-blob", "master"] {
            assert!(!json.contains(forbidden), "DTO에 {forbidden} 노출");
        }
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
