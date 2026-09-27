import { describe, expect, it } from "vitest";
import {
  attributionLabel,
  classificationLabel,
  classificationTone,
  confidenceLabel,
  engineLabel,
  formatBytes,
  isSignificantAi,
  modelsLabel,
  runtimeLabel,
  sortProcesses,
  sourcesLabel,
  topWorkloads,
  vramPercent,
  vramSourceLabel,
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
    parentPid: null,
    processName: "python.exe",
    displayName: "python.exe",
    productName: null,
    executable: "python.exe",
    classification: "unknown",
    runtime: null,
    models: [],
    modelSource: null,
    gpuPercent: null,
    dominantEngine: null,
    usedVramBytes: null,
    dedicatedVramBytes: null,
    sharedGpuBytes: null,
    vramSource: null,
    service: null,
    serviceKind: "application",
    managed: false,
    isServiceRoot: false,
    confidence: "low",
    sources: [],
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

  it("labels the vram source", () => {
    expect(vramSourceLabel("windowsPdh")).toBe("PDH");
    expect(vramSourceLabel("nvml")).toBe("NVML");
    expect(vramSourceLabel(null)).toBeNull();
  });
});

describe("engine and classification labels", () => {
  it("maps engine names to short labels", () => {
    expect(engineLabel("3D")).toBe("3D");
    expect(engineLabel("Compute")).toBe("Compute");
    expect(engineLabel("VideoDecode")).toBe("Decode");
    expect(engineLabel("VideoEncode")).toBe("Encode");
    expect(engineLabel(null)).toBe("N/A");
  });

  it("maps classifications and never calls unknown processes AI", () => {
    expect(classificationLabel("ai")).toBe("AI");
    expect(classificationLabel("game")).toBe("게임");
    expect(classificationLabel("graphics")).toBe("그래픽");
    expect(classificationLabel("browser")).toBe("브라우저");
    expect(classificationLabel("video")).toBe("영상");
    expect(classificationLabel("system")).toBe("시스템");
    expect(classificationLabel("unknown")).toBe("알 수 없음");
    expect(classificationTone("ai")).toBe("info");
    expect(classificationTone("game")).toBe("ok");
    expect(classificationTone("graphics")).toBe("muted");
  });

  it("maps confidence levels", () => {
    expect(confidenceLabel("exact")).toBe("확실");
    expect(confidenceLabel("high")).toBe("높음");
    expect(confidenceLabel("medium")).toBe("보통");
    expect(confidenceLabel("low")).toBe("추정");
  });
});

describe("sortProcesses", () => {
  const rows = [
    process({ pid: 1, gpuPercent: 10, usedVramBytes: 9 * 1024 ** 3 }),
    process({ pid: 2, gpuPercent: 80, usedVramBytes: 100 }),
    process({ pid: 3, gpuPercent: 80, usedVramBytes: 5 * 1024 ** 3 }),
    process({ pid: 4, gpuPercent: null, usedVramBytes: 20 * 1024 ** 3 }),
  ];

  it("sorts by GPU percent then VRAM by default", () => {
    expect(sortProcesses(rows).map((row) => row.pid)).toEqual([3, 2, 1, 4]);
  });

  it("supports a VRAM-first toggle", () => {
    expect(sortProcesses(rows, "vram").map((row) => row.pid)).toEqual([4, 1, 3, 2]);
  });
});

describe("topWorkloads", () => {
  it("prefers GPU load, excludes system rows and preserves a significant AI workload", () => {
    const rows = [
      process({ pid: 1, displayName: "dwm.exe", classification: "system", gpuPercent: 40 }),
      process({ pid: 2, displayName: "Game", classification: "game", gpuPercent: 90 }),
      process({ pid: 3, displayName: "msedge.exe", classification: "browser", gpuPercent: 20 }),
      process({ pid: 4, displayName: "Photoshop", classification: "graphics", gpuPercent: 10 }),
      process({
        pid: 5,
        displayName: "Ollama",
        classification: "ai",
        gpuPercent: 5,
        models: ["qwen3.5:9b"],
        usedVramBytes: 9 * 1024 ** 3,
      }),
    ];
    const result = topWorkloads(rows, 3);
    const names = result.rows.map((row) => row.displayName);
    expect(names).toContain("Game");
    expect(names).toContain("Ollama");
    expect(names).not.toContain("dwm.exe");
    expect(result.otherCount).toBe(1);
  });

  it("keeps pure GPU order when an AI row is already visible", () => {
    const rows = [
      process({ pid: 1, displayName: "Game", classification: "game", gpuPercent: 90 }),
      process({
        pid: 2,
        displayName: "Ollama",
        classification: "ai",
        gpuPercent: 30,
        models: ["qwen3.5:9b"],
      }),
      process({ pid: 3, displayName: "Browser", classification: "browser", gpuPercent: 10 }),
    ];
    expect(topWorkloads(rows, 2).rows.map((row) => row.pid)).toEqual([1, 2]);
  });
});

describe("labels", () => {
  it("maps attribution and runtime to readable labels", () => {
    expect(attributionLabel(process({ displayName: "Laya", service: "Laya" }))).toBe("Laya");
    expect(attributionLabel(process({ displayName: "", service: null, processName: "dwm.exe" }))).toBe(
      "dwm.exe",
    );
    expect(attributionLabel(process({ displayName: "", service: null, processName: "" }))).toBe(
      "PID 100",
    );
    expect(runtimeLabel(process({ runtime: "PyTorch / CUDA", classification: "ai" }))).toBe(
      "PyTorch / CUDA",
    );
    expect(runtimeLabel(process({ runtime: null, classification: "graphics" }))).toBe("그래픽");
  });

  it("joins model names and returns null when absent", () => {
    expect(modelsLabel(process({ models: ["english", "multilingual"] }))).toBe(
      "english · multilingual",
    );
    expect(modelsLabel(process())).toBeNull();
  });

  it("labels evidence sources", () => {
    expect(sourcesLabel(["nvmlCompute", "windowsPdh"])).toBe("NVML-C+PDH");
    expect(sourcesLabel([])).toBe("");
  });
});

describe("isSignificantAi", () => {
  it("requires AI classification plus model or meaningful VRAM", () => {
    expect(isSignificantAi(process({ classification: "ai", models: ["m"] }))).toBe(true);
    expect(
      isSignificantAi(process({ classification: "ai", usedVramBytes: 2 * 1024 ** 3 })),
    ).toBe(true);
    expect(isSignificantAi(process({ classification: "ai" }))).toBe(false);
    expect(
      isSignificantAi(process({ classification: "graphics", usedVramBytes: 4 * 1024 ** 3 })),
    ).toBe(false);
  });
});
