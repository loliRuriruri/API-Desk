import { useEffect, useState } from "react";
import { Badge } from "./Badges";
import { GaugeBar } from "./Gauge";
import { errorMessage } from "../lib/errors";
import {
  attributionLabel,
  confidenceLabel,
  formatBytes,
  gpuSnapshot,
  modelsLabel,
  vramPercent,
  vramTone,
  type GpuSnapshot,
} from "../lib/gpuMonitor";

const POLL_MS = 1_000;
const COLLAPSED_ROWS = 6;

function unavailableText(snapshot: GpuSnapshot | null): string {
  if (!snapshot) return "GPU 정보를 불러오는 중…";
  if (snapshot.available) return "";
  if (snapshot.reason === "no_gpu") return "지원 GPU 없음";
  return snapshot.detail ?? "NVIDIA GPU 정보를 사용할 수 없음";
}

export function GpuMonitorPanel() {
  const [snapshot, setSnapshot] = useState<GpuSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    let active = true;
    const tick = async () => {
      if (document.visibilityState !== "visible") return;
      try {
        const data = await gpuSnapshot("main", true);
        if (!active) return;
        setSnapshot(data);
        setError(null);
      } catch (err) {
        if (active) setError(errorMessage(err));
      }
    };
    void tick();
    const timer = window.setInterval(() => void tick(), POLL_MS);
    const onVisibility = () => void tick();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      active = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  if (error) {
    return (
      <section className="panel gpu-card">
        <header className="panel-header">
          <h3>GPU · VRAM</h3>
        </header>
        <p className="page-error">{error}</p>
      </section>
    );
  }

  if (!snapshot || !snapshot.available) {
    return (
      <section className="panel gpu-card">
        <header className="panel-header">
          <h3>GPU · VRAM</h3>
          {snapshot ? <Badge tone="muted">사용 불가</Badge> : null}
        </header>
        <p className="panel-hint">{unavailableText(snapshot)}</p>
      </section>
    );
  }

  const rows = showAll ? snapshot.processes : snapshot.processes.slice(0, COLLAPSED_ROWS);

  return (
    <section className="panel gpu-card">
      <header className="panel-header">
        <h3>GPU · VRAM</h3>
        <span className="stamp" title={`수집: ${snapshot.source}`}>
          {snapshot.source} · {snapshot.processes.length}개 프로세스
        </span>
      </header>

      {snapshot.gpus.map((info) => {
        const percent = vramPercent(info);
        return (
          <div className="gpu-block" key={info.index}>
            <div className="gpu-head">
              <strong>{info.name}</strong>
              <Badge tone="muted">GPU {info.index}</Badge>
              {info.temperatureC !== null ? (
                <Badge tone="muted">{info.temperatureC}°C</Badge>
              ) : null}
              {info.powerWatts !== null ? (
                <Badge tone="muted">{info.powerWatts.toFixed(0)}W</Badge>
              ) : null}
            </div>
            <div className="gpu-meters">
              <span className="gauge-label">VRAM</span>
              <GaugeBar percent={percent ?? 0} mode="used" />
              <span className="gauge-value mono">
                {formatBytes(info.memoryUsedBytes)} / {formatBytes(info.memoryTotalBytes)}
                {percent !== null ? ` · ${Math.round(percent)}%` : ""}
              </span>
              <Badge tone={vramTone(percent)}>{percent !== null ? `${Math.round(percent)}%` : "N/A"}</Badge>
            </div>
            <div className="gpu-meters">
              <span className="gauge-label">GPU</span>
              <GaugeBar percent={info.utilizationPercent ?? 0} mode="used" />
              <span className="gauge-value mono">
                {info.utilizationPercent !== null ? `${info.utilizationPercent}%` : "N/A"}
              </span>
              <span className="gauge-hint">
                여유 {formatBytes(info.memoryFreeBytes)}
              </span>
            </div>
          </div>
        );
      })}

      <div className="gpu-processes">
        <div className="gpu-process-row gpu-process-head">
          <span>PID</span>
          <span>워크로드</span>
          <span>모델</span>
          <span>출처/신뢰도</span>
          <span>VRAM</span>
        </div>
        {rows.map((process) => {
          const models = modelsLabel(process);
          return (
            <div className="gpu-process-row" key={`${process.pid}-${process.serviceKind}`}>
              <span className="mono">{process.pid}</span>
              <span>
                {attributionLabel(process)}
                {process.serviceKind === "managed_service" ? (
                  <Badge tone="info">{process.isServiceRoot ? "관리" : "하위"}</Badge>
                ) : null}
              </span>
              <span className="mono">{models ?? "—"}</span>
              <span className="muted">
                {process.modelSource ?? "—"} · {confidenceLabel(process.confidence)}
              </span>
              <span className="mono">{formatBytes(process.usedVramBytes)}</span>
            </div>
          );
        })}
        {snapshot.otherCount > 0 ? (
          <div className="gpu-process-row">
            <span className="mono">—</span>
            <span>Other</span>
            <span className="mono">—</span>
            <span className="muted">집계</span>
            <span className="mono">{snapshot.otherCount}개</span>
          </div>
        ) : null}
        {snapshot.processes.length > COLLAPSED_ROWS ? (
          <button
            type="button"
            className="button button-small"
            onClick={() => setShowAll((current) => !current)}
          >
            {showAll ? "접기" : `전체 보기 (+${snapshot.processes.length - COLLAPSED_ROWS})`}
          </button>
        ) : null}
      </div>
      <p className="panel-hint">
        프로세스별 VRAM은 Windows(WDDM)에서 제공되지 않으면 N/A로 표시합니다(0으로 표시하지 않음).
      </p>
    </section>
  );
}
