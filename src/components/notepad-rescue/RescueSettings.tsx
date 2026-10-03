import { useState, useEffect } from "react";
import {
  type NotepadRescueSettings,
  getSettings,
  updateSettings,
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

  useEffect(() => {
    if (!isOpen) return;
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
      })
      .catch((e) => {
        setError(`설정 로드 실패: ${String(e)}`);
        setLoading(false);
      });
  }, [isOpen]);

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
            disabled={saving || loading}
          >
            기본값으로 복원
          </button>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              type="button"
              className="button button-subtle"
              onClick={onClose}
              disabled={saving}
            >
              취소
            </button>
            <button
              type="button"
              className="button"
              onClick={handleSave}
              disabled={saving || loading}
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
            <small className="field-hint">기본 30일. 지정된 기간이 지난 스냅샷은 정리 대상이 됩니다.</small>
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
              메모장에서 탭이 닫히거나 삭제된 후에도 이 기간 동안 복구 가능합니다.
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
            <small className="field-hint">기본 1024 MB (1 GB). 초과 시 오래된 스냅샷부터 관리됩니다.</small>
          </div>
        </div>
      )}
    </Modal>
  );
}
