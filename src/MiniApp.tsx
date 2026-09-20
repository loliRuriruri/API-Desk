import { useEffect, useMemo, useRef, useState } from "react";
import {
  listAllCredentialFields,
  listCredentialsWithContext,
  listModels,
  listMonitorSnapshots,
  listUsageSnapshots,
  upsertUsageSnapshot,
  type CredentialWithContext,
  type MonitorSnapshot,
  type UsageSnapshot,
} from "./lib/db/repo";
import { useAsyncData } from "./hooks/useAsyncData";
import { useToast } from "./components/ToastProvider";
import { Badge } from "./components/Badges";
import { GaugeList } from "./components/Gauge";
import { SummaryChips } from "./components/SummaryChips";
import { errorMessage } from "./lib/errors";
import { formatDateTime, formatRelativeTime } from "./lib/format";
import {
  monitorProbe,
  monitorRefresh,
  fetchUsage,
  antigravityAccounts,
  antigravityDeleteAccount,
  antigravitySwitchAccount,
  type AntigravityAccount,
} from "./lib/system";
import { onUsageRefresh, onVaultChanged, openMainPage, showMainWindow, notifySettingsChanged } from "./lib/desktop";
import { usageAdapterFor, usageExtraSecretFor } from "./lib/providers/usage";
import { credentialRemainingPercent } from "./lib/alerts";
import { usageGauges } from "./lib/monitorGauges";
import { persistMonitorOutcome } from "./lib/monitorStore";
import { MonitorCard } from "./components/MonitorCard";
import { vaultStatus } from "./lib/vault";
import { applyTheme, loadSettings, saveSetting } from "./lib/settings";
import mikuIcon from "./assets/miku-icon.png";

const MONITOR_LABELS: Record<string, string> = {
  codex: "Codex",
  grok: "Grok Bot",
  "grok-build": "Grok Build",
  antigravity: "Antigravity",
  opencode: "OpenCode Go",
};

function percentLabel(value: number | null): string {
  if (value === null) return "";
  return `${Math.round(value)}%`;
}

export function MiniApp() {
  const { notify } = useToast();
  const { data, error, reload } = useAsyncData(async () => {
    const [status, usage, monitors, probes, credentials, fields, models, agAccounts] =
      await Promise.all([
        vaultStatus(),
        listUsageSnapshots(),
        listMonitorSnapshots(),
        monitorProbe(),
        listCredentialsWithContext(),
        listAllCredentialFields(),
        listModels(),
        antigravityAccounts(),
      ]);
    return { status, usage, monitors, probes, credentials, fields, models, agAccounts };
  }, "mini");
  const [busy, setBusy] = useState<string | null>(null);
  const [modelsOpen, setModelsOpen] = useState(false);
  const [alertsOn, setAlertsOn] = useState(true);
  const [alertThreshold, setAlertThreshold] = useState(20);
  const autoRefreshedRef = useRef(false);

  useEffect(() => {
    let active = true;
    loadSettings()
      .then((settings) => {
        if (!active) return;
        applyTheme(settings.theme);
        setAlertsOn(settings.alertsEnabled);
        setAlertThreshold(settings.alertThresholdPercent);
      })
      .catch(() => {
        // theme is best-effort while the vault is locked
      });
    return () => {
      active = false;
    };
  }, []);

  const toggleAlerts = async () => {
    const next = !alertsOn;
    setAlertsOn(next);
    try {
      await saveSetting("alertsEnabled", next);
      await notifySettingsChanged();
      notify(
        next
          ? `사용량 임계치 알림을 켰습니다 (${alertThreshold}%)`
          : "사용량 임계치 알림을 껐습니다",
        "success",
      );
    } catch (err) {
      setAlertsOn(!next);
      notify(errorMessage(err), "error");
    }
  };

  useEffect(() => {
    if (!data || autoRefreshedRef.current) return;
    autoRefreshedRef.current = true;
    const stale = data.probes.filter((probe) => {
      if (!probe.available) return false;
      const snapshot = data.monitors.find((item) => item.monitor === probe.monitor);
      if (!snapshot) return true;
      const age = Date.now() - new Date(snapshot.fetched_at).getTime();
      return Number.isNaN(age) || age > 5 * 60 * 1000;
    });
    if (stale.length === 0) return;
    void (async () => {
      await Promise.all(
        stale.map(async (probe) => {
          try {
            const outcome = await monitorRefresh(probe.monitor);
            await persistMonitorOutcome(outcome);
          } catch {
            // auto refresh is best-effort
          }
        }),
      );
      reload();
    })();
  }, [data, reload]);

  useEffect(() => onUsageRefresh(() => reload()), [reload]);

  useEffect(() => onVaultChanged(() => reload()), [reload]);

  const credentialById = useMemo(() => {
    const map = new Map<string, CredentialWithContext>();
    for (const credential of data?.credentials ?? []) map.set(credential.id, credential);
    return map;
  }, [data]);

  const usageRows = useMemo(() => {
    if (!data) return [];
    return data.usage
      .map((snapshot: UsageSnapshot) => {
        const credential = credentialById.get(snapshot.credential_id);
        return {
          snapshot,
          credential,
          remaining: credentialRemainingPercent(snapshot.remaining, snapshot.limit_total),
        };
      })
      .filter((row) => row.credential !== undefined);
  }, [data, credentialById]);

  const monitorByKey = useMemo(() => {
    const map = new Map<string, MonitorSnapshot>();
    for (const snapshot of data?.monitors ?? []) map.set(snapshot.monitor, snapshot);
    return map;
  }, [data]);

  const refreshUsageRow = async (snapshot: UsageSnapshot) => {
    const credential = credentialById.get(snapshot.credential_id);
    if (!credential) return;
    const adapter = usageAdapterFor(
      credential.provider_name,
      credential.provider_display_name,
      credential.provider_base_url,
    );
    if (!adapter) return;
    setBusy(`usage-${credential.id}`);
    try {
      const fields = (data?.fields ?? []).filter(
        (field) => field.credential_id === credential.id,
      );
      const outcome = await fetchUsage(
        adapter,
        credential.provider_base_url,
        credential.secret_id,
        usageExtraSecretFor(adapter, fields),
      );
      await upsertUsageSnapshot({
        credential_id: credential.id,
        adapter,
        status: outcome.status,
        summary: outcome.summary || outcome.message || "",
        used: outcome.used,
        limit_total: outcome.limit,
        remaining: outcome.remaining,
        currency: outcome.currency,
        details_json: outcome.details ? JSON.stringify(outcome.details) : null,
        fetched_at: new Date().toISOString(),
      });
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(null);
    }
  };

  const refreshMonitorRow = async (monitor: string) => {
    setBusy(`monitor-${monitor}`);
    try {
      const outcome = await monitorRefresh(monitor);
      await persistMonitorOutcome(outcome);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(null);
    }
  };

  const switchAccount = async (account: AntigravityAccount) => {
    setBusy("ag-account");
    try {
      await antigravitySwitchAccount(account.id);
      notify(
        `${account.email ?? account.label} 계정으로 전환했습니다. Antigravity 재시작 후 ↻로 갱신하세요`,
        "success",
      );
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(null);
    }
  };

  const deleteAccount = async (account: AntigravityAccount) => {
    setBusy("ag-account");
    try {
      await antigravityDeleteAccount(account.id);
      notify("저장된 계정을 삭제했습니다", "success");
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(null);
    }
  };

  const locked = data !== null && data.status.state !== "unlocked";
  const vaultUnlocked = !locked;

  return (
    <div className="mini">
      <header className="mini-header">
        <div className="mini-brand">
          <img className="mini-brand-icon" src={mikuIcon} alt="" />
          <strong>API Desk Mini</strong>
          {locked ? (
            <span className="mini-lock" title="Vault 잠김 · 계정 쿼터는 비밀번호 없이 조회됩니다">
              🔒
            </span>
          ) : null}
        </div>
        <div className="row-actions">
          <button
            type="button"
            className={`button button-small ${alertsOn ? "button-on" : ""}`}
            aria-pressed={alertsOn}
            title={`사용량 임계치 알림 ${alertsOn ? "켜짐" : "꺼짐"} (임계치 ${alertThreshold}%)`}
            onClick={() => void toggleAlerts()}
          >
            {alertsOn ? "🔔" : "🔕"}
          </button>
          <button
            type="button"
            className="button button-small"
            title="메인 창에서 API 관리 열기"
            onClick={() => void openMainPage("apis")}
          >
            API
          </button>
          <button
            type="button"
            className="button button-small"
            disabled={busy !== null}
            onClick={() => reload()}
          >
            ↻
          </button>
          <button
            type="button"
            className="button button-small"
            onClick={() => void showMainWindow()}
          >
            메인 창
          </button>
        </div>
      </header>

      {error ? (
        <p className="mini-note mini-error">
          데이터를 불러오지 못했습니다: {error}
          <button type="button" className="button button-small" onClick={() => reload()}>
            다시 시도
          </button>
        </p>
      ) : null}

      <div className="mini-body">
        <section className="mini-section">
          <h4>계정 쿼터</h4>
          {(data?.probes ?? []).map((probe) => (
            <MonitorCard
              key={probe.monitor}
              monitor={probe.monitor}
              label={MONITOR_LABELS[probe.monitor] ?? probe.label}
              available={probe.available}
              detail={probe.detail}
              snapshot={monitorByKey.get(probe.monitor) ?? null}
              busy={busy === `monitor-${probe.monitor}`}
              onRefresh={() => void refreshMonitorRow(probe.monitor)}
              accounts={data?.agAccounts ?? []}
              allSnapshots={data?.monitors ?? []}
              showModels
              compact
              accountBusy={busy === "ag-account"}
              onSwitchAccount={(account) => void switchAccount(account)}
              onDeleteAccount={(account) => void deleteAccount(account)}
            />
          ))}
        </section>

        <section className="mini-section">
          <button
            type="button"
            className="mini-section-toggle"
            onClick={() => setModelsOpen((current) => !current)}
          >
            <h4>모델 ({data?.models.length ?? 0})</h4>
            <span>{modelsOpen ? "▾" : "▸"}</span>
          </button>
          {modelsOpen ? (
            (data?.models ?? []).length === 0 ? (
              <p className="mini-note">등록된 모델이 없습니다 (모델 페이지에서 추가)</p>
            ) : (
              <>
                {(data?.models ?? []).slice(0, 8).map((model) => (
                  <div key={model.id} className="mini-model-row">
                    <span>
                      <span
                        className={`mini-dot ${model.is_active === 1 ? "mini-dot-on" : ""}`}
                      />
                      {model.display_name}
                    </span>
                    <span className="muted">{model.provider_display_name}</span>
                  </div>
                ))}
                {(data?.models.length ?? 0) > 8 ? (
                  <p className="mini-note">+{(data?.models.length ?? 0) - 8}개 더</p>
                ) : null}
              </>
            )
          ) : null}
        </section>

        <section className="mini-section">
          <h4>
            API 사용량
            {!vaultUnlocked ? (
              <span className="stamp"> 잠금 중 · 마지막 조회 값</span>
            ) : null}
          </h4>
          {usageRows.length === 0 ? (
            <p className="mini-note">조회된 API 사용량이 없습니다 (모델 페이지에서 조회)</p>
          ) : (
            usageRows.map(({ snapshot, credential, remaining }) => (
              <div key={snapshot.credential_id} className="mini-row">
                <div className="mini-row-main">
                  <span className="mini-row-title">
                    {credential?.name ?? snapshot.adapter}
                    {remaining !== null ? (
                      <Badge tone={remaining <= 20 ? "warn" : "muted"}>
                        {percentLabel(remaining)}
                      </Badge>
                    ) : null}
                  </span>
                  <SummaryChips text={snapshot.summary || "미조회"} className="mono" />
                  {usageGauges(snapshot.adapter, snapshot.details_json).length > 0 ? (
                    <GaugeList items={usageGauges(snapshot.adapter, snapshot.details_json)} />
                  ) : null}
                  {snapshot ? (
                    <span
                      className="mini-row-time"
                      title={`최근 조회 ${formatDateTime(snapshot.fetched_at)}`}
                    >
                      {formatRelativeTime(snapshot.fetched_at)}
                    </span>
                  ) : null}
                </div>
                <button
                  type="button"
                  className="button button-small"
                  disabled={busy === `usage-${snapshot.credential_id}`}
                  title={
                    vaultUnlocked
                      ? "사용량 새로고침"
                      : "Vault 잠금 해제 후 새로고침할 수 있습니다"
                  }
                  onClick={() => {
                    if (!vaultUnlocked) {
                      notify(
                        "Vault가 잠겨 있습니다. 메인 창에서 잠금을 해제하면 새로고침할 수 있습니다",
                        "error",
                      );
                      void showMainWindow();
                      return;
                    }
                    void refreshUsageRow(snapshot);
                  }}
                >
                  {busy === `usage-${snapshot.credential_id}` ? "…" : vaultUnlocked ? "↻" : "🔒"}
                </button>
              </div>
            ))
          )}
        </section>
      </div>
    </div>
  );
}
