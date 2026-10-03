use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::models::{NotepadNote, NotepadRescueSettings, NotepadSnapshot, ParseStatus};
use super::parser::parse_tab_state;
use super::repository;
use crate::error::AppError;

/// 복구 신뢰도 수준 (Recovery Confidence Status)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecoveryConfidence {
    /// 파싱된 텍스트와 원시(RAW) 백업 파일이 모두 온전히 보존됨
    Full,
    /// DB 메타데이터 및 복구 텍스트는 존재하나 디스크의 원시 CAS 블롭이 유실됨
    TextOnly,
    /// 원시 CAS 블롭 파일은 보존되었으나 파서 미지원/손상으로 텍스트 미추출 상태
    RawOnly,
    /// 텍스트와 원시 블롭 모두 사용할 수 없음
    Unavailable,
}

/// 단일 노트 복구 상세 정보 DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRecoveryDetailDto {
    pub note: NotepadNote,
    pub latest_snapshot: Option<NotepadSnapshot>,
    pub latest_valid_snapshot: Option<NotepadSnapshot>,
    pub latest_confidence: RecoveryConfidence,
    pub valid_confidence: Option<RecoveryConfidence>,
    pub is_latest_corrupt: bool,
}

/// 재해 복구 패키지 내보내기 결과 리포트
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPackageReport {
    pub package_path: String,
    pub total_notes_exported: usize,
    pub total_blobs_copied: usize,
    pub manifest_file: String,
    pub created_at: String,
}

/// 로컬 진단 보고서 DTO (본문 및 시크릿 배제)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReportDto {
    pub rescue_version: String,
    pub schema_version: i64,
    pub note_count: i64,
    pub active_note_count: i64,
    pub deleted_note_count: i64,
    pub snapshot_count: i64,
    pub cas_blob_count: i64,
    pub parse_status_counts: HashMap<String, i64>,
    pub missing_blobs_count: i64,
    pub orphan_blobs_count: i64,
    pub physical_storage_bytes: u64,
    pub max_storage_bytes: u64,
    pub has_quota_pressure: bool,
    pub watcher_enabled: bool,
    pub last_successful_snapshot_at: Option<String>,
    pub timestamp: String,
}

/// 원시 블롭 스캔 결과 (Scenario G 진단 지원)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawBlobScanResult {
    pub hash: String,
    pub file_size: u64,
    pub is_valid_tabstate: bool,
    pub format_version: Option<i64>,
    pub recovered_title: Option<String>,
    pub recovered_text_preview: Option<String>,
    pub parse_error: Option<String>,
}

/// Windows 예약 DOS 장치 이름 판정
pub fn is_reserved_dos_name(name: &str) -> bool {
    let base = name.trim();
    let upper = base.to_ascii_uppercase();
    let name_part = upper.split('.').next().unwrap_or("");
    matches!(
        name_part,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 내보내기용 안전한 파일명 생성
/// - Windows 금지 문자 및 제어문자 필터링
/// - 예약 DOS 장치 이름 충돌 회피 (_ 접두사 부여)
/// - 최대 50자 UTF-8 문자 경계 보존 절삭
/// - 빈 제목의 경우 "Untitled" 대체
pub fn sanitize_export_filename(title: &str, timestamp: i64) -> String {
    let safe_title: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            other => other,
        })
        .collect();

    let trimmed = safe_title.trim();
    let base_title = if trimmed.is_empty() {
        "Untitled"
    } else {
        trimmed
    };

    let bounded_title: String = if base_title.chars().count() > 50 {
        base_title.chars().take(50).collect()
    } else {
        base_title.to_string()
    };

    let final_title = if is_reserved_dos_name(&bounded_title) {
        format!("_{bounded_title}_")
    } else {
        bounded_title
    };

    format!("NotepadRescue_{final_title}_{timestamp}.txt")
}

/// 파일명 충돌 방지: 파일이 이미 존재하면 " (1)", " (2)" 접미사를 부여하여 덮어쓰기를 원천 방지합니다.
pub fn resolve_collision_path(target_path: &Path) -> PathBuf {
    if !target_path.exists() {
        return target_path.to_path_buf();
    }

    let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = target_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "export".to_string());
    let ext = target_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut counter = 1;
    loop {
        let candidate = parent.join(format!("{stem} ({counter}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
        if counter > 10000 {
            // 극한의 상황 대비 타임스탬프 나노초 폴백
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            return parent.join(format!("{stem}_{nanos}{ext}"));
        }
    }
}

/// 원자적 TXT 파일 쓰기 (.tmp 생성 -> 데이터 플러시 -> sync_all -> rename)
pub fn write_atomic_txt(target_path: &Path, content: &str) -> Result<PathBuf, AppError> {
    let resolved_path = resolve_collision_path(target_path);

    if let Some(parent) = resolved_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            AppError::Io(format!(
                "내보내기 디렉터리 생성 실패: {} ({e})",
                parent.display()
            ))
        })?;
    }

    let parent = resolved_path.parent().unwrap_or_else(|| Path::new("."));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(
        "{}.{}.tmp",
        resolved_path.file_name().unwrap().to_string_lossy(),
        nanos
    ));

    let mut file = File::create(&tmp_path).map_err(|e| {
        AppError::Io(format!(
            "임시 내보내기 파일 생성 실패: {} ({e})",
            tmp_path.display()
        ))
    })?;

    // UTF-8 바이트열 그대로 기록 (줄바꿈 변경/정규화 없이 원문 그대로 보존)
    file.write_all(content.as_bytes()).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!("내보내기 파일 쓰기 실패: {e}"))
    })?;

    file.sync_all().map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!("내보내기 디스크 플러시 실패: {e}"))
    })?;

    drop(file);

    fs::rename(&tmp_path, &resolved_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!(
            "원자적 파일 교체 실패: {} -> {} ({e})",
            tmp_path.display(),
            resolved_path.display()
        ))
    })?;

    Ok(resolved_path)
}

/// 내보내기 추적용 사이드카 메타데이터 JSON 파일 기록
pub fn write_export_sidecar(
    txt_path: &Path,
    note_id: &str,
    snapshot_id: &str,
    captured_at: &str,
    content_hash: &str,
) -> Result<PathBuf, AppError> {
    let sidecar_path = PathBuf::from(format!("{}.meta.json", txt_path.to_string_lossy()));
    let payload = serde_json::json!({
        "schemaVersion": "1.0",
        "noteId": note_id,
        "snapshotId": snapshot_id,
        "capturedAt": captured_at,
        "contentHash": content_hash,
        "exportedAt": chrono_now_iso(),
        "exportedFilePath": txt_path.to_string_lossy().to_string(),
    });

    let json_str = serde_json::to_string_pretty(&payload)
        .map_err(|e| AppError::InvalidRequest(format!("메타데이터 직렬화 실패: {e}")))?;

    write_atomic_txt(&sidecar_path, &json_str)?;
    Ok(sidecar_path)
}

/// 스냅샷의 복구 신뢰도(Recovery Confidence)를 판정합니다.
pub fn evaluate_recovery_confidence(
    snapshot: &NotepadSnapshot,
    storage_root: &Path,
) -> RecoveryConfidence {
    let has_text = snapshot
        .recovered_text
        .as_ref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);

    let blob_path = storage_root.join(&snapshot.raw_snapshot_path);
    let blob_exists = blob_path.is_file();

    match (has_text, blob_exists) {
        (true, true) => RecoveryConfidence::Full,
        (true, false) => RecoveryConfidence::TextOnly,
        (false, true) => RecoveryConfidence::RawOnly,
        (false, false) => RecoveryConfidence::Unavailable,
    }
}

/// 노트의 최신 스냅샷 및 최신 유효(PARSED) 스냅샷을 조회하여 복구 상세 정보를 생성합니다.
pub async fn get_note_recovery_detail(
    pool: &SqlitePool,
    storage_root: &Path,
    note_id: &str,
) -> Result<NoteRecoveryDetailDto, AppError> {
    let note = repository::get_note(pool, note_id)
        .await?
        .ok_or_else(|| AppError::InvalidRequest("노트를 찾을 수 없습니다".to_string()))?;

    // 1. 최신 스냅샷
    let latest_row = sqlx::query(
        "SELECT id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type,
                source_filename, source_size, parse_status, notepad_format_version, captured_at, created_at
         FROM notepad_snapshots
         WHERE note_id = ?1
         ORDER BY captured_at DESC, rowid DESC
         LIMIT 1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("최신 스냅샷 조회 실패: {e}")))?;

    let latest_snapshot = latest_row.and_then(|r| repository::map_snapshot_row(&r).ok());

    // 2. 최신 성공 파싱(PARSED) 스냅샷
    let valid_row = sqlx::query(
        "SELECT id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type,
                source_filename, source_size, parse_status, notepad_format_version, captured_at, created_at
         FROM notepad_snapshots
         WHERE note_id = ?1 AND parse_status = 'PARSED' AND recovered_text IS NOT NULL
         ORDER BY captured_at DESC, rowid DESC
         LIMIT 1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("최신 유효 스냅샷 조회 실패: {e}")))?;

    let latest_valid_snapshot = valid_row.and_then(|r| repository::map_snapshot_row(&r).ok());

    let latest_confidence = if let Some(ref s) = latest_snapshot {
        evaluate_recovery_confidence(s, storage_root)
    } else {
        RecoveryConfidence::Unavailable
    };

    let valid_confidence = latest_valid_snapshot
        .as_ref()
        .map(|s| evaluate_recovery_confidence(s, storage_root));

    let is_latest_corrupt = if let Some(ref s) = latest_snapshot {
        s.parse_status == ParseStatus::Corrupt
            || s.parse_status == ParseStatus::UnsupportedFormat
            || s.parse_status == ParseStatus::ReadError
    } else {
        false
    };

    Ok(NoteRecoveryDetailDto {
        note,
        latest_snapshot,
        latest_valid_snapshot,
        latest_confidence,
        valid_confidence,
        is_latest_corrupt,
    })
}

/// 재해 복구 패키지(Disaster Recovery Package) 로컬 내보내기
/// - manifest.json
/// - notes/<safe-title>.txt (최신 유효 텍스트)
/// - raw/<hash>.bin (참조된 물리 원시 스냅샷 블롭)
pub async fn export_recovery_package(
    pool: &SqlitePool,
    storage_root: &Path,
    destination_dir: &Path,
) -> Result<RecoveryPackageReport, AppError> {
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let package_dir_name = format!("NotepadRescue_RecoveryPackage_{now_ts}");
    let package_root = destination_dir.join(&package_dir_name);
    let notes_dir = package_root.join("notes");
    let raw_dir = package_root.join("raw");

    fs::create_dir_all(&notes_dir).map_err(|e| {
        AppError::Io(format!(
            "패키지 notes 디렉터리 생성 실패: {} ({e})",
            notes_dir.display()
        ))
    })?;
    fs::create_dir_all(&raw_dir).map_err(|e| {
        AppError::Io(format!(
            "패키지 raw 디렉터리 생성 실패: {} ({e})",
            raw_dir.display()
        ))
    })?;

    let notes = repository::list_notes(pool, &super::models::NoteFilter::default()).await?;
    let mut manifest_notes = Vec::new();
    let mut total_notes_exported = 0;
    let mut total_blobs_copied = 0;

    for note in &notes {
        // 최신 유효 스냅샷 선호
        let detail = get_note_recovery_detail(pool, storage_root, &note.id).await?;
        let chosen_snapshot = detail.latest_valid_snapshot.or(detail.latest_snapshot);

        if let Some(snapshot) = chosen_snapshot {
            let mut rel_txt_path = None;
            if let Some(ref text) = snapshot.recovered_text {
                let safe_filename = sanitize_export_filename(&note.display_title, now_ts as i64);
                let txt_dest = notes_dir.join(&safe_filename);
                if let Ok(final_path) = write_atomic_txt(&txt_dest, text) {
                    rel_txt_path = Some(format!(
                        "notes/{}",
                        final_path.file_name().unwrap().to_string_lossy()
                    ));
                    total_notes_exported += 1;
                }
            }

            // 원시 블롭 복사
            let mut rel_raw_path = None;
            let src_blob = storage_root.join(&snapshot.raw_snapshot_path);
            if src_blob.is_file() {
                let dest_blob = raw_dir.join(format!("{}.bin", snapshot.content_hash));
                if !dest_blob.exists() {
                    if fs::copy(&src_blob, &dest_blob).is_ok() {
                        total_blobs_copied += 1;
                    }
                }
                rel_raw_path = Some(format!("raw/{}.bin", snapshot.content_hash));
            }

            manifest_notes.push(serde_json::json!({
                "noteId": note.id,
                "displayTitle": note.display_title,
                "originalPath": note.original_path,
                "isSaved": note.is_saved,
                "isDeleted": note.is_deleted,
                "snapshotId": snapshot.id,
                "capturedAt": snapshot.captured_at,
                "contentHash": snapshot.content_hash,
                "parseStatus": snapshot.parse_status.as_str(),
                "notepadFormatVersion": snapshot.notepad_format_version,
                "relativeTxtPath": rel_txt_path,
                "relativeRawBlobPath": rel_raw_path,
            }));
        }
    }

    let manifest = serde_json::json!({
        "schemaVersion": "1.0",
        "createdAt": chrono_now_iso(),
        "exportScope": "FULL_LOCAL_DISASTER_RECOVERY",
        "notes": manifest_notes,
    });

    let manifest_path = package_root.join("manifest.json");
    let manifest_str = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::InvalidRequest(format!("매니페스트 직렬화 실패: {e}")))?;

    write_atomic_txt(&manifest_path, &manifest_str)?;

    Ok(RecoveryPackageReport {
        package_path: package_root.to_string_lossy().to_string(),
        total_notes_exported,
        total_blobs_copied,
        manifest_file: manifest_path.to_string_lossy().to_string(),
        created_at: chrono_now_iso(),
    })
}

/// 시스템 진단 보고서 생성 (Section 25 요구사항: 본문 및 시크릿 배제)
pub async fn get_diagnostic_report(
    pool: &SqlitePool,
    storage_root: &Path,
    settings: &NotepadRescueSettings,
) -> Result<DiagnosticReportDto, AppError> {
    let note_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notepad_notes")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let active_note_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notepad_notes WHERE is_deleted = 0")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let deleted_note_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notepad_notes WHERE is_deleted = 1")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let snapshot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notepad_snapshots")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let rows = sqlx::query(
        "SELECT parse_status, COUNT(*) as cnt FROM notepad_snapshots GROUP BY parse_status",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut parse_status_counts = HashMap::new();
    for row in rows {
        use sqlx::Row;
        let st: String = row.try_get("parse_status").unwrap_or_default();
        let cnt: i64 = row.try_get("cnt").unwrap_or(0);
        parse_status_counts.insert(st, cnt);
    }

    let storage_details = super::gc::get_storage_details(pool, storage_root, settings).await?;

    // 유실된 블롭 수 산출
    let hashes: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT content_hash FROM notepad_snapshots")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let mut missing_blobs_count = 0;
    for h in hashes {
        let p = storage_root.join(super::snapshot::relative_blob_path(&h));
        if !p.is_file() {
            missing_blobs_count += 1;
        }
    }

    let last_successful_snapshot_at: Option<String> = sqlx::query_scalar(
        "SELECT captured_at FROM notepad_snapshots WHERE parse_status = 'PARSED' ORDER BY captured_at DESC LIMIT 1"
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    Ok(DiagnosticReportDto {
        rescue_version: "1.0.0".to_string(),
        schema_version: 5,
        note_count,
        active_note_count,
        deleted_note_count,
        snapshot_count,
        cas_blob_count: storage_details.physical_blob_count,
        parse_status_counts,
        missing_blobs_count,
        orphan_blobs_count: storage_details.orphan_blob_count,
        physical_storage_bytes: storage_details.physical_disk_bytes,
        max_storage_bytes: storage_details.max_storage_bytes,
        has_quota_pressure: storage_details.has_quota_pressure,
        watcher_enabled: settings.watch_enabled,
        last_successful_snapshot_at,
        timestamp: chrono_now_iso(),
    })
}

/// Scenario G: DB 유실 시 물리 원시 CAS 스토리지 읽기 전용 스캔
pub fn scan_and_recover_raw_blobs(storage_root: &Path) -> Result<Vec<RawBlobScanResult>, AppError> {
    let snapshots_dir = storage_root.join("snapshots");
    if !snapshots_dir.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in walkdir_simple(&snapshots_dir) {
        if entry.is_file() && entry.extension().and_then(|e| e.to_str()) == Some("bin") {
            let file_stem = entry
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            if let Ok(bytes) = fs::read(&entry) {
                let hash = super::snapshot::compute_sha256(&bytes);
                let parsed = parse_tab_state(file_stem, &bytes);
                let is_valid = parsed.parse_status == ParseStatus::Parsed;
                let text_preview = parsed.recovered_text.map(|t| {
                    t.chars()
                        .filter(|c| *c != '\r' && *c != '\n')
                        .take(120)
                        .collect()
                });
                results.push(RawBlobScanResult {
                    hash,
                    file_size,
                    is_valid_tabstate: is_valid,
                    format_version: parsed.format_version,
                    recovered_title: parsed.original_path.or(Some(parsed.display_title)),
                    recovered_text_preview: text_preview,
                    parse_error: if is_valid {
                        None
                    } else {
                        Some(format!("{:?}", parsed.parse_status))
                    },
                });
            }
        }
    }

    Ok(results)
}

fn walkdir_simple(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                files.extend(walkdir_simple(&p));
            } else if p.is_file() {
                files.push(p);
            }
        }
    }
    files
}

fn chrono_now_iso() -> String {
    let now = std::time::SystemTime::now();
    let epoch = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 간이 RFC3339 생성
    let days = epoch / 86400;
    let rem = epoch % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;

    // 대략적인 년월일 계산 (1970 기준)
    let mut year = 1970;
    let mut day_count = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap { 366 } else { 365 };
        if day_count < days_in_year {
            break;
        }
        day_count -= days_in_year;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if leap {
        month_days[1] = 29;
    }
    let mut month = 1;
    for &md in &month_days {
        if day_count < md {
            break;
        }
        day_count -= md;
        month += 1;
    }
    let day = day_count + 1;

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_1_basic_txt() {
        let temp_dir = std::env::temp_dir().join("nr5-test-export-basic");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("test.txt");

        let res = write_atomic_txt(&target, "Hello Notepad Rescue!");
        assert!(res.is_ok());
        let final_path = res.unwrap();
        assert!(final_path.exists());
        let read = fs::read_to_string(&final_path).unwrap();
        assert_eq!(read, "Hello Notepad Rescue!");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_2_3_4_korean_japanese_emoji() {
        let temp_dir = std::env::temp_dir().join("nr5-test-export-cjk");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("cjk.txt");

        let content = "한국어 메모 복구\n日本語メモ復元\nEmoji 🚀✨🔥\n";
        let res = write_atomic_txt(&target, content);
        assert!(res.is_ok());
        let final_path = res.unwrap();
        let read = fs::read_to_string(&final_path).unwrap();
        assert_eq!(read, content);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_5_empty_note() {
        let temp_dir = std::env::temp_dir().join("nr5-test-export-empty");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("empty.txt");

        let res = write_atomic_txt(&target, "");
        assert!(res.is_ok());
        let final_path = res.unwrap();
        let read = fs::read_to_string(&final_path).unwrap();
        assert_eq!(read, "");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_6_large_note() {
        let temp_dir = std::env::temp_dir().join("nr5-test-export-large");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("large.txt");

        let large_content = "A".repeat(500_000);
        let res = write_atomic_txt(&target, &large_content);
        assert!(res.is_ok());
        let final_path = res.unwrap();
        assert_eq!(fs::metadata(&final_path).unwrap().len(), 500_000);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_7_8_crlf_and_lf_preservation() {
        let temp_dir = std::env::temp_dir().join("nr5-test-export-line-endings");
        let _ = fs::remove_dir_all(&temp_dir);

        let crlf = "Line1\r\nLine2\r\nLine3";
        let p1 = temp_dir.join("crlf.txt");
        let _ = write_atomic_txt(&p1, crlf).unwrap();
        assert_eq!(fs::read_to_string(&p1).unwrap(), crlf);

        let lf = "Line1\nLine2\nLine3";
        let p2 = temp_dir.join("lf.txt");
        let _ = write_atomic_txt(&p2, lf).unwrap();
        assert_eq!(fs::read_to_string(&p2).unwrap(), lf);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_9_invalid_filename_chars() {
        let bad_title = "bad/name:with*chars?\"and<pipe>|";
        let safe = sanitize_export_filename(bad_title, 12345);
        assert!(!safe.contains('/'));
        assert!(!safe.contains(':'));
        assert!(!safe.contains('*'));
        assert!(!safe.contains('?'));
        assert!(!safe.contains('"'));
        assert!(!safe.contains('<'));
        assert!(!safe.contains('>'));
        assert!(!safe.contains('|'));
        assert!(safe.starts_with("NotepadRescue_"));
    }

    #[test]
    fn test_export_10_reserved_dos_filename() {
        assert!(is_reserved_dos_name("CON"));
        assert!(is_reserved_dos_name("con"));
        assert!(is_reserved_dos_name("PRN.txt"));
        assert!(is_reserved_dos_name("aux"));
        assert!(is_reserved_dos_name("NUL"));
        assert!(is_reserved_dos_name("COM1"));

        let safe = sanitize_export_filename("CON", 12345);
        assert!(safe.contains("_CON_"));
        assert!(!safe.starts_with("NotepadRescue_CON_"));
    }

    #[test]
    fn test_export_11_duplicate_export_no_overwrite() {
        let temp_dir = std::env::temp_dir().join("nr5-test-collision");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("note.txt");

        let p1 = write_atomic_txt(&target, "version 1").unwrap();
        let p2 = write_atomic_txt(&target, "version 2").unwrap();

        assert_ne!(p1, p2);
        assert_eq!(fs::read_to_string(&p1).unwrap(), "version 1");
        assert_eq!(fs::read_to_string(&p2).unwrap(), "version 2");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_12_very_long_title() {
        let long_title = "한글제목".repeat(30); // 120 chars
        let safe = sanitize_export_filename(&long_title, 123);
        assert!(safe.len() < 250);
        assert!(safe.ends_with("_123.txt"));
    }

    #[test]
    fn test_export_13_atomic_failure_cleanup() {
        let temp_dir = std::env::temp_dir().join("nr5-test-atomic-fail");
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::create_dir_all(&temp_dir);

        // Windows에서 생성 불가능한 파일명으로 쓰기 실패 유도
        let invalid_target = temp_dir.join("bad:file:name*?.txt");
        let res = write_atomic_txt(&invalid_target, "test content");
        assert!(res.is_err());

        // 잔여 임시 파일이 없는지 확인
        let entries: Vec<_> = fs::read_dir(&temp_dir).unwrap().flatten().collect();
        for entry in entries {
            assert!(!entry.file_name().to_string_lossy().ends_with(".tmp"));
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_14_missing_destination_creates_dirs() {
        let temp_dir = std::env::temp_dir()
            .join("nr5-test-nested")
            .join("sub1")
            .join("sub2");
        let _ = fs::remove_dir_all(std::env::temp_dir().join("nr5-test-nested"));
        let target = temp_dir.join("created.txt");

        let res = write_atomic_txt(&target, "Nested directory created");
        assert!(res.is_ok());
        assert!(target.exists());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "Nested directory created"
        );

        let _ = fs::remove_dir_all(std::env::temp_dir().join("nr5-test-nested"));
    }

    #[test]
    fn test_export_15_no_silent_overwrite_multiple() {
        let temp_dir = std::env::temp_dir().join("nr5-test-no-overwrite");
        let _ = fs::remove_dir_all(&temp_dir);
        let target = temp_dir.join("doc.txt");

        let p1 = write_atomic_txt(&target, "First").unwrap();
        let p2 = write_atomic_txt(&target, "Second").unwrap();
        let p3 = write_atomic_txt(&target, "Third").unwrap();

        assert_eq!(p1, temp_dir.join("doc.txt"));
        assert_eq!(p2, temp_dir.join("doc (1).txt"));
        assert_eq!(p3, temp_dir.join("doc (2).txt"));

        assert_eq!(fs::read_to_string(&p1).unwrap(), "First");
        assert_eq!(fs::read_to_string(&p2).unwrap(), "Second");
        assert_eq!(fs::read_to_string(&p3).unwrap(), "Third");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ---------------------------------------------------------------- Recovery State Matrix (16-24)

    fn mock_snapshot(
        id: &str,
        note_id: &str,
        hash: &str,
        text: Option<&str>,
        status: ParseStatus,
    ) -> NotepadSnapshot {
        NotepadSnapshot {
            id: id.to_string(),
            note_id: note_id.to_string(),
            content_hash: hash.to_string(),
            raw_snapshot_path: format!("snapshots/ab/{hash}.bin"),
            recovered_text: text.map(|s| s.to_string()),
            source_type: crate::notepad_rescue::SourceType::TabState,
            source_filename: "tab.bin".to_string(),
            source_size: 100,
            parse_status: status,
            notepad_format_version: Some(1),
            captured_at: "2026-10-01T00:00:00Z".to_string(),
            created_at: "2026-10-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_recovery_16_confidence_full() {
        let temp_dir = std::env::temp_dir().join("nr5-conf-full");
        let _ = fs::remove_dir_all(&temp_dir);
        let blob_path = temp_dir.join("snapshots").join("ab").join("hash1.bin");
        fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
        fs::write(&blob_path, b"dummy blob").unwrap();

        let snap = mock_snapshot(
            "s1",
            "n1",
            "hash1",
            Some("Recovered text"),
            ParseStatus::Parsed,
        );
        let conf = evaluate_recovery_confidence(&snap, &temp_dir);
        assert_eq!(conf, RecoveryConfidence::Full);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_recovery_17_confidence_text_only() {
        let temp_dir = std::env::temp_dir().join("nr5-conf-text");
        let _ = fs::remove_dir_all(&temp_dir);

        // 원시 블롭 없음
        let snap = mock_snapshot(
            "s2",
            "n1",
            "missing_hash",
            Some("Recovered text"),
            ParseStatus::Parsed,
        );
        let conf = evaluate_recovery_confidence(&snap, &temp_dir);
        assert_eq!(conf, RecoveryConfidence::TextOnly);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_recovery_18_confidence_raw_only() {
        let temp_dir = std::env::temp_dir().join("nr5-conf-raw");
        let _ = fs::remove_dir_all(&temp_dir);
        let blob_path = temp_dir.join("snapshots").join("ab").join("raw_hash.bin");
        fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
        fs::write(&blob_path, b"raw blob data").unwrap();

        // 텍스트 없음 (손상 또는 미지원)
        let snap = mock_snapshot("s3", "n1", "raw_hash", None, ParseStatus::Corrupt);
        let conf = evaluate_recovery_confidence(&snap, &temp_dir);
        assert_eq!(conf, RecoveryConfidence::RawOnly);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_recovery_19_confidence_unavailable() {
        let temp_dir = std::env::temp_dir().join("nr5-conf-none");
        let _ = fs::remove_dir_all(&temp_dir);

        // 텍스트 없음 + 블롭 없음
        let snap = mock_snapshot("s4", "n1", "no_hash", None, ParseStatus::Corrupt);
        let conf = evaluate_recovery_confidence(&snap, &temp_dir);
        assert_eq!(conf, RecoveryConfidence::Unavailable);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_recovery_20_21_latest_parsed_vs_newest_corrupt() {
        tauri::async_runtime::block_on(async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            for m in crate::migrations::migrations() {
                sqlx::raw_sql(m.sql).execute(&pool).await.unwrap();
            }

            let temp_dir = std::env::temp_dir().join("nr5-db-test");
            let _ = fs::remove_dir_all(&temp_dir);

            // 노트 생성
            sqlx::query(
                "INSERT INTO notepad_notes (id, source_id, display_title, is_saved, is_deleted, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES ('note_1', 'src_1', 'Test Note', 0, 0, '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z', '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            // v1 (PARSED)
            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('snap_1', 'note_1', 'h1', 'snapshots/ab/h1.bin', 'Good Version 1', 'tab_state', 't.bin', 10, 'PARSED', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            // v2 (CORRUPT - 최신 스냅샷)
            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('snap_2', 'note_1', 'h2', 'snapshots/ab/h2.bin', NULL, 'tab_state', 't.bin', 10, 'CORRUPT', '2026-10-02T00:00:00Z', '2026-10-02T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            let detail = get_note_recovery_detail(&pool, &temp_dir, "note_1")
                .await
                .unwrap();

            // 20. 최신 스냅샷은 snap_2
            assert_eq!(detail.latest_snapshot.unwrap().id, "snap_2");
            assert!(detail.is_latest_corrupt);

            // 21. 최신 유효(PARSED) 스냅샷은 snap_1
            assert_eq!(detail.latest_valid_snapshot.as_ref().unwrap().id, "snap_1");
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .as_ref()
                    .unwrap()
                    .recovered_text
                    .as_deref()
                    .unwrap(),
                "Good Version 1"
            );

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_recovery_22_deleted_note_recovery() {
        tauri::async_runtime::block_on(async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            for m in crate::migrations::migrations() {
                sqlx::raw_sql(m.sql).execute(&pool).await.unwrap();
            }

            let temp_dir = std::env::temp_dir().join("nr5-del-test");
            let _ = fs::remove_dir_all(&temp_dir);

            // 삭제된 노트
            sqlx::query(
                "INSERT INTO notepad_notes (id, source_id, display_title, is_saved, is_deleted, deleted_at, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES ('note_del', 'src_del', 'Deleted Note', 0, 1, '2026-10-03T01:00:00Z', '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z', '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('snap_del', 'note_del', 'hd', 'snapshots/ab/hd.bin', 'Text of deleted note', 'tab_state', 't.bin', 10, 'PARSED', '2026-10-02T00:00:00Z', '2026-10-02T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            let detail = get_note_recovery_detail(&pool, &temp_dir, "note_del")
                .await
                .unwrap();
            assert!(detail.note.is_deleted);
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .unwrap()
                    .recovered_text
                    .unwrap(),
                "Text of deleted note"
            );

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_recovery_23_24_active_historical_and_unsupported_fallback() {
        tauri::async_runtime::block_on(async {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();

            for m in crate::migrations::migrations() {
                sqlx::raw_sql(m.sql).execute(&pool).await.unwrap();
            }

            let temp_dir = std::env::temp_dir().join("nr5-hist-test");
            let _ = fs::remove_dir_all(&temp_dir);

            sqlx::query(
                "INSERT INTO notepad_notes (id, source_id, display_title, is_saved, is_deleted, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES ('note_act', 'src_act', 'Active Multi Version', 0, 0, '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z', '2026-10-01T00:00:00Z', '2026-10-03T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            // v1 (PARSED)
            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('s1', 'note_act', 'h1', 'snapshots/ab/h1.bin', 'Version 1 Content', 'tab_state', 't.bin', 10, 'PARSED', '2026-10-01T00:00:00Z', '2026-10-01T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            // v2 (UNSUPPORTED_FORMAT)
            sqlx::query(
                "INSERT INTO notepad_snapshots (id, note_id, content_hash, raw_snapshot_path, recovered_text, source_type, source_filename, source_size, parse_status, captured_at, created_at)
                 VALUES ('s2', 'note_act', 'h2', 'snapshots/ab/h2.bin', NULL, 'tab_state', 't.bin', 10, 'UNSUPPORTED_FORMAT', '2026-10-02T00:00:00Z', '2026-10-02T00:00:00Z')"
            ).execute(&pool).await.unwrap();

            let detail = get_note_recovery_detail(&pool, &temp_dir, "note_act")
                .await
                .unwrap();
            // 23. v1 히스토리 보존 확인
            assert_eq!(
                detail
                    .latest_valid_snapshot
                    .unwrap()
                    .recovered_text
                    .unwrap(),
                "Version 1 Content"
            );

            // 24. v2 UNSUPPORTED_FORMAT 일 때 latest_snapshot은 s2
            assert_eq!(detail.latest_snapshot.as_ref().unwrap().id, "s2");
            assert_eq!(
                detail.latest_snapshot.as_ref().unwrap().parse_status,
                ParseStatus::UnsupportedFormat
            );

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }
}
