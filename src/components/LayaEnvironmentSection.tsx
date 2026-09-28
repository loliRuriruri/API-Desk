import { useCallback, useEffect, useState } from "react";
import { Badge } from "./Badges";
import { Modal } from "./Modal";
import { useToast } from "./ToastProvider";
import { errorMessage } from "../lib/errors";
import {
  benchmarkSummary,
  backendLabel,
  deviceLabel,
  environmentSummary,
  layaEnvActivate,
  layaEnvBenchmark,
  layaEnvCancel,
  layaEnvCheckUpdates,
  layaEnvCreateGpu,
  layaEnvList,
  layaEnvRollback,
  layaEnvStatus,
  onLayaEnvProgress,
  versionLabel,
  type LayaBenchmark,
  type LayaEnvEntry,
  type LayaEnvProgress,
  type LayaEnvironmentStatus,
} from "../lib/layaEnv";

interface LayaEnvironmentSectionProps {
  onChanged: () => void;
}

export function LayaEnvironmentSection({ onChanged }: LayaEnvironmentSectionProps) {
  const { notify } = useToast();
  const [status, setStatus] = useState<LayaEnvironmentStatus | null>(null);
  const [envs, setEnvs] = useState<LayaEnvEntry[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState<LayaEnvProgress[]>([]);
  const [resultOpen, setResultOpen] = useState(false);
  const [benchmark, setBenchmark] = useState<LayaBenchmark | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [next, list] = await Promise.all([layaEnvStatus(), layaEnvList()]);
      setStatus(next);
      setEnvs(list);
    } catch (error) {
      notify(errorMessage(error), "error");
    }
  }, [notify]);

  useEffect(() => {
    let active = true;
    const tick = async () => {
      try {
        const [next, list] = await Promise.all([layaEnvStatus(), layaEnvList()]);
        if (!active) return;
        setStatus(next);
        setEnvs(list);
      } catch (error) {
        if (active) notify(errorMessage(error), "error");
      }
    };
    void tick();
    const off = onLayaEnvProgress((event) => {
      setProgress((current) => [...current.slice(-40), event]);
    });
    return () => {
      active = false;
      off();
    };
  }, [notify]);

  const run = async (label: string, action: () => Promise<void>, done?: () => void) => {
    setBusy(label);
    setProgress((current) => [...current, { step: label, label, phase: "running", detail: null, at: "" }]);
    try {
      await action();
      done?.();
    } catch (error) {
      notify(errorMessage(error), "error");
    } finally {
      setBusy(null);
      void refresh();
    }
  };

  const candidate = envs.find((env) => env.state === "ready" && !env.active);
  const hasPrevious = envs.some((env) => env.state === "active" && env.kind === "gpu")
    ? envs.some((env) => env.state === "ready")
    : false;

  const info = status;
  const backend = info?.backend ?? (info?.cudaAvailable ? "torch" : "cpu");
  const device = info?.effectiveDevice ?? (info?.cudaAvailable ? "cuda" : "cpu");
  const healthy = backend === "tilelang" && device === "cuda";

  return (
    <div className="laya-env">
      <div className="laya-env-head">
        <strong>Laya 환경</strong>
        <Badge tone={healthy ? "ok" : info?.cudaAvailable ? "warn" : "danger"}>
          {environmentSummary(info)}
        </Badge>
      </div>

      <div className="service-info laya-env-grid">
        <div>
          <span className="stat-label">설치 버전</span>
          <span className="mono">
            {versionLabel(info?.layaVersion ?? null, info?.latestLayaVersion ?? null, Boolean(info?.updateAvailable))}
          </span>
        </div>
        <div>
          <span className="stat-label">Python</span>
          <span className="mono">{info?.pythonVersion ?? "확인 중"}</span>
        </div>
        <div>
          <span className="stat-label">PyTorch</span>
          <span className="mono">
            {info?.torchVersion ?? "확인 중"}
            {info?.torchVersion?.includes("+cpu") ? " ⚠ CPU 전용" : ""}
          </span>
        </div>
        <div>
          <span className="stat-label">CUDA Build</span>
          <span className="mono">{info?.torchCudaVersion ?? "없음"}</span>
        </div>
        <div>
          <span className="stat-label">GPU</span>
          <span className="mono">{info?.gpuName ?? "없음"}</span>
        </div>
        <div>
          <span className="stat-label">TileLang</span>
          <span className="mono">
            {info?.tilelangInstalled ? `설치됨${info.tilelangVersion ? ` ${info.tilelangVersion}` : ""}` : "미설치"}
          </span>
        </div>
        <div>
          <span className="stat-label">백엔드</span>
          <span className="mono">{backendLabel(backend)}</span>
        </div>
        <div>
          <span className="stat-label">실제 Device</span>
          <span className="mono">{deviceLabel(device)}</span>
        </div>
      </div>

      {info?.versionCheckError ? <p className="panel-hint">{info.versionCheckError}</p> : null}
      {info && info.problems.length > 0 ? (
        <ul className="laya-env-problems">
          {info.problems.map((item) => (
            <li key={item}>⚠ {item}</li>
          ))}
        </ul>
      ) : null}

      {envs.length > 0 ? (
        <table className="table table-compact">
          <thead>
            <tr>
              <th>환경</th>
              <th>버전</th>
              <th>백엔드</th>
              <th>상태</th>
            </tr>
          </thead>
          <tbody>
            {envs.map((env) => (
              <tr key={env.path}>
                <td className="mono">{env.path.split("\\").slice(-1)[0]}</td>
                <td className="mono">{env.version ?? "-"}</td>
                <td className="mono">
                  {(() => {
                    const metrics = env.metrics as { backend?: string } | null;
                    return metrics?.backend ? backendLabel(metrics.backend) : "-";
                  })()}
                </td>
                <td>
                  <Badge tone={env.active ? "ok" : env.state === "ready" ? "info" : "muted"}>
                    {env.active ? "활성" : env.state === "ready" ? "검증됨" : "불완전"}
                  </Badge>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}

      <div className="row-actions">
        <button
          type="button"
          className="button button-small"
          disabled={busy !== null}
          onClick={() => void run("check", async () => {
            const check = await layaEnvCheckUpdates(true);
            notify(
              check.latest
                ? `최신 stable: ${check.latest}${check.updateAvailable ? " (업데이트 있음)" : ""}`
                : check.error ?? "최신 버전을 확인할 수 없습니다",
              check.latest ? "info" : "error",
            );
          })}
        >
          {busy === "check" ? "확인 중…" : "업데이트 확인"}
        </button>
        <button
          type="button"
          className="button button-small"
          disabled={busy !== null}
          onClick={() => void run("create", () => layaEnvCreateGpu())}
        >
          {busy === "create" ? "환경 구성 중…" : "GPU 환경 구성/복구"}
        </button>
        <button
          type="button"
          className="button button-small"
          disabled={busy !== null || !candidate}
          title={candidate ? `${candidate.path} 활성화` : "검증된 후보 환경이 없습니다"}
          onClick={() =>
            void run("activate", async () => {
              if (!candidate) return;
              await layaEnvActivate(candidate.path);
              onChanged();
              setResultOpen(true);
              notify("Laya 환경을 활성화했습니다", "success");
            })
          }
        >
          {busy === "activate" ? "활성화 중…" : "검증된 후보 활성화"}
        </button>
        <button
          type="button"
          className="button button-small"
          disabled={busy !== null}
          onClick={() =>
            void run("restart", async () => {
              await layaEnvActivate(
                envs.find((env) => env.active)?.path ?? (status?.pythonPath ? "" : ""),
              );
              onChanged();
            }, () => notify("재시작 + 검증을 완료했습니다", "success"))
          }
          hidden
        >
          재시작 + 검증
        </button>
        <button
          type="button"
          className="button button-small"
          disabled={busy !== null}
          onClick={() =>
            void run("bench", async () => {
              const result = await layaEnvBenchmark();
              setBenchmark(result);
              notify(benchmarkSummary(result) ?? "벤치마크 완료", result.error ? "error" : "info");
            })
          }
        >
          {busy === "bench" ? "측정 중…" : "성능 테스트"}
        </button>
        <button
          type="button"
          className="button button-small button-ghost"
          disabled={busy !== null || !hasPrevious}
          onClick={() =>
            void run("rollback", async () => {
              await layaEnvRollback();
              onChanged();
              notify("이전 버전으로 되돌렸습니다", "info");
            })
          }
        >
          {busy === "rollback" ? "되돌리는 중…" : "이전 버전으로 되돌리기"}
        </button>
        {busy === "create" ? (
          <button
            type="button"
            className="button button-small button-danger-ghost"
            onClick={() => void layaEnvCancel()}
          >
            취소
          </button>
        ) : null}
      </div>

      {progress.length > 0 ? (
        <div className="laya-env-log mono">
          {progress.slice(-8).map((event, index) => (
            <div key={`${event.at}-${index}`} className={`laya-env-log-line ${event.phase}`}>
              {event.phase === "running" ? "…" : event.phase === "done" ? "✓" : "✕"} {event.label}
              {event.detail ? ` — ${event.detail}` : ""}
            </div>
          ))}
        </div>
      ) : null}

      {resultOpen ? (
      <Modal
        title="Laya 환경 활성화 완료"
        onClose={() => setResultOpen(false)}
        footer={
          <button type="button" className="button" onClick={() => setResultOpen(false)}>
            완료
          </button>
        }
      >
        <ul className="laya-env-result">
          <li>Laya {info?.layaVersion ?? "-"}</li>
          <li>GPU {info?.gpuName ?? "-"} {info?.effectiveDevice === "cuda" ? "PASS" : "FAIL"}</li>
          <li>CUDA {info?.cudaAvailable ? "PASS" : "FAIL"}</li>
          <li>TileLang {info?.tilelangInstalled ? "PASS" : "FAIL"}</li>
          <li>Fast Path {info?.backend === "tilelang" ? "PASS" : "FAIL"}</li>
          <li>Service {info?.serviceRunning ? "PASS" : "FAIL"}</li>
        </ul>
        {benchmark ? <p className="panel-hint">{benchmarkSummary(benchmark)}</p> : null}
      </Modal>
      ) : null}

      {benchmark ? (
        <p className="panel-hint">
          마지막 벤치마크: {benchmarkSummary(benchmark)} · {benchmark.loadedModels.join(", ")}
        </p>
      ) : null}
    </div>
  );
}
