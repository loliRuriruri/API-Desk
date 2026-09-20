export interface GaugeItem {
  label: string;
  percent: number | null;
  mode: "remaining" | "used";
  hint: string | null;
}

function money(value: number): string {
  return `$${value.toFixed(2)}`;
}

export function usageGauges(adapter: string, detailsJson: string | null): GaugeItem[] {
  if (adapter !== "openrouter" || !detailsJson) return [];
  let details: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(detailsJson);
    if (typeof parsed !== "object" || parsed === null) return [];
    details = parsed as Record<string, unknown>;
  } catch {
    return [];
  }
  const items: GaugeItem[] = [];
  const total = typeof details.creditTotal === "number" ? details.creditTotal : null;
  const available =
    typeof details.creditAvailable === "number" ? details.creditAvailable : null;
  if (total !== null && total > 0 && available !== null) {
    items.push({
      label: "크레딧",
      percent: Math.max(0, Math.min(100, (available / total) * 100)),
      mode: "remaining",
      hint: `${money(available)} / ${money(total)}`,
    });
  }
  const key = details.key as { data?: Record<string, unknown> } | undefined;
  const keyData = key?.data ?? (details.key as Record<string, unknown> | undefined);
  const limit = typeof keyData?.limit === "number" ? keyData.limit : null;
  const remaining =
    typeof keyData?.limit_remaining === "number"
      ? keyData.limit_remaining
      : typeof keyData?.limitRemaining === "number"
        ? keyData.limitRemaining
        : null;
  if (limit !== null && limit > 0 && remaining !== null) {
    items.push({
      label: "키 한도",
      percent: Math.max(0, Math.min(100, (remaining / limit) * 100)),
      mode: "remaining",
      hint: `${money(remaining)} / ${money(limit)}`,
    });
  }
  return items;
}

export interface MonitorModelItem {
  label: string;
  remaining: number | null;
  meta?: string;
}

export function monitorModels(detailsJson: string | null): MonitorModelItem[] {
  if (!detailsJson) return [];
  try {
    const parsed = JSON.parse(detailsJson) as {
      models?: Array<{ label?: string; remainingPercent?: number; meta?: string }>;
    };
    if (!Array.isArray(parsed.models)) return [];
    return parsed.models
      .filter((model) => typeof model.label === "string" && model.label.length > 0)
      .map((model) => ({
        label: model.label as string,
        remaining:
          typeof model.remainingPercent === "number" ? model.remainingPercent : null,
        meta: typeof model.meta === "string" ? model.meta : undefined,
      }));
  } catch {
    return [];
  }
}

function number(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function resetHint(iso: string | null | undefined): string | null {
  if (!iso) return null;
  const target = new Date(iso).getTime();
  if (Number.isNaN(target)) return null;
  const seconds = Math.max(0, Math.floor((target - Date.now()) / 1000));
  const hours = Math.floor(seconds / 3600);
  const days = Math.floor(hours / 24);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `리셋 ${days}d ${hours % 24}h`;
  if (hours > 0) return `리셋 ${hours}h ${minutes}m`;
  return `리셋 ${minutes}m`;
}

function resetHintFromSeconds(value: unknown): string | null {
  const seconds = number(value);
  if (seconds === null) return null;
  const hours = Math.floor(seconds / 3600);
  const days = Math.floor(hours / 24);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) return `리셋 ${days}d ${hours % 24}h`;
  if (hours > 0) return `리셋 ${hours}h ${minutes}m`;
  return `리셋 ${minutes}m`;
}

export function gaugesForMonitor(
  monitor: string,
  detailsJson: string | null,
  cap: number | null = null,
): GaugeItem[] {
  if (!detailsJson) return [];
  let details: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(detailsJson);
    if (typeof parsed !== "object" || parsed === null) return [];
    details = parsed as Record<string, unknown>;
  } catch {
    return [];
  }

  if (monitor === "codex") {
    const primary = number(details.primaryUsedPercent);
    const secondary = number(details.secondaryUsedPercent);
    const items: GaugeItem[] = [
      {
        label: "주간",
        percent: primary === null ? null : 100 - primary,
        mode: "remaining",
        hint: resetHint(details.primaryResetAt as string | undefined),
      },
      {
        label: "5시간",
        percent: secondary === null ? null : 100 - secondary,
        mode: "remaining",
        hint: resetHint(details.secondaryResetAt as string | undefined),
      },
    ];
    return items.filter((item) => item.percent !== null || item.hint !== null);
  }

  if (monitor === "grok") {
    const usedPercent = number(details.usedPercent);
    const limit = number(details.monthlyLimit);
    if (usedPercent !== null) {
      return [
        {
          label: "월간 사용",
          percent: usedPercent,
          mode: "used",
          hint: limit !== null && limit > 0 ? null : "한도 정보 없음",
        },
      ];
    }
    const used = number(details.used);
    if (used !== null && cap !== null && cap > 0) {
      return [
        {
          label: "월간 사용",
          percent: Math.round((used / cap) * 1000) / 10,
          mode: "used",
          hint: `목표 ${cap}회`,
        },
      ];
    }
    return [];
  }

  if (monitor === "grok-build") {
    const remaining = number(details.remainingPercent);
    const periodLabel = typeof details.periodLabel === "string" ? details.periodLabel : "기간";
    const used = number(details.usedPercent);
    return [
      {
        label: periodLabel,
        percent: remaining,
        mode: "remaining",
        hint:
          used === null
            ? "사용량 정보 없음"
            : resetHint(details.periodEnd as string | undefined),
      },
    ];
  }

  if (monitor === "antigravity" || monitor.startsWith("antigravity:")) {
    const groups = Array.isArray(details.quotaGroups)
      ? (details.quotaGroups as Array<Record<string, unknown>>)
      : [];
    const items: GaugeItem[] = [];
    for (const group of groups) {
      const name = typeof group.name === "string" ? group.name : "";
      const short = /gemini/i.test(name) ? "Gemini" : /claude/i.test(name) ? "Claude" : name;
      const weekly = number(group.weeklyRemainingPercent);
      const fiveHour = number(group.fiveHourRemainingPercent);
      if (weekly !== null) {
        items.push({
          label: `${short} 주간`,
          percent: weekly,
          mode: "remaining",
          hint: resetHint(group.weeklyResetAt as string | undefined),
        });
      }
      if (fiveHour !== null) {
        items.push({
          label: `${short} 5시간`,
          percent: fiveHour,
          mode: "remaining",
          hint: resetHint(group.fiveHourResetAt as string | undefined),
        });
      }
    }
    if (items.length > 0) return items;
    const gemini = number(details.geminiRemainingPercent);
    const claude = number(details.claudeRemainingPercent);
    const fallback: GaugeItem[] = [
      { label: "Gemini", percent: gemini, mode: "remaining", hint: null },
      { label: "Claude", percent: claude, mode: "remaining", hint: null },
    ];
    return fallback.filter((item) => item.percent !== null);
  }

  if (monitor === "opencode") {
    const items: GaugeItem[] = [];
    for (const [key, label] of [
      ["rolling", "롤링"],
      ["weekly", "주간"],
      ["monthly", "월간"],
    ] as const) {
      const entry = details[key] as Record<string, unknown> | null | undefined;
      if (!entry) continue;
      const percent = number(entry.percent);
      items.push({
        label,
        percent,
        mode: "used",
        hint: resetHintFromSeconds(entry.resetInSec),
      });
    }
    return items;
  }

  return [];
}
