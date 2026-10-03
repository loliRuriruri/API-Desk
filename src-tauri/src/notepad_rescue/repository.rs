use std::str::FromStr;

use sqlx::{Row, SqlitePool};

use super::models::{
    NoteFilter, NoteSummaryDto, NotepadNote, NotepadRescueSettings, NotepadSession,
    NotepadSnapshot, ParseStatus, SearchResultDto, SourceType, StorageStatistics,
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

pub async fn list_notes_summary(
    pool: &SqlitePool,
    filter: &NoteFilter,
) -> Result<Vec<NoteSummaryDto>, AppError> {
    let mut sql = String::from(
        "SELECT
            n.id,
            n.source_id,
            n.display_title,
            n.original_path,
            n.is_saved,
            n.is_deleted,
            n.first_seen_at,
            n.last_seen_at,
            n.deleted_at,
            (SELECT COUNT(*) FROM notepad_snapshots s WHERE s.note_id = n.id) AS version_count,
            s.id AS last_snapshot_id,
            s.captured_at AS last_snapshot_at,
            s.content_hash AS last_content_hash,
            s.source_size AS last_source_size,
            s.parse_status AS last_parse_status,
            s.recovered_text AS last_recovered_text
         FROM notepad_notes n
         LEFT JOIN notepad_snapshots s ON s.id = (
             SELECT id FROM notepad_snapshots WHERE note_id = n.id ORDER BY captured_at DESC, rowid DESC LIMIT 1
         )
         WHERE 1=1",
    );

    if let Some(deleted) = filter.is_deleted {
        if deleted {
            sql.push_str(" AND n.is_deleted = 1");
        } else {
            sql.push_str(" AND n.is_deleted = 0");
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
                n.display_title LIKE ?1
                OR n.original_path LIKE ?1
                OR n.id IN (
                    SELECT note_id FROM notepad_snapshots WHERE recovered_text LIKE ?1
                )
            )",
        );
    }

    sql.push_str(" ORDER BY n.last_seen_at DESC");

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
        .map_err(|e| AppError::Database(format!("노트 요약 목록 조회 실패: {e}")))?;

    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        let raw_preview: Option<String> = row.try_get("last_recovered_text").ok();
        let preview_text = raw_preview.map(|text| {
            let collapsed: String = text
                .chars()
                .map(|c| {
                    if c == '\r' || c == '\n' || c == '\t' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            let trimmed = collapsed.trim();
            if trimmed.chars().count() > 120 {
                let s: String = trimmed.chars().take(120).collect();
                format!("{s}...")
            } else {
                trimmed.to_string()
            }
        });

        let parse_status_str: Option<String> = row.try_get("last_parse_status").ok();
        let last_parse_status = parse_status_str.map(|s| ParseStatus::from_str(&s).unwrap());

        list.push(NoteSummaryDto {
            id: row.try_get("id").unwrap_or_default(),
            source_id: row.try_get("source_id").unwrap_or_default(),
            display_title: row.try_get("display_title").unwrap_or_default(),
            original_path: row.try_get("original_path").ok(),
            is_saved: row.try_get::<i64, _>("is_saved").unwrap_or(0) != 0,
            is_deleted: row.try_get::<i64, _>("is_deleted").unwrap_or(0) != 0,
            first_seen_at: row.try_get("first_seen_at").unwrap_or_default(),
            last_seen_at: row.try_get("last_seen_at").unwrap_or_default(),
            deleted_at: row.try_get("deleted_at").ok(),
            version_count: row.try_get("version_count").unwrap_or(0),
            last_snapshot_id: row.try_get("last_snapshot_id").ok(),
            last_snapshot_at: row.try_get("last_snapshot_at").ok(),
            last_content_hash: row.try_get("last_content_hash").ok(),
            last_source_size: row.try_get("last_source_size").ok(),
            last_parse_status,
            preview_text,
        });
    }

    Ok(list)
}

/// LIKE 패턴 이스케이프: %, _, \ 문자를 이스케이프 처리합니다.
pub fn escape_like_pattern(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for c in input.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 텍스트에서 검색어 주변의 제한된 크기 스니펫을 안전하게 추출합니다 (최대 max_chars 문자).
pub fn extract_snippet(text: &str, query: &str, max_chars: usize) -> Option<String> {
    let clean_text: String = text
        .chars()
        .map(|c| {
            if c == '\r' || c == '\n' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();

    let lower_text = clean_text.to_lowercase();
    let lower_query = query.to_lowercase();

    let byte_pos = lower_text
        .find(&lower_query)
        .or_else(|| clean_text.find(query))?;

    let char_pos = clean_text[..byte_pos].chars().count();
    let query_chars = query.chars().count();
    let total_chars = clean_text.chars().count();

    let context_radius = if max_chars > query_chars {
        (max_chars - query_chars) / 2
    } else {
        20
    };

    let start_char = char_pos.saturating_sub(context_radius);
    let end_char = (char_pos + query_chars + context_radius).min(total_chars);

    let snippet_raw: String = clean_text
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .collect();
    let trimmed = snippet_raw.trim();

    let mut result = String::new();
    if start_char > 0 {
        result.push_str("...");
    }
    result.push_str(trimmed);
    if end_char < total_chars {
        result.push_str("...");
    }

    Some(result)
}

/// 노트 제목, 경로, 본문 스냅샷을 대상으로 안전하게 검색합니다.
pub async fn search_notes(
    pool: &SqlitePool,
    query: &str,
    scope: Option<&str>,
    include_deleted: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchResultDto>, AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let bounded_query: String = if trimmed.chars().count() > 200 {
        trimmed.chars().take(200).collect()
    } else {
        trimmed.to_string()
    };

    let pattern = format!("%{}%", escape_like_pattern(&bounded_query));
    let normalized_scope = scope.unwrap_or("ALL").trim().to_ascii_uppercase();

    let mut sql = String::from(
        "SELECT
            n.id AS note_id,
            n.source_id,
            n.display_title,
            n.original_path,
            n.is_saved,
            n.is_deleted,
            n.last_seen_at,
            (SELECT COUNT(*) FROM notepad_snapshots s WHERE s.note_id = n.id) AS version_count,
            (
                SELECT captured_at FROM notepad_snapshots s
                WHERE s.note_id = n.id ORDER BY s.captured_at DESC, s.rowid DESC LIMIT 1
            ) AS last_snapshot_at,
            CASE
                WHEN n.display_title LIKE ?1 ESCAPE '\\' THEN 'TITLE'
                WHEN n.original_path LIKE ?1 ESCAPE '\\' THEN 'PATH'
                ELSE 'CONTENT'
            END AS matched_scope,
            (
                SELECT recovered_text FROM notepad_snapshots s
                WHERE s.note_id = n.id AND s.recovered_text LIKE ?1 ESCAPE '\\'
                ORDER BY s.captured_at DESC, s.rowid DESC LIMIT 1
            ) AS matching_recovered_text
         FROM notepad_notes n
         WHERE 1=1",
    );

    if !include_deleted {
        sql.push_str(" AND n.is_deleted = 0");
    }

    match normalized_scope.as_str() {
        "TITLE" => {
            sql.push_str(" AND n.display_title LIKE ?1 ESCAPE '\\'");
        }
        "PATH" => {
            sql.push_str(" AND n.original_path LIKE ?1 ESCAPE '\\'");
        }
        "CONTENT" => {
            sql.push_str(" AND EXISTS (SELECT 1 FROM notepad_snapshots s WHERE s.note_id = n.id AND s.recovered_text LIKE ?1 ESCAPE '\\')");
        }
        _ => {
            sql.push_str(
                " AND (
                    n.display_title LIKE ?1 ESCAPE '\\'
                    OR n.original_path LIKE ?1 ESCAPE '\\'
                    OR EXISTS (SELECT 1 FROM notepad_snapshots s WHERE s.note_id = n.id AND s.recovered_text LIKE ?1 ESCAPE '\\')
                )",
            );
        }
    }

    sql.push_str(" ORDER BY n.last_seen_at DESC");

    let bounded_limit = limit.clamp(1, 200);
    let bounded_offset = offset.max(0);
    sql.push_str(&format!(
        " LIMIT {} OFFSET {}",
        bounded_limit, bounded_offset
    ));

    let rows = sqlx::query(&sql)
        .bind(&pattern)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(format!("노트 검색 실패: {e}")))?;

    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let matched_scope: String = row
            .try_get("matched_scope")
            .unwrap_or_else(|_| "CONTENT".to_string());
        let matching_recovered_text: Option<String> = row.try_get("matching_recovered_text").ok();
        let display_title: String = row.try_get("display_title").unwrap_or_default();
        let original_path: Option<String> = row.try_get("original_path").ok();

        let snippet = match matched_scope.as_str() {
            "TITLE" => extract_snippet(&display_title, &bounded_query, 120),
            "PATH" => original_path
                .as_deref()
                .and_then(|p| extract_snippet(p, &bounded_query, 120)),
            _ => matching_recovered_text
                .as_deref()
                .and_then(|t| extract_snippet(t, &bounded_query, 120)),
        };

        results.push(SearchResultDto {
            note_id: row.try_get("note_id").unwrap_or_default(),
            source_id: row.try_get("source_id").unwrap_or_default(),
            display_title,
            original_path,
            is_saved: row.try_get::<i64, _>("is_saved").unwrap_or(0) != 0,
            is_deleted: row.try_get::<i64, _>("is_deleted").unwrap_or(0) != 0,
            last_seen_at: row.try_get("last_seen_at").unwrap_or_default(),
            matched_scope,
            snippet,
            version_count: row.try_get("version_count").unwrap_or(0),
            last_snapshot_at: row.try_get("last_snapshot_at").ok(),
        });
    }

    Ok(results)
}

pub async fn get_note_summary(
    pool: &SqlitePool,
    note_id: &str,
) -> Result<Option<NoteSummaryDto>, AppError> {
    let row = sqlx::query(
        "SELECT
            n.id,
            n.source_id,
            n.display_title,
            n.original_path,
            n.is_saved,
            n.is_deleted,
            n.first_seen_at,
            n.last_seen_at,
            n.deleted_at,
            (SELECT COUNT(*) FROM notepad_snapshots s WHERE s.note_id = n.id) AS version_count,
            s.id AS last_snapshot_id,
            s.captured_at AS last_snapshot_at,
            s.content_hash AS last_content_hash,
            s.source_size AS last_source_size,
            s.parse_status AS last_parse_status,
            s.recovered_text AS last_recovered_text
         FROM notepad_notes n
         LEFT JOIN notepad_snapshots s ON s.id = (
             SELECT id FROM notepad_snapshots WHERE note_id = n.id ORDER BY captured_at DESC, rowid DESC LIMIT 1
         )
         WHERE n.id = ?1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("노트 요약 조회 실패: {e}")))?;

    let Some(row) = row else {
        return Ok(None);
    };

    let raw_preview: Option<String> = row.try_get("last_recovered_text").ok();
    let preview_text = raw_preview.map(|text| {
        let collapsed: String = text
            .chars()
            .map(|c| {
                if c == '\r' || c == '\n' || c == '\t' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let trimmed = collapsed.trim();
        if trimmed.chars().count() > 120 {
            let s: String = trimmed.chars().take(120).collect();
            format!("{s}...")
        } else {
            trimmed.to_string()
        }
    });

    let parse_status_str: Option<String> = row.try_get("last_parse_status").ok();
    let last_parse_status = parse_status_str.map(|s| ParseStatus::from_str(&s).unwrap());

    Ok(Some(NoteSummaryDto {
        id: row.try_get("id").unwrap_or_default(),
        source_id: row.try_get("source_id").unwrap_or_default(),
        display_title: row.try_get("display_title").unwrap_or_default(),
        original_path: row.try_get("original_path").ok(),
        is_saved: row.try_get::<i64, _>("is_saved").unwrap_or(0) != 0,
        is_deleted: row.try_get::<i64, _>("is_deleted").unwrap_or(0) != 0,
        first_seen_at: row.try_get("first_seen_at").unwrap_or_default(),
        last_seen_at: row.try_get("last_seen_at").unwrap_or_default(),
        deleted_at: row.try_get("deleted_at").ok(),
        version_count: row.try_get("version_count").unwrap_or(0),
        last_snapshot_id: row.try_get("last_snapshot_id").ok(),
        last_snapshot_at: row.try_get("last_snapshot_at").ok(),
        last_content_hash: row.try_get("last_content_hash").ok(),
        last_source_size: row.try_get("last_source_size").ok(),
        last_parse_status,
        preview_text,
    }))
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
         ORDER BY captured_at DESC, rowid DESC",
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
         ORDER BY captured_at DESC, rowid DESC
         LIMIT 1",
    )
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("최신 스냅샷 조회 실패: {e}")))?;

    row.map(|r| map_snapshot_row(&r)).transpose()
}

pub async fn get_snapshot(
    pool: &SqlitePool,
    snapshot_id: &str,
) -> Result<Option<NotepadSnapshot>, AppError> {
    let row = sqlx::query(
        "SELECT * FROM notepad_snapshots
         WHERE id = ?1
         LIMIT 1",
    )
    .bind(snapshot_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("스냅샷 조회 실패: {e}")))?;

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
