import { useCallback, useEffect, useState } from "react";
import { useToast } from "./ToastProvider";
import { TurzxPreview } from "./TurzxPreview";
import { errorMessage } from "../lib/errors";
import {
  candidateLabel,
  connectTurzx,
  detectTurzxPorts,
  disconnectTurzx,
  fetchTurzxStatus,
  loadTurzxSettings,
  ORIENTATIONS,
  PAGE_MODES,
  saveTurzxSettings,
  statusSummary,
  testTurzx,
  type TurzxPortCandidate,
  type TurzxSettings,
  type TurzxStatus,
} from "../lib/turzxDisplay";

export function TurzxSettingsSection() {
  const { notify } = useToast();
  const [settings, setSettings] = useState<TurzxSettings | null>(null);
  const [status, setStatus] = useState<TurzxStatus | null>(null);
  const [candidates, setCandidates] = useState<TurzxPortCandidate[]>([]);
  const [busy, setBusy] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await fetchTurzxStatus());
    } catch {
      /* 상태 조회 실패는 무시(다음 폴링에서 재시도) */
    }
  }, []);

  useEffect(() => {
    let active = true;
    const init = async () => {
      try {
        const next = await loadTurzxSettings();
        if (active) setSettings(next);
      } catch (cause) {
        if (active) notify(errorMessage(cause), "error");
      }
    };
    const tick = async () => {
      try {
        const next = await fetchTurzxStatus();
        if (active) setStatus(next);
      } catch {
        /* 상태 조회 실패는 무시(다음 폴링에서 재시도) */
      }
    };
    void init();
    void tick();
    const timer = window.setInterval(() => void tick(), 2000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [notify]);

  const update = async <K extends keyof TurzxSettings>(field: K, value: TurzxSettings[K]) => {
    if (!settings) return;
    const next = { ...settings, [field]: value };
    setSettings(next);
    try {
      setSettings(await saveTurzxSettings(next));
    } catch (cause) {
      notify(errorMessage(cause), "error");
    }
  };

  const run = async (label: string, action: () => Promise<void>) => {
    setBusy(label);
    try {
      await action();
    } catch (cause) {
      notify(errorMessage(cause), "error");
    } finally {
      setBusy(null);
      void refreshStatus();
    }
  };

  if (!settings) {
    return (
      <section className="panel">
        <header className="panel-header">
          <h3>TURZX 3.5&quot; 디스플레이</h3>
        </header>
        <p className="empty-note">설정 불러오는 중…</p>
      </section>
    );
  }

  const ports = candidates.length > 0 ? candidates : (status?.candidates ?? []);

  return (
    <section className="panel">
      <header className="panel-header">
        <h3>TURZX 3.5&quot; 디스플레이</h3>
        <span className="panel-hint">{statusSummary(status)}</span>
      </header>

      <label className="field field-check">
        <input
          type="checkbox"
          checked={settings.enabled}
          onChange={(event) => void update("enabled", event.target.checked)}
        />
        <span>디스플레이 사용 (AI 텔레메트리 대시보드 전송)</span>
      </label>
      <label className="field field-check">
        <input
          type="checkbox"
          checked={settings.launchWithApp}
          onChange={(event) => void update("launchWithApp", event.target.checked)}
        />
        <span>앱 시작 시 자동 연결</span>
      </label>
      <label className="field field-check">
        <input
          type="checkbox"
          checked={settings.autoReconnect}
          onChange={(event) => void update("autoReconnect", event.target.checked)}
        />
        <span>연결이 끊기면 자동 재연결 (1s → 2s → 5s → 10s 백오프)</span>
      </label>

      <div className="field-grid">
        <label className="field">
          <span>COM 포트</span>
          <select
            value={settings.port}
            onChange={(event) => void update("port", event.target.value)}
          >
            <option value="auto">자동 (TURZX 우선)</option>
            {ports.map((candidate) => (
              <option key={candidate.port} value={candidate.port}>
                {candidateLabel(candidate)}
              </option>
            ))}
            {settings.port !== "auto" && !ports.some((item) => item.port === settings.port) ? (
              <option value={settings.port}>{settings.port} (수동)</option>
            ) : null}
          </select>
        </label>
        <label className="field">
          <span>화면 방향</span>
          <select
            value={settings.orientation}
            onChange={(event) => void update("orientation", event.target.value)}
          >
            {ORIENTATIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>화면 회전</span>
          <select
            value={settings.rotation}
            onChange={(event) => void update("rotation", Number(event.target.value))}
          >
            {[0, 90, 180, 270].map((value) => (
              <option key={value} value={value}>
                {value === 0 ? "0° (기본)" : `${value}°`}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>밝기 ({settings.brightness}%)</span>
          <input
            type="range"
            min={10}
            max={100}
            value={settings.brightness}
            onChange={(event) => void update("brightness", Number(event.target.value))}
          />
        </label>
        <label className="field">
          <span>갱신 주기 (초)</span>
          <input
            className="input input-narrow"
            type="number"
            min={1}
            max={30}
            value={settings.refreshSecs}
            onChange={(event) => void update("refreshSecs", Number(event.target.value) || 2)}
          />
        </label>
        <label className="field">
          <span>표시 페이지</span>
          <select
            value={settings.pageMode}
            onChange={(event) => void update("pageMode", event.target.value)}
          >
            {PAGE_MODES.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>로테이션 주기 (초)</span>
          <input
            className="input input-narrow"
            type="number"
            min={5}
            max={60}
            value={settings.pageRotationSecs}
            onChange={(event) =>
              void update("pageRotationSecs", Number(event.target.value) || 8)
            }
          />
        </label>
        <label className="field">
          <span>기기 이름</span>
          <input
            type="text"
            maxLength={24}
            value={settings.deviceLabel}
            onChange={(event) => void update("deviceLabel", event.target.value)}
          />
        </label>
      </div>

      <div className="row-actions">
        <button
          type="button"
          className="button"
          disabled={busy !== null}
          onClick={() =>
            void run("detect", async () => {
              const found = await detectTurzxPorts();
              setCandidates(found);
              notify(
                found.length > 0
                  ? `시리얼 포트 ${found.length}개 감지: ${found.map((item) => item.port).join(", ")}`
                  : "시리얼 포트를 찾지 못했습니다",
                found.length > 0 ? "info" : "error",
              );
            })
          }
        >
          {busy === "detect" ? "감지 중…" : "포트 감지"}
        </button>
        <button
          type="button"
          className="button"
          disabled={busy !== null}
          onClick={() =>
            void run("test", async () => {
              const result = await testTurzx();
              notify(
                result.connected
                  ? `연결 성공: ${result.port ?? "-"} · ${result.model ?? "패널 확인됨"}`
                  : `연결 실패: ${result.lastError ?? "알 수 없는 오류"}`,
                result.connected ? "info" : "error",
              );
            })
          }
        >
          {busy === "test" ? "테스트 중…" : "연결 테스트"}
        </button>
        <button
          type="button"
          className="button"
          disabled={busy !== null || !settings.enabled}
          onClick={() => void run("connect", async () => setSettings(await connectTurzx()))}
        >
          {busy === "connect" ? "연결 중…" : "지금 연결"}
        </button>
        <button
          type="button"
          className="button-ghost"
          disabled={busy !== null}
          onClick={() => void run("disconnect", async () => setSettings(await disconnectTurzx()))}
        >
          {busy === "disconnect" ? "해제 중…" : "연결 해제"}
        </button>
      </div>

      <TurzxPreview enabled={Boolean(status?.connected)} />

      <p className="form-hint">
        대시보드는 GPU·로컬 AI 서비스·계정 쿼터·API 사용량을 표시합니다. 전송 중에는 표시용 데이터만
        읽으며(시크릿·명령줄 미포함) 서비스를 시작/중지하지 않습니다. USB 시리얼 포트(115200bps)를
        사용하며, 연결 해제 상태에서도 앱은 정상 동작합니다.
      </p>
    </section>
  );
}
