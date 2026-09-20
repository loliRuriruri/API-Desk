use tauri_plugin_sql::{Migration, MigrationKind};

pub fn migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            description: "initial schema",
            kind: MigrationKind::Up,
            sql: INITIAL_SCHEMA,
        },
        Migration {
            version: 2,
            description: "credential fields",
            kind: MigrationKind::Up,
            sql: CREDENTIAL_FIELDS,
        },
        Migration {
            version: 3,
            description: "usage snapshots",
            kind: MigrationKind::Up,
            sql: USAGE_SNAPSHOTS,
        },
        Migration {
            version: 4,
            description: "monitor snapshots",
            kind: MigrationKind::Up,
            sql: MONITOR_SNAPSHOTS,
        },
    ]
}

const MONITOR_SNAPSHOTS: &str = r#"
CREATE TABLE monitor_snapshots (
    id           TEXT PRIMARY KEY,
    monitor      TEXT NOT NULL UNIQUE,
    status       TEXT NOT NULL,
    summary      TEXT,
    details_json TEXT,
    fetched_at   TEXT NOT NULL
);
"#;

const USAGE_SNAPSHOTS: &str = r#"
CREATE TABLE usage_snapshots (
    id            TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL UNIQUE REFERENCES credentials(id) ON DELETE CASCADE,
    adapter       TEXT NOT NULL,
    status        TEXT NOT NULL,
    summary       TEXT,
    used          REAL,
    limit_total   REAL,
    remaining     REAL,
    currency      TEXT,
    details_json  TEXT,
    fetched_at    TEXT NOT NULL
);
"#;

const CREDENTIAL_FIELDS: &str = r#"
CREATE TABLE credential_fields (
    id            TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    label         TEXT NOT NULL,
    env_name      TEXT,
    secret_id     TEXT NOT NULL UNIQUE,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE INDEX idx_credential_fields_credential ON credential_fields(credential_id);

INSERT INTO credential_fields (id, credential_id, label, env_name, secret_id, sort_order, created_at, updated_at)
SELECT 'field-' || id, id, 'API Key', env_name, secret_id, 0, created_at, updated_at FROM credentials;
"#;

const INITIAL_SCHEMA: &str = r#"
CREATE TABLE providers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    kind          TEXT NOT NULL DEFAULT 'openai-compatible',
    base_url      TEXT,
    homepage_url  TEXT,
    docs_url      TEXT,
    icon          TEXT,
    notes         TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE accounts (
    id            TEXT PRIMARY KEY,
    provider_id   TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    plan_type     TEXT NOT NULL DEFAULT 'payg'
                  CHECK (plan_type IN ('free','subscription','payg','credits','custom')),
    plan_name     TEXT,
    monthly_cost  REAL NOT NULL DEFAULT 0,
    currency      TEXT NOT NULL DEFAULT 'USD',
    billing_day   INTEGER,
    renewal_date  TEXT,
    status        TEXT NOT NULL DEFAULT 'active'
                  CHECK (status IN ('active','inactive','unknown','expired')),
    notes         TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE credentials (
    id               TEXT PRIMARY KEY,
    account_id       TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    secret_id        TEXT NOT NULL UNIQUE,
    env_name         TEXT,
    status           TEXT NOT NULL DEFAULT 'active'
                     CHECK (status IN ('active','inactive','expired','error','unknown')),
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    expires_at       TEXT,
    last_tested_at   TEXT,
    last_test_status TEXT,
    notes            TEXT
);

CREATE TABLE models (
    id             TEXT PRIMARY KEY,
    provider_id    TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    display_name   TEXT NOT NULL,
    category       TEXT NOT NULL DEFAULT 'LLM',
    context_length INTEGER,
    input_price    REAL,
    output_price   REAL,
    notes          TEXT,
    is_active      INTEGER NOT NULL DEFAULT 1,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    path        TEXT,
    description TEXT,
    notes       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE project_credentials (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    credential_id TEXT NOT NULL REFERENCES credentials(id) ON DELETE CASCADE,
    model_id      TEXT REFERENCES models(id) ON DELETE SET NULL,
    purpose       TEXT NOT NULL DEFAULT '',
    priority      INTEGER NOT NULL DEFAULT 0,
    env_override  TEXT,
    notes         TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (project_id, credential_id, purpose)
);

CREATE TABLE tags (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    color      TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE entity_tags (
    id          TEXT PRIMARY KEY,
    tag_id      TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('provider','account','credential','model','project')),
    entity_id   TEXT NOT NULL,
    UNIQUE (tag_id, entity_type, entity_id)
);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_accounts_provider ON accounts(provider_id);
CREATE INDEX idx_credentials_account ON credentials(account_id);
CREATE INDEX idx_models_provider ON models(provider_id);
CREATE INDEX idx_project_credentials_project ON project_credentials(project_id);
CREATE INDEX idx_project_credentials_credential ON project_credentials(credential_id);
CREATE INDEX idx_entity_tags_entity ON entity_tags(entity_type, entity_id);
"#;
