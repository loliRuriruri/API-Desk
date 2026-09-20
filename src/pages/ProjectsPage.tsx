import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  createProject,
  deleteProject,
  listProjects,
  updateProject,
  type ProjectInput,
} from "../lib/db/repo";
import { useAsyncData } from "../hooks/useAsyncData";
import { ConfirmDialog, Modal } from "../components/Modal";
import { EmptyState } from "../components/EmptyState";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";

interface ProjectFormState {
  id: string | null;
  name: string;
  path: string;
  description: string;
  notes: string;
}

const EMPTY: ProjectFormState = {
  id: null,
  name: "",
  path: "",
  description: "",
  notes: "",
};

export function ProjectsPage({
  focusId,
  defaultProjectDir,
  onOpenProject,
}: {
  focusId: string | null;
  defaultProjectDir: string;
  onOpenProject: (projectId: string) => void;
}) {
  const { data, loading, error, reload } = useAsyncData(() => listProjects(), "projects");
  const { notify } = useToast();
  const [form, setForm] = useState<ProjectFormState | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);

  const browse = async () => {
    if (!form) return;
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "프로젝트 폴더 선택",
      defaultPath: form.path || defaultProjectDir || undefined,
    });
    if (typeof selected === "string" && selected.length > 0) {
      const folderName = selected.split(/[\\/]/).filter(Boolean).pop() ?? "";
      setForm({
        ...form,
        path: selected,
        name: form.name.trim().length > 0 ? form.name : folderName,
      });
    }
  };

  const submit = async () => {
    if (!form) return;
    if (form.name.trim().length === 0) {
      notify("프로젝트 이름은 필수입니다", "error");
      return;
    }
    const payload: ProjectInput = {
      name: form.name.trim(),
      path: form.path.trim() || null,
      description: form.description.trim() || null,
      notes: form.notes.trim() || null,
    };
    try {
      if (form.id) {
        await updateProject(form.id, payload);
        notify("프로젝트가 수정되었습니다", "success");
      } else {
        const id = await createProject(payload);
        notify("프로젝트가 생성되었습니다", "success");
        onOpenProject(id);
      }
      setForm(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteProject(deleteTarget.id);
      notify("프로젝트가 삭제되었습니다", "success");
      setDeleteTarget(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  return (
    <div className="page">
      <div className="page-toolbar">
        <p className="toolbar-note">
          프로젝트별로 어떤 API 인증 정보와 모델을 어떤 용도(Coding Agent, Vision, TTS,
          Fallback…)로 쓰는지 매핑합니다.
        </p>
        <button type="button" className="button button-primary" onClick={() => setForm(EMPTY)}>
          + 새 프로젝트
        </button>
      </div>

      {loading && !data ? <div className="page-loading">프로젝트 불러오는 중…</div> : null}
      {error ? <div className="page-error">{error}</div> : null}

      <div className="project-grid">
        {data?.map((project) => (
          <div
            key={project.id}
            className={`project-card ${focusId === project.id ? "project-card-focus" : ""}`}
          >
            <button
              type="button"
              className="project-card-main"
              onClick={() => onOpenProject(project.id)}
            >
              <strong>{project.name}</strong>
              <span className="mono project-path">{project.path ?? "경로 미설정"}</span>
              <span className="muted">연결된 인증 정보 {project.link_count}개</span>
              {project.description ? (
                <span className="project-description">{project.description}</span>
              ) : null}
            </button>
            <div className="row-actions">
              <button
                type="button"
                className="button button-small"
                onClick={() =>
                  setForm({
                    id: project.id,
                    name: project.name,
                    path: project.path ?? "",
                    description: project.description ?? "",
                    notes: project.notes ?? "",
                  })
                }
              >
                편집
              </button>
              <button
                type="button"
                className="button button-small button-danger-ghost"
                onClick={() => setDeleteTarget({ id: project.id, name: project.name })}
              >
                삭제
              </button>
            </div>
          </div>
        ))}
        {data && data.length === 0 ? (
          <EmptyState
            text="아직 프로젝트가 없습니다"
            hint="API 키를 사용하는 저장소마다 하나씩 만드세요"
          />
        ) : null}
      </div>

      {form ? (
        <Modal
          title={form.id ? "프로젝트 편집" : "새 프로젝트"}
          onClose={() => setForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setForm(null)}>
                취소
              </button>
              <button type="button" className="button button-primary" onClick={() => void submit()}>
                {form.id ? "저장" : "생성"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>이름</span>
              <input
                value={form.name}
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </label>
            <label className="field field-wide">
              <span>경로</span>
              <div className="field-row">
                <input
                  className="mono"
                  value={form.path}
                  onChange={(event) => setForm({ ...form, path: event.target.value })}
                />
                <button type="button" className="button" onClick={() => void browse()}>
                  찾아보기…
                </button>
              </div>
            </label>
            <label className="field field-wide">
              <span>설명</span>
              <input
                value={form.description}
                onChange={(event) => setForm({ ...form, description: event.target.value })}
              />
            </label>
            <label className="field field-wide">
              <span>메모</span>
              <textarea
                rows={3}
                value={form.notes}
                onChange={(event) => setForm({ ...form, notes: event.target.value })}
              />
            </label>
          </div>
        </Modal>
      ) : null}

      {deleteTarget ? (
        <ConfirmDialog
          title="프로젝트 삭제"
          danger
          confirmLabel="삭제"
          message={`"${deleteTarget.name}" 프로젝트와 연결된 인증 정보 매핑을 삭제할까요?`}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => void confirmDelete()}
        />
      ) : null}
    </div>
  );
}
