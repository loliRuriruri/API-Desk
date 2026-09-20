export interface AlertItem {
  key: string;
  label: string;
  remainingPercent: number;
  message: string;
}

export interface AlertCandidate {
  key: string;
  label: string;
  remainingPercent: number | null;
  thresholdPercent?: number | null;
}

export function collectAlerts(
  candidates: AlertCandidate[],
  thresholdPercent: number,
): AlertItem[] {
  const globalThreshold = Math.max(1, Math.min(100, thresholdPercent));
  const alerts: AlertItem[] = [];
  for (const candidate of candidates) {
    if (candidate.remainingPercent === null) continue;
    const threshold = Math.max(
      1,
      Math.min(100, candidate.thresholdPercent ?? globalThreshold),
    );
    const remaining = Math.max(0, Math.min(100, candidate.remainingPercent));
    if (remaining > threshold) continue;
    alerts.push({
      key: candidate.key,
      label: candidate.label,
      remainingPercent: Math.round(remaining * 10) / 10,
      message: `${candidate.label}: 잔여 ${Math.round(remaining)}% (임계치 ${threshold}%)`,
    });
  }
  return alerts.sort((a, b) => a.remainingPercent - b.remainingPercent);
}

export function credentialRemainingPercent(
  remaining: number | null,
  limit: number | null,
): number | null {
  if (remaining === null) return null;
  if (limit !== null && limit > 0) {
    return Math.max(0, Math.min(100, (remaining / limit) * 100));
  }
  return null;
}

export function monitorRemainingPercent(
  monitor: string,
  detailsJson: string | null,
  cap: number | null = null,
): number | null {
  if (!detailsJson) return null;
  let details: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(detailsJson);
    if (typeof parsed !== "object" || parsed === null) return null;
    details = parsed as Record<string, unknown>;
  } catch {
    return null;
  }

  const number = (value: unknown): number | null =>
    typeof value === "number" && Number.isFinite(value) ? value : null;
  const min = (values: Array<number | null>): number | null => {
    const filtered = values.filter((value): value is number => value !== null);
    return filtered.length > 0 ? Math.min(...filtered) : null;
  };

  if (monitor === "codex") {
    const primaryUsed = number(details.primaryUsedPercent);
    const secondaryUsed = number(details.secondaryUsedPercent);
    return min([
      primaryUsed === null ? null : 100 - primaryUsed,
      secondaryUsed === null ? null : 100 - secondaryUsed,
    ]);
  }
  if (monitor === "grok") {
    const usedPercent = number(details.usedPercent);
    if (usedPercent !== null) return Math.max(0, 100 - usedPercent);
    const used = number(details.used);
    if (used !== null && cap !== null && cap > 0) {
      return Math.max(0, 100 - (used / cap) * 100);
    }
    return null;
  }
  if (monitor === "opencode") {
    return number(details.remainingPercent);
  }
  if (monitor === "antigravity" || monitor.startsWith("antigravity:")) {
    return min([
      number(details.geminiRemainingPercent),
      number(details.claudeRemainingPercent),
    ]);
  }
  return null;
}
