import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  persistMonitorOutcome,
  getEffectiveAntigravitySnapshot,
  isLiveAntigravityAccount,
  antigravityAccountSnapshots,
  setPendingAgSwitch,
  getPendingAgSwitch,
  type PendingSwitchState,
} from "../monitorStore";
import type { MonitorSnapshot } from "../db/repo";
import * as repo from "../db/repo";

vi.mock("../db/repo", () => ({
  upsertMonitorSnapshot: vi.fn().mockResolvedValue(undefined),
}));

describe("Antigravity Account Switch & Monitor Sync (Spec Cases A-H)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setPendingAgSwitch(null);
  });

  const snapshotA: MonitorSnapshot = {
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

  const snapshotB: MonitorSnapshot = {
    id: "snap-b",
    monitor: "antigravity",
    status: "ok",
    summary: "Google AI Pro · Gemini 45%",
    details_json: JSON.stringify({
      email: "b@example.com",
      tier: "Google AI Pro",
      geminiRemainingPercent: 45,
    }),
    fetched_at: "2026-09-29T10:05:00Z",
  };

  const historicalB: MonitorSnapshot = {
    id: "snap-hist-b",
    monitor: "antigravity:b@example.com",
    status: "ok",
    summary: "Google AI Pro · Gemini 50%",
    details_json: JSON.stringify({
      email: "b@example.com",
      tier: "Google AI Pro",
      geminiRemainingPercent: 50,
    }),
    fetched_at: "2026-09-28T12:00:00Z",
  };

  const historicalA: MonitorSnapshot = {
    id: "snap-hist-a",
    monitor: "antigravity:a@example.com",
    status: "ok",
    summary: "Google AI Pro · Gemini 80%",
    details_json: JSON.stringify({
      email: "a@example.com",
      tier: "Google AI Pro",
      geminiRemainingPercent: 80,
    }),
    fetched_at: "2026-09-29T10:00:00Z",
  };

  const histMap = new Map<string, MonitorSnapshot>([
    ["a@example.com", historicalA],
    ["b@example.com", historicalB],
  ]);

  it("Case A: live=A, requested switch=B, server still=A -> A remains current, B=restart_required, B does NOT display A snapshot", () => {
    const liveEmail = "a@example.com";
    const requestedEmail = "b@example.com";

    // Credential written, server still returns A
    expect(isLiveAntigravityAccount(liveEmail, liveEmail)).toBe(true);
    expect(isLiveAntigravityAccount(requestedEmail, liveEmail)).toBe(false);

    // Pending switch state is restart_required
    const pendingState: PendingSwitchState = {
      accountId: "acc-b",
      email: requestedEmail,
      state: "restart_required",
      liveEmailBefore: liveEmail,
      liveEmailAfter: liveEmail,
      message: "자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다.",
    };
    setPendingAgSwitch(pendingState);

    expect(getPendingAgSwitch()?.state).toBe("restart_required");

    // Snapshot attribution for B must be historical B, NEVER snapshot A!
    const effectiveB = getEffectiveAntigravitySnapshot(
      requestedEmail,
      liveEmail,
      snapshotA,
      histMap,
    );
    expect(effectiveB).toEqual(historicalB);
    expect(effectiveB?.details_json).toContain("b@example.com");
    expect(effectiveB?.details_json).not.toContain("a@example.com");

    // Snapshot attribution for A is the live snapshot A
    const effectiveA = getEffectiveAntigravitySnapshot(
      liveEmail,
      liveEmail,
      snapshotA,
      histMap,
    );
    expect(effectiveA).toEqual(snapshotA);
  });

  it("Case B: live=A, requested=B, after restart live=B -> B confirmed current, fresh B quota fetched, A becomes historical", () => {
    // Before restart: live was A
    const beforeLive = "a@example.com";
    expect(isLiveAntigravityAccount("b@example.com", beforeLive)).toBe(false);

    // After restart: language server reloaded, live is now B
    const afterLive = "b@example.com";
    expect(isLiveAntigravityAccount("b@example.com", afterLive)).toBe(true);
    expect(isLiveAntigravityAccount("a@example.com", afterLive)).toBe(false);

    // B confirmed current receives fresh live snapshot B
    const effectiveB = getEffectiveAntigravitySnapshot(
      "b@example.com",
      afterLive,
      snapshotB,
      histMap,
    );
    expect(effectiveB).toEqual(snapshotB);
    expect(effectiveB?.details_json).toContain("b@example.com");

    // A becomes historical
    const effectiveA = getEffectiveAntigravitySnapshot(
      "a@example.com",
      afterLive,
      snapshotB,
      histMap,
    );
    expect(effectiveA).toEqual(historicalA);
    expect(effectiveA?.details_json).toContain("a@example.com");
  });

  it("Case C: requested=B, after restart live=C -> mismatch, B not current, C detected as actual", () => {
    const requestedEmail = "b@example.com";
    const actualLiveEmail = "c@example.com";
    const snapshotC: MonitorSnapshot = {
      id: "snap-c",
      monitor: "antigravity",
      status: "ok",
      summary: "Google AI Pro · Gemini 10%",
      details_json: JSON.stringify({ email: actualLiveEmail, geminiRemainingPercent: 10 }),
      fetched_at: "2026-09-29T10:10:00Z",
    };

    // B is not current
    expect(isLiveAntigravityAccount(requestedEmail, actualLiveEmail)).toBe(false);
    // C is actual
    expect(isLiveAntigravityAccount(actualLiveEmail, actualLiveEmail)).toBe(true);

    // B does not receive C's snapshot
    const effectiveB = getEffectiveAntigravitySnapshot(
      requestedEmail,
      actualLiveEmail,
      snapshotC,
      histMap,
    );
    expect(effectiveB).toEqual(historicalB);
    expect(effectiveB).not.toEqual(snapshotC);
  });

  it("Case D: account.isActive=true for B while live snapshot email=A -> B must NOT receive current snapshot", () => {
    const liveEmail = "a@example.com";
    const accountB = {
      id: "acc-b",
      label: "b@example.com",
      email: "b@example.com",
      tier: "Google AI Pro",
      savedAt: "2026-09-20T00:00:00Z",
      isActive: true, // Legacy buggy flag simulation
    };

    // Even though accountB.isActive is true, snapshot must NOT be snapshotA!
    const effective = getEffectiveAntigravitySnapshot(
      accountB.email,
      liveEmail,
      snapshotA,
      histMap,
    );
    expect(effective).not.toEqual(snapshotA);
    expect(effective).toEqual(historicalB);

    // And isLive must be false!
    expect(isLiveAntigravityAccount(accountB.email, liveEmail)).toBe(false);
  });

  it("Case E: saved snapshot antigravity:B -> B historical tab receives only B snapshot", () => {
    const allSnapshots: MonitorSnapshot[] = [snapshotA, historicalA, historicalB];
    const map = antigravityAccountSnapshots(allSnapshots);

    expect(map.size).toBe(2);
    expect(map.get("b@example.com")).toEqual(historicalB);
    expect(map.get("a@example.com")).toEqual(historicalA);

    // B gets strictly B's historical snapshot
    const effectiveB = getEffectiveAntigravitySnapshot(
      "b@example.com",
      null, // server offline
      null,
      map,
    );
    expect(effectiveB).toEqual(historicalB);
  });

  it("Case G: fresh B quota persistence -> global antigravity email=B and antigravity:B email=B", async () => {
    const outcomeB = {
      monitor: "antigravity",
      status: "ok",
      summary: "Google AI Pro · Gemini 45%",
      details: {
        email: "b@example.com",
        tier: "Google AI Pro",
        geminiRemainingPercent: 45,
      },
      message: null,
    };

    await persistMonitorOutcome(outcomeB);

    // Global monitor write
    expect(repo.upsertMonitorSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({
        monitor: "antigravity",
        status: "ok",
        summary: "Google AI Pro · Gemini 45%",
        details_json: expect.stringContaining('"email":"b@example.com"'),
      }),
    );

    // Account specific monitor write
    expect(repo.upsertMonitorSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({
        monitor: "antigravity:b@example.com",
        status: "ok",
        summary: "Google AI Pro · Gemini 45%",
        details_json: expect.stringContaining('"email":"b@example.com"'),
      }),
    );
  });

  it("Case H: no cross-account snapshot contamination: rejecting mismatched identity on per-account key", async () => {
    // Attempting to persist under antigravity:b@example.com with details belonging to a@example.com
    const contaminatedOutcome = {
      monitor: "antigravity:b@example.com",
      status: "ok",
      summary: "Contaminated",
      details: {
        email: "a@example.com", // Mismatched!
        geminiRemainingPercent: 99,
      },
      message: null,
    };

    await persistMonitorOutcome(contaminatedOutcome);

    // Must NOT call upsertMonitorSnapshot because emails mismatch!
    expect(repo.upsertMonitorSnapshot).not.toHaveBeenCalled();
  });
});
