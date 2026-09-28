//! 상주 런타임: 창/프런트엔드와 무관하게 백엔드가 소유하는 수집·스케줄러.
//!
//! - 시작 로그(링 버퍼 + `resident.log`, 시크릿 없음)
//! - 사용량 스케줄러: Vault 자동 해제 후 6시간 초과 스냅샷을 주기적으로 갱신
//! - 상주 상태 조회(GPU/TURZX/사용량/로컬 서비스) — UI 진단용
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppError;

const LOG_LIMIT: usize = 400;
const STALE_USAGE_MS: i64 = 6 * 60 * 60 * 1000;
const USAGE_TICK_SECS: u64 = 15 * 60;

#[derive(Default)]
pub struct ResidentState {
    log: Mutex<VecDeque<String>>,
    usage: Mutex<UsageRuntime>,
}

#[derive(Default, Clone)]
struct UsageRuntime {
    ready: bool,
    last_refresh_at: Option<String>,
    last_error: Option<String>,
    updated: u64,
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

// ---------------------------------------------------------------- 사용량 스케줄러

/// Vault 자동 해제를 기다린 뒤 6시간 초과 사용량 스냅샷을 주기적으로 갱신한다.
pub fn start_usage_scheduler(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut reported_lock = false;
        loop {
            let unlocked = handle
                .try_state::<crate::vault::VaultState>()
                .map(|state| state.is_unlocked())
                .unwrap_or(false);
            if unlocked {
                break;
            }
            if !reported_lock {
                log(&handle, "usage", "vault 잠금 상태 — 자동 해제 후 갱신 시작");
                reported_lock = true;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        {
            if let Some(state) = handle.try_state::<ResidentState>() {
                if let Ok(mut usage) = state.usage.lock() {
                    usage.ready = true;
                }
            }
        }
        log(&handle, "usage", "스케줄러 시작 (6시간 초과 스냅샷 자동 갱신)");
        loop {
            match refresh_stale_usage(&handle) {
                Ok(updated) => {
                    if let Some(state) = handle.try_state::<ResidentState>() {
                        if let Ok(mut usage) = state.usage.lock() {
                            usage.last_refresh_at =
                                Some(crate::usage::epoch_to_rfc3339(now_secs()));
                            usage.last_error = None;
                            usage.updated = updated;
                        }
                    }
                    if updated > 0 {
                        log(&handle, "usage", &format!("{updated}건 갱신"));
                        let _ = handle.emit("usage-refresh", ());
                    }
                }
                Err(error) => {
                    let message = sanitize(&error.to_string());
                    if let Some(state) = handle.try_state::<ResidentState>() {
                        if let Ok(mut usage) = state.usage.lock() {
                            usage.last_error = Some(message.clone());
                        }
                    }
                    log(&handle, "usage", &format!("갱신 실패: {message}"));
                }
            }
            // 다음 주기까지 대기(프로세스 종료 시 함께 정리)
            std::thread::sleep(Duration::from_secs(USAGE_TICK_SECS));
        }
    });
}

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

fn refresh_stale_usage(app: &AppHandle) -> Result<u64, AppError> {
    let path = db_path(app)?;
    let vault = app.state::<crate::vault::VaultState>();
    let now_ms = now_secs() * 1000;
    let read_path = path.clone();
    let (rows, fields) = tauri::async_runtime::block_on(async move {
        let pool = SqlitePool::connect_with(
            SqliteConnectOptions::new().filename(&read_path).read_only(false),
        )
        .await
        .map_err(|error| AppError::Io(format!("DB 연결 실패: {error}")))?;
        let rows = sqlx::query(
            "SELECT u.id AS id, u.credential_id AS credential_id, u.adapter AS adapter, u.fetched_at AS fetched_at,
                    c.secret_id AS secret_id, p.base_url AS base_url
             FROM usage_snapshots u
             JOIN credentials c ON c.id = u.credential_id
             LEFT JOIN accounts a ON a.id = c.account_id
             LEFT JOIN providers p ON p.id = a.provider_id",
        )
        .fetch_all(&pool)
        .await
        .map_err(|error| AppError::Io(format!("사용량 조회 실패: {error}")))?;
        let fields = sqlx::query(
            "SELECT credential_id, secret_id, label, env_name FROM credential_fields",
        )
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        Ok::<_, AppError>((rows, fields))
    })?;

    let parse_ms = |text: &str| -> Option<i64> {
        crate::monitors::parse_rfc3339_secs(text).map(|secs| secs * 1000)
    };
    let mut updated = 0u64;
    let mut writes: Vec<(String, String, crate::usage::UsageOutcome)> = Vec::new();
    for row in &rows {
        let adapter: String = row.get("adapter");
        let fetched_at: String = row.get("fetched_at");
        if !is_stale(parse_ms(&fetched_at), now_ms, STALE_USAGE_MS) {
            continue;
        }
        let secret_id: String = row.get("secret_id");
        let base_url: Option<String> = row.try_get("base_url").ok().flatten();
        // xAI처럼 추가 시크릿이 필요한 어댑터: 같은 자격 증명의 team 필드를 재사용.
        let extra_secret_id: Option<String> = if adapter == "xai" {
            let credential_id: String = row.try_get("credential_id").unwrap_or_default();
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
        match tauri::async_runtime::block_on(crate::usage::fetch_usage_with(
            &vault,
            &adapter,
            base_url.as_deref(),
            &secret_id,
            extra_secret_id.as_deref(),
            None,
        )) {
            Ok(outcome) => {
                let id: String = row.get("id");
                writes.push((id, adapter, outcome));
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
    if !writes.is_empty() {
        let stamp = crate::usage::epoch_to_rfc3339(now_secs());
        let write_path = path.clone();
        let written = tauri::async_runtime::block_on(async move {
            let pool = SqlitePool::connect_with(
                SqliteConnectOptions::new().filename(&write_path).read_only(false),
            )
            .await
            .map_err(|error| AppError::Io(format!("DB 연결 실패: {error}")))?;
            let mut written = 0u64;
            for (id, adapter, outcome) in writes {
                let details = outcome
                    .details
                    .as_ref()
                    .map(|value| value.to_string());
                sqlx::query(
                    "UPDATE usage_snapshots SET adapter = ?1, status = ?2, summary = ?3,
                     used = ?4, limit_total = ?5, remaining = ?6, currency = ?7,
                     details_json = ?8, fetched_at = ?9 WHERE id = ?10",
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
                .bind(&id)
                .execute(&pool)
                .await
                .map_err(|error| AppError::Io(format!("사용량 저장 실패: {error}")))?;
                written += 1;
            }
            Ok::<_, AppError>(written)
        })?;
        updated = written;
    }
    Ok(updated)
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
                "ready": usage.ready,
                "lastRefreshAt": usage.last_refresh_at,
                "lastError": usage.last_error,
                "updated": usage.updated,
            })
        }))
        .unwrap_or_else(|| json!({ "ready": false }));
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
    fn staleness_rule_matches_six_hours() {
        let now = 10_000_000_000i64;
        let six_hours = STALE_USAGE_MS;
        assert!(!is_stale(Some(now - 60_000), now, six_hours));
        assert!(!is_stale(Some(now - six_hours + 1), now, six_hours));
        assert!(is_stale(Some(now - six_hours), now, six_hours));
        assert!(is_stale(None, now, six_hours), "파싱 실패는 갱신 대상");
    }
}
