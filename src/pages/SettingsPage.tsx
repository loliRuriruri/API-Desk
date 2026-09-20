import { useEffect, useState } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Modal } from "../components/Modal";
import { useToast } from "../components/ToastProvider";
import { useAsyncData } from "../hooks/useAsyncData";
import { listAllCredentialFields, listCredentialProjectUsage, listCredentialsWithContext } from "../lib/db/repo";
import { getAutostart, setAutostart } from "../lib/desktop";
import { errorMessage } from "../lib/errors";
import {
  loadAlertThresholds,
  saveAlertThresholds,
  type AlertThresholds,
} from "../lib/settings";
import { usageAdapterFor } from "../lib/providers/usage";
import { monitorProbe } from "../lib/system";
import { buildMetadataExport } from "../lib/export";
import {
  loadImportNotes,
  saveImportNotes,
  saveSetting,
  type AppSettings,
  type ThemeSetting,
} from "../lib/settings";
import {
  exportCredentials,
  getAppPaths,
  writeTextFile,
  type ExportEntryInput,
} from "../lib/system";
import { vaultReset } from "../lib/vault";

interface SettingsPageProps {
  settings: AppSettings;
  onChange: (settings: AppSettings) => void;
  onVaultChanged: () => void;
}

const CLEAR_OPTIONS = [
  { value: 0, label: "사용 안 함" },
  { value: 10, label: "10초" },
  { value: 30, label: "30초" },
  { value: 60, label: "60초" },
];

const HIDE_OPTIONS = [
  { value: 10, label: "10초" },
  { value: 30, label: "30초" },
  { value: 60, label: "60초" },
];

export function SettingsPage({ settings, onChange, onVaultChanged }: SettingsPageProps) {
  const { notify } = useToast();
  const { data: paths } = useAsyncData(() => getAppPaths(), "app-paths");
  const { data: notes, reload: reloadNotes } = useAsyncData(
    () => loadImportNotes(),
    "import-notes",
  );
  const [resetOpen, setResetOpen] = useState(false);
  const [resetConfirm, setResetConfirm] = useState("");
  const [exportOpen, setExportOpen] = useState<"txt" | "csv" | null>(null);
  const [exportConfirmed, setExportConfirmed] = useState(false);
  const [exportBusy, setExportBusy] = useState(false);
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [thresholds, setThresholds] = useState<AlertThresholds>({});

  useEffect(() => {
    void getAutostart().then(setAutostartEnabled);
    void loadAlertThresholds().then(setThresholds);
  }, []);

  const { data: thresholdItems } = useAsyncData(async () => {
    const [probes, credentials] = await Promise.all([
      monitorProbe(),
      listCredentialsWithContext(),
    ]);
    const usageCredentials = credentials
      .map((credential) => ({
        credential,
        adapter: usageAdapterFor(
          credential.provider_name,
          credential.provider_display_name,
          credential.provider_base_url,
        ),
      }))
      .filter((row) => row.adapter !== null);
    return { probes, usageCredentials };
  }, "alert-threshold-items");

  const updateThreshold = (key: string, raw: string) => {
    const next: AlertThresholds = { ...thresholds };
    if (raw.trim() === "") {
      delete next[key];
    } else {
      next[key] = Math.max(1, Math.min(90, Number(raw) || settings.alertThresholdPercent));
    }
    setThresholds(next);
    void saveAlertThresholds(next);
  };

  const update = async <K extends keyof AppSettings>(field: K, value: AppSettings[K]) => {
    try {
      await saveSetting(field, value);
      onChange({ ...settings, [field]: value });
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const browseDefaultDir = async () => {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "기본 프로젝트 폴더 선택",
      defaultPath: settings.defaultProjectDir || undefined,
    });
    if (typeof selected === "string" && selected.length > 0) {
      await update("defaultProjectDir", selected);
    }
  };

  const exportMetadata = async () => {
    try {
      const target = await saveDialog({
        title: "API Desk 메타데이터 내보내기",
        defaultPath: "api-desk-metadata.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!target) return;
      const content = await buildMetadataExport();
      await writeTextFile(target, content);
      notify("메타데이터를 내보냈습니다 (Secret 미포함)", "success");
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const buildExportEntries = async (): Promise<ExportEntryInput[]> => {
    const [credentials, usage, fields] = await Promise.all([
      listCredentialsWithContext(),
      listCredentialProjectUsage(),
      listAllCredentialFields(),
    ]);
    const usageByCredential = new Map<string, string[]>();
    for (const row of usage) {
      const list = usageByCredential.get(row.credential_id) ?? [];
      list.push(`${row.project_name}${row.purpose ? ` (${row.purpose})` : ""}`);
      usageByCredential.set(row.credential_id, list);
    }
    const fieldsByCredential = new Map<string, typeof fields>();
    for (const field of fields) {
      const list = fieldsByCredential.get(field.credential_id) ?? [];
      list.push(field);
      fieldsByCredential.set(field.credential_id, list);
    }
    const entries: ExportEntryInput[] = [];
    for (const credential of credentials) {
      const credentialFields = fieldsByCredential.get(credential.id) ?? [];
      const secretRefs =
        credentialFields.length > 0
          ? credentialFields.map((field) => ({
              field: field.label,
              envName: field.env_name,
              secretId: field.secret_id,
            }))
          : [
              {
                field: "API Key",
                envName: credential.env_name,
                secretId: credential.secret_id,
              },
            ];
      for (const ref of secretRefs) {
        entries.push({
          provider: credential.provider_display_name,
          account: credential.account_name,
          name: credential.name,
          field: ref.field,
          envName: ref.envName ?? credential.env_name,
          status: credential.status,
          lastTestStatus: credential.last_test_status,
          lastTestedAt: credential.last_tested_at,
          expiresAt: credential.expires_at,
          projectUsage: usageByCredential.get(credential.id)?.join(", ") ?? null,
          notes: credential.notes,
          secretId: ref.secretId,
        });
      }
    }
    return entries;
  };

  const runExport = async (format: "txt" | "csv") => {
    setExportBusy(true);
    try {
      const target = await saveDialog({
        title: format === "csv" ? "API 키 CSV 내보내기" : "API 키 텍스트 내보내기",
        defaultPath: format === "csv" ? "api-desk-keys.csv" : "api-desk-keys.txt",
        filters:
          format === "csv"
            ? [{ name: "CSV (Excel)", extensions: ["csv"] }]
            : [{ name: "텍스트", extensions: ["txt"] }],
      });
      if (!target) return;
      const entries = await buildExportEntries();
      const result = await exportCredentials(entries, target, format);
      notify(
        `키 ${result.exported}개를 내보냈습니다${
          result.missing > 0 ? ` (Vault에 없는 키 ${result.missing}개는 빈 값)` : ""
        } · 파일을 안전하게 보관하세요`,
        "success",
      );
      setExportOpen(null);
      setExportConfirmed(false);
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setExportBusy(false);
    }
  };

  const deleteNote = async (id: string) => {
    if (!notes) return;
    try {
      await saveImportNotes(notes.filter((note) => note.id !== id));
      reloadNotes();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const confirmReset = async () => {
    try {
      await vaultReset(resetConfirm);
      notify("Vault를 초기화했습니다. 새 마스터 비밀번호를 만드세요.", "success");
      setResetOpen(false);
      setResetConfirm("");
      onVaultChanged();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  return (
    <div className="page settings-page">
      <section className="panel">
        <header className="panel-header">
          <h3>화면</h3>
        </header>
        <label className="field field-inline">
          <span>테마</span>
          <select
            value={settings.theme}
            onChange={(event) => void update("theme", event.target.value as ThemeSetting)}
          >
            <option value="system">시스템</option>
            <option value="dark">다크</option>
            <option value="light">라이트</option>
            <option value="frutiger-aero">프루티거 에어로 아쿠아 글로스</option>
          </select>
        </label>
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>비밀 값 처리</h3>
        </header>
        <label className="field field-inline">
          <span>클립보드 지우기</span>
          <select
            value={settings.clipboardClearSecs}
            onChange={(event) => void update("clipboardClearSecs", Number(event.target.value))}
          >
            {CLEAR_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="field field-inline">
          <span>보기 자동 숨김</span>
          <select
            value={settings.autoHideSecs}
            onChange={(event) => void update("autoHideSecs", Number(event.target.value))}
          >
            {HIDE_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <p className="form-hint">
          클립보드에 복사한 값이 그대로 남아 있을 때만 지웁니다 — 그 사이 다른 값을 복사했다면
          건드리지 않습니다.
        </p>
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>프로젝트 및 저장 위치</h3>
        </header>
        <label className="field field-wide">
          <span>기본 프로젝트 폴더</span>
          <div className="field-row">
            <input
              className="mono"
              value={settings.defaultProjectDir}
              onChange={(event) => void update("defaultProjectDir", event.target.value)}
            />
            <button type="button" className="button" onClick={() => void browseDefaultDir()}>
              찾아보기…
            </button>
          </div>
        </label>
        <dl className="path-list">
          <div>
            <dt>데이터베이스</dt>
            <dd className="mono">{paths?.dbPath ?? "…"}</dd>
          </div>
          <div>
            <dt>Vault</dt>
            <dd className="mono">{paths?.vaultPath ?? "…"}</dd>
          </div>
          <div>
            <dt>Salt</dt>
            <dd className="mono">{paths?.saltPath ?? "…"}</dd>
          </div>
        </dl>
        <p className="form-hint">
          메타데이터는 SQLite에 저장됩니다. API 키는 저장되지 않고 암호화된 Stronghold Vault에
          보관됩니다.
        </p>
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>상주 · 알림 · 자동 시작</h3>
        </header>
        <label className="field field-check">
          <input
            type="checkbox"
            checked={settings.trayResident}
            onChange={(event) => void update("trayResident", event.target.checked)}
          />
          <span>창을 닫아도 트레이에 남기기 (상주)</span>
        </label>
        <label className="field field-check">
          <input
            type="checkbox"
            checked={settings.openMiniOnStart}
            onChange={(event) => void update("openMiniOnStart", event.target.checked)}
          />
          <span>시작할 때 미니 창 열기 (Vault 잠금 상태에서도 사용량·쿼터 확인 가능)</span>
        </label>
        <label className="field field-check">
          <input
            type="checkbox"
            checked={autostartEnabled}
            onChange={(event) => {
              const next = event.target.checked;
              setAutostartEnabled(next);
              void setAutostart(next);
            }}
          />
          <span>Windows 시작 시 자동 실행 (창 없이 트레이에서 대기)</span>
        </label>
        <label className="field field-check">
          <input
            type="checkbox"
            checked={settings.alertsEnabled}
            onChange={(event) => void update("alertsEnabled", event.target.checked)}
          />
          <span>사용량 임계치 알림</span>
        </label>
        <label className="field field-inline">
          <span>알림 임계치 (%)</span>
          <input
            className="input input-narrow"
            type="number"
            min={1}
            max={90}
            value={settings.alertThresholdPercent}
            onChange={(event) =>
              void update(
                "alertThresholdPercent",
                Math.max(1, Math.min(90, Number(event.target.value) || 20)),
              )
            }
          />
        </label>
        <label className="field field-inline">
          <span>자동 잠금 (Vault)</span>
          <select
            value={settings.autoLockMinutes}
            onChange={(event) => void update("autoLockMinutes", Number(event.target.value))}
          >
            <option value={0}>사용 안 함 (수동 잠금)</option>
            <option value={5}>5분 유휴 후</option>
            <option value={15}>15분 유휴 후</option>
            <option value={30}>30분 유휴 후</option>
            <option value={60}>60분 유휴 후</option>
          </select>
        </label>
        <p className="form-hint">
          트레이 아이콘을 클릭하면 미니 창이 열리고, 가운데 메뉴에서 사용량 새로고침·종료를 실행할 수
          있습니다. 알림은 임계치 이하로 내려간 항목을 값이 바뀔 때 한 번씩 알려줍니다.
        </p>
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>항목별 알림 임계치</h3>
          <span className="panel-hint">
            비우면 전역 임계치({settings.alertThresholdPercent}%) 사용
          </span>
        </header>
        {thresholdItems ? (
          <table className="table table-compact">
            <tbody>
              {thresholdItems.probes.map((probe) => (
                <tr key={`monitor-${probe.monitor}`}>
                  <td>{probe.label}</td>
                  <td className="muted">계정 쿼터</td>
                  <td className="num">
                    <input
                      className="input input-narrow"
                      type="number"
                      min={1}
                      max={90}
                      placeholder={String(settings.alertThresholdPercent)}
                      value={thresholds[`monitor-${probe.monitor}`] ?? ""}
                      onChange={(event) =>
                        updateThreshold(`monitor-${probe.monitor}`, event.target.value)
                      }
                    />
                  </td>
                </tr>
              ))}
              {thresholdItems.usageCredentials.map(({ credential, adapter }) => (
                <tr key={`usage-${credential.id}`}>
                  <td>
                    {credential.provider_display_name} · {credential.name}
                  </td>
                  <td className="muted">{adapter}</td>
                  <td className="num">
                    <input
                      className="input input-narrow"
                      type="number"
                      min={1}
                      max={90}
                      placeholder={String(settings.alertThresholdPercent)}
                      value={thresholds[`usage-${credential.id}`] ?? ""}
                      onChange={(event) =>
                        updateThreshold(`usage-${credential.id}`, event.target.value)
                      }
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="empty-note">임계치를 설정할 항목을 불러오는 중…</p>
        )}
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>백업 및 내보내기</h3>
        </header>
        <div className="row-actions">
          <button type="button" className="button" onClick={() => void exportMetadata()}>
            메타데이터 JSON 내보내기
          </button>
        </div>
        <p className="form-hint">
          내보내기 파일에는 프로바이더·계정·프로젝트·인증 정보 참조만 포함됩니다(Secret 미포함).
        </p>
      </section>

      <section className="panel panel-danger">
        <header className="panel-header">
          <h3>키 내보내기 (Secret 포함)</h3>
        </header>
        <p className="warn-note">
          내보낸 파일에는 API 키 원문이 <strong>평문</strong>으로 저장됩니다. 암호화된 드라이브나
          안전한 위치에만 보관하고, 사용 후에는 삭제하세요.
        </p>
        <div className="row-actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              setExportOpen("txt");
              setExportConfirmed(false);
            }}
          >
            텍스트(.txt)
          </button>
          <button
            type="button"
            className="button"
            onClick={() => {
              setExportOpen("csv");
              setExportConfirmed(false);
            }}
          >
            CSV (Excel)
          </button>
        </div>
        <p className="form-hint">
          프로바이더 · 계정 · 환경변수 · 상태 · 사용 프로젝트 정보를 키와 함께 내보냅니다.
        </p>
      </section>

      <section className="panel">
        <header className="panel-header">
          <h3>가져오기 메모</h3>
        </header>
        {notes && notes.length > 0 ? (
          <ul className="list">
            {notes.map((note) => (
              <li key={note.id} className="note-row">
                <div>
                  <div className="cell-sub">
                    {new Date(note.createdAt).toLocaleString("ko-KR")} · 키가 탐지되지 않아 보관한
                    텍스트
                  </div>
                  <pre className="unparsed-block">{note.text}</pre>
                </div>
                <button
                  type="button"
                  className="button button-small button-danger-ghost"
                  onClick={() => void deleteNote(note.id)}
                >
                  삭제
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="empty-note">저장된 가져오기 메모가 없습니다.</p>
        )}
      </section>

      <section className="panel panel-danger">
        <header className="panel-header">
          <h3>위험 구역</h3>
        </header>
        <p className="warn-note">
          Vault를 초기화하면 암호화된 Stronghold 파일이 영구 삭제됩니다. 저장된 모든 API 키는
          복구할 수 없게 됩니다. 메타데이터 행은 남지만 비밀 값은 사라집니다.
        </p>
        <button
          type="button"
          className="button button-danger"
          onClick={() => setResetOpen(true)}
        >
          Vault 초기화
        </button>
      </section>

      {exportOpen ? (
        <Modal
          title={exportOpen === "csv" ? "키 CSV 내보내기" : "키 텍스트 내보내기"}
          onClose={() => setExportOpen(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setExportOpen(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-danger"
                disabled={!exportConfirmed || exportBusy}
                onClick={() => void runExport(exportOpen)}
              >
                {exportBusy ? "내보내는 중…" : "내보내기"}
              </button>
            </>
          }
        >
          <p>
            이 파일에는 <strong>API 키 원문</strong>이 평문으로 저장됩니다.
          </p>
          <ul className="plain-list">
            <li>암호화된 드라이브 등 안전한 위치에만 저장하세요.</li>
            <li>메신저·클라우드 동기화 폴더에 두지 마세요.</li>
            <li>Vault에 없는 키는 빈 값으로 내보내고 개수를 알려드립니다.</li>
            <li>사용 후에는 파일을 삭제하세요.</li>
          </ul>
          <label className="field field-check">
            <input
              type="checkbox"
              checked={exportConfirmed}
              onChange={(event) => setExportConfirmed(event.target.checked)}
            />
            <span>위 내용을 이해했고 평문 내보내기에 동의합니다</span>
          </label>
        </Modal>
      ) : null}

      {resetOpen ? (
        <Modal
          title="Vault 초기화"
          onClose={() => setResetOpen(false)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setResetOpen(false)}>
                취소
              </button>
              <button
                type="button"
                className="button button-danger"
                disabled={resetConfirm !== "RESET"}
                onClick={() => void confirmReset()}
              >
                Vault 영구 삭제
              </button>
            </>
          }
        >
          <p>
            암호화된 Vault 파일과 Salt를 삭제합니다. 확인하려면 <strong>RESET</strong>을
            입력하세요.
          </p>
          <input
            className="input mono"
            value={resetConfirm}
            onChange={(event) => setResetConfirm(event.target.value)}
            placeholder="RESET"
          />
        </Modal>
      ) : null}
    </div>
  );
}
