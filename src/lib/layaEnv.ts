import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface LayaVersionCheck {
  installed: string | null;
  latest: string | null;
  updateAvailable: boolean;
  source: string;
  error: string | null;
  checkedAt: string | null;
  fromCache: boolean;
}

export interface LayaEnvironmentStatus {
  pythonPath: string;
  pythonVersion: string | null;
  layaVersion: string | null;
  latestLayaVersion: string | null;
  updateAvailable: boolean;
  versionCheckError: string | null;
  torchVersion: string | null;
  torchCudaVersion: string | null;
  cudaAvailable: boolean;
  gpuName: string | null;
  computeCapability: string | null;
  tilelangInstalled: boolean;
  tilelangVersion: string | null;
  fastPathSupported: boolean;
  fastPathActive: boolean | null;
  effectiveDevice: string | null;
  backend: string | null;
  environmentKind: string;
  environmentVersion: string | null;
  serviceRunning: boolean;
  servicePid: number | null;
  health: Record<string, unknown> | null;
  problems: string[];
}

export interface LayaEnvEntry {
  path: string;
  version: string | null;
  kind: string;
  state: string;
  createdAt: string | null;
  metrics: Record<string, unknown> | null;
  active: boolean;
}

export interface LayaBenchmark {
  pythonPath: string;
  device: string | null;
  backend: string | null;
  layaVersion: string | null;
  torchVersion: string | null;
  coldLoadMs: number | null;
  warmP50Ms: number | null;
  batchP50Ms: number | null;
  iterations: number;
  loadedModels: string[];
  vramUsedBytes: number | null;
  error: string | null;
}

export interface LayaEnvProgress {
  step: string;
  label: string;
  phase: "running" | "done" | "failed" | string;
  detail: string | null;
  at: string;
}

export async function layaEnvStatus(): Promise<LayaEnvironmentStatus> {
  return invoke<LayaEnvironmentStatus>("laya_env_status");
}

export async function layaEnvCheckUpdates(force = false): Promise<LayaVersionCheck> {
  return invoke<LayaVersionCheck>("laya_env_check_updates", { force });
}

export async function layaEnvCreateGpu(targetVersion?: string): Promise<void> {
  await invoke("laya_env_create_gpu", { targetVersion: targetVersion ?? null });
}

export async function layaEnvCancel(): Promise<void> {
  await invoke("laya_env_cancel");
}

export async function layaEnvList(): Promise<LayaEnvEntry[]> {
  return invoke<LayaEnvEntry[]>("laya_env_list");
}

export async function layaEnvActivate(envPath: string): Promise<string> {
  return invoke<string>("laya_env_activate", { envPath });
}

export async function layaEnvRollback(): Promise<string> {
  return invoke<string>("laya_env_rollback");
}

export async function layaEnvBenchmark(envPath?: string, iterations = 5): Promise<LayaBenchmark> {
  return invoke<LayaBenchmark>("laya_env_benchmark", {
    envPath: envPath ?? null,
    iterations,
  });
}

export function onLayaEnvProgress(handler: (event: LayaEnvProgress) => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen<LayaEnvProgress>("laya-env-progress", (event) => handler(event.payload)).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/** 상태 요약 라벨(헬스 체크 결과 우선). */
export function environmentSummary(status: LayaEnvironmentStatus | null): string {
  if (!status) return "확인 중…";
  if (status.backend === "tilelang" && status.effectiveDevice === "cuda") {
    return "RTX GPU Fast Path 정상";
  }
  if (status.cudaAvailable) {
    return "CUDA 사용 가능 · Fast Path 미적용";
  }
  return "CPU 모드로 실행 중";
}

export function backendLabel(backend: string | null | undefined): string {
  switch (backend) {
    case "tilelang":
      return "TileLang (fast)";
    case "torch":
      return "PyTorch (stock)";
    case "cpu":
      return "CPU";
    default:
      return "확인 중";
  }
}

export function deviceLabel(device: string | null | undefined): string {
  switch (device) {
    case "cuda":
      return "CUDA";
    case "cpu":
      return "CPU";
    default:
      return "확인 필요";
  }
}

export function versionLabel(installed: string | null, latest: string | null, update: boolean): string {
  if (!installed) return "확인 불가";
  if (!latest) return `${installed} (최신 확인 불가)`;
  return update ? `${installed} → ${latest}` : `${installed} (최신)`;
}

export function benchmarkSummary(result: LayaBenchmark | null): string | null {
  if (!result) return null;
  if (result.error) return `실패: ${result.error}`;
  const parts: string[] = [];
  if (result.coldLoadMs !== null) parts.push(`콜드 ${result.coldLoadMs}ms`);
  if (result.warmP50Ms !== null) parts.push(`웜 p50 ${result.warmP50Ms.toFixed(0)}ms`);
  if (result.batchP50Ms !== null) parts.push(`배치 p50 ${result.batchP50Ms.toFixed(0)}ms`);
  if (result.vramUsedBytes !== null) {
    parts.push(`VRAM ${(result.vramUsedBytes / 1024 ** 3).toFixed(2)}GB`);
  }
  return parts.join(" · ");
}
