import { useState, useEffect } from "react";
import {
  type NoteSummaryDto,
  type NotepadSnapshot,
  getSnapshot,
  exportText,
  openExportedNote,
  formatBytes,
  formatRelativeTime,
  getParseStatusBadge,
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
}: NotePreviewProps) {
  const [snapshot, setSnapshot] = useState<NotepadSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [feedbackMsg, setFeedbackMsg] = useState<string | null>(null);

  // Fetch full snapshot when note changes or lastSnapshotId updates
  useEffect(() => {
    if (!note || !note.lastSnapshotId) {
      setSnapshot(null);
      setExportedPath(null);
      return;
    }

    let active = true;
    setLoading(true);
    setExportedPath(null);
    setFeedbackMsg(null);

    getSnapshot(note.lastSnapshotId)
      .then((data) => {
        if (active) {
          setSnapshot(data);
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
          <button
            type="button"
            className="button"
            onClick={handleCopy}
            disabled={!textContent || loading}
            title="복구된 텍스트를 클립보드에 복사합니다"
          >
            {copied ? "✓ 복사됨" : "텍스트 복사"}
          </button>

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

      {feedbackMsg && (
        <div className="preview-notice-banner">
          <span>{feedbackMsg}</span>
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
