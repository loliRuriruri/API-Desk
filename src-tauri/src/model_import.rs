use serde::Serialize;
use tauri::State;

use crate::error::AppError;
use crate::vault::{self, VaultState};

const TIMEOUT_MS: u64 = 15_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedModel {
    pub id: String,
    pub display_name: String,
    pub owned_by: Option<String>,
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn parse_openai_list(body: &serde_json::Value) -> Vec<ImportedModel> {
    body.get("data")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id = item.get("id").and_then(serde_json::Value::as_str)?;
                    let name = item
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(id);
                    Some(ImportedModel {
                        id: id.to_string(),
                        display_name: name.to_string(),
                        owned_by: item
                            .get("owned_by")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_google_list(body: &serde_json::Value) -> Vec<ImportedModel> {
    body.get("models")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let raw = item.get("name").and_then(serde_json::Value::as_str)?;
                    let id = raw.strip_prefix("models/").unwrap_or(raw).to_string();
                    let display = item
                        .get("displayName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    Some(ImportedModel {
                        id,
                        display_name: display,
                        owned_by: item
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
pub async fn fetch_models(
    state: State<'_, VaultState>,
    kind: String,
    base_url: Option<String>,
    secret_id: String,
) -> Result<Vec<ImportedModel>, AppError> {
    let secret = vault::read_secret_string(&state, &secret_id)
        .map_err(|_| AppError::SecretNotFound)?
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .user_agent("API-Desk/0.1 (local)")
        .build()
        .map_err(|e| AppError::Network(e.to_string()))?;

    let base = base_url.unwrap_or_default();
    let (url, header_name, header_value, parser): (
        String,
        &str,
        String,
        fn(&serde_json::Value) -> Vec<ImportedModel>,
    ) = match kind.as_str() {
        "anthropic" => {
            let base = if base.trim().is_empty() {
                "https://api.anthropic.com".to_string()
            } else {
                base
            };
            (
                join_url(&base, "/v1/models"),
                "x-api-key",
                secret.clone(),
                parse_openai_list,
            )
        }
        "google" => {
            let base = if base.trim().is_empty() {
                "https://generativelanguage.googleapis.com".to_string()
            } else {
                base
            };
            (
                join_url(&base, "/v1beta/models"),
                "x-goog-api-key",
                secret.clone(),
                parse_google_list,
            )
        }
        "openrouter" => {
            let base = if base.trim().is_empty() {
                "https://openrouter.ai/api/v1".to_string()
            } else {
                base
            };
            (
                join_url(&base, "/models"),
                "Authorization",
                format!("Bearer {secret}"),
                parse_openai_list,
            )
        }
        _ => {
            if base.trim().is_empty() {
                return Err(AppError::InvalidRequest(
                    "이 프로바이더에는 Base URL이 필요합니다".into(),
                ));
            }
            (
                join_url(&base, "/models"),
                "Authorization",
                format!("Bearer {secret}"),
                parse_openai_list,
            )
        }
    };

    if url.contains(&secret) {
        return Err(AppError::InvalidRequest(
            "API 키를 요청 URL에 넣을 수 없습니다".into(),
        ));
    }

    let mut request = client.get(&url).header(header_name, header_value);
    if kind == "anthropic" {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request
        .send()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(AppError::Network(format!(
            "모델 목록 조회 실패 (HTTP {status})"
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Network(e.to_string()))?;
    let mut models = parser(&body);
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_style_model_list() {
        let body = serde_json::json!({
            "data": [
                { "id": "gpt-4o-mini", "owned_by": "openai" },
                { "id": "gpt-4o", "owned_by": "openai" }
            ]
        });
        let models = parse_openai_list(&body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o-mini");
        assert_eq!(models[0].display_name, "gpt-4o-mini");
        assert_eq!(models[0].owned_by.as_deref(), Some("openai"));
    }

    #[test]
    fn parses_google_style_model_list() {
        let body = serde_json::json!({
            "models": [
                { "name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro", "version": "001" }
            ]
        });
        let models = parse_google_list(&body);
        assert_eq!(models[0].id, "gemini-2.5-pro");
        assert_eq!(models[0].display_name, "Gemini 2.5 Pro");
    }

    #[test]
    fn tolerates_unexpected_shapes() {
        assert!(parse_openai_list(&serde_json::json!({})).is_empty());
        assert!(parse_google_list(&serde_json::json!({ "models": "nope" })).is_empty());
    }
}
