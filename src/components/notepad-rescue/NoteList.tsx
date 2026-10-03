import { type NoteSummaryDto, formatRelativeTime } from "../../lib/notepadRescue";
import { Badge } from "../Badges";
import { EmptyState } from "../EmptyState";

interface NoteListProps {
  notes: NoteSummaryDto[];
  selectedNoteId: string | null;
  onSelectNote: (note: NoteSummaryDto) => void;
  category: "active" | "deleted" | "all";
  onCategoryChange: (cat: "active" | "deleted" | "all") => void;
  searchQuery: string;
  onSearchChange: (query: string) => void;
  loading: boolean;
}

/// 검색어 하이라이팅 컴포넌트 (raw HTML 주입 없이 React 텍스트 노드로 안전하게 렌더링).
function HighlightMatch({ text, query }: { text: string; query: string }) {
  if (!query.trim()) return <>{text}</>;
  const trimmed = query.trim();
  const lowerText = text.toLowerCase();
  const lowerQuery = trimmed.toLowerCase();
  const idx = lowerText.indexOf(lowerQuery);
  if (idx === -1) return <>{text}</>;

  const before = text.slice(0, idx);
  const match = text.slice(idx, idx + trimmed.length);
  const after = text.slice(idx + trimmed.length);

  return (
    <>
      {before}
      <span
        style={{
          background: "rgba(234, 179, 8, 0.25)",
          color: "#facc15",
          fontWeight: "bold",
          borderRadius: "2px",
          padding: "0 2px",
        }}
      >
        {match}
      </span>
      {after}
    </>
  );
}

export function NoteList({
  notes,
  selectedNoteId,
  onSelectNote,
  category,
  onCategoryChange,
  searchQuery,
  onSearchChange,
  loading,
}: NoteListProps) {
  // 백엔드에서 카테고리 및 검색 필터링/스니펫을 처리하므로 notes 목록을 직접 사용
  const displayNotes = notes;

  return (
    <div className="notepad-rescue-note-list-container">
      {/* Category Tabs & Search Bar */}
      <div className="notepad-rescue-list-header">
        <div className="tab-group">
          <button
            type="button"
            className={`tab-item ${category === "active" ? "active" : ""}`}
            onClick={() => onCategoryChange("active")}
          >
            활성 메모
          </button>
          <button
            type="button"
            className={`tab-item ${category === "deleted" ? "active" : ""}`}
            onClick={() => onCategoryChange("deleted")}
          >
            삭제된 메모
          </button>
          <button
            type="button"
            className={`tab-item ${category === "all" ? "active" : ""}`}
            onClick={() => onCategoryChange("all")}
          >
            전체 ({notes.length})
          </button>
        </div>

        <div className="search-bar">
          <input
            type="text"
            className="input search-input"
            placeholder="제목, 파일 경로, 본문 검색…"
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
          />
          {searchQuery && (
            <button
              type="button"
              className="icon-button search-clear"
              onClick={() => onSearchChange("")}
              title="검색어 지우기"
            >
              ✕
            </button>
          )}
        </div>
      </div>

      {/* Note Rows */}
      <div className="notepad-rescue-notes-scroll">
        {loading && notes.length === 0 ? (
          <div className="empty-state-container">
            <span className="text-muted">메모 목록을 불러오는 중…</span>
          </div>
        ) : displayNotes.length === 0 ? (
          <div className="empty-state-container">
            <EmptyState
              text={
                searchQuery
                  ? "검색 결과가 없습니다"
                  : category === "deleted"
                    ? "삭제된 메모가 없습니다"
                    : "감시 중인 메모가 없습니다"
              }
              hint={
                searchQuery
                  ? "다른 검색어로 찾아보세요"
                  : "Windows 11 메모장에서 새 탭을 열면 자동으로 여기에 백업됩니다"
              }
            />
          </div>
        ) : (
          <div className="note-row-list">
            {displayNotes.map((note) => {
              const isSelected = note.id === selectedNoteId;
              return (
                <div
                  key={note.id}
                  className={`note-row-item ${isSelected ? "selected" : ""}`}
                  onClick={() => onSelectNote(note)}
                  role="button"
                  tabIndex={0}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      onSelectNote(note);
                    }
                  }}
                >
                  <div className="note-row-top">
                    <span className="note-row-title" title={note.displayTitle}>
                      <HighlightMatch
                        text={note.displayTitle || "제목 없는 메모"}
                        query={searchQuery}
                      />
                    </span>
                    <span className="note-row-time">
                      {formatRelativeTime(note.lastSnapshotAt || note.lastSeenAt)}
                    </span>
                  </div>

                  {note.originalPath && (
                    <div className="note-row-path" title={note.originalPath}>
                      <HighlightMatch
                        text={note.originalPath}
                        query={searchQuery}
                      />
                    </div>
                  )}

                  <div className="note-row-preview">
                    {note.previewText ? (
                      <HighlightMatch
                        text={note.previewText}
                        query={searchQuery}
                      />
                    ) : (
                      <span className="text-faint">(텍스트 내용 없음)</span>
                    )}
                  </div>

                  <div className="note-row-badges">
                    {note.isDeleted ? (
                      <Badge tone="danger">삭제됨</Badge>
                    ) : note.isSaved ? (
                      <Badge tone="muted">저장됨</Badge>
                    ) : (
                      <Badge tone="warn">미저장 탭</Badge>
                    )}

                    <span className="version-pill">
                      v{note.versionCount}
                    </span>

                    {note.lastParseStatus && note.lastParseStatus !== "PARSED" && (
                      <Badge tone="warn">{note.lastParseStatus}</Badge>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
