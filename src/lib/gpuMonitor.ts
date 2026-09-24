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

export interface GpuProcess {
  pid: number;
  processName: string;
  usedVramBytes: number | null;
  service: string | null;
  serviceKind: string;
  models: string[];
  modelSource: string | null;
  confidence: string;
  executable: string | null;
  isServiceRoot: boolean;
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

export interface WorkloadRows {
  rows: GpuProcess[];
  otherCount: number;
}

/** 미니 카드용: 상위 workload만 남기고 나머지는 Other로 묶는다. */
export function topWorkloads(processes: GpuProcess[], limit: number): WorkloadRows {
  const named = processes.filter((process) => process.serviceKind !== "other");
  const rows = named.slice(0, Math.max(0, limit));
  const hiddenNamed = named.length - rows.length;
  const others = processes.filter((process) => process.serviceKind === "other").length;
  return { rows, otherCount: hiddenNamed + others };
}

export function confidenceLabel(confidence: string): string {
  switch (confidence) {
    case "high":
      return "확실";
    case "medium":
      return "보통";
    default:
      return "추정";
  }
}

export function attributionLabel(process: GpuProcess): string {
  if (process.service) return process.service;
  if (process.serviceKind === "unknown_ai") return "Unknown AI workload";
  return process.processName;
}

export function modelsLabel(process: GpuProcess): string | null {
  if (process.models.length === 0) return null;
  return process.models.join(" · ");
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
