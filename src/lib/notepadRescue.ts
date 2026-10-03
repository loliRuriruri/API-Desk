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

export type SearchScope = "ALL" | "TITLE" | "PATH" | "CONTENT";

export interface SearchResultDto {
  noteId: string;
  sourceId: string;
  displayTitle: string;
  originalPath?: string | null;
  isSaved: boolean;
  isDeleted: boolean;
  lastSeenAt: string;
  matchedScope: "TITLE" | "PATH" | "CONTENT" | string;
  snippet?: string | null;
  versionCount: number;
  lastSnapshotAt?: string | null;
}

export type DiffKind = "UNCHANGED" | "ADDED" | "REMOVED";

export interface DiffBlock {
  kind: DiffKind;
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  lines: string[];
}

export interface DiffResultDto {
  noteId: string;
  baseSnapshotId: string;
  targetSnapshotId: string;
  isTooLarge: boolean;
  totalAddedLines: number;
  totalRemovedLines: number;
  totalUnchangedLines: number;
  blocks: DiffBlock[];
  message?: string | null;
}

export interface GcPlan {
  expiredSnapshotIds: string[];
  expiredNoteIds: string[];
  unreferencedBlobHashes: string[];
  reclaimableBlobBytes: number;
  staleTmpFiles: string[];
  staleTmpBytes: number;
  orphanBlobsToDelete: string[];
  orphanBlobBytes: number;
  totalReclaimableBytes: number;
  currentStorageBytes: number;
  estimatedStorageAfterBytes: number;
  hasQuotaPressure: boolean;
}

export interface GcReport {
  deletedSnapshotsCount: number;
  deletedNotesCount: number;
  deletedBlobsCount: number;
  deletedBlobsBytes: number;
  deletedTmpCount: number;
  deletedTmpBytes: number;
  deletedOrphansCount: number;
  totalReclaimedBytes: number;
  hasQuotaPressure: boolean;
  storageAfterBytes: number;
}

export interface StorageDetailDto {
  logicalSnapshotCount: number;
  physicalBlobCount: number;
  logicalSourceBytes: number;
  physicalDiskBytes: number;
  deduplicatedBytes: number;
  reclaimableBytes: number;
  orphanBlobCount: number;
  tmpBytes: number;
  maxStorageBytes: number;
  quotaPercentage: number;
  hasQuotaPressure: boolean;
}

export interface NotepadNote {
  id: string;
  sourceId: string;
  displayTitle: string;
  originalPath?: string | null;
  isSaved: boolean;
  isDeleted: boolean;
  firstSeenAt: string;
  lastSeenAt: string;
  deletedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export type RecoveryConfidence = "FULL" | "TEXT_ONLY" | "RAW_ONLY" | "UNAVAILABLE";

export interface NoteRecoveryDetailDto {
  note: NotepadNote;
  latestSnapshot?: NotepadSnapshot | null;
  latestValidSnapshot?: NotepadSnapshot | null;
  latestConfidence: RecoveryConfidence;
  validConfidence?: RecoveryConfidence | null;
  isLatestCorrupt: boolean;
}

export interface RecoveryPackageReport {
  packagePath: string;
  totalNotesExported: number;
  totalBlobsCopied: number;
  manifestFile: string;
  createdAt: string;
}

export interface DiagnosticReportDto {
  rescueVersion: string;
  schemaVersion: number;
  noteCount: number;
  activeNoteCount: number;
  deletedNoteCount: number;
  snapshotCount: number;
  casBlobCount: number;
  parseStatusCounts: Record<string, number>;
  missingBlobsCount: number;
  orphanBlobsCount: number;
  physicalStorageBytes: number;
  maxStorageBytes: number;
  hasQuotaPressure: boolean;
  watcherEnabled: boolean;
  lastSuccessfulSnapshotAt?: string | null;
  timestamp: string;
}

export interface RawBlobScanResult {
  hash: string;
  fileSize: number;
  isValidTabstate: boolean;
  formatVersion?: number | null;
  recoveredTitle?: string | null;
  recoveredTextPreview?: string | null;
  parseError?: string | null;
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
  provider?: string | null;
  agentRunning?: boolean | null;
}

export interface AgentProtectionStatusDto {
  isRunning: boolean;
  agentRunning?: boolean;
  pid?: number | null;
  status: string;
  lastHeartbeatAt?: string | null;
  startupEnabled: boolean;
  inProcessWatching?: boolean;
  watchPath?: string | null;
  storageRoot?: string | null;
  uptimeSecs?: number | null;
  provider?: string;
  isCompatible?: boolean;
  protocolVersion?: number;
  statusMessage?: string | null;
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

export async function searchNotes(
  query: string,
  scope: SearchScope = "ALL",
  includeDeleted: boolean = false,
  limit: number = 50,
  offset: number = 0,
): Promise<SearchResultDto[]> {
  return invoke<SearchResultDto[]>("rescue_search_notes", {
    query,
    scope,
    includeDeleted,
    limit,
    offset,
  });
}

export async function diffSnapshots(
  noteId: string,
  baseSnapshotId: string,
  targetSnapshotId: string,
): Promise<DiffResultDto> {
  return invoke<DiffResultDto>("rescue_diff_snapshots", {
    noteId,
    baseSnapshotId,
    targetSnapshotId,
  });
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

export async function gcPreview(): Promise<GcPlan> {
  return invoke<GcPlan>("rescue_gc_preview");
}

export async function gcExecute(): Promise<GcReport> {
  return invoke<GcReport>("rescue_gc_execute");
}

export async function getStorageDetails(): Promise<StorageDetailDto> {
  return invoke<StorageDetailDto>("rescue_get_storage_details");
}

export async function getRecoveryDetail(noteId: string): Promise<NoteRecoveryDetailDto> {
  return invoke<NoteRecoveryDetailDto>("rescue_get_recovery_detail", { noteId });
}

export async function exportRecoveryPackage(
  destinationDir?: string,
): Promise<RecoveryPackageReport> {
  return invoke<RecoveryPackageReport>("rescue_export_package", { destinationDir });
}

export async function getDiagnosticReport(): Promise<DiagnosticReportDto> {
  return invoke<DiagnosticReportDto>("rescue_get_diagnostics");
}

export async function scanRawStorage(): Promise<RawBlobScanResult[]> {
  return invoke<RawBlobScanResult[]>("rescue_scan_raw_storage");
}

export async function bulkExportNotes(destinationDir?: string): Promise<string[]> {
  return invoke<string[]>("rescue_bulk_export_notes", { destinationDir });
}

export async function getAgentStatus(): Promise<AgentProtectionStatusDto> {
  return invoke<AgentProtectionStatusDto>("rescue_get_agent_status");
}

export async function startAgent(): Promise<AgentProtectionStatusDto> {
  return invoke<AgentProtectionStatusDto>("rescue_start_agent");
}

export async function stopAgent(): Promise<AgentProtectionStatusDto> {
  return invoke<AgentProtectionStatusDto>("rescue_stop_agent");
}

export async function restartAgent(): Promise<AgentProtectionStatusDto> {
  return invoke<AgentProtectionStatusDto>("rescue_restart_agent");
}

export async function setStartupProtection(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("rescue_set_startup_protection", { enabled });
}

export async function getStartupProtection(): Promise<boolean> {
  return invoke<boolean>("rescue_get_startup_protection");
}

export interface RestoreNativeReport {
  success: boolean;
  noteId: string;
  snapshotId: string;
  restoredTabUuid: string;
  restoredTabPath: string;
  backupDir: string;
  notepadLaunched: boolean;
  message: string;
}

export interface RollbackReport {
  success: boolean;
  restoredNoteId?: string | null;
  backupTimestamp?: string | null;
  removedTabFile?: string | null;
  removedWindowFiles: string[];
  message: string;
}

export async function restoreNativeTab(
  noteId: string,
  snapshotId?: string,
): Promise<RestoreNativeReport> {
  return invoke<RestoreNativeReport>("rescue_restore_native_tab", { noteId, snapshotId });
}

export async function rollbackLastRestore(): Promise<RollbackReport> {
  return invoke<RollbackReport>("rescue_rollback_last_restore");
}

export async function canRollback(): Promise<boolean> {
  return invoke<boolean>("rescue_can_rollback");
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
