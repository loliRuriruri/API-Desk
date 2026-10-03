import { useState, useEffect, useCallback } from "react";
import {
  type NotepadSnapshot,
  type DiffResultDto,
  listSnapshots,
  exportText,
  diffSnapshots,
  formatBytes,
  formatRelativeTime,
  getParseStatusBadge,
  restoreNativeTab,
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
  const [restoringVersion, setRestoringVersion] = useState(false);
  const [restoreMsg, setRestoreMsg] = useState<{ text: string; isError: boolean } | null>(null);

  // Diff state
  const [viewMode, setViewMode] = useState<"preview" | "diff">("preview");
  const [baseSnapshotId, setBaseSnapshotId] = useState<string | null>(null);
  const [diffResult, setDiffResult] = useState<DiffResultDto | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setLoading(true);
    listSnapshots(noteId)
      .then((data) => {
        if (active) {
          setSnapshots(data);
          if (data.length > 0) {
            setSelectedSnapshot(data[0]);
            // Default base snapshot is previous version (index 1) or self
            if (data.length > 1) {
              setBaseSnapshotId(data[1].id);
            } else {
              setBaseSnapshotId(data[0].id);
            }
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

  // Load diff when in diff mode or when base/target changes
  const runDiff = useCallback(
    async (baseId: string, targetId: string) => {
      try {
        setDiffLoading(true);
        setDiffError(null);
        const result = await diffSnapshots(noteId, baseId, targetId);
        setDiffResult(result);
      } catch (err) {
        setDiffError(String(err));
        setDiffResult(null);
      } finally {
        setDiffLoading(false);
      }
    },
    [noteId],
  );

  const handleSelectSnapshot = (snap: NotepadSnapshot, idx: number) => {
    setSelectedSnapshot(snap);
    // When selecting a snapshot, set base to the next older one if possible
    if (idx + 1 < snapshots.length) {
      setBaseSnapshotId(snapshots[idx + 1].id);
      if (viewMode === "diff") {
        void runDiff(snapshots[idx + 1].id, snap.id);
      }
    } else if (snapshots.length > 1 && idx > 0) {
      setBaseSnapshotId(snapshots[idx - 1].id);
      if (viewMode === "diff") {
        void runDiff(snapshots[idx - 1].id, snap.id);
      }
    } else {
      setBaseSnapshotId(snap.id);
      if (viewMode === "diff") {
        void runDiff(snap.id, snap.id);
      }
    }
  };

  const handleSwitchToDiff = () => {
    setViewMode("diff");
    if (selectedSnapshot && baseSnapshotId) {
      void runDiff(baseSnapshotId, selectedSnapshot.id);
    }
  };

  const handleBaseChange = (newBaseId: string) => {
    setBaseSnapshotId(newBaseId);
    if (selectedSnapshot) {
      void runDiff(newBaseId, selectedSnapshot.id);
    }
  };

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

  const handleRestoreVersionToNotepad = async () => {
    if (!selectedSnapshot) return;
    if (selectedSnapshot.parseStatus !== "PARSED") {
      setRestoreMsg({
        text: "정상 파싱된(PARSED) 스냅샷만 메모장 탭으로 복원할 수 있습니다.",
        isError: true,
      });
      return;
    }
    try {
      setRestoringVersion(true);
      setRestoreMsg(null);
      await restoreNativeTab(noteId, selectedSnapshot.id);
      setRestoreMsg({
        text: "이 버전이 메모장 탭으로 성공적으로 복원되었습니다. (메모장이 실행되었습니다)",
        isError: false,
      });
    } catch (e: any) {
      const errStr = typeof e === "string" ? e : e?.message || String(e);
      setRestoreMsg({ text: errStr, isError: true });
    } finally {
      setRestoringVersion(false);
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
                  onClick={() => handleSelectSnapshot(snap, idx)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      handleSelectSnapshot(snap, idx);
                    }
                  }}
                >
                  <div className="version-item-top">
                    <span className="version-tag">
                      {idx === 0
                        ? "최신 (v" + (snapshots.length - idx) + ")"
                        : "v" + (snapshots.length - idx)}
                    </span>
                    <span className="version-time">
                      {formatRelativeTime(snap.capturedAt)}
                    </span>
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

        {/* Right: Snapshot Detail or Diff */}
        <div className="version-history-preview">
          {selectedSnapshot ? (
            <div className="history-preview-content">
              {/* Tab Selector: Preview vs Diff */}
              <div
                style={{
                  display: "flex",
                  gap: "8px",
                  borderBottom: "1px solid var(--border-color, #333)",
                  paddingBottom: "8px",
                  marginBottom: "12px",
                }}
              >
                <button
                  type="button"
                  className={`button button-subtle ${viewMode === "preview" ? "active" : ""}`}
                  onClick={() => setViewMode("preview")}
                  style={{
                    fontWeight: viewMode === "preview" ? "bold" : "normal",
                    borderBottom:
                      viewMode === "preview" ? "2px solid var(--accent, #3b82f6)" : "none",
                  }}
                >
                  상세 미리보기
                </button>
                <button
                  type="button"
                  className={`button button-subtle ${viewMode === "diff" ? "active" : ""}`}
                  onClick={handleSwitchToDiff}
                  disabled={snapshots.length < 1}
                  style={{
                    fontWeight: viewMode === "diff" ? "bold" : "normal",
                    borderBottom:
                      viewMode === "diff" ? "2px solid var(--accent, #3b82f6)" : "none",
                  }}
                >
                  버전 비교 (Diff)
                </button>
              </div>

              {viewMode === "preview" ? (
                <>
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
                        className="button button-accent"
                        onClick={handleRestoreVersionToNotepad}
                        disabled={
                          !selectedSnapshot.recoveredText ||
                          selectedSnapshot.parseStatus !== "PARSED" ||
                          restoringVersion
                        }
                        title="선택된 버전의 스냅샷을 실제 Windows 메모장 탭으로 복원합니다"
                      >
                        {restoringVersion ? "메모장 복원 중…" : "메모장 탭으로 복원"}
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

                  {restoreMsg && (
                    <div
                      style={{
                        background: restoreMsg.isError
                          ? "rgba(239, 68, 68, 0.12)"
                          : "rgba(34, 197, 94, 0.12)",
                        border: `1px solid ${
                          restoreMsg.isError
                            ? "rgba(239, 68, 68, 0.4)"
                            : "rgba(34, 197, 94, 0.4)"
                        }`,
                        borderRadius: 6,
                        padding: "8px 12px",
                        marginBottom: 10,
                        fontSize: "0.85rem",
                        color: restoreMsg.isError ? "#f87171" : "#4ade80",
                        display: "flex",
                        justifyContent: "space-between",
                        alignItems: "center",
                      }}
                    >
                      <span>
                        {restoreMsg.isError ? `⚠️ ${restoreMsg.text}` : `✓ ${restoreMsg.text}`}
                      </span>
                      <button
                        type="button"
                        className="button button-subtle"
                        style={{ fontSize: "0.75rem", padding: "2px 6px" }}
                        onClick={() => setRestoreMsg(null)}
                      >
                        닫기
                      </button>
                    </div>
                  )}

                  {exportMsg && <div className="preview-notice-banner">{exportMsg}</div>}

                  {selectedSnapshot.recoveredText ? (
                    <pre className="history-pre-box">{selectedSnapshot.recoveredText}</pre>
                  ) : (
                    <div className="history-unparsed-box">
                      <p>이 버전은 텍스트가 정상 추출되지 않았습니다.</p>
                      <p className="text-muted">
                        상태: {selectedSnapshot.parseStatus} · RAW 파일:{" "}
                        {selectedSnapshot.rawSnapshotPath}
                      </p>
                    </div>
                  )}
                </>
              ) : (
                /* Diff View */
                <div className="diff-view-panel">
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "space-between",
                      flexWrap: "wrap",
                      gap: "10px",
                      marginBottom: "12px",
                      background: "rgba(255, 255, 255, 0.03)",
                      padding: "8px 12px",
                      borderRadius: "6px",
                    }}
                  >
                    <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
                      <span className="text-muted">기준 버전:</span>
                      <select
                        className="select"
                        value={baseSnapshotId || ""}
                        onChange={(e) => handleBaseChange(e.target.value)}
                        style={{ padding: "4px 8px", fontSize: "0.85rem" }}
                      >
                        {snapshots.map((s, idx) => (
                          <option key={s.id} value={s.id}>
                            v{snapshots.length - idx} (
                            {formatRelativeTime(s.capturedAt)})
                            {s.id === selectedSnapshot.id ? " [선택됨]" : ""}
                          </option>
                        ))}
                      </select>
                      <span className="text-muted">→ 대상 버전:</span>
                      <strong>
                        v
                        {snapshots.length -
                          snapshots.findIndex((s) => s.id === selectedSnapshot.id)}
                      </strong>
                    </div>

                    {diffResult && !diffResult.isTooLarge && (
                      <div style={{ display: "flex", gap: "10px", fontSize: "0.85rem" }}>
                        <span style={{ color: "#4ade80", fontWeight: "bold" }}>
                          +{diffResult.totalAddedLines}줄 추가
                        </span>
                        <span style={{ color: "#f87171", fontWeight: "bold" }}>
                          -{diffResult.totalRemovedLines}줄 삭제
                        </span>
                        <span className="text-muted">
                          동일 {diffResult.totalUnchangedLines}줄
                        </span>
                      </div>
                    )}
                  </div>

                  {diffLoading ? (
                    <div style={{ padding: "20px", textAlign: "center", color: "#888" }}>
                      Diff 비교 연산 중…
                    </div>
                  ) : diffError ? (
                    <div style={{ padding: "16px", color: "#f87171" }}>
                      비교 실패: {diffError}
                    </div>
                  ) : diffResult?.isTooLarge ? (
                    <div
                      style={{
                        padding: "16px",
                        background: "rgba(245, 158, 11, 0.1)",
                        border: "1px solid rgba(245, 158, 11, 0.3)",
                        borderRadius: "6px",
                        color: "#fbbf24",
                      }}
                    >
                      <p>
                        <strong>대용량 문서 안내:</strong> {diffResult.message}
                      </p>
                      <p style={{ marginTop: "8px", fontSize: "0.9rem" }}>
                        기존 버전: {diffResult.totalRemovedLines}줄 · 대상 버전:{" "}
                        {diffResult.totalAddedLines}줄
                      </p>
                    </div>
                  ) : diffResult ? (
                    <div
                      className="unified-diff-container"
                      style={{
                        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
                        fontSize: "0.85rem",
                        background: "var(--diff-bg, rgba(0, 0, 0, 0.2))",
                        borderRadius: "6px",
                        padding: "8px 0",
                        overflowX: "auto",
                        maxHeight: "520px",
                        overflowY: "auto",
                        lineHeight: 1.5,
                      }}
                    >
                      {diffResult.blocks.length === 0 ? (
                        <div style={{ padding: "16px", textAlign: "center", color: "#888" }}>
                          내용 변경이 없습니다 (두 버전이 완전히 일치함).
                        </div>
                      ) : (
                        diffResult.blocks.map((block, bIdx) => (
                          <div key={bIdx} className={`diff-block diff-${block.kind.toLowerCase()}`}>
                            {block.lines.map((line, lIdx) => {
                              const isAdded = block.kind === "ADDED";
                              const isRemoved = block.kind === "REMOVED";
                              return (
                                <div
                                  key={lIdx}
                                  style={{
                                    display: "flex",
                                    padding: "1px 12px",
                                    background: isAdded
                                      ? "rgba(34, 197, 94, 0.12)"
                                      : isRemoved
                                        ? "rgba(239, 68, 68, 0.12)"
                                        : "transparent",
                                    color: isAdded
                                      ? "#4ade80"
                                      : isRemoved
                                        ? "#f87171"
                                        : "inherit",
                                    whiteSpace: "pre-wrap",
                                    wordBreak: "break-all",
                                  }}
                                >
                                  <span
                                    style={{
                                      userSelect: "none",
                                      width: "20px",
                                      flexShrink: 0,
                                      opacity: 0.7,
                                    }}
                                  >
                                    {isAdded ? "+" : isRemoved ? "-" : " "}
                                  </span>
                                  <span style={{ flex: 1 }}>{line}</span>
                                </div>
                              );
                            })}
                          </div>
                        ))
                      )}
                    </div>
                  ) : null}
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
