// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StartupGate } from "../../pages/StartupGate";
import type { VaultStatus } from "../../lib/vault";

vi.mock("../../lib/vault", () => ({
  vaultEnrollAutoUnlock: vi.fn(async () => {}),
  vaultRetryAutoUnlock: vi.fn(async () => {}),
  vaultUnlock: vi.fn(async () => {}),
}));

import { vaultEnrollAutoUnlock, vaultRetryAutoUnlock, vaultUnlock } from "../../lib/vault";

function status(overrides: Partial<VaultStatus> = {}): VaultStatus {
  return {
    state: "unlocked",
    autoUnlock: "ready",
    autoUnlockError: null,
    vaultPath: "C:/Vault/vault.hold",
    saltPath: "C:/Vault/vault.salt",
    ...overrides,
  };
}

function renderGate(value: VaultStatus, onEnter = vi.fn(), onUnlocked = vi.fn()) {
  render(<StartupGate status={value} onEnter={onEnter} onUnlocked={onUnlocked} />);
  return { onEnter, onUnlocked };
}

describe("StartupGate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });
  afterEach(() => cleanup());

  it("shows the splash with 시작 and no password input when unlocked", () => {
    const { onEnter } = renderGate(status());
    expect(screen.getByText("AI Runtime Dashboard")).toBeTruthy();
    expect(screen.getByText("시작")).toBeTruthy();
    expect(document.querySelector('input[type="password"]')).toBeNull();
    fireEvent.click(screen.getByText("시작"));
    expect(onEnter).toHaveBeenCalledTimes(1);
  });

  it("shows initializing state without a start button", () => {
    renderGate(status({ state: "locked", autoUnlock: "initializing" }));
    expect(screen.getByText(/초기화 중/)).toBeTruthy();
    expect(screen.queryByText("시작")).toBeNull();
  });

  it("legacy enrollment asks for the master password once and enrolls", async () => {
    const { onEnter, onUnlocked } = renderGate(
      status({ state: "locked", autoUnlock: "legacy" }),
    );
    expect(screen.getByText(/앞으로 이 PC에서는 비밀번호 없이 시작합니다/)).toBeTruthy();
    const input = document.querySelector('input[type="password"]') as HTMLInputElement;
    expect(input).toBeTruthy();
    fireEvent.change(input, { target: { value: "master-pass-1234" } });
    fireEvent.click(screen.getByText("이 PC에 자동 잠금해제 등록"));
    await vi.waitFor(() => expect(vaultEnrollAutoUnlock).toHaveBeenCalledWith("master-pass-1234"));
    expect(onUnlocked).toHaveBeenCalled();
    expect(onEnter).toHaveBeenCalled();
  });

  it("fails closed with recovery options when auto unlock failed", async () => {
    renderGate(
      status({
        state: "locked",
        autoUnlock: "failed",
        autoUnlockError: "자동 잠금해제 정보를 복호화할 수 없습니다",
      }),
    );
    expect(screen.getByText("자동 잠금해제를 사용할 수 없습니다")).toBeTruthy();
    expect(screen.getByText(/복호화할 수 없습니다/)).toBeTruthy();
    fireEvent.click(screen.getByText("다시 시도"));
    await vi.waitFor(() => expect(vaultRetryAutoUnlock).toHaveBeenCalled());
  });

  it("recovers with the legacy master password when provided", async () => {
    const { onEnter } = renderGate(
      status({ state: "locked", autoUnlock: "failed", autoUnlockError: "무결성 오류" }),
    );
    const input = document.querySelector('input[type="password"]') as HTMLInputElement;
    fireEvent.change(input, { target: { value: "legacy-pass" } });
    fireEvent.click(screen.getByText("기존 마스터 비밀번호로 복구"));
    await vi.waitFor(() => expect(vaultUnlock).toHaveBeenCalledWith("legacy-pass"));
    expect(onEnter).toHaveBeenCalled();
  });

  it("offers instant reopening after a session lock (ready but locked)", async () => {
    renderGate(status({ state: "locked", autoUnlock: "ready" }));
    expect(screen.getByText("Vault 세션 잠김")).toBeTruthy();
    fireEvent.click(screen.getByText("자동 잠금해제로 열기"));
    await vi.waitFor(() => expect(vaultRetryAutoUnlock).toHaveBeenCalled());
  });

  it("never renders a password value or leaks secrets in markup", () => {
    const { unmount } = render(
      <StartupGate
        status={status({ state: "locked", autoUnlock: "failed", autoUnlockError: "안내" })}
        onEnter={vi.fn()}
        onUnlocked={vi.fn()}
      />,
    );
    expect(document.body.innerHTML).not.toMatch(/sk-[A-Za-z0-9]/);
    unmount();
  });
});
