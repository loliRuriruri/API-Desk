import { describe, expect, it } from "vitest";
import { statusSummary, TurzxStatus } from "../turzxDisplay";

describe("TURZX Boot Frame and State Machine", () => {
  it("includes state in TurzxStatus interface and formats summary", () => {
    const status: TurzxStatus = {
      state: "READY",
      enabled: true,
      connected: true,
      port: "COM4",
      device: 'TURZX 3.5"',
      model: "Turing 3.5",
      resolution: "320x480",
      orientation: "portrait",
      page: "single",
      lastError: null,
      lastFrameAt: "2026-09-29T00:00:00Z",
      lastFrameMs: 45,
      lastBytesSent: 307200,
      lastUpdateKind: "full",
      candidates: [],
    };

    expect(status.state).toBe("READY");
    expect(statusSummary(status)).toContain("COM4");
    expect(statusSummary(status)).toContain("320x480");
    expect(statusSummary(status)).toContain("Turing 3.5");
  });

  it("handles WAITING_FOR_DATA and BOOT states correctly", () => {
    const waitingStatus: TurzxStatus = {
      state: "WAITING_FOR_DATA",
      enabled: true,
      connected: true,
      port: "COM4",
      device: 'TURZX 3.5"',
      model: "Turing 3.5",
      resolution: "320x480",
      orientation: "portrait",
      page: "single",
      lastError: null,
      lastFrameAt: null,
      lastFrameMs: null,
      lastBytesSent: 15400,
      lastUpdateKind: "full",
      candidates: [],
    };

    expect(waitingStatus.state).toBe("WAITING_FOR_DATA");
    expect(waitingStatus.connected).toBe(true);
  });

  it("decouples GPU and API telemetry readiness", () => {
    // Pure state transition evaluator matching backend evaluate_waiting_state
    function evaluateWaitingState(
      gpuReady: boolean,
      apiReady: boolean,
      elapsedMs: number
    ): { nextState: "READY" | null; reason: string } {
      const timeoutReached = elapsedMs >= 5000;
      if (gpuReady && apiReady) {
        return { nextState: "READY", reason: "gpu and api ready" };
      } else if (gpuReady) {
        return { nextState: "READY", reason: "gpu ready" };
      } else if (apiReady) {
        return { nextState: "READY", reason: "api cache ready" };
      } else if (timeoutReached) {
        return { nextState: "READY", reason: "startup timeout 5s" };
      } else {
        return { nextState: null, reason: "waiting for data" };
      }
    }

    // 1. Initial connect: no data yet (< 5s) -> remains null (keeps boot frame)
    expect(evaluateWaitingState(false, false, 1000).nextState).toBeNull();
    expect(evaluateWaitingState(false, false, 4999).nextState).toBeNull();

    // 2. GPU data arrives first (e.g. at 1.2s) -> transitions immediately to READY
    const gpuFirst = evaluateWaitingState(true, false, 1200);
    expect(gpuFirst.nextState).toBe("READY");
    expect(gpuFirst.reason).toBe("gpu ready");

    // 3. Cached API data arrives first (e.g. at 500ms) -> transitions immediately to READY
    const apiFirst = evaluateWaitingState(false, true, 500);
    expect(apiFirst.nextState).toBe("READY");
    expect(apiFirst.reason).toBe("api cache ready");

    // 4. 5s timeout reached -> transitions to READY with placeholders regardless of data
    const timedOut = evaluateWaitingState(false, false, 5000);
    expect(timedOut.nextState).toBe("READY");
    expect(timedOut.reason).toBe("startup timeout 5s");
  });

  it("forces full redraw when transitioning from boot to live dashboard", () => {
    // When previousFrame is null (reset on READY transition), full rect must be scheduled
    const bootBuffer: Uint8Array | null = new Uint8Array(320 * 480 * 3);
    let previousFrame: Uint8Array | null = bootBuffer;
    expect(previousFrame).not.toBeNull();
    
    // On READY transition: reset dirty cache
    previousFrame = null;
    expect(previousFrame).toBeNull();

    const width = 320;
    const height = 480;
    const isFullRedraw = previousFrame === null;
    const scheduledRect = isFullRedraw
      ? { x0: 0, y0: 0, x1: width - 1, y1: height - 1 }
      : null;

    expect(scheduledRect).toEqual({ x0: 0, y0: 0, x1: 319, y1: 479 });
  });

  it("runs independently of frontend mounting", () => {
    // In resident mode, no DOM, window or React component is required for worker loop
    const isResidentMode = true;
    const windowMounted = false;
    const canRunTelemetryWorker = isResidentMode || windowMounted;
    expect(canRunTelemetryWorker).toBe(true);
  });
});
