import { useState, useEffect } from "react";
import {
  type NotepadSnapshot,
  listSnapshots,
  exportText,
  formatBytes,
  formatRelativeTime,
  getParseStatusBadge,
} from "../../lib/notepadRescue";
import { Badge } from "../Badges";

interface VersionHistoryProps {
  noteId: string;
  noteTitle: string;
  onClose: () => void;
  onSelectSnapshot?: (snapshot: NotepadSnapshot) => void;
}

export function VersionHistory({
  noteId,
  noteTitle,
  onClose,
}: VersionHistoryProps) {
  const [snapshots, setSnapshots] = useState<NotepadSnapshot[]>([]);
  const [selectedSnapshot, setSelectedSnapshot] = useState<NotepadSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [copied, setCopied] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    listSnapshots(noteId)
      .then((data) => {
        if (active) {
          setSnapshots(data);
          if (data.length > 0) {
            setSelectedSnapshot(data[0]);
          }
          setLoading(false);
        }
      })
      .catch((e) => {
        if (active) {
          console.error("Failed to load snapshots", e);
          setLoading(false);
        }
      });

    return () => {
      active = false;
    };
  }, [noteId]);

  const handleCopy = async () => {
    if (!selectedSnapshot?.recoveredText) return;
    try {
      await navigator.clipboard.writeText(selectedSnapshot.recoveredText);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setExportMsg("클립보드 복사 실패");
    }
  };

  const handleExport = async () => {
    if (!selectedSnapshot) return;
    try {
      setExporting(true);
      setExportMsg(null);
      const outPath = await exportText(selectedSnapshot.id);
      setExportMsg(`내보내기 완료: ${outPath}`);
    } catch (e) {
      setExportMsg(`내보내기 실패: ${String(e)}`);
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className="version-history-panel">
      {/* Panel Header */}
      <div className="version-history-header">
        <div>
          <h3>버전 히스토리</h3>
          <p className="text-muted" title={noteTitle}>
            {noteTitle} ({snapshots.length}개 스냅샷)
          </p>
        </div>
        <button
          type="button"
          className="icon-button"
          onClick={onClose}
          aria-label="닫기"
        >
          ✕
        </button>
      </div>

      <div className="version-history-body">
        {/* Left: Snapshots List */}
        <div className="version-history-list">
          {loading ? (
            <div className="history-loading">불러오는 중…</div>
          ) : snapshots.length === 0 ? (
            <div className="history-empty">저장된 스냅샷이 없습니다.</div>
          ) : (
            snapshots.map((snap, idx) => {
              const isSelected = selectedSnapshot?.id === snap.id;
              const statusBadge = getParseStatusBadge(snap.parseStatus);
              return (
                <div
                  key={snap.id}
                  className={`version-item ${isSelected ? "selected" : ""}`}
                  onClick={() => setSelectedSnapshot(snap)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      setSelectedSnapshot(snap);
                    }
                  }}
                >
                  <div className="version-item-top">
                    <span className="version-tag">
                      {idx === 0 ? "최신 (v" + (snapshots.length - idx) + ")" : "v" + (snapshots.length - idx)}
                    </span>
                    <span className="version-time">{formatRelativeTime(snap.capturedAt)}</span>
                  </div>
                  <div className="version-item-meta">
                    <span className="mono" title={snap.contentHash}>
                      {snap.contentHash.substring(0, 8)}…
                    </span>
                    <span>·</span>
                    <span>{formatBytes(snap.sourceSize)}</span>
                    <span>·</span>
                    <Badge tone={statusBadge.tone}>{statusBadge.label}</Badge>
                  </div>
                </div>
              );
            })
          )}
        </div>

        {/* Right: Snapshot Detail Preview */}
        <div className="version-history-preview">
          {selectedSnapshot ? (
            <div className="history-preview-content">
              <div className="history-preview-meta">
                <div>
                  <strong>캡처 일시:</strong> {selectedSnapshot.capturedAt}
                </div>
                <div className="history-preview-actions">
                  <button
                    type="button"
                    className="button button-subtle"
                    onClick={handleCopy}
                    disabled={!selectedSnapshot.recoveredText}
                  >
                    {copied ? "✓ 복사됨" : "텍스트 복사"}
                  </button>
                  <button
                    type="button"
                    className="button button-subtle"
                    onClick={handleExport}
                    disabled={!selectedSnapshot.recoveredText || exporting}
                  >
                    {exporting ? "내보내는 중…" : "TXT 내보내기"}
                  </button>
                </div>
              </div>

              {exportMsg && <div className="preview-notice-banner">{exportMsg}</div>}

              {selectedSnapshot.recoveredText ? (
                <pre className="history-pre-box">{selectedSnapshot.recoveredText}</pre>
              ) : (
                <div className="history-unparsed-box">
                  <p>이 버전은 텍스트가 정상 추출되지 않았습니다.</p>
                  <p className="text-muted">
                    상태: {selectedSnapshot.parseStatus} · RAW 파일: {selectedSnapshot.rawSnapshotPath}
                  </p>
                </div>
              )}
            </div>
          ) : (
            <div className="history-no-select">좌측에서 버전을 선택하세요.</div>
          )}
        </div>
      </div>
    </div>
  );
}
