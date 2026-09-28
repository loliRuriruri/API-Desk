//! Windows DPAPI(CurrentUser) 기반 자동 잠금해제 블롭.
//!
//! - vault.hold의 마스터 비밀번호를 `CryptProtectData`(CurrentUser 범위)로 보호해
//!   `vault.autounlock` 파일에 원자적으로 저장한다.
//! - 복호화한 평문 비밀번호는 `Zeroizing`으로 즉시 0화하며, 프런트엔드/로그/에러
//!   메시지에 절대 노출하지 않는다.
//! - 신규 설치는 OS CSPRNG(BCryptGenRandom)로 생성한 고엔트로피 비밀번호를 사용한다.
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::error::AppError;

pub const BLOB_FILE: &str = "vault.autounlock";
/// 앱 고정 추가 엔트로피(비밀 아님, 블롭 결합 목적).
const ENTROPY: &[u8] = b"api-desk-vault-autounlock-v1";
const PASSWORD_BYTES: usize = 32;

/// 자동 잠금해제 시작 상태 분류.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AutoUnlockState {
    /// vault + blob 존재: DPAPI로 자동 해제 시도.
    Ready,
    /// vault만 존재: 1회 비밀번호 등록 필요.
    LegacyEnrollmentRequired,
    /// vault 없음 + blob 존재: 불일치(새로 만들지 않음).
    Inconsistent,
    /// blob이 있으나 복호화/해제 실패.
    Failed,
}

impl AutoUnlockState {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoUnlockState::Ready => "ready",
            AutoUnlockState::LegacyEnrollmentRequired => "legacy",
            AutoUnlockState::Inconsistent => "inconsistent",
            AutoUnlockState::Failed => "failed",
        }
    }
}

/// (vault 존재, blob 존재) → 시작 상태. None이면 신규 설치(자동 생성).
pub fn classify(vault_exists: bool, blob_exists: bool) -> Option<AutoUnlockState> {
    match (vault_exists, blob_exists) {
        (true, true) => Some(AutoUnlockState::Ready),
        (true, false) => Some(AutoUnlockState::LegacyEnrollmentRequired),
        (false, true) => Some(AutoUnlockState::Inconsistent),
        (false, false) => None,
    }
}

pub fn blob_path(app: &tauri::AppHandle) -> Result<PathBuf, AppError> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| AppError::Io(format!("데이터 폴더를 확인할 수 없습니다: {error}")))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(BLOB_FILE))
}

/// 블롭을 원자적으로 저장한다(같은 볼륨에서 임시 파일 → 교체).
pub fn save_blob_atomic(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|error| AppError::Io(format!("자동 잠금해제 파일을 쓸 수 없습니다: {error}")))?;
    replace_file(&tmp, path)
        .map_err(|error| AppError::Io(format!("자동 잠금해제 파일을 저장할 수 없습니다: {error}")))?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

pub fn read_blob(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok().filter(|bytes| !bytes.is_empty())
}

// ---------------------------------------------------------------- DPAPI

#[cfg(windows)]
mod dpapi {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct DataBlob {
        pub cb_data: u32,
        pub pb_data: *mut u8,
    }

    extern "system" {
        pub fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        pub fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        pub fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
}

/// 마스터 비밀번호를 DPAPI(CurrentUser)로 보호한다.
#[cfg(windows)]
pub fn protect(plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
    use zeroize::Zeroize;
    let mut entropy = ENTROPY.to_vec();
    let mut input = plaintext.to_vec();
    let result = (|| -> Result<Vec<u8>, AppError> {
        let in_blob = dpapi::DataBlob {
            cb_data: input.len() as u32,
            pb_data: input.as_mut_ptr(),
        };
        let entropy_blob = dpapi::DataBlob {
            cb_data: entropy.len() as u32,
            pb_data: entropy.as_mut_ptr(),
        };
        let mut out = dpapi::DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        let ok = unsafe {
            dpapi::CryptProtectData(
                &in_blob,
                std::ptr::null(),
                &entropy_blob,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dpapi::CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.pb_data.is_null() {
            return Err(AppError::Vault(
                "Windows 자동 잠금해제 보호에 실패했습니다".into(),
            ));
        }
        let bytes = unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() };
        unsafe {
            dpapi::LocalFree(out.pb_data as *mut std::ffi::c_void);
        }
        Ok(bytes)
    })();
    input.zeroize();
    entropy.zeroize();
    result
}

/// DPAPI 블롭을 복호화한다(평문은 Zeroizing으로 반환).
#[cfg(windows)]
pub fn unprotect(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, AppError> {
    use zeroize::Zeroize;
    let mut entropy = ENTROPY.to_vec();
    let mut input = blob.to_vec();
    let result = (|| -> Result<Zeroizing<Vec<u8>>, AppError> {
        let in_blob = dpapi::DataBlob {
            cb_data: input.len() as u32,
            pb_data: input.as_mut_ptr(),
        };
        let entropy_blob = dpapi::DataBlob {
            cb_data: entropy.len() as u32,
            pb_data: entropy.as_mut_ptr(),
        };
        let mut out = dpapi::DataBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        let ok = unsafe {
            dpapi::CryptUnprotectData(
                &in_blob,
                std::ptr::null_mut(),
                &entropy_blob,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dpapi::CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.pb_data.is_null() {
            return Err(AppError::Vault(
                "자동 잠금해제 정보를 복호화할 수 없습니다".into(),
            ));
        }
        let bytes = Zeroizing::new(
            unsafe { std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec() },
        );
        // DPAPI가 할당한 평문 버퍼도 즉시 지운다.
        unsafe {
            let slice = std::slice::from_raw_parts_mut(out.pb_data, out.cb_data as usize);
            slice.zeroize();
            dpapi::LocalFree(out.pb_data as *mut std::ffi::c_void);
        }
        Ok(bytes)
    })();
    input.zeroize();
    entropy.zeroize();
    result
}

#[cfg(not(windows))]
pub fn protect(_plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
    Err(AppError::Vault("이 플랫폼은 자동 잠금해제를 지원하지 않습니다".into()))
}

#[cfg(not(windows))]
pub fn unprotect(_blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, AppError> {
    Err(AppError::Vault("이 플랫폼은 자동 잠금해제를 지원하지 않습니다".into()))
}

// ---------------------------------------------------------------- 비밀번호 생성/변환

/// OS CSPRNG(BCryptGenRandom)로 고엔트로피 로컬 비밀번호를 만든다.
pub fn generate_password() -> Result<Zeroizing<String>, AppError> {
    let mut bytes = Zeroizing::new(vec![0u8; PASSWORD_BYTES]);
    fill_random(bytes.as_mut_slice())?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.as_slice());
    Ok(Zeroizing::new(encoded))
}

#[cfg(windows)]
fn fill_random(buffer: &mut [u8]) -> Result<(), AppError> {
    use std::ffi::c_void;
    extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let ok = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if ok != 0 {
        return Err(AppError::Vault("난수 생성에 실패했습니다".into()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn fill_random(buffer: &mut [u8]) -> Result<(), AppError> {
    // 테스트/비Windows 환경에서는 시간 기반으로 채우지 않고 실패한다(보안 우선).
    let _ = buffer;
    Err(AppError::Vault("이 플랫폼은 난수 생성을 지원하지 않습니다".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_startup_matrix() {
        assert_eq!(classify(true, true), Some(AutoUnlockState::Ready));
        assert_eq!(
            classify(true, false),
            Some(AutoUnlockState::LegacyEnrollmentRequired)
        );
        assert_eq!(classify(false, true), Some(AutoUnlockState::Inconsistent));
        assert_eq!(classify(false, false), None, "신규 설치는 자동 생성");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_current_user() {
        let secret = b"correct horse battery staple";
        let blob = protect(secret).unwrap();
        assert_ne!(blob, secret, "블롭에 평문이 그대로 있으면 안 된다");
        let restored = unprotect(&blob).unwrap();
        assert_eq!(restored.as_slice(), secret);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_tampered_blob() {
        let mut blob = protect(b"secret-value").unwrap();
        let middle = blob.len() / 2;
        blob[middle] ^= 0x5A;
        assert!(unprotect(&blob).is_err(), "변조된 블롭은 실패해야 한다(무결성)");
    }

    #[cfg(windows)]
    #[test]
    fn generated_password_is_random_and_not_in_blob() {
        let first = generate_password().unwrap();
        let second = generate_password().unwrap();
        assert_ne!(first.as_str(), second.as_str(), "매번 다른 비밀번호");
        assert!(first.len() >= 40, "32바이트 base64 = 44자 이상");
        let blob = protect(first.as_bytes()).unwrap();
        let needle = first.as_bytes();
        let contained = blob.windows(needle.len()).any(|window| window == needle);
        assert!(!contained, "블롭에 평문이 포함되면 안 된다");
    }

    #[test]
    fn saves_blob_atomically_and_reads_back() {
        let dir = std::env::temp_dir().join("api-desk-autounlock-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault.autounlock");
        save_blob_atomic(&path, b"first").unwrap();
        assert_eq!(read_blob(&path).as_deref(), Some(b"first".as_slice()));
        // 기존 파일이 있어도 교체된다.
        save_blob_atomic(&path, b"second").unwrap();
        assert_eq!(read_blob(&path).as_deref(), Some(b"second".as_slice()));
        assert!(!path.with_extension("tmp").exists(), "임시 파일 정리");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_blob_is_treated_as_missing() {
        let dir = std::env::temp_dir().join("api-desk-autounlock-tests-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.autounlock");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(read_blob(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
