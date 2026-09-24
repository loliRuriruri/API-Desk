import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface LocalServiceEnvVar {
  key: string;
  value: string;
}

export interface LocalServiceDefinition {
  id: string;
  label: string;
  description: string;
  defaultEndpoint: string;
  defaultPort: number;
  defaultDevice: string;
  deviceEnv: string;
  staticEnv: LocalServiceEnvVar[];
  secretEnv: string;
  secretId: string;
  apiKeyLabel: string;
  defaultHealthPath: string;
  supportsLoginStartup: boolean;
}

export interface LocalServiceConfig {
  id: string;
  executable: string;
  args: string[];
  workdir: string;
  endpoint: string;
  port: number;
  device: string;
  healthPath: string;
  autoStart: boolean;
  keepAliveOnExit: boolean;
}

export interface LocalServiceStatus {
  id: string;
  label: string;
  description: string;
  state: string;
  endpoint: string;
  port: number;
  device: string;
  pid: number | null;
  managed: boolean;
  uptimeSecs: number | null;
  apiKeySet: boolean;
  apiKeyHint: string | null;
  apiKeyConfigured: boolean;
  vaultLocked: boolean;
  executable: string;
  executableSet: boolean;
  args: string[];
  workdir: string;
  healthPath: string;
  autoStart: boolean;
  keepAliveOnExit: boolean;
  logLines: number;
  lastError: string | null;
}

export interface ServiceHealth {
  ok: boolean;
  status: number;
  latencyMs: number;
  url: string;
  detail: string | null;
  label?: string;
}

export function formatUptime(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return "—";
  }
  const total = Math.floor(seconds);
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3_600);
  const minutes = Math.floor((total % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${total % 60}s`;
  return `${total}s`;
}

export function serviceStateLabel(state: string): string {
  switch (state) {
    case "running":
      return "실행 중";
    case "starting":
      return "시작 중";
    case "error":
      return "오류";
    case "stopped":
      return "중지됨";
    default:
      return state;
  }
}

export function serviceStateTone(state: string): "ok" | "warn" | "danger" | "muted" {
  switch (state) {
    case "running":
      return "ok";
    case "starting":
      return "warn";
    case "error":
      return "danger";
    default:
      return "muted";
  }
}

export function apiKeyLabel(status: LocalServiceStatus): string {
  if (status.apiKeySet) {
    return status.apiKeyHint ? `설정됨 (${status.apiKeyHint}) · Bearer 인증` : "설정됨 · Bearer 인증";
  }
  if (status.vaultLocked) return "Vault 잠금 — 무인증 모드로 실행";
  return "미설정 — localhost 전용 무인증";
}

/** 미니 카드용 짧은 인증 상태 표기 (키 값은 절대 포함하지 않는다). */
export function miniAuthLabel(status: LocalServiceStatus): string {
  if (status.apiKeySet) return "Bearer 인증";
  if (status.apiKeyConfigured && status.vaultLocked) return "Vault 잠금";
  return "무인증 (localhost)";
}

/** Start를 막아야 하는 사유(없으면 null). 백엔드 start_block_reason과 동일한 규칙. */
export function startBlockReason(status: LocalServiceStatus): string | null {
  if (!status.executableSet) return "실행 파일 경로 미설정";
  if (status.apiKeyConfigured && status.vaultLocked) return "Vault 잠금 해제 필요";
  if (status.apiKeyConfigured && !status.apiKeySet) return "저장된 키를 찾을 수 없음";
  return null;
}

export function onLocalServiceStatusChanged(handler: () => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen("local-service-status-changed", () => handler()).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export async function localServiceDefinitions(): Promise<LocalServiceDefinition[]> {
  return invoke<LocalServiceDefinition[]>("local_service_definitions");
}

export async function localServiceStatuses(): Promise<LocalServiceStatus[]> {
  return invoke<LocalServiceStatus[]>("local_service_status");
}

export async function localServiceConfigs(): Promise<LocalServiceConfig[]> {
  return invoke<LocalServiceConfig[]>("local_service_configs");
}

export async function saveLocalServiceConfig(
  config: LocalServiceConfig,
): Promise<LocalServiceConfig[]> {
  return invoke<LocalServiceConfig[]>("local_service_save_config", { config });
}

export async function startLocalService(id: string): Promise<LocalServiceStatus> {
  return invoke<LocalServiceStatus>("local_service_start", { id });
}

export async function stopLocalService(id: string): Promise<LocalServiceStatus> {
  return invoke<LocalServiceStatus>("local_service_stop", { id });
}

export async function restartLocalService(id: string): Promise<LocalServiceStatus> {
  return invoke<LocalServiceStatus>("local_service_restart", { id });
}

export async function checkLocalServiceHealth(id: string): Promise<ServiceHealth> {
  return invoke<ServiceHealth>("local_service_health", { id });
}

export async function localServiceLogs(id: string, limit = 120): Promise<string[]> {
  return invoke<string[]>("local_service_logs", { id, limit });
}

export async function setLocalServiceApiKey(id: string, value: string): Promise<void> {
  await invoke("local_service_set_api_key", { id, value });
}

export async function clearLocalServiceApiKey(id: string): Promise<void> {
  await invoke("local_service_clear_api_key", { id });
}

export async function autostartLocalServices(): Promise<string[]> {
  return invoke<string[]>("local_service_autostart");
}
