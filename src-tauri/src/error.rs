use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Vault가 아직 생성되지 않았습니다")]
    VaultNotInitialized,
    #[error("Vault가 잠겨 있습니다")]
    VaultLocked,
    #[error("Vault 잠금 해제에 실패했습니다. 마스터 비밀번호를 확인하세요.")]
    VaultUnlockFailed,
    #[error("Vault에서 비밀 값을 찾을 수 없습니다")]
    SecretNotFound,
    #[error("Vault 오류: {0}")]
    Vault(String),
    #[error("잘못된 경로: {0}")]
    InvalidPath(String),
    #[error("잘못된 요청: {0}")]
    InvalidRequest(String),
    #[error("파일 오류: {0}")]
    Io(String),
    #[error("네트워크 오류: {0}")]
    Network(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::VaultNotInitialized => "VAULT_NOT_INITIALIZED",
            AppError::VaultLocked => "VAULT_LOCKED",
            AppError::VaultUnlockFailed => "VAULT_UNLOCK_FAILED",
            AppError::SecretNotFound => "SECRET_NOT_FOUND",
            AppError::Vault(_) => "VAULT_ERROR",
            AppError::InvalidPath(_) => "INVALID_PATH",
            AppError::InvalidRequest(_) => "INVALID_REQUEST",
            AppError::Io(_) => "IO_ERROR",
            AppError::Network(_) => "NETWORK_ERROR",
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        AppError::Io(value.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}
