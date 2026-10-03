import { useState, useEffect } from "react";
import {
  type NoteSummaryDto,
  type NotepadSnapshot,
  type NoteRecoveryDetailDto,
  getSnapshot,
  getRecoveryDetail,
  exportText,
  openExportedNote,
  formatBytes,
  formatRelativeTime,
  getParseStatusBadge,
  restoreNativeTab,
  rollbackLastRestore,
  canRollback,
} from "../../lib/notepadRescue";
import { Badge } from "../Badges";
import { EmptyState } from "../EmptyState";

interface NotePreviewProps {
  note: NoteSummaryDto | null;
  onOpenHistory: () => void;
  showHistory: boolean;
  onSnapshotUpdated?: () => void;
}

export function NotePreview({
  note,
  onOpenHistory,
  showHistory,
  onSnapshotUpdated,
}: NotePreviewProps) {
  const [snapshot, setSnapshot] = useState<NotepadSnapshot | null>(null);
  const [recoveryDetail, setRecoveryDetail] = useState<NoteRecoveryDetailDto | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [feedbackMsg, setFeedbackMsg] = useState<string | null>(null);

  // NR.7A Native Tab Restore & Rollback state
  const [restoringNative, setRestoringNative] = useState(false);
  const [rollingBack, setRollingBack] = useState(false);
  const [hasRollback, setHasRollback] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  const [restoreSuccess, setRestoreSuccess] = useState<string | null>(null);

  // Check if rollback is available
  useEffect(() => {
    canRollback().then(setHasRollback).catch(() => setHasRollback(false));
  }, [note?.id]);

  // Fetch full snapshot and recovery detail when note changes or lastSnapshotId updates
  useEffect(() => {
    if (!note || !note.lastSnapshotId) {
      setSnapshot(null);
      setRecoveryDetail(null);
      setExportedPath(null);
      return;
    }

    let active = true;
    setLoading(true);
    setExportedPath(null);
    setFeedbackMsg(null);

    Promise.all([
      getSnapshot(note.lastSnapshotId),
      getRecoveryDetail(note.id).catch(() => null),
    ])
      .then(([snapData, detailData]) => {
        if (active) {
          setSnapshot(snapData);
          setRecoveryDetail(detailData);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (active) {
          console.error("Failed to load snapshot text", err);
          setLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, [note?.id, note?.lastSnapshotId]);

  const handleCopy = async () => {
    if (!snapshot?.recoveredText) return;
    try {
      await navigator.clipboard.writeText(snapshot.recoveredText);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setFeedbackMsg("클립보드 복사 실패");
    }
  };

  const handleExport = async () => {
    if (!snapshot) return;
    try {
      setExporting(true);
      setFeedbackMsg(null);
      const outPath = await exportText(snapshot.id);
      setExportedPath(outPath);
      setFeedbackMsg(`내보내기 완료: ${outPath}`);
    } catch (e) {
      setFeedbackMsg(`내보내기 실패: ${String(e)}`);
    } finally {
      setExporting(false);
    }
  };

  const handleOpenExported = async () => {
    if (!exportedPath) return;
    try {
      await openExportedNote(exportedPath);
    } catch (e) {
      setFeedbackMsg(`열기 실패: ${String(e)}`);
    }
  };

  const handleRestoreNative = async () => {
    if (!note) return;
    if (snapshot?.parseStatus !== "PARSED") {
      setRestoreError("정상 파싱된(PARSED) 스냅샷만 메모장 탭으로 복원할 수 있습니다.");
      return;
    }
    try {
      setRestoringNative(true);
      setRestoreError(null);
      setRestoreSuccess(null);
      await restoreNativeTab(note.id, snapshot?.id);
      setRestoreSuccess("메모장 탭으로 성공적으로 복원되었습니다. (메모장이 실행되었습니다)");
      setHasRollback(true);
      onSnapshotUpdated?.();
    } catch (e: any) {
      const errStr = typeof e === "string" ? e : e?.message || String(e);
      setRestoreError(errStr);
    } finally {
      setRestoringNative(false);
    }
  };

  const handleRollback = async () => {
    try {
      setRollingBack(true);
      setRestoreError(null);
      setRestoreSuccess(null);
      const rep = await rollbackLastRestore();
      setRestoreSuccess(rep.message || "마지막 복원이 정상적으로 되돌려졌습니다.");
      setHasRollback(false);
      onSnapshotUpdated?.();
    } catch (e: any) {
      const errStr = typeof e === "string" ? e : e?.message || String(e);
      setRestoreError(errStr);
    } finally {
      setRollingBack(false);
    }
  };

  if (!note) {
    return (
      <div className="notepad-rescue-preview-empty">
        <EmptyState
          text="선택된 메모가 없습니다"
          hint="왼쪽 목록에서 메모를 클릭하여 복구된 내용을 확인하세요"
        />
      </div>
    );
  }

  const parseStatusInfo = getParseStatusBadge(snapshot?.parseStatus || note.lastParseStatus);
  const textContent = snapshot?.recoveredText ?? "";
  const linesCount = textContent ? textContent.split("\n").length : 0;
  const charsCount = textContent ? textContent.length : 0;

  const confidenceInfo = (() => {
    switch (recoveryDetail?.latestConfidence) {
      case "FULL":
        return { label: "✓ 완전 복구 가능", tone: "ok" as const };
      case "TEXT_ONLY":
        return { label: "⚠ 텍스트 복구 (원시 유실)", tone: "warn" as const };
      case "RAW_ONLY":
        return { label: "⚠ 원시 백업 보존", tone: "info" as const };
      case "UNAVAILABLE":
        return { label: "✕ 복구 불가", tone: "danger" as const };
      default:
        return null;
    }
  })();

  return (
    <div className="notepad-rescue-preview-container">
      {/* Top Header */}
      <div className="notepad-rescue-preview-header">
        <div className="preview-header-meta">
          <div className="preview-title-row">
            <h2 className="preview-title" title={note.displayTitle}>
              {note.displayTitle || "제목 없는 메모"}
            </h2>
            {note.isDeleted ? (
              <Badge tone="danger">삭제됨</Badge>
            ) : note.isSaved ? (
              <Badge tone="muted">저장됨</Badge>
            ) : (
              <Badge tone="warn">미저장 탭</Badge>
            )}
            <Badge tone={parseStatusInfo.tone}>{parseStatusInfo.label}</Badge>
            {confidenceInfo && (
              <Badge tone={confidenceInfo.tone}>{confidenceInfo.label}</Badge>
            )}
          </div>

          {note.originalPath && (
            <div className="preview-path-row">
              <span className="preview-path-label">원문 경로:</span>
              <span className="preview-path-val" title={note.originalPath}>
                {note.originalPath}
              </span>
            </div>
          )}

          <div className="preview-info-row">
            <span>마지막 캡처: {formatRelativeTime(note.lastSnapshotAt)}</span>
            <span>·</span>
            <span>버전: {note.versionCount}개</span>
            <span>·</span>
            <span>크기: {formatBytes(snapshot?.sourceSize ?? note.lastSourceSize)}</span>
            {snapshot?.contentHash && (
              <>
                <span>·</span>
                <span className="mono" title={`SHA-256: ${snapshot.contentHash}`}>
                  해시: {snapshot.contentHash.substring(0, 10)}…
                </span>
              </>
            )}
          </div>
        </div>

        {/* Action Buttons */}
        <div className="preview-header-actions">
          {recoveryDetail?.latestValidSnapshot && snapshot?.id !== recoveryDetail.latestValidSnapshot.id && (
            <button
              type="button"
              className="button button-accent"
              onClick={() => setSnapshot(recoveryDetail.latestValidSnapshot!)}
              title="가장 최근 정상적으로 파싱된 유효 버전을 불러옵니다"
            >
              최신 유효 복구
            </button>
          )}

          <button
            type="button"
            className="button"
            onClick={handleCopy}
            disabled={!textContent || loading}
            title="복구된 텍스트를 클립보드에 복사합니다"
          >
            {copied ? "✓ 복사됨" : "텍스트 복구"}
          </button>

          <button
            type="button"
            className="button button-accent"
            onClick={handleRestoreNative}
            disabled={loading || restoringNative || rollingBack || snapshot?.parseStatus !== "PARSED"}
            title="이 스냅샷을 실제 Windows 메모장 탭으로 복원하고 메모장을 엽니다"
          >
            {restoringNative ? "메모장 복원 중…" : "메모장 탭으로 복원"}
          </button>

          {hasRollback && (
            <button
              type="button"
              className="button button-subtle"
              onClick={handleRollback}
              disabled={rollingBack || restoringNative}
              title="방금 수행한 메모장 탭 복원을 이전 상태로 되돌립니다"
            >
              {rollingBack ? "되돌리는 중…" : "마지막 복원 되돌리기"}
            </button>
          )}

          <button
            type="button"
            className="button button-subtle"
            onClick={handleExport}
            disabled={!textContent || exporting || loading}
            title="텍스트를 외부 TXT 파일로 안전하게 내보냅니다"
          >
            {exporting ? "내보내는 중…" : "TXT 내보내기"}
          </button>

          {exportedPath && (
            <button
              type="button"
              className="button button-subtle"
              onClick={handleOpenExported}
              title="내보낸 TXT 파일을 메모장으로 실행합니다"
            >
              메모장으로 열기
            </button>
          )}

          <button
            type="button"
            className={`button ${showHistory ? "button-accent" : "button-subtle"}`}
            onClick={onOpenHistory}
            title="이 메모의 과거 스냅샷 버전 기록을 확인합니다"
          >
            버전 기록 ({note.versionCount})
          </button>
        </div>
      </div>

      {/* Prominent Restore Error Banner (e.g. Notepad running check) */}
      {restoreError && (
        <div
          style={{
            background: "rgba(239, 68, 68, 0.12)",
            border: "1px solid rgba(239, 68, 68, 0.4)",
            borderRadius: 6,
            padding: "10px 14px",
            marginBottom: 12,
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div>
            <div style={{ fontWeight: 600, color: "#f87171", fontSize: "0.88rem" }}>
              ⚠️ {restoreError}
            </div>
            {restoreError.includes("닫아주세요") && (
              <div style={{ fontSize: "0.8rem", color: "var(--text-muted)", marginTop: 2 }}>
                Windows 메모장 프로그램이 열려 있으면 안전한 데이터 복원을 위해 작업을 진행할 수 없습니다. 메모장을 먼저 닫은 후 다시 복원 버튼을 클릭해 주세요.
              </div>
            )}
          </div>
          <button
            type="button"
            className="button button-subtle"
            style={{ fontSize: "0.75rem", padding: "2px 8px" }}
            onClick={() => setRestoreError(null)}
          >
            닫기
          </button>
        </div>
      )}

      {/* Prominent Restore Success Banner */}
      {restoreSuccess && (
        <div
          style={{
            background: "rgba(34, 197, 94, 0.12)",
            border: "1px solid rgba(34, 197, 94, 0.4)",
            borderRadius: 6,
            padding: "10px 14px",
            marginBottom: 12,
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div style={{ fontWeight: 600, color: "#4ade80", fontSize: "0.88rem" }}>
            ✓ {restoreSuccess}
          </div>
          <button
            type="button"
            className="button button-subtle"
            style={{ fontSize: "0.75rem", padding: "2px 8px" }}
            onClick={() => setRestoreSuccess(null)}
          >
            닫기
          </button>
        </div>
      )}

      {feedbackMsg && (
        <div className="preview-notice-banner">
          <span>{feedbackMsg}</span>
        </div>
      )}

      {/* Deleted Note Recovery Status Banner */}
      {note.isDeleted && (
        <div
          style={{
            background: "rgba(239, 68, 68, 0.08)",
            border: "1px solid rgba(239, 68, 68, 0.25)",
            borderRadius: 6,
            padding: "10px 14px",
            marginBottom: 12,
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
          }}
        >
          <div>
            <div style={{ fontWeight: 600, color: "#f87171", fontSize: "0.85rem" }}>
              ⚠️ 메모장에서 닫힘 / 삭제됨 (Deleted from Notepad) — 백업본 복구 가능 (Backup available)
            </div>
            <div style={{ fontSize: "0.78rem", color: "var(--text-muted)", marginTop: 2 }}>
              Notepad Rescue에 보관된 백업을 안전하게 복구하거나 메모장 탭으로 다시 열 수 있습니다.
            </div>
          </div>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            {recoveryDetail?.latestValidSnapshot && snapshot?.id !== recoveryDetail.latestValidSnapshot.id && (
              <button
                type="button"
                className="button button-subtle"
                style={{ fontSize: "0.8rem", padding: "4px 10px" }}
                onClick={() => setSnapshot(recoveryDetail.latestValidSnapshot!)}
                title="가장 최근 정상적으로 파싱된 텍스트 스냅샷을 불러옵니다"
              >
                최신 유효 버전 복구
              </button>
            )}
            <button
              type="button"
              className="button button-accent"
              style={{ fontSize: "0.82rem", padding: "5px 12px" }}
              onClick={handleRestoreNative}
              disabled={loading || restoringNative || rollingBack || snapshot?.parseStatus !== "PARSED"}
              title="삭제된 메모를 실제 Windows 메모장 탭으로 즉시 복원합니다"
            >
              {restoringNative ? "복원 중…" : "메모장 탭으로 복원"}
            </button>
          </div>
        </div>
      )}

      {/* Parse Error Banner */}
      {snapshot && snapshot.parseStatus !== "PARSED" && (
        <div className="preview-warning-banner">
          <strong>안전 보존 알림:</strong>
          <span> {parseStatusInfo.description}</span>
          <div className="banner-subtext">
            RAW 바이너리 스냅샷 파일(<code>{snapshot.rawSnapshotPath}</code>)은 손실 없이 보존되어 있습니다.
          </div>
          {recoveryDetail?.latestValidSnapshot && (
            <div style={{ marginTop: 8 }}>
              <button
                type="button"
                className="button button-accent"
                style={{ fontSize: "0.8rem", padding: "4px 10px" }}
                onClick={() => setSnapshot(recoveryDetail.latestValidSnapshot!)}
              >
                마지막 정상 버전 복구 ({formatRelativeTime(recoveryDetail.latestValidSnapshot.capturedAt)})
              </button>
            </div>
          )}
        </div>
      )}

      {/* Text Editor / Preview Box */}
      <div className="notepad-rescue-preview-body">
        {loading ? (
          <div className="preview-loading">
            <span className="text-muted">스냅샷 복구 내용을 불러오는 중…</span>
          </div>
        ) : !snapshot ? (
          <div className="preview-loading">
            <span className="text-faint">스냅샷 내용이 비어 있습니다</span>
          </div>
        ) : snapshot.parseStatus !== "PARSED" ? (
          <div className="preview-unparsed">
            <div className="unparsed-icon">📄</div>
            <h3>텍스트 미리보기를 지원하지 않는 형식입니다</h3>
            <p className="text-muted">
              이 탭은 지원되지 않는 인코딩이거나 아직 저장 구조가 완성되지 않은 상태에서 캡처되었습니다.
              스냅샷 원본 파일은 디스크에 안전하게 격리 보관되어 있습니다.
            </p>
          </div>
        ) : (
          <div className="preview-text-wrapper">
            <div className="preview-status-stats">
              <span>{linesCount}줄</span>
              <span>·</span>
              <span>{charsCount.toLocaleString()}글자</span>
            </div>
            <pre className="preview-pre-content">{textContent}</pre>
          </div>
        )}
      </div>
    </div>
  );
}
