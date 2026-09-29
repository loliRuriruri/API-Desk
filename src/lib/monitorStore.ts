import { upsertMonitorSnapshot, type MonitorSnapshot } from "./db/repo";
import {
  type MonitorOutcome,
  type AntigravityAccount,
  type AntigravitySwitchResult,
  antigravitySwitchAccount,
  antigravityConfirmSwitch,
  monitorRefresh,
} from "./system";

export async function persistMonitorOutcome(outcome: MonitorOutcome): Promise<void> {
  const fetchedAt = new Date().toISOString();
  const summary = outcome.summary || outcome.message || "";
  const detailsJson = outcome.details ? JSON.stringify(outcome.details) : null;

  if (outcome.monitor === "antigravity") {
    const details = outcome.details as { email?: string } | null;
    const email = details?.email?.trim();

    await upsertMonitorSnapshot({
      monitor: outcome.monitor,
      status: outcome.status,
      summary,
      details_json: detailsJson,
      fetched_at: fetchedAt,
    });

    if (email) {
      await upsertMonitorSnapshot({
        monitor: `antigravity:${email}`,
        status: outcome.status,
        summary,
        details_json: detailsJson,
        fetched_at: fetchedAt,
      });
    }
  } else if (outcome.monitor.startsWith("antigravity:")) {
    const targetEmail = outcome.monitor.slice("antigravity:".length).trim().toLowerCase();
    const details = outcome.details as { email?: string } | null;
    const email = details?.email?.trim();
    if (email && email.toLowerCase() === targetEmail) {
      await upsertMonitorSnapshot({
        monitor: outcome.monitor,
        status: outcome.status,
        summary,
        details_json: detailsJson,
        fetched_at: fetchedAt,
      });
    } else {
      console.warn(
        `Snapshot identity mismatch: target=${targetEmail}, details=${email ?? "none"} - skipping snapshot persistence`,
      );
    }
  } else {
    await upsertMonitorSnapshot({
      monitor: outcome.monitor,
      status: outcome.status,
      summary,
      details_json: detailsJson,
      fetched_at: fetchedAt,
    });
  }
}

export function antigravityAccountSnapshots(
  snapshots: MonitorSnapshot[],
): Map<string, MonitorSnapshot> {
  const map = new Map<string, MonitorSnapshot>();
  for (const snapshot of snapshots) {
    if (snapshot.monitor.startsWith("antigravity:")) {
      map.set(snapshot.monitor.slice("antigravity:".length), snapshot);
    }
  }
  return map;
}

export function getEffectiveAntigravitySnapshot(
  email: string,
  liveCurrentEmail: string | null,
  currentSnapshot: MonitorSnapshot | null,
  historicalSnapshots: Map<string, MonitorSnapshot>,
): MonitorSnapshot | null {
  if (liveCurrentEmail && email.trim().toLowerCase() === liveCurrentEmail.trim().toLowerCase()) {
    return currentSnapshot;
  }
  return historicalSnapshots.get(email) ?? null;
}

export function isLiveAntigravityAccount(
  email: string,
  liveCurrentEmail: string | null,
): boolean {
  if (!liveCurrentEmail) return false;
  return email.trim().toLowerCase() === liveCurrentEmail.trim().toLowerCase();
}

export interface PendingSwitchState {
  accountId: string;
  email: string;
  state: "credential_updated" | "confirmed" | "restart_required" | "polling" | "timeout" | "mismatch";
  message?: string | null;
  liveEmailBefore?: string | null;
  liveEmailAfter?: string | null;
}

let currentPendingSwitch: PendingSwitchState | null = null;
const pendingSwitchListeners = new Set<(state: PendingSwitchState | null) => void>();

export function getPendingAgSwitch(): PendingSwitchState | null {
  return currentPendingSwitch;
}

export function setPendingAgSwitch(state: PendingSwitchState | null): void {
  currentPendingSwitch = state;
  for (const listener of pendingSwitchListeners) {
    try {
      listener(state);
    } catch (e) {
      console.error(e);
    }
  }
}

export function subscribePendingAgSwitch(
  listener: (state: PendingSwitchState | null) => void,
): () => void {
  pendingSwitchListeners.add(listener);
  listener(currentPendingSwitch);
  return () => {
    pendingSwitchListeners.delete(listener);
  };
}

export async function runAntigravitySwitch(
  account: AntigravityAccount,
  callbacks: {
    onNotify?: (message: string, tone: "success" | "warn" | "error" | "info") => void;
    onReload?: () => Promise<void> | void;
  } = {},
): Promise<AntigravitySwitchResult> {
  const accountEmail = account.email ?? account.label;
  setPendingAgSwitch({
    accountId: account.id,
    email: accountEmail,
    state: "credential_updated",
    message: "계정 자격 증명을 전환했습니다. Antigravity 세션 전환 확인 중…",
  });

  const res = await antigravitySwitchAccount(account.id);
  await callbacks.onReload?.();

  if (res.state === "confirmed") {
    if (res.outcome) {
      await persistMonitorOutcome(res.outcome);
    } else {
      const outcome = await monitorRefresh("antigravity");
      await persistMonitorOutcome(outcome);
    }
    setPendingAgSwitch(null);
    callbacks.onNotify?.(`${accountEmail} 계정 전환 완료`, "success");
    await callbacks.onReload?.();
    return res;
  }

  setPendingAgSwitch({
    accountId: account.id,
    email: accountEmail,
    state: "restart_required",
    message: res.message || "자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다.",
    liveEmailBefore: res.liveEmailBefore,
    liveEmailAfter: res.liveEmailAfter,
  });
  callbacks.onNotify?.("자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다.", "warn");

  // Auto-confirmation polling (up to 20s)
  void (async () => {
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      const current = getPendingAgSwitch();
      if (!current || current.accountId !== account.id) {
        break;
      }
      try {
        const check = await antigravityConfirmSwitch(account.id);
        if (check.state === "confirmed") {
          if (check.outcome) {
            await persistMonitorOutcome(check.outcome);
          } else {
            const outcome = await monitorRefresh("antigravity");
            await persistMonitorOutcome(outcome);
          }
          setPendingAgSwitch(null);
          callbacks.onNotify?.(`${accountEmail} 계정 전환 완료`, "success");
          await callbacks.onReload?.();
          break;
        } else if (check.state === "mismatch") {
          setPendingAgSwitch({
            accountId: account.id,
            email: accountEmail,
            state: "mismatch",
            message: check.message,
            liveEmailAfter: check.liveEmailAfter,
          });
          callbacks.onNotify?.(
            check.message ||
              `예상 계정과 실제 로그인 계정이 다릅니다. (요청: ${accountEmail}, 실제: ${check.liveEmailAfter ?? "알 수 없음"})`,
            "error",
          );
          break;
        }
      } catch {
        // ignore transient network errors during restart
      }
    }
  })();

  return res;
}

export async function runAntigravityConfirm(
  account: AntigravityAccount,
  callbacks: {
    onNotify?: (message: string, tone: "success" | "warn" | "error" | "info") => void;
    onReload?: () => Promise<void> | void;
  } = {},
): Promise<AntigravitySwitchResult> {
  const accountEmail = account.email ?? account.label;
  const check = await antigravityConfirmSwitch(account.id);

  if (check.state === "confirmed") {
    if (check.outcome) {
      await persistMonitorOutcome(check.outcome);
    } else {
      const outcome = await monitorRefresh("antigravity");
      await persistMonitorOutcome(outcome);
    }
    setPendingAgSwitch(null);
    callbacks.onNotify?.(`${accountEmail} 계정 전환 완료`, "success");
    await callbacks.onReload?.();
  } else if (check.state === "mismatch") {
    setPendingAgSwitch({
      accountId: account.id,
      email: accountEmail,
      state: "mismatch",
      message: check.message,
      liveEmailAfter: check.liveEmailAfter,
    });
    callbacks.onNotify?.(
      check.message ||
        `예상 계정과 실제 로그인 계정이 다릅니다. (요청: ${accountEmail}, 실제: ${check.liveEmailAfter ?? "알 수 없음"})`,
      "error",
    );
  } else {
    setPendingAgSwitch({
      accountId: account.id,
      email: accountEmail,
      state: "restart_required",
      message: check.message || "Antigravity가 아직 이전 세션으로 실행 중입니다. Antigravity를 재시작하세요.",
      liveEmailAfter: check.liveEmailAfter,
    });
    callbacks.onNotify?.(check.message || "Antigravity 재시작이 필요합니다", "warn");
  }

  return check;
}
