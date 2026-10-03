import {
  type RescueStatusDto,
  formatBytes,
  formatRelativeTime,
} from "../../lib/notepadRescue";
import { Badge } from "../Badges";

interface RescueStatusProps {
  status: RescueStatusDto | null;
  loading: boolean;
  onRefresh: () => void;
  onOpenSettings: () => void;
  onCleanup: () => void;
  cleaning: boolean;
}

export function RescueStatus({
  status,
  loading,
  onRefresh,
  onOpenSettings,
  onCleanup,
  cleaning,
}: RescueStatusProps) {
  const getStatusBadge = (statusCode?: string) => {
    switch (statusCode) {
      case "ACTIVE":
        return <Badge tone="ok">실시간 보호 중</Badge>;
      case "DISABLED":
        return <Badge tone="muted">보호 비활성화</Badge>;
      case "NOTEPAD_NOT_FOUND":
        return <Badge tone="warn">메모장 미설치/미발견</Badge>;
      case "WATCH_PATH_MISSING":
        return <Badge tone="warn">세션 폴더 대기</Badge>;
      case "WATCHER_ERROR":
        return <Badge tone="danger">워커 오류</Badge>;
      default:
        return <Badge tone="muted">상태 확인 중</Badge>;
    }
  };

  const stats = status?.storageStats;

  return (
    <div className="notepad-rescue-status-bar">
      <div className="notepad-rescue-header">
        <div className="notepad-rescue-title-area">
          <div className="notepad-rescue-title-row">
            <h1 className="notepad-rescue-title">Notepad Rescue</h1>
            {getStatusBadge(status?.status)}
            {status?.provider && (
              <Badge tone={status.provider === "AGENT" ? "ok" : "info"}>
                {status.provider === "AGENT" ? "백그라운드 에이전트" : "API Desk 내장"}
              </Badge>
            )}
          </div>
          <p className="notepad-rescue-subtitle">
            Windows 11 메모장의 미저장 탭 및 열린 세션을 비침습적으로 자동 백업하고 복구합니다.
          </p>
        </div>

        <div className="notepad-rescue-actions">
          <button
            type="button"
            className="button button-subtle"
            onClick={onCleanup}
            disabled={cleaning || loading}
            title="스토리지 내 만료된 임시 파일(.tmp)을 정리합니다"
          >
            {cleaning ? "정리 중…" : "임시 파일 정리"}
          </button>
          <button
            type="button"
            className="button button-subtle"
            onClick={onRefresh}
            disabled={loading}
            title="상태 및 노트 목록을 새로고침합니다"
          >
            새로고침
          </button>
          <button
            type="button"
            className="button"
            onClick={onOpenSettings}
            title="보관 주기 및 자동 감시 설정을 변경합니다"
          >
            설정
          </button>
        </div>
      </div>

      {status?.message && status.status !== "ACTIVE" && (
        <div className="alert alert-warning" style={{ marginTop: 10 }}>
          <span>{status.message}</span>
        </div>
      )}

      <div className="notepad-rescue-cards">
        <div className="notepad-rescue-card">
          <span className="card-label">보호 중인 메모</span>
          <span className="card-value card-value-ok">
            {stats ? stats.activeNotes : "-"}
            <span className="card-subvalue">/ {stats ? stats.totalNotes : "-"}</span>
          </span>
        </div>
        <div className="notepad-rescue-card">
          <span className="card-label">삭제된 메모</span>
          <span className="card-value">
            {stats ? stats.deletedNotes : "-"}
          </span>
        </div>
        <div className="notepad-rescue-card">
          <span className="card-label">스냅샷 버전</span>
          <span className="card-value">
            {stats ? stats.totalSnapshots : "-"}
          </span>
        </div>
        <div className="notepad-rescue-card">
          <span className="card-label">스토리지 사용량</span>
          <span className="card-value">
            {stats ? formatBytes(stats.totalSnapshotBytes) : "-"}
          </span>
        </div>
        <div className="notepad-rescue-card">
          <span className="card-label">마지막 백업</span>
          <span className="card-value card-value-time">
            {formatRelativeTime(status?.lastSnapshotAt)}
          </span>
        </div>
      </div>
    </div>
  );
}
