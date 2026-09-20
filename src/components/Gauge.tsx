import type { GaugeItem } from "../lib/monitorGauges";

function gaugeColor(percent: number, mode: GaugeItem["mode"]): string {
  const remaining = mode === "remaining" ? percent : 100 - percent;
  if (remaining >= 60) return "var(--ok)";
  if (remaining >= 30) return "var(--warn)";
  return "var(--danger)";
}

export function GaugeBar({ percent, mode }: { percent: number; mode: GaugeItem["mode"] }) {
  const clamped = Math.max(0, Math.min(100, percent));
  return (
    <div className="gauge" aria-hidden="true">
      <div
        className="gauge-fill"
        style={{ width: `${clamped}%`, background: gaugeColor(clamped, mode) }}
      />
    </div>
  );
}

export function GaugeList({ items }: { items: GaugeItem[] }) {
  const visible = items.filter((item) => item.percent !== null);
  if (visible.length === 0) return null;
  return (
    <div className="gauge-list">
      {visible.map((item) => (
        <div key={item.label} className="gauge-row">
          <span className="gauge-label">{item.label}</span>
          <GaugeBar percent={item.percent ?? 0} mode={item.mode} />
          <span className="gauge-value mono">
            {Math.round(item.percent ?? 0)}%
            {item.mode === "used" ? " 사용" : ""}
          </span>
          {item.hint ? <span className="gauge-hint">{item.hint}</span> : null}
        </div>
      ))}
    </div>
  );
}
