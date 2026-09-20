import { useState, type FormEvent } from "react";
import { errorMessage } from "../lib/errors";
import { vaultInit, vaultUnlock, type VaultStatus } from "../lib/vault";
import mikuBanner from "../assets/miku-banner.png";
import mikuIcon from "../assets/miku-icon.png";

interface VaultGateProps {
  status: VaultStatus;
  onUnlocked: () => void;
}

export function VaultGate({ status, onUnlocked }: VaultGateProps) {
  const initializing = status.state === "uninitialized";
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    if (initializing) {
      if (password.length < 4) {
        setError("마스터 비밀번호는 4자 이상이어야 합니다.");
        return;
      }
      if (password !== confirm) {
        setError("비밀번호가 일치하지 않습니다.");
        return;
      }
    }
    if (password.length === 0) {
      setError("마스터 비밀번호를 입력하세요.");
      return;
    }
    setBusy(true);
    try {
      if (initializing) {
        await vaultInit(password);
      } else {
        await vaultUnlock(password);
      }
      setPassword("");
      setConfirm("");
      onUnlocked();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="gate gate-split">
      <form className="gate-card" onSubmit={(event) => void submit(event)}>
        <div className="gate-brand">
          <img className="brand-mark-img brand-mark-img-large" src={mikuIcon} alt="" />
          <div>
            <h1>API Desk</h1>
            <p>로컬 전용 API 키 · 계정 관리자</p>
          </div>
        </div>

        <h2>{initializing ? "Vault 만들기" : "Vault 잠금 해제"}</h2>
        <p className="gate-help">
          {initializing
            ? "API 키는 이 마스터 비밀번호로 IOTA Stronghold에 암호화되어 저장됩니다. 비밀번호는 어디에도 저장되지 않으므로, 분실하면 키를 복구할 수 없습니다. 4자 이상이면 되지만 짧을수록 추측되기 쉽습니다."
            : "로컬 Stronghold Vault를 복호화하려면 마스터 비밀번호를 입력하세요."}
        </p>

        <label className="field">
          <span>마스터 비밀번호</span>
          <input
            type="password"
            value={password}
            autoFocus
            onChange={(event) => setPassword(event.target.value)}
            placeholder={initializing ? "4자 이상" : ""}
          />
        </label>

        {initializing ? (
          <label className="field">
            <span>비밀번호 확인</span>
            <input
              type="password"
              value={confirm}
              onChange={(event) => setConfirm(event.target.value)}
            />
          </label>
        ) : null}

        {error ? <div className="gate-error">{error}</div> : null}

        <button type="submit" className="button button-primary gate-submit" disabled={busy}>
          {busy ? "처리 중…" : initializing ? "Vault 생성" : "잠금 해제"}
        </button>

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
