import { useState, useEffect, useCallback } from "react";
import {
  type NotepadRescueSettings,
  type StorageDetailDto,
  type GcPlan,
  type GcReport,
  getSettings,
  updateSettings,
  getStorageDetails,
  gcPreview,
  gcExecute,
  formatBytes,
} from "../../lib/notepadRescue";
import { Modal } from "../Modal";

interface RescueSettingsProps {
  isOpen: boolean;
  onClose: () => void;
  onSettingsSaved?: (newSettings: NotepadRescueSettings) => void;
}

export function RescueSettings({
  isOpen,
  onClose,
  onSettingsSaved,
}: RescueSettingsProps) {
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Form states
  const [enabled, setEnabled] = useState(true);
  const [watchEnabled, setWatchEnabled] = useState(true);
  const [retentionDays, setRetentionDays] = useState(30);
  const [deletedRetentionDays, setDeletedRetentionDays] = useState(30);
  const [maxStorageMB, setMaxStorageMB] = useState(1024); // 1 GiB

  // Storage details & GC states
  const [storageDetails, setStorageDetails] = useState<StorageDetailDto | null>(null);
  const [previewPlan, setPreviewPlan] = useState<GcPlan | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [executingGc, setExecutingGc] = useState(false);
  const [gcReport, setGcReport] = useState<GcReport | null>(null);

  const loadData = useCallback(() => {
    setLoading(true);
    setError(null);
    getSettings()
      .then((s) => {
        setEnabled(s.enabled);
        setWatchEnabled(s.watchEnabled);
        setRetentionDays(s.retentionDays);
        setDeletedRetentionDays(s.deletedRetentionDays);
        setMaxStorageMB(Math.round(s.maxStorageBytes / (1024 * 1024)));
        setLoading(false);
        // 비동기 스토리지 현황 로드 (선택적)
        getStorageDetails()
          .then((d) => setStorageDetails(d))
          .catch(() => {});
      })
      .catch((e) => {
        setError(`설정 로드 실패: ${String(e)}`);
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    if (!isOpen) return;
    loadData();
    setPreviewPlan(null);
    setGcReport(null);
  }, [isOpen, loadData]);

  const handleSave = async () => {
    try {
      setSaving(true);
      setError(null);

      const maxStorageBytes = Math.max(10, maxStorageMB) * 1024 * 1024;
      const updated = await updateSettings({
        enabled,
        watchEnabled,
        retentionDays: Math.max(1, retentionDays),
        deletedRetentionDays: Math.max(1, deletedRetentionDays),
        maxStorageBytes,
      });

      if (onSettingsSaved) {
        onSettingsSaved(updated);
      }
      onClose();
    } catch (e) {
      setError(`설정 저장 실패: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const handleResetDefaults = () => {
    setEnabled(true);
    setWatchEnabled(true);
    setRetentionDays(30);
    setDeletedRetentionDays(30);
    setMaxStorageMB(1024);
  };

  const handlePreviewGc = async () => {
    try {
      setPreviewLoading(true);
      setError(null);
      setGcReport(null);
      const plan = await gcPreview();
      setPreviewPlan(plan);
    } catch (e) {
      setError(`정리 미리보기 실패: ${String(e)}`);
    } finally {
      setPreviewLoading(false);
    }
  };

  const handleExecuteGc = async () => {
    try {
      setExecutingGc(true);
      setError(null);
      const report = await gcExecute();
      setGcReport(report);
      setPreviewPlan(null);
      // 스토리지 현황 재갱신
      const updatedDetails = await getStorageDetails();
      setStorageDetails(updatedDetails);
    } catch (e) {
      setError(`정리 실행 실패: ${String(e)}`);
    } finally {
      setExecutingGc(false);
    }
  };

  if (!isOpen) return null;

  return (
    <Modal
      title="Notepad Rescue 설정"
      subtitle="메모장 자동 백업 및 보관 정책을 설정합니다"
      onClose={onClose}
      footer={
        <div style={{ display: "flex", justifyContent: "space-between", width: "100%" }}>
          <button
            type="button"
            className="button button-subtle"
            onClick={handleResetDefaults}
            disabled={saving || loading || executingGc}
          >
            기본값으로 복원
          </button>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              type="button"
              className="button button-subtle"
              onClick={onClose}
              disabled={saving || executingGc}
            >
              취소
            </button>
            <button
              type="button"
              className="button"
              onClick={handleSave}
              disabled={saving || loading || executingGc}
            >
              {saving ? "저장 중…" : "저장"}
            </button>
          </div>
        </div>
      }
    >
      {loading ? (
        <div style={{ padding: "20px 0", color: "var(--text-muted)" }}>설정을 불러오는 중…</div>
      ) : (
        <div className="settings-form">
          {error && (
            <div className="alert alert-danger" style={{ marginBottom: 14 }}>
              {error}
            </div>
          )}

          {/* 스토리지 상태 및 쿼터 압박 경고 */}
          {storageDetails && (
            <div
              style={{
                background: storageDetails.hasQuotaPressure
                  ? "rgba(239, 68, 68, 0.1)"
                  : "rgba(255, 255, 255, 0.03)",
                border: storageDetails.hasQuotaPressure
                  ? "1px solid rgba(239, 68, 68, 0.3)"
                  : "1px solid var(--border-color, #333)",
                borderRadius: "8px",
                padding: "12px",
                marginBottom: "16px",
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", marginBottom: "6px" }}>
                <strong>스토리지 현황</strong>
                <span className="text-muted" style={{ fontSize: "0.85rem" }}>
                  {formatBytes(storageDetails.physicalDiskBytes)} / {formatBytes(storageDetails.maxStorageBytes)} (
                  {storageDetails.quotaPercentage.toFixed(1)}%)
                </span>
              </div>

              {storageDetails.hasQuotaPressure && (
                <div
                  style={{
                    color: "#f87171",
                    fontSize: "0.85rem",
                    marginBottom: "8px",
                    fontWeight: "bold",
                  }}
                >
                  ⚠️ 스토리지 할당 한도에 도달했습니다. 활성 메모의 최신 백업은 데이터 안전 정책에 의해
                  항상 보호되지만, 불필요한 오래된 스냅샷의 안전 정리를 권장합니다.
                </div>
              )}

              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: "repeat(3, 1fr)",
                  gap: "8px",
                  fontSize: "0.8rem",
                  color: "var(--text-muted)",
                }}
              >
                <div>논리 버전: {storageDetails.logicalSnapshotCount}개</div>
                <div>물리 블롭: {storageDetails.physicalBlobCount}개</div>
                <div>중복 제거 절약: {formatBytes(storageDetails.deduplicatedBytes)}</div>
                <div>안전 정리 가능: {formatBytes(storageDetails.reclaimableBytes)}</div>
                <div>임시 파일: {formatBytes(storageDetails.tmpBytes)}</div>
                <div>고아 블롭: {storageDetails.orphanBlobCount}개</div>
              </div>
            </div>
          )}

          {/* 스토리지 유지관리 (GC) */}
          <div
            style={{
              background: "rgba(255, 255, 255, 0.02)",
              border: "1px solid var(--border-color, #333)",
              borderRadius: "8px",
              padding: "12px",
              marginBottom: "16px",
            }}
          >
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
              <div>
                <strong>저장공간 유지관리 (Safe GC)</strong>
                <p style={{ margin: "2px 0 0 0", fontSize: "0.8rem", color: "var(--text-muted)" }}>
                  다중 참조 무결성을 확인하여 안전하게 만료된 데이터만 회수합니다.
                </p>
              </div>
              <button
                type="button"
                className="button button-subtle"
                onClick={handlePreviewGc}
                disabled={previewLoading || executingGc}
              >
                {previewLoading ? "분석 중…" : "정리 미리보기"}
              </button>
            </div>

            {/* 미리보기 결과 */}
            {previewPlan && (
              <div
                style={{
                  marginTop: "12px",
                  padding: "10px",
                  background: "rgba(59, 130, 246, 0.08)",
                  borderRadius: "6px",
                  border: "1px solid rgba(59, 130, 246, 0.2)",
                }}
              >
                <div style={{ fontSize: "0.85rem", fontWeight: "bold", marginBottom: "6px" }}>
                  정리 대상 분석 결과
                </div>
                <ul
                  style={{
                    margin: "0 0 10px 0",
                    paddingLeft: "18px",
                    fontSize: "0.8rem",
                    color: "var(--text-muted)",
                  }}
                >
                  <li>만료된 과거 버전: {previewPlan.expiredSnapshotIds.length}개</li>
                  <li>만료된 삭제 메모: {previewPlan.expiredNoteIds.length}개</li>
                  <li>참조 해제 물리 블롭: {previewPlan.unreferencedBlobHashes.length}개</li>
                  <li>임시 파일 (.tmp): {previewPlan.staleTmpFiles.length}개</li>
                  <li>
                    <strong>예상 확보 가능 공간: {formatBytes(previewPlan.totalReclaimableBytes)}</strong>
                  </li>
                </ul>

                <button
                  type="button"
                  className="button"
                  onClick={handleExecuteGc}
                  disabled={executingGc || previewPlan.totalReclaimableBytes === 0}
                  style={{ fontSize: "0.85rem" }}
                >
                  {executingGc ? "안전 정리 실행 중…" : "안전 정리 실행"}
                </button>
              </div>
            )}

            {/* 정리 완료 보고 */}
            {gcReport && (
              <div
                style={{
                  marginTop: "12px",
                  padding: "10px",
                  background: "rgba(34, 197, 94, 0.08)",
                  borderRadius: "6px",
                  border: "1px solid rgba(34, 197, 94, 0.2)",
                  color: "#4ade80",
                  fontSize: "0.85rem",
                }}
              >
                ✓ 안전 정리가 완료되었습니다: {formatBytes(gcReport.totalReclaimedBytes)} 공간 회수
                (스냅샷 {gcReport.deletedSnapshotsCount}개, 물리 블롭 {gcReport.deletedBlobsCount}개 정리)
              </div>
            )}
          </div>

          <div className="settings-field">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
              />
              <span>Notepad Rescue 활성화</span>
            </label>
            <small className="field-hint">
              비활성화 시 모든 백업 및 감시 동작이 중단됩니다.
            </small>
          </div>

          <div className="settings-field">
            <label className="checkbox-label">
              <input
                type="checkbox"
                checked={watchEnabled}
                onChange={(e) => setWatchEnabled(e.target.checked)}
                disabled={!enabled}
              />
              <span>API Desk 실행 중 백그라운드 자동 감시</span>
            </label>
            <small className="field-hint">
              메모장에서 탭을 열거나 수정할 때 실시간으로 스냅샷을 캡처합니다.
            </small>
          </div>

          <hr className="settings-divider" />

          <div className="settings-field">
            <label className="field-label">스냅샷 보관 기간 (일)</label>
            <input
              type="number"
              className="input"
              min={1}
              max={365}
              value={retentionDays}
              onChange={(e) => setRetentionDays(Number(e.target.value))}
              disabled={!enabled}
            />
            <small className="field-hint">
              기본 30일. 지정된 기간이 지난 이전 스냅샷 버전이 정리 대상이 됩니다. (단, 활성 메모의 최신
              스냅샷은 항상 안전 보존됩니다)
            </small>
          </div>

          <div className="settings-field">
            <label className="field-label">삭제된 메모 보관 기간 (일)</label>
            <input
              type="number"
              className="input"
              min={1}
              max={365}
              value={deletedRetentionDays}
              onChange={(e) => setDeletedRetentionDays(Number(e.target.value))}
              disabled={!enabled}
            />
            <small className="field-hint">
              기본 30일. 메모장에서 탭이 닫히거나 삭제된 후에도 이 기간 동안 복구가 유지됩니다. 보관
              기간이 지나면 노트 메타데이터와 스냅샷이 안전하게 소멸됩니다.
            </small>
          </div>

          <div className="settings-field">
            <label className="field-label">최대 스토리지 용량 (MB)</label>
            <input
              type="number"
              className="input"
              min={100}
              max={102400}
              step={100}
              value={maxStorageMB}
              onChange={(e) => setMaxStorageMB(Number(e.target.value))}
              disabled={!enabled}
            />
            <small className="field-hint">
              기본 1024 MB (1 GB). 초과 시 오래된 스냅샷부터 안전하게 관리되며, 최신 백업은 보호됩니다.
            </small>
          </div>
        </div>
      )}
    </Modal>
  );
}
