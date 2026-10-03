// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NotepadRescuePage } from "../../pages/NotepadRescuePage";
import {
  type NoteSummaryDto,
  type RescueStatusDto,
  type NotepadSnapshot,
  formatBytes,
  formatRelativeTime,
  getParseStatusBadge,
} from "../notepadRescue";

// Mock Tauri invoke & listen
const mockGetStatus = vi.fn();
const mockListNotes = vi.fn();
const mockGetNote = vi.fn();
const mockListSnapshots = vi.fn();
const mockGetSnapshot = vi.fn();
const mockGetSettings = vi.fn();
const mockUpdateSettings = vi.fn();
const mockExportText = vi.fn();
const mockCleanupStale = vi.fn();
const mockOpenExportedNote = vi.fn();

let statusListener: ((event: unknown) => void) | null = null;
let snapshotListener: ((event: unknown) => void) | null = null;
let noteDeletedListener: ((event: unknown) => void) | null = null;

const unlistenStatus = vi.fn();
const unlistenSnapshot = vi.fn();
const unlistenDeleted = vi.fn();

vi.mock("../notepadRescue", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../notepadRescue")>();
  return {
    ...actual,
    getStatus: () => mockGetStatus(),
    listNotes: (cat: string, q: string) => mockListNotes(cat, q),
    getNote: (id: string) => mockGetNote(id),
    listSnapshots: (id: string) => mockListSnapshots(id),
    getSnapshot: (id: string) => mockGetSnapshot(id),
    getSettings: () => mockGetSettings(),
    updateSettings: (s: unknown) => mockUpdateSettings(s),
    exportText: (id: string, dest?: string) => mockExportText(id, dest),
    cleanupStale: () => mockCleanupStale(),
    openExportedNote: (path: string) => mockOpenExportedNote(path),
    onRescueStatus: (cb: (e: unknown) => void) => {
      statusListener = cb;
      return Promise.resolve(unlistenStatus);
    },
    onSnapshotCreated: (cb: (e: unknown) => void) => {
      snapshotListener = cb;
      return Promise.resolve(unlistenSnapshot);
    },
    onNoteDeleted: (cb: (e: unknown) => void) => {
      noteDeletedListener = cb;
      return Promise.resolve(unlistenDeleted);
    },
  };
});

function defaultStatus(overrides: Partial<RescueStatusDto> = {}): RescueStatusDto {
  return {
    enabled: true,
    watchEnabled: true,
    isWatching: true,
    status: "ACTIVE",
    message: null,
    storageStats: {
      totalNotes: 2,
      activeNotes: 1,
      deletedNotes: 1,
      totalSnapshots: 5,
      totalSessions: 1,
      totalSnapshotBytes: 1048576,
      newestSnapshotAt: "2026-10-03T10:00:00Z",
      oldestSnapshotAt: "2026-10-01T00:00:00Z",
    },
    lastSnapshotAt: "2026-10-03T10:00:00Z",
    localStatePath: "C:\\Users\\test\\AppData\\Local\\Packages\\...\\LocalState",
    ...overrides,
  };
}

function sampleActiveNote(): NoteSummaryDto {
  return {
    id: "note-active-1",
    sourceId: "src-tab-1",
    displayTitle: "프로젝트 회의록",
    originalPath: "D:\\Notes\\meeting.txt",
    isSaved: true,
    isDeleted: false,
    firstSeenAt: "2026-10-01T10:00:00Z",
    lastSeenAt: "2026-10-03T10:00:00Z",
    deletedAt: null,
    versionCount: 3,
    lastSnapshotId: "snap-active-1",
    lastSnapshotAt: "2026-10-03T10:00:00Z",
    lastContentHash: "abcdef1234567890",
    lastSourceSize: 1200,
    lastParseStatus: "PARSED",
    previewText: "회의 안건: API Desk 릴리즈 일정 조율",
  };
}

function sampleDeletedNote(): NoteSummaryDto {
  return {
    id: "note-deleted-2",
    sourceId: "src-tab-2",
    displayTitle: "미저장 임시 아이디어 💡",
    originalPath: null,
    isSaved: false,
    isDeleted: true,
    firstSeenAt: "2026-10-02T10:00:00Z",
    lastSeenAt: "2026-10-02T11:00:00Z",
    deletedAt: "2026-10-02T12:00:00Z",
    versionCount: 1,
    lastSnapshotId: "snap-deleted-2",
    lastSnapshotAt: "2026-10-02T11:00:00Z",
    lastContentHash: "9876543210fedcba",
    lastSourceSize: 500,
    lastParseStatus: "PARSED",
    previewText: "한국어 텍스트, 日本語テキスト, 및 Emoji 🎉✨",
  };
}

function sampleSnapshot(noteId: string, overrides: Partial<NotepadSnapshot> = {}): NotepadSnapshot {
  return {
    id: "snap-active-1",
    noteId,
    contentHash: "abcdef1234567890",
    rawSnapshotPath: "snapshots/ab/abcdef1234567890.bin",
    recoveredText: "회의 안건: API Desk 릴리즈 일정 조율\n\n1. 한국어 지원 확인\n2. 日本語対応確認\n3. 이모지 테스트 🚀🌟",
    sourceType: "tab_state",
    sourceFilename: "src-tab-1.bin",
    sourceSize: 1200,
    parseStatus: "PARSED",
    notepadFormatVersion: 1,
    capturedAt: "2026-10-03T10:00:00Z",
    createdAt: "2026-10-03T10:00:00Z",
    ...overrides,
  };
}

describe("Notepad Rescue Frontend Components & Page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    statusListener = null;
    snapshotListener = null;
    noteDeletedListener = null;

    mockGetStatus.mockResolvedValue(defaultStatus());
    mockListNotes.mockResolvedValue([sampleActiveNote()]);
    mockGetSnapshot.mockResolvedValue(sampleSnapshot("note-active-1"));
    mockListSnapshots.mockResolvedValue([sampleSnapshot("note-active-1")]);
    mockGetSettings.mockResolvedValue({
      enabled: true,
      watchEnabled: true,
      retentionDays: 30,
      deletedRetentionDays: 30,
      maxStorageBytes: 1073741824,
      updatedAt: "2026-10-01T00:00:00Z",
    });
  });

  afterEach(() => cleanup());

  it("renders Notepad Rescue Page and displays ACTIVE protection status", async () => {
    render(<NotepadRescuePage />);

    expect(screen.getByText("Notepad Rescue")).toBeDefined();
    await waitFor(() => {
      expect(screen.getByText("실시간 보호 중")).toBeDefined();
    });
    expect(screen.getByText("보호 중인 메모")).toBeDefined();
  });

  it("renders active note list and displays note preview on selection", async () => {
    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("프로젝트 회의록")).toBeDefined();
    });

    // Click note to select
    const noteRow = screen.getByText("프로젝트 회의록");
    fireEvent.click(noteRow);

    await waitFor(() => {
      expect(mockGetSnapshot).toHaveBeenCalledWith("snap-active-1");
      expect(screen.getAllByText(/API Desk 릴리즈 일정 조율/).length).toBeGreaterThan(0);
      expect(screen.getByText(/한국어 지원 확인/)).toBeDefined();
      expect(screen.getByText(/日本語対応確認/)).toBeDefined();
      expect(screen.getByText(/이모지 테스트 🚀🌟/)).toBeDefined();
    });
  });

  it("switches category to deleted notes and renders deleted list", async () => {
    mockListNotes.mockImplementation((cat: string) => {
      if (cat === "deleted") {
        return Promise.resolve([sampleDeletedNote()]);
      }
      return Promise.resolve([sampleActiveNote()]);
    });

    render(<NotepadRescuePage />);
    const deletedTab = screen.getByRole("button", { name: "삭제된 메모" });
    fireEvent.click(deletedTab);

    await waitFor(() => {
      expect(mockListNotes).toHaveBeenCalledWith("deleted", "");
      expect(screen.getByText("미저장 임시 아이디어 💡")).toBeDefined();
    });
  });

  it("displays empty state cleanly when no notes match category or search", async () => {
    mockListNotes.mockResolvedValue([]);
    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("감시 중인 메모가 없습니다")).toBeDefined();
    });
  });

  it("displays WATCHER_ERROR status badge when backend watcher reports error", async () => {
    mockGetStatus.mockResolvedValue(
      defaultStatus({
        status: "WATCHER_ERROR",
        isWatching: false,
        message: "TabState 파일 접근 권한 거부",
      }),
    );

    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("워커 오류")).toBeDefined();
      expect(screen.getByText("TabState 파일 접근 권한 거부")).toBeDefined();
    });
  });

  it("handles parse failure (UNSUPPORTED_FORMAT / CORRUPT) with safe raw preservation banner", async () => {
    const corruptSnapshot = sampleSnapshot("note-active-1", {
      parseStatus: "CORRUPT",
      recoveredText: null,
    });
    mockGetSnapshot.mockResolvedValue(corruptSnapshot);

    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("프로젝트 회의록")).toBeDefined();
    });

    fireEvent.click(screen.getByText("프로젝트 회의록"));

    await waitFor(() => {
      expect(screen.getByText("안전 보존 알림:")).toBeDefined();
      expect(screen.getByText(/텍스트 미리보기를 지원하지 않는 형식입니다/)).toBeDefined();
      expect(screen.getByText(/RAW 바이너리 스냅샷 파일/)).toBeDefined();
    });
  });

  it("opens version history and displays older snapshot versions", async () => {
    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("프로젝트 회의록")).toBeDefined();
    });
    fireEvent.click(screen.getByText("프로젝트 회의록"));

    await waitFor(() => {
      expect(screen.getByText("버전 기록 (3)")).toBeDefined();
    });

    const historyBtn = screen.getByText("버전 기록 (3)");
    fireEvent.click(historyBtn);

    await waitFor(() => {
      expect(mockListSnapshots).toHaveBeenCalledWith("note-active-1");
      expect(screen.getByText("버전 히스토리")).toBeDefined();
    });
  });

  it("opens settings modal and saves updated retention settings", async () => {
    mockUpdateSettings.mockResolvedValue({
      enabled: true,
      watchEnabled: true,
      retentionDays: 60,
      deletedRetentionDays: 30,
      maxStorageBytes: 1073741824,
      updatedAt: "2026-10-03T11:00:00Z",
    });

    render(<NotepadRescuePage />);

    const settingsBtn = screen.getByTitle("보관 주기 및 자동 감시 설정을 변경합니다");
    fireEvent.click(settingsBtn);

    await waitFor(() => {
      expect(screen.getByText("Notepad Rescue 설정")).toBeDefined();
    });

    const saveBtn = screen.getByText("저장");
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(mockUpdateSettings).toHaveBeenCalled();
    });
  });

  it("handles event snapshot-created and note-deleted reactive updates", async () => {
    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(statusListener).not.toBeNull();
      expect(snapshotListener).not.toBeNull();
      expect(noteDeletedListener).not.toBeNull();
    });

    // Simulate snapshot event
    if (snapshotListener) {
      snapshotListener({
        noteId: "note-active-1",
        sourceId: "src-tab-1",
        displayTitle: "프로젝트 회의록",
        snapshotId: "snap-active-2",
        contentHash: "newhash",
        sourceType: "tab_state",
        isSaved: true,
        capturedAt: "2026-10-03T11:00:00Z",
      });
    }

    // Simulate note deleted event
    if (noteDeletedListener) {
      noteDeletedListener({
        noteId: "note-active-1",
        sourceId: "src-tab-1",
        deletedAt: "2026-10-03T12:00:00Z",
      });
    }
  });

  it("cleans up all event listeners upon unmount without leaks", async () => {
    const { unmount } = render(<NotepadRescuePage />);
    await waitFor(() => {
      expect(statusListener).not.toBeNull();
    });
    unmount();

    expect(unlistenStatus).toHaveBeenCalled();
    expect(unlistenSnapshot).toHaveBeenCalled();
    expect(unlistenDeleted).toHaveBeenCalled();
  });

  it("handles very large text safely in preview without crash", async () => {
    const hugeText = "A".repeat(100_000);
    mockGetSnapshot.mockResolvedValue(
      sampleSnapshot("note-active-1", { recoveredText: hugeText }),
    );

    render(<NotepadRescuePage />);

    await waitFor(() => {
      expect(screen.getByText("프로젝트 회의록")).toBeDefined();
    });
    fireEvent.click(screen.getByText("프로젝트 회의록"));

    await waitFor(() => {
      expect(screen.getByText("100,000글자")).toBeDefined();
    });
  });
});

describe("Utility format functions", () => {
  it("formats bytes accurately in human-readable units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1048576)).toBe("1.0 MB");
    expect(formatBytes(1073741824)).toBe("1.0 GB");
  });

  it("formats relative time correctly", () => {
    expect(formatRelativeTime(null)).toBe("-");
    const now = new Date().toISOString();
    expect(formatRelativeTime(now)).toBe("방금 전");
  });

  it("returns appropriate badge tone and label for parse statuses", () => {
    expect(getParseStatusBadge("PARSED").tone).toBe("ok");
    expect(getParseStatusBadge("CORRUPT").tone).toBe("warn");
    expect(getParseStatusBadge("UNSUPPORTED_FORMAT").tone).toBe("warn");
    expect(getParseStatusBadge("READ_ERROR").tone).toBe("danger");
  });
});
