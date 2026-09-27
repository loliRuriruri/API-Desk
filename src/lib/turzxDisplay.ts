import { invoke } from "@tauri-apps/api/core";

export interface TurzxSettings {
  enabled: boolean;
  port: string;
  orientation: string;
  rotation: number;
  brightness: number;
  refreshSecs: number;
  pageMode: string;
  pageRotationSecs: number;
  autoReconnect: boolean;
  launchWithApp: boolean;
  deviceLabel: string;
}

export interface TurzxPortCandidate {
  port: string;
  description: string;
  serialNumber: string | null;
  vid: number | null;
  pid: number | null;
  manufacturer: string | null;
  score: number;
  knownDevice: boolean;
}

export interface TurzxStatus {
  enabled: boolean;
  connected: boolean;
  port: string | null;
  device: string | null;
  model: string | null;
  resolution: string;
  orientation: string;
  page: string;
  lastError: string | null;
  lastFrameAt: string | null;
  lastFrameMs: number | null;
  lastBytesSent: number | null;
  lastUpdateKind: string | null;
  candidates: TurzxPortCandidate[];
}

export interface TurzxPreview {
  width: number;
  height: number;
  page: string;
  mode: string;
  rgbBase64: string;
}

export const DEFAULT_TURZX_SETTINGS: TurzxSettings = {
  enabled: false,
  port: "auto",
  orientation: "portrait",
  rotation: 0,
  brightness: 70,
  refreshSecs: 2,
  pageMode: "single",
  pageRotationSecs: 8,
  autoReconnect: true,
  launchWithApp: true,
  deviceLabel: 'TURZX 3.5"',
};

export const PAGE_MODES: Array<{ value: string; label: string }> = [
  { value: "single", label: "단일 대시보드" },
  { value: "runtime", label: "런타임(GPU/모델)" },
  { value: "api", label: "API 사용량" },
  { value: "rotate", label: "자동 로테이션" },
];

export const ORIENTATIONS: Array<{ value: string; label: string }> = [
  { value: "portrait", label: "세로 (320x480)" },
  { value: "landscape", label: "가로 (480x320)" },
];

export function statusSummary(status: TurzxStatus | null): string {
  if (!status) return "확인 중…";
  if (!status.enabled) return "비활성";
  if (!status.connected) return status.lastError ? `연결 안 됨 · ${status.lastError}` : "연결 안 됨";
  const parts = [status.device ?? "TURZX", status.port ?? "-", status.resolution];
  if (status.model) parts.push(status.model);
  return parts.join(" · ");
}

export function pageLabel(page: string): string {
  return PAGE_MODES.find((item) => item.value === page)?.label ?? page;
}

export function candidateLabel(candidate: TurzxPortCandidate): string {
  const known = candidate.knownDevice ? " · TURZX 후보" : "";
  const serial = candidate.serialNumber ? ` · ${candidate.serialNumber}` : "";
  return `${candidate.port}${known}${serial}`;
}

export function portOptionLabel(settings: TurzxSettings, status: TurzxStatus | null): string {
  if (settings.port === "auto") {
    const best = status?.candidates?.[0];
    return best ? `자동 (${best.port})` : "자동";
  }
  return `수동 (${settings.port})`;
}

/** base64 RGB → RGBA 픽셀 버퍼(canvas용). */
export function decodePreview(preview: TurzxPreview): Uint8ClampedArray {
  const binary = atob(preview.rgbBase64);
  const rgba = new Uint8ClampedArray(preview.width * preview.height * 4);
  for (let index = 0, offset = 0; index < binary.length; index += 3, offset += 4) {
    rgba[offset] = binary.charCodeAt(index);
    rgba[offset + 1] = binary.charCodeAt(index + 1);
    rgba[offset + 2] = binary.charCodeAt(index + 2);
    rgba[offset + 3] = 255;
  }
  return rgba;
}

export async function loadTurzxSettings(): Promise<TurzxSettings> {
  return invoke<TurzxSettings>("turzx_settings");
}

export async function saveTurzxSettings(settings: TurzxSettings): Promise<TurzxSettings> {
  return invoke<TurzxSettings>("turzx_save_settings", { settings });
}

export async function fetchTurzxStatus(): Promise<TurzxStatus> {
  return invoke<TurzxStatus>("turzx_status");
}

export async function detectTurzxPorts(): Promise<TurzxPortCandidate[]> {
  return invoke<TurzxPortCandidate[]>("turzx_detect");
}

export async function testTurzx(): Promise<TurzxStatus> {
  return invoke<TurzxStatus>("turzx_test");
}

export async function connectTurzx(): Promise<TurzxSettings> {
  return invoke<TurzxSettings>("turzx_connect");
}

export async function disconnectTurzx(): Promise<TurzxSettings> {
  return invoke<TurzxSettings>("turzx_disconnect");
}

export async function fetchTurzxPreview(
  mode: "live" | "sample",
  page?: string,
): Promise<TurzxPreview> {
  return invoke<TurzxPreview>("turzx_preview", { mode, page: page ?? null });
}

export async function exportTurzxPreview(path?: string): Promise<string> {
  return invoke<string>("turzx_export_preview", { path: path ?? null });
}
