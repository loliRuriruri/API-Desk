use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 파싱 상태 (Notepad TabState/WindowState 바이너리 해석 결과).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ParseStatus {
    Pending,
    Parsed,
    UnsupportedFormat,
    Corrupt,
    ReadError,
}

impl ParseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Parsed => "PARSED",
            Self::UnsupportedFormat => "UNSUPPORTED_FORMAT",
            Self::Corrupt => "CORRUPT",
            Self::ReadError => "READ_ERROR",
        }
    }
}

impl fmt::Display for ParseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ParseStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "PARSED" => Self::Parsed,
            "UNSUPPORTED_FORMAT" => Self::UnsupportedFormat,
            "CORRUPT" => Self::Corrupt,
            "READ_ERROR" => Self::ReadError,
            _ => Self::Pending,
        })
    }
}

/// 원본 소스 파일 유형.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceType {
    TabState,
    WindowState,
}

impl SourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TabState => "TAB_STATE",
            Self::WindowState => "WINDOW_STATE",
        }
    }
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SourceType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_uppercase().as_str() {
            "WINDOW_STATE" => Self::WindowState,
            _ => Self::TabState,
        })
    }
}

/// 논리적 메모장 탭/노트 엔티티.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotepadNote {
    pub id: String,
    pub source_id: String,
    pub display_title: String,
    pub original_path: Option<String>,
    pub is_saved: bool,
    pub is_deleted: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 특정 시점의 탭 스냅샷 메타데이터.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotepadSnapshot {
    pub id: String,
    pub note_id: String,
    pub content_hash: String,
    pub raw_snapshot_path: String,
    pub recovered_text: Option<String>,
    pub source_type: SourceType,
    pub source_filename: String,
    pub source_size: i64,
    pub parse_status: ParseStatus,
    pub notepad_format_version: Option<i64>,
    pub captured_at: String,
    pub created_at: String,
}

/// 창/세션 스냅샷 메타데이터 (WindowState).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotepadSession {
    pub id: String,
    pub source_filename: String,
    pub content_hash: String,
    pub raw_snapshot_path: String,
    pub captured_at: String,
    pub parse_status: ParseStatus,
    pub created_at: String,
}

/// 메모장 복구 설정.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotepadRescueSettings {
    pub enabled: bool,
    pub watch_enabled: bool,
    pub retention_days: u32,
    pub deleted_retention_days: u32,
    pub max_storage_bytes: u64,
    pub updated_at: String,
}

impl Default for NotepadRescueSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            watch_enabled: true,
            retention_days: 30,
            deleted_retention_days: 30,
            max_storage_bytes: 1_073_741_824, // 1 GB
            updated_at: String::new(),
        }
    }
}

/// 스토리지 및 통계 집계 데이터.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatistics {
    pub total_notes: i64,
    pub active_notes: i64,
    pub deleted_notes: i64,
    pub total_snapshots: i64,
    pub total_sessions: i64,
    pub total_snapshot_bytes: i64,
    pub oldest_snapshot_at: Option<String>,
    pub newest_snapshot_at: Option<String>,
}

/// 노트 조회용 필터 옵션.
#[derive(Debug, Clone, Default)]
pub struct NoteFilter {
    pub is_deleted: Option<bool>,
    pub query: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
