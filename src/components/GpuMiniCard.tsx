import { useEffect, useState } from "react";
import { Badge } from "./Badges";
import { GaugeBar } from "./Gauge";
import { errorMessage } from "../lib/errors";
import {
  attributionLabel,
  engineLabel,
  formatBytes,
  gpuSnapshot,
  modelsLabel,
  runtimeLabel,
  topWorkloads,
  vramPercent,
  vramTone,
  type GpuSnapshot,
} from "../lib/gpuMonitor";

const POLL_MS = 2_000;
const MINI_ROWS = 3;

interface GpuMiniCardProps {
  onOpenDetail: () => void;
}

export function GpuMiniCard({ onOpenDetail }: GpuMiniCardProps) {
  const [snapshot, setSnapshot] = useState<GpuSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const tick = async () => {
      if (document.visibilityState !== "visible") return;
      try {
        const data = await gpuSnapshot("mini", true);
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
      <div className="mini-gpu">
        <span className="mini-note">GPU: {error}</span>
      </div>
    );
  }

  if (!snapshot) {
    return (
      <div className="mini-gpu">
        <span className="mini-note">GPU 정보 불러오는 중…</span>
      </div>
    );
  }

  if (!snapshot.available) {
    return (
      <div className="mini-gpu">
        <span className="mini-note">
          {snapshot.reason === "no_gpu" ? "지원 GPU 없음" : "NVIDIA GPU 정보를 사용할 수 없음"}
        </span>
      </div>
    );
  }

  const info = snapshot.gpus[0];
  const percent = vramPercent(info);
  const { rows, otherCount } = topWorkloads(snapshot.processes, MINI_ROWS);

  return (
    <div className="mini-gpu">
      <div className="mini-gpu-head">
        <span className="mono">
          GPU · {info?.name.replace("NVIDIA GeForce ", "") ?? "—"}
        </span>
        <Badge tone={vramTone(percent)}>
          {percent !== null ? `${Math.round(percent)}%` : "N/A"}
        </Badge>
      </div>
      <div className="gpu-meters">
        <span className="gauge-label">VRAM</span>
        <GaugeBar percent={percent ?? 0} mode="used" />
        <span className="gauge-value mono">
          {formatBytes(info?.memoryUsedBytes)} / {formatBytes(info?.memoryTotalBytes)}
        </span>
      </div>
      <div className="mini-gpu-meta mono">
        GPU {info?.utilizationPercent ?? "N/A"}% · {info?.temperatureC ?? "N/A"}°C
        {info?.powerWatts !== null && info?.powerWatts !== undefined
          ? ` · ${info.powerWatts.toFixed(0)}W`
          : ""}
      </div>

      {rows.map((process) => {
        const models = modelsLabel(process);
        const runtime = runtimeLabel(process);
        const engine = engineLabel(process.dominantEngine);
        return (
          <div className="mini-gpu-row" key={process.pid}>
            <span className="mini-gpu-row-main">
              <span>{attributionLabel(process)}</span>
              <span className="mini-gpu-sub mono">
                {process.gpuPercent !== null ? `${Math.round(process.gpuPercent)}%` : "N/A"}
                {engine !== "N/A" ? ` · ${engine}` : ""}
                {models ? ` · ${models}` : ` · ${runtime}`}
              </span>
            </span>
            <span className="mono">{formatBytes(process.usedVramBytes)}</span>
          </div>
        );
      })}
      {otherCount > 0 ? (
        <div className="mini-gpu-row">
          <span className="mini-gpu-row-main">그 외</span>
          <span className="mono">{otherCount}개</span>
        </div>
      ) : null}

      <div className="row-actions">
        <button type="button" className="button button-small button-ghost" onClick={onOpenDetail}>
          상세
        </button>
      </div>
    </div>
  );
}
