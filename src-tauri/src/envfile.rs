use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::AppError;
use crate::vault::{self, VaultState};

#[derive(Debug, Clone, PartialEq)]
pub enum EnvLine {
    Assignment { name: String, value: String, raw: String },
    Other(String),
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_safe_value_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-./:@+,%$~".contains(c)
}

fn unescape_double(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some(other) => result.push(other),
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn escape_double(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            _ => result.push(c),
        }
    }
    result
}

pub fn parse_env(content: &str) -> Vec<EnvLine> {
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return EnvLine::Other(line.to_string());
            }
            let body = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim_start();
            let Some(eq) = body.find('=') else {
                return EnvLine::Other(line.to_string());
            };
            let name = body[..eq].trim();
            if name.is_empty() || !name.chars().all(is_name_char) {
                return EnvLine::Other(line.to_string());
            }
            let mut value = body[eq + 1..].trim().to_string();
            let bytes = value.as_bytes();
            if bytes.len() >= 2 {
                let first = bytes[0];
                let last = bytes[bytes.len() - 1];
                if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
                    let inner = &value[1..value.len() - 1];
                    value = if first == b'"' {
                        unescape_double(inner)
                    } else {
                        inner.to_string()
                    };
                }
            }
            EnvLine::Assignment {
                name: name.to_string(),
                value,
                raw: line.to_string(),
            }
        })
        .collect()
}

pub fn format_assignment(name: &str, value: &str) -> String {
    if !value.is_empty() && value.chars().all(is_safe_value_char) {
        format!("{name}={value}")
    } else {
        format!("{name}=\"{}\"", escape_double(value))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiffEntry {
    pub name: String,
    pub action: &'static str,
}

pub struct MergeOutcome {
    pub content: String,
    pub diff: Vec<DiffEntry>,
    pub changed: bool,
}

pub fn merge_env(existing: &str, updates: &[(String, Option<String>)]) -> MergeOutcome {
    let mut lines = parse_env(existing);
    let mut diff = Vec::new();
    let mut changed = false;

    for (name, value) in updates {
        let Some(value) = value else {
            diff.push(DiffEntry {
                name: name.clone(),
                action: "missing",
            });
            continue;
        };
        let index = lines.iter().position(|line| match line {
            EnvLine::Assignment { name: n, .. } => n.eq_ignore_ascii_case(name),
            EnvLine::Other(_) => false,
        });
        match index {
            Some(i) => {
                let current = match &lines[i] {
                    EnvLine::Assignment { value, .. } => value.clone(),
                    EnvLine::Other(_) => unreachable!(),
                };
                if current == *value {
                    diff.push(DiffEntry {
                        name: name.clone(),
                        action: "unchanged",
                    });
                } else {
                    lines[i] = EnvLine::Assignment {
                        name: name.clone(),
                        value: value.clone(),
                        raw: format_assignment(name, value),
                    };
                    changed = true;
                    diff.push(DiffEntry {
                        name: name.clone(),
                        action: "update",
                    });
                }
            }
            None => {
                lines.push(EnvLine::Assignment {
                    name: name.clone(),
                    value: value.clone(),
                    raw: format_assignment(name, value),
                });
                changed = true;
                diff.push(DiffEntry {
                    name: name.clone(),
                    action: "add",
                });
            }
        }
    }

    let mut content = String::new();
    for line in &lines {
        match line {
            EnvLine::Assignment { raw, .. } | EnvLine::Other(raw) => {
                content.push_str(raw);
                content.push('\n');
            }
        }
    }

    MergeOutcome {
        content,
        diff,
        changed,
    }
}

fn validate_project_dir(project_path: &str) -> Result<PathBuf, AppError> {
    if project_path.trim().is_empty() {
        return Err(AppError::InvalidPath("프로젝트 경로가 비어 있습니다".into()));
    }
    let path = Path::new(project_path);
    if !path.exists() {
        return Err(AppError::InvalidPath(format!("경로가 존재하지 않습니다: {project_path}")));
    }
    if !path.is_dir() {
        return Err(AppError::InvalidPath(format!("폴더가 아닙니다: {project_path}")));
    }
    path.canonicalize()
        .map_err(|e| AppError::InvalidPath(format!("{project_path}: {e}")))
}

fn validate_env_file_name(file_name: &str) -> Result<(), AppError> {
    if !file_name.starts_with(".env")
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(AppError::InvalidRequest(format!(
            "안전하지 않은 .env 파일 이름: {file_name}"
        )));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanFile {
    pub file_name: String,
    pub variables: Vec<String>,
}

#[tauri::command]
pub fn scan_project(project_path: String) -> Result<Vec<ScanFile>, AppError> {
    let root = validate_project_dir(&project_path)?;
    let mut files = Vec::new();
    let entries = std::fs::read_dir(&root)?;
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.starts_with(".env") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut variables: Vec<String> = Vec::new();
        for line in parse_env(&content) {
            if let EnvLine::Assignment { name, .. } = line {
                if !variables.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
                    variables.push(name);
                }
            }
        }
        files.push(ScanFile {
            file_name,
            variables,
        });
    }
    files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(files)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTarget {
    pub var_name: String,
    pub secret_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub var_name: String,
    pub found: bool,
    pub masked: String,
}

#[tauri::command]
pub fn import_env_values(
    state: State<'_, VaultState>,
    project_path: String,
    file_name: String,
    targets: Vec<ImportTarget>,
) -> Result<Vec<ImportResult>, AppError> {
    let root = validate_project_dir(&project_path)?;
    validate_env_file_name(&file_name)?;
    let content = std::fs::read_to_string(root.join(&file_name))?;
    let parsed = parse_env(&content);

    let mut results = Vec::new();
    for target in targets {
        let found = parsed.iter().find_map(|line| match line {
            EnvLine::Assignment { name, value, .. } if name.eq_ignore_ascii_case(&target.var_name) => {
                Some(value.clone())
            }
            _ => None,
        });
        match found {
            Some(value) if !value.trim().is_empty() => {
                let masked = vault::mask_secret(&value);
                vault::write_secret(&state, &target.secret_id, &value)?;
                results.push(ImportResult {
                    var_name: target.var_name,
                    found: true,
                    masked,
                });
            }
            _ => results.push(ImportResult {
                var_name: target.var_name,
                found: false,
                masked: String::new(),
            }),
        }
    }
    Ok(results)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvEntry {
    pub env_name: String,
    pub secret_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvPreview {
    pub file_name: String,
    pub file_path: String,
    pub existing: bool,
    pub changed: bool,
    pub lines: Vec<DiffEntry>,
}

fn build_updates(
    state: &VaultState,
    entries: &[EnvEntry],
) -> Result<Vec<(String, Option<String>)>, AppError> {
    let mut updates = Vec::new();
    for entry in entries {
        let value = if entry.secret_id.trim().is_empty() {
            None
        } else {
            vault::read_secret_string(state, &entry.secret_id)
                .ok()
                .map(|v| v.to_string())
        };
        let dup = updates
            .iter()
            .any(|(name, _): &(String, Option<String>)| name.eq_ignore_ascii_case(&entry.env_name));
        if !dup {
            updates.push((entry.env_name.clone(), value));
        }
    }
    Ok(updates)
}

pub fn preview_env(
    state: &VaultState,
    project_path: &str,
    entries: &[EnvEntry],
) -> Result<EnvPreview, AppError> {
    let root = validate_project_dir(project_path)?;
    let file_path = root.join(".env");
    let existing_content = if file_path.exists() {
        Some(std::fs::read_to_string(&file_path)?)
    } else {
        None
    };
    if existing_content.is_none() && entries.is_empty() {
        return Err(AppError::InvalidRequest(
            "이 프로젝트에 연결된 인증 정보가 없습니다".into(),
        ));
    }
    let updates = build_updates(state, entries)?;
    let outcome = merge_env(existing_content.as_deref().unwrap_or(""), &updates);
    Ok(EnvPreview {
        file_name: ".env".into(),
        file_path: file_path.to_string_lossy().to_string(),
        existing: existing_content.is_some(),
        changed: outcome.changed,
        lines: outcome.diff,
    })
}

#[tauri::command]
pub fn env_preview(
    state: State<'_, VaultState>,
    project_path: String,
    entries: Vec<EnvEntry>,
) -> Result<EnvPreview, AppError> {
    preview_env(&state, &project_path, &entries)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvApplyResult {
    pub file_path: String,
    pub existing: bool,
    pub backup_path: Option<String>,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub missing: usize,
    pub written: bool,
}

pub fn apply_env(
    state: &VaultState,
    project_path: &str,
    entries: &[EnvEntry],
    mode: &str,
) -> Result<EnvApplyResult, AppError> {
    let root = validate_project_dir(project_path)?;
    let file_path = root.join(".env");
    let existing_content = if file_path.exists() {
        Some(std::fs::read_to_string(&file_path)?)
    } else {
        None
    };
    let updates = build_updates(state, entries)?;
    let outcome = merge_env(existing_content.as_deref().unwrap_or(""), &updates);

    let count = |action: &str| outcome.diff.iter().filter(|d| d.action == action).count();
    let mut backup_path = None;
    let mut written = false;

    if outcome.changed {
        if existing_content.is_some() && mode == "backup" {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let backup = root.join(format!(".env.backup-{stamp}"));
            std::fs::copy(&file_path, &backup)?;
            backup_path = Some(backup.to_string_lossy().to_string());
        }
        std::fs::write(&file_path, outcome.content)?;
        written = true;
    }

    Ok(EnvApplyResult {
        file_path: file_path.to_string_lossy().to_string(),
        existing: existing_content.is_some(),
        backup_path,
        added: count("add"),
        updated: count("update"),
        unchanged: count("unchanged"),
        missing: count("missing"),
        written,
    })
}

#[tauri::command]
pub fn env_apply(
    state: State<'_, VaultState>,
    project_path: String,
    entries: Vec<EnvEntry>,
    mode: String,
) -> Result<EnvApplyResult, AppError> {
    apply_env(&state, &project_path, &entries, &mode)
}

#[tauri::command]
pub fn write_text_file(path: String, content: String) -> Result<(), AppError> {
    if path.trim().is_empty() {
        return Err(AppError::InvalidPath("경로가 비어 있습니다".into()));
    }
    let target = PathBuf::from(&path);
    if let Some(parent) = target.parent() {
        if !parent.exists() {
            return Err(AppError::InvalidPath(format!(
                "폴더가 존재하지 않습니다: {}",
                parent.display()
            )));
        }
    }
    std::fs::write(target, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_SECRET: &str = "test-secret-not-a-real-api-key";

    #[test]
    fn parses_env_lines() {
        let content = "# comment\nOPENAI_API_KEY=sk-test-123\n\nexport ANTHROPIC_API_KEY=\"quoted value\"\nBAD LINE\nX='single'\n";
        let parsed = parse_env(content);
        assert_eq!(
            parsed[1],
            EnvLine::Assignment {
                name: "OPENAI_API_KEY".into(),
                value: "sk-test-123".into(),
                raw: "OPENAI_API_KEY=sk-test-123".into()
            }
        );
        assert_eq!(
            parsed[3],
            EnvLine::Assignment {
                name: "ANTHROPIC_API_KEY".into(),
                value: "quoted value".into(),
                raw: "export ANTHROPIC_API_KEY=\"quoted value\"".into()
            }
        );
        assert!(matches!(parsed[2], EnvLine::Other(_)));
        assert!(matches!(parsed[4], EnvLine::Other(_)));
        assert_eq!(
            parsed[5],
            EnvLine::Assignment {
                name: "X".into(),
                value: "single".into(),
                raw: "X='single'".into()
            }
        );
    }

    #[test]
    fn merges_and_preserves_unknown_lines() {
        let existing = "# db settings\nDATABASE_URL=postgres://localhost\nOPENROUTER_API_KEY=old\n";
        let updates = vec![
            ("OPENROUTER_API_KEY".to_string(), Some(FAKE_SECRET.to_string())),
            ("OPENAI_API_KEY".to_string(), Some("second-fake-value".to_string())),
            ("MISSING".to_string(), None),
        ];
        let outcome = merge_env(existing, &updates);
        assert!(outcome.changed);
        assert!(outcome.content.contains("# db settings"));
        assert!(outcome.content.contains("DATABASE_URL=postgres://localhost"));
        assert!(outcome.content.contains(&format!("OPENROUTER_API_KEY={FAKE_SECRET}")));
        assert!(outcome.content.contains("OPENAI_API_KEY=second-fake-value"));
        assert!(outcome.content.contains("OPENROUTER_API_KEY=") && !outcome.content.contains("OPENROUTER_API_KEY=old"));
        assert_eq!(
            outcome.diff,
            vec![
                DiffEntry { name: "OPENROUTER_API_KEY".into(), action: "update" },
                DiffEntry { name: "OPENAI_API_KEY".into(), action: "add" },
                DiffEntry { name: "MISSING".into(), action: "missing" },
            ]
        );
    }

    #[test]
    fn merge_reports_unchanged_without_rewriting() {
        let existing = format!("OPENAI_API_KEY={FAKE_SECRET}\n");
        let updates = vec![("OPENAI_API_KEY".to_string(), Some(FAKE_SECRET.to_string()))];
        let outcome = merge_env(&existing, &updates);
        assert!(!outcome.changed);
        assert_eq!(outcome.content, existing);
        assert_eq!(outcome.diff[0].action, "unchanged");
    }

    #[test]
    fn quotes_values_that_need_it() {
        assert_eq!(format_assignment("KEY", "simple-value_1"), "KEY=simple-value_1");
        assert_eq!(format_assignment("KEY", "has space"), "KEY=\"has space\"");
        assert_eq!(format_assignment("KEY", "quote\"inside"), "KEY=\"quote\\\"inside\"");
        assert_eq!(format_assignment("KEY", ""), "KEY=\"\"");
    }

    #[test]
    fn roundtrips_quoted_values() {
        let line = format_assignment("KEY", "value with # and \"quotes\"");
        let parsed = parse_env(&line);
        assert_eq!(
            parsed[0],
            EnvLine::Assignment {
                name: "KEY".into(),
                value: "value with # and \"quotes\"".into(),
                raw: line
            }
        );
    }

    #[test]
    fn rejects_unsafe_env_file_names() {
        assert!(validate_env_file_name(".env").is_ok());
        assert!(validate_env_file_name(".env.local").is_ok());
        assert!(validate_env_file_name("../secret").is_err());
        assert!(validate_env_file_name(".env/../../etc").is_err());
        assert!(validate_env_file_name("id_rsa").is_err());
    }

    #[test]
    fn vault_and_env_pipeline_end_to_end() {
        let dir = std::env::temp_dir().join(format!("api-desk-env-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("proj");
        std::fs::create_dir_all(&project).unwrap();

        std::fs::write(
            project.join(".env"),
            "DATABASE_URL=postgres://localhost/db\nOPENAI_API_KEY=old-value\n# keep me\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".env.local"),
            "OPENROUTER_API_KEY=test-secret-not-a-real-key\n",
        )
        .unwrap();

        let paths = crate::vault::VaultPaths {
            vault: dir.join("vault.hold"),
            salt: dir.join("vault.salt"),
        };
        let stronghold = crate::vault::open_at(&paths, "test-master-password", true).unwrap();
        crate::vault::write_secret_to(&stronghold, "secret-openai", FAKE_SECRET).unwrap();
        assert_eq!(
            crate::vault::read_secret_from(&stronghold, "secret-openai")
                .unwrap()
                .as_slice(),
            FAKE_SECRET.as_bytes()
        );
        assert!(crate::vault::open_at(&paths, "wrong-password", false).is_err());
        let state = crate::vault::VaultState::with_test_stronghold(stronghold);

        let project_path = project.to_string_lossy().to_string();
        let files = scan_project(project_path.clone()).unwrap();
        assert_eq!(files.len(), 2);
        let env_file = files.iter().find(|file| file.file_name == ".env").unwrap();
        assert!(env_file.variables.iter().any(|name| name == "DATABASE_URL"));
        assert!(env_file.variables.iter().any(|name| name == "OPENAI_API_KEY"));
        assert!(!env_file.variables.iter().any(|name| name.contains("secret")));

        let entries = vec![
            EnvEntry {
                env_name: "OPENAI_API_KEY".into(),
                secret_id: "secret-openai".into(),
            },
            EnvEntry {
                env_name: "OPENROUTER_API_KEY".into(),
                secret_id: "missing-secret".into(),
            },
        ];

        let preview = preview_env(&state, &project_path, &entries).unwrap();
        assert!(preview.existing);
        assert!(preview.changed);
        assert_eq!(preview.lines[0].action, "update");
        assert_eq!(preview.lines[1].action, "missing");

        let result = apply_env(&state, &project_path, &entries, "backup").unwrap();
        assert!(result.written);
        assert_eq!(result.updated, 1);
        assert_eq!(result.missing, 1);
        let backup = result.backup_path.expect("backup file should be created");
        assert!(Path::new(&backup).exists());

        let content = std::fs::read_to_string(project.join(".env")).unwrap();
        assert!(content.contains(&format!("OPENAI_API_KEY={FAKE_SECRET}")));
        assert!(!content.contains("OPENAI_API_KEY=old-value"));
        assert!(content.contains("DATABASE_URL=postgres://localhost/db"));
        assert!(content.contains("# keep me"));
        assert!(!content.contains("OPENROUTER_API_KEY="));

        let again = apply_env(&state, &project_path, &entries, "merge").unwrap();
        assert!(!again.written);
        assert_eq!(again.unchanged, 1);
        assert_eq!(again.missing, 1);

        crate::vault::delete_secret(&state, "secret-openai").unwrap();
        let after_delete = preview_env(&state, &project_path, &entries).unwrap();
        assert_eq!(after_delete.lines[0].action, "missing");
        assert!(!crate::vault::has_secret(&state, "secret-openai"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
