import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ParseStatus =
  | "PENDING"
  | "PARSED"
  | "UNSUPPORTED_FORMAT"
  | "CORRUPT"
  | "READ_ERROR";

export type SourceType = "tab_state" | "window_state";

export interface StorageStatistics {
  totalNotes: number;
  activeNotes: number;
  deletedNotes: number;
  totalSnapshots: number;
  totalSessions: number;
  totalSnapshotBytes: number;
  oldestSnapshotAt?: string | null;
  newestSnapshotAt?: string | null;
}

export interface NoteSummaryDto {
  id: string;
  sourceId: string;
  displayTitle: string;
  originalPath?: string | null;
  isSaved: boolean;
  isDeleted: boolean;
  firstSeenAt: string;
  lastSeenAt: string;
  deletedAt?: string | null;
  versionCount: number;
  lastSnapshotId?: string | null;
  lastSnapshotAt?: string | null;
  lastContentHash?: string | null;
  lastSourceSize?: number | null;
  lastParseStatus?: ParseStatus | null;
  previewText?: string | null;
}

export interface NotepadSnapshot {
  id: string;
  noteId: string;
  contentHash: string;
  rawSnapshotPath: string;
  recoveredText?: string | null;
  sourceType: SourceType;
  sourceFilename: string;
  sourceSize: number;
  parseStatus: ParseStatus;
  notepadFormatVersion?: number | null;
  capturedAt: string;
  createdAt: string;
}

export interface NotepadRescueSettings {
  enabled: boolean;
  watchEnabled: boolean;
  retentionDays: number;
  deletedRetentionDays: number;
  maxStorageBytes: number;
  updatedAt: string;
}

export interface RescueStatusDto {
  enabled: boolean;
  watchEnabled: boolean;
  isWatching: boolean;
  status: "ACTIVE" | "DISABLED" | "NOTEPAD_NOT_FOUND" | "WATCH_PATH_MISSING" | "WATCHER_ERROR" | string;
  message?: string | null;
  storageStats: StorageStatistics;
  lastSnapshotAt?: string | null;
  localStatePath?: string | null;
}

export interface RescueStatusEvent {
  enabled: boolean;
  watchEnabled: boolean;
  isWatching: boolean;
  status: string;
  message?: string | null;
}

export interface SnapshotCreatedEvent {
  noteId: string;
  sourceId: string;
  displayTitle: string;
  snapshotId: string;
  contentHash: string;
  sourceType: SourceType;
  isSaved: boolean;
  capturedAt: string;
}

export interface NoteDeletedEvent {
  noteId: string;
  sourceId: string;
  deletedAt: string;
}

// ============================================================================
// Tauri API Calls
// ============================================================================

export async function getStatus(): Promise<RescueStatusDto> {
  return invoke<RescueStatusDto>("rescue_get_status");
}

export async function listNotes(
  category?: "active" | "deleted" | "all",
  query?: string,
  limit?: number,
  offset?: number,
): Promise<NoteSummaryDto[]> {
  return invoke<NoteSummaryDto[]>("rescue_list_notes", {
    category,
    query: query?.trim() ? query.trim() : undefined,
    limit,
    offset,
  });
}

export async function getNote(noteId: string): Promise<NoteSummaryDto | null> {
  return invoke<NoteSummaryDto | null>("rescue_get_note", { noteId });
}

export async function listSnapshots(noteId: string): Promise<NotepadSnapshot[]> {
  return invoke<NotepadSnapshot[]>("rescue_list_snapshots", { noteId });
}

export async function getSnapshot(snapshotId: string): Promise<NotepadSnapshot | null> {
  return invoke<NotepadSnapshot | null>("rescue_get_snapshot", { snapshotId });
}

export async function getSettings(): Promise<NotepadRescueSettings> {
  return invoke<NotepadRescueSettings>("rescue_get_settings");
}

export async function updateSettings(
  settings: Partial<NotepadRescueSettings>,
): Promise<NotepadRescueSettings> {
  return invoke<NotepadRescueSettings>("rescue_update_settings", { settings });
}

export async function exportText(
  snapshotId: string,
  destinationPath?: string,
): Promise<string> {
  return invoke<string>("rescue_export_text", { snapshotId, destinationPath });
}

export async function cleanupStale(): Promise<StorageStatistics> {
  return invoke<StorageStatistics>("rescue_cleanup_stale");
}

export async function openExportedNote(path: string): Promise<void> {
  return invoke<void>("rescue_open_exported_note", { path });
}

// ============================================================================
// Event Listeners
// ============================================================================

export async function onRescueStatus(
  callback: (status: RescueStatusEvent) => void,
): Promise<UnlistenFn> {
  return listen<RescueStatusEvent>("rescue://status", (event) => callback(event.payload));
}

export async function onSnapshotCreated(
  callback: (event: SnapshotCreatedEvent) => void,
): Promise<UnlistenFn> {
  return listen<SnapshotCreatedEvent>("rescue://snapshot-created", (event) =>
    callback(event.payload),
  );
}

export async function onNoteDeleted(
  callback: (event: NoteDeletedEvent) => void,
): Promise<UnlistenFn> {
  return listen<NoteDeletedEvent>("rescue://note-deleted", (event) =>
    callback(event.payload),
  );
}

// ============================================================================
// Formatting & Utilities
// ============================================================================

export function formatBytes(bytes?: number | null): string {
  if (bytes == null || bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const val = bytes / Math.pow(1024, i);
  return `${val.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export function formatRelativeTime(isoString?: string | null): string {
  if (!isoString) return "-";
  try {
    const time = new Date(isoString).getTime();
    if (isNaN(time)) return isoString;
    const diff = Math.floor((Date.now() - time) / 1000);
    if (diff < 5) return "방금 전";
    if (diff < 60) return `${diff}초 전`;
    const mins = Math.floor(diff / 60);
    if (mins < 60) return `${mins}분 전`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}시간 전`;
    const days = Math.floor(hours / 24);
    if (days < 30) return `${days}일 전`;
    return new Date(isoString).toLocaleDateString("ko-KR", {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return isoString;
  }
}

export function getParseStatusBadge(status?: ParseStatus | null): {
  label: string;
  tone: "ok" | "warn" | "danger" | "muted";
  description: string;
} {
  switch (status) {
    case "PARSED":
      return { label: "파싱 완료", tone: "ok", description: "텍스트가 정상 추출되었습니다" };
    case "CORRUPT":
      return {
        label: "손상됨(RAW 보존)",
        tone: "warn",
        description: "파일 구조 또는 CRC가 올바르지 않으나 RAW 스냅샷은 안전 보존됨",
      };
    case "UNSUPPORTED_FORMAT":
      return {
        label: "미지원 포맷",
        tone: "warn",
        description: "알 수 없는 메모장 형식 버전이나 RAW 스냅샷은 안전 보존됨",
      };
    case "READ_ERROR":
      return { label: "읽기 실패", tone: "danger", description: "파일 읽기 오류" };
    case "PENDING":
    default:
      return { label: "대기 중", tone: "muted", description: "파싱 대기 중" };
  }
}
