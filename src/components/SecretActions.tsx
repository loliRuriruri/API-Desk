import { useCallback, useEffect, useRef, useState } from "react";
import { secretCopy, secretMask, secretReveal } from "../lib/vault";
import { errorMessage } from "../lib/errors";
import { useToast } from "./ToastProvider";

interface SecretActionsProps {
  secretId: string;
  mask?: string | null;
  autoHideSecs: number;
  clipboardClearSecs: number;
  onMissing?: () => void;
}

export function SecretActions({
  secretId,
  mask,
  autoHideSecs,
  clipboardClearSecs,
  onMissing,
}: SecretActionsProps) {
  const [fetched, setFetched] = useState<{ secretId: string; mask: string | null } | null>(
    null,
  );
  const [revealed, setRevealed] = useState<string | null>(null);
  const timerRef = useRef<number | null>(null);
  const { notify } = useToast();

  const displayMask =
    mask !== undefined && mask !== null
      ? mask
      : fetched && fetched.secretId === secretId
        ? fetched.mask
        : null;
  const missing = fetched !== null && fetched.secretId === secretId && fetched.mask === null;

  useEffect(() => {
    if (mask !== undefined && mask !== null) return;
    let active = true;
    secretMask(secretId)
      .then((value) => {
        if (active) setFetched({ secretId, mask: value });
      })
      .catch(() => {
        if (!active) return;
        setFetched({ secretId, mask: null });
        onMissing?.();
      });
    return () => {
      active = false;
    };
    // onMissing is intentionally not a dependency: it would retrigger the fetch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [secretId, mask]);

  useEffect(
    () => () => {
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    },
    [],
  );

  const hide = useCallback(() => {
    setRevealed(null);
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
  }, []);

  const reveal = useCallback(async () => {
    if (missing) {
      notify("이 키는 Vault에 없습니다. '편집'에서 API 키를 다시 입력하세요.", "error");
      return;
    }
    try {
      const value = await secretReveal(secretId);
      setRevealed(value);
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(
        () => setRevealed(null),
        Math.max(5, autoHideSecs) * 1000,
      );
    } catch (error) {
      notify(errorMessage(error), "error");
    }
  }, [secretId, autoHideSecs, missing, notify]);

  const copy = useCallback(async () => {
    if (missing) {
      notify("이 키는 Vault에 없습니다. '편집'에서 API 키를 다시 입력하세요.", "error");
      return;
    }
    try {
      await secretCopy(secretId, clipboardClearSecs);
      notify(
        clipboardClearSecs > 0
          ? `클립보드에 복사됨 · ${clipboardClearSecs}초 후 자동 삭제`
          : "클립보드에 복사됨",
        "success",
      );
    } catch (error) {
      notify(errorMessage(error), "error");
    }
  }, [secretId, clipboardClearSecs, missing, notify]);

  return (
    <span className="secret-actions">
      <code
        className={`secret-value ${revealed !== null ? "secret-revealed" : ""} ${
          missing ? "secret-missing" : ""
        }`}
      >
        {revealed ?? displayMask ?? "••••••••"}
      </code>
      {revealed !== null ? (
        <span className="secret-auto-hide-note">{Math.max(5, autoHideSecs)}초 후 자동 숨김</span>
      ) : null}
      <button
        type="button"
        className="button button-small"
        onClick={revealed === null ? () => void reveal() : hide}
        title={missing ? "Vault에 키가 없습니다 — '편집'에서 다시 입력하세요" : undefined}
      >
        {revealed === null ? "보기" : "숨기기"}
      </button>
      <button
        type="button"
        className="button button-small"
        onClick={() => void copy()}
        title={missing ? "Vault에 키가 없습니다 — '편집'에서 다시 입력하세요" : undefined}
      >
        복사
      </button>
    </span>
  );
}
