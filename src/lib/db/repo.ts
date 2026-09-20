import { execute, select, selectOne } from "./index";
import type {
  Account,
  Credential,
  ModelInfo,
  Project,
  ProjectCredential,
  Provider,
  ProviderKind,
  SearchResult,
  Tag,
  TestStatus,
} from "../../types/domain";
import { newId, nowIso } from "../format";

async function updateRow(
  table: string,
  id: string,
  patch: Record<string, unknown>,
): Promise<void> {
  const keys = Object.keys(patch);
  if (keys.length === 0) return;
  const assignments = keys.map((key) => `${key} = ?`).join(", ");
  const values = keys.map((key) => patch[key]);
  await execute(`UPDATE ${table} SET ${assignments}, updated_at = ? WHERE id = ?`, [
    ...values,
    nowIso(),
    id,
  ]);
}

export interface ProviderInput {
  name: string;
  display_name: string;
  kind: ProviderKind;
  base_url?: string | null;
  homepage_url?: string | null;
  docs_url?: string | null;
  notes?: string | null;
}

export async function listProviders(): Promise<Provider[]> {
  return select<Provider>(
    "SELECT * FROM providers ORDER BY display_name COLLATE NOCASE",
  );
}

export async function getProvider(id: string): Promise<Provider | null> {
  return selectOne<Provider>("SELECT * FROM providers WHERE id = ?", [id]);
}

export async function createProvider(input: ProviderInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO providers (id, name, display_name, kind, base_url, homepage_url, docs_url, icon, notes, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)`,
    [
      id,
      input.name,
      input.display_name,
      input.kind,
      input.base_url ?? null,
      input.homepage_url ?? null,
      input.docs_url ?? null,
      input.notes ?? null,
      now,
      now,
    ],
  );
  return id;
}

export async function updateProvider(
  id: string,
  patch: Partial<ProviderInput>,
): Promise<void> {
  await updateRow("providers", id, patch as Record<string, unknown>);
}

export async function deleteProvider(id: string): Promise<void> {
  await execute("DELETE FROM providers WHERE id = ?", [id]);
}

export interface AccountInput {
  provider_id: string;
  name: string;
  plan_type: Account["plan_type"];
  plan_name?: string | null;
  monthly_cost: number;
  currency: string;
  billing_day?: number | null;
  renewal_date?: string | null;
  status: Account["status"];
  notes?: string | null;
}

export async function listAccounts(providerId?: string): Promise<Account[]> {
  if (providerId) {
    return select<Account>(
      "SELECT * FROM accounts WHERE provider_id = ? ORDER BY name COLLATE NOCASE",
      [providerId],
    );
  }
  return select<Account>("SELECT * FROM accounts ORDER BY name COLLATE NOCASE");
}

export async function getAccount(id: string): Promise<Account | null> {
  return selectOne<Account>("SELECT * FROM accounts WHERE id = ?", [id]);
}

export async function createAccount(input: AccountInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO accounts (id, provider_id, name, plan_type, plan_name, monthly_cost, currency, billing_day, renewal_date, status, notes, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      id,
      input.provider_id,
      input.name,
      input.plan_type,
      input.plan_name ?? null,
      input.monthly_cost,
      input.currency,
      input.billing_day ?? null,
      input.renewal_date ?? null,
      input.status,
      input.notes ?? null,
      now,
      now,
    ],
  );
  return id;
}

export async function updateAccount(
  id: string,
  patch: Partial<Omit<AccountInput, "provider_id">>,
): Promise<void> {
  await updateRow("accounts", id, patch as Record<string, unknown>);
}

export async function deleteAccount(id: string): Promise<void> {
  await execute("DELETE FROM accounts WHERE id = ?", [id]);
}

export interface CredentialInput {
  account_id: string;
  name: string;
  secret_id: string;
  env_name?: string | null;
  status: Credential["status"];
  expires_at?: string | null;
  notes?: string | null;
}

export async function listCredentials(accountId: string): Promise<Credential[]> {
  return select<Credential>(
    "SELECT * FROM credentials WHERE account_id = ? ORDER BY name COLLATE NOCASE",
    [accountId],
  );
}

export async function getCredential(id: string): Promise<Credential | null> {
  return selectOne<Credential>("SELECT * FROM credentials WHERE id = ?", [id]);
}

export interface CredentialWithContext extends Credential {
  account_name: string;
  provider_id: string;
  provider_name: string;
  provider_display_name: string;
  provider_kind: ProviderKind;
  provider_base_url: string | null;
}

const CREDENTIAL_CONTEXT_SQL = `
  SELECT c.*, a.name AS account_name, a.provider_id AS provider_id,
         p.name AS provider_name, p.display_name AS provider_display_name,
         p.kind AS provider_kind, p.base_url AS provider_base_url
  FROM credentials c
  JOIN accounts a ON a.id = c.account_id
  JOIN providers p ON p.id = a.provider_id
`;

export async function listCredentialsWithContext(): Promise<CredentialWithContext[]> {
  return select<CredentialWithContext>(
    `${CREDENTIAL_CONTEXT_SQL} ORDER BY p.display_name COLLATE NOCASE, a.name COLLATE NOCASE, c.name COLLATE NOCASE`,
  );
}

export async function getCredentialWithContext(
  id: string,
): Promise<CredentialWithContext | null> {
  return selectOne<CredentialWithContext>(`${CREDENTIAL_CONTEXT_SQL} WHERE c.id = ?`, [id]);
}

export async function createCredential(input: CredentialInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO credentials (id, account_id, name, secret_id, env_name, status, created_at, updated_at, expires_at, last_tested_at, last_test_status, notes)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?)`,
    [
      id,
      input.account_id,
      input.name,
      input.secret_id,
      input.env_name ?? null,
      input.status,
      now,
      now,
      input.expires_at ?? null,
      input.notes ?? null,
    ],
  );
  return id;
}

export async function updateCredential(
  id: string,
  patch: Partial<Omit<CredentialInput, "account_id" | "secret_id">>,
): Promise<void> {
  await updateRow("credentials", id, patch as Record<string, unknown>);
}

export async function recordTestResult(
  id: string,
  status: TestStatus,
): Promise<void> {
  await execute(
    "UPDATE credentials SET last_tested_at = ?, last_test_status = ?, updated_at = ? WHERE id = ?",
    [nowIso(), status, nowIso(), id],
  );
}

export async function deleteCredential(id: string): Promise<void> {
  await execute("DELETE FROM credentials WHERE id = ?", [id]);
}

export interface CredentialField {
  id: string;
  credential_id: string;
  label: string;
  env_name: string | null;
  secret_id: string;
  sort_order: number;
  created_at: string;
  updated_at: string;
}

export async function listAllCredentialFields(): Promise<CredentialField[]> {
  return select<CredentialField>(
    "SELECT * FROM credential_fields ORDER BY credential_id, sort_order, created_at",
  );
}

export async function listCredentialFields(credentialId: string): Promise<CredentialField[]> {
  return select<CredentialField>(
    "SELECT * FROM credential_fields WHERE credential_id = ? ORDER BY sort_order, created_at",
    [credentialId],
  );
}

export interface CredentialFieldInput {
  credential_id: string;
  label: string;
  env_name?: string | null;
  secret_id: string;
  sort_order: number;
}

export async function createCredentialField(input: CredentialFieldInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO credential_fields (id, credential_id, label, env_name, secret_id, sort_order, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      id,
      input.credential_id,
      input.label,
      input.env_name ?? null,
      input.secret_id,
      input.sort_order,
      now,
      now,
    ],
  );
  return id;
}

export async function updateCredentialField(
  id: string,
  patch: { label?: string; env_name?: string | null },
): Promise<void> {
  await updateRow("credential_fields", id, patch as Record<string, unknown>);
}

export async function deleteCredentialField(id: string): Promise<void> {
  await execute("DELETE FROM credential_fields WHERE id = ?", [id]);
}

export interface ModelInput {
  provider_id: string;
  name: string;
  display_name: string;
  category: ModelInfo["category"];
  context_length?: number | null;
  input_price?: number | null;
  output_price?: number | null;
  notes?: string | null;
  is_active: number;
}

export interface ModelWithProvider extends ModelInfo {
  provider_display_name: string;
}

export async function listModels(): Promise<ModelWithProvider[]> {
  return select<ModelWithProvider>(
    `SELECT m.*, p.display_name AS provider_display_name
     FROM models m JOIN providers p ON p.id = m.provider_id
     ORDER BY p.display_name COLLATE NOCASE, m.display_name COLLATE NOCASE`,
  );
}

export async function listModelsByProvider(providerId: string): Promise<ModelInfo[]> {
  return select<ModelInfo>(
    "SELECT * FROM models WHERE provider_id = ? ORDER BY display_name COLLATE NOCASE",
    [providerId],
  );
}

export async function createModel(input: ModelInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO models (id, provider_id, name, display_name, category, context_length, input_price, output_price, notes, is_active, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      id,
      input.provider_id,
      input.name,
      input.display_name,
      input.category,
      input.context_length ?? null,
      input.input_price ?? null,
      input.output_price ?? null,
      input.notes ?? null,
      input.is_active,
      now,
      now,
    ],
  );
  return id;
}

export async function updateModel(id: string, patch: Partial<ModelInput>): Promise<void> {
  await updateRow("models", id, patch as Record<string, unknown>);
}

export async function deleteModel(id: string): Promise<void> {
  await execute("DELETE FROM models WHERE id = ?", [id]);
}

export interface ProjectInput {
  name: string;
  path?: string | null;
  description?: string | null;
  notes?: string | null;
}

export interface ProjectWithCounts extends Project {
  link_count: number;
}

export async function listProjects(): Promise<ProjectWithCounts[]> {
  return select<ProjectWithCounts>(
    `SELECT p.*, (SELECT COUNT(*) FROM project_credentials pc WHERE pc.project_id = p.id) AS link_count
     FROM projects p ORDER BY p.name COLLATE NOCASE`,
  );
}

export async function getProject(id: string): Promise<Project | null> {
  return selectOne<Project>("SELECT * FROM projects WHERE id = ?", [id]);
}

export async function createProject(input: ProjectInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO projects (id, name, path, description, notes, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
    [id, input.name, input.path ?? null, input.description ?? null, input.notes ?? null, now, now],
  );
  return id;
}

export async function updateProject(id: string, patch: Partial<ProjectInput>): Promise<void> {
  await updateRow("projects", id, patch as Record<string, unknown>);
}

export async function deleteProject(id: string): Promise<void> {
  await execute("DELETE FROM projects WHERE id = ?", [id]);
}

export interface ProjectLinkInput {
  project_id: string;
  credential_id: string;
  model_id?: string | null;
  purpose: string;
  priority: number;
  env_override?: string | null;
  notes?: string | null;
}

export interface ProjectLink extends ProjectCredential {
  credential_name: string;
  env_name: string | null;
  account_name: string;
  provider_id: string;
  provider_name: string;
  provider_display_name: string;
  provider_kind: ProviderKind;
  model_name: string | null;
  model_display_name: string | null;
}

export async function listProjectLinks(projectId: string): Promise<ProjectLink[]> {
  return select<ProjectLink>(
    `SELECT pc.*, c.name AS credential_name, c.env_name AS env_name,
            a.name AS account_name, p.id AS provider_id, p.name AS provider_name,
            p.display_name AS provider_display_name, p.kind AS provider_kind,
            m.name AS model_name, m.display_name AS model_display_name
     FROM project_credentials pc
     JOIN credentials c ON c.id = pc.credential_id
     JOIN accounts a ON a.id = c.account_id
     JOIN providers p ON p.id = a.provider_id
     LEFT JOIN models m ON m.id = pc.model_id
     WHERE pc.project_id = ?
     ORDER BY pc.priority ASC, pc.purpose COLLATE NOCASE`,
    [projectId],
  );
}

export async function createProjectLink(input: ProjectLinkInput): Promise<string> {
  const id = newId();
  const now = nowIso();
  await execute(
    `INSERT INTO project_credentials (id, project_id, credential_id, model_id, purpose, priority, env_override, notes, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    [
      id,
      input.project_id,
      input.credential_id,
      input.model_id ?? null,
      input.purpose,
      input.priority,
      input.env_override ?? null,
      input.notes ?? null,
      now,
      now,
    ],
  );
  return id;
}

export async function updateProjectLink(
  id: string,
  patch: Partial<Omit<ProjectLinkInput, "project_id" | "credential_id">>,
): Promise<void> {
  await updateRow("project_credentials", id, patch as Record<string, unknown>);
}

export async function deleteProjectLink(id: string): Promise<void> {
  await execute("DELETE FROM project_credentials WHERE id = ?", [id]);
}

export async function listCredentialsForProject(projectId: string): Promise<string[]> {
  const rows = await select<{ credential_id: string }>(
    "SELECT credential_id FROM project_credentials WHERE project_id = ?",
    [projectId],
  );
  return rows.map((row) => row.credential_id);
}

export async function listTags(): Promise<Tag[]> {
  return select<Tag>("SELECT * FROM tags ORDER BY name COLLATE NOCASE");
}

export interface CredentialProjectUsage {
  credential_id: string;
  project_name: string;
  purpose: string;
}

export async function listCredentialProjectUsage(): Promise<CredentialProjectUsage[]> {
  return select<CredentialProjectUsage>(
    `SELECT pc.credential_id, p.name AS project_name, pc.purpose
     FROM project_credentials pc
     JOIN projects p ON p.id = pc.project_id
     ORDER BY p.name COLLATE NOCASE`,
  );
}

export interface UsageSnapshot {
  id: string;
  credential_id: string;
  adapter: string;
  status: string;
  summary: string | null;
  used: number | null;
  limit_total: number | null;
  remaining: number | null;
  currency: string | null;
  details_json: string | null;
  fetched_at: string;
}

export async function listUsageSnapshots(): Promise<UsageSnapshot[]> {
  return select<UsageSnapshot>("SELECT * FROM usage_snapshots ORDER BY fetched_at DESC");
}

export interface UsageSnapshotInput {
  credential_id: string;
  adapter: string;
  status: string;
  summary: string | null;
  used: number | null;
  limit_total: number | null;
  remaining: number | null;
  currency: string | null;
  details_json: string | null;
  fetched_at: string;
}

export async function upsertUsageSnapshot(input: UsageSnapshotInput): Promise<void> {
  await execute(
    `INSERT INTO usage_snapshots (id, credential_id, adapter, status, summary, used, limit_total, remaining, currency, details_json, fetched_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(credential_id) DO UPDATE SET
       adapter = excluded.adapter,
       status = excluded.status,
       summary = excluded.summary,
       used = excluded.used,
       limit_total = excluded.limit_total,
       remaining = excluded.remaining,
       currency = excluded.currency,
       details_json = excluded.details_json,
       fetched_at = excluded.fetched_at`,
    [
      newId(),
      input.credential_id,
      input.adapter,
      input.status,
      input.summary,
      input.used,
      input.limit_total,
      input.remaining,
      input.currency,
      input.details_json,
      input.fetched_at,
    ],
  );
}

export interface MonitorSnapshot {
  id: string;
  monitor: string;
  status: string;
  summary: string | null;
  details_json: string | null;
  fetched_at: string;
}

export async function listMonitorSnapshots(): Promise<MonitorSnapshot[]> {
  return select<MonitorSnapshot>("SELECT * FROM monitor_snapshots ORDER BY monitor");
}

export interface MonitorSnapshotInput {
  monitor: string;
  status: string;
  summary: string | null;
  details_json: string | null;
  fetched_at: string;
}

export async function upsertMonitorSnapshot(input: MonitorSnapshotInput): Promise<void> {
  await execute(
    `INSERT INTO monitor_snapshots (id, monitor, status, summary, details_json, fetched_at)
     VALUES (?, ?, ?, ?, ?, ?)
     ON CONFLICT(monitor) DO UPDATE SET
       status = excluded.status,
       summary = excluded.summary,
       details_json = excluded.details_json,
       fetched_at = excluded.fetched_at`,
    [newId(), input.monitor, input.status, input.summary, input.details_json, input.fetched_at],
  );
}

export async function getSetting(key: string): Promise<string | null> {
  const row = await selectOne<{ value: string }>(
    "SELECT value FROM settings WHERE key = ?",
    [key],
  );
  return row ? row.value : null;
}

export async function setSetting(key: string, value: string): Promise<void> {
  await execute(
    `INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`,
    [key, value, nowIso()],
  );
}

export async function searchAll(query: string, limitPerKind = 6): Promise<SearchResult[]> {
  const trimmed = query.trim();
  if (trimmed.length === 0) return [];
  const like = `%${trimmed}%`;
  const results: SearchResult[] = [];

  const providers = await select<Provider>(
    `SELECT * FROM providers
     WHERE display_name LIKE ? OR name LIKE ? OR IFNULL(notes,'') LIKE ?
     ORDER BY display_name COLLATE NOCASE LIMIT ?`,
    [like, like, like, limitPerKind],
  );
  for (const provider of providers) {
    results.push({
      kind: "provider",
      id: provider.id,
      label: provider.display_name,
      sublabel: `프로바이더 · ${provider.kind}`,
      extra: provider.base_url,
    });
  }

  const accounts = await select<Account & { provider_display_name: string }>(
    `SELECT a.*, p.display_name AS provider_display_name
     FROM accounts a JOIN providers p ON p.id = a.provider_id
     WHERE a.name LIKE ? OR IFNULL(a.plan_name,'') LIKE ? OR IFNULL(a.notes,'') LIKE ?
     ORDER BY a.name COLLATE NOCASE LIMIT ?`,
    [like, like, like, limitPerKind],
  );
  for (const account of accounts) {
    results.push({
      kind: "account",
      id: account.id,
      label: account.name,
      sublabel: `계정 · ${account.provider_display_name}`,
      extra: account.plan_name,
    });
  }

  const credentials = await select<
    Credential & { provider_display_name: string; account_name: string }
  >(
    `SELECT c.*, p.display_name AS provider_display_name, a.name AS account_name
     FROM credentials c
     JOIN accounts a ON a.id = c.account_id
     JOIN providers p ON p.id = a.provider_id
     WHERE c.name LIKE ? OR IFNULL(c.env_name,'') LIKE ? OR IFNULL(c.notes,'') LIKE ?
     ORDER BY c.name COLLATE NOCASE LIMIT ?`,
    [like, like, like, limitPerKind],
  );
  for (const credential of credentials) {
    results.push({
      kind: "credential",
      id: credential.id,
      label: credential.name,
      sublabel: `인증 정보 · ${credential.provider_display_name} / ${credential.account_name}`,
      extra: credential.env_name,
    });
  }

  const models = await select<ModelInfo & { provider_display_name: string }>(
    `SELECT m.*, p.display_name AS provider_display_name
     FROM models m JOIN providers p ON p.id = m.provider_id
     WHERE m.name LIKE ? OR m.display_name LIKE ? OR IFNULL(m.notes,'') LIKE ?
     ORDER BY m.display_name COLLATE NOCASE LIMIT ?`,
    [like, like, like, limitPerKind],
  );
  for (const model of models) {
    results.push({
      kind: "model",
      id: model.id,
      label: model.display_name,
      sublabel: `모델 · ${model.provider_display_name}`,
      extra: model.category,
    });
  }

  const projects = await select<Project>(
    `SELECT * FROM projects
     WHERE name LIKE ? OR IFNULL(path,'') LIKE ? OR IFNULL(description,'') LIKE ? OR IFNULL(notes,'') LIKE ?
     ORDER BY name COLLATE NOCASE LIMIT ?`,
    [like, like, like, like, limitPerKind],
  );
  for (const project of projects) {
    results.push({
      kind: "project",
      id: project.id,
      label: project.name,
      sublabel: "프로젝트",
      extra: project.path,
    });
  }

  const tags = await select<Tag>("SELECT * FROM tags WHERE name LIKE ? ORDER BY name LIMIT ?", [
    like,
    limitPerKind,
  ]);
  for (const tag of tags) {
    results.push({
      kind: "tag",
      id: tag.id,
      label: tag.name,
      sublabel: "태그",
      extra: null,
    });
  }

  return results;
}

export interface ProviderCard {
  id: string;
  display_name: string;
  kind: ProviderKind;
  base_url: string | null;
  account_count: number;
  credential_count: number;
  monthly_cost: number;
  last_activity: string | null;
}

export async function listProviderCards(): Promise<ProviderCard[]> {
  return select<ProviderCard>(
    `SELECT p.id, p.display_name, p.kind, p.base_url,
       (SELECT COUNT(*) FROM accounts a WHERE a.provider_id = p.id) AS account_count,
       (SELECT COUNT(*) FROM credentials c JOIN accounts a ON a.id = c.account_id WHERE a.provider_id = p.id) AS credential_count,
       (SELECT IFNULL(SUM(a.monthly_cost), 0) FROM accounts a WHERE a.provider_id = p.id AND a.status = 'active') AS monthly_cost,
       (SELECT MAX(c.updated_at) FROM credentials c JOIN accounts a ON a.id = c.account_id WHERE a.provider_id = p.id) AS last_activity
     FROM providers p
     ORDER BY p.display_name COLLATE NOCASE`,
  );
}

export interface DashboardCounts {
  total: number;
  active: number;
  needs_check: number;
  unused: number;
}

export async function dashboardCounts(): Promise<DashboardCounts> {
  const row = await selectOne<DashboardCounts>(
    `SELECT
       (SELECT COUNT(*) FROM credentials) AS total,
       (SELECT COUNT(*) FROM credentials WHERE status = 'active') AS active,
       (SELECT COUNT(*) FROM credentials WHERE last_test_status IS NULL OR last_test_status != 'connected') AS needs_check,
       (SELECT COUNT(*) FROM credentials c WHERE NOT EXISTS (SELECT 1 FROM project_credentials pc WHERE pc.credential_id = c.id)) AS unused`,
  );
  return row ?? { total: 0, active: 0, needs_check: 0, unused: 0 };
}

export interface CostLine {
  provider_display_name: string;
  account_name: string;
  plan_name: string | null;
  monthly_cost: number;
  currency: string;
}

export async function listCostLines(): Promise<CostLine[]> {
  return select<CostLine>(
    `SELECT p.display_name AS provider_display_name, a.name AS account_name, a.plan_name,
            a.monthly_cost, a.currency
     FROM accounts a JOIN providers p ON p.id = a.provider_id
     WHERE a.status = 'active' AND a.monthly_cost > 0
     ORDER BY a.monthly_cost DESC`,
  );
}

export interface UpcomingRenewal {
  account_name: string;
  provider_display_name: string;
  billing_day: number | null;
  renewal_date: string | null;
  monthly_cost: number;
  currency: string;
}

export async function listUpcomingRenewals(): Promise<UpcomingRenewal[]> {
  return select<UpcomingRenewal>(
    `SELECT a.name AS account_name, p.display_name AS provider_display_name, a.billing_day,
            a.renewal_date, a.monthly_cost, a.currency
     FROM accounts a JOIN providers p ON p.id = a.provider_id
     WHERE a.status = 'active' AND (a.billing_day IS NOT NULL OR a.renewal_date IS NOT NULL)
     ORDER BY IFNULL(a.renewal_date, ''), IFNULL(a.billing_day, 32)`,
  );
}

export interface RecentCredential {
  id: string;
  name: string;
  env_name: string | null;
  updated_at: string;
  provider_display_name: string;
  last_test_status: TestStatus | null;
}

export async function listRecentCredentials(limit = 6): Promise<RecentCredential[]> {
  return select<RecentCredential>(
    `SELECT c.id, c.name, c.env_name, c.updated_at, p.display_name AS provider_display_name, c.last_test_status
     FROM credentials c
     JOIN accounts a ON a.id = c.account_id
     JOIN providers p ON p.id = a.provider_id
     ORDER BY c.updated_at DESC LIMIT ?`,
    [limit],
  );
}

export interface ExpiringCredential {
  id: string;
  name: string;
  env_name: string | null;
  expires_at: string;
  provider_display_name: string;
}

export async function listExpiringCredentials(): Promise<ExpiringCredential[]> {
  return select<ExpiringCredential>(
    `SELECT c.id, c.name, c.env_name, c.expires_at, p.display_name AS provider_display_name
     FROM credentials c
     JOIN accounts a ON a.id = c.account_id
     JOIN providers p ON p.id = a.provider_id
     WHERE c.expires_at IS NOT NULL AND c.expires_at != ''
     ORDER BY c.expires_at ASC LIMIT 8`,
  );
}
