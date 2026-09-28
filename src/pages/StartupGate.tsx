import { useState } from "react";
import { errorMessage } from "../lib/errors";
import {
  vaultEnrollAutoUnlock,
  vaultRetryAutoUnlock,
  vaultUnlock,
  type VaultStatus,
} from "../lib/vault";
import mikuIcon from "../assets/miku-icon.png";
import mikuBanner from "../assets/miku-banner.png";

interface StartupGateProps {
  status: VaultStatus;
  onEnter: () => void;
  onUnlocked: () => void;
}

/**
 * 시작 게이트(비밀번호 없는 시작).
 * - 정상: 스플래시 + [시작] (프런트 상태 전환만, 백그라운드 초기화와 무관)
 * - 레거시: 1회 자동 잠금해제 등록(마지막 비밀번호 입력)
 * - 실패/불일치: 복구 화면(마스터 비밀번호로 복구 / 다시 시도)
 * 어떤 상태에서도 비밀번호를 저장/직렬화하지 않는다.
 */
export function StartupGate({ status, onEnter, onUnlocked }: StartupGateProps) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (action: () => Promise<void>, enterAfter = false) => {
    setError(null);
    setBusy(true);
    try {
      await action();
      setPassword("");
      onUnlocked();
      if (enterAfter) onEnter();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  if (status.state === "unlocked") {
    return (
      <div className="gate gate-split">
        <div className="gate-card gate-splash">
          <div className="gate-brand">
            <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
            <div>
              <h1>API Desk</h1>
              <p>AI Runtime Dashboard</p>
            </div>
          </div>
          <button type="button" className="button button-primary gate-submit" onClick={onEnter}>
            시작
          </button>
          <p className="gate-help">
            창을 닫아도 트레이에서 사용량 추적과 디스플레이가 계속 동작합니다.
          </p>
        </div>
        <div
          className="gate-art"
          style={{ backgroundImage: `url(${mikuBanner})` }}
          aria-hidden="true"
        />
      </div>
    );
  }

  if (status.autoUnlock === "initializing") {
    return (
      <div className="gate gate-split">
        <div className="gate-card gate-splash">
          <div className="gate-brand">
            <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
            <div>
              <h1>API Desk</h1>
              <p>AI Runtime Dashboard</p>
            </div>
          </div>
          <p className="gate-help">Vault 초기화 중…</p>
        </div>
        <div
          className="gate-art"
          style={{ backgroundImage: `url(${mikuBanner})` }}
          aria-hidden="true"
        />
      </div>
    );
  }

  if (status.autoUnlock === "legacy") {
    return (
      <div className="gate gate-split">
        <form
          className="gate-card"
          onSubmit={(event) => {
            event.preventDefault();
            if (password.length === 0) {
              setError("현재 마스터 비밀번호를 입력하세요.");
              return;
            }
            void run(() => vaultEnrollAutoUnlock(password), true);
          }}
        >
          <div className="gate-brand">
            <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
            <div>
              <h1>API Desk</h1>
              <p>AI Runtime Dashboard</p>
            </div>
          </div>
          <h2>1회 자동 잠금해제 등록</h2>
          <p className="gate-help">
            앞으로 이 PC에서는 비밀번호 없이 시작합니다. 지금 한 번만 현재 마스터 비밀번호를
            입력하면, Windows 사용자 보호(DPAPI)로 안전하게 보관되어 다음부터는 자동으로
            잠금이 해제됩니다. Vault 암호화는 그대로 유지되며 키는 이동하지 않습니다.
          </p>
          <label className="field">
            <span>현재 마스터 비밀번호</span>
            <input
              type="password"
              value={password}
              autoFocus
              onChange={(event) => setPassword(event.target.value)}
            />
          </label>
          {error ? <div className="gate-error">{error}</div> : null}
          <button type="submit" className="button button-primary gate-submit" disabled={busy}>
            {busy ? "등록 중…" : "이 PC에 자동 잠금해제 등록"}
          </button>
        </form>
        <div
          className="gate-art"
          style={{ backgroundImage: `url(${mikuBanner})` }}
          aria-hidden="true"
        />
      </div>
    );
  }

  if (status.autoUnlock === "ready") {
    // 세션 잠금(설정에서 잠근 경우): 자동 잠금해제로 즉시 다시 열 수 있다.
    return (
      <div className="gate gate-split">
        <div className="gate-card gate-splash">
          <div className="gate-brand">
            <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
            <div>
              <h1>API Desk</h1>
              <p>Vault 세션 잠김</p>
            </div>
          </div>
          <p className="gate-help">
            Windows 사용자 보호(DPAPI)로 저장된 자동 잠금해제로 즉시 다시 열 수 있습니다.
          </p>
          {error ? <div className="gate-error">{error}</div> : null}
          <div className="row-actions">
            <button
              type="button"
              className="button button-primary gate-submit"
              disabled={busy}
              onClick={() => void run(() => vaultRetryAutoUnlock(), true)}
            >
              {busy ? "여는 중…" : "자동 잠금해제로 열기"}
            </button>
          </div>
        </div>
        <div
          className="gate-art"
          style={{ backgroundImage: `url(${mikuBanner})` }}
          aria-hidden="true"
        />
      </div>
    );
  }

  // failed | inconsistent
  return (
    <div className="gate gate-split">
      <form
        className="gate-card"
        onSubmit={(event) => {
          event.preventDefault();
          if (password.length === 0) {
            setError("마스터 비밀번호를 입력하세요.");
            return;
          }
          void run(() => vaultUnlock(password), true);
        }}
      >
        <div className="gate-brand">
          <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
          <div>
            <h1>API Desk</h1>
            <p>AI Runtime Dashboard</p>
          </div>
        </div>
        <h2>자동 잠금해제를 사용할 수 없습니다</h2>
        <p className="gate-help">
          {status.autoUnlockError ??
            "저장된 자동 잠금해제 정보를 사용할 수 없습니다. 기존 마스터 비밀번호로 복구하거나 다시 시도하세요. Vault 데이터는 그대로입니다."}
        </p>
        <label className="field">
          <span>기존 마스터 비밀번호</span>
          <input
            type="password"
            value={password}
            autoFocus
            onChange={(event) => setPassword(event.target.value)}
          />
        </label>
        {error ? <div className="gate-error">{error}</div> : null}
        <div className="row-actions">
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? "복구 중…" : "기존 마스터 비밀번호로 복구"}
          </button>
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => void run(() => vaultRetryAutoUnlock())}
          >
            다시 시도
          </button>
        </div>
        <dl className="gate-paths">
          <div>
            <dt>Vault 경로</dt>
            <dd>{status.vaultPath}</dd>
          </div>
        </dl>
      </form>
      <div
        className="gate-art"
        style={{ backgroundImage: `url(${mikuBanner})` }}
        aria-hidden="true"
      />
    </div>
  );
}
