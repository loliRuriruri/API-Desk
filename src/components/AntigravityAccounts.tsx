import { useMemo, useState } from "react";
import type { MonitorSnapshot } from "../lib/db/repo";
import type { AntigravityAccount } from "../lib/system";
import { antigravityAccountSnapshots } from "../lib/monitorStore";
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
  onSwitchAccount?: (account: AntigravityAccount) => void;
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
  onSwitchAccount,
  onDeleteAccount,
  onRefreshCurrent,
}: AntigravityAccountsProps) {
  const byEmail = useMemo(() => antigravityAccountSnapshots(snapshots), [snapshots]);
  const currentEmail = useMemo(() => liveEmail(current), [current]);

  const tabs = useMemo<AccountTab[]>(() => {
    const list: AccountTab[] = [];
    const seen = new Set<string>();
    for (const account of accounts) {
      const email = account.email ?? account.label;
      if (seen.has(email)) continue;
      seen.add(email);
      const mine = currentEmail === email ? current : byEmail.get(email) ?? null;
      list.push({
        key: account.id,
        email,
        account,
        snapshot: account.isActive ? current ?? mine : mine,
      });
    }
    for (const [email, snapshot] of byEmail) {
      if (seen.has(email)) continue;
      seen.add(email);
      list.push({ key: `snap:${email}`, email, account: null, snapshot });
    }
    if (currentEmail && !seen.has(currentEmail)) {
      list.push({ key: `live:${currentEmail}`, email: currentEmail, account: null, snapshot: current });
    }
    return list.sort((a, b) => {
      const aCurrent = a.account?.isActive || a.email === currentEmail;
      const bCurrent = b.account?.isActive || b.email === currentEmail;
      if (aCurrent !== bCurrent) return aCurrent ? -1 : 1;
      return 0;
    });
  }, [accounts, byEmail, current, currentEmail]);

  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  if (tabs.length === 0) return null;

  const selected = tabs.find((tab) => tab.key === selectedKey) ?? tabs[0];
  const gauges = gaugesForMonitor("antigravity", selected.snapshot?.details_json ?? null);
  const isCurrent = selected.account?.isActive || selected.email === currentEmail;

  return (
    <div className="ag-accounts">
      <div className="ag-tabs" role="tablist" aria-label="Antigravity 계정">
        {tabs.map((tab) => {
          const percent = monitorRemainingPercent("antigravity", tab.snapshot?.details_json ?? null);
          const active = tab.key === selected.key;
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
              {tab.account?.isActive || tab.email === currentEmail ? (
                <span className="mini-dot mini-dot-on" />
              ) : null}
              <span className="ag-tab-label">{shortLabel(tab.email)}</span>
              {percent !== null ? <span className="ag-tab-pct mono">{Math.round(percent)}%</span> : null}
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
          {isCurrent ? <Badge tone="ok">현재</Badge> : null}
          <span className="ag-panel-actions">
            {selected.account && !isCurrent && onSwitchAccount ? (
              <button
                type="button"
                className="button button-small"
                disabled={busy}
                onClick={() => onSwitchAccount(selected.account as AntigravityAccount)}
              >
                전환
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
