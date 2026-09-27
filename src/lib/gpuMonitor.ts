import { invoke } from "@tauri-apps/api/core";

export interface GpuInfo {
  index: number;
  name: string;
  memoryTotalBytes: number | null;
  memoryUsedBytes: number | null;
  memoryFreeBytes: number | null;
  utilizationPercent: number | null;
  temperatureC: number | null;
  powerWatts: number | null;
}

export type GpuClassification =
  | "ai"
  | "game"
  | "graphics"
  | "browser"
  | "video"
  | "system"
  | "unknown";

export interface GpuProcess {
  pid: number;
  parentPid: number | null;
  processName: string;
  displayName: string;
  productName: string | null;
  executable: string | null;
  classification: GpuClassification | string;
  runtime: string | null;
  models: string[];
  modelSource: string | null;
  gpuPercent: number | null;
  dominantEngine: string | null;
  usedVramBytes: number | null;
  dedicatedVramBytes: number | null;
  sharedGpuBytes: number | null;
  vramSource: string | null;
  service: string | null;
  serviceKind: string;
  managed: boolean;
  isServiceRoot: boolean;
  confidence: string;
  sources: string[];
}

export interface GpuSnapshot {
  available: boolean;
  reason: string | null;
  source: string;
  fetchedAt: string;
  gpus: GpuInfo[];
  processes: GpuProcess[];
  otherCount: number;
  detail: string | null;
}

const GIB = 1024 * 1024 * 1024;

/** VRAM 등 바이트 값을 GiB 문자열로. 값이 없으면 0으로 속이지 않고 "N/A". */
export function formatBytes(bytes: number | null | undefined, digits = 1): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return "N/A";
  const value = bytes / GIB;
  if (value >= 100) return `${value.toFixed(0)} GB`;
  return `${value.toFixed(digits)} GB`;
}

export function vramPercent(info: GpuInfo | null | undefined): number | null {
  if (!info) return null;
  const total = info.memoryTotalBytes;
  const used = info.memoryUsedBytes;
  if (total === null || used === null || total <= 0) return null;
  return Math.max(0, Math.min(100, (used / total) * 100));
}

export function vramTone(percent: number | null): "ok" | "warn" | "danger" | "muted" {
  if (percent === null) return "muted";
  if (percent >= 90) return "danger";
  if (percent >= 75) return "warn";
  return "ok";
}

/** 엔진 이름을 짧은 표시 라벨로. */
export function engineLabel(engine: string | null | undefined): string {
  switch (engine) {
    case "3D":
      return "3D";
    case "Compute":
      return "Compute";
    case "Copy":
      return "Copy";
    case "VideoDecode":
      return "Decode";
    case "VideoEncode":
      return "Encode";
    case "VideoProcessing":
      return "Video";
    case null:
    case undefined:
    case "":
      return "N/A";
    default:
      return engine;
  }
}

/** 분류 라벨. AI로 보이지 않는 프로세스를 AI로 부르지 않는다. */
export function classificationLabel(classification: string): string {
  switch (classification) {
    case "ai":
      return "AI";
    case "game":
      return "게임";
    case "graphics":
      return "그래픽";
    case "browser":
      return "브라우저";
    case "video":
      return "영상";
    case "system":
      return "시스템";
    default:
      return "알 수 없음";
  }
}

export function classificationTone(
  classification: string,
): "ok" | "warn" | "danger" | "info" | "muted" {
  switch (classification) {
    case "ai":
      return "info";
    case "game":
      return "ok";
    default:
      return "muted";
  }
}

export function confidenceLabel(confidence: string): string {
  switch (confidence) {
    case "exact":
      return "확실";
    case "high":
      return "높음";
    case "medium":
      return "보통";
    default:
      return "추정";
  }
}

export function sourcesLabel(sources: string[]): string {
  const map: Record<string, string> = {
    nvmlCompute: "NVML-C",
    nvmlGraphics: "NVML-G",
    windowsPdh: "PDH",
  };
  return sources.map((source) => map[source] ?? source).join("+");
}

export function vramSourceLabel(source: string | null): string | null {
  if (source === "windowsPdh") return "PDH";
  if (source === "nvml") return "NVML";
  return null;
}

/** 표시 이름: 서비스/앱 이름이 있으면 우선, 없으면 실행 파일 이름. */
export function attributionLabel(process: GpuProcess): string {
  if (process.displayName) return process.displayName;
  if (process.service) return process.service;
  if (process.processName) return process.processName;
  return `PID ${process.pid}`;
}

/** 런타임 설명(예: "Ollama", "PyTorch / CUDA", "Browser / 3D"). */
export function runtimeLabel(process: GpuProcess): string {
  if (process.models.length > 0 && process.runtime === "Ollama") return process.runtime;
  return process.runtime ?? classificationLabel(process.classification);
}

export function modelsLabel(process: GpuProcess): string | null {
  if (process.models.length === 0) return null;
  return process.models.join(" · ");
}

export type ProcessSortKey = "gpu" | "vram";

/** 정렬: 기본 GPU % 내림차순 → VRAM 내림차순 (VRAM 토글 시 반대 기준). */
export function sortProcesses(processes: GpuProcess[], key: ProcessSortKey = "gpu"): GpuProcess[] {
  const rankGpu = (process: GpuProcess) => (process.gpuPercent === null ? -1 : process.gpuPercent);
  const rankVram = (process: GpuProcess) =>
    process.usedVramBytes === null ? -1 : process.usedVramBytes;
  return [...processes].sort((a, b) => {
    const first = key === "gpu" ? rankGpu(b) - rankGpu(a) : rankVram(b) - rankVram(a);
    if (first !== 0) return first;
    const second = key === "gpu" ? rankVram(b) - rankVram(a) : rankGpu(b) - rankGpu(a);
    if (second !== 0) return second;
    return a.pid - b.pid;
  });
}

/** 유의미한 AI 모델 워크로드(모델 식별 + VRAM 1GB 이상 또는 모델 식별만으로도 보존 대상). */
export function isSignificantAi(process: GpuProcess): boolean {
  return (
    process.classification === "ai" &&
    (process.models.length > 0 ||
      (process.usedVramBytes !== null && process.usedVramBytes >= GIB))
  );
}

export interface WorkloadRows {
  rows: GpuProcess[];
  otherCount: number;
}

/**
 * 미니 카드/TURZX용 상위 목록.
 * GPU 부하 우선 정렬이되, 시스템 프로세스는 제외하고 AI 모델 워크로드는 보존한다.
 */
export function topWorkloads(processes: GpuProcess[], limit: number): WorkloadRows {
  const visible = processes.filter((process) => process.classification !== "system");
  const sorted = sortProcesses(visible, "gpu");
  const limitValue = Math.max(1, limit);
  const rows = sorted.slice(0, limitValue);
  const missingAi =
    !rows.some((process) => process.classification === "ai") &&
    sorted.slice(limitValue).find((process) => isSignificantAi(process));
  if (missingAi && rows.length >= limitValue) {
    // 가장 낮은 우선순위의 행을 AI 워크로드로 대체해 보존한다.
    rows[rows.length - 1] = missingAi;
    rows.sort((a, b) => {
      const gpu = (b.gpuPercent ?? -1) - (a.gpuPercent ?? -1);
      if (gpu !== 0) return gpu;
      return (b.usedVramBytes ?? -1) - (a.usedVramBytes ?? -1);
    });
  }
  return { rows, otherCount: Math.max(0, visible.length - rows.length) };
}

export async function gpuSnapshot(view: "main" | "mini", visible = true): Promise<GpuSnapshot> {
  return invoke<GpuSnapshot>("gpu_snapshot", { view, visible });
}

export async function gpuProcessDetails(): Promise<{
  processes: GpuProcess[];
  otherCount: number;
}> {
  return invoke<{ processes: GpuProcess[]; otherCount: number }>("gpu_process_details");
}
