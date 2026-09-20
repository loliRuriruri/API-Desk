use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::AppError;
use crate::vault::{self, VaultState};

const MAX_BODY_SNIPPET: usize = 160;
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

#[derive(Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSpec {
    pub header: String,
    #[serde(default)]
    pub prefix: Option<String>,
    pub secret_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub auth: Option<AuthSpec>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn default_method() -> String {
    "GET".into()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestOutcome {
    pub status: String,
    pub http_status: Option<u16>,
    pub message: String,
    pub latency_ms: u64,
}

pub fn classify_status(status: u16) -> &'static str {
    match status {
        200..=299 => "connected",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        405 => "unsupported",
        408 => "timeout",
        409 => "conflict",
        429 => "rate_limited",
        400..=499 => "bad_request",
        500..=599 => "server_error",
        _ => "unsupported",
    }
}

pub fn redact(body: &str, secret: &str) -> String {
    let without_secret = if secret.is_empty() {
        body.to_string()
    } else {
        body.replace(secret, "[redacted]")
    };
    let collapsed: String = without_secret
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(MAX_BODY_SNIPPET).collect()
}

pub fn ensure_secret_not_in_url(url: &str, secret: &str) -> Result<(), AppError> {
    if !secret.is_empty() && url.contains(secret) {
        return Err(AppError::InvalidRequest(
            "API 키를 요청 URL에 넣을 수 없습니다".into(),
        ));
    }
    Ok(())
}

fn status_message(status: u16, status_text: &str) -> String {
    match status {
        401 | 403 => "프로바이더가 이 API 키를 거부했습니다".into(),
        404 => "테스트 엔드포인트를 찾을 수 없습니다. 프로바이더 설정의 Base URL을 확인하세요.".into(),
        429 => "프로바이더가 호출을 제한했습니다. 키는 유효하지만 현재 제한된 상태입니다.".into(),
        500..=599 => "프로바이더가 서버 오류를 반환했습니다. 키는 아직 유효할 수 있습니다.".into(),
        _ => format!("HTTP {status} {status_text}"),
    }
}

#[tauri::command]
pub async fn test_http(
    state: State<'_, VaultState>,
    request: TestRequest,
) -> Result<TestOutcome, AppError> {
    let url = request.url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::InvalidRequest("테스트 URL이 비어 있습니다".into()));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::InvalidRequest(
            "테스트 URL은 http:// 또는 https://로 시작해야 합니다".into(),
        ));
    }

    let mut secret = if let Some(auth) = &request.auth {
        vault::read_secret_string(&state, &auth.secret_id)
            .map_err(|_| AppError::SecretNotFound)?
            .to_string()
    } else {
        String::new()
    };

    let result = async {
        ensure_secret_not_in_url(&url, &secret)?;

        let timeout = Duration::from_millis(
            request
                .timeout_ms
                .unwrap_or(DEFAULT_TIMEOUT_MS)
                .clamp(1_000, 60_000),
        );
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent("API-Desk/0.1 (local)")
            .build()
            .map_err(|e| AppError::Network(e.to_string()))?;

        let method = reqwest::Method::from_bytes(request.method.to_uppercase().as_bytes())
            .map_err(|_| AppError::InvalidRequest(format!("지원되지 않는 메서드: {}", request.method)))?;
        let mut builder = client.request(method, &url);
        for header in &request.headers {
            builder = builder.header(header.name.as_str(), header.value.as_str());
        }
        if let Some(auth) = &request.auth {
            let value = match &auth.prefix {
                Some(prefix) if !prefix.is_empty() => format!("{prefix}{secret}"),
                _ => secret.clone(),
            };
            builder = builder.header(auth.header.as_str(), value);
        }

        let started = Instant::now();
        let response = match builder.send().await {
            Ok(response) => response,
            Err(error) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let status = if error.is_timeout() { "timeout" } else { "network_error" };
                let message = if error.is_timeout() {
                    format!("{}ms 후 요청 시간이 초과되었습니다", timeout.as_millis())
                } else if error.is_connect() {
                    "프로바이더에 연결하지 못했습니다. 네트워크 연결과 Base URL을 확인하세요."
                        .to_string()
                } else {
                    "네트워크 요청이 실패했습니다".to_string()
                };
                return Ok(TestOutcome {
                    status: status.into(),
                    http_status: None,
                    message,
                    latency_ms,
                });
            }
        };

        let http_status = response.status().as_u16();
        let status_text = response
            .status()
            .canonical_reason()
            .unwrap_or_default()
            .to_string();
        let body = response
            .bytes()
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .unwrap_or_default();
        let latency_ms = started.elapsed().as_millis() as u64;
        let status = classify_status(http_status);
        let mut message = status_message(http_status, &status_text);
        if http_status >= 400 {
            let snippet = redact(&body, &secret);
            if !snippet.is_empty() {
                message = format!("{message} · {snippet}");
            }
        }

        Ok(TestOutcome {
            status: status.into(),
            http_status: Some(http_status),
            message,
            latency_ms,
        })
    }
    .await;

    secret.zeroize_value();
    result
}

trait ZeroizeValue {
    fn zeroize_value(&mut self);
}

impl ZeroizeValue for String {
    fn zeroize_value(&mut self) {
        use zeroize::Zeroize;
        self.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_http_statuses() {
        assert_eq!(classify_status(200), "connected");
        assert_eq!(classify_status(204), "connected");
        assert_eq!(classify_status(401), "unauthorized");
        assert_eq!(classify_status(403), "forbidden");
        assert_eq!(classify_status(404), "not_found");
        assert_eq!(classify_status(429), "rate_limited");
        assert_eq!(classify_status(500), "server_error");
        assert_eq!(classify_status(302), "unsupported");
    }

    #[test]
    fn redacts_secret_from_body_snippets() {
        let body = "error: invalid key test-secret-not-a-real-api-key provided";
        let redacted = redact(body, "test-secret-not-a-real-api-key");
        assert!(!redacted.contains("test-secret-not-a-real-api-key"));
        assert!(redacted.contains("[redacted]"));
    }

    #[test]
    fn rejects_secret_in_url() {
        assert!(ensure_secret_not_in_url("https://example.com/v1/models?key=abc123", "abc123").is_err());
        assert!(ensure_secret_not_in_url("https://example.com/v1/models", "abc123").is_ok());
    }
}
