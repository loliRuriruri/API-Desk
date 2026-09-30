//! 상주 런타임: 창/프런트엔드와 무관하게 백엔드가 소유하는 수집·스케줄러.
//!
//! - 시작 로그(링 버퍼 + `resident.log`, 시크릿 없음)
//! - 사용량 스케줄러: 백엔드 상주 주기 갱신 (모니터 5종 + 자격 증명 사용량)
//! - 상주 상태 조회(GPU/TURZX/사용량/로컬 서비스) — UI 진단용

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppError;

pub const LOG_LIMIT: usize = 400;
pub const STALE_USAGE_MS: i64 = 5 * 60 * 1000;      // 5분
#[allow(dead_code)]
pub const STALE_MONITOR_MS: i64 = 5 * 60 * 1000;    // 5분
pub const USAGE_TICK_SECS: u64 = 60;                // 60초

pub const MONITOR_PROVIDERS: [&str; 5] = [
    "codex",
    "grok",
    "grok-build",
    "opencode",
    "antigravity",
];

pub fn stale_threshold_ms() -> i64 {
    std::env::var("API_DESK_USAGE_STALE_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(STALE_USAGE_MS)
}

pub fn tick_interval_secs() -> u64 {
    std::env::var("API_DESK_USAGE_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(USAGE_TICK_SECS)
}

pub struct ResidentState {
    pub log: Mutex<VecDeque<String>>,
    pub usage: Mutex<UsageRuntime>,
    pub active_refreshes: Mutex<HashSet<String>>,
    pub shutdown: Arc<AtomicBool>,
}

impl Default for ResidentState {
    fn default() -> Self {
        Self {
            log: Mutex::new(VecDeque::new()),
            usage: Mutex::new(UsageRuntime::default()),
            active_refreshes: Mutex::new(HashSet::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone)]
pub struct UsageRuntime {
    pub state: String,
    pub ready: bool,
    pub last_refresh_at: Option<String>,
    pub last_error: Option<String>,
    pub updated: u64,
}

impl Default for UsageRuntime {
    fn default() -> Self {
        Self {
            state: "WAITING_FOR_VAULT".into(),
            ready: false,
            last_refresh_at: None,
            last_error: None,
            updated: 0,
        }
    }
}

pub struct RefreshGuard {
    app: AppHandle,
    key: String,
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<ResidentState>() {
            if let Ok(mut set) = state.active_refreshes.lock() {
                set.remove(&self.key);
            }
        }
    }
}

pub fn try_acquire_refresh(app: &AppHandle, key: &str) -> Option<RefreshGuard> {
    let state = app.try_state::<ResidentState>()?;
    let mut set = state.active_refreshes.lock().ok()?;
    if set.contains(key) {
        return None;
    }
    set.insert(key.to_string());
    Some(RefreshGuard {
        app: app.clone(),
        key: key.to_string(),
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_whitespace() {
        let lower = token.to_lowercase();
        if lower.starts_with("hf_")
            || lower.starts_with("sk-")
            || lower.starts_with("bearer")
            || lower.starts_with("authorization:")
        {
            out.push_str("[redacted]");
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().chars().take(300).collect()
}

/// 백엔드 시작 이벤트(창/클릭과 무관). 시크릿 금지.
pub fn log(app: &AppHandle, tag: &str, message: &str) {
    let line = format!(
        "[{}] [{}] {}",
        crate::usage::epoch_to_rfc3339(now_secs()),
        tag,
        sanitize(message)
    );
    if let Some(state) = app.try_state::<ResidentState>() {
        if let Ok(mut buffer) = state.log.lock() {
            if buffer.len() >= LOG_LIMIT {
                buffer.pop_front();
            }
            buffer.push_back(line.clone());
        }
    }
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("resident.log"))
        {
            use std::io::Write;
            let _ = writeln!(file, "{line}");
        }
    }
    let _ = app.emit("resident-log", line);
}

/// 백엔드 세션 시작 경계 로그(스펙 §13: session start pid=... build=...).
pub fn log_session_start(app: &AppHandle) {
    let pid = std::process::id();
    let build = env!("CARGO_PKG_VERSION");
    log(app, "resident", &format!("session start pid={pid} build={build}"));
}

// ---------------------------------------------------------------- 사용량 스케줄러

fn db_path(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| AppError::Io(error.to_string()))?;
    Ok(dir.join("apidb.db"))
}

/// fetched_at(ms)이 max_age를 넘었는지(파싱 실패 = 오래된 것으로 간주).
pub fn is_stale(fetched_ms: Option<i64>, now_ms: i64, max_age_ms: i64) -> bool {
    match fetched_ms {
        Some(at) => now_ms - at >= max_age_ms,
        None => true,
    }
}

pub async fn is_monitor_stale_in_db(pool: &SqlitePool, monitor: &str, max_age_ms: i64) -> bool {
    let row = sqlx::query("SELECT fetched_at FROM monitor_snapshots WHERE monitor = ?1")
        .bind(monitor)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    match row {
        Some(row) => {
            let fetched_at: String = row.try_get("fetched_at").unwrap_or_default();
            let now_ms = now_secs() * 1000;
            let parse_ms = |text: &str| crate::monitors::parse_rfc3339_secs(text).map(|s| s * 1000);
            match parse_ms(&fetched_at) {
                Some(at) => now_ms - at >= max_age_ms,
                None => true,
            }
        }
        None => true,
    }
}

pub async fn persist_monitor_outcome(
    pool: &SqlitePool,
    outcome: &crate::monitors::MonitorOutcome,
) -> Result<(), AppError> {
    let now = crate::usage::epoch_to_rfc3339(now_secs());
    let summary = if outcome.summary.is_empty() {
        outcome.message.clone()
    } else {
        Some(outcome.summary.clone())
    };
    let details_json = outcome.details.as_ref().map(|v| v.to_string());

    sqlx::query(
        "INSERT INTO monitor_snapshots (id, monitor, status, summary, details_json, fetched_at)
         VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(monitor) DO UPDATE SET
            status = excluded.status,
            summary = excluded.summary,
            details_json = excluded.details_json,
            fetched_at = excluded.fetched_at",
    )
    .bind(&outcome.monitor)
    .bind(&outcome.status)
    .bind(&summary)
    .bind(&details_json)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| AppError::Io(format!("모니터 스냅샷 저장 실패: {e}")))?;

    if outcome.monitor == "antigravity" {
        let email = outcome
            .details
            .as_ref()
            .and_then(|d| d.get("email"))
            .and_then(|e| e.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        if let Some(email) = email {
            let account_key = format!("antigravity:{email}");
            sqlx::query(
                "INSERT INTO monitor_snapshots (id, monitor, status, summary, details_json, fetched_at)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(monitor) DO UPDATE SET
                    status = excluded.status,
                    summary = excluded.summary,
                    details_json = excluded.details_json,
                    fetched_at = excluded.fetched_at",
            )
            .bind(&account_key)
            .bind(&outcome.status)
            .bind(&summary)
            .bind(&details_json)
            .bind(&now)
            .execute(pool)
            .await
            .map_err(|e| AppError::Io(format!("antigravity 계정 스냅샷 저장 실패: {e}")))?;
        }
    }
    Ok(())
}

pub async fn refresh_single_monitor(
    app: &AppHandle,
    pool: &SqlitePool,
    monitor: &str,
    force: bool,
    max_age_ms: i64,
) -> Result<bool, AppError> {
    if !force && !is_monitor_stale_in_db(pool, monitor, max_age_ms).await {
        return Ok(false);
    }

    let key = format!("monitor:{monitor}");
    let _guard = match try_acquire_refresh(app, &key) {
        Some(guard) => guard,
        None => return Ok(false),
    };

    log(app, "usage", &format!("{monitor} stale -> refresh"));

    match crate::monitors::monitor_refresh(monitor.to_string()).await {
        Ok(outcome) => {
            persist_monitor_outcome(pool, &outcome).await?;
            if outcome.status == "ok" {
                log(app, "usage", &format!("{monitor} updated"));
            } else {
                log(app, "usage", &format!("{monitor} {}", outcome.status));
            }
            Ok(true)
        }
        Err(error) => {
            if monitor == "antigravity" {
                log(app, "usage", "antigravity unavailable");
                // 기존 계정 스냅샷 및 details 보존: 전역 antigravity 행의 status만 unavailable 처리
                let _ = sqlx::query(
                    "UPDATE monitor_snapshots SET status = 'unavailable' WHERE monitor = 'antigravity'",
                )
                .execute(pool)
                .await;
            } else {
                log(app, "usage", &format!("{monitor} unavailable"));
            }
            Err(error)
        }
    }
}

pub async fn refresh_stale_monitors(
    app: &AppHandle,
    pool: &SqlitePool,
    force: bool,
    max_age_ms: i64,
) -> (u64, bool) {
    let mut updated = 0u64;
    let mut had_hard_error = false;
    for monitor in MONITOR_PROVIDERS {
        match refresh_single_monitor(app, pool, monitor, force, max_age_ms).await {
            Ok(true) => updated += 1,
            Ok(false) => {}
            Err(_) => {
                if monitor != "antigravity" {
                    had_hard_error = true;
                }
            }
        }
    }
    (updated, had_hard_error)
}

async fn refresh_stale_usage_with_pool(
    app: &AppHandle,
    pool: &SqlitePool,
    force: bool,
    max_age_ms: i64,
) -> Result<u64, AppError> {
    let vault = app.state::<crate::vault::VaultState>();
    let now_ms = now_secs() * 1000;

    let rows = sqlx::query(
        "SELECT u.id AS id, u.credential_id AS credential_id, u.adapter AS adapter, u.fetched_at AS fetched_at,
                c.secret_id AS secret_id, p.base_url AS base_url
         FROM usage_snapshots u
         JOIN credentials c ON c.id = u.credential_id
         LEFT JOIN accounts a ON a.id = c.account_id
         LEFT JOIN providers p ON p.id = a.provider_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Io(format!("사용량 조회 실패: {error}")))?;

    let fields = sqlx::query(
        "SELECT credential_id, secret_id, label, env_name FROM credential_fields",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let parse_ms = |text: &str| -> Option<i64> {
        crate::monitors::parse_rfc3339_secs(text).map(|secs| secs * 1000)
    };

    let mut writes: Vec<(String, String, crate::usage::UsageOutcome)> = Vec::new();
    for row in &rows {
        let adapter: String = row.get("adapter");
        let fetched_at: String = row.get("fetched_at");
        let credential_id: String = row.try_get("credential_id").unwrap_or_default();

        if !force && !is_stale(parse_ms(&fetched_at), now_ms, max_age_ms) {
            continue;
        }

        let key = format!("usage:{credential_id}");
        let _guard = match try_acquire_refresh(app, &key) {
            Some(guard) => guard,
            None => continue,
        };

        let secret_id: String = row.get("secret_id");
        let base_url: Option<String> = row.try_get("base_url").ok().flatten();

        let extra_secret_id: Option<String> = if adapter == "xai" {
            fields
                .iter()
                .filter(|field| {
                    let owner: String = field.try_get("credential_id").unwrap_or_default();
                    owner == credential_id
                })
                .find(|field| {
                    let label: String = field.try_get("label").unwrap_or_default();
                    let env: Option<String> = field.try_get("env_name").ok().flatten();
                    format!("{label} {}", env.unwrap_or_default())
                        .to_lowercase()
                        .contains("team")
                })
                .map(|field| field.get("secret_id"))
        } else {
            None
        };

        match crate::usage::fetch_usage_with(
            &vault,
            &adapter,
            base_url.as_deref(),
            &secret_id,
            extra_secret_id.as_deref(),
            None,
        )
        .await
        {
            Ok(outcome) => {
                writes.push((credential_id, adapter, outcome));
            }
            Err(error) => {
                log(
                    app,
                    "usage",
                    &format!("{adapter} 조회 실패: {}", sanitize(&error.to_string())),
                );
            }
        }
    }

    let mut written = 0u64;
    if !writes.is_empty() {
        let stamp = crate::usage::epoch_to_rfc3339(now_secs());
        for (credential_id, adapter, outcome) in writes {
            let details = outcome.details.as_ref().map(|value| value.to_string());
            sqlx::query(
                "UPDATE usage_snapshots SET adapter = ?1, status = ?2, summary = ?3,
                 used = ?4, limit_total = ?5, remaining = ?6, currency = ?7,
                 details_json = ?8, fetched_at = ?9 WHERE credential_id = ?10",
            )
            .bind(&adapter)
            .bind(&outcome.status)
            .bind(if outcome.summary.is_empty() {
                outcome.message.clone()
            } else {
                Some(outcome.summary.clone())
            })
            .bind(outcome.used)
            .bind(outcome.limit)
            .bind(outcome.remaining)
            .bind(&outcome.currency)
            .bind(details)
            .bind(&stamp)
            .bind(&credential_id)
            .execute(pool)
            .await
            .map_err(|error| AppError::Io(format!("사용량 저장 실패: {error}")))?;
            written += 1;
        }
    }
    Ok(written)
}

pub async fn refresh_all_stale(
    app: &AppHandle,
    force: bool,
) -> Result<u64, AppError> {
    let path = db_path(app)?;
    let pool = match SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new().filename(&path).read_only(false),
        )
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            log(app, "usage", &format!("DB 연결 실패: {e}"));
            return Err(AppError::Io(format!("DB 연결 실패: {e}")));
        }
    };

    let max_age_ms = stale_threshold_ms();
    let (monitor_updated, monitor_error) = refresh_stale_monitors(app, &pool, force, max_age_ms).await;

    let vault_unlocked = app
        .try_state::<crate::vault::VaultState>()
        .map(|s| s.is_unlocked())
        .unwrap_or(false);

    let (usage_updated, usage_error) = if vault_unlocked {
        match refresh_stale_usage_with_pool(app, &pool, force, max_age_ms).await {
            Ok(count) => (count, false),
            Err(e) => {
                log(app, "usage", &format!("사용량 갱신 오류: {}", sanitize(&e.to_string())));
                (0, true)
            }
        }
    } else {
        (0, false)
    };

    let total_updated = monitor_updated + usage_updated;
    let new_state = if monitor_error || usage_error {
        "DEGRADED"
    } else if !vault_unlocked {
        "WAITING_FOR_VAULT"
    } else {
        "READY"
    };

    if let Some(state) = app.try_state::<ResidentState>() {
        if let Ok(mut usage) = state.usage.lock() {
            usage.state = new_state.to_string();
            usage.ready = true;
            if total_updated > 0 || usage.last_refresh_at.is_none() {
                usage.last_refresh_at = Some(crate::usage::epoch_to_rfc3339(now_secs()));
            }
            usage.updated += total_updated;
            if monitor_error || usage_error {
                usage.last_error = Some("일부 프로바이더 갱신 실패".into());
            } else {
                usage.last_error = None;
            }
        }
    }

    if total_updated > 0 {
        crate::turzx::invalidate_api_rows_cache();
        let _ = app.emit("usage-snapshot-updated", ());
        let _ = app.emit("usage-refresh", ());
        log(app, "usage", &format!("{total_updated}건 갱신"));
    }

    Ok(total_updated)
}

/// 백그라운드 사용량 상주 스케줄러: UI 마운트 여부와 무관하게 부팅 시 가동된다.
pub fn start_usage_scheduler(app: &AppHandle) {
    let handle = app.clone();
    let shutdown = handle
        .try_state::<ResidentState>()
        .map(|s| s.shutdown.clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    std::thread::spawn(move || {
        log(&handle, "usage", "scheduler started");

        while !shutdown.load(Ordering::SeqCst) {
            match tauri::async_runtime::block_on(refresh_all_stale(&handle, false)) {
                Ok(_) => {}
                Err(error) => {
                    log(&handle, "usage", &format!("refresh error: {error}"));
                }
            }

            let interval_secs = tick_interval_secs();
            log(&handle, "usage", &format!("next check in {interval_secs}s"));

            let interval_ms = interval_secs * 1000;
            let mut waited_ms = 0;
            while waited_ms < interval_ms && !shutdown.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                waited_ms += 250;
            }
        }

        log(&handle, "usage", "scheduler stopped");
    });
}

pub fn stop_usage_scheduler(app: &AppHandle) {
    if let Some(state) = app.try_state::<ResidentState>() {
        state.shutdown.store(true, Ordering::SeqCst);
    }
}

#[tauri::command]
pub async fn resident_refresh_usage(
    app: AppHandle,
    force: Option<bool>,
) -> Result<u64, AppError> {
    refresh_all_stale(&app, force.unwrap_or(false)).await
}

// ---------------------------------------------------------------- 상태 조회

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidentStatus {
    pub log: Vec<String>,
    pub gpu: Value,
    pub turzx: Value,
    pub usage: Value,
    pub services: Vec<Value>,
}

#[tauri::command]
pub fn resident_status(app: AppHandle) -> ResidentStatus {
    let log = app
        .try_state::<ResidentState>()
        .and_then(|state| state.log.lock().ok().map(|buffer| buffer.iter().cloned().collect::<Vec<_>>()))
        .unwrap_or_default();
    let log: Vec<String> = log.iter().rev().take(60).rev().cloned().collect();
    let usage = app
        .try_state::<ResidentState>()
        .and_then(|state| state.usage.lock().ok().map(|usage| {
            json!({
                "state": usage.state,
                "ready": usage.ready,
                "lastRefreshAt": usage.last_refresh_at,
                "lastError": usage.last_error,
                "updated": usage.updated,
            })
        }))
        .unwrap_or_else(|| json!({ "state": "WAITING_FOR_VAULT", "ready": false }));
    let gpu = serde_json::to_value(crate::gpu_monitor::runtime_status(&app)).unwrap_or(Value::Null);
    let turzx = app
        .try_state::<crate::turzx::TurzxState>()
        .map(|_| serde_json::to_value(crate::turzx::turzx_status(app.clone())).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    let services = crate::local_services::definitions()
        .into_iter()
        .map(|definition| {
            let (running, pid) = crate::local_services::service_runtime(&app, &definition.id);
            json!({
                "id": definition.id,
                "label": definition.label,
                "running": running,
                "pid": pid,
            })
        })
        .collect();
    ResidentStatus {
        log,
        gpu,
        turzx,
        usage,
        services,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_rule_matches_five_minutes() {
        let now = 10_000_000_000i64;
        let five_min = STALE_MONITOR_MS;
        assert!(!is_stale(Some(now - 60_000), now, five_min), "1분 전은 신선");
        assert!(!is_stale(Some(now - five_min + 1), now, five_min), "4분 59초 전은 신선");
        assert!(is_stale(Some(now - five_min), now, five_min), "5분 경과는 stale");
        assert!(is_stale(Some(now - 600_000), now, five_min), "10분 경과는 stale");
        assert!(is_stale(None, now, five_min), "기록 없음은 stale");
    }

    #[test]
    fn staleness_rule_matches_six_hours() {
        let now = 10_000_000_000i64;
        let six_hours = 6 * 60 * 60 * 1000;
        assert!(!is_stale(Some(now - 60_000), now, six_hours));
        assert!(!is_stale(Some(now - six_hours + 1), now, six_hours));
        assert!(is_stale(Some(now - six_hours), now, six_hours));
        assert!(is_stale(None, now, six_hours), "파싱 실패는 갱신 대상");
    }

    #[test]
    fn test_sanitizes_tokens() {
        assert_eq!(sanitize("Bearer sk-abc12345"), "[redacted] [redacted]");
        assert_eq!(sanitize("hf_secretvalue123"), "[redacted]");
        assert_eq!(sanitize("normal text"), "normal text");
    }

    #[test]
    fn test_scheduler_state_lifecycle() {
        let mut runtime = UsageRuntime::default();
        assert_eq!(runtime.state, "WAITING_FOR_VAULT");
        assert!(!runtime.ready);

        runtime.state = "READY".into();
        runtime.ready = true;
        assert_eq!(runtime.state, "READY");
        assert!(runtime.ready);

        runtime.state = "DEGRADED".into();
        assert_eq!(runtime.state, "DEGRADED");
    }

    #[test]
    fn test_shutdown_flag_terminates() {
        let shutdown = Arc::new(AtomicBool::new(false));
        assert!(!shutdown.load(Ordering::SeqCst));
        shutdown.store(true, Ordering::SeqCst);
        assert!(shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn test_concurrency_guard_prevents_duplicate() {
        let state = ResidentState::default();
        let key = "monitor:codex";
        {
            let mut set = state.active_refreshes.lock().unwrap();
            assert!(set.insert(key.to_string()), "first acquisition succeeds");
            assert!(!set.insert(key.to_string()), "duplicate acquisition fails");
            assert!(set.insert("monitor:opencode".to_string()), "different provider succeeds");
        }
        {
            let mut set = state.active_refreshes.lock().unwrap();
            set.remove(key);
        }
        {
            let mut set = state.active_refreshes.lock().unwrap();
            assert!(set.insert(key.to_string()), "re-acquisition succeeds after release");
        }
    }

    #[test]
    fn test_persist_monitor_outcome_and_staleness() {
        tauri::async_runtime::block_on(async {
            let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::query(
                "CREATE TABLE monitor_snapshots (
                    id TEXT PRIMARY KEY,
                    monitor TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL,
                    summary TEXT,
                    details_json TEXT,
                    fetched_at TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();

            assert!(is_monitor_stale_in_db(&pool, "codex", 300_000).await);

            let outcome = crate::monitors::MonitorOutcome {
                monitor: "codex".into(),
                status: "ok".into(),
                summary: "100% left".into(),
                details: Some(json!({"plan": "Pro"})),
                message: None,
            };
            persist_monitor_outcome(&pool, &outcome).await.unwrap();

            assert!(!is_monitor_stale_in_db(&pool, "codex", 300_000).await);

            let row = sqlx::query(
                "SELECT monitor, status, summary, details_json FROM monitor_snapshots WHERE monitor = 'codex'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let monitor: String = row.get("monitor");
            let status: String = row.get("status");
            let summary: Option<String> = row.get("summary");
            assert_eq!(monitor, "codex");
            assert_eq!(status, "ok");
            assert_eq!(summary.as_deref(), Some("100% left"));
        });
    }

    #[test]
    fn test_antigravity_dual_snapshot_persistence() {
        tauri::async_runtime::block_on(async {
            let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::query(
                "CREATE TABLE monitor_snapshots (
                    id TEXT PRIMARY KEY,
                    monitor TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL,
                    summary TEXT,
                    details_json TEXT,
                    fetched_at TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();

            let outcome = crate::monitors::MonitorOutcome {
                monitor: "antigravity".into(),
                status: "ok".into(),
                summary: "Free · normal".into(),
                details: Some(json!({
                    "email": "user@example.com",
                    "tier": "free"
                })),
                message: None,
            };
            persist_monitor_outcome(&pool, &outcome).await.unwrap();

            let global = sqlx::query("SELECT status FROM monitor_snapshots WHERE monitor = 'antigravity'")
                .fetch_optional(&pool)
                .await
                .unwrap();
            assert!(global.is_some());

            let account = sqlx::query(
                "SELECT status FROM monitor_snapshots WHERE monitor = 'antigravity:user@example.com'",
            )
            .fetch_optional(&pool)
            .await
            .unwrap();
            assert!(account.is_some());
        });
    }

    #[test]
    fn test_antigravity_unavailable_preserves_details() {
        tauri::async_runtime::block_on(async {
            let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::query(
                "CREATE TABLE monitor_snapshots (
                    id TEXT PRIMARY KEY,
                    monitor TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL,
                    summary TEXT,
                    details_json TEXT,
                    fetched_at TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();

            let outcome = crate::monitors::MonitorOutcome {
                monitor: "antigravity".into(),
                status: "ok".into(),
                summary: "Pro · normal".into(),
                details: Some(json!({
                    "email": "dev@company.com",
                    "tier": "pro"
                })),
                message: None,
            };
            persist_monitor_outcome(&pool, &outcome).await.unwrap();

            sqlx::query("UPDATE monitor_snapshots SET status = 'unavailable' WHERE monitor = 'antigravity'")
                .execute(&pool)
                .await
                .unwrap();

            let row = sqlx::query(
                "SELECT status, summary, details_json FROM monitor_snapshots WHERE monitor = 'antigravity'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let status: String = row.get("status");
            let summary: Option<String> = row.get("summary");
            let details: Option<String> = row.get("details_json");
            assert_eq!(status, "unavailable");
            assert_eq!(summary.as_deref(), Some("Pro · normal"), "details/summary must be preserved");
            assert!(details.unwrap().contains("dev@company.com"), "email must be preserved");

            let acct_row = sqlx::query(
                "SELECT status FROM monitor_snapshots WHERE monitor = 'antigravity:dev@company.com'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            let acct_status: String = acct_row.get("status");
            assert_eq!(acct_status, "ok", "account snapshot preserved");
        });
    }

    #[test]
    fn test_auth_error_persists_state_cleanly() {
        tauri::async_runtime::block_on(async {
            let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::query(
                "CREATE TABLE monitor_snapshots (
                    id TEXT PRIMARY KEY,
                    monitor TEXT NOT NULL UNIQUE,
                    status TEXT NOT NULL,
                    summary TEXT,
                    details_json TEXT,
                    fetched_at TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();

            let auth_error_outcome = crate::monitors::MonitorOutcome {
                monitor: "grok".into(),
                status: "unavailable".into(),
                summary: String::new(),
                details: None,
                message: Some("Grok 토큰이 만료되었습니다".into()),
            };
            persist_monitor_outcome(&pool, &auth_error_outcome).await.unwrap();

            let row = sqlx::query("SELECT status, summary FROM monitor_snapshots WHERE monitor = 'grok'")
                .fetch_one(&pool)
                .await
                .unwrap();
            let status: String = row.get("status");
            let summary: Option<String> = row.get("summary");
            assert_eq!(status, "unavailable");
            assert_eq!(summary.as_deref(), Some("Grok 토큰이 만료되었습니다"));
        });
    }
}
