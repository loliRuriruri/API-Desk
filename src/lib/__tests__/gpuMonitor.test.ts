import { describe, expect, it } from "vitest";
import {
  attributionLabel,
  confidenceLabel,
  formatBytes,
  modelsLabel,
  topWorkloads,
  vramPercent,
  vramTone,
  type GpuInfo,
  type GpuProcess,
} from "../gpuMonitor";

function gpu(overrides: Partial<GpuInfo> = {}): GpuInfo {
  return {
    index: 0,
    name: "NVIDIA GeForce RTX 5090",
    memoryTotalBytes: 32 * 1024 ** 3,
    memoryUsedBytes: 8 * 1024 ** 3,
    memoryFreeBytes: 24 * 1024 ** 3,
    utilizationPercent: 2,
    temperatureC: 47,
    powerWatts: 46.2,
    ...overrides,
  };
}

function process(overrides: Partial<GpuProcess> = {}): GpuProcess {
  return {
    pid: 100,
    processName: "python.exe",
    usedVramBytes: null,
    service: null,
    serviceKind: "unknown",
    models: [],
    modelSource: null,
    confidence: "low",
    executable: "python.exe",
    isServiceRoot: false,
    ...overrides,
  };
}

describe("formatBytes", () => {
  it("formats GiB and never fakes missing values as zero", () => {
    expect(formatBytes(8 * 1024 ** 3)).toBe("8.0 GB");
    expect(formatBytes(1024 ** 3 / 2)).toBe("0.5 GB");
    expect(formatBytes(0)).toBe("0.0 GB");
    expect(formatBytes(null)).toBe("N/A");
    expect(formatBytes(undefined)).toBe("N/A");
    expect(formatBytes(Number.NaN)).toBe("N/A");
  });
});

describe("vram helpers", () => {
  it("computes usage percent and tones", () => {
    expect(vramPercent(gpu())).toBeCloseTo(25, 5);
    expect(vramPercent(gpu({ memoryUsedBytes: null }))).toBeNull();
    expect(vramPercent(null)).toBeNull();
    expect(vramTone(25)).toBe("ok");
    expect(vramTone(80)).toBe("warn");
    expect(vramTone(95)).toBe("danger");
    expect(vramTone(null)).toBe("muted");
  });
});

describe("topWorkloads", () => {
  it("keeps the top named workloads and aggregates the rest into Other", () => {
    const rows = [
      process({ pid: 1, service: "Laya", serviceKind: "managed_service" }),
      process({ pid: 2, service: "Ollama", serviceKind: "ollama" }),
      process({ pid: 3, service: "ComfyUI", serviceKind: "comfyui" }),
      process({ pid: 4, service: "vLLM", serviceKind: "vllm" }),
      process({ pid: 5, service: "Other", serviceKind: "other" }),
      process({ pid: 6, service: "Other", serviceKind: "other" }),
    ];
    const result = topWorkloads(rows, 3);
    expect(result.rows.map((row) => row.pid)).toEqual([1, 2, 3]);
    expect(result.otherCount).toBe(3); // 숨겨진 named 1 + other 2
    expect(topWorkloads(rows, 10).otherCount).toBe(2);
  });
});

describe("labels", () => {
  it("maps confidence and attribution to readable labels", () => {
    expect(confidenceLabel("high")).toBe("확실");
    expect(confidenceLabel("medium")).toBe("보통");
    expect(confidenceLabel("low")).toBe("추정");
    expect(attributionLabel(process({ service: "Laya" }))).toBe("Laya");
    expect(attributionLabel(process({ serviceKind: "unknown_ai" }))).toBe("Unknown AI workload");
    expect(attributionLabel(process({ processName: "dwm.exe" }))).toBe("dwm.exe");
  });

  it("joins model names and returns null when absent", () => {
    expect(modelsLabel(process({ models: ["english", "multilingual"] }))).toBe(
      "english · multilingual",
    );
    expect(modelsLabel(process())).toBeNull();
  });
});
