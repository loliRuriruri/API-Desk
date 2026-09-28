// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GpuMonitorPanel } from "../../components/GpuMonitorPanel";
import { runtimeNotice } from "../../lib/gpuMonitor";

const gpuSnapshot = vi.fn();
const gpuRuntimeStatus = vi.fn();
const gpuRestartSampler = vi.fn();

vi.mock("../../lib/gpuMonitor", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/gpuMonitor")>();
  return {
    ...actual,
    gpuSnapshot: (...args: unknown[]) => gpuSnapshot(...args),
    gpuRuntimeStatus: (...args: unknown[]) => gpuRuntimeStatus(...args),
    gpuRestartSampler: (...args: unknown[]) => gpuRestartSampler(...args),
  };
});

function snapshot(overrides: Record<string, unknown> = {}) {
  return {
    available: true,
    reason: null,
    source: "nvml",
    fetchedAt: "2026-09-29T00:00:00Z",
    gpus: [
      {
        index: 0,
        name: "NVIDIA GeForce RTX 5090",
        memoryTotalBytes: 34_000_000_000,
        memoryUsedBytes: 10_000_000_000,
        memoryFreeBytes: 24_000_000_000,
        utilizationPercent: 7,
        temperatureC: 50,
        powerWatts: 70,
      },
    ],
    processes: [],
    otherCount: 0,
    detail: null,
    ...overrides,
  };
}

function runtime(state: string, lastError: string | null = null) {
  return { state, lastSampleAt: null, lastError, nvmlReady: state === "ready", pdhReady: state === "ready", samples: 1 };
}

describe("GpuMonitorPanel resident states", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });
  afterEach(() => cleanup());

  it("shows a bounded starting state instead of an infinite spinner", async () => {
    gpuSnapshot.mockReturnValue(new Promise(() => {}));
    gpuRuntimeStatus.mockResolvedValue(runtime("starting"));
    render(<GpuMonitorPanel />);
    expect(await screen.findByText("GPU 모니터 시작 중…")).toBeTruthy();
    // 시작 중에는 재시도 버튼을 노출하지 않는다(5초 타임아웃 전).
    expect(screen.queryByText("GPU 모니터 재시도")).toBeNull();
  });

  it("surfaces the backend error with a retry button after the timeout", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    gpuSnapshot.mockReturnValue(new Promise(() => {}));
    gpuRuntimeStatus.mockResolvedValue(runtime("error", "NVML initialization failed"));
    gpuRestartSampler.mockResolvedValue(runtime("starting"));
    render(<GpuMonitorPanel />);
    await vi.advanceTimersByTimeAsync(6_000);
    await waitFor(() =>
      expect(screen.getByText(/NVML initialization failed/)).toBeTruthy(),
    );
    fireEvent.click(screen.getByText("GPU 모니터 재시도"));
    await vi.waitFor(() => expect(gpuRestartSampler).toHaveBeenCalledTimes(1));
  });

  it("renders telemetry once the sampler is ready", async () => {
    gpuSnapshot.mockResolvedValue(snapshot());
    gpuRuntimeStatus.mockResolvedValue(runtime("ready"));
    render(<GpuMonitorPanel />);
    expect(await screen.findByText("NVIDIA GeForce RTX 5090")).toBeTruthy();
    expect(screen.queryByText("GPU 모니터 시작 중…")).toBeNull();
  });

  it("maps runtime notices for mini/panel", () => {
    expect(runtimeNotice(runtime("ready"))).toBeNull();
    expect(runtimeNotice(runtime("starting"))).toBe("GPU 모니터 시작 중…");
    expect(runtimeNotice(runtime("degraded", "PDH 지연"))).toContain("경고");
    expect(runtimeNotice(runtime("error", "NVML 실패"))).toContain("NVML 실패");
  });
});
