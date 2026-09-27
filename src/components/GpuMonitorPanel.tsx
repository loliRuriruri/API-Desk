import { useEffect, useState } from "react";
import { Badge } from "./Badges";
import { GaugeBar } from "./Gauge";
import { errorMessage } from "../lib/errors";
import {
  attributionLabel,
  classificationLabel,
  classificationTone,
  confidenceLabel,
  engineLabel,
  formatBytes,
  gpuSnapshot,
  modelsLabel,
  runtimeLabel,
  sortProcesses,
  sourcesLabel,
  type GpuSnapshot,
  type ProcessSortKey,
  vramPercent,
  vramSourceLabel,
  vramTone,
} from "../lib/gpuMonitor";

const POLL_MS = 1_000;
const COLLAPSED_ROWS = 8;

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
  const [sortKey, setSortKey] = useState<ProcessSortKey>("gpu");

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

  const ordered = sortProcesses(snapshot.processes, sortKey);
  const rows = showAll ? ordered : ordered.slice(0, COLLAPSED_ROWS);

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
              <span className="gauge-hint">여유 {formatBytes(info.memoryFreeBytes)}</span>
            </div>
          </div>
        );
      })}

      <div className="gpu-processes">
        <div className="gpu-process-toolbar">
          <span className="panel-hint">GPU 프로세스</span>
          <div className="gpu-process-sort">
            <button
              type="button"
              className={sortKey === "gpu" ? "button-small button-on" : "button-small"}
              onClick={() => setSortKey("gpu")}
            >
              GPU %
            </button>
            <button
              type="button"
              className={sortKey === "vram" ? "button-small button-on" : "button-small"}
              onClick={() => setSortKey("vram")}
            >
              VRAM
            </button>
          </div>
        </div>
        <div className="gpu-process-row gpu-process-head">
          <span>PID</span>
          <span>애플리케이션</span>
          <span>GPU %</span>
          <span>VRAM</span>
          <span>엔진</span>
          <span>종류</span>
          <span>모델/런타임</span>
        </div>
        {rows.map((process) => {
          const models = modelsLabel(process);
          const runtime = runtimeLabel(process);
          const vramSource = vramSourceLabel(process.vramSource);
          const sources = sourcesLabel(process.sources);
          const tooltip = [
            process.executable ? `실행: ${process.executable}` : null,
            process.productName ? `제품: ${process.productName}` : null,
            `신뢰도: ${confidenceLabel(process.confidence)}`,
            sources ? `근거: ${sources}` : null,
            vramSource ? `VRAM 출처: ${vramSource}` : null,
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <div className="gpu-process-row" key={process.pid} title={tooltip}>
              <span className="mono">{process.pid}</span>
              <span>
                {attributionLabel(process)}
                {process.managed ? (
                  <Badge tone="info">{process.isServiceRoot ? "관리" : "하위"}</Badge>
                ) : null}
              </span>
              <span className="mono">
                {process.gpuPercent !== null ? `${Math.round(process.gpuPercent)}%` : "N/A"}
              </span>
              <span className="mono">
                {formatBytes(process.usedVramBytes)}
                {vramSource === "PDH" ? " ·PDH" : ""}
              </span>
              <span className="mono">{engineLabel(process.dominantEngine)}</span>
              <span>
                <Badge tone={classificationTone(process.classification)}>
                  {classificationLabel(process.classification)}
                </Badge>
              </span>
              <span className="mono">
                {models ?? runtime}
                {models ? <span className="muted"> · {runtime}</span> : null}
              </span>
            </div>
          );
        })}
        {snapshot.otherCount > 0 ? (
          <div className="gpu-process-row gpu-process-idle">
            <span className="mono">—</span>
            <span>유휴/저신호</span>
            <span className="mono">N/A</span>
            <span className="mono">N/A</span>
            <span className="mono">—</span>
            <span>
              <Badge tone="muted">대기</Badge>
            </span>
            <span className="muted">{snapshot.otherCount}개 (지표 없음)</span>
          </div>
        ) : null}
        {ordered.length > COLLAPSED_ROWS ? (
          <button
            type="button"
            className="button button-small"
            onClick={() => setShowAll((current) => !current)}
          >
            {showAll ? "접기" : `전체 보기 (+${ordered.length - COLLAPSED_ROWS})`}
          </button>
        ) : null}
      </div>
      <p className="panel-hint">
        프로세스 VRAM은 Windows(WDDM)에서 제공되지 않을 수 있어 N/A로 표시합니다(0으로 속이지 않음).
        GPU %는 프로세스의 가장 바쁜 엔진 기준이며 합산하지 않습니다.
      </p>
    </section>
  );
}
