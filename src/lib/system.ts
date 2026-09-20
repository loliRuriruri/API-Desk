import { invoke } from "@tauri-apps/api/core";
import type { TestOutcome, TestRequestSpec } from "./providers/adapters";

export interface ScanFile {
  fileName: string;
  variables: string[];
}

export interface EnvImportTarget {
  varName: string;
  secretId: string;
}

export interface EnvImportResult {
  varName: string;
  found: boolean;
  masked: string;
}

export type EnvDiffAction = "add" | "update" | "unchanged" | "missing";

export interface EnvDiffLine {
  name: string;
  action: EnvDiffAction;
}

export interface EnvPreview {
  fileName: string;
  filePath: string;
  existing: boolean;
  changed: boolean;
  lines: EnvDiffLine[];
}

export interface EnvApplyResult {
  filePath: string;
  existing: boolean;
  backupPath: string | null;
  added: number;
  updated: number;
  unchanged: number;
  missing: number;
  written: boolean;
}

export interface EnvEntry {
  envName: string;
  secretId: string;
}

export async function scanProject(projectPath: string): Promise<ScanFile[]> {
  return invoke<ScanFile[]>("scan_project", { projectPath });
}

export async function importEnvValues(
  projectPath: string,
  fileName: string,
  targets: EnvImportTarget[],
): Promise<EnvImportResult[]> {
  return invoke<EnvImportResult[]>("import_env_values", { projectPath, fileName, targets });
}

export async function envPreview(projectPath: string, entries: EnvEntry[]): Promise<EnvPreview> {
  return invoke<EnvPreview>("env_preview", { projectPath, entries });
}

export async function envApply(
  projectPath: string,
  entries: EnvEntry[],
  mode: "merge" | "backup",
): Promise<EnvApplyResult> {
  return invoke<EnvApplyResult>("env_apply", { projectPath, entries, mode });
}

export async function testHttp(request: TestRequestSpec): Promise<TestOutcome> {
  return invoke<TestOutcome>("test_http", { request });
}

export async function writeTextFile(path: string, content: string): Promise<void> {
  await invoke("write_text_file", { path, content });
}

export interface AppPaths {
  dbPath: string;
  vaultPath: string;
  saltPath: string;
  appConfigDir: string;
  appLocalDataDir: string;
}

export async function getAppPaths(): Promise<AppPaths> {
  return invoke<AppPaths>("app_paths");
}

export interface ExportEntryInput {
  provider: string;
  account: string;
  name: string;
  field: string;
  envName: string | null;
  status: string;
  lastTestStatus: string | null;
  lastTestedAt: string | null;
  expiresAt: string | null;
  projectUsage: string | null;
  notes: string | null;
  secretId: string;
}

export interface ExportResult {
  filePath: string;
  exported: number;
  missing: number;
  format: string;
}

export async function exportCredentials(
  entries: ExportEntryInput[],
  path: string,
  format: "txt" | "csv",
): Promise<ExportResult> {
  return invoke<ExportResult>("export_credentials", { entries, path, format });
}

export interface UsageOutcome {
  adapter: string;
  status: "ok" | "unsupported" | string;
  summary: string;
  used: number | null;
  limit: number | null;
  remaining: number | null;
  currency: string | null;
  details: unknown;
  message: string | null;
}

export async function fetchUsage(
  adapter: string,
  baseUrl: string | null,
  secretId: string,
  extraSecretId: string | null = null,
): Promise<UsageOutcome> {
  return invoke<UsageOutcome>("fetch_usage", { adapter, baseUrl, secretId, extraSecretId });
}

export interface MonitorProbe {
  monitor: string;
  label: string;
  available: boolean;
  detail: string;
}

export interface MonitorOutcome {
  monitor: string;
  status: "ok" | "unavailable" | string;
  summary: string;
  details: unknown;
  message: string | null;
}

export async function monitorProbe(): Promise<MonitorProbe[]> {
  return invoke<MonitorProbe[]>("monitor_probe");
}

export async function monitorRefresh(monitor: string): Promise<MonitorOutcome> {
  return invoke<MonitorOutcome>("monitor_refresh", { monitor });
}

export interface AntigravityAccount {
  id: string;
  label: string;
  email: string | null;
  tier: string | null;
  savedAt: string;
  isActive: boolean;
}

export async function antigravityAccounts(): Promise<AntigravityAccount[]> {
  return invoke<AntigravityAccount[]>("monitor_antigravity_accounts");
}

export async function antigravitySaveCurrent(label: string): Promise<AntigravityAccount> {
  return invoke<AntigravityAccount>("monitor_antigravity_save_current", { label });
}

export async function antigravitySwitchAccount(id: string): Promise<void> {
  await invoke("monitor_antigravity_switch_account", { id });
}

export async function antigravityDeleteAccount(id: string): Promise<void> {
  await invoke("monitor_antigravity_delete_account", { id });
}

export interface ImportedModel {
  id: string;
  displayName: string;
  ownedBy: string | null;
}

export async function fetchModels(
  kind: string,
  baseUrl: string | null,
  secretId: string,
): Promise<ImportedModel[]> {
  return invoke<ImportedModel[]>("fetch_models", { kind, baseUrl, secretId });
}
