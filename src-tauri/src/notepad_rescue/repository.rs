use std::str::FromStr;

use sqlx::{Row, SqlitePool};

use super::models::{
    NoteFilter, NotepadNote, NotepadRescueSettings, NotepadSession, NotepadSnapshot, ParseStatus,
    SourceType, StorageStatistics,
};
use crate::error::AppError;

/// 설정 키 목록.
const KEY_ENABLED: &str = "enabled";
const KEY_WATCH_ENABLED: &str = "watch_enabled";
const KEY_RETENTION_DAYS: &str = "retention_days";
const KEY_DELETED_RETENTION_DAYS: &str = "deleted_retention_days";
const KEY_MAX_STORAGE_BYTES: &str = "max_storage_bytes";

// ---------------------------------------------------------------- Settings

pub async fn get_settings(pool: &SqlitePool) -> Result<NotepadRescueSettings, AppError> {
    let rows = sqlx::query("SELECT key, value, updated_at FROM notepad_rescue_settings")
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(format!("설정 조회 실패: {e}")))?;

    let mut settings = NotepadRescueSettings::default();

    for row in rows {
        let key: String = row.try_get("key").unwrap_or_default();
        let val: String = row.try_get("value").unwrap_or_default();
        let updated: String = row.try_get("updated_at").unwrap_or_default();

        if !updated.is_empty() && (settings.updated_at.is_empty() || updated > settings.updated_at)
        {
            settings.updated_at = updated;
        }

        match key.as_str() {
            KEY_ENABLED => {
                settings.enabled = val.trim().eq_ignore_ascii_case("true") || val.trim() == "1";
            }
            KEY_WATCH_ENABLED => {
                settings.watch_enabled =
                    val.trim().eq_ignore_ascii_case("true") || val.trim() == "1";
            }
            KEY_RETENTION_DAYS => {
                if let Ok(num) = val.trim().parse::<u32>() {
                    settings.retention_days = num;
                }
            }
            KEY_DELETED_RETENTION_DAYS => {
                if let Ok(num) = val.trim().parse::<u32>() {
                    settings.deleted_retention_days = num;
                }
            }
            KEY_MAX_STORAGE_BYTES => {
                if let Ok(num) = val.trim().parse::<u64>() {
                    settings.max_storage_bytes = num;
                }
            }
            _ => {}
        }
    }

    Ok(settings)
}

pub async fn update_settings(
    pool: &SqlitePool,
    settings: &NotepadRescueSettings,
) -> Result<(), AppError> {
    let pairs = [
        (KEY_ENABLED, settings.enabled.to_string()),
        (KEY_WATCH_ENABLED, settings.watch_enabled.to_string()),
        (KEY_RETENTION_DAYS, settings.retention_days.to_string()),
        (
            KEY_DELETED_RETENTION_DAYS,
            settings.deleted_retention_days.to_string(),
        ),
        (
            KEY_MAX_STORAGE_BYTES,
            settings.max_storage_bytes.to_string(),
        ),
    ];

    let updated_at = if settings.updated_at.is_empty() {
        crate::usage::epoch_to_rfc3339(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        )
    } else {
        settings.updated_at.clone()
    };

    for (k, v) in pairs {
        sqlx::query(
            "INSERT INTO notepad_rescue_settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(k)
        .bind(v)
        .bind(&updated_at)
        .execute(pool)
        .await
        .map_err(|e| AppError::Database(format!("설정 저장 실패 ({k}): {e}")))?;
    }

    Ok(())
}

// ---------------------------------------------------------------- Notes

fn map_note_row(row: &sqlx::sqlite::SqliteRow) -> Result<NotepadNote, AppError> {
    let is_saved_int: i64 = row
        .try_get("is_saved")
        .map_err(|e| AppError::Database(e.to_string()))?;
    let is_deleted_int: i64 = row
        .try_get("is_deleted")
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(NotepadNote {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(e.to_string()))?,
        source_id: row
            .try_get("source_id")
            .map_err(|e| AppError::Database(e.to_string()))?,
        display_title: row
            .try_get("display_title")
            .map_err(|e| AppError::Database(e.to_string()))?,
        original_path: row.try_get("original_path").ok().flatten(),
        is_saved: is_saved_int != 0,
        is_deleted: is_deleted_int != 0,
        first_seen_at: row
            .try_get("first_seen_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
        last_seen_at: row
            .try_get("last_seen_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
        deleted_at: row.try_get("deleted_at").ok().flatten(),
        created_at: row
            .try_get("created_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
    })
}

pub async fn upsert_note(pool: &SqlitePool, note: &NotepadNote) -> Result<NotepadNote, AppError> {
    sqlx::query(
        "INSERT INTO notepad_notes (
            id, source_id, display_title, original_path, is_saved, is_deleted,
            first_seen_at, last_seen_at, deleted_at, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(source_id) DO UPDATE SET
            display_title = excluded.display_title,
            original_path = COALESCE(excluded.original_path, notepad_notes.original_path),
            is_saved = excluded.is_saved,
            is_deleted = excluded.is_deleted,
            last_seen_at = excluded.last_seen_at,
            deleted_at = excluded.deleted_at,
            updated_at = excluded.updated_at",
    )
    .bind(&note.id)
    .bind(&note.source_id)
    .bind(&note.display_title)
    .bind(&note.original_path)
    .bind(if note.is_saved { 1i64 } else { 0i64 })
    .bind(if note.is_deleted { 1i64 } else { 0i64 })
    .bind(&note.first_seen_at)
    .bind(&note.last_seen_at)
    .bind(&note.deleted_at)
    .bind(&note.created_at)
    .bind(&note.updated_at)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("노트 저장 실패: {e}")))?;

    get_note_by_source_id(pool, &note.source_id)
        .await?
        .ok_or_else(|| AppError::Database("저장된 노트를 찾을 수 없습니다".into()))
}

pub async fn mark_note_deleted(
    pool: &SqlitePool,
    source_id: &str,
    deleted_at: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query(
        "UPDATE notepad_notes
         SET is_deleted = 1, deleted_at = ?2, updated_at = ?2
         WHERE source_id = ?1 AND is_deleted = 0",
    )
    .bind(source_id)
    .bind(deleted_at)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("노트 삭제 처리 실패: {e}")))?;

    Ok(result.rows_affected() > 0)
}

pub async fn get_note(pool: &SqlitePool, id: &str) -> Result<Option<NotepadNote>, AppError> {
    let row = sqlx::query("SELECT * FROM notepad_notes WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Database(format!("노트 조회 실패: {e}")))?;

    row.map(|r| map_note_row(&r)).transpose()
}

pub async fn get_note_by_source_id(
    pool: &SqlitePool,
    source_id: &str,
) -> Result<Option<NotepadNote>, AppError> {
    let row = sqlx::query("SELECT * FROM notepad_notes WHERE source_id = ?1")
        .bind(source_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Database(format!("노트 조회 실패: {e}")))?;

    row.map(|r| map_note_row(&r)).transpose()
}

pub async fn list_notes(
    pool: &SqlitePool,
    filter: &NoteFilter,
) -> Result<Vec<NotepadNote>, AppError> {
    let mut sql = String::from("SELECT * FROM notepad_notes WHERE 1=1");

    if let Some(deleted) = filter.is_deleted {
        if deleted {
            sql.push_str(" AND is_deleted = 1");
        } else {
            sql.push_str(" AND is_deleted = 0");
        }
    }

    let has_query = filter
        .query
        .as_ref()
        .map(|q| !q.trim().is_empty())
        .unwrap_or(false);
    if has_query {
        sql.push_str(
            " AND (
                display_title LIKE ?1
                OR original_path LIKE ?1
                OR id IN (
                    SELECT note_id FROM notepad_snapshots WHERE recovered_text LIKE ?1
                )
            )",
        );
    }

    sql.push_str(" ORDER BY last_seen_at DESC");

    if let Some(limit) = filter.limit {
        sql.push_str(&format!(" LIMIT {}", limit.max(1)));
        if let Some(offset) = filter.offset {
            sql.push_str(&format!(" OFFSET {}", offset.max(0)));
        }
    }

    let mut query = sqlx::query(&sql);
    if has_query {
        let pattern = format!("%{}%", filter.query.as_deref().unwrap().trim());
        query = query.bind(pattern);
    }

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(format!("노트 목록 조회 실패: {e}")))?;

    rows.iter().map(map_note_row).collect()
}

// ---------------------------------------------------------------- Snapshots

fn map_snapshot_row(row: &sqlx::sqlite::SqliteRow) -> Result<NotepadSnapshot, AppError> {
    let source_type_str: String = row
        .try_get("source_type")
        .map_err(|e| AppError::Database(e.to_string()))?;
    let parse_status_str: String = row
        .try_get("parse_status")
        .map_err(|e| AppError::Database(e.to_string()))?;

    Ok(NotepadSnapshot {
        id: row
            .try_get("id")
            .map_err(|e| AppError::Database(e.to_string()))?,
        note_id: row
            .try_get("note_id")
            .map_err(|e| AppError::Database(e.to_string()))?,
        content_hash: row
            .try_get("content_hash")
            .map_err(|e| AppError::Database(e.to_string()))?,
        raw_snapshot_path: row
            .try_get("raw_snapshot_path")
            .map_err(|e| AppError::Database(e.to_string()))?,
        recovered_text: row.try_get("recovered_text").ok().flatten(),
        source_type: SourceType::from_str(&source_type_str).unwrap(),
        source_filename: row
            .try_get("source_filename")
            .map_err(|e| AppError::Database(e.to_string()))?,
        source_size: row
            .try_get("source_size")
            .map_err(|e| AppError::Database(e.to_string()))?,
        parse_status: ParseStatus::from_str(&parse_status_str).unwrap(),
        notepad_format_version: row.try_get("notepad_format_version").ok().flatten(),
        captured_at: row
            .try_get("captured_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| AppError::Database(e.to_string()))?,
    })
}

pub async fn insert_snapshot_metadata(
    pool: &SqlitePool,
    snapshot: &NotepadSnapshot,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO notepad_snapshots (
            id, note_id, content_hash, raw_snapshot_path, recovered_text,
            source_type, source_filename, source_size, parse_status,
            notepad_format_version, captured_at, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&snapshot.id)
    .bind(&snapshot.note_id)
    .bind(&snapshot.content_hash)
    .bind(&snapshot.raw_snapshot_path)
    .bind(&snapshot.recovered_text)
    .bind(snapshot.source_type.as_str())
    .bind(&snapshot.source_filename)
    .bind(snapshot.source_size)
    .bind(snapshot.parse_status.as_str())
    .bind(snapshot.notepad_format_version)
    .bind(&snapshot.captured_at)
    .bind(&snapshot.created_at)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("스냅샷 메타데이터 저장 실패: {e}")))?;

    Ok(())
}

pub async fn list_note_snapshots(
    pool: &SqlitePool,
    note_id: &str,
) -> Result<Vec<NotepadSnapshot>, AppError> {
    let rows = sqlx::query(
        "SELECT * FROM notepad_snapshots
         WHERE note_id = ?1
         ORDER BY captured_at DESC",
    )
    .bind(note_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("스냅샷 목록 조회 실패: {e}")))?;

    rows.iter().map(map_snapshot_row).collect()
}

pub async fn get_latest_snapshot_for_note(
    pool: &SqlitePool,
    note_id: &str,
) -> Result<Option<NotepadSnapshot>, AppError> {
    let row = sqlx::query(
        "SELECT * FROM notepad_snapshots
         WHERE note_id = ?1
         ORDER BY captured_at DESC
         LIMIT 1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("최신 스냅샷 조회 실패: {e}")))?;

    row.map(|r| map_snapshot_row(&r)).transpose()
}

// ---------------------------------------------------------------- Sessions

pub async fn insert_session_snapshot(
    pool: &SqlitePool,
    session: &NotepadSession,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO notepad_sessions (
            id, source_filename, content_hash, raw_snapshot_path, captured_at, parse_status, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&session.id)
    .bind(&session.source_filename)
    .bind(&session.content_hash)
    .bind(&session.raw_snapshot_path)
    .bind(&session.captured_at)
    .bind(session.parse_status.as_str())
    .bind(&session.created_at)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("세션 스냅샷 저장 실패: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------- Statistics

pub async fn get_storage_statistics(pool: &SqlitePool) -> Result<StorageStatistics, AppError> {
    let notes_row = sqlx::query(
        "SELECT
            COUNT(*) AS total_notes,
            COALESCE(SUM(CASE WHEN is_deleted = 0 THEN 1 ELSE 0 END), 0) AS active_notes,
            COALESCE(SUM(CASE WHEN is_deleted = 1 THEN 1 ELSE 0 END), 0) AS deleted_notes
         FROM notepad_notes",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("노트 통계 조회 실패: {e}")))?;

    let snapshots_row = sqlx::query(
        "SELECT
            COUNT(*) AS total_snapshots,
            COALESCE(SUM(source_size), 0) AS total_snapshot_bytes,
            MIN(captured_at) AS oldest_snapshot_at,
            MAX(captured_at) AS newest_snapshot_at
         FROM notepad_snapshots",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("스냅샷 통계 조회 실패: {e}")))?;

    let sessions_row = sqlx::query("SELECT COUNT(*) AS total_sessions FROM notepad_sessions")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("세션 통계 조회 실패: {e}")))?;

    Ok(StorageStatistics {
        total_notes: notes_row.try_get("total_notes").unwrap_or(0),
        active_notes: notes_row.try_get("active_notes").unwrap_or(0),
        deleted_notes: notes_row.try_get("deleted_notes").unwrap_or(0),
        total_snapshots: snapshots_row.try_get("total_snapshots").unwrap_or(0),
        total_sessions: sessions_row.try_get("total_sessions").unwrap_or(0),
        total_snapshot_bytes: snapshots_row.try_get("total_snapshot_bytes").unwrap_or(0),
        oldest_snapshot_at: snapshots_row.try_get("oldest_snapshot_at").ok().flatten(),
        newest_snapshot_at: snapshots_row.try_get("newest_snapshot_at").ok().flatten(),
    })
}
