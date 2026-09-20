use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::State;

use crate::error::AppError;
use crate::vault::{self, VaultState};

const DEFAULT_TIMEOUT_MS: u64 = 15_000;
const MAX_BODY: usize = 8_192;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageOutcome {
    pub adapter: String,
    pub status: String,
    pub summary: String,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub currency: Option<String>,
    pub details: Option<Value>,
    pub message: Option<String>,
}

fn money(value: f64) -> String {
    if (value.fract()).abs() < 0.005 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}


pub fn epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn epoch_to_rfc3339(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs = epoch_secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

pub fn parse_openrouter_key(body: &Value) -> Result<UsageOutcome, AppError> {
    let data = body
        .get("data")
        .ok_or_else(|| AppError::InvalidRequest("OpenRouter 응답에 data가 없습니다".into()))?;
    let used = data.get("usage").and_then(Value::as_f64);
    let limit = data.get("limit").and_then(Value::as_f64);
    let remaining = data.get("limit_remaining").and_then(Value::as_f64);
    let free_tier = data
        .get("is_free_tier")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let summary = match (used, limit, remaining) {
        (Some(used), Some(limit), Some(remaining)) => format!(
            "사용 ${} · 한도 ${} · 남음 ${}",
            money(used),
            money(limit),
            money(remaining)
        ),
        (Some(used), _, Some(remaining)) => {
            format!("사용 ${} · 남음 ${}", money(used), money(remaining))
        }
        (Some(used), Some(limit), None) => {
            format!("사용 ${} · 한도 ${}", money(used), money(limit))
        }
        (Some(used), _, _) => format!("사용 ${}", money(used)),
        _ => "사용 정보 없음".to_string(),
    };
    Ok(UsageOutcome {
        adapter: "openrouter".into(),
        status: "ok".into(),
        summary: if free_tier {
            format!("{summary} · 무료 티어")
        } else {
            summary
        },
        used,
        limit,
        remaining,
        currency: Some("USD".into()),
        details: Some(body.clone()),
        message: None,
    })
}

pub fn parse_deepseek_balance(body: &Value) -> Result<UsageOutcome, AppError> {
    let available = body
        .get("is_available")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let first = body
        .get("balance_infos")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .ok_or_else(|| AppError::InvalidRequest("DeepSeek 잔액 정보가 없습니다".into()))?;
    let currency = first
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("CNY")
        .to_string();
    let total = first
        .get("total_balance")
        .and_then(|value| match value {
            Value::String(text) => text.parse::<f64>().ok(),
            other => other.as_f64(),
        })
        .unwrap_or(0.0);
    let symbol = if currency == "CNY" { "¥" } else { "" };
    Ok(UsageOutcome {
        adapter: "deepseek".into(),
        status: "ok".into(),
        summary: format!(
            "잔액 {}{} ({})",
            symbol,
            money(total),
            if available { "사용 가능" } else { "잔액 부족" }
        ),
        used: None,
        limit: None,
        remaining: Some(total),
        currency: Some(currency),
        details: Some(body.clone()),
        message: None,
    })
}

pub fn parse_tavily_usage(body: &Value) -> Result<UsageOutcome, AppError> {
    let key_usage = body
        .get("key")
        .and_then(|key| key.get("usage"))
        .and_then(Value::as_f64);
    let key_limit = body
        .get("key")
        .and_then(|key| key.get("limit"))
        .and_then(Value::as_f64);
    let account_usage = body
        .get("account")
        .and_then(|account| account.get("plan_usage"))
        .and_then(Value::as_f64);
    let account_limit = body
        .get("account")
        .and_then(|account| account.get("plan_limit"))
        .and_then(Value::as_f64);

    let (used, limit) = match (key_usage, key_limit) {
        (Some(used), Some(limit)) if limit > 0.0 => (used, limit),
        _ => match (account_usage, account_limit) {
            (Some(used), Some(limit)) if limit > 0.0 => (used, limit),
            _ => (
                key_usage.or(account_usage).unwrap_or(0.0),
                0.0,
            ),
        },
    };
    let summary = if limit > 0.0 {
        let remaining = (limit - used).max(0.0);
        format!(
            "사용 {} · 한도 {} · 남음 {}",
            money(used),
            money(limit),
            money(remaining)
        )
    } else {
        format!("사용 {} · 한도 정보 없음", money(used))
    };
    Ok(UsageOutcome {
        adapter: "tavily".into(),
        status: "ok".into(),
        summary,
        used: Some(used),
        limit: if limit > 0.0 { Some(limit) } else { None },
        remaining: if limit > 0.0 {
            Some((limit - used).max(0.0))
        } else {
            None
        },
        currency: None,
        details: Some(body.clone()),
        message: None,
    })
}

pub fn parse_openrouter_credits(body: &Value) -> Option<(f64, f64, f64)> {
    let data = body.get("data")?;
    let total = data.get("total_credits").and_then(Value::as_f64)?;
    let used = data
        .get("total_usage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some((total, used, (total - used).max(0.0)))
}

pub fn merge_openrouter_credits(
    key: UsageOutcome,
    credits: Option<(f64, f64, f64)>,
) -> UsageOutcome {
    let Some((total, used, available)) = credits else {
        return key;
    };
    let percent = if total > 0.0 {
        (available / total * 100.0 * 10.0).round() / 10.0
    } else {
        0.0
    };
    let key_note = match (key.remaining, key.limit) {
        (Some(remaining), Some(limit)) => format!(
            " · 키 한도 ${} (남음 ${})",
            money(limit),
            money(remaining)
        ),
        (Some(remaining), None) => format!(" · 키 남음 ${}", money(remaining)),
        _ => String::new(),
    };
    let free_note = if key.summary.contains("무료 티어") {
        " · 무료 티어"
    } else {
        ""
    };
    UsageOutcome {
        adapter: "openrouter".into(),
        status: "ok".into(),
        summary: format!(
            "크레딧 ${} 남음 (충전 ${}, 사용 ${}){key_note}{free_note}",
            money(available),
            money(total),
            money(used)
        ),
        used: Some(used),
        limit: Some(total),
        remaining: Some(available),
        currency: Some("USD".into()),
        details: Some(json!({
            "creditTotal": total,
            "creditUsed": used,
            "creditAvailable": available,
            "creditRemainingPercent": percent,
            "key": key.details,
        })),
        message: None,
    }
}

pub fn parse_openai_costs(body: &Value) -> Result<UsageOutcome, AppError> {
    let buckets = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidRequest("OpenAI costs 응답에 data가 없습니다".into()))?;
    let mut total = 0.0_f64;
    let mut currency = "usd".to_string();
    for bucket in buckets {
        if let Some(results) = bucket.get("results").and_then(Value::as_array) {
            for result in results {
                if let Some(amount) = result.get("amount") {
                    total += amount.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                    if let Some(code) = amount.get("currency").and_then(Value::as_str) {
                        currency = code.to_string();
                    }
                }
            }
        }
    }
    let symbol = if currency.eq_ignore_ascii_case("usd") {
        "$"
    } else {
        ""
    };
    Ok(UsageOutcome {
        adapter: "openai".into(),
        status: "ok".into(),
        summary: format!("최근 30일 {symbol}{} (Admin 비용 리포트)", money(total)),
        used: Some(total),
        limit: None,
        remaining: None,
        currency: Some(currency.to_uppercase()),
        details: Some(body.clone()),
        message: None,
    })
}

pub fn parse_anthropic_cost_report(body: &Value) -> Result<UsageOutcome, AppError> {
    let buckets = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidRequest("Anthropic 비용 응답에 data가 없습니다".into()))?;
    let mut cents = 0.0_f64;
    for bucket in buckets {
        if let Some(results) = bucket.get("results").and_then(Value::as_array) {
            for result in results {
                if let Some(amount) = result.get("amount") {
                    let parsed = match amount {
                        Value::String(text) => text.parse::<f64>().ok(),
                        other => other.as_f64(),
                    };
                    cents += parsed.unwrap_or(0.0);
                }
            }
        }
    }
    let usd = cents / 100.0;
    Ok(UsageOutcome {
        adapter: "anthropic".into(),
        status: "ok".into(),
        summary: format!("최근 30일 ${} (Admin 비용 리포트)", money(usd)),
        used: Some(usd),
        limit: None,
        remaining: None,
        currency: Some("USD".into()),
        details: Some(body.clone()),
        message: None,
    })
}

pub fn parse_xai_balance(body: &Value, team_id: &str) -> Result<UsageOutcome, AppError> {
    let cents = body
        .get("total")
        .and_then(|total| total.get("val"))
        .and_then(|value| match value {
            Value::String(text) => text.parse::<f64>().ok(),
            other => other.as_f64(),
        })
        .ok_or_else(|| AppError::InvalidRequest("xAI 잔액 정보가 없습니다".into()))?;
    let usd = cents / 100.0;
    Ok(UsageOutcome {
        adapter: "xai".into(),
        status: "ok".into(),
        summary: format!("선불 크레딧 ${} (team {})", money(usd), team_id),
        used: None,
        limit: None,
        remaining: Some(usd),
        currency: Some("USD".into()),
        details: Some(body.clone()),
        message: None,
    })
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

async fn get_json(
    client: &reqwest::Client,
    url: &str,
    secret: &str,
    header: &str,
    prefix: Option<&str>,
) -> Result<Value, AppError> {
    if url.contains(secret) {
        return Err(AppError::InvalidRequest(
            "API 키를 요청 URL에 넣을 수 없습니다".into(),
        ));
    }
    let value = match prefix {
        Some(prefix) => format!("{prefix}{secret}"),
        None => secret.to_string(),
    };
    let response = client
        .get(url)
        .header(header, value)
        .send()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        let snippet: String = text.chars().take(200).collect();
        return Err(AppError::Network(format!(
            "HTTP {status} · {}",
            snippet.replace(secret, "[redacted]")
        )));
    }
    let body = if text.len() > MAX_BODY {
        text.chars().take(MAX_BODY).collect::<String>()
    } else {
        text
    };
    serde_json::from_str(&body)
        .map_err(|e| AppError::InvalidRequest(format!("JSON 파싱 실패: {e}")))
}

struct RawResponse {
    status: u16,
    body: Option<Value>,
}

async fn request_json(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    secret: &str,
) -> Result<RawResponse, AppError> {
    if url.contains(secret) {
        return Err(AppError::InvalidRequest(
            "API 키를 요청 URL에 넣을 수 없습니다".into(),
        ));
    }
    let mut builder = client.get(url);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let response = builder
        .send()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Ok(RawResponse {
            status,
            body: None,
        });
    }
    let body = if text.len() > MAX_BODY {
        text.chars().take(MAX_BODY).collect::<String>()
    } else {
        text
    };
    let parsed = serde_json::from_str::<Value>(&body)
        .map_err(|e| AppError::InvalidRequest(format!("JSON 파싱 실패: {e}")))?;
    Ok(RawResponse {
        status,
        body: Some(parsed),
    })
}

fn auth_required(adapter: &str, hint: &str) -> UsageOutcome {
    UsageOutcome {
        adapter: adapter.into(),
        status: "auth_required".into(),
        summary: String::new(),
        used: None,
        limit: None,
        remaining: None,
        currency: None,
        details: None,
        message: Some(hint.into()),
    }
}

#[tauri::command]
pub async fn fetch_usage(
    state: State<'_, VaultState>,
    adapter: String,
    base_url: Option<String>,
    secret_id: String,
    extra_secret_id: Option<String>,
    timeout_ms: Option<u64>,
) -> Result<UsageOutcome, AppError> {
    let secret = vault::read_secret_string(&state, &secret_id)
        .map_err(|_| AppError::SecretNotFound)?
        .to_string();
    let extra_secret = extra_secret_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .and_then(|id| vault::read_secret_string(&state, id).ok())
        .map(|value| value.to_string());

    let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(2_000, 30_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("API-Desk/0.1 (local)")
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;

    let result = match adapter.as_str() {
        "openrouter" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".into());
            let key_outcome = {
                let body = get_json(
                    &client,
                    &join_url(&base, "/key"),
                    &secret,
                    "Authorization",
                    Some("Bearer "),
                )
                .await?;
                parse_openrouter_key(&body)?
            };
            let credits = get_json(
                &client,
                &join_url(&base, "/credits"),
                &secret,
                "Authorization",
                Some("Bearer "),
            )
            .await
            .ok()
            .and_then(|body| parse_openrouter_credits(&body));
            Ok(merge_openrouter_credits(key_outcome, credits))
        }
        "deepseek" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.deepseek.com".into());
            let body = get_json(
                &client,
                &join_url(&base, "/user/balance"),
                &secret,
                "Authorization",
                Some("Bearer "),
            )
            .await?;
            parse_deepseek_balance(&body)
        }
        "tavily" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.tavily.com".into());
            let body = get_json(
                &client,
                &join_url(&base, "/usage"),
                &secret,
                "Authorization",
                Some("Bearer "),
            )
            .await?;
            parse_tavily_usage(&body)
        }
        "openai" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            let start = epoch_secs() - 30 * 24 * 3600;
            let url = format!(
                "{}?start_time={}&limit=30&bucket_width=1d",
                join_url(&base, "/organization/costs"),
                start
            );
            let response = request_json(
                &client,
                &url,
                &[(
                    "Authorization".into(),
                    format!("Bearer {secret}"),
                )],
                &secret,
            )
            .await?;
            if response.status == 401 || response.status == 403 {
                Ok(auth_required(
                    "openai",
                    "Admin API 키가 필요합니다 (sk-admin-…). 일반 API 키로는 비용을 조회할 수 없습니다",
                ))
            } else if let Some(body) = response.body {
                parse_openai_costs(&body)
            } else {
                Ok(auth_required(
                    "openai",
                    &format!("OpenAI 비용 조회 실패 (HTTP {})", response.status),
                ))
            }
        }
        "anthropic" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.anthropic.com".into());
            let now = epoch_secs();
            let url = format!(
                "{}?starting_at={}&ending_at={}&bucket_width=1d",
                join_url(&base, "/v1/organizations/cost_report"),
                epoch_to_rfc3339(now - 30 * 24 * 3600),
                epoch_to_rfc3339(now)
            );
            let response = request_json(
                &client,
                &url,
                &[
                    ("anthropic-version".into(), "2023-06-01".into()),
                    ("x-api-key".into(), secret.clone()),
                ],
                &secret,
            )
            .await?;
            if response.status == 401 || response.status == 403 {
                Ok(auth_required(
                    "anthropic",
                    "Admin API 키가 필요합니다 (sk-ant-admin-…). 일반 API 키로는 비용을 조회할 수 없습니다",
                ))
            } else if let Some(body) = response.body {
                parse_anthropic_cost_report(&body)
            } else {
                Ok(auth_required(
                    "anthropic",
                    &format!("Anthropic 비용 조회 실패 (HTTP {})", response.status),
                ))
            }
        }
        "xai" => {
            let base = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://management-api.x.ai".into());
            let Some(team_id) = extra_secret
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(auth_required(
                    "xai",
                    "xAI 선불 크레딧 조회에는 Management Key와 Team ID 필드가 함께 필요합니다 (필드 추가: Management Key / Team ID)",
                ));
            };
            let url = join_url(
                &base,
                &format!("/v1/billing/teams/{team_id}/prepaid/balance"),
            );
            let response = request_json(
                &client,
                &url,
                &[("Authorization".into(), format!("Bearer {secret}"))],
                &secret,
            )
            .await?;
            if response.status == 401 || response.status == 403 {
                Ok(auth_required(
                    "xai",
                    "xAI Management Key가 필요합니다 (Management Keys 권한)",
                ))
            } else if let Some(body) = response.body {
                parse_xai_balance(&body, team_id)
            } else {
                Ok(auth_required(
                    "xai",
                    &format!("xAI 크레딧 조회 실패 (HTTP {})", response.status),
                ))
            }
        }
        other => Ok(UsageOutcome {
            adapter: other.to_string(),
            status: "unsupported".into(),
            summary: String::new(),
            used: None,
            limit: None,
            remaining: None,
            currency: None,
            details: None,
            message: Some("이 프로바이더는 사용량 조회를 지원하지 않습니다".into()),
        }),
    };

    let mut secret = secret;
    {
        use zeroize::Zeroize;
        secret.zeroize();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_openrouter_key_response() {
        let body = json!({
            "data": {
                "label": "sk-or-...",
                "usage": 1.5,
                "limit": 10.0,
                "limit_remaining": 8.5,
                "is_free_tier": false
            }
        });
        let outcome = parse_openrouter_key(&body).unwrap();
        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.summary, "사용 $1.50 · 한도 $10 · 남음 $8.50");
        assert_eq!(outcome.remaining, Some(8.5));
    }

    #[test]
    fn marks_openrouter_free_tier() {
        let body = json!({ "data": { "usage": 0.2, "is_free_tier": true } });
        let outcome = parse_openrouter_key(&body).unwrap();
        assert!(outcome.summary.contains("무료 티어"));
    }

    #[test]
    fn parses_deepseek_balance_response() {
        let body = json!({
            "is_available": true,
            "balance_infos": [
                { "currency": "CNY", "total_balance": "12.34" }
            ]
        });
        let outcome = parse_deepseek_balance(&body).unwrap();
        assert_eq!(outcome.summary, "잔액 ¥12.34 (사용 가능)");
        assert_eq!(outcome.remaining, Some(12.34));
    }

    #[test]
    fn parses_tavily_usage_response() {
        let body = json!({
            "key": { "usage": 120.0, "limit": 1000.0 },
            "account": { "plan_usage": 500.0, "plan_limit": 1000.0 }
        });
        let outcome = parse_tavily_usage(&body).unwrap();
        assert_eq!(outcome.summary, "사용 120 · 한도 1000 · 남음 880");
    }

    #[test]
    fn parses_openrouter_credits_and_merges() {
        let credits = json!({ "data": { "total_credits": 10.0, "total_usage": 0.97 } });
        let parsed = parse_openrouter_credits(&credits).unwrap();
        assert_eq!(parsed.0, 10.0);
        assert_eq!(parsed.1, 0.97);
        assert!((parsed.2 - 9.03).abs() < 0.0001);

        let key = parse_openrouter_key(&json!({
            "data": { "usage": 0.92, "limit": 1.0, "limit_remaining": 0.08, "is_free_tier": false }
        }))
        .unwrap();
        let merged = merge_openrouter_credits(key, Some(parsed));
        assert!(merged.summary.contains("크레딧 $9.03 남음"), "summary: {}", merged.summary);
        assert!(merged.summary.contains("충전 $10"));
        assert!(merged.summary.contains("사용 $0.97"));
        assert!(merged.summary.contains("키 한도 $1 (남음 $0.08)"));
        assert_eq!(merged.remaining, Some(9.03));
        assert_eq!(merged.limit, Some(10.0));
        let details = merged.details.unwrap();
        assert_eq!(details["creditRemainingPercent"], json!(90.3));
    }

    #[test]
    fn falls_back_to_key_summary_without_credits() {
        let key = parse_openrouter_key(&json!({
            "data": { "usage": 0.92, "limit": 1.0, "limit_remaining": 0.08 }
        }))
        .unwrap();
        let merged = merge_openrouter_credits(key, None);
        assert!(merged.summary.contains("남음 $0.08"));
    }

    #[test]
    fn parses_openai_costs_response() {
        let body = json!({
            "data": [
                { "results": [ { "amount": { "value": 0.06, "currency": "usd" } } ] },
                { "results": [ { "amount": { "value": 1.94, "currency": "usd" } } ] }
            ]
        });
        let outcome = parse_openai_costs(&body).unwrap();
        assert_eq!(outcome.summary, "최근 30일 $2 (Admin 비용 리포트)");
        assert_eq!(outcome.used, Some(2.0));
    }

    #[test]
    fn parses_anthropic_cost_report_cents() {
        let body = json!({
            "data": [
                { "results": [ { "amount": "1234.5" }, { "amount": "65.5" } ] }
            ]
        });
        let outcome = parse_anthropic_cost_report(&body).unwrap();
        assert_eq!(outcome.summary, "최근 30일 $13 (Admin 비용 리포트)");
        assert_eq!(outcome.used, Some(13.0));
    }

    #[test]
    fn parses_xai_prepaid_balance_cents() {
        let body = json!({ "total": { "val": "2500" }, "changes": [] });
        let outcome = parse_xai_balance(&body, "team-1").unwrap();
        assert_eq!(outcome.summary, "선불 크레딧 $25 (team team-1)");
        assert_eq!(outcome.remaining, Some(25.0));
    }

    #[test]
    fn formats_epoch_to_rfc3339() {
        assert_eq!(epoch_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(1_730_419_200), "2024-11-01T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn rejects_openrouter_response_without_data() {
        assert!(parse_openrouter_key(&json!({})).is_err());
    }
}
