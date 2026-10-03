use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tokio::sync::Mutex;

use super::models::NotepadRescueSettings;
use super::snapshot::resolve_blob_path;
use crate::error::AppError;

/// GC 동시성 락: 캡처 파이프라인과 GC 실행 간의 충돌을 방지합니다.
pub static GC_MUTEX: std::sync::OnceLock<Arc<Mutex<()>>> = std::sync::OnceLock::new();

pub fn get_gc_mutex() -> Arc<Mutex<()>> {
    GC_MUTEX
        .get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
}

/// 임시 파일(.tmp) 안전 유예 기간: 1시간
pub const TMP_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// 고아 블롭(Orphan Blob) 안전 유예 기간: 24시간
pub const ORPHAN_GRACE_PERIOD: Duration = Duration::from_secs(86400);

/// GC 실행 계획 (Dry Run 결과).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GcPlan {
    pub expired_snapshot_ids: Vec<String>,
    pub expired_note_ids: Vec<String>,
    pub unreferenced_blob_hashes: Vec<String>,
    pub reclaimable_blob_bytes: u64,
    pub stale_tmp_files: Vec<String>,
    pub stale_tmp_bytes: u64,
    pub orphan_blobs_to_delete: Vec<String>,
    pub orphan_blob_bytes: u64,
    pub total_reclaimable_bytes: u64,
    pub current_storage_bytes: u64,
    pub estimated_storage_after_bytes: u64,
    pub has_quota_pressure: bool,
}

/// GC 실행 결과 보고서.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub deleted_snapshots_count: usize,
    pub deleted_notes_count: usize,
    pub deleted_blobs_count: usize,
    pub deleted_blobs_bytes: u64,
    pub deleted_tmp_count: usize,
    pub deleted_tmp_bytes: u64,
    pub deleted_orphans_count: usize,
    pub total_reclaimed_bytes: u64,
    pub has_quota_pressure: bool,
    pub storage_after_bytes: u64,
}

/// 상세 스토리지 현황 DTO.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageDetailDto {
    pub logical_snapshot_count: i64,
    pub physical_blob_count: i64,
    pub logical_source_bytes: u64,
    pub physical_disk_bytes: u64,
    pub deduplicated_bytes: u64,
    pub reclaimable_bytes: u64,
    pub orphan_blob_count: i64,
    pub tmp_bytes: u64,
    pub max_storage_bytes: u64,
    pub quota_percentage: f64,
    pub has_quota_pressure: bool,
}

/// ISO 8601 / RFC 3339 타임스탬프를 Unix 에포크 초로 변환합니다.
pub fn parse_timestamp_epoch(ts: &str) -> i64 {
    if let Ok(dt) = chrono_or_simple_parse(ts) {
        dt
    } else {
        0
    }
}

fn chrono_or_simple_parse(ts: &str) -> Result<i64, ()> {
    // RFC 3339 간이 파서
    let trimmed = ts.trim();
    if trimmed.len() < 19 {
        return Err(());
    }
    let parts: Vec<&str> = trimmed.split('T').collect();
    if parts.len() != 2 {
        return Err(());
    }
    let ymd: Vec<i64> = parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
    let time_clean = parts[1].trim_end_matches('Z');
    let hms: Vec<i64> = time_clean
        .split(':')
        .take(3)
        .filter_map(|s| s.split('.').next().unwrap_or(s).parse().ok())
        .collect();

    if ymd.len() == 3 && hms.len() >= 2 {
        let (y, m, d) = (ymd[0], ymd[1], ymd[2]);
        let h = hms[0];
        let min = hms[1];
        let s = if hms.len() > 2 { hms[2] } else { 0 };

        // 간단한 에포크 계산 (윤년 대략 반영)
        let mut days = (y - 1970) * 365 + (y - 1969) / 4;
        let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        if (1..=12).contains(&m) {
            days += month_days[(m - 1) as usize];
            if m > 2 && (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) {
                days += 1;
            }
        }
        days += d - 1;
        Ok(days * 86400 + h * 3600 + min * 60 + s)
    } else {
        Err(())
    }
}

fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let unced = strip_unc_prefix(path);
    let mut normalized = PathBuf::new();
    for comp in unced.components() {
        match comp {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            c => normalized.push(c),
        }
    }
    normalized
}

/// 경로 탐색(Path Traversal) 공격 방어: 대상 파일이 정확히 storage_root 하위에 위치하는지 검증합니다.
pub fn verify_path_within_root(storage_root: &Path, target_path: &Path) -> Result<(), AppError> {
    let canon_root = if storage_root.exists() {
        storage_root
            .canonicalize()
            .unwrap_or_else(|_| storage_root.to_path_buf())
    } else {
        storage_root.to_path_buf()
    };
    let norm_root = normalize_path(&canon_root);

    let canon_target = if target_path.exists() {
        target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.to_path_buf())
    } else {
        target_path.to_path_buf()
    };
    let norm_target = normalize_path(&canon_target);

    let root_str = norm_root.to_string_lossy().to_lowercase().replace('/', "\\");
    let target_str = norm_target.to_string_lossy().to_lowercase().replace('/', "\\");

    let is_inside = if target_str == root_str {
        true
    } else if target_str.starts_with(&root_str) {
        let remainder = &target_str[root_str.len()..];
        remainder.starts_with('\\')
    } else {
        false
    };

    if is_inside {
        Ok(())
    } else {
        Err(AppError::InvalidRequest(format!(
            "보안 위반: 허용되지 않은 디렉터리 경로 접근입니다: {}",
            target_path.display()
        )))
    }
}

/// GC 대상 스냅샷 및 파일들을 식별하여 안전한 실행 계획(Dry Run)을 생성합니다.
pub async fn plan_gc(
    pool: &SqlitePool,
    storage_root: &Path,
    settings: &NotepadRescueSettings,
    now_epoch: i64,
) -> Result<GcPlan, AppError> {
    let mut plan = GcPlan::default();

    // 1. 활성/삭제 노트 조회
    let notes_rows = sqlx::query(
        "SELECT id, is_deleted, deleted_at FROM notepad_notes ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(format!("노트 목록 조회 실패: {e}")))?;

    let active_retention_secs = (settings.retention_days as i64) * 86400;
    let deleted_retention_secs = (settings.deleted_retention_days as i64) * 86400;
    let active_cutoff = now_epoch - active_retention_secs;

    let mut expired_snapshot_ids = Vec::new();
    let mut expired_note_ids = Vec::new();

    for row in &notes_rows {
        let note_id: String = row.try_get("id").unwrap_or_default();
        let is_deleted: bool = row.try_get::<i64, _>("is_deleted").unwrap_or(0) != 0;
        let deleted_at_str: Option<String> = row.try_get("deleted_at").ok().flatten();

        let snapshots = sqlx::query(
            "SELECT id, captured_at, source_size, content_hash
             FROM notepad_snapshots
             WHERE note_id = ?1
             ORDER BY captured_at DESC, rowid DESC",
        )
        .bind(&note_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Database(format!("스냅샷 조회 실패: {e}")))?;

        if is_deleted {
            let deleted_epoch = deleted_at_str
                .as_deref()
                .map(parse_timestamp_epoch)
                .unwrap_or(0);

            if deleted_epoch > 0 && now_epoch >= deleted_epoch + deleted_retention_secs {
                // 삭제 보관 기간 만료: 노트와 모든 스냅샷 제거 대상
                expired_note_ids.push(note_id);
                for snap_row in snapshots {
                    let s_id: String = snap_row.try_get("id").unwrap_or_default();
                    expired_snapshot_ids.push(s_id);
                }
            } else {
                // 삭제 보관 기간 내: 최소 복원 보장(최신 1개 보존), 이전 만료 스냅샷만 정리
                for (idx, snap_row) in snapshots.into_iter().enumerate() {
                    if idx == 0 {
                        continue; // 최신 스냅샷 절대 보존
                    }
                    let captured_str: String = snap_row.try_get("captured_at").unwrap_or_default();
                    let captured_epoch = parse_timestamp_epoch(&captured_str);
                    if captured_epoch > 0 && captured_epoch <= active_cutoff {
                        let s_id: String = snap_row.try_get("id").unwrap_or_default();
                        expired_snapshot_ids.push(s_id);
                    }
                }
            }
        } else {
            // 활성 노트: 최소 복원 보장(최신 1개 절대 보존), 나머지 중 retention_days 초과한 버전만 정리
            for (idx, snap_row) in snapshots.into_iter().enumerate() {
                if idx == 0 {
                    continue; // 최신 스냅샷 절대 보존 (Invariant)
                }
                let captured_str: String = snap_row.try_get("captured_at").unwrap_or_default();
                let captured_epoch = parse_timestamp_epoch(&captured_str);
                if captured_epoch > 0 && captured_epoch <= active_cutoff {
                    let s_id: String = snap_row.try_get("id").unwrap_or_default();
                    expired_snapshot_ids.push(s_id);
                }
            }
        }
    }

    // 2. 물리적 CAS 블롭 다중 테이블 참조 계수 계산
    // notepad_snapshots(보존 대상) + notepad_sessions(창 상태)
    let unreferenced_hashes = find_unreferenced_hashes(pool, &expired_snapshot_ids).await?;

    let mut reclaimable_blob_bytes = 0u64;
    for hash in &unreferenced_hashes {
        let rel_path = super::snapshot::relative_blob_path(hash);
        let abs_path = resolve_blob_path(storage_root, &rel_path);
        if abs_path.exists() {
            if let Ok(meta) = std::fs::metadata(&abs_path) {
                reclaimable_blob_bytes += meta.len();
            }
        }
    }

    // 3. 임시 파일(.tmp) 검사 (> 1시간)
    let (stale_tmps, stale_tmp_bytes) = scan_stale_tmp_files(storage_root, TMP_GRACE_PERIOD);

    // 4. 고아 블롭(Orphan) 검사 (> 24시간)
    let (orphan_blobs, orphan_blob_bytes) =
        scan_orphan_blobs(pool, storage_root, ORPHAN_GRACE_PERIOD).await?;

    // 5. 물리 디스크 전체 사용량 계산
    let current_storage_bytes = calculate_dir_size(storage_root);
    let total_reclaimable = reclaimable_blob_bytes + stale_tmp_bytes + orphan_blob_bytes;
    let estimated_after = current_storage_bytes.saturating_sub(total_reclaimable);

    // 6. Quota 압박 평가
    let has_quota_pressure = current_storage_bytes > settings.max_storage_bytes
        && estimated_after > settings.max_storage_bytes;

    plan.expired_snapshot_ids = expired_snapshot_ids;
    plan.expired_note_ids = expired_note_ids;
    plan.unreferenced_blob_hashes = unreferenced_hashes;
    plan.reclaimable_blob_bytes = reclaimable_blob_bytes;
    plan.stale_tmp_files = stale_tmps;
    plan.stale_tmp_bytes = stale_tmp_bytes;
    plan.orphan_blobs_to_delete = orphan_blobs;
    plan.orphan_blob_bytes = orphan_blob_bytes;
    plan.total_reclaimable_bytes = total_reclaimable;
    plan.current_storage_bytes = current_storage_bytes;
    plan.estimated_storage_after_bytes = estimated_after;
    plan.has_quota_pressure = has_quota_pressure;

    Ok(plan)
}

/// 참조가 완전히 0이 되는 물리 CAS 블롭 해시 목록을 반환합니다.
async fn find_unreferenced_hashes(
    pool: &SqlitePool,
    candidate_expired_snapshots: &[String],
) -> Result<Vec<String>, AppError> {
    if candidate_expired_snapshots.is_empty() {
        return Ok(Vec::new());
    }

    // 후보 스냅샷들의 고유 해시 추출
    let mut candidate_hashes = HashSet::new();
    for chunk in candidate_expired_snapshots.chunks(200) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT content_hash FROM notepad_snapshots WHERE id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql);
        for id in chunk {
            query = query.bind(id);
        }
        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Database(format!("후보 해시 조회 실패: {e}")))?;
        for r in rows {
            let h: String = r.try_get("content_hash").unwrap_or_default();
            if !h.is_empty() {
                candidate_hashes.insert(h);
            }
        }
    }

    if candidate_hashes.is_empty() {
        return Ok(Vec::new());
    }

    // 잔여 참조가 남아있는 해시들을 DB에서 역조회 (스냅샷 + 세션)
    let mut unreferenced = Vec::new();
    let expired_set: HashSet<&str> = candidate_expired_snapshots.iter().map(|s| s.as_str()).collect();

    for hash in candidate_hashes {
        // 1. 잔여 notepad_snapshots 참조 수 확인:
        // 이 해시를 참조하는 스냅샷 목록 중 삭제 대상이 아닌 스냅샷이 존재하는지 확인
        let snap_rows = sqlx::query("SELECT id FROM notepad_snapshots WHERE content_hash = ?1")
            .bind(&hash)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Database(format!("스냅샷 상세 참조 확인 실패: {e}")))?;

        let alive_snaps = snap_rows
            .iter()
            .filter(|r| {
                let id: String = r.try_get("id").unwrap_or_default();
                !expired_set.contains(id.as_str())
            })
            .count();

        // 2. notepad_sessions 참조 수 확인
        let session_ref_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notepad_sessions WHERE content_hash = ?1",
        )
        .bind(&hash)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(format!("세션 참조 확인 실패: {e}")))?;

        if alive_snaps == 0 && session_ref_count == 0 {
            unreferenced.push(hash);
        }
    }

    Ok(unreferenced)
}

/// 디렉터리 내 .tmp 파일 중 지정된 경과 시간(grace)을 초과한 파일 목록과 바이트를 수집합니다.
pub fn scan_stale_tmp_files(storage_root: &Path, grace: Duration) -> (Vec<String>, u64) {
    let mut files = Vec::new();
    let mut bytes = 0u64;

    let now = SystemTime::now();

    let walker = walkdir_simple(storage_root);
    for path in walker {
        if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
            if verify_path_within_root(storage_root, &path).is_err() {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&path) {
                let modified = meta.modified().unwrap_or(now);
                if let Ok(elapsed) = now.duration_since(modified) {
                    if elapsed >= grace {
                        bytes += meta.len();
                        files.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    (files, bytes)
}

/// 디렉터리 내 .bin 파일 중 DB에 참조가 없는 고아 블롭(Orphan) 중 24시간 이상 지난 블롭을 수집합니다.
pub async fn scan_orphan_blobs(
    pool: &SqlitePool,
    storage_root: &Path,
    grace: Duration,
) -> Result<(Vec<String>, u64), AppError> {
    let mut orphan_files = Vec::new();
    let mut orphan_bytes = 0u64;

    let snapshots_dir = storage_root.join("snapshots");
    if !snapshots_dir.is_dir() {
        return Ok((orphan_files, orphan_bytes));
    }

    let now = SystemTime::now();
    let bin_files = walkdir_simple(&snapshots_dir);

    for path in bin_files {
        if path.extension().and_then(|e| e.to_str()) == Some("bin") {
            if verify_path_within_root(storage_root, &path).is_err() {
                continue;
            }
            let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if file_stem.len() != 64 {
                continue;
            }

            // DB 참조 확인
            let in_snapshots: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM notepad_snapshots WHERE content_hash = ?1",
            )
            .bind(file_stem)
            .fetch_one(pool)
            .await
            .unwrap_or(1);

            let in_sessions: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM notepad_sessions WHERE content_hash = ?1",
            )
            .bind(file_stem)
            .fetch_one(pool)
            .await
            .unwrap_or(1);

            if in_snapshots == 0 && in_sessions == 0 {
                if let Ok(meta) = std::fs::metadata(&path) {
                    let modified = meta.modified().unwrap_or(now);
                    if let Ok(elapsed) = now.duration_since(modified) {
                        if elapsed >= grace {
                            orphan_bytes += meta.len();
                            orphan_files.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    Ok((orphan_files, orphan_bytes))
}

/// 디렉터리 내 모든 일반 파일 경로를 재귀 탐색합니다.
fn walkdir_simple(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];

    while let Some(dir) = dirs.pop() {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.is_file() {
                    files.push(p);
                }
            }
        }
    }

    files
}

/// 디렉터리 내 모든 파일의 총 바이트를 계산합니다.
pub fn calculate_dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    for f in walkdir_simple(root) {
        if let Ok(meta) = std::fs::metadata(f) {
            total += meta.len();
        }
    }
    total
}

/// GC 계획을 안전하게 물리 파일 및 DB에 반영합니다.
pub async fn execute_gc(
    pool: &SqlitePool,
    storage_root: &Path,
    settings: &NotepadRescueSettings,
    now_epoch: i64,
) -> Result<GcReport, AppError> {
    let gc_lock = get_gc_mutex();
    let _guard = gc_lock.lock().await;

    // 1. 삭제 직전 최신 조건 재검증 (Revalidation)
    let plan = plan_gc(pool, storage_root, settings, now_epoch).await?;

    let mut report = GcReport {
        deleted_snapshots_count: 0,
        deleted_notes_count: 0,
        deleted_blobs_count: 0,
        deleted_blobs_bytes: 0,
        deleted_tmp_count: 0,
        deleted_tmp_bytes: 0,
        deleted_orphans_count: 0,
        total_reclaimed_bytes: 0,
        has_quota_pressure: plan.has_quota_pressure,
        storage_after_bytes: 0,
    };

    // 2. 물리 CAS 블롭 안전 삭제
    for hash in &plan.unreferenced_blob_hashes {
        let rel_path = super::snapshot::relative_blob_path(hash);
        let abs_path = resolve_blob_path(storage_root, &rel_path);

        // 보안 검증: storage_root 경계를 벗어날 수 없음
        if verify_path_within_root(storage_root, &abs_path).is_ok() && abs_path.is_file() {
            if let Ok(meta) = std::fs::metadata(&abs_path) {
                let size = meta.len();
                if std::fs::remove_file(&abs_path).is_ok() {
                    report.deleted_blobs_count += 1;
                    report.deleted_blobs_bytes += size;
                    report.total_reclaimed_bytes += size;

                    // 빈 부모 디렉터리 정리
                    if let Some(parent) = abs_path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
    }

    // 3. 만료된 임시 파일(.tmp) 삭제
    for tmp_str in &plan.stale_tmp_files {
        let p = PathBuf::from(tmp_str);
        if verify_path_within_root(storage_root, &p).is_ok() && p.is_file() {
            if let Ok(meta) = std::fs::metadata(&p) {
                let size = meta.len();
                if std::fs::remove_file(&p).is_ok() {
                    report.deleted_tmp_count += 1;
                    report.deleted_tmp_bytes += size;
                    report.total_reclaimed_bytes += size;
                }
            }
        }
    }

    // 4. 만료된 고아 블롭(Orphan) 삭제
    for orphan_str in &plan.orphan_blobs_to_delete {
        let p = PathBuf::from(orphan_str);
        if verify_path_within_root(storage_root, &p).is_ok() && p.is_file() {
            if let Ok(meta) = std::fs::metadata(&p) {
                let size = meta.len();
                if std::fs::remove_file(&p).is_ok() {
                    report.deleted_orphans_count += 1;
                    report.total_reclaimed_bytes += size;
                }
            }
        }
    }

    // 5. DB 논리 메타데이터 정리 (트랜잭션 내 일괄 처리)
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Database(format!("트랜잭션 시작 실패: {e}")))?;

    // 5-1. 스냅샷 레코드 삭제
    for chunk in plan.expired_snapshot_ids.chunks(200) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM notepad_snapshots WHERE id IN ({placeholders})");
        let mut query = sqlx::query(&sql);
        for id in chunk {
            query = query.bind(id);
        }
        let res = query
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("스냅샷 DB 삭제 실패: {e}")))?;
        report.deleted_snapshots_count += res.rows_affected() as usize;
    }

    // 5-2. 만료된 삭제 노트 레코드 삭제
    for chunk in plan.expired_note_ids.chunks(200) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM notepad_notes WHERE id IN ({placeholders})");
        let mut query = sqlx::query(&sql);
        for id in chunk {
            query = query.bind(id);
        }
        let res = query
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Database(format!("노트 DB 삭제 실패: {e}")))?;
        report.deleted_notes_count += res.rows_affected() as usize;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Database(format!("트랜잭션 커밋 실패: {e}")))?;

    report.storage_after_bytes = calculate_dir_size(storage_root);
    Ok(report)
}

/// 상세 스토리지 통계를 계산합니다.
pub async fn get_storage_details(
    pool: &SqlitePool,
    storage_root: &Path,
    settings: &NotepadRescueSettings,
) -> Result<StorageDetailDto, AppError> {
    let logical_snapshot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notepad_snapshots")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    let logical_source_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(source_size), 0) FROM notepad_snapshots")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    let physical_disk_bytes = calculate_dir_size(storage_root);

    let snapshots_dir = storage_root.join("snapshots");
    let physical_blob_count = if snapshots_dir.is_dir() {
        walkdir_simple(&snapshots_dir)
            .into_iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bin"))
            .count() as i64
    } else {
        0
    };

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let plan = plan_gc(pool, storage_root, settings, now_epoch).await?;

    let deduplicated_bytes = if (logical_source_bytes as u64) > physical_disk_bytes {
        (logical_source_bytes as u64) - physical_disk_bytes
    } else {
        0
    };

    let quota_percentage = if settings.max_storage_bytes > 0 {
        ((physical_disk_bytes as f64) / (settings.max_storage_bytes as f64) * 100.0).min(999.0)
    } else {
        0.0
    };

    Ok(StorageDetailDto {
        logical_snapshot_count,
        physical_blob_count,
        logical_source_bytes: logical_source_bytes.max(0) as u64,
        physical_disk_bytes,
        deduplicated_bytes,
        reclaimable_bytes: plan.total_reclaimable_bytes,
        orphan_blob_count: plan.orphan_blobs_to_delete.len() as i64,
        tmp_bytes: plan.stale_tmp_bytes,
        max_storage_bytes: settings.max_storage_bytes,
        quota_percentage,
        has_quota_pressure: plan.has_quota_pressure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notepad_rescue::models::{
        NotepadNote, NotepadRescueSettings, NotepadSession, NotepadSnapshot, ParseStatus, SourceType,
    };
    use crate::notepad_rescue::repository::{
        insert_session_snapshot, insert_snapshot_metadata, upsert_note,
    };
    use sqlx::sqlite::SqlitePoolOptions;
    use std::fs;

    async fn create_test_db() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("인메모리 DB 연결 실패");

        sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&pool)
            .await
            .expect("외래 키 활성화 실패");

        for migration in crate::migrations::migrations() {
            sqlx::raw_sql(migration.sql)
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("마이그레이션 실패: {e}"));
        }

        pool
    }

    fn create_test_blob(root: &Path, hash: &str, content: &[u8]) -> PathBuf {
        let rel = super::super::snapshot::relative_blob_path(hash);
        let abs = resolve_blob_path(root, &rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&abs, content).unwrap();
        abs
    }

    #[test]
    fn test_49_50_path_traversal_and_storage_boundary() {
        let temp_dir = std::env::temp_dir().join("nr4-test-boundary");
        let _ = fs::create_dir_all(&temp_dir);

        // 정당한 하위 경로 -> 통과
        let valid_child = temp_dir.join("snapshots").join("12").join("file.bin");
        assert!(verify_path_within_root(&temp_dir, &valid_child).is_ok());

        // 경로 탈출 시도 (..) -> 차단
        let malicious_path = temp_dir.join("..").join("secret.txt");
        assert!(verify_path_within_root(&temp_dir, &malicious_path).is_err());

        // 시스템 루트 직접 접근 시도 -> 차단
        let abs_escape = PathBuf::from("C:\\Windows\\System32\\calc.exe");
        assert!(verify_path_within_root(&temp_dir, &abs_escape).is_err());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_27_28_29_active_note_retention_and_latest_guarantee() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-27-29");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            let note = NotepadNote {
                id: "note_active".to_string(),
                source_id: "src_active".to_string(),
                display_title: "Active Note".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false,
                first_seen_at: "2026-08-01T00:00:00Z".to_string(),
                last_seen_at: "2026-08-01T00:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-08-01T00:00:00Z".to_string(),
                updated_at: "2026-08-01T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            // 100일 전 스냅샷 1 (최신 버전이 아님, 삭제 대상 후보)
            let snap_old = NotepadSnapshot {
                id: "snap_old".to_string(),
                note_id: "note_active".to_string(),
                content_hash: "hash_old_111111111111111111111111111111111111111111111111111111111111".to_string(),
                raw_snapshot_path: "snapshots/ha/hash_old.bin".to_string(),
                recovered_text: Some("Old Content".to_string()),
                source_type: SourceType::TabState,
                source_filename: "src.bin".to_string(),
                source_size: 11,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-05-01T00:00:00Z".to_string(),
                created_at: "2026-05-01T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap_old).await.unwrap();
            create_test_blob(&temp_dir, &snap_old.content_hash, b"Old Content");

            // 50일 전 스냅샷 2 (최신 버전 -> 30일 초과했음에도 최소 복원 보장으로 절대 삭제 불가)
            let snap_latest = NotepadSnapshot {
                id: "snap_latest".to_string(),
                note_id: "note_active".to_string(),
                content_hash: "hash_latest_2222222222222222222222222222222222222222222222222222222222".to_string(),
                raw_snapshot_path: "snapshots/ha/hash_latest.bin".to_string(),
                recovered_text: Some("Latest Content".to_string()),
                source_type: SourceType::TabState,
                source_filename: "src.bin".to_string(),
                source_size: 14,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-08-01T00:00:00Z".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap_latest).await.unwrap();
            create_test_blob(&temp_dir, &snap_latest.content_hash, b"Latest Content");

            let settings = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 1_000_000_000,
                ..Default::default()
            };

            // 기준 시간: 2026-10-01
            let now_epoch = parse_timestamp_epoch("2026-10-01T00:00:00Z");

            // 46. Dry-run 검증: 변경사항 전혀 발생하지 않아야 함
            let plan = plan_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert_eq!(plan.expired_snapshot_ids.len(), 1);
            assert_eq!(plan.expired_snapshot_ids[0], "snap_old");
            // 최신 스냅샷은 절대 만료 후보에 포함되지 않아야 함 (Invariant 29)
            assert!(!plan.expired_snapshot_ids.contains(&"snap_latest".to_string()));

            // GC 실행
            let report = execute_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert_eq!(report.deleted_snapshots_count, 1);
            assert_eq!(report.deleted_blobs_count, 1);

            // DB 확인: snap_old는 삭제되었고 snap_latest는 안전 보존됨
            let remaining_snaps = sqlx::query("SELECT id FROM notepad_snapshots")
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(remaining_snaps.len(), 1);
            assert_eq!(remaining_snaps[0].try_get::<String, _>("id").unwrap(), "snap_latest");

            // 디스크 확인: snap_latest 블롭 파일은 물리적으로 그대로 존재해야 함
            let rel_latest = super::super::snapshot::relative_blob_path(&snap_latest.content_hash);
            let abs_latest = resolve_blob_path(&temp_dir, &rel_latest);
            assert!(abs_latest.exists());

            // 48. Idempotency 검증: 재실행 시 0개 삭제
            let report_2 = execute_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert_eq!(report_2.deleted_snapshots_count, 0);
            assert_eq!(report_2.deleted_blobs_count, 0);

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_30_31_deleted_note_retention() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-30-31");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            // 삭제된 지 10일 된 노트 (보관 기간 30일 이내 -> 보존)
            let note_recent = NotepadNote {
                id: "note_del_recent".to_string(),
                source_id: "src_del_recent".to_string(),
                display_title: "Recent Deleted Note".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: true,
                first_seen_at: "2026-09-01T00:00:00Z".to_string(),
                last_seen_at: "2026-09-20T00:00:00Z".to_string(),
                deleted_at: Some("2026-09-20T00:00:00Z".to_string()),
                created_at: "2026-09-01T00:00:00Z".to_string(),
                updated_at: "2026-09-20T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note_recent).await.unwrap();

            // 삭제된 지 40일 된 노트 (보관 기간 30일 초과 -> 제거)
            let note_old = NotepadNote {
                id: "note_del_old".to_string(),
                source_id: "src_del_old".to_string(),
                display_title: "Old Deleted Note".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: true,
                first_seen_at: "2026-08-01T00:00:00Z".to_string(),
                last_seen_at: "2026-08-20T00:00:00Z".to_string(),
                deleted_at: Some("2026-08-20T00:00:00Z".to_string()),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                updated_at: "2026-08-20T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note_old).await.unwrap();

            let snap_old = NotepadSnapshot {
                id: "snap_del_old".to_string(),
                note_id: "note_del_old".to_string(),
                content_hash: "hash_del_old_33333333333333333333333333333333333333333333333333333333".to_string(),
                raw_snapshot_path: "snapshots/33/del_old.bin".to_string(),
                recovered_text: Some("Del Old".to_string()),
                source_type: SourceType::TabState,
                source_filename: "del.bin".to_string(),
                source_size: 7,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-08-20T00:00:00Z".to_string(),
                created_at: "2026-08-20T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap_old).await.unwrap();
            create_test_blob(&temp_dir, &snap_old.content_hash, b"Del Old");

            let settings = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 1_000_000_000,
                ..Default::default()
            };

            let now_epoch = parse_timestamp_epoch("2026-10-01T00:00:00Z");
            let plan = plan_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();

            // note_del_old는 제거 대상, note_del_recent는 보존
            assert!(plan.expired_note_ids.contains(&"note_del_old".to_string()));
            assert!(!plan.expired_note_ids.contains(&"note_del_recent".to_string()));

            execute_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();

            // note_del_recent는 DB에 그대로 남아 있어야 함
            let notes = sqlx::query("SELECT id FROM notepad_notes WHERE id = 'note_del_recent'")
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(notes.len(), 1);

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_32_34_35_shared_blob_multi_snapshot_reference_safety() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-32-35");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            let shared_hash = "shared_hash_444444444444444444444444444444444444444444444444444444444".to_string();
            let blob_path = create_test_blob(&temp_dir, &shared_hash, b"Shared Content");

            let note1 = NotepadNote {
                id: "note1".to_string(),
                source_id: "src1".to_string(),
                display_title: "Note 1".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: true,
                first_seen_at: "2026-08-01T00:00:00Z".to_string(),
                last_seen_at: "2026-08-01T00:00:00Z".to_string(),
                deleted_at: Some("2026-08-01T00:00:00Z".to_string()), // 만료 대상
                created_at: "2026-08-01T00:00:00Z".to_string(),
                updated_at: "2026-08-01T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note1).await.unwrap();

            let note2 = NotepadNote {
                id: "note2".to_string(),
                source_id: "src2".to_string(),
                display_title: "Note 2".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: false, // 활성 상태
                first_seen_at: "2026-09-25T00:00:00Z".to_string(),
                last_seen_at: "2026-09-25T00:00:00Z".to_string(),
                deleted_at: None,
                created_at: "2026-09-25T00:00:00Z".to_string(),
                updated_at: "2026-09-25T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note2).await.unwrap();

            // 스냅샷 1: note1에 속하며 만료 대상
            let snap1 = NotepadSnapshot {
                id: "snap1".to_string(),
                note_id: "note1".to_string(),
                content_hash: shared_hash.clone(),
                raw_snapshot_path: "p1".to_string(),
                recovered_text: Some("Shared".to_string()),
                source_type: SourceType::TabState,
                source_filename: "f1".to_string(),
                source_size: 14,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-08-01T00:00:00Z".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap1).await.unwrap();

            // 스냅샷 2: note2에 속하며 활성 보존 대상
            let snap2 = NotepadSnapshot {
                id: "snap2".to_string(),
                note_id: "note2".to_string(),
                content_hash: shared_hash.clone(),
                raw_snapshot_path: "p2".to_string(),
                recovered_text: Some("Shared".to_string()),
                source_type: SourceType::TabState,
                source_filename: "f2".to_string(),
                source_size: 14,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-09-25T00:00:00Z".to_string(),
                created_at: "2026-09-25T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap2).await.unwrap();

            let settings = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 1_000_000_000,
                ..Default::default()
            };
            let now_epoch = parse_timestamp_epoch("2026-10-01T00:00:00Z");

            // GC 계획: snap1은 만료 대상이지만 shared_hash는 snap2가 참조 중이므로 블롭 삭제 목록에 포함되지 않아야 함!
            let plan = plan_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert_eq!(plan.expired_snapshot_ids, vec!["snap1".to_string()]);
            assert!(plan.unreferenced_blob_hashes.is_empty(), "다른 스냅샷이 참조 중인 공유 블롭은 삭제 목록에 포함되면 안 됨");

            execute_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();

            // 물리 파일 여전히 보존 확인
            assert!(blob_path.exists(), "공유 블롭은 물리적으로 보존되어야 함");

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_33_shared_blob_snapshot_and_session_reference_safety() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-33");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            let session_hash = "session_hash_5555555555555555555555555555555555555555555555555555555".to_string();
            let blob_path = create_test_blob(&temp_dir, &session_hash, b"Session Data");

            // 세션(WindowState)에 동일 해시 등록
            let session = NotepadSession {
                id: "sess1".to_string(),
                source_filename: "window.bin".to_string(),
                content_hash: session_hash.clone(),
                raw_snapshot_path: "p".to_string(),
                captured_at: "2026-09-01T00:00:00Z".to_string(),
                parse_status: ParseStatus::Parsed,
                created_at: "2026-09-01T00:00:00Z".to_string(),
            };
            insert_session_snapshot(&pool, &session).await.unwrap();

            let note = NotepadNote {
                id: "note_del".to_string(),
                source_id: "src_del".to_string(),
                display_title: "Note".to_string(),
                original_path: None,
                is_saved: false,
                is_deleted: true,
                first_seen_at: "2026-08-01T00:00:00Z".to_string(),
                last_seen_at: "2026-08-01T00:00:00Z".to_string(),
                deleted_at: Some("2026-08-01T00:00:00Z".to_string()),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                updated_at: "2026-08-01T00:00:00Z".to_string(),
            };
            upsert_note(&pool, &note).await.unwrap();

            let snap = NotepadSnapshot {
                id: "snap_del".to_string(),
                note_id: "note_del".to_string(),
                content_hash: session_hash.clone(),
                raw_snapshot_path: "p".to_string(),
                recovered_text: Some("Text".to_string()),
                source_type: SourceType::TabState,
                source_filename: "f".to_string(),
                source_size: 10,
                parse_status: ParseStatus::Parsed,
                notepad_format_version: Some(1),
                captured_at: "2026-08-01T00:00:00Z".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
            };
            insert_snapshot_metadata(&pool, &snap).await.unwrap();

            let settings = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 1_000_000_000,
                ..Default::default()
            };
            let now_epoch = parse_timestamp_epoch("2026-10-01T00:00:00Z");

            // 스냅샷은 만료되지만 세션이 동일 해시를 참조하므로 블롭은 보존되어야 함!
            let plan = plan_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert_eq!(plan.expired_snapshot_ids, vec!["snap_del".to_string()]);
            assert!(plan.unreferenced_blob_hashes.is_empty());

            execute_gc(&pool, &temp_dir, &settings, now_epoch).await.unwrap();
            assert!(blob_path.exists(), "세션이 참조 중인 CAS 블롭은 삭제되면 안 됨");

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_41_42_tmp_cleanup_grace_period() {
        let temp_dir = std::env::temp_dir().join("nr4-test-41-42");
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::create_dir_all(&temp_dir);

        let fresh_tmp = temp_dir.join("fresh.tmp");
        fs::write(&fresh_tmp, b"Fresh Temporary File").unwrap();

        // 1시간 유예 기간 적용 시 방금 생성된 fresh_tmp는 삭제 대상에 포함되지 않아야 함
        let (tmps, _) = scan_stale_tmp_files(&temp_dir, Duration::from_secs(3600));
        assert!(tmps.is_empty(), "신선한 .tmp 파일은 보존되어야 함");

        // 유예 기간 0초로 설정 시 삭제 대상에 포함
        let (tmps_zero, bytes) = scan_stale_tmp_files(&temp_dir, Duration::from_secs(0));
        assert_eq!(tmps_zero.len(), 1);
        assert!(bytes > 0);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_39_40_orphan_blob_grace_period() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-39-40");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            let orphan_hash = "6666666666666666666666666666666666666666666666666666666666666666";
            create_test_blob(&temp_dir, orphan_hash, b"Orphan Data");

            // 24시간 유예 기간 적용 시 방금 생성된 고아 블롭은 삭제 대상에 포함되지 않아야 함 (Invariant 39)
            let (orphans, _) = scan_orphan_blobs(&pool, &temp_dir, Duration::from_secs(86400)).await.unwrap();
            assert!(orphans.is_empty(), "24시간 이내의 고아 블롭은 유예 기간 동안 보존되어야 함");

            // 유예 기간 경과(0초) 후에는 삭제 대상에 포함 (Invariant 40)
            let (orphans_expired, bytes) = scan_orphan_blobs(&pool, &temp_dir, Duration::from_secs(0)).await.unwrap();
            assert_eq!(orphans_expired.len(), 1);
            assert!(bytes > 0);

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }

    #[test]
    fn test_36_37_38_quota_pressure_evaluation() {
        tauri::async_runtime::block_on(async {
            let pool = create_test_db().await;
            let temp_dir = std::env::temp_dir().join("nr4-test-quota");
            let _ = fs::remove_dir_all(&temp_dir);
            let _ = fs::create_dir_all(&temp_dir);

            // 1KB 크기 파일 생성
            let hash = "7777777777777777777777777777777777777777777777777777777777777777";
            create_test_blob(&temp_dir, hash, &vec![0u8; 2048]);

            // 쿼터 1KB 설정 -> 현재 사용량 2KB로 쿼터 초과
            let settings_pressure = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 1024, // 1 KB
                ..Default::default()
            };

            let now_epoch = parse_timestamp_epoch("2026-10-01T00:00:00Z");
            let plan = plan_gc(&pool, &temp_dir, &settings_pressure, now_epoch).await.unwrap();
            assert!(plan.has_quota_pressure, "쿼터를 초과하고 안전 정리가 불가능한 경우 QUOTA_PRESSURE가 보고되어야 함");

            // 쿼터 10MB 설정 -> 쿼터 여유
            let settings_ok = NotepadRescueSettings {
                retention_days: 30,
                deleted_retention_days: 30,
                max_storage_bytes: 10_000_000,
                ..Default::default()
            };
            let plan_ok = plan_gc(&pool, &temp_dir, &settings_ok, now_epoch).await.unwrap();
            assert!(!plan_ok.has_quota_pressure);

            let _ = fs::remove_dir_all(&temp_dir);
        });
    }
}
