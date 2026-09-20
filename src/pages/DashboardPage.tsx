import { useState } from "react";
import {
  dashboardCounts,
  listCostLines,
  listExpiringCredentials,
  listMonitorSnapshots,
  listProviderCards,
  listRecentCredentials,
  listUpcomingRenewals,
} from "../lib/db/repo";
import { useAsyncData } from "../hooks/useAsyncData";
import { Badge, TestStatusBadge } from "../components/Badges";
import { Modal } from "../components/Modal";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";
import { formatDate, formatDateTime, formatMoney } from "../lib/format";
import {
  antigravityAccounts,
  antigravityDeleteAccount,
  antigravitySaveCurrent,
  antigravitySwitchAccount,
  monitorProbe,
  monitorRefresh,
  type AntigravityAccount,
} from "../lib/system";
import { persistMonitorOutcome } from "../lib/monitorStore";
import { MonitorCard } from "../components/MonitorCard";
import mikuHero from "../assets/miku-hero.png";

interface DashboardPageProps {
  onOpenApis: () => void;
}

export function DashboardPage({ onOpenApis }: DashboardPageProps) {
  const { data, loading, error, reload } = useAsyncData(async () => {
    const [
      counts,
      cards,
      costs,
      recent,
      renewals,
      expiring,
      probes,
      monitorSnapshots,
      agAccounts,
    ] = await Promise.all([
      dashboardCounts(),
      listProviderCards(),
      listCostLines(),
      listRecentCredentials(8),
      listUpcomingRenewals(),
      listExpiringCredentials(),
      monitorProbe(),
      listMonitorSnapshots(),
      antigravityAccounts(),
    ]);
    return {
      counts,
      cards,
      costs,
      recent,
      renewals,
      expiring,
      probes,
      monitorSnapshots,
      agAccounts,
    };
  }, "dashboard");
  const { notify } = useToast();
  const [monitorBusy, setMonitorBusy] = useState<string | null>(null);
  const [accountsOpen, setAccountsOpen] = useState(false);
  const [accounts, setAccounts] = useState<AntigravityAccount[]>([]);
  const [accountsBusy, setAccountsBusy] = useState(false);
  const [accountLabel, setAccountLabel] = useState("");

  const reloadAccounts = async () => {
    try {
      setAccounts(await antigravityAccounts());
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const openAccounts = async () => {
    setAccountsOpen(true);
    await reloadAccounts();
  };

  const saveCurrentAccount = async () => {
    setAccountsBusy(true);
    try {
      const saved = await antigravitySaveCurrent(accountLabel);
      notify(
        `계정을 저장했습니다: ${saved.email ?? saved.label} — Antigravity에서 다른 계정으로 로그인한 뒤 다시 저장하면 여러 계정을 관리할 수 있습니다`,
        "success",
      );
      setAccountLabel("");
      await reloadAccounts();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setAccountsBusy(false);
    }
  };

  const switchAccount = async (account: AntigravityAccount) => {
    setAccountsBusy(true);
    try {
      await antigravitySwitchAccount(account.id);
      notify(
        `${account.email ?? account.label} 계정으로 전환했습니다. Antigravity를 재시작한 뒤 [조회]하세요`,
        "success",
      );
      await reloadAccounts();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setAccountsBusy(false);
    }
  };

  const deleteAccount = async (account: AntigravityAccount) => {
    try {
      await antigravityDeleteAccount(account.id);
      notify("저장된 계정을 삭제했습니다", "success");
      await reloadAccounts();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const refreshMonitor = async (monitor: string): Promise<boolean> => {
    setMonitorBusy(monitor);
    try {
      const outcome = await monitorRefresh(monitor);
      await persistMonitorOutcome(outcome);
      return true;
    } catch (err) {
      notify(errorMessage(err), "error");
      return false;
    } finally {
      setMonitorBusy(null);
    }
  };

  const refreshOneMonitor = async (monitor: string) => {
    const success = await refreshMonitor(monitor);
    if (success) reload();
  };

  const refreshAllMonitors = async () => {
    if (!data) return;
    let ok = 0;
    for (const probe of data.probes) {
      if (!probe.available) continue;
      const success = await refreshMonitor(probe.monitor);
      if (success) ok += 1;
    }
    notify(
      ok > 0 ? `계정 사용량 ${ok}건을 갱신했습니다` : "갱신할 수 있는 모니터가 없습니다",
      ok > 0 ? "success" : "info",
    );
    reload();
  };

  if (loading && !data) return <div className="page-loading">대시보드 불러오는 중…</div>;
  if (error) {
    return (
      <div className="page-error">
        <p>{error}</p>
        <button type="button" className="button" onClick={reload}>
          다시 시도
        </button>
      </div>
    );
  }
  if (!data) return null;

  const { counts, cards, costs, recent, renewals, expiring } = data;
  const snapshotByMonitor = new Map(
    data.monitorSnapshots.map((snapshot) => [snapshot.monitor, snapshot]),
  );
  const totals = new Map<string, number>();
  for (const line of costs) {
    totals.set(line.currency, (totals.get(line.currency) ?? 0) + line.monthly_cost);
  }
  const needsCheck = recent.filter(
    (credential) => credential.last_test_status !== "connected",
  );

  return (
    <div className="page">
      <section className="panel hero-panel">
        <div className="hero-copy">
          <h2>API Desk에 오신 것을 환영합니다</h2>
          <p className="muted">
            API 키는 Stronghold Vault에, 메타데이터는 SQLite에 — 모든 데이터는 이 PC에만
            저장됩니다.
          </p>
        </div>
        <img className="hero-art" src={mikuHero} alt="" />
      </section>

      <div className="stat-row">
        <div className="stat">
          <span className="stat-label">등록 API</span>
          <span className="stat-value">{counts.total}</span>
        </div>
        <div className="stat">
          <span className="stat-label">사용 중</span>
          <span className="stat-value">{counts.active}</span>
        </div>
        <div className="stat">
          <span className="stat-label">체크 필요</span>
          <span className="stat-value stat-warn">{counts.needs_check}</span>
        </div>
        <div className="stat">
          <span className="stat-label">미사용</span>
          <span className="stat-value">{counts.unused}</span>
        </div>
      </div>

      <div className="dashboard-grid">
        <section className="panel">
          <header className="panel-header">
            <h3>이번 달 고정비</h3>
            <span className="panel-hint">활성 요금제 {costs.length}개</span>
          </header>
          {costs.length === 0 ? (
            <p className="empty-note">
              고정비가 등록된 계정이 없습니다. 계정에 요금제 비용을 입력하세요.
            </p>
          ) : (
            <table className="table table-compact">
              <tbody>
                {costs.map((line, index) => (
                  <tr key={`${line.provider_display_name}-${line.account_name}-${index}`}>
                    <td>{line.provider_display_name}</td>
                    <td className="muted">{line.account_name}</td>
                    <td className="num">{formatMoney(line.monthly_cost, line.currency)}</td>
                  </tr>
                ))}
              </tbody>
              <tfoot>
                {Array.from(totals.entries()).map(([currency, total]) => (
                  <tr key={currency}>
                    <td colSpan={2}>합계 ({currency})</td>
                    <td className="num strong">{formatMoney(total, currency)}</td>
                  </tr>
                ))}
              </tfoot>
            </table>
          )}
        </section>

        <section className="panel">
          <header className="panel-header">
            <h3>프로바이더</h3>
            <button type="button" className="button button-small" onClick={onOpenApis}>
              API 관리 열기
            </button>
          </header>
          <div className="provider-cards">
            {cards.length === 0 ? (
              <p className="empty-note">
                아직 프로바이더가 없습니다. API 관리에서 첫 프로바이더를 추가하세요.
              </p>
            ) : (
              cards.map((card) => (
                <button
                  key={card.id}
                  type="button"
                  className="provider-card"
                  onClick={onOpenApis}
                >
                  <div className="provider-card-head">
                    <strong>{card.display_name}</strong>
                    <Badge tone={card.credential_count > 0 ? "ok" : "muted"}>
                      {card.credential_count > 0 ? "사용 중" : "비어 있음"}
                    </Badge>
                  </div>
                  <div className="provider-card-meta">
                    <span>키 {card.credential_count}개</span>
                    <span>
                      {card.monthly_cost > 0 ? formatMoney(card.monthly_cost, "USD") : "$0"} / 월
                    </span>
                  </div>
                  <div className="provider-card-foot">
                    <span>{card.kind}</span>
                    <span>{card.last_activity ? formatDateTime(card.last_activity) : "기록 없음"}</span>
                  </div>
                </button>
              ))
            )}
          </div>
        </section>
      </div>

      <section className="panel">
        <header className="panel-header">
          <h3>계정 사용량 · 쿼터</h3>
          <button
            type="button"
            className="button button-small"
            disabled={monitorBusy !== null}
            onClick={() => void refreshAllMonitors()}
          >
            {monitorBusy !== null ? "갱신 중…" : "모두 새로고침"}
          </button>
        </header>
        <div className="monitor-grid">
          {data.probes.map((probe) => (
            <MonitorCard
              key={probe.monitor}
              monitor={probe.monitor}
              label={probe.label}
              available={probe.available}
              detail={probe.detail}
              snapshot={snapshotByMonitor.get(probe.monitor) ?? null}
              busy={monitorBusy === probe.monitor}
              onRefresh={() => void refreshOneMonitor(probe.monitor)}
              onOpenAccounts={
                probe.monitor === "antigravity" ? () => void openAccounts() : undefined
              }
              accounts={data.agAccounts}
              allSnapshots={data.monitorSnapshots}
            />
          ))}
        </div>
        <p className="form-hint">
          Codex는 <code>~/.codex/auth.json</code>, Grok은 <code>XAI_API_KEY</code> 또는{" "}
          <code>~/.grok/auth.json</code>, Antigravity는 실행 중인 로컬 서버에서 읽습니다. 토큰은
          Rust 백엔드 밖으로 나가지 않으며 조회는 버튼을 누를 때만 실행됩니다.
        </p>
      </section>

      <div className="dashboard-grid dashboard-grid-4">
        <section className="panel">
          <header className="panel-header">
            <h3>최근 사용/수정</h3>
          </header>
          {recent.length === 0 ? (
            <p className="empty-note">아직 기록이 없습니다.</p>
          ) : (
            <ul className="list">
              {recent.map((credential) => (
                <li key={`recent-${credential.id}`}>
                  <button type="button" className="list-link" onClick={onOpenApis}>
                    <span className="list-title">{credential.name}</span>
                    <span className="list-sub">
                      {credential.provider_display_name} · {credential.env_name ?? "환경변수 없음"}
                    </span>
                    <span className="list-meta">{formatDateTime(credential.updated_at)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel">
          <header className="panel-header">
            <h3>체크 필요</h3>
          </header>
          {needsCheck.length === 0 ? (
            <p className="empty-note">최근 키는 모두 테스트되었습니다.</p>
          ) : (
            <ul className="list">
              {needsCheck.map((credential) => (
                <li key={`check-${credential.id}`}>
                  <button type="button" className="list-link" onClick={onOpenApis}>
                    <span className="list-title">{credential.name}</span>
                    <span className="list-sub">{credential.provider_display_name}</span>
                    <TestStatusBadge status={credential.last_test_status} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel">
          <header className="panel-header">
            <h3>다가오는 결제일</h3>
          </header>
          {renewals.length === 0 ? (
            <p className="empty-note">결제일이 설정된 계정이 없습니다.</p>
          ) : (
            <ul className="list">
              {renewals.slice(0, 8).map((renewal, index) => (
                <li key={`renewal-${index}`}>
                  <div className="list-static">
                    <span className="list-title">{renewal.account_name}</span>
                    <span className="list-sub">
                      {renewal.provider_display_name} ·{" "}
                      {renewal.renewal_date
                        ? formatDate(renewal.renewal_date)
                        : `매월 ${renewal.billing_day}일`}
                    </span>
                    <span className="list-meta">
                      {renewal.monthly_cost > 0
                        ? formatMoney(renewal.monthly_cost, renewal.currency)
                        : "—"}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel">
          <header className="panel-header">
            <h3>Key 만료 예정</h3>
          </header>
          {expiring.length === 0 ? (
            <p className="empty-note">만료일이 등록된 키가 없습니다.</p>
          ) : (
            <ul className="list">
              {expiring.map((credential) => (
                <li key={`expiring-${credential.id}`}>
                  <button type="button" className="list-link" onClick={onOpenApis}>
                    <span className="list-title">{credential.name}</span>
                    <span className="list-sub">
                      {credential.provider_display_name} ·{" "}
                      {credential.env_name ?? "환경변수 없음"}
                    </span>
                    <span className="list-meta">{formatDate(credential.expires_at)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
      {accountsOpen ? (
        <Modal
          title="Antigravity 계정"
          subtitle="현재 로그인된 계정을 저장하고, 저장된 계정으로 전환합니다"
          onClose={() => setAccountsOpen(false)}
          footer={
            <button type="button" className="button" onClick={() => setAccountsOpen(false)}>
              닫기
            </button>
          }
        >
          <div className="accounts-toolbar">
            <input
              className="input"
              placeholder="라벨 (선택)"
              value={accountLabel}
              onChange={(event) => setAccountLabel(event.target.value)}
            />
            <button
              type="button"
              className="button button-primary"
              disabled={accountsBusy}
              onClick={() => void saveCurrentAccount()}
            >
              현재 계정 저장
            </button>
          </div>
          {accounts.length === 0 ? (
            <p className="empty-note">
              저장된 계정이 없습니다. Antigravity에 로그인된 상태에서 “현재 계정 저장”을 누르세요.
            </p>
          ) : (
            <table className="table table-compact">
              <thead>
                <tr>
                  <th>계정</th>
                  <th>등급</th>
                  <th>저장 시각</th>
                  <th aria-label="작업" />
                </tr>
              </thead>
              <tbody>
                {accounts.map((account) => (
                  <tr key={account.id}>
                    <td>
                      {account.email ?? account.label}{" "}
                      {account.isActive ? <Badge tone="ok">현재</Badge> : null}
                    </td>
                    <td className="muted">{account.tier ?? "—"}</td>
                    <td className="muted">{formatDateTime(account.savedAt)}</td>
                    <td className="row-actions">
                      <button
                        type="button"
                        className="button button-small"
                        disabled={accountsBusy}
                        onClick={() => void switchAccount(account)}
                      >
                        전환
                      </button>
                      <button
                        type="button"
                        className="button button-small button-danger-ghost"
                        onClick={() => void deleteAccount(account)}
                      >
                        삭제
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p className="form-hint">
            전환은 Windows 자격 증명(“gemini:antigravity”)을 저장된 값으로 바꿉니다. Antigravity를
            재시작한 뒤 [조회]하면 해당 계정의 주간·5시간 쿼터가 갱신됩니다.
          </p>
        </Modal>
      ) : null}
    </div>
  );
}
