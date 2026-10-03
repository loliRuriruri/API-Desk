use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::models::{NotepadSnapshot, ParseStatus, SourceType};
use super::parser::{compute_tabstate_crc32, extract_stored_crc32, NOTEPAD_MAGIC};
use super::repository;
use super::snapshot::compute_sha256;
use super::watcher::default_notepad_local_state_dir;
use crate::error::AppError;

/// Notepad 실행 중 오류 메시지 상수
pub const NOTEPAD_RUNNING_ERROR: &str = "메모장 탭 복원을 위해 메모장을 먼저 닫아주세요.";

/// 네이티브 탭 복원 결과 리포트
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreNativeReport {
    pub success: bool,
    pub note_id: String,
    pub snapshot_id: String,
    pub restored_tab_uuid: String,
    pub restored_tab_path: String,
    pub backup_dir: String,
    pub notepad_launched: bool,
    pub message: String,
}

/// 롤백 실행 결과 리포트
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackReport {
    pub success: bool,
    pub restored_note_id: Option<String>,
    pub backup_timestamp: Option<String>,
    pub removed_tab_file: Option<String>,
    pub removed_window_files: Vec<String>,
    pub message: String,
}

/// 사전 안전 백업 메타데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyBackupMetadata {
    pub timestamp: String,
    pub backup_dir: String,
    pub note_id: String,
    pub snapshot_id: String,
    pub restored_tab_filename: String,
    pub restored_tab_path: String,
    pub restored_window_uuid: String,
    pub restored_window_paths: Vec<String>,
    pub backed_up_tabs: Vec<String>,
    pub backed_up_windows: Vec<String>,
}

static UUID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 순수 Rust로 RFC 4122 v4 UUID 형식 문자열을 생성합니다.
pub fn generate_uuid_v4() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let c = UUID_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut sha = Sha256::new();
    sha.update(now.as_nanos().to_le_bytes());
    sha.update(c.to_le_bytes());
    let hash = sha.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[0..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 1

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// GUID 문자열(8-4-4-4-12)을 Windows 바이너리 형식(Data1/2/3 LE, Data4 BE) 16바이트로 변환합니다.
pub fn guid_str_to_bytes_le(s: &str) -> Option<[u8; 16]> {
    let clean = s.trim().trim_start_matches('{').trim_end_matches('}');
    let parts: Vec<&str> = clean.split('-').collect();
    if parts.len() != 5 {
        return None;
    }
    if parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
    {
        return None;
    }
    let d1 = u32::from_str_radix(parts[0], 16).ok()?;
    let d2 = u16::from_str_radix(parts[1], 16).ok()?;
    let d3 = u16::from_str_radix(parts[2], 16).ok()?;
    let d4_0 = u16::from_str_radix(parts[3], 16).ok()?;
    let d4_1 = u64::from_str_radix(parts[4], 16).ok()?;

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&d1.to_le_bytes());
    result[4..6].copy_from_slice(&d2.to_le_bytes());
    result[6..8].copy_from_slice(&d3.to_le_bytes());
    result[8..10].copy_from_slice(&d4_0.to_be_bytes());
    let p4_bytes = d4_1.to_be_bytes();
    result[10..16].copy_from_slice(&p4_bytes[2..8]);
    Some(result)
}

/// Windows 11 Notepad WindowState 52바이트 바이너리를 생성합니다.
/// 포맷:
/// - Magic: "NP\0\x2c\x00\x01" (6바이트)
/// - Tab GUID: 16바이트 (Little-Endian)
/// - Window Config: 26바이트 (표준 기본 윈도우 좌표 및 크기 설정)
/// - CRC32: 4바이트 (Big-Endian, 버퍼[2..48] 범위 계산)
pub fn build_window_state_content(tab_guid_le: &[u8; 16]) -> [u8; 52] {
    let config_bytes: [u8; 26] = [
        0x00, 0x1e, 0x00, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0x6a, 0x06, 0x00, 0x00, 0xa9, 0x03,
        0x00, 0x00, 0x4c, 0x06, 0x00, 0x00, 0x8b, 0x03, 0x00, 0x00, 0x00,
    ];
    let mut ws_body = Vec::with_capacity(48);
    ws_body.extend_from_slice(b"NP\x00\x2c\x00\x01");
    ws_body.extend_from_slice(tab_guid_le);
    ws_body.extend_from_slice(&config_bytes);

    let mut hasher = Hasher::new();
    hasher.update(&ws_body[2..]);
    let crc = hasher.finalize();

    let mut result = [0u8; 52];
    result[0..48].copy_from_slice(&ws_body);
    result[48..52].copy_from_slice(&crc.to_be_bytes());
    result
}

/// 현재 시스템에서 `notepad.exe` 프로세스가 실행 중인지 확인합니다.
pub fn is_notepad_running() -> bool {
    #[cfg(windows)]
    {
        let output = match crate::process::hidden_command("tasklist")
            .args(["/FI", "IMAGENAME eq notepad.exe", "/FO", "CSV", "/NH"])
            .output()
        {
            Ok(out) => out,
            Err(_) => return false,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.to_lowercase().contains("notepad.exe")
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Notepad 프로세스를 정상 실행합니다.
pub fn launch_notepad() -> Result<(), AppError> {
    #[cfg(windows)]
    {
        crate::process::hidden_command("cmd.exe")
            .args(["/c", "start", "", "notepad.exe"])
            .spawn()
            .map_err(|e| AppError::Io(format!("메모장 실행 실패: {e}")))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// 임시 파일 작성 -> flush/sync -> rename을 통한 원자적(atomic) 파일 발행.
pub fn atomic_write(target_path: &Path, data: &[u8]) -> Result<(), AppError> {
    let parent = target_path.parent().ok_or_else(|| {
        AppError::Io(format!(
            "유효하지 않은 부모 디렉터리: {}",
            target_path.display()
        ))
    })?;
    fs::create_dir_all(parent)
        .map_err(|e| AppError::Io(format!("디렉터리 생성 실패 ({}): {e}", parent.display())))?;

    let filename = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_path = parent.join(format!("{filename}.{}.tmp", generate_uuid_v4()));

    let mut file = File::create(&tmp_path)
        .map_err(|e| AppError::Io(format!("임시 파일 생성 실패 ({}): {e}", tmp_path.display())))?;

    file.write_all(data).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!("임시 파일 쓰기 실패: {e}"))
    })?;

    file.sync_all().map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!("디스크 동기화 실패: {e}"))
    })?;

    drop(file);

    fs::rename(&tmp_path, target_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        AppError::Io(format!(
            "원자적 파일 교체(rename) 실패 ({} -> {}): {e}",
            tmp_path.display(),
            target_path.display()
        ))
    })?;

    Ok(())
}

/// 복원 수행 전 현재 Notepad의 live TabState 및 WindowState 전체를 안전 백업합니다.
pub fn create_safety_backup(
    storage_root: &Path,
    notepad_local_state: &Path,
) -> Result<(PathBuf, String, Vec<String>, Vec<String>), AppError> {
    let live_tab_state = notepad_local_state.join("TabState");
    let live_window_state = notepad_local_state.join("WindowState");

    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let safe_ts = format!("backup_{now_millis}");

    let backup_root = storage_root.join("restore_backups").join(&safe_ts);
    let backup_tab_state = backup_root.join("TabState");
    let backup_window_state = backup_root.join("WindowState");

    fs::create_dir_all(&backup_tab_state).map_err(|e| {
        AppError::Io(format!(
            "백업 TabState 디렉터리 생성 실패 ({}): {e}",
            backup_tab_state.display()
        ))
    })?;
    fs::create_dir_all(&backup_window_state).map_err(|e| {
        let _ = fs::remove_dir_all(&backup_root);
        AppError::Io(format!(
            "백업 WindowState 디렉터리 생성 실패 ({}): {e}",
            backup_window_state.display()
        ))
    })?;

    let mut backed_up_tabs = Vec::new();
    if live_tab_state.exists() {
        match fs::read_dir(&live_tab_state) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let dest = backup_tab_state.join(&name);
                        if let Err(e) = fs::copy(&path, &dest) {
                            let _ = fs::remove_dir_all(&backup_root);
                            return Err(AppError::Io(format!(
                                "TabState 백업 복사 실패 ({} -> {}): {e}",
                                path.display(),
                                dest.display()
                            )));
                        }
                        backed_up_tabs.push(name);
                    }
                }
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&backup_root);
                return Err(AppError::Io(format!("TabState 읽기 실패: {e}")));
            }
        }
    }

    let mut backed_up_windows = Vec::new();
    if live_window_state.exists() {
        match fs::read_dir(&live_window_state) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let dest = backup_window_state.join(&name);
                        if let Err(e) = fs::copy(&path, &dest) {
                            let _ = fs::remove_dir_all(&backup_root);
                            return Err(AppError::Io(format!(
                                "WindowState 백업 복사 실패 ({} -> {}): {e}",
                                path.display(),
                                dest.display()
                            )));
                        }
                        backed_up_windows.push(name);
                    }
                }
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&backup_root);
                return Err(AppError::Io(format!("WindowState 읽기 실패: {e}")));
            }
        }
    }

    Ok((backup_root, safe_ts, backed_up_tabs, backed_up_windows))
}

/// 복원 대상 스냅샷의 원본 RAW 바이너리를 철저히 검증합니다.
/// 검증 항목:
/// - parse_status == PARSED
/// - source_type == TabState
/// - RAW blob 파일 존재 여부
/// - SHA-256 해시 일치 여부
/// - NP 매직 바이트 ("NP\0")
/// - CRC32 무결성
pub fn validate_snapshot_for_restore(
    storage_root: &Path,
    snapshot: &NotepadSnapshot,
) -> Result<Vec<u8>, AppError> {
    if snapshot.parse_status != ParseStatus::Parsed {
        return Err(AppError::InvalidRequest(
            "정상 파싱된(PARSED) 스냅샷만 메모장 탭으로 복원할 수 있습니다.".to_string(),
        ));
    }

    if snapshot.source_type != SourceType::TabState {
        return Err(AppError::InvalidRequest(
            "TabState 형식의 스냅샷만 메모장 탭으로 복원할 수 있습니다.".to_string(),
        ));
    }

    let blob_path = storage_root.join(&snapshot.raw_snapshot_path);
    if !blob_path.exists() {
        return Err(AppError::InvalidRequest(format!(
            "보관된 RAW 스냅샷 파일을 찾을 수 없습니다: {}",
            blob_path.display()
        )));
    }

    let data = fs::read(&blob_path).map_err(|e| {
        AppError::Io(format!(
            "RAW 스냅샷 읽기 실패 ({}): {e}",
            blob_path.display()
        ))
    })?;

    let computed_hash = compute_sha256(&data);
    if computed_hash != snapshot.content_hash {
        return Err(AppError::InvalidRequest(format!(
            "스냅샷 해시 무결성 검증 실패 (기대값: {}, 실제값: {})",
            snapshot.content_hash, computed_hash
        )));
    }

    if data.len() < 4 || &data[0..3] != NOTEPAD_MAGIC {
        return Err(AppError::InvalidRequest(
            "Notepad 매직 바이트가 유효하지 않습니다.".to_string(),
        ));
    }

    if let (Some(computed), Some(stored)) =
        (compute_tabstate_crc32(&data), extract_stored_crc32(&data))
    {
        if computed != stored {
            return Err(AppError::InvalidRequest(format!(
                "TabState CRC32 무결성 오류: 계산값={computed:08x}, 저장값={stored:08x}"
            )));
        }
    } else {
        return Err(AppError::InvalidRequest(
            "TabState CRC32를 추출할 수 없습니다.".to_string(),
        ));
    }

    Ok(data)
}

/// 보관된 스냅샷을 실제 Windows Notepad의 live TabState/WindowState로 복원합니다.
pub async fn restore_native_tab(
    pool: &SqlitePool,
    storage_root: &Path,
    note_id: &str,
    snapshot_id: Option<&str>,
) -> Result<RestoreNativeReport, AppError> {
    // 1. Notepad 실행 상태 검증 (실행 중이면 거부)
    if is_notepad_running() {
        return Err(AppError::InvalidRequest(NOTEPAD_RUNNING_ERROR.to_string()));
    }

    // 2. 노트 조회
    let note = repository::get_note(pool, note_id)
        .await?
        .ok_or_else(|| AppError::InvalidRequest(format!("노트를 찾을 수 없습니다: {note_id}")))?;

    // 3. 복원 대상 스냅샷 조회
    let snapshot = if let Some(sid) = snapshot_id {
        let snap = repository::get_snapshot(pool, sid)
            .await?
            .ok_or_else(|| AppError::InvalidRequest(format!("스냅샷을 찾을 수 없습니다: {sid}")))?;
        if snap.note_id != note.id {
            return Err(AppError::InvalidRequest(
                "지정한 스냅샷이 해당 노트에 속하지 않습니다.".to_string(),
            ));
        }
        snap
    } else {
        // 최신 PARSED 스냅샷 조회
        let row = sqlx::query(
            "SELECT * FROM notepad_snapshots
             WHERE note_id = ?1 AND parse_status = 'PARSED'
             ORDER BY captured_at DESC, rowid DESC
             LIMIT 1",
        )
        .bind(&note.id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Database(format!("스냅샷 조회 실패: {e}")))?;

        match row {
            Some(r) => repository::map_snapshot_row(&r)?,
            None => {
                return Err(AppError::InvalidRequest(
                    "복원 가능한 정상(PARSED) 스냅샷이 존재하지 않습니다.".to_string(),
                ));
            }
        }
    };

    // 4. 스냅샷 무결성 및 RAW 데이터 검증
    let raw_data = validate_snapshot_for_restore(storage_root, &snapshot)?;

    // 5. Notepad LocalState 디렉터리 확인
    let notepad_local_state = default_notepad_local_state_dir().ok_or_else(|| {
        AppError::InvalidRequest(
            "Windows Notepad LocalState 경로를 확인할 수 없습니다.".to_string(),
        )
    })?;

    let tab_state_dir = notepad_local_state.join("TabState");
    let window_state_dir = notepad_local_state.join("WindowState");

    fs::create_dir_all(&tab_state_dir).map_err(|e| {
        AppError::Io(format!(
            "TabState 디렉터리 생성 실패 ({}): {e}",
            tab_state_dir.display()
        ))
    })?;
    fs::create_dir_all(&window_state_dir).map_err(|e| {
        AppError::Io(format!(
            "WindowState 디렉터리 생성 실패 ({}): {e}",
            window_state_dir.display()
        ))
    })?;

    // 6. 사전 안전 백업 (실패 시 복원 즉시 중단)
    let (backup_dir, backup_ts, backed_up_tabs, backed_up_windows) =
        create_safety_backup(storage_root, &notepad_local_state)?;

    // 7. 충돌 방지 탭 GUID 및 파일명 결정
    // 기존 탭 파일과 충돌하지 않도록 보장
    let (tab_uuid_str, tab_filename, tab_guid_le) = match guid_str_to_bytes_le(&note.source_id) {
        Some(guid_le)
            if !tab_state_dir
                .join(format!("{}.bin", note.source_id))
                .exists() =>
        {
            (
                note.source_id.clone(),
                format!("{}.bin", note.source_id),
                guid_le,
            )
        }
        _ => {
            let new_uuid = generate_uuid_v4();
            let guid_le = guid_str_to_bytes_le(&new_uuid).ok_or_else(|| {
                AppError::InvalidRequest("생성된 UUID 바이트 변환 실패".to_string())
            })?;
            (new_uuid.clone(), format!("{new_uuid}.bin"), guid_le)
        }
    };

    let target_tab_path = tab_state_dir.join(&tab_filename);

    // 8. 윈도우 식별자 및 WindowState 바이너리 생성
    let win_uuid_str = generate_uuid_v4();
    let ws_content = build_window_state_content(&tab_guid_le);

    let target_win_0 = window_state_dir.join(format!("{win_uuid_str}.0.bin"));
    let target_win_1 = window_state_dir.join(format!("{win_uuid_str}.1.bin"));

    // 9. 원자적(atomic) 파일 쓰기
    if let Err(e) = atomic_write(&target_tab_path, &raw_data) {
        return Err(AppError::Io(format!("TabState 복원 파일 쓰기 실패: {e}")));
    }

    if let Err(e) = atomic_write(&target_win_0, &ws_content) {
        let _ = fs::remove_file(&target_tab_path);
        return Err(AppError::Io(format!("WindowState 0 쓰기 실패: {e}")));
    }

    if let Err(e) = atomic_write(&target_win_1, &[]) {
        let _ = fs::remove_file(&target_tab_path);
        let _ = fs::remove_file(&target_win_0);
        return Err(AppError::Io(format!("WindowState 1 쓰기 실패: {e}")));
    }

    // 10. 롤백용 last_restore.json 기록
    let meta = SafetyBackupMetadata {
        timestamp: backup_ts,
        backup_dir: backup_dir.to_string_lossy().to_string(),
        note_id: note.id.clone(),
        snapshot_id: snapshot.id.clone(),
        restored_tab_filename: tab_filename.clone(),
        restored_tab_path: target_tab_path.to_string_lossy().to_string(),
        restored_window_uuid: win_uuid_str,
        restored_window_paths: vec![
            target_win_0.to_string_lossy().to_string(),
            target_win_1.to_string_lossy().to_string(),
        ],
        backed_up_tabs,
        backed_up_windows,
    };

    let restore_backups_dir = storage_root.join("restore_backups");
    let last_restore_file = restore_backups_dir.join("last_restore.json");
    let meta_json = serde_json::to_string_pretty(&meta)
        .map_err(|e| AppError::InvalidRequest(format!("메타데이터 직렬화 실패: {e}")))?;
    let _ = atomic_write(&last_restore_file, meta_json.as_bytes());

    // 백업 디렉터리 내에도 backup_meta.json 보관
    let backup_meta_file = backup_dir.join("backup_meta.json");
    let _ = atomic_write(&backup_meta_file, meta_json.as_bytes());

    // 11. Notepad 정상 실행
    let launch_res = launch_notepad();
    let notepad_launched = launch_res.is_ok();

    Ok(RestoreNativeReport {
        success: true,
        note_id: note.id,
        snapshot_id: snapshot.id,
        restored_tab_uuid: tab_uuid_str,
        restored_tab_path: target_tab_path.to_string_lossy().to_string(),
        backup_dir: backup_dir.to_string_lossy().to_string(),
        notepad_launched,
        message: "메모장 탭으로 성공적으로 복원되었습니다.".to_string(),
    })
}

/// 마지막으로 수행된 네이티브 탭 복원을 안전하게 되돌립니다.
pub fn rollback_last_restore(
    storage_root: &Path,
    notepad_local_state: &Path,
) -> Result<RollbackReport, AppError> {
    rollback_last_restore_internal(storage_root, notepad_local_state, true)
}

/// 롤백 내부 로직 (단위 테스트에서 프로세스 확인 옵션 지원).
pub fn rollback_last_restore_internal(
    storage_root: &Path,
    notepad_local_state: &Path,
    check_running: bool,
) -> Result<RollbackReport, AppError> {
    // 1. Notepad 실행 상태 검증 (실행 중이면 거부)
    if check_running && is_notepad_running() {
        return Err(AppError::InvalidRequest(NOTEPAD_RUNNING_ERROR.to_string()));
    }

    let restore_backups_dir = storage_root.join("restore_backups");
    let last_restore_file = restore_backups_dir.join("last_restore.json");

    if !last_restore_file.exists() {
        return Err(AppError::InvalidRequest(
            "되돌릴 수 있는 이전 복원 내역이 없습니다.".to_string(),
        ));
    }

    let meta_str = fs::read_to_string(&last_restore_file)
        .map_err(|e| AppError::Io(format!("복원 메타데이터 읽기 실패: {e}")))?;
    let meta: SafetyBackupMetadata = serde_json::from_str(&meta_str)
        .map_err(|e| AppError::InvalidRequest(format!("복원 메타데이터 역직렬화 실패: {e}")))?;

    let backup_dir = PathBuf::from(&meta.backup_dir);
    if !backup_dir.exists() {
        return Err(AppError::InvalidRequest(format!(
            "사전 안전 백업 디렉터리를 찾을 수 없습니다: {}",
            backup_dir.display()
        )));
    }

    let backup_tab_state = backup_dir.join("TabState");
    let backup_window_state = backup_dir.join("WindowState");

    if !backup_tab_state.exists() || !backup_window_state.exists() {
        return Err(AppError::InvalidRequest(
            "백업 디렉터리 내 TabState 또는 WindowState가 손상되었습니다.".to_string(),
        ));
    }

    // 2. 복원되었던 파일 제거
    let mut removed_tab = None;
    let tab_path = PathBuf::from(&meta.restored_tab_path);
    if tab_path.exists() {
        let _ = fs::remove_file(&tab_path);
        removed_tab = Some(meta.restored_tab_filename.clone());
    }

    let mut removed_windows = Vec::new();
    for win_path_str in &meta.restored_window_paths {
        let p = PathBuf::from(win_path_str);
        if p.exists() {
            let _ = fs::remove_file(&p);
            removed_windows.push(win_path_str.clone());
        }
    }

    // 3. 백업된 파일들을 live 디렉터리로 복구
    let live_tab_state = notepad_local_state.join("TabState");
    let live_window_state = notepad_local_state.join("WindowState");

    if let Ok(entries) = fs::read_dir(&backup_tab_state) {
        for entry in entries.flatten() {
            let src = entry.path();
            if src.is_file() {
                let name = entry.file_name();
                let dest = live_tab_state.join(name);
                let _ = fs::copy(&src, &dest);
            }
        }
    }

    if let Ok(entries) = fs::read_dir(&backup_window_state) {
        for entry in entries.flatten() {
            let src = entry.path();
            if src.is_file() {
                let name = entry.file_name();
                let dest = live_window_state.join(name);
                let _ = fs::copy(&src, &dest);
            }
        }
    }

    // 4. last_restore.json 제거
    let _ = fs::remove_file(&last_restore_file);

    Ok(RollbackReport {
        success: true,
        restored_note_id: Some(meta.note_id),
        backup_timestamp: Some(meta.timestamp),
        removed_tab_file: removed_tab,
        removed_window_files: removed_windows,
        message: "마지막 복원이 정상적으로 취소(롤백)되었습니다.".to_string(),
    })
}

/// 되돌릴 수 있는 이전 복원 내역이 존재하는지 확인합니다.
pub fn can_rollback(storage_root: &Path) -> bool {
    storage_root
        .join("restore_backups")
        .join("last_restore.json")
        .exists()
}
