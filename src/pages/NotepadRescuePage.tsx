import { useState, useEffect, useCallback, useRef } from "react";
import {
  type NoteSummaryDto,
  type RescueStatusDto,
  type SnapshotCreatedEvent,
  type NoteDeletedEvent,
  type RescueStatusEvent,
  getStatus,
  listNotes,
  searchNotes,
  cleanupStale,
  onRescueStatus,
  onSnapshotCreated,
  onNoteDeleted,
} from "../lib/notepadRescue";
import { RescueStatus } from "../components/notepad-rescue/RescueStatus";
import { NoteList } from "../components/notepad-rescue/NoteList";
import { NotePreview } from "../components/notepad-rescue/NotePreview";
import { VersionHistory } from "../components/notepad-rescue/VersionHistory";
import { RescueSettings } from "../components/notepad-rescue/RescueSettings";

export function NotepadRescuePage() {
  const [status, setStatus] = useState<RescueStatusDto | null>(null);
  const [notes, setNotes] = useState<NoteSummaryDto[]>([]);
  const [selectedNote, setSelectedNote] = useState<NoteSummaryDto | null>(null);
  const [category, setCategory] = useState<"active" | "deleted" | "all">("active");
  const [searchQuery, setSearchQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [cleaning, setCleaning] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showHistory, setShowHistory] = useState(false);

  const selectedNoteIdRef = useRef<string | null>(null);
  selectedNoteIdRef.current = selectedNote?.id ?? null;

  // Refresh status
  const refreshStatus = useCallback(async () => {
    try {
      const data = await getStatus();
      setStatus(data);
    } catch (e) {
      console.error("Failed to load Notepad Rescue status", e);
    }
  }, []);

  // Refresh notes list (uses hardened backend search query with bounded snippets when searching)
  const refreshNotes = useCallback(async () => {
    try {
      const q = searchQuery.trim();
      if (q) {
        const includeDeleted = category === "all" || category === "deleted";
        const results = await searchNotes(q, "ALL", includeDeleted);
        const filtered = results.filter((r) => {
          if (category === "active") return !r.isDeleted;
          if (category === "deleted") return r.isDeleted;
          return true;
        });
        const mapped: NoteSummaryDto[] = filtered.map((r) => ({
          id: r.noteId,
          sourceId: r.sourceId,
          displayTitle: r.displayTitle,
          originalPath: r.originalPath,
          isSaved: r.isSaved,
          isDeleted: r.isDeleted,
          firstSeenAt: r.lastSeenAt,
          lastSeenAt: r.lastSeenAt,
          versionCount: r.versionCount,
          lastSnapshotAt: r.lastSnapshotAt,
          previewText: r.snippet || null,
        }));
        setNotes(mapped);
        if (selectedNoteIdRef.current) {
          const updated = mapped.find((n) => n.id === selectedNoteIdRef.current);
          if (updated) setSelectedNote(updated);
        }
      } else {
        const data = await listNotes(category, "");
        setNotes(data);
        if (selectedNoteIdRef.current) {
          const updated = data.find((n) => n.id === selectedNoteIdRef.current);
          if (updated) setSelectedNote(updated);
        }
      }
    } catch (e) {
      console.error("Failed to load Notepad Rescue notes", e);
    } finally {
      setLoading(false);
    }
  }, [category, searchQuery]);

  // Initial load
  useEffect(() => {
    void refreshStatus();
    void refreshNotes();
  }, [refreshStatus, refreshNotes]);

  // Event bridge setup & cleanup
  useEffect(() => {
    let unlistenStatus: (() => void) | undefined;
    let unlistenSnapshot: (() => void) | undefined;
    let unlistenDeleted: (() => void) | undefined;
    let mounted = true;

    onRescueStatus((event: RescueStatusEvent) => {
      if (!mounted) return;
      setStatus((prev) => {
        if (!prev) return null;
        return {
          ...prev,
          enabled: event.enabled,
          watchEnabled: event.watchEnabled,
          isWatching: event.isWatching,
          status: event.status,
          message: event.message,
        };
      });
    }).then((unlisten) => {
      if (mounted) unlistenStatus = unlisten;
      else unlisten();
    });

    onSnapshotCreated((_event: SnapshotCreatedEvent) => {
      if (!mounted) return;
      // Refresh status & notes upon new snapshot
      void refreshStatus();
      void refreshNotes();
    }).then((unlisten) => {
      if (mounted) unlistenSnapshot = unlisten;
      else unlisten();
    });

    onNoteDeleted((event: NoteDeletedEvent) => {
      if (!mounted) return;
      setNotes((prevNotes) =>
        prevNotes.map((n) =>
          n.id === event.noteId
            ? { ...n, isDeleted: true, deletedAt: event.deletedAt }
            : n,
        ),
      );
      if (selectedNoteIdRef.current === event.noteId) {
        setSelectedNote((prev) =>
          prev ? { ...prev, isDeleted: true, deletedAt: event.deletedAt } : null,
        );
      }
      void refreshStatus();
    }).then((unlisten) => {
      if (mounted) unlistenDeleted = unlisten;
      else unlisten();
    });

    return () => {
      mounted = false;
      if (unlistenStatus) unlistenStatus();
      if (unlistenSnapshot) unlistenSnapshot();
      if (unlistenDeleted) unlistenDeleted();
    };
  }, [refreshStatus, refreshNotes]);

  // Handle orphan cleanup
  const handleCleanup = async () => {
    try {
      setCleaning(true);
      const newStats = await cleanupStale();
      setStatus((prev) => (prev ? { ...prev, storageStats: newStats } : null));
    } catch (e) {
      console.error("Cleanup failed", e);
    } finally {
      setCleaning(false);
    }
  };

  return (
    <div className="notepad-rescue-page">
      {/* Top Status Header */}
      <RescueStatus
        status={status}
        loading={loading}
        onRefresh={() => {
          void refreshStatus();
          void refreshNotes();
        }}
        onOpenSettings={() => setShowSettings(true)}
        onCleanup={() => void handleCleanup()}
        cleaning={cleaning}
      />

      {/* Main Content: Split List and Preview */}
      <div className="notepad-rescue-split-layout">
        <div className="notepad-rescue-left-pane">
          <NoteList
            notes={notes}
            selectedNoteId={selectedNote?.id ?? null}
            onSelectNote={(note) => {
              setSelectedNote(note);
              setShowHistory(false);
            }}
            category={category}
            onCategoryChange={(cat) => setCategory(cat)}
            searchQuery={searchQuery}
            onSearchChange={(q) => setSearchQuery(q)}
            loading={loading}
          />
        </div>

        <div className="notepad-rescue-right-pane">
          <NotePreview
            note={selectedNote}
            showHistory={showHistory}
            onOpenHistory={() => setShowHistory((prev) => !prev)}
            onSnapshotUpdated={() => {
              void refreshStatus();
              void refreshNotes();
            }}
          />

          {showHistory && selectedNote && (
            <VersionHistory
              noteId={selectedNote.id}
              noteTitle={selectedNote.displayTitle}
              onClose={() => setShowHistory(false)}
            />
          )}
        </div>
      </div>

      {/* Settings Modal */}
      <RescueSettings
        isOpen={showSettings}
        onClose={() => setShowSettings(false)}
        onSettingsSaved={() => {
          void refreshStatus();
        }}
      />
    </div>
  );
}
