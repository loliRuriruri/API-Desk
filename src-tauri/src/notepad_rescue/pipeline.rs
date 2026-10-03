use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use tauri::Emitter;

use super::models::{
    NoteDeletedEvent, NotepadNote, NotepadSession, NotepadSnapshot, ParsedTabState,
    ParsedWindowState, SnapshotCreatedEvent, SourceType,
};
use super::parser;
use super::reader;
use super::repository;
use super::snapshot;
use crate::error::AppError;

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 고유한 엔티티 ID를 생성합니다.
pub fn generate_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}-{counter:x}")
}

/// 현재 시각을 RFC 3339 (밀리초 포함) 형식 문자열로 반환합니다.
pub fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let base = crate::usage::epoch_to_rfc3339(secs);
    if let Some(stripped) = base.strip_suffix('Z') {
        format!("{stripped}.{millis:03}Z")
    } else {
        base
    }
}

/// 파일명에서 source_id(탭 식별자)를 추출합니다.
/// e.g. "1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2.bin" -> "1ee7a4a9-0a43-4ee4-9f27-b8c435f93df2"
pub fn extract_source_id(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    if filename.starts_with('.') || filename.ends_with(".tmp") || filename.ends_with(".bak") {
        return None;
    }

    if let Some(stripped) = filename.strip_suffix(".bin") {
        // TabState의 .0.bin / .1.bin 등 청크 파일은 원본 .bin의 보조 파일이므로 stem 추출 시 정리
        let stem = stripped
            .strip_suffix(".0")
            .or_else(|| stripped.strip_suffix(".1"))
            .unwrap_or(stripped);
        Some(stem.to_string())
    } else {
        None
    }
}

/// 상태 파일(TabState 또는 WindowState)을 안전하게 읽어 Content-Addressed 스토리지에 보관하고 DB에 메타데이터를 기록합니다.
/// 원칙: RAW SNAPSHOT 저장은 항상 파서 실행보다 먼저 수행되며, 파싱 실패 시에도 RAW 파일과 메타데이터는 안전하게 보존됩니다.
pub async fn capture_state_file(
    path: &Path,
    source_type: SourceType,
    pool: &SqlitePool,
    storage_root: &Path,
    app: Option<&tauri::AppHandle>,
) -> Result<Option<SnapshotCreatedEvent>, AppError> {
    let source_id = match extract_source_id(path) {
        Some(id) if !id.trim().is_empty() => id,
        _ => return Ok(None),
    };

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.bin")
        .to_string();

    // 1. 안전한 공유 읽기 (Safe Reader)
    let data = match reader::safe_read_file(path) {
        Ok(bytes) => bytes,
        Err(AppError::Io(msg)) if msg.contains("존재하지 않습니다") => {
            // 읽기 시도 중 파일이 이미 닫히거나 삭제됨
            return Ok(None);
        }
        Err(e) => return Err(e),
    };

    if data.is_empty() {
        return Ok(None);
    }

    // 2. 전체 바이너리 바이트열 SHA-256 계산
    let hash = snapshot::compute_sha256(&data);

    // 3. Content-Addressed Physical Storage에 원자적 저장 (물리적 중복 제거)
    let rel_snapshot_path = snapshot::store_blob(storage_root, &hash, &data)?;

    let captured_at = now_rfc3339();

    // 4. 소스 유형별 파싱 및 DB 영속화
    match source_type {
        SourceType::TabState => {
            let parsed: ParsedTabState = parser::parse_tab_state(&source_id, &data);

            // 4-1. notepad_notes upsert
            let existing_note = repository::get_note_by_source_id(pool, &source_id).await?;
            let note = match existing_note {
                Some(mut existing) => {
                    existing.display_title = parsed.display_title.clone();
                    if parsed.original_path.is_some() {
                        existing.original_path = parsed.original_path.clone();
                    }
                    existing.is_saved = parsed.is_saved;
                    existing.is_deleted = false;
                    existing.last_seen_at = captured_at.clone();
                    existing.updated_at = captured_at.clone();
                    existing
                }
                None => NotepadNote {
                    id: generate_id("note"),
                    source_id: source_id.clone(),
                    display_title: parsed.display_title.clone(),
                    original_path: parsed.original_path.clone(),
                    is_saved: parsed.is_saved,
                    is_deleted: false,
                    first_seen_at: captured_at.clone(),
                    last_seen_at: captured_at.clone(),
                    deleted_at: None,
                    created_at: captured_at.clone(),
                    updated_at: captured_at.clone(),
                },
            };

            let saved_note = repository::upsert_note(pool, &note).await?;

            // 4-2. 동일 노트의 최신 스냅샷과 내용 해시가 동일한 경우 중복 row 생성 방지
            let latest_snap =
                repository::get_latest_snapshot_for_note(pool, &saved_note.id).await?;
            if let Some(last) = latest_snap {
                if last.content_hash == hash {
                    return Ok(None);
                }
            }

            // 4-3. notepad_snapshots 메타데이터 추가
            let snapshot_id = generate_id("snap");
            let snapshot = NotepadSnapshot {
                id: snapshot_id.clone(),
                note_id: saved_note.id.clone(),
                content_hash: hash.clone(),
                raw_snapshot_path: rel_snapshot_path,
                recovered_text: parsed.recovered_text,
                source_type: SourceType::TabState,
                source_filename: filename,
                source_size: data.len() as i64,
                parse_status: parsed.parse_status,
                notepad_format_version: parsed.format_version,
                captured_at: captured_at.clone(),
                created_at: captured_at.clone(),
            };

            repository::insert_snapshot_metadata(pool, &snapshot).await?;

            // 4-4. 프런트엔드 알림 이벤트
            let event = SnapshotCreatedEvent {
                note_id: saved_note.id,
                source_id: saved_note.source_id,
                display_title: saved_note.display_title,
                snapshot_id,
                content_hash: hash,
                source_type: SourceType::TabState,
                is_saved: saved_note.is_saved,
                captured_at,
            };

            if let Some(app_handle) = app {
                let _ = app_handle.emit("notepad-rescue:snapshot-created", &event);
            }

            Ok(Some(event))
        }
        SourceType::WindowState => {
            let parsed: ParsedWindowState = parser::parse_window_state(&source_id, &data);

            let session = NotepadSession {
                id: generate_id("session"),
                source_filename: filename,
                content_hash: hash,
                raw_snapshot_path: rel_snapshot_path,
                captured_at: captured_at.clone(),
                parse_status: parsed.parse_status,
                created_at: captured_at,
            };

            repository::insert_session_snapshot(pool, &session).await?;
            Ok(None)
        }
    }
}

/// 파일 삭제 감지 시 호출되어 연계된 노트의 소프트 삭제(`is_deleted = 1`)를 수행합니다.
/// 기존 RAW 스냅샷이나 메타데이터는 일체 삭제되지 않습니다.
pub async fn handle_file_removed(
    path: &Path,
    source_type: SourceType,
    pool: &SqlitePool,
    app: Option<&tauri::AppHandle>,
) -> Result<Option<NoteDeletedEvent>, AppError> {
    if source_type != SourceType::TabState {
        return Ok(None);
    }

    let source_id = match extract_source_id(path) {
        Some(id) if !id.trim().is_empty() => id,
        _ => return Ok(None),
    };

    // 실제로 파일이 아직 존재하지 않는지 최종 확인 (원자적 교체/Rename 오인 방지)
    if path.exists() {
        return Ok(None);
    }

    let existing = repository::get_note_by_source_id(pool, &source_id).await?;
    let Some(note) = existing else {
        return Ok(None);
    };

    if note.is_deleted {
        return Ok(None);
    }

    let deleted_at = now_rfc3339();
    let affected = repository::mark_note_deleted(pool, &source_id, &deleted_at).await?;

    if affected {
        let event = NoteDeletedEvent {
            note_id: note.id,
            source_id,
            deleted_at,
        };

        if let Some(app_handle) = app {
            let _ = app_handle.emit("notepad-rescue:note-deleted", &event);
        }

        Ok(Some(event))
    } else {
        Ok(None)
    }
}
