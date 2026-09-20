import { useMemo, useState } from "react";
import {
  createModel,
  deleteModel,
  listAllCredentialFields,
  listCredentialsWithContext,
  listModels,
  listMonitorSnapshots,
  listProviders,
  listUsageSnapshots,
  updateModel,
  upsertUsageSnapshot,
  type ModelInput,
} from "../lib/db/repo";
import { useAsyncData } from "../hooks/useAsyncData";
import { ConfirmDialog, Modal } from "../components/Modal";
import { Badge } from "../components/Badges";
import { EmptyState } from "../components/EmptyState";
import { GaugeBar } from "../components/Gauge";
import { MonitorCard } from "../components/MonitorCard";
import { SummaryChips } from "../components/SummaryChips";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";
import { formatDateTime, formatRelativeTime } from "../lib/format";
import { credentialRemainingPercent } from "../lib/alerts";
import { persistMonitorOutcome } from "../lib/monitorStore";
import {
  usageAdapterFor,
  usageExtraSecretFor,
  usageStatusLabel,
} from "../lib/providers/usage";
import {
  antigravityAccounts,
  antigravityDeleteAccount,
  antigravitySwitchAccount,
  fetchModels,
  fetchUsage,
  monitorProbe,
  monitorRefresh,
  type AntigravityAccount,
  type ImportedModel,
} from "../lib/system";
import type { ModelCategory } from "../types/domain";

const CATEGORIES: ModelCategory[] = [
  "LLM",
  "Coding",
  "Vision",
  "Embedding",
  "Image",
  "TTS",
  "STT",
  "Other",
];

interface ModelFormState {
  id: string | null;
  provider_id: string;
  name: string;
  display_name: string;
  category: ModelCategory;
  context_length: string;
  input_price: string;
  output_price: string;
  notes: string;
  is_active: boolean;
}

const EMPTY_FORM: ModelFormState = {
  id: null,
  provider_id: "",
  name: "",
  display_name: "",
  category: "LLM",
  context_length: "",
  input_price: "",
  output_price: "",
  notes: "",
  is_active: true,
};

function toNumberOrNull(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed.length === 0) return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

export function ModelsPage() {
  const { data, loading, error, reload } = useAsyncData(async () => {
    const [models, providers, credentials, snapshots, fields, probes, monitors, agAccounts] =
      await Promise.all([
        listModels(),
        listProviders(),
        listCredentialsWithContext(),
        listUsageSnapshots(),
        listAllCredentialFields(),
        monitorProbe(),
        listMonitorSnapshots(),
        antigravityAccounts(),
      ]);
    return {
      models,
      providers,
      credentials,
      snapshots,
      fields,
      probes,
      monitors,
      agAccounts,
    };
  }, "models");
  const { notify } = useToast();
  const [filter, setFilter] = useState("");
  const [form, setForm] = useState<ModelFormState | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [usageBusy, setUsageBusy] = useState<string | null>(null);
  const [monitorBusy, setMonitorBusy] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [importProviderId, setImportProviderId] = useState("");
  const [importCredentialId, setImportCredentialId] = useState("");
  const [importBusy, setImportBusy] = useState(false);
  const [imported, setImported] = useState<ImportedModel[]>([]);
  const [importSelected, setImportSelected] = useState<Set<string>>(new Set());

  const filtered = useMemo(() => {
    if (!data) return [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return data.models;
    return data.models.filter((model) =>
      [model.name, model.display_name, model.provider_display_name, model.category]
        .join(" ")
        .toLowerCase()
        .includes(needle),
    );
  }, [data, filter]);

  const usageRows = useMemo(() => {
    if (!data) return [];
    const snapshotByCredential = new Map(
      data.snapshots.map((snapshot) => [snapshot.credential_id, snapshot]),
    );
    return data.credentials
      .map((credential) => ({
        credential,
        adapter: usageAdapterFor(
          credential.provider_name,
          credential.provider_display_name,
          credential.provider_base_url,
        ),
        snapshot: snapshotByCredential.get(credential.id) ?? null,
      }))
      .filter((row) => row.adapter !== null);
  }, [data]);

  const monitorByKey = useMemo(() => {
    const map = new Map(data?.monitors.map((snapshot) => [snapshot.monitor, snapshot]) ?? []);
    return map;
  }, [data]);

  const importCredentials = useMemo(
    () =>
      data
        ? data.credentials.filter((credential) => credential.provider_id === importProviderId)
        : [],
    [data, importProviderId],
  );

  const openCreate = () => {
    setForm({
      ...EMPTY_FORM,
      provider_id: data?.providers[0]?.id ?? "",
    });
  };

  const submit = async () => {
    if (!form) return;
    if (!form.provider_id) {
      notify("프로바이더를 먼저 선택하세요", "error");
      return;
    }
    if (form.name.trim().length === 0) {
      notify("모델 이름은 필수입니다", "error");
      return;
    }
    const payload: ModelInput = {
      provider_id: form.provider_id,
      name: form.name.trim(),
      display_name: form.display_name.trim() || form.name.trim(),
      category: form.category,
      context_length: toNumberOrNull(form.context_length),
      input_price: toNumberOrNull(form.input_price),
      output_price: toNumberOrNull(form.output_price),
      notes: form.notes.trim() || null,
      is_active: form.is_active ? 1 : 0,
    };
    try {
      if (form.id) {
        await updateModel(form.id, payload);
        notify("모델이 수정되었습니다", "success");
      } else {
        await createModel(payload);
        notify("모델이 추가되었습니다", "success");
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
      await deleteModel(deleteTarget);
      notify("모델이 삭제되었습니다", "success");
      setDeleteTarget(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const toggleActive = async (id: string, isActive: boolean) => {
    try {
      await updateModel(id, { is_active: isActive ? 0 : 1 });
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const refreshUsage = async (credentialId: string): Promise<boolean> => {
    if (!data) return false;
    const row = usageRows.find((item) => item.credential.id === credentialId);
    if (!row || !row.adapter) return false;
    setUsageBusy(credentialId);
    try {
      const fields = data.fields.filter((field) => field.credential_id === credentialId);
      const outcome = await fetchUsage(
        row.adapter,
        row.credential.provider_base_url,
        row.credential.secret_id,
        usageExtraSecretFor(row.adapter, fields),
      );
      await upsertUsageSnapshot({
        credential_id: credentialId,
        adapter: row.adapter,
        status: outcome.status,
        summary: outcome.summary || outcome.message || "",
        used: outcome.used,
        limit_total: outcome.limit,
        remaining: outcome.remaining,
        currency: outcome.currency,
        details_json: outcome.details ? JSON.stringify(outcome.details) : null,
        fetched_at: new Date().toISOString(),
      });
      return true;
    } catch (err) {
      notify(`${row.credential.name}: ${errorMessage(err)}`, "error");
      return false;
    } finally {
      setUsageBusy(null);
    }
  };

  const refreshAllUsage = async () => {
    let ok = 0;
    for (const row of usageRows) {
      const success = await refreshUsage(row.credential.id);
      if (success) ok += 1;
    }
    notify(
      ok > 0 ? `사용량 ${ok}건을 갱신했습니다` : "갱신된 사용량이 없습니다",
      ok > 0 ? "success" : "info",
    );
    reload();
  };

  const refreshOne = async (credentialId: string) => {
    const success = await refreshUsage(credentialId);
    if (success) {
      notify("사용량을 갱신했습니다", "success");
      reload();
    }
  };

  const refreshMonitor = async (monitor: string): Promise<boolean> => {
    setMonitorBusy(monitor);
    try {
      const outcome = await monitorRefresh(monitor);
      await persistMonitorOutcome(outcome);
      return true;
    } catch (err) {
      notify(errorMessage(err), "error");
      return false;
    } finally {
      setMonitorBusy(null);
    }
  };

  const refreshOneMonitor = async (monitor: string) => {
    const success = await refreshMonitor(monitor);
    if (success) {
      notify("모니터를 새로고침했습니다", "success");
      reload();
    }
  };

  const switchAgAccount = async (account: AntigravityAccount) => {
    setMonitorBusy("antigravity-account");
    try {
      await antigravitySwitchAccount(account.id);
      notify(
        `${account.email ?? account.label} 계정으로 전환했습니다. Antigravity 재시작 후 ↻로 갱신하세요`,
        "success",
      );
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setMonitorBusy(null);
    }
  };

  const deleteAgAccount = async (account: AntigravityAccount) => {
    setMonitorBusy("antigravity-account");
    try {
      await antigravityDeleteAccount(account.id);
      notify("저장된 계정을 삭제했습니다", "success");
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setMonitorBusy(null);
    }
  };

  const refreshAllMonitors = async () => {
    if (!data) return;
    let ok = 0;
    for (const probe of data.probes) {
      if (!probe.available) continue;
      const success = await refreshMonitor(probe.monitor);
      if (success) ok += 1;
    }
    notify(
      ok > 0 ? `모니터 ${ok}건을 갱신했습니다` : "갱신할 수 있는 모니터가 없습니다",
      ok > 0 ? "success" : "info",
    );
    reload();
  };

  const openImport = () => {
    if (!data) return;
    const providerWithCredentials =
      data.providers.find((provider) =>
        data.credentials.some((credential) => credential.provider_id === provider.id),
      ) ?? data.providers[0];
    const providerId = providerWithCredentials?.id ?? "";
    const credential = data.credentials.find((item) => item.provider_id === providerId);
    setImportProviderId(providerId);
    setImportCredentialId(credential?.id ?? "");
    setImported([]);
    setImportSelected(new Set());
    setImportOpen(true);
  };

  const runImportFetch = async () => {
    if (!data) return;
    const credential = data.credentials.find((item) => item.id === importCredentialId);
    if (!credential) {
      notify("인증 정보를 선택하세요", "error");
      return;
    }
    setImportBusy(true);
    try {
      const models = await fetchModels(
        credential.provider_kind,
        credential.provider_base_url,
        credential.secret_id,
      );
      setImported(models);
      setImportSelected(new Set(models.map((model) => model.id)));
      notify(`모델 ${models.length}개를 가져왔습니다`, models.length > 0 ? "success" : "info");
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setImportBusy(false);
    }
  };

  const submitImport = async () => {
    if (!data) return;
    const existing = new Set(
      data.models
        .filter((model) => model.provider_id === importProviderId)
        .map((model) => model.name),
    );
    const chosen = imported.filter(
      (model) => importSelected.has(model.id) && !existing.has(model.id),
    );
    if (chosen.length === 0) {
      notify("추가할 새 모델이 없습니다", "info");
      return;
    }
    setImportBusy(true);
    let added = 0;
    try {
      for (const model of chosen) {
        await createModel({
          provider_id: importProviderId,
          name: model.id,
          display_name: model.displayName || model.id,
          category: "LLM",
          context_length: null,
          input_price: null,
          output_price: null,
          notes: model.ownedBy ? `가져옴 · ${model.ownedBy}` : "프로바이더에서 가져옴",
          is_active: 1,
        });
        added += 1;
      }
      notify(`모델 ${added}개를 추가했습니다`, "success");
      setImportOpen(false);
      setImported([]);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setImportBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="page-toolbar">
        <input
          className="input"
          placeholder="모델 검색…"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <div className="row-actions">
          <button type="button" className="button" onClick={openImport}>
            프로바이더에서 가져오기
          </button>
          <button type="button" className="button button-primary" onClick={openCreate}>
            + 모델 추가
          </button>
        </div>
      </div>

      {loading && !data ? <div className="page-loading">모델 불러오는 중…</div> : null}
      {error ? <div className="page-error">{error}</div> : null}

      {data ? (
        <section className="panel">
          <header className="panel-header">
            <h3>계정 쿼터 · 모니터링</h3>
            <button
              type="button"
              className="button button-small"
              disabled={monitorBusy !== null}
              onClick={() => void refreshAllMonitors()}
            >
              {monitorBusy !== null ? "갱신 중…" : "모두 새로고침"}
            </button>
          </header>
          <div className="monitor-grid">
            {data.probes.map((probe) => (
              <MonitorCard
                key={probe.monitor}
                monitor={probe.monitor}
                label={probe.label}
                available={probe.available}
                detail={probe.detail}
                snapshot={monitorByKey.get(probe.monitor) ?? null}
                busy={monitorBusy === probe.monitor}
                onRefresh={() => void refreshOneMonitor(probe.monitor)}
                accounts={data.agAccounts}
                allSnapshots={data.monitors}
                showModels
                accountBusy={monitorBusy === "antigravity-account"}
                onSwitchAccount={(account) => void switchAgAccount(account)}
                onDeleteAccount={(account) => void deleteAgAccount(account)}
              />
            ))}
          </div>
        </section>
      ) : null}

      {data ? (
        <section className="panel">
          <header className="panel-header">
            <h3>API 사용량 · 잔액</h3>
            <button
              type="button"
              className="button button-small"
              disabled={usageBusy !== null || usageRows.length === 0}
              onClick={() => void refreshAllUsage()}
            >
              {usageBusy !== null ? "갱신 중…" : "모두 새로고침"}
            </button>
          </header>
          {usageRows.length === 0 ? (
            <EmptyState
              text="사용량 조회를 지원하는 인증 정보가 없습니다"
              hint="OpenRouter · DeepSeek · Tavily · OpenAI(Admin) · Anthropic(Admin) · xAI(Management) 키를 등록하세요"
            />
          ) : (
            <table className="table table-compact">
              <thead>
                <tr>
                  <th>프로바이더</th>
                  <th>인증 정보</th>
                  <th>사용량</th>
                  <th className="num">남은 %</th>
                  <th>상태</th>
                  <th>조회 시각</th>
                  <th aria-label="작업" />
                </tr>
              </thead>
              <tbody>
                {usageRows.map(({ credential, snapshot }) => {
                  const remainingPercent = credentialRemainingPercent(
                    snapshot?.remaining ?? null,
                    snapshot?.limit_total ?? null,
                  );
                  return (
                    <tr key={credential.id}>
                      <td>{credential.provider_display_name}</td>
                      <td>{credential.name}</td>
                      <td className="mono">
                        <SummaryChips text={snapshot?.summary || "미조회"} />
                      </td>
                      <td className="num usage-gauge-cell">
                        {remainingPercent !== null ? (
                          <span className="usage-gauge">
                            <GaugeBar percent={remainingPercent} mode="remaining" />
                            <span className="mono">{Math.round(remainingPercent)}%</span>
                          </span>
                        ) : (
                          <span className="muted">—</span>
                        )}
                      </td>
                      <td>
                        {snapshot ? (
                          <Badge
                            tone={
                              snapshot.status === "ok"
                                ? "ok"
                                : snapshot.status === "unsupported"
                                  ? "muted"
                                  : snapshot.status === "auth_required"
                                    ? "warn"
                                    : "danger"
                            }
                          >
                            {usageStatusLabel(snapshot.status)}
                          </Badge>
                        ) : (
                          <Badge tone="muted">미조회</Badge>
                        )}
                      </td>
                      <td className="muted">
                        {snapshot ? (
                          <span
                            className="stamp"
                            title={`최근 조회 ${formatDateTime(snapshot.fetched_at)}`}
                          >
                            {formatRelativeTime(snapshot.fetched_at)}
                          </span>
                        ) : (
                          "?"
                        )}
                      </td>
                      <td className="row-actions">
                        <button
                          type="button"
                          className="button button-small"
                          disabled={usageBusy === credential.id}
                          onClick={() => void refreshOne(credential.id)}
                        >
                          {usageBusy === credential.id ? "조회 중…" : "조회"}
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
          <p className="form-hint">
            조회는 버튼을 누를 때만 실행되며 결과는 로컬 DB에 저장됩니다. API 키는 Vault에서
            Rust 백엔드만 사용합니다.
          </p>
        </section>
      ) : null}

      {data ? (
        <section className="panel">
          <header className="panel-header">
            <h3>모델 카탈로그 ({data.models.length})</h3>
          </header>
          <table className="table">
            <thead>
              <tr>
                <th>프로바이더</th>
                <th>모델</th>
                <th>분류</th>
                <th className="num">컨텍스트</th>
                <th className="num">입력 $/M</th>
                <th className="num">출력 $/M</th>
                <th>활성</th>
                <th aria-label="작업" />
              </tr>
            </thead>
            <tbody>
              {filtered.map((model) => (
                <tr key={model.id}>
                  <td>{model.provider_display_name}</td>
                  <td>
                    <div className="cell-title">{model.display_name}</div>
                    <div className="cell-sub mono">{model.name}</div>
                  </td>
                  <td>{model.category}</td>
                  <td className="num">
                    {model.context_length ? model.context_length.toLocaleString() : "—"}
                  </td>
                  <td className="num">{model.input_price ?? "—"}</td>
                  <td className="num">{model.output_price ?? "—"}</td>
                  <td>
                    <input
                      type="checkbox"
                      checked={model.is_active === 1}
                      onChange={() => void toggleActive(model.id, model.is_active === 1)}
                      aria-label={`${model.display_name} 활성 전환`}
                    />
                  </td>
                  <td className="row-actions">
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() => {
                        setForm({
                          id: model.id,
                          provider_id: model.provider_id,
                          name: model.name,
                          display_name: model.display_name,
                          category: model.category,
                          context_length: model.context_length?.toString() ?? "",
                          input_price: model.input_price?.toString() ?? "",
                          output_price: model.output_price?.toString() ?? "",
                          notes: model.notes ?? "",
                          is_active: model.is_active === 1,
                        });
                      }}
                    >
                      편집
                    </button>
                    <button
                      type="button"
                      className="button button-small button-danger-ghost"
                      onClick={() => setDeleteTarget(model.id)}
                    >
                      삭제
                    </button>
                  </td>
                </tr>
              ))}
              {filtered.length === 0 ? (
                <tr>
                  <td colSpan={8}>
                    <EmptyState
                      text="아직 모델이 없습니다"
                      hint="직접 추가하거나 상단의 “프로바이더에서 가져오기”로 모델 목록을 불러오세요"
                    />
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </section>
      ) : null}

      {form ? (
        <Modal
          title={form.id ? "모델 편집" : "모델 추가"}
          onClose={() => setForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setForm(null)}>
                취소
              </button>
              <button type="button" className="button button-primary" onClick={() => void submit()}>
                {form.id ? "저장" : "추가"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>프로바이더</span>
              <select
                value={form.provider_id}
                onChange={(event) => setForm({ ...form, provider_id: event.target.value })}
              >
                {data?.providers.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.display_name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>분류</span>
              <select
                value={form.category}
                onChange={(event) =>
                  setForm({ ...form, category: event.target.value as ModelCategory })
                }
              >
                {CATEGORIES.map((category) => (
                  <option key={category} value={category}>
                    {category}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>모델 ID</span>
              <input
                className="mono"
                placeholder="예: gpt-4o-mini"
                value={form.name}
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </label>
            <label className="field">
              <span>표시 이름</span>
              <input
                value={form.display_name}
                onChange={(event) => setForm({ ...form, display_name: event.target.value })}
              />
            </label>
            <label className="field">
              <span>컨텍스트 길이</span>
              <input
                value={form.context_length}
                onChange={(event) => setForm({ ...form, context_length: event.target.value })}
              />
            </label>
            <label className="field">
              <span>입력 가격 ($/1M)</span>
              <input
                value={form.input_price}
                onChange={(event) => setForm({ ...form, input_price: event.target.value })}
              />
            </label>
            <label className="field">
              <span>출력 가격 ($/1M)</span>
              <input
                value={form.output_price}
                onChange={(event) => setForm({ ...form, output_price: event.target.value })}
              />
            </label>
            <label className="field field-check">
              <input
                type="checkbox"
                checked={form.is_active}
                onChange={(event) => setForm({ ...form, is_active: event.target.checked })}
              />
              <span>활성</span>
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

      {importOpen ? (
        <Modal
          title="프로바이더에서 모델 가져오기"
          subtitle="OpenAI 호환 /models, Anthropic, Google 모델 목록을 읽어 카탈로그에 추가합니다"
          wide
          onClose={() => setImportOpen(false)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setImportOpen(false)}>
                취소
              </button>
              <button
                type="button"
                className="button"
                disabled={importBusy}
                onClick={() => void runImportFetch()}
              >
                {importBusy ? "가져오는 중…" : "목록 불러오기"}
              </button>
              <button
                type="button"
                className="button button-primary"
                disabled={importBusy || imported.length === 0}
                onClick={() => void submitImport()}
              >
                선택 추가 ({importSelected.size})
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>프로바이더</span>
              <select
                value={importProviderId}
                onChange={(event) => {
                  const id = event.target.value;
                  setImportProviderId(id);
                  const credential = data?.credentials.find(
                    (item) => item.provider_id === id,
                  );
                  setImportCredentialId(credential?.id ?? "");
                  setImported([]);
                  setImportSelected(new Set());
                }}
              >
                {data?.providers.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.display_name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>인증 정보</span>
              <select
                value={importCredentialId}
                onChange={(event) => setImportCredentialId(event.target.value)}
              >
                {importCredentials.length === 0 ? (
                  <option value="">(인증 정보 없음)</option>
                ) : null}
                {importCredentials.map((credential) => (
                  <option key={credential.id} value={credential.id}>
                    {credential.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {imported.length > 0 ? (
            <div className="import-list">
              {imported.map((model) => (
                <label key={model.id} className="import-row">
                  <input
                    type="checkbox"
                    checked={importSelected.has(model.id)}
                    onChange={(event) => {
                      const next = new Set(importSelected);
                      if (event.target.checked) next.add(model.id);
                      else next.delete(model.id);
                      setImportSelected(next);
                    }}
                  />
                  <span className="mono">{model.id}</span>
                  <span className="muted">
                    {model.displayName !== model.id ? model.displayName : ""}
                  </span>
                </label>
              ))}
            </div>
          ) : (
            <p className="form-hint">
              “목록 불러오기”를 누르면 프로바이더의 모델 목록을 가져옵니다. API 키는 Rust
              백엔드에서만 사용됩니다.
            </p>
          )}
          <p className="form-hint">이미 등록된 모델 이름은 건너뜁니다.</p>
        </Modal>
      ) : null}

      {deleteTarget ? (
        <ConfirmDialog
          title="모델 삭제"
          danger
          confirmLabel="삭제"
          message="이 모델을 목록에서 삭제할까요? 참조하던 프로젝트 연결은 해제됩니다."
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => void confirmDelete()}
        />
      ) : null}
    </div>
  );
}
