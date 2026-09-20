import { useMemo, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  createCredential,
  createProjectLink,
  deleteProject,
  deleteProjectLink,
  getProject,
  listAccounts,
  listCredentialsWithContext,
  listModels,
  listProjectLinks,
  listProviders,
  createProvider,
  createAccount,
  updateProjectLink,
  type CredentialWithContext,
  type ProjectLink,
} from "../lib/db/repo";
import { useAsyncData } from "../hooks/useAsyncData";
import { Badge } from "../components/Badges";
import { ConfirmDialog, Modal } from "../components/Modal";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";
import { buildEnvEntries } from "../lib/env/entries";
import { newId } from "../lib/format";
import { detectProviderFromEnvName } from "../lib/providers/presets";
import { secretDelete } from "../lib/vault";
import {
  envApply,
  envPreview,
  importEnvValues,
  scanProject,
  type EnvImportTarget,
  type EnvPreview,
  type ScanFile,
} from "../lib/system";

const PURPOSES = [
  "주 LLM",
  "코딩 에이전트",
  "비전",
  "TTS",
  "STT",
  "임베딩",
  "이미지",
  "폴백",
  "리서치",
  "기타",
];

interface LinkFormState {
  id: string | null;
  credential_id: string;
  model_id: string;
  purpose: string;
  priority: string;
  env_override: string;
  notes: string;
}

interface ProjectDetailPageProps {
  projectId: string;
  onBack: () => void;
  onDeleted: () => void;
}

export function ProjectDetailPage({ projectId, onBack, onDeleted }: ProjectDetailPageProps) {
  const { data, loading, error, reload } = useAsyncData(async () => {
    const [project, links, credentials, models, providers] = await Promise.all([
      getProject(projectId),
      listProjectLinks(projectId),
      listCredentialsWithContext(),
      listModels(),
      listProviders(),
    ]);
    return { project, links, credentials, models, providers };
  }, `project-${projectId}`);
  const { notify } = useToast();

  const [linkForm, setLinkForm] = useState<LinkFormState | null>(null);
  const [scan, setScan] = useState<ScanFile[] | null>(null);
  const [selectedVars, setSelectedVars] = useState<Set<string>>(new Set());
  const [scanBusy, setScanBusy] = useState(false);
  const [preview, setPreview] = useState<EnvPreview | null>(null);
  const [applyBusy, setApplyBusy] = useState(false);
  const [deleteProjectOpen, setDeleteProjectOpen] = useState(false);

  const project = data?.project ?? null;
  const credentials = useMemo(() => data?.credentials ?? [], [data]);
  const credentialById = useMemo(() => {
    const map = new Map<string, CredentialWithContext>();
    for (const credential of credentials) map.set(credential.id, credential);
    return map;
  }, [credentials]);

  const envEntries = useMemo(() => {
    if (!data) return [];
    return buildEnvEntries(
      data.links.map((link) => ({
        credential_id: link.credential_id,
        env_override: link.env_override,
      })),
      credentials.map((credential) => ({
        id: credential.id,
        env_name: credential.env_name,
        secret_id: credential.secret_id,
      })),
    );
  }, [data, credentials]);

  const registeredEnvNames = useMemo(() => {
    const names = new Set<string>();
    for (const credential of credentials) {
      if (credential.env_name) names.add(credential.env_name.toUpperCase());
    }
    return names;
  }, [credentials]);

  const modelOptions = useMemo(() => {
    if (!linkForm || !data) return [];
    const credential = credentialById.get(linkForm.credential_id);
    if (!credential) return data.models;
    return data.models.filter((model) => model.provider_id === credential.provider_id);
  }, [linkForm, data, credentialById]);

  const openLinkForm = (link: ProjectLink | null) => {
    setLinkForm({
      id: link?.id ?? null,
      credential_id: link?.credential_id ?? credentials[0]?.id ?? "",
      model_id: link?.model_id ?? "",
      purpose: link?.purpose ?? "주 LLM",
      priority: (link?.priority ?? 0).toString(),
      env_override: link?.env_override ?? "",
      notes: link?.notes ?? "",
    });
  };

  const submitLink = async () => {
    if (!linkForm) return;
    if (!linkForm.credential_id) {
      notify("연결할 인증 정보를 선택하세요", "error");
      return;
    }
    const priority = Number(linkForm.priority.trim() || "0");
    try {
      if (linkForm.id) {
        await updateProjectLink(linkForm.id, {
          model_id: linkForm.model_id || null,
          purpose: linkForm.purpose.trim(),
          priority: Number.isFinite(priority) ? priority : 0,
          env_override: linkForm.env_override.trim() || null,
          notes: linkForm.notes.trim() || null,
        });
        notify("연결이 수정되었습니다", "success");
      } else {
        await createProjectLink({
          project_id: projectId,
          credential_id: linkForm.credential_id,
          model_id: linkForm.model_id || null,
          purpose: linkForm.purpose.trim(),
          priority: Number.isFinite(priority) ? priority : 0,
          env_override: linkForm.env_override.trim() || null,
          notes: linkForm.notes.trim() || null,
        });
        notify("프로젝트에 인증 정보를 연결했습니다", "success");
      }
      setLinkForm(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const runScan = async () => {
    if (!project?.path) {
      notify("프로젝트 경로를 먼저 설정하세요", "error");
      return;
    }
    try {
      const files = await scanProject(project.path);
      setScan(files);
      const pending = new Set<string>();
      for (const file of files) {
        for (const variable of file.variables) {
          if (!registeredEnvNames.has(variable.toUpperCase())) pending.add(variable);
        }
      }
      setSelectedVars(pending);
      if (files.length === 0) {
        notify(".env 파일을 찾지 못했습니다", "info");
      }
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const importScanned = async () => {
    if (!scan || !project?.path) return;
    setScanBusy(true);
    try {
      let imported = 0;
      let skipped = 0;
      const handledVars = new Set<string>();
      for (const file of scan) {
        const targets: EnvImportTarget[] = [];
        const created: Array<{ varName: string; secretId: string }> = [];
        for (const varName of file.variables) {
          const upper = varName.toUpperCase();
          if (registeredEnvNames.has(upper) || handledVars.has(upper)) continue;
          if (!selectedVars.has(varName)) continue;
          handledVars.add(upper);
          const preset = detectProviderFromEnvName(varName);
          const provider = data?.providers.find(
            (item) => item.name === (preset?.name ?? varName.toLowerCase()),
          );
          let providerId: string;
          if (provider) {
            providerId = provider.id;
          } else {
            providerId = await createProvider({
              name: preset?.name ?? varName.toLowerCase(),
              display_name: preset?.displayName ?? `${varName} (가져옴)`,
              kind: preset?.kind ?? "custom",
              base_url: preset?.baseUrl ?? null,
              homepage_url: preset?.homepageUrl ?? null,
              docs_url: preset?.docsUrl ?? null,
              notes: "프로젝트 스캔에서 자동 생성",
            });
          }
          const accounts = await listAccounts(providerId);
          let accountId = accounts[0]?.id;
          if (!accountId) {
            accountId = await createAccount({
              provider_id: providerId,
              name: "기본",
              plan_type: "payg",
              plan_name: null,
              monthly_cost: 0,
              currency: "USD",
              billing_day: null,
              renewal_date: null,
              status: "active",
              notes: "프로젝트 스캔에서 자동 생성",
            });
          }
          const secretId = newId();
          await createCredential({
            account_id: accountId,
            name: varName,
            secret_id: secretId,
            env_name: varName,
            status: "unknown",
            expires_at: null,
            notes: `${file.fileName}에서 가져옴`,
          });
          targets.push({ varName, secretId });
          created.push({ varName, secretId });
        }
        if (targets.length > 0) {
          const results = await importEnvValues(project.path, file.fileName, targets);
          for (const result of results) {
            if (result.found) {
              imported += 1;
            } else {
              skipped += 1;
              const failed = created.find((item) => item.varName === result.varName);
              if (failed) {
                await secretDelete(failed.secretId);
              }
            }
          }
        }
      }
      notify(
        `키 ${imported}개를 가져왔습니다${skipped > 0 ? `, ${skipped}개 건너뜀` : ""}`,
        imported > 0 ? "success" : "info",
      );
      setScan(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setScanBusy(false);
    }
  };

  const openEnvPreview = async () => {
    if (!project?.path) {
      notify("프로젝트 경로를 먼저 설정하세요", "error");
      return;
    }
    if (envEntries.length === 0) {
      notify("환경변수 이름이 있는 연결된 인증 정보가 없습니다", "error");
      return;
    }
    try {
      const result = await envPreview(project.path, envEntries);
      setPreview(result);
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const applyEnv = async (mode: "merge" | "backup") => {
    if (!preview || !project?.path) return;
    setApplyBusy(true);
    try {
      const result = await envApply(project.path, envEntries, mode);
      if (result.written) {
        notify(
          `${result.filePath} 저장 완료 (+${result.added} ~${result.updated})${result.backupPath ? ` · 백업: ${result.backupPath}` : ""}`,
          "success",
        );
      } else {
        notify("변경할 내용이 없습니다 — .env가 이미 최신입니다", "info");
      }
      setPreview(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setApplyBusy(false);
    }
  };

  const removeLink = async (link: ProjectLink) => {
    try {
      await deleteProjectLink(link.id);
      notify("연결을 해제했습니다", "success");
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const removeProject = async () => {
    try {
      await deleteProject(projectId);
      notify("프로젝트가 삭제되었습니다", "success");
      onDeleted();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  if (loading && !data) return <div className="page-loading">프로젝트 불러오는 중…</div>;
  if (error) return <div className="page-error">{error}</div>;
  if (!project) return <div className="page-error">프로젝트를 찾을 수 없습니다</div>;

  return (
    <div className="page">
      <button type="button" className="link-button back-link" onClick={onBack}>
        ← 프로젝트 목록
      </button>

      <header className="detail-header">
        <div>
          <h2>{project.name}</h2>
          <p className="mono">{project.path ?? "경로 미설정"}</p>
          {project.description ? <p className="muted">{project.description}</p> : null}
        </div>
        <div className="row-actions">
          <button
            type="button"
            className="button"
            disabled={!project.path}
            onClick={() => project.path && void openPath(project.path)}
          >
            폴더 열기
          </button>
          <button type="button" className="button" onClick={() => void runScan()}>
            프로젝트 스캔
          </button>
          <button type="button" className="button button-primary" onClick={() => void openEnvPreview()}>
            .env 생성
          </button>
          <button
            type="button"
            className="button button-danger-ghost"
            onClick={() => setDeleteProjectOpen(true)}
          >
            삭제
          </button>
        </div>
      </header>

      <section className="panel">
        <header className="panel-header">
          <h3>연결된 API</h3>
          <button type="button" className="button button-small" onClick={() => openLinkForm(null)}>
            + 인증 정보 연결
          </button>
        </header>
        <table className="table">
          <thead>
            <tr>
              <th>용도</th>
              <th>프로바이더</th>
              <th>계정</th>
              <th>인증 정보</th>
              <th>모델</th>
              <th>환경변수</th>
              <th className="num">우선순위</th>
              <th aria-label="작업" />
            </tr>
          </thead>
          <tbody>
            {data?.links.map((link) => {
              const credential = credentialById.get(link.credential_id);
              return (
                <tr key={link.id}>
                  <td>
                    <Badge tone="info">{link.purpose || "—"}</Badge>
                  </td>
                  <td>{link.provider_display_name}</td>
                  <td className="muted">{link.account_name}</td>
                  <td>
                    <div className="cell-title">{link.credential_name}</div>
                    <div className="cell-sub">
                      {credential ? (
                        <span>
                          {credential.status} · {credential.last_test_status ?? "미테스트"}
                        </span>
                      ) : (
                        <span>인증 정보 없음</span>
                      )}
                    </div>
                  </td>
                  <td>{link.model_display_name ?? <span className="muted">—</span>}</td>
                  <td className="mono">{link.env_override ?? link.env_name ?? "—"}</td>
                  <td className="num">{link.priority}</td>
                  <td className="row-actions">
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() => openLinkForm(link)}
                    >
                      편집
                    </button>
                    <button
                      type="button"
                      className="button button-small button-danger-ghost"
                      onClick={() => void removeLink(link)}
                    >
                      해제
                    </button>
                  </td>
                </tr>
              );
            })}
            {data && data.links.length === 0 ? (
              <tr>
                <td colSpan={8} className="empty-note">
                  아직 연결된 인증 정보가 없습니다. 인증 정보를 연결하고 “코딩 에이전트” 같은
                  용도를 지정하세요.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </section>

      {project.notes ? (
        <section className="panel">
          <header className="panel-header">
            <h3>메모</h3>
          </header>
          <p className="preserve-lines">{project.notes}</p>
        </section>
      ) : null}

      {linkForm ? (
        <Modal
          title={linkForm.id ? "연결 편집" : "인증 정보 연결"}
          onClose={() => setLinkForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setLinkForm(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => void submitLink()}
              >
                {linkForm.id ? "저장" : "연결"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field field-wide">
              <span>인증 정보</span>
              <select
                value={linkForm.credential_id}
                onChange={(event) =>
                  setLinkForm({ ...linkForm, credential_id: event.target.value, model_id: "" })
                }
              >
                {credentials.map((credential) => (
                  <option key={credential.id} value={credential.id}>
                    {credential.provider_display_name} / {credential.account_name} /{" "}
                    {credential.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>용도</span>
              <input
                list="purpose-options"
                value={linkForm.purpose}
                onChange={(event) => setLinkForm({ ...linkForm, purpose: event.target.value })}
              />
              <datalist id="purpose-options">
                {PURPOSES.map((purpose) => (
                  <option key={purpose} value={purpose} />
                ))}
              </datalist>
            </label>
            <label className="field">
              <span>우선순위</span>
              <input
                value={linkForm.priority}
                onChange={(event) => setLinkForm({ ...linkForm, priority: event.target.value })}
              />
            </label>
            <label className="field">
              <span>모델</span>
              <select
                value={linkForm.model_id}
                onChange={(event) => setLinkForm({ ...linkForm, model_id: event.target.value })}
              >
                <option value="">특정 모델 없음</option>
                {modelOptions.map((model) => (
                  <option key={model.id} value={model.id}>
                    {model.display_name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>환경변수 재정의</span>
              <input
                className="mono"
                placeholder="비우면 인증 정보의 환경변수 이름 사용"
                value={linkForm.env_override}
                onChange={(event) =>
                  setLinkForm({ ...linkForm, env_override: event.target.value })
                }
              />
            </label>
            <label className="field field-wide">
              <span>메모</span>
              <textarea
                rows={2}
                value={linkForm.notes}
                onChange={(event) => setLinkForm({ ...linkForm, notes: event.target.value })}
              />
            </label>
          </div>
        </Modal>
      ) : null}

      {scan ? (
        <Modal
          title="프로젝트 스캔"
          subtitle={project.path ?? undefined}
          wide
          onClose={() => setScan(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setScan(null)}>
                닫기
              </button>
              <button
                type="button"
                className="button button-primary"
                disabled={scanBusy || selectedVars.size === 0}
                onClick={() => void importScanned()}
              >
                {scanBusy ? "가져오는 중…" : `미등록 Key 가져오기 (${selectedVars.size})`}
              </button>
            </>
          }
        >
          {scan.length === 0 ? (
            <p className="empty-note">프로젝트 루트에서 .env 파일을 찾지 못했습니다.</p>
          ) : (
            scan.map((file) => (
              <div key={file.fileName} className="scan-file">
                <h4 className="mono">{file.fileName}</h4>
                <table className="table table-compact">
                  <tbody>
                    {file.variables.map((variable) => {
                      const registered = registeredEnvNames.has(variable.toUpperCase());
                      return (
                        <tr key={`${file.fileName}-${variable}`}>
                          <td>
                            {!registered ? (
                              <input
                                type="checkbox"
                                checked={selectedVars.has(variable)}
                                onChange={(event) => {
                                  const next = new Set(selectedVars);
                                  if (event.target.checked) next.add(variable);
                                  else next.delete(variable);
                                  setSelectedVars(next);
                                }}
                                aria-label={variable}
                              />
                            ) : null}
                          </td>
                          <td className="mono">{variable}</td>
                          <td>
                            {registered ? (
                              <Badge tone="ok">등록됨</Badge>
                            ) : (
                              <Badge tone="warn">미등록</Badge>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                    {file.variables.length === 0 ? (
                      <tr>
                        <td className="empty-note">환경변수를 찾지 못했습니다.</td>
                      </tr>
                    ) : null}
                  </tbody>
                </table>
              </div>
            ))
          )}
          <p className="form-hint">
            스캔은 변수 이름만 읽습니다. 값은 가져오기를 누를 때만 Vault에 복사됩니다.
          </p>
        </Modal>
      ) : null}

      {preview ? (
        <Modal
          title=".env 생성"
          subtitle={preview.filePath}
          onClose={() => setPreview(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setPreview(null)}>
                취소
              </button>
              {preview.existing && preview.changed ? (
                <>
                  <button
                    type="button"
                    className="button"
                    disabled={applyBusy}
                    onClick={() => void applyEnv("merge")}
                  >
                    병합하여 쓰기
                  </button>
                  <button
                    type="button"
                    className="button button-primary"
                    disabled={applyBusy}
                    onClick={() => void applyEnv("backup")}
                  >
                    백업 후 쓰기
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={applyBusy}
                  onClick={() => void applyEnv("merge")}
                >
                  {preview.changed ? ".env 쓰기" : "닫기"}
                </button>
              )}
            </>
          }
        >
          {preview.existing ? (
            <p className="warn-note">
              기존 .env 파일이 있습니다. 승인 없이는 덮어쓰지 않습니다 — 백업 후 쓰기를 선택하면
              타임스탬프가 붙은 백업 파일이 남습니다.
            </p>
          ) : (
            <p className="empty-note">프로젝트 루트에 새 .env 파일이 생성됩니다.</p>
          )}
          <div className="diff-list mono">
            {preview.lines.map((line) => (
              <div key={`${line.action}-${line.name}`} className={`diff-line diff-${line.action}`}>
                <span className="diff-symbol">
                  {line.action === "add"
                    ? "+"
                    : line.action === "update"
                      ? "~"
                      : line.action === "missing"
                        ? "!"
                        : " "}
                </span>
                <span>{line.name}</span>
                <span className="diff-note">
                  {line.action === "unchanged"
                    ? "변경 없음"
                    : line.action === "missing"
                      ? "Vault에 비밀 값 없음"
                      : ""}
                </span>
              </div>
            ))}
          </div>
          <p className="form-hint">이 미리보기에는 키 값이 표시되지 않습니다.</p>
        </Modal>
      ) : null}

      {deleteProjectOpen ? (
        <ConfirmDialog
          title="프로젝트 삭제"
          danger
          confirmLabel="삭제"
          message={`"${project.name}" 프로젝트와 인증 정보 연결을 삭제할까요? Vault의 비밀 값은 유지됩니다.`}
          onCancel={() => setDeleteProjectOpen(false)}
          onConfirm={() => void removeProject()}
        />
      ) : null}
    </div>
  );
}
