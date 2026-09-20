import { upsertMonitorSnapshot, type MonitorSnapshot } from "./db/repo";
import type { MonitorOutcome } from "./system";

export async function persistMonitorOutcome(outcome: MonitorOutcome): Promise<void> {
  const fetchedAt = new Date().toISOString();
  const summary = outcome.summary || outcome.message || "";
  const detailsJson = outcome.details ? JSON.stringify(outcome.details) : null;

  await upsertMonitorSnapshot({
    monitor: outcome.monitor,
    status: outcome.status,
    summary,
    details_json: detailsJson,
    fetched_at: fetchedAt,
  });

  if (outcome.monitor === "antigravity") {
    const details = outcome.details as { email?: string } | null;
    const email = details?.email?.trim();
    if (email) {
      await upsertMonitorSnapshot({
        monitor: `antigravity:${email}`,
        status: outcome.status,
        summary,
        details_json: detailsJson,
        fetched_at: fetchedAt,
      });
    }
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
