import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Badge } from "../components/Badges";
import { EmptyState } from "../components/EmptyState";
import { useToast } from "../components/ToastProvider";
import { useAsyncData } from "../hooks/useAsyncData";
import { errorMessage } from "../lib/errors";
import {
  apiKeyLabel,
  checkLocalServiceHealth,
  clearLocalServiceApiKey,
  formatUptime,
  localServiceConfigs,
  localServiceDefinitions,
  localServiceLogs,
  localServiceStatuses,
  onLocalServiceStatusChanged,
  restartLocalService,
  saveLocalServiceConfig,
  serviceStateLabel,
  serviceStateTone,
  setLocalServiceApiKey,
  startLocalService,
  stopLocalService,
  type LocalServiceConfig,
  type LocalServiceDefinition,
  type LocalServiceStatus,
  type ServiceHealth,
} from "../lib/localServices";

const POLL_MS = 2_000;

interface ServiceDraft {
  executable: string;
  args: string;
  workdir: string;
  endpoint: string;
  port: string;
  device: string;
  healthPath: string;
  autoStart: boolean;
  keepAliveOnExit: boolean;
}

function draftFrom(config: LocalServiceConfig): ServiceDraft {
  return {
    executable: config.executable,
    args: config.args.join(" "),
    workdir: config.workdir,
    endpoint: config.endpoint,
    port: String(config.port),
    device: config.device,
    healthPath: config.healthPath,
    autoStart: config.autoStart,
    keepAliveOnExit: config.keepAliveOnExit,
  };
}

export function LocalServicesPage() {
  const { notify } = useToast();
  const { data, error, reload } = useAsyncData(async () => {
    const [defs, list, configs] = await Promise.all([
      localServiceDefinitions(),
      localServiceStatuses(),
      localServiceConfigs(),
    ]);
    return { defs, list, configs };
  }, "local-services");

  const [live, setLive] = useState<LocalServiceStatus[] | null>(null);
  const [drafts, setDrafts] = useState<Record<string, ServiceDraft>>({});
  const [logs, setLogs] = useState<Record<string, string[]>>({});
  const [health, setHealth] = useState<Record<string, ServiceHealth>>({});
  const [keyInput, setKeyInput] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState<Record<string, boolean>>({});
  const [logsOpen, setLogsOpen] = useState<Record<string, boolean>>({});
  const logsOpenRef = useRef<Record<string, boolean>>({});

  // 메인/미니 어느 쪽에서 바뀌어도 즉시 반영(이벤트) + 이벤트 누락 대비 3초 폴링
  useEffect(
    () =>
      onLocalServiceStatusChanged(() => {
        void reload();
      }),
    [reload],
  );

  useEffect(() => {
    const timer = window.setInterval(() => {
      void localServiceStatuses()
        .then((list) => setLive(list))
        .catch(() => {});
      for (const [id, open] of Object.entries(logsOpenRef.current)) {
        if (!open) continue;
        void localServiceLogs(id, 200)
          .then((lines) => setLogs((current) => ({ ...current, [id]: lines })))
          .catch(() => {});
      }
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, []);

  const definitions = useMemo(() => data?.defs ?? [], [data]);
  const configs = useMemo(() => data?.configs ?? [], [data]);
  const statuses = live ?? data?.list ?? [];

  const definitionById = useMemo(() => {
    const map = new Map<string, LocalServiceDefinition>();
    for (const definition of definitions) map.set(definition.id, definition);
    return map;
  }, [definitions]);

  const configById = useMemo(() => {
    const map = new Map<string, LocalServiceConfig>();
    for (const config of configs) map.set(config.id, config);
    return map;
  }, [configs]);

  const draftFor = useCallback(
    (config: LocalServiceConfig): ServiceDraft => drafts[config.id] ?? draftFrom(config),
    [drafts],
  );

  const updateDraft = (id: string, patch: Partial<ServiceDraft>) => {
    setDrafts((current) => {
      const base = current[id];
      if (!base) return current;
      return { ...current, [id]: { ...base, ...patch } };
    });
  };

  const ensureDraft = (config: LocalServiceConfig) => {
    setDrafts((current) =>
      current[config.id] ? current : { ...current, [config.id]: draftFrom(config) },
    );
  };

  const refreshAll = useCallback(async () => {
    await reload();
    setLive(null);
  }, [reload]);

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await action();
      await refreshAll();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(null);
    }
  };

  const toggleLogs = async (id: string) => {
    const next = !logsOpenRef.current[id];
    logsOpenRef.current = { ...logsOpenRef.current, [id]: next };
    setLogsOpen((current) => ({ ...current, [id]: next }));
    if (!next) return;
    try {
      const lines = await localServiceLogs(id, 200);
      setLogs((current) => ({ ...current, [id]: lines }));
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const saveConfig = async (definition: LocalServiceDefinition, draft: ServiceDraft) => {
    const port = Number(draft.port);
    if (!Number.isFinite(port) || port <= 0 || port > 65_535) {
      notify("포트 번호를 확인하세요 (1~65535)", "error");
      return;
    }
    const config: LocalServiceConfig = {
      id: definition.id,
      executable: draft.executable.trim(),
      args: draft.args
        .split(" ")
        .map((arg) => arg.trim())
        .filter((arg) => arg.length > 0),
      workdir: draft.workdir.trim(),
      endpoint: draft.endpoint.trim(),
      port: Math.round(port),
      device: draft.device.trim(),
      healthPath: draft.healthPath.trim(),
      autoStart: draft.autoStart,
      keepAliveOnExit: draft.keepAliveOnExit,
    };
    await run(`save-${definition.id}`, async () => {
      await saveLocalServiceConfig(config);
      setDrafts((current) => ({ ...current, [definition.id]: draftFrom(config) }));
      notify(`${definition.label} 설정을 저장했습니다`, "success");
    });
  };

  const storeKey = async (definition: LocalServiceDefinition) => {
    const value = (keyInput[definition.id] ?? "").trim();
    if (value.length === 0) {
      notify("저장할 API 키를 입력하세요", "error");
      return;
    }
    await run(`key-${definition.id}`, async () => {
      await setLocalServiceApiKey(definition.id, value);
      setKeyInput((current) => ({ ...current, [definition.id]: "" }));
      notify(`${definition.apiKeyLabel}를 Vault에 저장했습니다`, "success");
    });
  };

  const clearKey = async (definition: LocalServiceDefinition) => {
    await run(`key-${definition.id}`, async () => {
      await clearLocalServiceApiKey(definition.id);
      notify(`${definition.apiKeyLabel}를 삭제했습니다`, "success");
    });
  };

  return (
    <div className="page">
      <section className="panel">
        <header className="panel-header">
          <h3>로컬 서비스</h3>
          <button
            type="button"
            className="button button-small"
            disabled={busy !== null}
            onClick={() => void refreshAll()}
          >
            ↻ 새로고침
          </button>
        </header>
        <p className="panel-hint">
          이 PC에서 상주하는 로컬 AI 서비스를 API Desk에서 직접 실행·중지하고 상태를 확인합니다.
          실행 파일 경로만 지정하면 바로 실행할 수 있고, API 키는 선택 사항입니다. Vault에 키를
          저장하면 자식 프로세스 환경변수로만 주입되어 Bearer 인증으로 동작하고, 키가 없으면
          주입 없이 localhost 전용 무인증 모드로 실행합니다 (화면·로그에 평문 노출 없음).
        </p>
        {error ? <p className="page-error">{error}</p> : null}
        {definitions.length === 0 && !error ? (
          <EmptyState
            text="등록된 로컬 서비스가 없습니다"
            hint="서비스 정의는 백엔드(local_services.rs)에 등록됩니다. Laya가 기본 제공됩니다."
          />
        ) : null}
      </section>

      {statuses.map((status) => {
        const definition = definitionById.get(status.id);
        const config = configById.get(status.id);
        if (!definition || !config) return null;
        const draft = draftFor(config);
        const running = status.state === "running" || status.state === "starting";
        const healthResult = health[status.id];
        return (
          <section className="panel service-card" key={status.id}>
            <header className="panel-header service-head">
              <div className="service-title">
                <h3>{status.label}</h3>
                <Badge tone={serviceStateTone(status.state)}>
                  {serviceStateLabel(status.state)}
                </Badge>
                {status.managed ? <Badge tone="info">API Desk 관리</Badge> : null}
                {!status.managed && status.pid ? (
                  <Badge tone="muted">외부 프로세스</Badge>
                ) : null}
              </div>
              <div className="row-actions">
                <button
                  type="button"
                  className="button button-small button-primary"
                  disabled={busy !== null || running || !status.executableSet}
                  title={
                    status.executableSet
                      ? undefined
                      : "설정에서 실행 파일 경로를 먼저 지정하세요"
                  }
                  onClick={() =>
                    void run(`start-${status.id}`, () => startLocalService(status.id))
                  }
                >
                  Start
                </button>
                <button
                  type="button"
                  className="button button-small"
                  disabled={busy !== null || (!running && !status.pid)}
                  onClick={() =>
                    void run(`stop-${status.id}`, () => stopLocalService(status.id))
                  }
                >
                  Stop
                </button>
                <button
                  type="button"
                  className="button button-small"
                  disabled={busy !== null || !status.executableSet}
                  onClick={() =>
                    void run(`restart-${status.id}`, () => restartLocalService(status.id))
                  }
                >
                  Restart
                </button>
                <button
                  type="button"
                  className="button button-small"
                  disabled={busy !== null}
                  onClick={() =>
                    void run(`health-${status.id}`, async () => {
                      const result = await checkLocalServiceHealth(status.id);
                      setHealth((current) => ({ ...current, [status.id]: result }));
                      notify(
                        result.ok
                          ? `${status.label} 정상 (HTTP ${result.status}, ${result.latencyMs}ms)`
                          : `${status.label} 응답 없음 또는 오류 (HTTP ${result.status})`,
                        result.ok ? "success" : "error",
                      );
                    })
                  }
                >
                  Health Check
                </button>
                <button
                  type="button"
                  className="button button-small button-ghost"
                  onClick={() => {
                    ensureDraft(config);
                    setSettingsOpen((current) => ({
                      ...current,
                      [status.id]: !current[status.id],
                    }));
                  }}
                >
                  {settingsOpen[status.id] ? "설정 닫기" : "설정"}
                </button>
              </div>
            </header>

            <p className="panel-hint">{status.description}</p>
            {status.lastError ? <p className="page-error">{status.lastError}</p> : null}

            <div className="service-info">
              <div>
                <span className="stat-label">Endpoint</span>
                <span className="mono">{status.endpoint}</span>
              </div>
              <div>
                <span className="stat-label">Port</span>
                <span className="mono">{status.port}</span>
              </div>
              <div>
                <span className="stat-label">Device</span>
                <span className="mono">{status.device}</span>
              </div>
              <div>
                <span className="stat-label">PID</span>
                <span className="mono">{status.pid ?? "—"}</span>
              </div>
              <div>
                <span className="stat-label">가동 시간</span>
                <span className="mono">{formatUptime(status.uptimeSecs)}</span>
              </div>
              <div>
                <span className="stat-label">{definition.apiKeyLabel}</span>
                <span className="mono">{apiKeyLabel(status)}</span>
              </div>
              {healthResult ? (
                <div>
                  <span className="stat-label">헬스체크</span>
                  <span className="mono">
                    {healthResult.ok ? "정상" : "실패"} · HTTP {healthResult.status} ·{" "}
                    {healthResult.latencyMs}ms
                  </span>
                </div>
              ) : null}
            </div>

            <div className="api-key-row">
              <input
                className="input"
                type="password"
                autoComplete="off"
                placeholder={`${definition.apiKeyLabel} 값을 입력해 Vault에 저장`}
                value={keyInput[status.id] ?? ""}
                onChange={(event) =>
                  setKeyInput((current) => ({ ...current, [status.id]: event.target.value }))
                }
              />
              <button
                type="button"
                className="button button-small"
                disabled={busy !== null || status.vaultLocked}
                title={status.vaultLocked ? "Vault 잠금 해제 후 저장할 수 있습니다" : undefined}
                onClick={() => void storeKey(definition)}
              >
                키 저장
              </button>
              <button
                type="button"
                className="button button-small button-danger-ghost"
                disabled={busy !== null || status.vaultLocked || !status.apiKeySet}
                onClick={() => void clearKey(definition)}
              >
                키 삭제
              </button>
            </div>

            <div className="service-env mono">
              주입 환경변수:{" "}
              {[
                ...definition.staticEnv.map((item) => `${item.key}=${item.value}`),
                `${definition.deviceEnv}=${status.device}`,
                `${definition.secretEnv}=${status.apiKeySet ? "<Vault 값>" : "(주입 안 함)"}`,
              ].join(" · ")}
            </div>

            {settingsOpen[status.id] ? (
              <div className="service-settings">
                <label className="field">
                  <span>실행 파일 / venv 파이썬 경로</span>
                  <input
                    className="input mono"
                    placeholder="예: C:\\laya\\venv\\Scripts\\python.exe"
                    value={draft.executable}
                    onChange={(event) => updateDraft(status.id, { executable: event.target.value })}
                  />
                </label>
                <label className="field">
                  <span>실행 인자 (공백 구분)</span>
                  <input
                    className="input mono"
                    placeholder="예: -m laya.server --port 8000"
                    value={draft.args}
                    onChange={(event) => updateDraft(status.id, { args: event.target.value })}
                  />
                </label>
                <label className="field">
                  <span>작업 디렉터리 (선택)</span>
                  <input
                    className="input mono"
                    value={draft.workdir}
                    onChange={(event) => updateDraft(status.id, { workdir: event.target.value })}
                  />
                </label>
                <div className="field-row">
                  <label className="field">
                    <span>Endpoint</span>
                    <input
                      className="input mono"
                      value={draft.endpoint}
                      onChange={(event) => updateDraft(status.id, { endpoint: event.target.value })}
                    />
                  </label>
                  <label className="field">
                    <span>Port</span>
                    <input
                      className="input input-narrow mono"
                      value={draft.port}
                      onChange={(event) => updateDraft(status.id, { port: event.target.value })}
                    />
                  </label>
                  <label className="field">
                    <span>Device</span>
                    <input
                      className="input input-narrow mono"
                      value={draft.device}
                      onChange={(event) => updateDraft(status.id, { device: event.target.value })}
                    />
                  </label>
                  <label className="field">
                    <span>Health 경로</span>
                    <input
                      className="input input-narrow mono"
                      value={draft.healthPath}
                      onChange={(event) =>
                        updateDraft(status.id, { healthPath: event.target.value })
                      }
                    />
                  </label>
                </div>
                <label className="field field-check">
                  <input
                    type="checkbox"
                    checked={draft.autoStart}
                    onChange={(event) => updateDraft(status.id, { autoStart: event.target.checked })}
                  />
                  <span>API Desk 시작 시 자동 실행 (Vault 잠금 해제 후)</span>
                </label>
                <label className="field field-check">
                  <input
                    type="checkbox"
                    checked={draft.keepAliveOnExit}
                    onChange={(event) =>
                      updateDraft(status.id, { keepAliveOnExit: event.target.checked })
                    }
                  />
                  <span>API Desk를 종료해도 서비스를 유지</span>
                </label>
                <div className="row-actions">
                  <button
                    type="button"
                    className="button button-small button-primary"
                    disabled={busy !== null}
                    onClick={() => void saveConfig(definition, draft)}
                  >
                    설정 저장
                  </button>
                  <span className="form-hint">
                    Windows 로그인 시 자동 실행은 추후 지원 예정입니다 (인터페이스 분리됨)
                  </span>
                </div>
              </div>
            ) : null}

            <div className="service-logs">
              <button
                type="button"
                className="mini-models-toggle"
                onClick={() => void toggleLogs(status.id)}
              >
                <span>최근 로그 ({status.logLines})</span>
                <span>{logsOpen[status.id] ? "▾" : "▸"}</span>
              </button>
              {logsOpen[status.id] ? (
                <pre className="log-view mono">
                  {(logs[status.id] ?? []).length === 0
                    ? "로그가 없습니다. Start를 누르면 표준 출력/오류를 수집합니다."
                    : (logs[status.id] ?? []).join("\n")}
                </pre>
              ) : null}
            </div>
          </section>
        );
      })}
    </div>
  );
}
