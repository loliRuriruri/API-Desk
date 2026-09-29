import { useEffect, useMemo, useState } from "react";
import type { MonitorSnapshot } from "../lib/db/repo";
import type { AntigravityAccount } from "../lib/system";
import {
  antigravityAccountSnapshots,
  getPendingAgSwitch,
  subscribePendingAgSwitch,
  type PendingSwitchState,
} from "../lib/monitorStore";
import { monitorRemainingPercent } from "../lib/alerts";
import { gaugesForMonitor } from "../lib/monitorGauges";
import { formatDateTime, formatRelativeTime } from "../lib/format";
import { Badge } from "./Badges";
import { GaugeList } from "./Gauge";
import { SummaryChips } from "./SummaryChips";

interface AntigravityAccountsProps {
  accounts: AntigravityAccount[];
  snapshots: MonitorSnapshot[];
  current?: MonitorSnapshot | null;
  busy?: boolean;
  pendingSwitch?: PendingSwitchState | null;
  onSwitchAccount?: (account: AntigravityAccount) => void;
  onConfirmSwitch?: (account: AntigravityAccount) => void;
  onDeleteAccount?: (account: AntigravityAccount) => void;
  onRefreshCurrent?: () => void;
}

interface AccountTab {
  key: string;
  email: string;
  account: AntigravityAccount | null;
  snapshot: MonitorSnapshot | null;
}

function liveEmail(snapshot: MonitorSnapshot | null | undefined): string | null {
  if (!snapshot?.details_json) return null;
  try {
    const parsed = JSON.parse(snapshot.details_json) as { email?: unknown };
    return typeof parsed.email === "string" && parsed.email.trim().length > 0
      ? parsed.email.trim()
      : null;
  } catch {
    return null;
  }
}

function shortLabel(email: string): string {
  const at = email.indexOf("@");
  return at > 0 ? email.slice(0, at) : email;
}

export function AntigravityAccounts({
  accounts,
  snapshots,
  current = null,
  busy = false,
  pendingSwitch: propPendingSwitch,
  onSwitchAccount,
  onConfirmSwitch,
  onDeleteAccount,
  onRefreshCurrent,
}: AntigravityAccountsProps) {
  const [storePendingSwitch, setStorePendingSwitch] = useState<PendingSwitchState | null>(() =>
    getPendingAgSwitch(),
  );

  useEffect(() => {
    return subscribePendingAgSwitch((state) => {
      setStorePendingSwitch(state);
    });
  }, []);

  const pendingSwitch = propPendingSwitch !== undefined ? propPendingSwitch : storePendingSwitch;

  const byEmail = useMemo(() => antigravityAccountSnapshots(snapshots), [snapshots]);
  const currentEmail = useMemo(() => liveEmail(current), [current]);

  const tabs = useMemo<AccountTab[]>(() => {
    const list: AccountTab[] = [];
    const seen = new Set<string>();
    for (const account of accounts) {
      const email = account.email ?? account.label;
      const lower = email.toLowerCase();
      if (seen.has(lower)) continue;
      seen.add(lower);
      const isLive = currentEmail !== null && lower === currentEmail.toLowerCase();
      const snapshot = isLive ? current : byEmail.get(email) ?? null;
      list.push({
        key: account.id,
        email,
        account,
        snapshot,
      });
    }
    for (const [email, snapshot] of byEmail) {
      const lower = email.toLowerCase();
      if (seen.has(lower)) continue;
      seen.add(lower);
      list.push({ key: `snap:${email}`, email, account: null, snapshot });
    }
    if (currentEmail && !seen.has(currentEmail.toLowerCase())) {
      list.push({ key: `live:${currentEmail}`, email: currentEmail, account: null, snapshot: current });
    }
    return list.sort((a, b) => {
      const aCurrent = currentEmail !== null && a.email.toLowerCase() === currentEmail.toLowerCase();
      const bCurrent = currentEmail !== null && b.email.toLowerCase() === currentEmail.toLowerCase();
      if (aCurrent !== bCurrent) return aCurrent ? -1 : 1;
      return 0;
    });
  }, [accounts, byEmail, current, currentEmail]);

  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  if (tabs.length === 0) return null;

  const selected = tabs.find((tab) => tab.key === selectedKey) ?? tabs[0];
  const gauges = gaugesForMonitor("antigravity", selected.snapshot?.details_json ?? null);
  const isCurrent =
    currentEmail !== null && selected.email.toLowerCase() === currentEmail.toLowerCase();

  const isPendingForSelected =
    pendingSwitch !== null &&
    ((selected.account && selected.account.id === pendingSwitch.accountId) ||
      selected.email.toLowerCase() === pendingSwitch.email.toLowerCase());

  return (
    <div className="ag-accounts">
      <div className="ag-tabs" role="tablist" aria-label="Antigravity 계정">
        {tabs.map((tab) => {
          const percent = monitorRemainingPercent("antigravity", tab.snapshot?.details_json ?? null);
          const active = tab.key === selected.key;
          const tabIsCurrent =
            currentEmail !== null && tab.email.toLowerCase() === currentEmail.toLowerCase();
          const tabIsPending =
            pendingSwitch !== null &&
            !tabIsCurrent &&
            ((tab.account && tab.account.id === pendingSwitch.accountId) ||
              tab.email.toLowerCase() === pendingSwitch.email.toLowerCase());

          return (
            <button
              key={tab.key}
              type="button"
              role="tab"
              aria-selected={active}
              title={tab.email}
              className={`ag-tab ${active ? "ag-tab-active" : ""}`}
              onClick={() => setSelectedKey(tab.key)}
            >
              {tabIsCurrent ? <span className="mini-dot mini-dot-on" title="현재 라이브" /> : null}
              {tabIsPending ? <span className="mini-dot mini-dot-warn" title="전환 대기" /> : null}
              <span className="ag-tab-label">{shortLabel(tab.email)}</span>
              {tabIsPending ? (
                <span className="ag-tab-pending" style={{ fontSize: "9px", opacity: 0.8 }}>
                  (대기)
                </span>
              ) : percent !== null ? (
                <span className="ag-tab-pct mono">{Math.round(percent)}%</span>
              ) : null}
            </button>
          );
        })}
      </div>

      <div className="ag-panel">
        <div className="ag-panel-head">
          <span className="ag-panel-email">
            {selected.email}
            {selected.account?.tier ? <span className="muted"> · {selected.account.tier}</span> : null}
          </span>
          {isCurrent ? (
            <Badge tone="ok">현재</Badge>
          ) : isPendingForSelected ? (
            <Badge
              tone={
                pendingSwitch?.state === "mismatch" || pendingSwitch?.state === "timeout"
                  ? "danger"
                  : "warn"
              }
            >
              {pendingSwitch?.state === "restart_required"
                ? "재시작 필요"
                : pendingSwitch?.state === "mismatch"
                  ? "계정 불일치"
                  : pendingSwitch?.state === "timeout"
                    ? "시간 초과"
                    : "전환 중…"}
            </Badge>
          ) : null}
          <span className="ag-panel-actions">
            {selected.account && !isCurrent && onSwitchAccount ? (
              <button
                type="button"
                className="button button-small"
                disabled={busy}
                onClick={() => onSwitchAccount(selected.account as AntigravityAccount)}
              >
                {isPendingForSelected ? "다시 전환" : "전환"}
              </button>
            ) : null}
            {isCurrent && onRefreshCurrent ? (
              <button
                type="button"
                className="button button-small"
                disabled={busy}
                onClick={onRefreshCurrent}
              >
                {busy ? "조회 중…" : "↻"}
              </button>
            ) : null}
            {selected.account && onDeleteAccount ? (
              <button
                type="button"
                className="button button-small button-danger-ghost"
                disabled={busy}
                onClick={() => onDeleteAccount(selected.account as AntigravityAccount)}
              >
                삭제
              </button>
            ) : null}
          </span>
        </div>

        {isPendingForSelected && !isCurrent ? (
          <div
            className={`ag-pending-card ${pendingSwitch?.state === "mismatch" ? "ag-pending-mismatch" : ""}`}
          >
            <p className="mini-note" style={{ margin: 0 }}>
              {pendingSwitch?.message ||
                (pendingSwitch?.state === "restart_required"
                  ? "자격 증명을 변경했습니다. Antigravity 재시작이 필요합니다."
                  : "세션 전환 확인 중…")}
            </p>
            {onConfirmSwitch && selected.account ? (
              <div className="row-actions" style={{ marginTop: 4 }}>
                <button
                  type="button"
                  className="button button-small button-primary"
                  disabled={busy}
                  onClick={() => onConfirmSwitch(selected.account as AntigravityAccount)}
                >
                  Antigravity 재시작 후 확인
                </button>
              </div>
            ) : null}
          </div>
        ) : null}

        {selected.snapshot ? (
          <>
            {!isCurrent ? (
              <SummaryChips text={selected.snapshot.summary ?? ""} className="mono" />
            ) : null}
            {gauges.length > 0 ? <GaugeList items={gauges} /> : null}
            <div className="monitor-foot">
              <span className="muted">
                {isCurrent ? "현재 계정" : "마지막 확인"} ·{" "}
                <span
                  className="stamp"
                  title={`최근 조회 ${formatDateTime(selected.snapshot.fetched_at)}`}
                >
                  {formatRelativeTime(selected.snapshot.fetched_at)}
                </span>
                {selected.account ? "" : " · 자격 증명 미저장"}
              </span>
            </div>
            {!selected.account ? (
              <p className="mini-note">
                이 계정의 자격 증명이 저장되지 않아 전환할 수 없습니다. Antigravity에서 해당
                계정으로 로그인한 뒤 조회하면 자동으로 기억됩니다.
              </p>
            ) : null}
          </>
        ) : (
          <p className="mini-note">
            아직 조회 기록이 없습니다. 해당 계정으로 로그인해 조회하면 자동으로 기억됩니다.
          </p>
        )}
      </div>
    </div>
  );
}
