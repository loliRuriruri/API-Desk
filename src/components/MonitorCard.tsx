import { useEffect, useState } from "react";
import type { MonitorSnapshot } from "../lib/db/repo";
import type { AntigravityAccount } from "../lib/system";
import { monitorRemainingPercent } from "../lib/alerts";
import { gaugesForMonitor, monitorModels } from "../lib/monitorGauges";
import { antigravityAccountSnapshots } from "../lib/monitorStore";
import { loadMonitorCaps, saveMonitorCap } from "../lib/settings";
import { formatDateTime, formatRelativeTime } from "../lib/format";
import { Badge } from "./Badges";
import { GaugeList } from "./Gauge";
import { SummaryChips } from "./SummaryChips";
import { AntigravityAccounts } from "./AntigravityAccounts";

interface MonitorCardProps {
  monitor: string;
  label: string;
  available: boolean;
  detail: string;
  snapshot: MonitorSnapshot | null;
  busy: boolean;
  onRefresh: () => void;
  onOpenAccounts?: () => void;
  accounts?: AntigravityAccount[];
  allSnapshots?: MonitorSnapshot[];
  showModels?: boolean;
  compact?: boolean;
  accountBusy?: boolean;
  onSwitchAccount?: (account: AntigravityAccount) => void;
  onDeleteAccount?: (account: AntigravityAccount) => void;
}

export function MonitorCard({
  monitor,
  label,
  available,
  detail,
  snapshot,
  busy,
  onRefresh,
  onOpenAccounts,
  accounts = [],
  allSnapshots = [],
  showModels = false,
  compact = false,
  accountBusy = false,
  onSwitchAccount,
  onDeleteAccount,
}: MonitorCardProps) {
  const [modelsOpen, setModelsOpen] = useState(false);
  const [cap, setCap] = useState<number | null>(null);
  const [capDraft, setCapDraft] = useState("");

  useEffect(() => {
    let active = true;
    loadMonitorCaps()
      .then((caps) => {
        if (!active) return;
        const value = caps[monitor] ?? null;
        setCap(value);
        setCapDraft(value !== null ? String(value) : "");
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [monitor]);

  const details = (() => {
    if (!snapshot?.details_json) return null;
    try {
      const parsed: unknown = JSON.parse(snapshot.details_json);
      return typeof parsed === "object" && parsed !== null
        ? (parsed as Record<string, unknown>)
        : null;
    } catch {
      return null;
    }
  })();
  const usedCount = typeof details?.used === "number" ? (details.used as number) : null;
  const hasUsedPercent = typeof details?.usedPercent === "number";
  const needsCap = monitor === "grok" && usedCount !== null && !hasUsedPercent;

  const commitCap = async () => {
    if (capDraft === (cap !== null ? String(cap) : "")) return;
    const parsed = Number(capDraft);
    const next = Number.isFinite(parsed) && parsed > 0 ? Math.round(parsed) : null;
    try {
      await saveMonitorCap(monitor, next);
      setCap(next);
      setCapDraft(next !== null ? String(next) : "");
    } catch {
      // goal is best-effort
    }
  };

  const remaining = monitorRemainingPercent(monitor, snapshot?.details_json ?? null);
  const gauges = gaugesForMonitor(monitor, snapshot?.details_json ?? null, cap);
  const models = monitorModels(snapshot?.details_json ?? null);
  const hasAccountTabs =
    monitor === "antigravity" &&
    (accounts.length > 0 || antigravityAccountSnapshots(allSnapshots).size > 0);

  const tone: "ok" | "warn" | "danger" | "muted" =
    remaining === null ? "muted" : remaining >= 60 ? "ok" : remaining >= 30 ? "warn" : "danger";
  const snapshotStatusLabel = snapshot
    ? snapshot.status === "ok"
      ? "정상"
      : snapshot.status === "unavailable"
        ? "사용 불가"
        : "오류"
    : null;
  const statusLabel = !available ? "사용 불가" : snapshotStatusLabel ?? "준비됨";
  const showStatusBadge = !available || snapshot === null || snapshot.status !== "ok";

  return (
    <div className={`monitor-card ${compact ? "monitor-card-compact" : ""}`}>
      <div className="monitor-head">
        <div className="monitor-title">
          <strong>{label}</strong>
          {remaining !== null ? <Badge tone={tone}>{Math.round(remaining)}%</Badge> : null}
          {showStatusBadge ? (
            <Badge tone={available ? "muted" : "warn"}>{statusLabel}</Badge>
          ) : null}
        </div>
        <div className="row-actions">
          {snapshot ? (
            <span className="stamp" title={`최근 조회 ${formatDateTime(snapshot.fetched_at)}`}>
              {formatRelativeTime(snapshot.fetched_at)}
            </span>
          ) : (
            <span className="stamp">미조회</span>
          )}
          {onOpenAccounts ? (
            <button type="button" className="button button-small" onClick={onOpenAccounts}>
              계정
            </button>
          ) : null}
          <button
            type="button"
            className="button button-small"
            disabled={busy || !available}
            onClick={onRefresh}
          >
            {busy ? "조회 중…" : "↻"}
          </button>
        </div>
      </div>

      <SummaryChips text={snapshot?.summary || detail} className="mono" />

      {gauges.length > 0 && !hasAccountTabs ? <GaugeList items={gauges} /> : null}

      {needsCap ? (
        <div className="monitor-cap">
          <span className="gauge-label">월 목표</span>
          <input
            className="input input-narrow"
            type="number"
            min={1}
            placeholder="예: 150"
            value={capDraft}
            onChange={(event) => setCapDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void commitCap();
            }}
            onBlur={() => void commitCap()}
          />
          <span className="muted">회</span>
          <span className="gauge-hint">
            {cap !== null ? `현재 ${usedCount}회 사용` : "목표를 정하면 게이지가 표시됩니다"}
          </span>
        </div>
      ) : null}

      {monitor === "antigravity" ? (
        <AntigravityAccounts
          accounts={accounts}
          snapshots={allSnapshots}
          current={snapshot}
          busy={accountBusy || busy}
          onSwitchAccount={onSwitchAccount}
          onDeleteAccount={onDeleteAccount}
          onRefreshCurrent={onRefresh}
        />
      ) : null}

      {showModels && models.length > 0 ? (
        <div className="mini-models">
          <button
            type="button"
            className="mini-models-toggle"
            onClick={() => setModelsOpen((current) => !current)}
          >
            <span>모델 {models.length}</span>
            <span>{modelsOpen ? "▾" : "▸"}</span>
          </button>
          {modelsOpen
            ? models.slice(0, 10).map((model) => (
                <div key={model.label} className="mini-model-row">
                  <span>{model.label}</span>
                  <span className="mono">
                    {model.remaining !== null
                      ? `${Math.round(model.remaining)}%`
                      : (model.meta ?? "—")}
                  </span>
                </div>
              ))
            : null}
          {modelsOpen && models.length > 10 ? (
            <div className="mini-model-row">
              <span className="muted">+{models.length - 10}개 더</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
