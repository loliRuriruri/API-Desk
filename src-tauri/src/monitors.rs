use std::path::PathBuf;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};

use crate::error::AppError;

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_UA: &str = "codex_cli_rs/0.76.0 (Windows 10.0.26200; x86_64) WindowsTerminal";
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const GROK_SUBS_URL: &str = "https://grok.com/rest/subscriptions";
const ANTIGRAVITY_STATUS_RPC: &str =
    "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const ANTIGRAVITY_QUOTA_RPC: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const OPENCODE_USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const CALL_TIMEOUT_MS: u64 = 12_000;
const LOCAL_TIMEOUT_MS: u64 = 6_000;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonitorProbe {
    pub monitor: String,
    pub label: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorOutcome {
    pub monitor: String,
    pub status: String,
    pub summary: String,
    pub details: Option<Value>,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityAccount {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub tier: Option<String>,
    pub saved_at: String,
    pub is_active: bool,
}

fn home_dir() -> Result<PathBuf, AppError> {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .map_err(|_| AppError::Io("USERPROFILE 환경변수를 찾을 수 없습니다".into()))
}

fn appdata_dir() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(PathBuf::from)
}

fn read_json_file(path: &PathBuf) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let part = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(part).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn clamp_percent(value: f64) -> f64 {
    (value.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

fn trim_number(value: f64) -> String {
    if (value.fract()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn remaining_from_used(used: Option<f64>) -> Option<f64> {
    used.map(|used| clamp_percent(100.0 - used))
}

fn codex_reset_timestamp(raw: &Value) -> Option<String> {
    if let Some(value) = raw.get("reset_at").or_else(|| raw.get("resetAt")) {
        if let Some(number) = value.as_f64() {
            return Some(crate::usage::epoch_to_rfc3339(number as i64));
        }
        if let Some(text) = value.as_str() {
            if let Ok(number) = text.parse::<f64>() {
                return Some(crate::usage::epoch_to_rfc3339(number as i64));
            }
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    let seconds = raw
        .get("reset_after_seconds")
        .or_else(|| raw.get("resetAfterSeconds"))
        .and_then(Value::as_f64)?;
    Some(crate::usage::epoch_to_rfc3339(now_millis() / 1000 + seconds as i64))
}

fn reset_hint_until(iso: &str) -> Option<String> {
    let target = parse_rfc3339_secs(iso)?;
    let now = now_millis() / 1000;
    Some(format!("리셋 {}", format_reset_in((target - now).max(0))))
}

pub fn parse_codex_usage(
    body: &Value,
    fallback_plan: Option<&str>,
    fallback_email: Option<&str>,
) -> MonitorOutcome {
    let plan = body
        .get("plan_type")
        .or_else(|| body.get("planType"))
        .and_then(Value::as_str)
        .or(fallback_plan)
        .unwrap_or("Codex");
    let email = body
        .get("email")
        .and_then(Value::as_str)
        .or(fallback_email)
        .unwrap_or("");
    let rate = body
        .get("rate_limit")
        .or_else(|| body.get("rateLimit"))
        .cloned()
        .unwrap_or(json!({}));
    let limit_reached = rate
        .get("limit_reached")
        .or_else(|| rate.get("limitReached"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let window = |key: &str| -> Option<(f64, Option<String>)> {
        let raw = rate
            .get(key)
            .or_else(|| {
                let camel = match key {
                    "primary_window" => "primaryWindow",
                    "secondary_window" => "secondaryWindow",
                    other => other,
                };
                rate.get(camel)
            })?
            .clone();
        let used = raw
            .get("used_percent")
            .or_else(|| raw.get("usedPercent"))
            .and_then(Value::as_f64)?;
        let reset = codex_reset_timestamp(&raw);
        Some((clamp_percent(used), reset))
    };

    let primary = window("primary_window");
    let secondary = window("secondary_window");
    let primary_remaining = primary
        .as_ref()
        .and_then(|(used, _)| remaining_from_used(Some(*used)));
    let secondary_remaining = secondary
        .as_ref()
        .and_then(|(used, _)| remaining_from_used(Some(*used)));

    let mut parts: Vec<String> = Vec::new();
    if !plan.is_empty() {
        parts.push(plan.to_string());
    }
    if let Some(remaining) = primary_remaining {
        let hint = primary
            .as_ref()
            .and_then(|(_, reset)| reset.as_deref())
            .and_then(reset_hint_until);
        parts.push(match hint {
            Some(hint) => format!("주간 {}% 남음 ({hint})", trim_number(remaining)),
            None => format!("주간 {}% 남음", trim_number(remaining)),
        });
    }
    if let Some(remaining) = secondary_remaining {
        let hint = secondary
            .as_ref()
            .and_then(|(_, reset)| reset.as_deref())
            .and_then(reset_hint_until);
        parts.push(match hint {
            Some(hint) => format!("5시간 {}% 남음 ({hint})", trim_number(remaining)),
            None => format!("5시간 {}% 남음", trim_number(remaining)),
        });
    }
    if limit_reached {
        parts.push("한도 도달".into());
    }
    if parts.is_empty() {
        parts.push("사용량 정보 없음".into());
    }

    MonitorOutcome {
        monitor: "codex".into(),
        status: "ok".into(),
        summary: parts.join(" · "),
        details: Some(json!({
            "plan": plan,
            "email": email,
            "limitReached": limit_reached,
            "primaryUsedPercent": primary.as_ref().map(|(used, _)| *used),
            "primaryResetAt": primary.as_ref().and_then(|(_, reset)| reset.clone()),
            "secondaryUsedPercent": secondary.as_ref().map(|(used, _)| *used),
            "secondaryResetAt": secondary.as_ref().and_then(|(_, reset)| reset.clone()),
        })),
        message: None,
    }
}

pub fn parse_grok_billing(billing: &Value, tier: Option<&str>) -> MonitorOutcome {
    let config = billing.get("config").cloned().unwrap_or(json!({}));
    let limit = config
        .get("monthlyLimit")
        .and_then(|value| value.get("val"))
        .and_then(Value::as_f64);
    let used = config
        .get("used")
        .and_then(|value| value.get("val"))
        .and_then(Value::as_f64);
    let (used_percent, remaining) = match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => (
            Some((used / limit * 100.0 * 10.0).round() / 10.0),
            Some((limit - used).max(0.0)),
        ),
        _ => (None, None),
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(tier) = tier {
        parts.push(tier.to_string());
    }
    match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => {
            let percent = (used / limit * 100.0 * 10.0).round() / 10.0;
            parts.push(format!(
                "사용 {}/{} ({}%)",
                trim_number(used),
                trim_number(limit),
                trim_number(percent)
            ));
        }
        (Some(used), _) => {
            parts.push(format!("사용 {} · 월 한도 정보 없음", trim_number(used)));
        }
        _ => parts.push("구독 사용량 정보 없음".into()),
    }
    if let Some(remaining) = remaining {
        parts.push(format!("남음 {}", trim_number(remaining)));
    }

    MonitorOutcome {
        monitor: "grok".into(),
        status: "ok".into(),
        summary: parts.join(" · "),
        details: Some(json!({
            "tier": tier,
            "monthlyLimit": limit,
            "used": used,
            "usedPercent": used_percent,
            "remaining": remaining,
        })),
        message: None,
    }
}

pub fn parse_grok_tier(subs: &Value) -> Option<String> {
    let list = subs.get("subscriptions")?.as_array()?;
    let active = list
        .iter()
        .find(|entry| {
            entry
                .get("status")
                .and_then(Value::as_str)
                .map(|status| status.contains("ACTIVE"))
                .unwrap_or(false)
        })
        .or_else(|| list.first())?;
    let tier = active.get("tier").and_then(Value::as_str)?;
    let cleaned = tier.trim_start_matches("SUBSCRIPTION_TIER_").replace('_', " ");
    let mut result = String::new();
    for word in cleaned.split_whitespace() {
        let lower = word.to_lowercase();
        let mut chars = lower.chars();
        if let Some(first) = chars.next() {
            result.extend(first.to_uppercase());
            result.extend(chars);
            result.push(' ');
        }
    }
    let result = result.trim().to_string();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn percentage(fraction: Option<f64>) -> Option<f64> {
    fraction.map(|value| clamp_percent(value * 100.0))
}

pub fn quota_bucket(group: &Value, window: &str) -> Option<(Option<f64>, Option<String>)> {
    let buckets = group.get("buckets")?.as_array()?;
    let bucket = buckets.iter().find(|bucket| {
        bucket
            .get("window")
            .and_then(Value::as_str)
            .map(|value| value.eq_ignore_ascii_case(window))
            .unwrap_or(false)
            || bucket
                .get("displayName")
                .and_then(Value::as_str)
                .map(|value| {
                    if window == "weekly" {
                        value.to_lowercase().contains("week")
                    } else {
                        value
                            .to_lowercase()
                            .replace(' ', "")
                            .contains("5hour")
                    }
                })
                .unwrap_or(false)
    })?;
    let remaining = percentage(
        bucket
            .get("remainingFraction")
            .and_then(Value::as_f64),
    );
    let reset = bucket
        .get("resetTime")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((remaining, reset))
}

pub fn parse_antigravity_quota_groups(body: &Value) -> Value {
    let groups = body
        .get("response")
        .and_then(|response| response.get("groups"))
        .or_else(|| body.get("groups"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut result = Vec::new();
    for group in &groups {
        let name = group
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let weekly = quota_bucket(group, "weekly");
        let five_hour = quota_bucket(group, "5h");
        result.push(json!({
            "name": name,
            "weeklyRemainingPercent": weekly.as_ref().and_then(|(percent, _)| *percent),
            "weeklyResetAt": weekly.as_ref().and_then(|(_, reset)| reset.clone()),
            "fiveHourRemainingPercent": five_hour.as_ref().and_then(|(percent, _)| *percent),
            "fiveHourResetAt": five_hour.as_ref().and_then(|(_, reset)| reset.clone()),
        }));
    }
    Value::Array(result)
}

pub fn parse_antigravity_status(body: &Value, quota_groups: Option<&Value>) -> MonitorOutcome {
    let status = body
        .get("userStatus")
        .or_else(|| body.get("user_status"))
        .cloned()
        .unwrap_or(json!({}));
    let email = status.get("email").and_then(Value::as_str).unwrap_or("");
    let tier = status
        .get("userTier")
        .and_then(|tier| tier.get("name"))
        .and_then(Value::as_str)
        .or_else(|| {
            status
                .get("planStatus")
                .and_then(|plan| plan.get("planInfo"))
                .and_then(|info| info.get("planName"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Google AI Pro");

    let configs = status
        .get("cascadeModelConfigData")
        .and_then(|data| data.get("clientModelConfigs"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut models: Vec<Value> = Vec::new();
    let mut gemini_min: Option<f64> = None;
    let mut claude_min: Option<f64> = None;
    for config in &configs {
        let label = config.get("label").and_then(Value::as_str).unwrap_or("");
        let remaining_percent = percentage(
            config
                .get("quotaInfo")
                .and_then(|info| info.get("remainingFraction"))
                .and_then(Value::as_f64),
        );
        let reset = config
            .get("quotaInfo")
            .and_then(|info| info.get("resetTime"))
            .and_then(Value::as_str);
        if let Some(percent) = remaining_percent {
            let lower = label.to_lowercase();
            if lower.contains("gemini") {
                gemini_min = Some(gemini_min.map_or(percent, |current: f64| current.min(percent)));
            } else if lower.contains("claude") {
                claude_min = Some(claude_min.map_or(percent, |current: f64| current.min(percent)));
            }
        }
        if models.len() < 16 && !label.is_empty() {
            models.push(json!({
                "label": label,
                "remainingPercent": remaining_percent,
                "resetAt": reset,
            }));
        }
    }

    let groups = quota_groups.cloned().unwrap_or_else(|| json!([]));
    let group_value = |needle: &str| -> Option<Value> {
        groups.as_array()?.iter().find_map(|group| {
            let name = group.get("name")?.as_str()?.to_lowercase();
            if name.contains(needle) {
                Some(group.clone())
            } else {
                None
            }
        })
    };
    let gemini_group = group_value("gemini");
    let claude_group = group_value("claude");

    let mut parts: Vec<String> = vec![tier.to_string()];
    for (label, group) in [("Gemini", &gemini_group), ("Claude", &claude_group)] {
        if let Some(group) = group {
            let weekly = group.get("weeklyRemainingPercent").and_then(Value::as_f64);
            let five_hour = group
                .get("fiveHourRemainingPercent")
                .and_then(Value::as_f64);
            let mut segments = Vec::new();
            if let Some(weekly) = weekly {
                segments.push(format!("주간 {}%", trim_number(weekly)));
            }
            if let Some(five_hour) = five_hour {
                segments.push(format!("5시간 {}%", trim_number(five_hour)));
            }
            if !segments.is_empty() {
                parts.push(format!("{label} {}", segments.join(" · ")));
            }
        }
    }
    if parts.len() == 1 {
        if let Some(gemini) = gemini_min {
            parts.push(format!("Gemini {}% 남음", trim_number(gemini)));
        }
        if let Some(claude) = claude_min {
            parts.push(format!("Claude {}% 남음", trim_number(claude)));
        }
    }
    if parts.len() == 1 {
        parts.push("쿼터 정보 없음".into());
    }

    MonitorOutcome {
        monitor: "antigravity".into(),
        status: "ok".into(),
        summary: parts.join(" · "),
        details: Some(json!({
            "email": email,
            "tier": tier,
            "geminiRemainingPercent": gemini_min,
            "claudeRemainingPercent": claude_min,
            "quotaGroups": groups,
            "models": models,
        })),
        message: None,
    }
}

fn unavailable(monitor: &str, message: &str) -> MonitorOutcome {
    MonitorOutcome {
        monitor: monitor.into(),
        status: "unavailable".into(),
        summary: String::new(),
        details: None,
        message: Some(message.into()),
    }
}

fn codex_auth_path() -> Option<PathBuf> {
    home_dir().ok().map(|home| home.join(".codex").join("auth.json"))
}

fn grok_auth_path() -> Option<PathBuf> {
    home_dir().ok().map(|home| home.join(".grok").join("auth.json"))
}

fn grok_token() -> Option<String> {
    if let Ok(value) = std::env::var("XAI_API_KEY") {
        let trimmed = value.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    let auth = read_json_file(&grok_auth_path()?)?;
    for value in auth.as_object()?.values() {
        if let Some(key) = value.get("key").and_then(Value::as_str) {
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    None
}

fn grok_cli_path() -> Option<PathBuf> {
    let managed = home_dir().ok()?.join(".grok").join("bin").join("grok.exe");
    if managed.exists() {
        Some(managed)
    } else {
        None
    }
}

pub fn parse_grok_build_billing(body: &Value) -> MonitorOutcome {
    let config = body.get("config").cloned().unwrap_or(json!({}));
    let tier = body
        .get("subscription_tier")
        .and_then(Value::as_str)
        .map(|value| {
            let cleaned = value.trim_start_matches("SUBSCRIPTION_TIER_").replace('_', " ");
            if cleaned.is_empty() {
                "SuperGrok".to_string()
            } else {
                cleaned
            }
        })
        .unwrap_or_else(|| "SuperGrok".into());
    let unified = config
        .get("isUnifiedBillingUser")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let period = config.get("currentPeriod").cloned().unwrap_or(json!({}));
    let period_type = period
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase();
    let period_label = if period_type.contains("WEEKLY") {
        "주간"
    } else if period_type.contains("MONTHLY") {
        "월간"
    } else {
        "기간"
    };
    let period_end = period
        .get("end")
        .and_then(Value::as_str)
        .map(str::to_string);
    let reset_hint = period_end
        .as_deref()
        .and_then(parse_rfc3339_secs)
        .map(|target| {
            let now = now_millis() / 1000;
            format_reset_in((target - now).max(0))
        });

    let used_percent = config
        .get("creditUsagePercent")
        .and_then(Value::as_f64)
        .map(|value| clamp_percent(value));

    let balance = |key: &str| -> Option<f64> {
        config
            .get(key)
            .and_then(|entry| entry.get("val"))
            .and_then(|value| match value {
                Value::String(text) => text.parse::<f64>().ok(),
                other => other.as_f64(),
            })
    };
    let prepaid = balance("prepaidBalance");
    let on_demand_cap = balance("onDemandCap");
    let on_demand_used = balance("onDemandUsed");

    let mut parts = vec![tier.clone()];
    if let Some(percent) = used_percent {
        let remaining = clamp_percent(100.0 - percent);
        parts.push(format!("{period_label} {}% 남음", trim_number(remaining)));
    } else {
        parts.push(format!("{period_label} 사용량 정보 없음"));
    }
    if let Some(reset) = &reset_hint {
        parts.push(format!("리셋 {reset}"));
    }
    if let Some(prepaid) = prepaid {
        if prepaid > 0.0 {
            parts.push(format!("선불 ${}", trim_number(prepaid / 100.0)));
        }
    }
    if let (Some(cap), Some(used)) = (on_demand_cap, on_demand_used) {
        if cap > 0.0 {
            parts.push(format!(
                "온디맨드 {}/{}",
                trim_number(used / 100.0),
                trim_number(cap / 100.0)
            ));
        }
    }

    MonitorOutcome {
        monitor: "grok-build".into(),
        status: "ok".into(),
        summary: format!("Grok Build {}", parts.join(" · ")),
        details: Some(json!({
            "tier": tier,
            "unifiedBilling": unified,
            "periodLabel": period_label,
            "periodEnd": period_end,
            "usedPercent": used_percent,
            "remainingPercent": used_percent.map(|percent| clamp_percent(100.0 - percent)),
            "prepaidBalance": prepaid,
            "onDemandCap": on_demand_cap,
            "onDemandUsed": on_demand_used,
        })),
        message: None,
    }
}

fn hidden_command(program: &PathBuf) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn run_grok_build_billing(timeout_ms: u64) -> Option<Value> {
    use std::io::{BufRead, BufReader, Write};

    let cli = grok_cli_path()?;
    let mut child = hidden_command(&cli)
        .args(["agent", "stdio"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let stdout = child.stdout.take()?;
    let mut stdin = child.stdin.take()?;
    let start = std::time::Instant::now();
    let deadline = Duration::from_millis(timeout_ms);

    let _ = stdin.write_all(
        format!(
            "{}\n",
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "clientInfo": { "name": "api-desk", "version": "0.1" },
                    "capabilities": {}
                }
            })
        )
        .as_bytes(),
    );
    let _ = stdin.flush();

    let mut reader = BufReader::new(stdout);
    let mut requested = false;
    let mut result: Option<Value> = None;

    loop {
        if start.elapsed() > deadline {
            break;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                if message.get("id").and_then(Value::as_i64) == Some(1) && !requested {
                    requested = true;
                    let _ = stdin.write_all(
                        format!(
                            "{}\n",
                            json!({
                                "jsonrpc": "2.0",
                                "id": 2,
                                "method": "_x.ai/billing",
                                "params": {}
                            })
                        )
                        .as_bytes(),
                    );
                    let _ = stdin.flush();
                    continue;
                }
                if message.get("id").and_then(Value::as_i64) == Some(2) {
                    result = message.get("result").cloned();
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let _ = child.kill();
    result
}

fn antigravity_endpoint_candidates() -> Vec<(String, u16, String)> {
    let Some(log_path) = appdata_dir().map(|dir| dir.join("Antigravity").join("logs").join("main.log"))
    else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(log_path) else {
        return Vec::new();
    };
    parse_antigravity_endpoints(&content)
}

fn parse_antigravity_endpoints(content: &str) -> Vec<(String, u16, String)> {
    let lines: Vec<&str> = content.lines().collect();

    let mut spawns: Vec<(usize, String)> = Vec::new();
    let mut ports: Vec<(usize, u16)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(at) = line.find("--csrf_token") {
            let rest = line[at + "--csrf_token".len()..].trim_start();
            let token: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !token.is_empty() {
                spawns.push((index, token));
            }
        }
        if let Some(at) = line.find("https://127.0.0.1:") {
            let rest = &line[at + "https://127.0.0.1:".len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>() {
                ports.push((index, port));
            }
        }
    }

    let mut candidates: Vec<(u16, String)> = Vec::new();
    for (spawn_index, token) in spawns.iter().rev() {
        let next_spawn = spawns
            .iter()
            .map(|(index, _)| *index)
            .filter(|index| index > spawn_index)
            .min()
            .unwrap_or(usize::MAX);
        let mut block_ports: Vec<u16> = ports
            .iter()
            .filter(|(index, _)| *index > *spawn_index && *index < next_spawn)
            .map(|(_, port)| *port)
            .collect();
        block_ports.sort_unstable();
        block_ports.dedup();
        for port in block_ports.into_iter().rev() {
            if candidates.iter().any(|(known, _)| *known == port) {
                continue;
            }
            candidates.push((port, token.clone()));
            if candidates.len() >= 6 {
                break;
            }
        }
        if candidates.len() >= 6 {
            break;
        }
    }

    candidates
        .into_iter()
        .map(|(port, token)| ("https://127.0.0.1".to_string(), port, token))
        .collect()
}

fn find_antigravity_endpoint() -> Option<(String, u16, String)> {
    antigravity_endpoint_candidates().into_iter().next()
}

fn antigravity_process_csrf() -> Option<String> {
    let script = r#"
$ProgressPreference = 'SilentlyContinue'
$p = Get-CimInstance Win32_Process -Filter "Name='language_server.exe'" | Select-Object -First 1
if ($p -and $p.CommandLine) {
  $i = $p.CommandLine.IndexOf('--csrf_token')
  if ($i -ge 0) {
    $rest = $p.CommandLine.Substring($i + 12).TrimStart()
    ($rest -split '\s+')[0]
  }
}
"#;
    let output = run_powershell(script).ok()?;
    let token = output.trim().to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn tcp_alive(port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, std::time::Duration::from_millis(400)).is_ok()
}

fn antigravity_endpoint_live() -> bool {
    antigravity_endpoint_candidates()
        .into_iter()
        .take(4)
        .any(|(_, port, _)| tcp_alive(port))
}

fn opencode_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for key in ["USAGE_WIDGET_OPENCODE_DIR", "OPENCODE_CONFIG_DIR"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                dirs.push(PathBuf::from(value));
            }
        }
    }
    if let Ok(home) = home_dir() {
        dirs.push(home.join(".local").join("share").join("opencode"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("opencode"));
    }
    if let Some(appdata) = appdata_dir() {
        dirs.push(appdata.join("opencode"));
    }
    dirs
}

fn opencode_db_path() -> Option<PathBuf> {
    for dir in opencode_candidate_dirs() {
        let candidate = dir.join("opencode.db");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn opencode_auth_path() -> Option<PathBuf> {
    for dir in opencode_candidate_dirs() {
        let candidate = dir.join("auth.json");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn opencode_go_key() -> Option<String> {
    let path = opencode_auth_path()?;
    let auth = read_json_file(&path)?;
    let key = auth
        .get("opencode-go")
        .and_then(|entry| entry.get("key"))
        .and_then(Value::as_str)?;
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

fn reset_in_seconds(value: &Value) -> Option<i64> {
    let text = value
        .get("resetsAt")
        .or_else(|| value.get("resets_at"))
        .and_then(Value::as_str)?;
    let target = parse_rfc3339_secs(text)?;
    let now = now_millis() / 1000;
    Some((target - now).max(0))
}

fn parse_rfc3339_secs(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let number = |start: usize, len: usize| -> Option<i64> {
        text.get(start..start + len)?.parse::<i64>().ok()
    };
    let year = number(0, 4)?;
    let month = number(5, 2)?;
    let day = number(8, 2)?;
    let hour = number(11, 2)?;
    let minute = number(14, 2)?;
    let second = number(17, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn format_reset_in(seconds: i64) -> String {
    let hours = seconds / 3_600;
    let days = hours / 24;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {}h", hours % 24)
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub fn parse_opencode_usage(body: &Value) -> Option<(String, Value, f64)> {
    let usage = body.get("usage")?;
    let window = |key: &str| -> Option<Value> {
        usage.get(key).map(|entry| entry.clone())
    };
    let rolling = window("rolling")?;
    let weekly = window("weekly");
    let monthly = window("monthly");
    let percent = |value: &Option<Value>| -> Option<f64> {
        value
            .as_ref()
            .and_then(|entry| entry.get("percent"))
            .and_then(Value::as_f64)
    };
    let label = |value: &Option<Value>, label: &str| -> Option<String> {
        let percent = percent(value)?;
        let reset = value
            .as_ref()
            .and_then(reset_in_seconds)
            .map(format_reset_in);
        Some(match reset {
            Some(reset) => format!("{label} {}% ({reset})", trim_number(percent)),
            None => format!("{label} {}%", trim_number(percent)),
        })
    };

    let rolling_label = label(&Some(rolling.clone()), "롤링")?;
    let weekly_label = label(&weekly, "주간");
    let monthly_label = label(&monthly, "월간");
    let mut parts = vec![rolling_label];
    if let Some(value) = weekly_label {
        parts.push(value);
    }
    if let Some(value) = monthly_label {
        parts.push(value);
    }
    let summary = format!("OpenCode Go {}", parts.join(" · "));

    let max_used = [percent(&Some(rolling.clone())), percent(&weekly), percent(&monthly)]
        .into_iter()
        .flatten()
        .fold(0.0_f64, f64::max);
    let remaining = clamp_percent(100.0 - max_used);

    let details = json!({
        "rolling": to_usage_entry(&Some(rolling)),
        "weekly": to_usage_entry(&weekly),
        "monthly": to_usage_entry(&monthly),
        "maxUsedPercent": max_used,
        "remainingPercent": remaining,
    });
    Some((summary, details, remaining))
}

fn to_usage_entry(value: &Option<Value>) -> Value {
    match value {
        Some(entry) => json!({
            "percent": entry.get("percent").and_then(Value::as_f64),
            "status": entry.get("status").and_then(Value::as_str),
            "resetsAt": entry.get("resetsAt").and_then(Value::as_str),
            "resetInSec": reset_in_seconds(entry),
        }),
        None => Value::Null,
    }
}

const WINDOWS: [(&str, i64); 4] = [
    ("fiveHour", 5 * 3_600),
    ("daily", 24 * 3_600),
    ("weekly", 7 * 24 * 3_600),
    ("monthly", 30 * 24 * 3_600),
];

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn bucket_sessions(sessions: &[(i64, f64)], now_ms: i64) -> Value {
    let mut windows = serde_json::Map::new();
    for (key, span_seconds) in WINDOWS {
        let cutoff = now_ms - span_seconds * 1000;
        let mut count = 0_u64;
        let mut cost = 0.0_f64;
        for (created, session_cost) in sessions {
            if *created >= cutoff {
                count += 1;
                cost += *session_cost;
            }
        }
        windows.insert(
            key.to_string(),
            json!({ "sessions": count, "cost": (cost * 10000.0).round() / 10000.0 }),
        );
    }
    Value::Object(windows)
}

fn format_cost(cost: f64) -> String {
    if cost == 0.0 {
        "$0".into()
    } else if cost < 0.01 {
        format!("${cost:.4}")
    } else if cost < 1.0 {
        format!("${cost:.3}")
    } else {
        format!("${cost:.2}")
    }
}

async fn query_opencode_db(db_path: &PathBuf) -> Result<(Value, Value, Value), AppError> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true);
    let pool = SqlitePool::connect_with(options)
        .await
        .map_err(|e| AppError::Io(format!("opencode.db 연결 실패: {e}")))?;

    let rows = sqlx::query("SELECT time_created, cost FROM session")
        .fetch_all(&pool)
        .await
        .map_err(|e| AppError::Io(format!("세션 조회 실패: {e}")))?;
    let sessions: Vec<(i64, f64)> = rows
        .iter()
        .map(|row| {
            let created: Option<i64> = row.try_get("time_created").ok();
            let cost: Option<f64> = row.try_get("cost").ok();
            (created.unwrap_or(0), cost.unwrap_or(0.0))
        })
        .collect();
    let windows = bucket_sessions(&sessions, now_millis());

    let model_rows = sqlx::query(
        "SELECT model, COUNT(*) AS c, COALESCE(SUM(cost), 0) AS cost FROM session \
         WHERE time_created >= ? GROUP BY model ORDER BY cost DESC LIMIT 6",
    )
    .bind(now_millis() - 7 * 24 * 3_600 * 1000)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();
    let mut models = Vec::new();
    for row in &model_rows {
        let raw: Option<String> = row.try_get("model").ok();
        let count: Option<i64> = row.try_get("c").ok();
        let cost: Option<f64> = row.try_get("cost").ok();
        let label = raw
            .as_deref()
            .map(parse_session_model)
            .unwrap_or_else(|| "unknown".into());
        models.push(json!({
            "label": label,
            "remainingPercent": null,
            "meta": format!("{}세션 · {}", count.unwrap_or(0), format_cost(cost.unwrap_or(0.0))),
        }));
    }

    let last = sqlx::query(
        "SELECT title, model, COALESCE(cost, 0) AS cost, time_created FROM session \
         ORDER BY time_created DESC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);
    let last_session = match last {
        Some(row) => {
            let title: Option<String> = row.try_get("title").ok();
            let model: Option<String> = row.try_get("model").ok();
            let cost: Option<f64> = row.try_get("cost").ok();
            let at: Option<i64> = row.try_get("time_created").ok();
            json!({
                "title": title,
                "model": model.as_deref().map(parse_session_model),
                "cost": cost.unwrap_or(0.0),
                "at": at.unwrap_or(0),
            })
        }
        None => json!(null),
    };

    let _ = pool.close().await;
    Ok((
        windows,
        Value::Array(models),
        last_session,
    ))
}

fn parse_session_model(raw: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        if let Some(id) = parsed.get("id").and_then(Value::as_str) {
            return id.to_string();
        }
    }
    raw.to_string()
}

fn antigravity_accounts_dir() -> Option<PathBuf> {
    let dir = appdata_dir()?.join("com.apidesk.desktop").join("antigravity-accounts");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn antigravity_index_path() -> Option<PathBuf> {
    Some(antigravity_accounts_dir()?.join("index.json"))
}

fn load_antigravity_accounts() -> Vec<AntigravityAccount> {
    let Some(path) = antigravity_index_path() else {
        return Vec::new();
    };
    let Some(value) = read_json_file(&path) else {
        return Vec::new();
    };
    serde_json::from_value(value).unwrap_or_default()
}

fn save_antigravity_accounts(accounts: &[AntigravityAccount]) -> Result<(), AppError> {
    let path = antigravity_index_path().ok_or_else(|| AppError::Io("계정 폴더를 만들 수 없습니다".into()))?;
    let text = serde_json::to_string_pretty(accounts).map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

fn powershell_encoded(script: &str) -> String {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    STANDARD.encode(utf16)
}

fn run_powershell(script: &str) -> Result<String, AppError> {
    let encoded = powershell_encoded(script);
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded,
        ])
        .output()
        .map_err(|e| AppError::Io(format!("PowerShell 실행 실패: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Io(format!(
            "PowerShell 실패: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

const CRED_READ_SCRIPT: &str = r#"
$ProgressPreference = 'SilentlyContinue'
$sig = @'
using System;
using System.Runtime.InteropServices;
public class CredHelper {
    [DllImport("Advapi32.dll", EntryPoint = "CredReadW", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool CredRead(string target, int type, int reservedFlag, out IntPtr credentialPtr);
    [DllImport("Advapi32.dll", EntryPoint = "CredFree", SetLastError = true)]
    public static extern void CredFree(IntPtr credential);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct CREDENTIAL {
        public int Flags;
        public int Type;
        public string TargetName;
        public string Comment;
        public long LastWritten;
        public int CredentialBlobSize;
        public IntPtr CredentialBlob;
        public int Persist;
        public int AttributeCount;
        public IntPtr Attributes;
        public string TargetAlias;
        public string UserName;
    }
}
'@
Add-Type -TypeDefinition $sig
$ptr = [IntPtr]::Zero
if ([CredHelper]::CredRead("gemini:antigravity", 1, 0, [ref]$ptr)) {
    $cred = [System.Runtime.InteropServices.Marshal]::PtrToStructure($ptr, [Type][CredHelper+CREDENTIAL])
    $bytes = New-Object byte[] $cred.CredentialBlobSize
    [System.Runtime.InteropServices.Marshal]::Copy($cred.CredentialBlob, $bytes, 0, $cred.CredentialBlobSize)
    [CredHelper]::CredFree($ptr)
    [Console]::Out.Write([System.Text.Encoding]::UTF8.GetString($bytes))
}
"#;

fn cred_write_script(secret: &str) -> String {
    let escaped = secret.replace('\'', "''");
    format!(
        r#"
$ProgressPreference = 'SilentlyContinue'
$sig = @'
using System;
using System.Runtime.InteropServices;
public class CredWriter {{
    [DllImport("Advapi32.dll", EntryPoint = "CredWriteW", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool CredWrite([In] ref CREDENTIAL userCredential, [In] uint flags);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct CREDENTIAL {{
        public uint Flags;
        public uint Type;
        public string TargetName;
        public string Comment;
        public long LastWritten;
        public uint CredentialBlobSize;
        public IntPtr CredentialBlob;
        public uint Persist;
        public uint AttributeCount;
        public IntPtr Attributes;
        public string TargetAlias;
        public string UserName;
    }}
    public static bool Write(string target, string user, string secret) {{
        byte[] bytes = System.Text.Encoding.UTF8.GetBytes(secret);
        IntPtr blobPtr = Marshal.AllocHGlobal(bytes.Length);
        try {{
            Marshal.Copy(bytes, 0, blobPtr, bytes.Length);
            CREDENTIAL cred = new CREDENTIAL();
            cred.Flags = 0;
            cred.Type = 1;
            cred.TargetName = target;
            cred.UserName = user;
            cred.CredentialBlobSize = (uint)bytes.Length;
            cred.CredentialBlob = blobPtr;
            cred.Persist = 2;
            return CredWrite(ref cred, 0);
        }} finally {{
            Marshal.FreeHGlobal(blobPtr);
        }}
    }}
}}
'@
Add-Type -TypeDefinition $sig
[CredWriter]::Write("gemini:antigravity", "antigravity", '{escaped}')
"#
    )
}

fn read_antigravity_credential() -> Result<String, AppError> {
    let output = run_powershell(CRED_READ_SCRIPT)?;
    if output.is_empty() {
        return Err(AppError::Vault(
            "Windows 자격 증명에서 Antigravity 로그인을 찾을 수 없습니다".into(),
        ));
    }
    Ok(output)
}

#[tauri::command]
pub fn monitor_probe() -> Vec<MonitorProbe> {
    let codex = codex_auth_path().map(|path| path.exists()).unwrap_or(false);
    let grok = grok_token().is_some();
    let grok_build = grok_cli_path().is_some();
    let antigravity = antigravity_endpoint_live();
    let opencode_db = opencode_db_path();
    let opencode_auth = opencode_auth_path();
    let opencode = opencode_db.is_some() || opencode_auth.is_some();
    vec![
        MonitorProbe {
            monitor: "codex".into(),
            label: "Codex".into(),
            available: codex,
            detail: if codex {
                "~/.codex/auth.json 확인됨".into()
            } else {
                "~/.codex/auth.json 없음 (codex login 필요)".into()
            },
        },
        MonitorProbe {
            monitor: "grok".into(),
            label: "Grok Bot".into(),
            available: grok,
            detail: if grok {
                "XAI_API_KEY 또는 ~/.grok/auth.json 확인됨".into()
            } else {
                "XAI_API_KEY / ~/.grok/auth.json 없음".into()
            },
        },
        MonitorProbe {
            monitor: "grok-build".into(),
            label: "Grok Build".into(),
            available: grok_build,
            detail: if grok_build {
                "~/.grok/bin/grok.exe 확인됨".into()
            } else {
                "grok CLI(~/.grok/bin/grok.exe) 없음".into()
            },
        },
        MonitorProbe {
            monitor: "antigravity".into(),
            label: "Antigravity".into(),
            available: antigravity,
            detail: if antigravity {
                "Antigravity 로컬 API 응답 확인".into()
            } else {
                "Antigravity가 실행 중이 아니거나 로컬 API가 응답하지 않습니다".into()
            },
        },
        MonitorProbe {
            monitor: "opencode".into(),
            label: "OpenCode Go".into(),
            available: opencode,
            detail: match (&opencode_db, &opencode_auth) {
                (Some(db), _) => format!("세션 DB 확인됨: {}", db.to_string_lossy()),
                (None, Some(_)) => "auth.json만 발견됨 (세션 기록 없음)".into(),
                _ => "opencode.db / auth.json을 찾을 수 없음".into(),
            },
        },
    ]
}

#[tauri::command]
pub async fn monitor_refresh(monitor: String) -> Result<MonitorOutcome, AppError> {
    match monitor.as_str() {
        "codex" => refresh_codex().await,
        "grok" => refresh_grok().await,
        "grok-build" => refresh_grok_build().await,
        "antigravity" => refresh_antigravity().await,
        "opencode" => refresh_opencode().await,
        other => Err(AppError::InvalidRequest(format!("알 수 없는 모니터: {other}"))),
    }
}

async fn refresh_grok_build() -> Result<MonitorOutcome, AppError> {
    if grok_cli_path().is_none() {
        return Ok(unavailable(
            "grok-build",
            "grok CLI(~/.grok/bin/grok.exe)를 찾을 수 없습니다",
        ));
    }
    let billing = tauri::async_runtime::spawn_blocking(|| run_grok_build_billing(20_000))
        .await
        .map_err(|e| AppError::Io(format!("grok CLI 작업 실패: {e}")))?;
    match billing {
        Some(value) => Ok(parse_grok_build_billing(&value)),
        None => Ok(unavailable(
            "grok-build",
            "grok CLI에서 빌링 정보를 받지 못했습니다 (로그인/업데이트 확인)",
        )),
    }
}

async fn refresh_opencode() -> Result<MonitorOutcome, AppError> {
    let go_key = opencode_go_key();
    let local = match opencode_db_path() {
        Some(db_path) => query_opencode_db(&db_path)
            .await
            .ok()
            .map(|(windows, models, last_session)| (db_path, windows, models, last_session)),
        None => None,
    };

    let mut api_error: Option<String> = None;
    if let Some(key) = go_key.as_deref() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(CALL_TIMEOUT_MS))
            .user_agent("API-Desk/0.1 (local)")
            .build()
            .map_err(|e| AppError::Network(e.to_string()))?;
        let response = client
            .get(OPENCODE_USAGE_URL)
            .header("Authorization", format!("Bearer {key}"))
            .header("Accept", "application/json")
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<Value>().await {
                    Ok(body) => match parse_opencode_usage(&body) {
                        Some((summary, usage_details, remaining)) => {
                            let mut details = usage_details;
                            if let Some(object) = details.as_object_mut() {
                                object.insert("plan".into(), json!("OpenCode Go"));
                                object.insert("goKeyPresent".into(), json!(true));
                                object.insert("usageApiAvailable".into(), json!(true));
                                if let Some((path, windows, models, last_session)) = &local {
                                    object.insert("dbPath".into(), json!(path.to_string_lossy()));
                                    object.insert("localWindows".into(), windows.clone());
                                    object.insert("models".into(), models.clone());
                                    object.insert("lastSession".into(), last_session.clone());
                                }
                                object.insert("remainingPercent".into(), json!(remaining));
                            }
                            return Ok(MonitorOutcome {
                                monitor: "opencode".into(),
                                status: "ok".into(),
                                summary,
                                details: Some(details),
                                message: None,
                            });
                        }
                        None => api_error = Some("usage 응답 형식이 예상과 다릅니다".into()),
                    },
                    Err(error) => api_error = Some(format!("JSON 파싱 실패: {error}")),
                }
            }
            Ok(response) => {
                api_error = Some(format!("HTTP {}", response.status().as_u16()));
            }
            Err(error) => api_error = Some(format!("요청 실패: {error}")),
        }
    }

    let Some((db_path, windows, models, last_session)) = local else {
        return Ok(unavailable(
            "opencode",
            "OpenCode 세션 DB(opencode.db)와 opencode-go 키를 찾을 수 없습니다",
        ));
    };
    let daily = windows
        .get("daily")
        .cloned()
        .unwrap_or(json!({ "sessions": 0, "cost": 0 }));
    let five_hour = windows
        .get("fiveHour")
        .cloned()
        .unwrap_or(json!({ "sessions": 0, "cost": 0 }));
    let weekly = windows
        .get("weekly")
        .cloned()
        .unwrap_or(json!({ "sessions": 0, "cost": 0 }));
    let daily_sessions = daily.get("sessions").and_then(Value::as_u64).unwrap_or(0);
    let daily_cost = daily.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    let five_sessions = five_hour
        .get("sessions")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let five_cost = five_hour.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    let weekly_cost = weekly.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    let model_hint = last_session
        .get("model")
        .and_then(Value::as_str)
        .map(|model| format!(" · 최근 {model}"))
        .unwrap_or_default();
    let api_note = api_error
        .as_deref()
        .map(|reason| format!(" · usage API {reason}"))
        .unwrap_or_default();
    let plan = if go_key.is_some() {
        "OpenCode Go"
    } else {
        "OpenCode"
    };
    let summary = format!(
        "{plan} 로컬 집계 24시간 {daily_sessions}세션 · {}{model_hint}{api_note} · 5시간 {five_sessions}세션 {}",
        format_cost(daily_cost),
        format_cost(five_cost)
    );
    Ok(MonitorOutcome {
        monitor: "opencode".into(),
        status: "ok".into(),
        summary,
        details: Some(json!({
            "plan": plan,
            "goKeyPresent": go_key.is_some(),
            "usageApiAvailable": false,
            "usageApiError": api_error,
            "dbPath": db_path.to_string_lossy(),
            "localWindows": windows,
            "weeklyCost": weekly_cost,
            "models": models,
            "lastSession": last_session,
        })),
        message: Some("Go 사용량 API 조회에 실패해 로컬 세션 기록으로 표시합니다".into()),
    })
}

async fn refresh_codex() -> Result<MonitorOutcome, AppError> {
    let path = codex_auth_path().ok_or_else(|| AppError::Io("홈 폴더를 찾을 수 없습니다".into()))?;
    let auth = read_json_file(&path)
        .ok_or_else(|| AppError::Vault("~/.codex/auth.json을 읽을 수 없습니다".into()))?;
    let tokens = auth.get("tokens").cloned().unwrap_or(json!({}));
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let id_payload = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(decode_jwt_payload)
        .unwrap_or(json!({}));

    if access_token.is_empty() {
        return Ok(unavailable("codex", "Codex 로그인이 필요합니다 (codex login)"));
    }

    let email = id_payload
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    let plan = id_payload
        .get("https://api.openai.com/auth")
        .and_then(|node| node.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(CALL_TIMEOUT_MS))
        .user_agent(CODEX_UA)
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;
    let response = client
        .get(CODEX_USAGE_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("ChatGPT-Account-Id", account_id)
        .send()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    if response.status().as_u16() == 401 {
        return Ok(unavailable(
            "codex",
            "토큰이 만료되었습니다 — codex login 후 다시 시도하세요",
        ));
    }
    if !response.status().is_success() {
        return Ok(unavailable(
            "codex",
            &format!("Codex 사용량 조회 실패 (HTTP {})", response.status().as_u16()),
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    Ok(parse_codex_usage(&body, plan.as_deref(), email.as_deref()))
}

async fn refresh_grok() -> Result<MonitorOutcome, AppError> {
    let token = grok_token().ok_or_else(|| {
        AppError::Vault("XAI_API_KEY 또는 ~/.grok/auth.json을 찾을 수 없습니다".into())
    })?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(CALL_TIMEOUT_MS))
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;
    let bust = now_millis();
    let billing_result = client
        .get(format!("{GROK_BILLING_URL}?_={bust}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Cache-Control", "no-cache, no-store")
        .send()
        .await;
    let billing = match billing_result {
        Ok(response) if response.status().is_success() => response
            .json::<Value>()
            .await
            .map_err(|e| AppError::Network(e.to_string()))?,
        Ok(response) if response.status().as_u16() == 401 => {
            return Ok(unavailable("grok", "Grok 토큰이 만료되었습니다"));
        }
        Ok(response) => {
            return Ok(unavailable(
                "grok",
                &format!("Grok 사용량 조회 실패 (HTTP {})", response.status().as_u16()),
            ));
        }
        Err(error) => return Err(AppError::Network(error.to_string())),
    };

    let tier = match client
        .get(GROK_SUBS_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response.json::<Value>().await.ok(),
        _ => None,
    }
    .and_then(|value| parse_grok_tier(&value));

    Ok(parse_grok_billing(&billing, tier.as_deref()))
}

async fn antigravity_rpc(
    client: &reqwest::Client,
    base: &str,
    port: u16,
    csrf: &str,
    rpc: &str,
) -> Result<Value, AppError> {
    let response = client
        .post(format!("{base}:{port}{rpc}"))
        .header("Content-Type", "application/json")
        .header("x-codeium-csrf-token", csrf)
        .body("{}")
        .send()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Network(format!(
            "로컬 RPC 실패 (HTTP {})",
            response.status().as_u16()
        )));
    }
    response
        .json::<Value>()
        .await
        .map_err(|e| AppError::Network(e.to_string()))
}

fn antigravity_status_email_tier(status: &Value) -> (Option<String>, Option<String>) {
    let user = status.get("userStatus");
    let email = user
        .and_then(|value| value.get("email"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let tier = user
        .and_then(|value| value.get("userTier"))
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (email, tier)
}

fn remember_antigravity_account(status: &Value) -> Vec<AntigravityAccount> {
    let (email, tier) = antigravity_status_email_tier(status);
    let mut accounts = load_antigravity_accounts();
    let mut dirty = false;

    if let Some(email) = email.as_deref() {
        let known = accounts
            .iter()
            .any(|account| account.email.as_deref() == Some(email));
        if !known {
            if let (Ok(credential), Some(dir)) =
                (read_antigravity_credential(), antigravity_accounts_dir())
            {
                let id = format!("acc-{}", now_millis());
                let stored = std::fs::create_dir_all(dir.join(&id)).is_ok()
                    && std::fs::write(dir.join(&id).join("credential.json"), credential).is_ok();
                if stored {
                    accounts.push(AntigravityAccount {
                        id,
                        label: email.to_string(),
                        email: Some(email.to_string()),
                        tier,
                        saved_at: iso_now(),
                        is_active: true,
                    });
                    dirty = true;
                }
            }
        }

        for account in accounts.iter_mut() {
            let should = account.email.as_deref() == Some(email);
            if account.is_active != should {
                account.is_active = should;
                dirty = true;
            }
        }
    }

    if dirty {
        let _ = save_antigravity_accounts(&accounts);
    }

    accounts
}

async fn refresh_antigravity() -> Result<MonitorOutcome, AppError> {
    let log_candidates = antigravity_endpoint_candidates();
    if log_candidates.is_empty() {
        return Err(AppError::Network(
            "Antigravity 로그에서 로컬 API 주소를 찾을 수 없습니다 · Antigravity를 실행한 뒤 다시 조회하세요"
                .into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(LOCAL_TIMEOUT_MS))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;

    // The running language server rotates its csrf token on every restart and the
    // token is not always written to main.log, so read it from the process itself.
    let process_csrf = tauri::async_runtime::spawn_blocking(antigravity_process_csrf)
        .await
        .ok()
        .flatten();

    let ports: Vec<u16> = log_candidates
        .iter()
        .map(|(_, port, _)| *port)
        .collect();
    let (alive, rest): (Vec<u16>, Vec<u16>) = ports.into_iter().partition(|port| tcp_alive(*port));
    let ordered: Vec<u16> = alive.into_iter().chain(rest).take(4).collect();

    let mut endpoint: Option<(u16, String, Value)> = None;
    'outer: for port in ordered {
        let mut tokens: Vec<String> = Vec::new();
        if let Some(token) = process_csrf.as_ref() {
            tokens.push(token.clone());
        }
        if let Some((_, _, token)) = log_candidates.iter().find(|(_, known, _)| *known == port) {
            if !tokens.contains(token) {
                tokens.push(token.clone());
            }
        }
        for token in tokens {
            match antigravity_rpc(&client, "https://127.0.0.1", port, &token, ANTIGRAVITY_STATUS_RPC)
                .await
            {
                Ok(status) => {
                    endpoint = Some((port, token, status));
                    break 'outer;
                }
                Err(_) => continue,
            }
        }
    }

    let Some((port, csrf, status)) = endpoint else {
        return Err(AppError::Network(
            "Antigravity가 실행 중이 아니거나 로컬 API가 응답하지 않습니다 · Antigravity를 실행한 뒤 다시 조회하세요"
                .into(),
        ));
    };

    let quota = antigravity_rpc(
        &client,
        "https://127.0.0.1",
        port,
        &csrf,
        ANTIGRAVITY_QUOTA_RPC,
    )
    .await
    .ok();
    let groups = quota.as_ref().map(parse_antigravity_quota_groups);

    let mut outcome = parse_antigravity_status(&status, groups.as_ref());
    if let Some(details) = outcome.details.as_mut() {
        let accounts = remember_antigravity_account(&status);
        if let Some(object) = details.as_object_mut() {
            object.insert(
                "savedAccounts".into(),
                serde_json::to_value(accounts).unwrap_or(json!([])),
            );
        }
    }
    Ok(outcome)
}

#[tauri::command]
pub fn monitor_antigravity_accounts() -> Vec<AntigravityAccount> {
    load_antigravity_accounts()
}

#[tauri::command]
pub async fn monitor_antigravity_save_current(
    label: String,
) -> Result<AntigravityAccount, AppError> {
    let credential = read_antigravity_credential()?;
    let mut email = None;
    let mut tier = None;
    if let Some((base, port, csrf)) = find_antigravity_endpoint() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(LOCAL_TIMEOUT_MS))
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| AppError::Network(e.to_string()))?;
        if let Ok(status) = antigravity_rpc(&client, &base, port, &csrf, ANTIGRAVITY_STATUS_RPC).await
        {
            email = status
                .get("userStatus")
                .and_then(|value| value.get("email"))
                .and_then(Value::as_str)
                .map(str::to_string);
            tier = status
                .get("userStatus")
                .and_then(|value| value.get("userTier"))
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }

    let dir = antigravity_accounts_dir()
        .ok_or_else(|| AppError::Io("계정 폴더를 만들 수 없습니다".into()))?;
    let id = format!("acc-{}", now_millis());
    std::fs::create_dir_all(dir.join(&id))?;
    std::fs::write(dir.join(&id).join("credential.json"), credential)?;

    let mut accounts = load_antigravity_accounts();
    accounts.retain(|account| {
        !(email.is_some() && account.email == email) && account.label != label
    });
    let account = AntigravityAccount {
        id,
        label: if label.trim().is_empty() {
            email.clone().unwrap_or_else(|| "Antigravity 계정".into())
        } else {
            label
        },
        email,
        tier,
        saved_at: iso_now(),
        is_active: true,
    };
    accounts.push(account.clone());
    save_antigravity_accounts(&accounts)?;
    Ok(account)
}

#[tauri::command]
pub fn monitor_antigravity_switch_account(id: String) -> Result<(), AppError> {
    let dir = antigravity_accounts_dir()
        .ok_or_else(|| AppError::Io("계정 폴더를 만들 수 없습니다".into()))?;
    let credential = std::fs::read_to_string(dir.join(&id).join("credential.json"))
        .map_err(|_| AppError::Io("저장된 계정 정보를 찾을 수 없습니다".into()))?;
    run_powershell(&cred_write_script(&credential))?;

    let mut accounts = load_antigravity_accounts();
    for account in accounts.iter_mut() {
        account.is_active = account.id == id;
    }
    save_antigravity_accounts(&accounts)?;
    Ok(())
}

#[tauri::command]
pub fn monitor_antigravity_delete_account(id: String) -> Result<(), AppError> {
    let dir = antigravity_accounts_dir()
        .ok_or_else(|| AppError::Io("계정 폴더를 만들 수 없습니다".into()))?;
    let _ = std::fs::remove_dir_all(dir.join(&id));
    let mut accounts = load_antigravity_accounts();
    accounts.retain(|account| account.id != id);
    save_antigravity_accounts(&accounts)?;
    Ok(())
}

fn iso_now() -> String {
    crate::usage::epoch_to_rfc3339(now_millis() / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_antigravity_csrf_with_ports_newest_first() {
        let log = "\
[info] Host bridge server listening on http://127.0.0.1:9000
Spawning: language_server.exe --csrf_token AAA-111
[info] [Auto-Restart] Port changed! Reloading all windows with URL: https://127.0.0.1:9001/
[info]   Local:       https://127.0.0.1:9001/
Spawning: language_server.exe --csrf_token BBB-222
[info] [Auto-Restart] Port changed! Reloading all windows with URL: https://127.0.0.1:9002/
[info]   Local:       https://127.0.0.1:9002/
";
        let candidates = parse_antigravity_endpoints(log);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].1, 9002);
        assert_eq!(candidates[0].2, "BBB-222");
        assert_eq!(candidates[1].1, 9001);
        assert_eq!(candidates[1].2, "AAA-111");
    }

    #[test]
    fn parses_codex_epoch_reset_at() {
        let now = now_millis() / 1000;
        let body = json!({
            "plan_type": "team",
            "rate_limit": {
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 0,
                    "reset_after_seconds": 604800,
                    "reset_at": now + 3600
                },
                "secondary_window": null
            }
        });
        let outcome = parse_codex_usage(&body, None, None);
        assert!(outcome.summary.contains("주간 100% 남음"), "summary: {}", outcome.summary);
        assert!(outcome.summary.contains("리셋 1h 0m"), "summary: {}", outcome.summary);
        let details = outcome.details.unwrap();
        let reset = details["primaryResetAt"].as_str().unwrap();
        assert!(reset.ends_with('Z') && reset.contains('T'), "reset: {reset}");
        assert!(details["secondaryUsedPercent"].is_null());
    }

    #[test]
    fn parses_codex_usage_windows() {
        let body = json!({
            "plan_type": "plus",
            "email": "user@example.com",
            "rate_limit": {
                "limit_reached": false,
                "primary_window": { "used_percent": 38.0 },
                "secondary_window": { "used_percent": 12.5 }
            }
        });
        let outcome = parse_codex_usage(&body, None, None);
        assert!(outcome.summary.contains("plus"));
        assert!(outcome.summary.contains("주간 62% 남음"));
        assert!(outcome.summary.contains("5시간 87.5% 남음"));
    }

    #[test]
    fn parses_grok_billing_usage() {
        let billing = json!({ "config": { "monthlyLimit": { "val": 100.0 }, "used": { "val": 42.0 } } });
        let outcome = parse_grok_billing(&billing, Some("SuperGrok"));
        assert!(outcome.summary.contains("사용 42/100 (42%)"));
    }

    #[test]
    fn parses_grok_tier_name() {
        let subs = json!({ "subscriptions": [ { "status": "SUBSCRIPTION_STATUS_ACTIVE", "tier": "SUBSCRIPTION_TIER_SUPER_GROK" } ] });
        assert_eq!(parse_grok_tier(&subs), Some("Super Grok".into()));
    }

    #[test]
    fn parses_antigravity_quota_windows() {
        let quota = json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.73 },
                            { "bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.55 }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            { "bucketId": "claude-weekly", "window": "weekly", "remainingFraction": 0.91 },
                            { "bucketId": "claude-5h", "window": "5h", "remainingFraction": 0.88 }
                        ]
                    }
                ]
            }
        });
        let groups = parse_antigravity_quota_groups(&quota);
        let status = json!({
            "userStatus": {
                "email": "user@example.com",
                "userTier": { "name": "Google AI Pro" },
                "cascadeModelConfigData": { "clientModelConfigs": [] }
            }
        });
        let outcome = parse_antigravity_status(&status, Some(&groups));
        assert!(outcome.summary.contains("Gemini 주간 73% · 5시간 55%"));
        assert!(outcome.summary.contains("Claude 주간 91% · 5시간 88%"));
        let details = outcome.details.unwrap();
        assert_eq!(details["quotaGroups"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn buckets_opencode_sessions_by_window() {
        let now = 1_000_000_000_000_i64;
        let sessions = vec![
            (now - 60 * 60 * 1000, 0.5),          // 1시간 전 → 5시간/24시간/7일/30일 포함
            (now - 6 * 60 * 60 * 1000, 1.5),      // 6시간 전 → 24시간/7일/30일
            (now - 3 * 24 * 60 * 60 * 1000, 2.0), // 3일 전 → 7일/30일
            (now - 40 * 24 * 60 * 60 * 1000, 9.0),// 40일 전 → 제외
        ];
        let windows = bucket_sessions(&sessions, now);
        assert_eq!(windows["fiveHour"]["sessions"], 1);
        assert_eq!(windows["daily"]["sessions"], 2);
        assert_eq!(windows["weekly"]["sessions"], 3);
        assert_eq!(windows["monthly"]["sessions"], 3);
        assert_eq!(windows["fiveHour"]["cost"], 0.5);
        assert_eq!(windows["weekly"]["cost"], 4.0);
    }

    #[test]
    fn parses_antigravity_status_models() {
        let body = json!({
            "userStatus": {
                "email": "user@example.com",
                "userTier": { "name": "Google AI Pro" },
                "cascadeModelConfigData": {
                    "clientModelConfigs": [
                        { "label": "Gemini 3.1 Pro", "quotaInfo": { "remainingFraction": 0.82 } },
                        { "label": "Gemini 3.7 Flash", "quotaInfo": { "remainingFraction": 0.64 } },
                        { "label": "Claude Sonnet 4.6", "quotaInfo": { "remainingFraction": 0.91 } }
                    ]
                }
            }
        });
        let outcome = parse_antigravity_status(&body, None);
        assert!(outcome.summary.contains("Gemini 64% 남음"));
        assert!(outcome.summary.contains("Claude 91% 남음"));
    }

    #[test]
    fn decodes_jwt_payload_without_padding() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"a@b.c"}"#);
        let token = format!("x.{payload}.y");
        let decoded = decode_jwt_payload(&token).unwrap();
        assert_eq!(decoded.get("email").and_then(Value::as_str), Some("a@b.c"));
    }

    #[test]
    fn parses_opencode_go_usage_percentages() {
        let body = json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 10, "resetsAt": "2026-09-19T17:18:23.603Z" },
                "weekly": { "status": "ok", "percent": 8, "resetsAt": "2026-09-21T00:00:00.000Z" },
                "monthly": { "status": "ok", "percent": 54, "resetsAt": "2026-10-10T13:15:30.000Z" }
            }
        });
        let (summary, details, remaining) = parse_opencode_usage(&body).unwrap();
        assert!(summary.starts_with("OpenCode Go 롤링 10%"), "summary: {summary}");
        assert!(summary.contains("주간 8%"));
        assert!(summary.contains("월간 54%"));
        assert_eq!(details["monthly"]["percent"], json!(54.0));
        assert_eq!(remaining, 46.0);
        assert!(details["rolling"]["resetInSec"].is_number());
    }

    #[test]
    fn parses_rfc3339_and_reset_labels() {
        assert_eq!(parse_rfc3339_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_secs("2024-11-01T00:00:00Z"), Some(1_730_419_200));
        assert_eq!(format_reset_in(51 * 60), "51m");
        assert_eq!(format_reset_in(3 * 3_600 + 20 * 60), "3h 20m");
        assert_eq!(format_reset_in(30 * 3_600), "1d 6h");
    }

    #[test]
    fn formats_powershell_utf16_encoded_command() {
        let encoded = powershell_encoded("Write-Output 1");
        assert!(!encoded.is_empty());
        assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn probes_all_monitors() {
        let probes = monitor_probe();
        let names: Vec<&str> = probes.iter().map(|probe| probe.monitor.as_str()).collect();
        assert_eq!(
            names,
            vec!["codex", "grok", "grok-build", "antigravity", "opencode"]
        );
        assert!(probes.iter().all(|probe| !probe.label.is_empty()));
    }

    #[test]
    fn refreshes_grok_build_when_cli_present() {
        if grok_cli_path().is_none() {
            return;
        }
        let outcome = tauri::async_runtime::block_on(refresh_grok_build()).unwrap();
        eprintln!("grok-build status={} summary={}", outcome.status, outcome.summary);
        assert!(!outcome.summary.is_empty() || outcome.message.is_some());
    }

    #[test]
    fn parses_grok_build_billing_response() {
        let body = json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-09-19T13:08:24Z",
                    "end": "2026-09-26T13:08:24Z"
                },
                "onDemandCap": { "val": "0" },
                "onDemandUsed": { "val": "0" },
                "prepaidBalance": { "val": "2500" },
                "isUnifiedBillingUser": true,
                "creditUsagePercent": 37.5
            },
            "subscription_tier": "SuperGrok"
        });
        let outcome = parse_grok_build_billing(&body);
        assert_eq!(outcome.status, "ok");
        assert!(outcome.summary.contains("SuperGrok"));
        assert!(outcome.summary.contains("주간 62.5% 남음"), "summary: {}", outcome.summary);
        assert!(outcome.summary.contains("선불 $25"));
        let details = outcome.details.unwrap();
        assert_eq!(details["remainingPercent"], json!(62.5));
        assert_eq!(details["periodLabel"], json!("주간"));
    }

    #[test]
    fn refreshes_opencode_from_local_db_when_present() {
        if opencode_db_path().is_none() {
            return;
        }
        let outcome = tauri::async_runtime::block_on(refresh_opencode()).unwrap();
        assert_eq!(outcome.status, "ok");
        assert!(
            outcome.summary.contains("OpenCode"),
            "summary: {}",
            outcome.summary
        );
        let details = outcome.details.expect("details");
        assert!(
            details.get("rolling").is_some() || details.get("localWindows").is_some(),
            "details: {details}"
        );
        if opencode_go_key().is_some() && details.get("rolling").is_some() {
            assert!(
                outcome.summary.contains("롤링") && outcome.summary.contains("%"),
                "summary: {}",
                outcome.summary
            );
        }
    }

    #[test]
    fn reads_opencode_db_copy_when_available() {
        let Some(db) = opencode_db_path() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("api-desk-opencode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("opencode.db");
        if std::fs::copy(&db, &target).is_err() {
            return;
        }
        for suffix in ["-wal", "-shm"] {
            let source = PathBuf::from(format!("{}{}", db.to_string_lossy(), suffix));
            if source.exists() {
                let _ = std::fs::copy(
                    &source,
                    PathBuf::from(format!("{}{}", target.to_string_lossy(), suffix)),
                );
            }
        }
        let result = tauri::async_runtime::block_on(query_opencode_db(&target));
        match result {
            Ok((windows, models, _last)) => {
                assert!(windows.get("daily").is_some());
                assert!(models.is_array());
            }
            Err(error) => panic!("opencode DB 쿼리 실패: {error}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
