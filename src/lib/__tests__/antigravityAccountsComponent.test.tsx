// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { AntigravityAccounts } from "../../components/AntigravityAccounts";
import type { MonitorSnapshot } from "../db/repo";
import type { AntigravityAccount } from "../system";
import { setPendingAgSwitch } from "../monitorStore";

describe("AntigravityAccounts Component UI Behavior", () => {
  beforeEach(() => {
    setPendingAgSwitch(null);
  });

  afterEach(() => {
    cleanup();
  });

  const accountA: AntigravityAccount = {
    id: "acc-a",
    label: "a@example.com",
    email: "a@example.com",
    tier: "Google AI Pro",
    savedAt: "2026-09-20T00:00:00Z",
    isActive: true,
  };

  const accountB: AntigravityAccount = {
    id: "acc-b",
    label: "b@example.com",
    email: "b@example.com",
    tier: "Google AI Pro",
    savedAt: "2026-09-24T00:00:00Z",
    isActive: false,
  };

  const liveSnapshotA: MonitorSnapshot = {
    id: "snap-a",
    monitor: "antigravity",
    status: "ok",
    summary: "Google AI Pro · Gemini 80%",
    details_json: JSON.stringify({
      email: "a@example.com",
      tier: "Google AI Pro",
      geminiRemainingPercent: 80,
    }),
    fetched_at: "2026-09-29T10:00:00Z",
  };

  const historicalSnapshotB: MonitorSnapshot = {
    id: "snap-hist-b",
    monitor: "antigravity:b@example.com",
    status: "ok",
    summary: "Google AI Pro · Gemini 45%",
    details_json: JSON.stringify({
      email: "b@example.com",
      tier: "Google AI Pro",
      geminiRemainingPercent: 45,
    }),
    fetched_at: "2026-09-28T12:00:00Z",
  };

  it("shows '현재' for live account A and does NOT show '현재' for account B", () => {
    render(
      <AntigravityAccounts
        accounts={[accountA, accountB]}
        snapshots={[liveSnapshotA, historicalSnapshotB]}
        current={liveSnapshotA}
      />,
    );

    // Initial selected is live A
    expect(screen.getByText("현재")).toBeDefined();
    expect(screen.getByText("a@example.com")).toBeDefined();

    // Click tab for B
    const tabB = screen.getByTitle("b@example.com");
    fireEvent.click(tabB);

    // B must NOT have the '현재' badge!
    expect(screen.queryByText("현재")).toBeNull();
    // B must show "마지막 확인"
    expect(screen.getByText(/마지막 확인/)).toBeDefined();
  });

  it("shows '재시작 필요' badge and banner when pending switch is set for account B", () => {
    const onConfirm = vi.fn();
    render(
      <AntigravityAccounts
        accounts={[accountA, accountB]}
        snapshots={[liveSnapshotA, historicalSnapshotB]}
        current={liveSnapshotA}
        pendingSwitch={{
          accountId: "acc-b",
          email: "b@example.com",
          state: "restart_required",
          message: "자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다.",
        }}
        onConfirmSwitch={onConfirm}
      />,
    );

    // Click tab B
    const tabB = screen.getByTitle("b@example.com");
    fireEvent.click(tabB);

    // Shows '재시작 필요' badge
    expect(screen.getByText("재시작 필요")).toBeDefined();
    // Shows message
    expect(
      screen.getByText("자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다."),
    ).toBeDefined();

    // Has 'Antigravity 재시작 후 확인' button
    const confirmBtn = screen.getByRole("button", { name: /Antigravity 재시작 후 확인/i });
    expect(confirmBtn).toBeDefined();

    fireEvent.click(confirmBtn);
    expect(onConfirm).toHaveBeenCalledWith(accountB);
  });
});
