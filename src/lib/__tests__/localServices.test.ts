import { describe, expect, it } from "vitest";
import {
  apiKeyLabel,
  formatUptime,
  miniAuthLabel,
  serviceStateLabel,
  serviceStateTone,
  startBlockReason,
  type LocalServiceStatus,
} from "../localServices";

function status(overrides: Partial<LocalServiceStatus>): LocalServiceStatus {
  return {
    id: "laya",
    label: "Laya",
    description: "",
    state: "stopped",
    endpoint: "http://127.0.0.1:8000",
    port: 8000,
    device: "cuda",
    pid: null,
    managed: false,
    uptimeSecs: null,
    apiKeySet: false,
    apiKeyHint: null,
    apiKeyConfigured: false,
    vaultLocked: false,
    executable: "",
    executableSet: false,
    args: [],
    workdir: "",
    healthPath: "/health",
    autoStart: false,
    keepAliveOnExit: true,
    logLines: 0,
    lastError: null,
    ...overrides,
  };
}

describe("formatUptime", () => {
  it("formats days, hours and minutes", () => {
    expect(formatUptime(0)).toBe("0s");
    expect(formatUptime(45)).toBe("45s");
    expect(formatUptime(125)).toBe("2m 5s");
    expect(formatUptime(3_725)).toBe("1h 2m");
    expect(formatUptime(90_000)).toBe("1d 1h 0m");
  });

  it("returns a dash for unknown values", () => {
    expect(formatUptime(null)).toBe("—");
    expect(formatUptime(undefined)).toBe("—");
    expect(formatUptime(-5)).toBe("—");
  });
});

describe("service state helpers", () => {
  it("maps states to labels and tones", () => {
    expect(serviceStateLabel("running")).toBe("실행 중");
    expect(serviceStateLabel("stopped")).toBe("중지됨");
    expect(serviceStateTone("running")).toBe("ok");
    expect(serviceStateTone("starting")).toBe("warn");
    expect(serviceStateTone("error")).toBe("danger");
    expect(serviceStateTone("stopped")).toBe("muted");
  });
});

describe("apiKeyLabel", () => {
  it("never exposes the full key and marks the auth mode", () => {
    expect(apiKeyLabel(status({ apiKeySet: true, apiKeyHint: "••••abcd" }))).toBe(
      "설정됨 (••••abcd) · Bearer 인증",
    );
    expect(apiKeyLabel(status({ apiKeySet: true, apiKeyHint: null }))).toBe(
      "설정됨 · Bearer 인증",
    );
    expect(apiKeyLabel(status({ apiKeySet: false }))).toBe("미설정 — localhost 전용 무인증");
    expect(apiKeyLabel(status({ apiKeySet: false, vaultLocked: true }))).toBe(
      "Vault 잠금 — 무인증 모드로 실행",
    );
  });
});

describe("mini summary helpers", () => {
  it("labels the auth mode without exposing the key", () => {
    expect(miniAuthLabel(status({ apiKeySet: true, apiKeyHint: "••••abcd" }))).toBe("Bearer 인증");
    expect(miniAuthLabel(status({ apiKeyConfigured: true, vaultLocked: true }))).toBe("Vault 잠금");
    expect(miniAuthLabel(status({}))).toBe("무인증 (localhost)");
  });

  it("blocks Start only when a configured key is unavailable", () => {
    expect(startBlockReason(status({}))).toBe("실행 파일 경로 미설정");
    expect(
      startBlockReason(status({ executableSet: true, apiKeyConfigured: true, vaultLocked: true })),
    ).toBe("Vault 잠금 해제 필요");
    expect(
      startBlockReason(status({ executableSet: true, apiKeyConfigured: true, vaultLocked: false })),
    ).toBe("저장된 키를 찾을 수 없음");
    expect(startBlockReason(status({ executableSet: true }))).toBeNull();
    expect(
      startBlockReason(status({ executableSet: true, vaultLocked: true, apiKeyConfigured: false })),
    ).toBeNull();
  });
});