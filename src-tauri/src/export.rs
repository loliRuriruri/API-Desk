use serde::{Deserialize, Serialize};
use tauri::State;
use zeroize::Zeroize;

use crate::error::AppError;
use crate::vault::{self, VaultState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    pub provider: String,
    pub account: String,
    pub name: String,
    pub field: String,
    pub env_name: Option<String>,
    pub status: String,
    pub last_test_status: Option<String>,
    pub last_tested_at: Option<String>,
    pub expires_at: Option<String>,
    pub project_usage: Option<String>,
    pub notes: Option<String>,
    pub secret_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub file_path: String,
    pub exported: usize,
    pub missing: usize,
    pub format: String,
}

pub fn csv_field(value: &str) -> String {
    if value.contains(',')
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn format_csv(rows: &[(ExportEntry, Option<String>)]) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(
        "provider,account,credential,field,env_name,status,last_test_status,last_tested_at,expires_at,projects,notes,api_key\r\n",
    );
    for (entry, secret) in rows {
        let fields = [
            entry.provider.clone(),
            entry.account.clone(),
            entry.name.clone(),
            entry.field.clone(),
            entry.env_name.clone().unwrap_or_default(),
            entry.status.clone(),
            entry.last_test_status.clone().unwrap_or_default(),
            entry.last_tested_at.clone().unwrap_or_default(),
            entry.expires_at.clone().unwrap_or_default(),
            entry.project_usage.clone().unwrap_or_default(),
            entry.notes.clone().unwrap_or_default(),
            secret.clone().unwrap_or_default(),
        ];
        let line = fields
            .iter()
            .map(|field| csv_field(field))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

pub fn format_txt(rows: &[(ExportEntry, Option<String>)]) -> String {
    let mut out = String::new();
    for (index, (entry, secret)) in rows.iter().enumerate() {
        out.push_str(&format!("[{}] {}\n", index + 1, entry.name));
        out.push_str(&format!("프로바이더    : {}\n", entry.provider));
        out.push_str(&format!("계정          : {}\n", entry.account));
        out.push_str(&format!("필드          : {}\n", entry.field));
        out.push_str(&format!(
            "환경변수      : {}\n",
            entry.env_name.clone().unwrap_or_else(|| "-".into())
        ));
        out.push_str(&format!("상태          : {}\n", entry.status));
        out.push_str(&format!(
            "최근 테스트   : {}\n",
            entry
                .last_test_status
                .clone()
                .unwrap_or_else(|| "미테스트".into())
        ));
        out.push_str(&format!(
            "만료          : {}\n",
            entry.expires_at.clone().unwrap_or_else(|| "-".into())
        ));
        out.push_str(&format!(
            "사용 프로젝트 : {}\n",
            entry.project_usage.clone().unwrap_or_else(|| "-".into())
        ));
        if let Some(notes) = &entry.notes {
            if !notes.trim().is_empty() {
                out.push_str(&format!("메모          : {notes}\n"));
            }
        }
        match secret {
            Some(value) => out.push_str(&format!("API KEY       : {value}\n")),
            None => out.push_str("API KEY       : (Vault에 없음)\n"),
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("(내보낼 인증 정보가 없습니다)\n");
    }
    out
}

#[tauri::command]
pub fn export_credentials(
    state: State<'_, VaultState>,
    entries: Vec<ExportEntry>,
    path: String,
    format: String,
) -> Result<ExportResult, AppError> {
    if path.trim().is_empty() {
        return Err(AppError::InvalidPath("경로가 비어 있습니다".into()));
    }
    let mut rows: Vec<(ExportEntry, Option<String>)> = Vec::new();
    let mut missing = 0usize;
    for entry in entries {
        let secret = vault::read_secret_string(&state, &entry.secret_id)
            .ok()
            .map(|value| value.to_string());
        if secret.is_none() {
            missing += 1;
        }
        rows.push((entry, secret));
    }

    let content = if format == "csv" {
        format_csv(&rows)
    } else {
        format_txt(&rows)
    };
    std::fs::write(&path, content)?;

    for (_, secret) in rows.iter_mut() {
        if let Some(value) = secret {
            value.zeroize();
        }
    }

    Ok(ExportResult {
        file_path: path,
        exported: rows.len(),
        missing,
        format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExportEntry {
        ExportEntry {
            provider: "OpenAI".into(),
            account: "Personal".into(),
            name: "Main Key".into(),
            field: "API Key".into(),
            env_name: Some("OPENAI_API_KEY".into()),
            status: "active".into(),
            last_test_status: Some("connected".into()),
            last_tested_at: Some("2026-09-19T10:00:00Z".into()),
            expires_at: None,
            project_usage: Some("MikuChat (코딩 에이전트)".into()),
            notes: Some("월 $20".into()),
            secret_id: "secret-1".into(),
        }
    }

    #[test]
    fn csv_escapes_special_characters() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("has,comma"), "\"has,comma\"");
        assert_eq!(csv_field("has\"quote"), "\"has\"\"quote\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn csv_contains_bom_header_and_secret() {
        let rows = vec![(sample(), Some("test-secret-not-a-real-key".into()))];
        let csv = format_csv(&rows);
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("provider,account,credential,field"));
        assert!(csv.contains("test-secret-not-a-real-key"));
        assert!(csv.contains("\"MikuChat (코딩 에이전트)\"") || csv.contains("MikuChat (코딩 에이전트)"));
        assert!(csv.ends_with("\r\n"));
    }

    #[test]
    fn txt_marks_missing_secrets() {
        let rows = vec![
            (sample(), Some("test-secret-not-a-real-key".into())),
            (ExportEntry { name: "Gone".into(), ..sample() }, None),
        ];
        let txt = format_txt(&rows);
        assert!(txt.contains("test-secret-not-a-real-key"));
        assert!(txt.contains("(Vault에 없음)"));
        assert!(txt.contains("사용 프로젝트 : MikuChat (코딩 에이전트)"));
        assert!(txt.contains("[2] Gone"));
    }
}
