import { describe, expect, it } from "vitest";
import {
  candidateLabel,
  decodePreview,
  DEFAULT_TURZX_SETTINGS,
  pageLabel,
  portOptionLabel,
  statusSummary,
  type TurzxPortCandidate,
  type TurzxStatus,
} from "../turzxDisplay";

function status(overrides: Partial<TurzxStatus> = {}): TurzxStatus {
  return {
    enabled: true,
    connected: false,
    port: null,
    device: null,
    model: null,
    resolution: "320x480",
    orientation: "portrait",
    page: "single",
    lastError: null,
    lastFrameAt: null,
    lastFrameMs: null,
    lastBytesSent: null,
    lastUpdateKind: null,
    candidates: [],
    ...overrides,
  };
}

describe("turzx status helpers", () => {
  it("summarizes disconnected, disabled and connected states", () => {
    expect(statusSummary(null)).toBe("확인 중…");
    expect(statusSummary(status({ enabled: false }))).toBe("비활성");
    expect(statusSummary(status({ lastError: "포트 없음" }))).toBe("연결 안 됨 · 포트 없음");
    expect(
      statusSummary(
        status({ connected: true, port: "COM4", device: 'TURZX 3.5"', model: 'UsbMonitor 3.5"' }),
      ),
    ).toBe('TURZX 3.5" · COM4 · 320x480 · UsbMonitor 3.5"');
  });

  it("maps page modes to korean labels", () => {
    expect(pageLabel("single")).toBe("단일 대시보드");
    expect(pageLabel("runtime")).toBe("런타임(GPU/모델)");
    expect(pageLabel("api")).toBe("API 사용량");
    expect(pageLabel("rotate")).toBe("자동 로테이션");
    expect(pageLabel("unknown")).toBe("unknown");
  });

  it("labels port candidates and auto selection", () => {
    const candidate: TurzxPortCandidate = {
      port: "COM4",
      description: "USB Serial (VID 1A86:PID 5722)",
      serialNumber: "USB35INCHIPSV2",
      vid: 0x1a86,
      pid: 0x5722,
      manufacturer: null,
      score: 100,
      knownDevice: true,
    };
    expect(candidateLabel(candidate)).toBe("COM4 · TURZX 후보 · USB35INCHIPSV2");
    expect(portOptionLabel(DEFAULT_TURZX_SETTINGS, status({ candidates: [candidate] }))).toBe(
      "자동 (COM4)",
    );
    expect(portOptionLabel({ ...DEFAULT_TURZX_SETTINGS, port: "COM7" }, null)).toBe("수동 (COM7)");
  });

  it("decodes preview rgb into rgba with full alpha", () => {
    const preview = {
      width: 2,
      height: 1,
      page: "single",
      mode: "sample",
      rgbBase64: btoa(String.fromCharCode(1, 2, 3, 4, 5, 6)),
    };
    const rgba = decodePreview(preview);
    expect(rgba.length).toBe(2 * 1 * 4);
    expect(Array.from(rgba.slice(0, 4))).toEqual([1, 2, 3, 255]);
    expect(Array.from(rgba.slice(4, 8))).toEqual([4, 5, 6, 255]);
  });

  it("keeps defaults aligned with the backend contract", () => {
    expect(DEFAULT_TURZX_SETTINGS.refreshSecs).toBe(2);
    expect(DEFAULT_TURZX_SETTINGS.brightness).toBe(70);
    expect(DEFAULT_TURZX_SETTINGS.pageMode).toBe("single");
    expect(DEFAULT_TURZX_SETTINGS.pageRotationSecs).toBe(8);
    expect(DEFAULT_TURZX_SETTINGS.autoReconnect).toBe(true);
  });
});
