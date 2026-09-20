import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  createAccount,
  createCredential,
  createCredentialField,
  createProvider,
  deleteAccount,
  deleteCredential,
  deleteCredentialField,
  deleteProvider,
  listAccounts,
  listAllCredentialFields,
  listCredentialsWithContext,
  listProviders,
  listUsageSnapshots,
  recordTestResult,
  updateAccount,
  updateCredential,
  updateCredentialField,
  updateProvider,
  upsertUsageSnapshot,
  type AccountInput,
  type CredentialField,
  type CredentialWithContext,
  type ProviderInput,
  type UsageSnapshot,
} from "../lib/db/repo";
import { useAsyncData } from "../hooks/useAsyncData";
import { AccountStatusBadge, Badge, CredentialStatusBadge, TestStatusBadge } from "../components/Badges";
import { ConfirmDialog, Modal } from "../components/Modal";
import { EmptyState } from "../components/EmptyState";
import { SecretActions } from "../components/SecretActions";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";
import { formatDate, formatDateTime, formatMoney, newId } from "../lib/format";
import { testStatusLabel } from "../lib/labels";
import { buildTestRequest, isTestSuccess } from "../lib/providers/adapters";
import { CUSTOM_PRESET, lookupProviderInfo, PROVIDER_PRESETS } from "../lib/providers/presets";
import { usageAdapterFor, usageExtraSecretFor, usageStatusLabel } from "../lib/providers/usage";
import { secretDelete, secretMasks, secretStore } from "../lib/vault";
import { fetchUsage, testHttp } from "../lib/system";
import type {
  AccountStatus,
  CredentialStatus,
  PlanType,
  ProviderKind,
} from "../types/domain";

type DeleteTarget =
  | { kind: "provider"; id: string; label: string }
  | { kind: "account"; id: string; label: string }
  | { kind: "credential"; id: string; label: string; secretId: string }
  | { kind: "field"; id: string; label: string; secretId: string };

interface ProviderFormState {
  id: string | null;
  preset: string;
  name: string;
  display_name: string;
  kind: ProviderKind;
  base_url: string;
  homepage_url: string;
  docs_url: string;
  notes: string;
}

interface AccountFormState {
  id: string | null;
  name: string;
  plan_type: PlanType;
  plan_name: string;
  monthly_cost: string;
  currency: string;
  billing_day: string;
  renewal_date: string;
  status: AccountStatus;
  notes: string;
}

interface CredentialFormState {
  id: string | null;
  name: string;
  env_name: string;
  status: CredentialStatus;
  expires_at: string;
  notes: string;
  secret: string;
  extraFields: Array<{ label: string; env_name: string; value: string }>;
  primaryLabel: string;
}

interface FieldFormState {
  credentialId: string;
  id: string | null;
  label: string;
  env_name: string;
  value: string;
}

const PLAN_TYPES: PlanType[] = ["free", "subscription", "payg", "credits", "custom"];
const ACCOUNT_STATUSES: AccountStatus[] = ["active", "inactive", "unknown", "expired"];
const CREDENTIAL_STATUSES: CredentialStatus[] = [
  "active",
  "inactive",
  "expired",
  "error",
  "unknown",
];

const emptyProviderForm = (): ProviderFormState => ({
  id: null,
  preset: "",
  name: "",
  display_name: "",
  kind: "openai-compatible",
  base_url: "",
  homepage_url: "",
  docs_url: "",
  notes: "",
});

export interface ApisFocus {
  kind: string;
  id: string;
}

export function ApisPage({
  focus,
  autoHideSecs,
  clipboardClearSecs,
}: {
  focus: ApisFocus | null;
  autoHideSecs: number;
  clipboardClearSecs: number;
}) {
  const { data, loading, error, reload } = useAsyncData(async () => {
    const [providers, accounts, credentials, fields, snapshots] = await Promise.all([
      listProviders(),
      listAccounts(),
      listCredentialsWithContext(),
      listAllCredentialFields(),
      listUsageSnapshots(),
    ]);
    return { providers, accounts, credentials, fields, snapshots };
  }, "apis");
  const { notify } = useToast();

  const [providerIdState, setProviderIdState] = useState<string | null>(null);
  const [accountIdState, setAccountIdState] = useState<string | null>(null);
  const [expandedState, setExpandedState] = useState<string | null>(null);
  const [maskState, setMaskState] = useState<{
    key: string;
    masks: Record<string, string>;
  } | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [usageBusy, setUsageBusy] = useState<string | null>(null);

  const [providerForm, setProviderForm] = useState<ProviderFormState | null>(null);
  const [accountForm, setAccountForm] = useState<AccountFormState | null>(null);
  const [credentialForm, setCredentialForm] = useState<CredentialFormState | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null);
  const [fieldForm, setFieldForm] = useState<FieldFormState | null>(null);
  const [fillBusy, setFillBusy] = useState(false);

  const fillProviderFromCatalog = () => {
    if (!providerForm) return;
    const info =
      lookupProviderInfo(providerForm.display_name) ?? lookupProviderInfo(providerForm.name);
    if (!info) {
      notify("카탈로그에서 이 프로바이더를 찾지 못했습니다", "info");
      return;
    }
    setProviderForm({
      ...providerForm,
      name: providerForm.name.trim() || info.name,
      display_name: providerForm.display_name.trim() || info.displayName,
      kind: info.kind,
      base_url: info.baseUrl,
      homepage_url: info.homepageUrl,
      docs_url: info.docsUrl,
    });
    notify("프로바이더 정보를 채웠습니다 — 저장을 눌러 적용하세요", "success");
  };

  const fillCustomProviders = async () => {
    if (!data) return;
    const targets = data.providers.filter(
      (provider) =>
        provider.kind === "custom" || !provider.base_url || provider.base_url.trim().length === 0,
    );
    if (targets.length === 0) {
      notify("채울 커스텀 프로바이더가 없습니다", "info");
      return;
    }
    setFillBusy(true);
    let filled = 0;
    const unmatched: string[] = [];
    try {
      for (const provider of targets) {
        const info =
          lookupProviderInfo(provider.display_name) ?? lookupProviderInfo(provider.name);
        if (!info) {
          unmatched.push(provider.display_name);
          continue;
        }
        await updateProvider(provider.id, {
          kind: info.kind,
          base_url: provider.base_url?.trim() ? provider.base_url : info.baseUrl,
          homepage_url: provider.homepage_url?.trim() ? provider.homepage_url : info.homepageUrl,
          docs_url: provider.docs_url?.trim() ? provider.docs_url : info.docsUrl,
        });
        filled += 1;
      }
      notify(
        filled > 0
          ? `프로바이더 ${filled}개를 채웠습니다${unmatched.length > 0 ? ` · 미확인: ${unmatched.join(", ")}` : ""}`
          : `카탈로그에서 일치하는 프로바이더가 없습니다: ${unmatched.join(", ")}`,
        filled > 0 ? "success" : "info",
      );
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setFillBusy(false);
    }
  };

  const focusTarget = useMemo(() => {
    if (!focus || !data) return null;
    if (focus.kind === "provider") {
      return { providerId: focus.id, accountId: null, credentialId: null };
    }
    if (focus.kind === "account") {
      const account = data.accounts.find((item) => item.id === focus.id);
      return account
        ? { providerId: account.provider_id, accountId: account.id, credentialId: null }
        : null;
    }
    if (focus.kind === "credential") {
      const credential = data.credentials.find((item) => item.id === focus.id);
      return credential
        ? {
            providerId: credential.provider_id,
            accountId: credential.account_id,
            credentialId: credential.id,
          }
        : null;
    }
    return null;
  }, [focus, data]);

  const providerId = useMemo(() => {
    if (data && providerIdState && data.providers.some((item) => item.id === providerIdState)) {
      return providerIdState;
    }
    if (focusTarget) return focusTarget.providerId;
    return data?.providers[0]?.id ?? null;
  }, [data, providerIdState, focusTarget]);

  const providerAccounts = useMemo(
    () => (data ? data.accounts.filter((account) => account.provider_id === providerId) : []),
    [data, providerId],
  );

  const accountId = useMemo(() => {
    if (accountIdState && providerAccounts.some((account) => account.id === accountIdState)) {
      return accountIdState;
    }
    if (focusTarget && focusTarget.providerId === providerId && focusTarget.accountId) {
      return focusTarget.accountId;
    }
    return providerAccounts[0]?.id ?? null;
  }, [accountIdState, providerAccounts, focusTarget, providerId]);

  const expandedCredentialId = expandedState ?? focusTarget?.credentialId ?? null;

  const selectedProvider = useMemo(
    () => data?.providers.find((provider) => provider.id === providerId) ?? null,
    [data, providerId],
  );
  const selectedAccount = useMemo(
    () => providerAccounts.find((account) => account.id === accountId) ?? null,
    [providerAccounts, accountId],
  );
  const accountCredentials = useMemo(
    () => (data ? data.credentials.filter((credential) => credential.account_id === accountId) : []),
    [data, accountId],
  );
  const fieldsByCredential = useMemo(() => {
    const map = new Map<string, CredentialField[]>();
    for (const field of data?.fields ?? []) {
      const list = map.get(field.credential_id) ?? [];
      list.push(field);
      map.set(field.credential_id, list);
    }
    return map;
  }, [data]);
  const fieldsFor = (credentialId: string): CredentialField[] =>
    fieldsByCredential.get(credentialId) ?? [];
  const snapshotByCredential = useMemo(() => {
    const map = new Map<string, UsageSnapshot>();
    for (const snapshot of data?.snapshots ?? []) {
      map.set(snapshot.credential_id, snapshot);
    }
    return map;
  }, [data]);
  const credentialIdKey = accountCredentials
    .flatMap((credential) => {
      const fields = fieldsFor(credential.id);
      return fields.length > 0
        ? fields.map((field) => field.secret_id)
        : [credential.secret_id];
    })
    .join(",");

  useEffect(() => {
    const secretIds = credentialIdKey.length > 0 ? credentialIdKey.split(",") : [];
    if (secretIds.length === 0) return;
    let active = true;
    secretMasks(secretIds)
      .then((value) => {
        if (active) setMaskState({ key: credentialIdKey, masks: value });
      })
      .catch(() => {
        if (active) setMaskState({ key: credentialIdKey, masks: {} });
      });
    return () => {
      active = false;
    };
  }, [credentialIdKey]);

  const masks = maskState?.masks ?? {};
  const masksReady = maskState !== null && maskState.key === credentialIdKey;
  const isSecretMissing = (secretId: string) => masksReady && !(secretId in masks);

  const submitProvider = async () => {
    if (!providerForm) return;
    if (providerForm.display_name.trim().length === 0) {
      notify("프로바이더 이름은 필수입니다", "error");
      return;
    }
    const payload: ProviderInput = {
      name: providerForm.name.trim() || providerForm.display_name.trim().toLowerCase(),
      display_name: providerForm.display_name.trim(),
      kind: providerForm.kind,
      base_url: providerForm.base_url.trim() || null,
      homepage_url: providerForm.homepage_url.trim() || null,
      docs_url: providerForm.docs_url.trim() || null,
      notes: providerForm.notes.trim() || null,
    };
    try {
      if (providerForm.id) {
        await updateProvider(providerForm.id, payload);
        notify("프로바이더가 수정되었습니다", "success");
      } else {
        const id = await createProvider(payload);
        setProviderIdState(id);
        notify("프로바이더가 추가되었습니다", "success");
      }
      setProviderForm(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const submitAccount = async () => {
    if (!accountForm || !providerId) return;
    if (accountForm.name.trim().length === 0) {
      notify("계정 이름은 필수입니다", "error");
      return;
    }
    const cost = Number(accountForm.monthly_cost.trim() || "0");
    const payload: AccountInput = {
      provider_id: providerId,
      name: accountForm.name.trim(),
      plan_type: accountForm.plan_type,
      plan_name: accountForm.plan_name.trim() || null,
      monthly_cost: Number.isFinite(cost) ? cost : 0,
      currency: accountForm.currency.trim().toUpperCase() || "USD",
      billing_day: accountForm.billing_day.trim()
        ? Number(accountForm.billing_day.trim())
        : null,
      renewal_date: accountForm.renewal_date.trim() || null,
      status: accountForm.status,
      notes: accountForm.notes.trim() || null,
    };
    try {
      if (accountForm.id) {
        await updateAccount(accountForm.id, payload);
        notify("계정이 수정되었습니다", "success");
      } else {
        const id = await createAccount(payload);
        setAccountIdState(id);
        notify("계정이 추가되었습니다", "success");
      }
      setAccountForm(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const submitCredential = async () => {
    if (!credentialForm || !selectedProvider) return;
    if (credentialForm.name.trim().length === 0) {
      notify("인증 정보 이름은 필수입니다", "error");
      return;
    }
    if (!credentialForm.id && credentialForm.secret.length === 0) {
      notify("API 키 값은 필수입니다", "error");
      return;
    }
    try {
      if (credentialForm.id) {
        await updateCredential(credentialForm.id, {
          name: credentialForm.name.trim(),
          env_name: credentialForm.env_name.trim() || null,
          status: credentialForm.status,
          expires_at: credentialForm.expires_at.trim() || null,
          notes: credentialForm.notes.trim() || null,
        });
        if (credentialForm.secret.length > 0) {
          const existing = data?.credentials.find((item) => item.id === credentialForm.id);
          if (existing) {
            await secretStore(existing.secret_id, credentialForm.secret);
          }
        }
        notify(
          credentialForm.secret.length > 0
            ? "인증 정보가 수정되고 키가 교체되었습니다"
            : "인증 정보가 수정되었습니다",
          "success",
        );
      } else {
        let targetAccountId = accountId;
        if (!targetAccountId) {
          targetAccountId = await createAccount({
            provider_id: selectedProvider.id,
            name: "기본",
            plan_type: "payg",
            plan_name: null,
            monthly_cost: 0,
            currency: "USD",
            billing_day: null,
            renewal_date: null,
            status: "active",
            notes: "API 키 추가 시 자동 생성된 계정",
          });
          setAccountIdState(targetAccountId);
        }
        const secretId = newId();
        await secretStore(secretId, credentialForm.secret);
        const createdFieldSecrets: string[] = [];
        try {
          const id = await createCredential({
            account_id: targetAccountId,
            name: credentialForm.name.trim(),
            secret_id: secretId,
            env_name: credentialForm.env_name.trim() || null,
            status: credentialForm.status,
            expires_at: credentialForm.expires_at.trim() || null,
            notes: credentialForm.notes.trim() || null,
          });
          let order = 1;
          for (const extra of credentialForm.extraFields) {
            if (extra.value.trim().length === 0) continue;
            const fieldSecretId = newId();
            await secretStore(fieldSecretId, extra.value);
            createdFieldSecrets.push(fieldSecretId);
            await createCredentialField({
              credential_id: id,
              label: extra.label.trim() || `필드 ${order + 1}`,
              env_name: extra.env_name.trim() || null,
              secret_id: fieldSecretId,
              sort_order: order,
            });
            order += 1;
          }
          setExpandedState(id);
        } catch (dbError) {
          await secretDelete(secretId);
          for (const fieldSecretId of createdFieldSecrets) {
            await secretDelete(fieldSecretId);
          }
          throw dbError;
        }
        notify("API 키를 Vault에 저장했습니다", "success");
      }
      setCredentialForm(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const collectSecrets = (list: CredentialWithContext[]) =>
    list.flatMap((credential) => {
      const fields = fieldsFor(credential.id);
      return fields.length > 0
        ? fields.map((field) => field.secret_id)
        : [credential.secret_id];
    });

  const submitField = async () => {
    if (!fieldForm) return;
    if (fieldForm.id === null) {
      if (fieldForm.label.trim().length === 0) {
        notify("필드 이름은 필수입니다", "error");
        return;
      }
      if (fieldForm.value.trim().length === 0) {
        notify("값은 필수입니다", "error");
        return;
      }
      try {
        const existing = fieldsFor(fieldForm.credentialId);
        const secretId = newId();
        await secretStore(secretId, fieldForm.value);
        try {
          await createCredentialField({
            credential_id: fieldForm.credentialId,
            label: fieldForm.label.trim(),
            env_name: fieldForm.env_name.trim() || null,
            secret_id: secretId,
            sort_order: existing.length,
          });
        } catch (dbError) {
          await secretDelete(secretId);
          throw dbError;
        }
        notify("필드를 추가했습니다", "success");
      } catch (err) {
        notify(errorMessage(err), "error");
        return;
      }
    } else {
      try {
        await updateCredentialField(fieldForm.id, {
          label: fieldForm.label.trim(),
          env_name: fieldForm.env_name.trim() || null,
        });
        const field = (data?.fields ?? []).find((item) => item.id === fieldForm.id);
        if (field && fieldForm.value.trim().length > 0) {
          await secretStore(field.secret_id, fieldForm.value);
        }
        notify("필드를 수정했습니다", "success");
      } catch (err) {
        notify(errorMessage(err), "error");
        return;
      }
    }
    setFieldForm(null);
    reload();
  };

  const confirmDelete = async () => {
    if (!deleteTarget || !data) return;
    try {
      if (deleteTarget.kind === "field") {
        await secretDelete(deleteTarget.secretId);
        await deleteCredentialField(deleteTarget.id);
      } else if (deleteTarget.kind === "credential") {
        const secrets = collectSecrets(
          data.credentials.filter((credential) => credential.id === deleteTarget.id),
        );
        for (const secretId of secrets) {
          await secretDelete(secretId);
        }
        await deleteCredential(deleteTarget.id);
        setExpandedState(null);
      } else if (deleteTarget.kind === "account") {
        const secrets = collectSecrets(
          data.credentials.filter((credential) => credential.account_id === deleteTarget.id),
        );
        for (const secretId of secrets) {
          await secretDelete(secretId);
        }
        await deleteAccount(deleteTarget.id);
        setAccountIdState(null);
      } else {
        const accountIds = data.accounts
          .filter((account) => account.provider_id === deleteTarget.id)
          .map((account) => account.id);
        const secrets = collectSecrets(
          data.credentials.filter((credential) => accountIds.includes(credential.account_id)),
        );
        for (const secretId of secrets) {
          await secretDelete(secretId);
        }
        await deleteProvider(deleteTarget.id);
        setProviderIdState(null);
      }
      notify("삭제되었습니다", "success");
      setDeleteTarget(null);
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  const runTest = async (credential: CredentialWithContext) => {
    const baseUrl = credential.provider_base_url ?? "";
    if (baseUrl.trim().length === 0) {
      notify("이 프로바이더에는 Base URL이 없습니다. 먼저 프로바이더를 편집하세요.", "error");
      return;
    }
    setTestingId(credential.id);
    try {
      const request = buildTestRequest(
        credential.provider_kind,
        baseUrl,
        credential.secret_id,
      );
      const outcome = await testHttp(request);
      await recordTestResult(credential.id, outcome.status);
      notify(
        `${testStatusLabel(outcome.status)} · ${outcome.message} (${outcome.latencyMs} ms)`,
        isTestSuccess(outcome.status) ? "success" : "error",
      );
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setTestingId(null);
    }
  };

  const refreshUsageFor = async (credential: CredentialWithContext) => {
    const adapter = usageAdapterFor(
      credential.provider_name,
      credential.provider_display_name,
      credential.provider_base_url,
    );
    if (!adapter) {
      notify("이 프로바이더는 사용량 조회를 지원하지 않습니다", "info");
      return;
    }
    setUsageBusy(credential.id);
    try {
      const outcome = await fetchUsage(
        adapter,
        credential.provider_base_url,
        credential.secret_id,
        usageExtraSecretFor(adapter, fieldsFor(credential.id)),
      );
      await upsertUsageSnapshot({
        credential_id: credential.id,
        adapter,
        status: outcome.status,
        summary: outcome.summary || outcome.message || "",
        used: outcome.used,
        limit_total: outcome.limit,
        remaining: outcome.remaining,
        currency: outcome.currency,
        details_json: outcome.details ? JSON.stringify(outcome.details) : null,
        fetched_at: new Date().toISOString(),
      });
      notify(
        outcome.status === "ok"
          ? `사용량: ${outcome.summary}`
          : (outcome.message ?? "사용량 정보를 가져오지 못했습니다"),
        outcome.status === "ok" ? "success" : "info",
      );
      reload();
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setUsageBusy(null);
    }
  };

  if (loading && !data) return <div className="page-loading">API 데이터 불러오는 중…</div>;
  if (error) return <div className="page-error">{error}</div>;

  return (
    <div className="page page-flush">
      <div className="apis-layout">
        <section className="pane pane-tree">
          <header className="pane-header">
            <h3>프로바이더</h3>
            <div className="row-actions">
              <button
                type="button"
                className="button button-small"
                disabled={fillBusy}
                onClick={() => void fillCustomProviders()}
                title="이름이 일치하는 커스텀 프로바이더의 종류·Base URL·홈페이지·문서를 자동으로 채웁니다"
              >
                {fillBusy ? "채우는 중…" : "커스텀 자동 채우기"}
              </button>
              <button
                type="button"
                className="button button-small button-primary"
                onClick={() => setProviderForm(emptyProviderForm())}
              >
                + 프로바이더
              </button>
            </div>
          </header>
          <div className="pane-scroll">
            {data?.providers.length === 0 ? (
              <EmptyState
                text="아직 프로바이더가 없습니다"
                hint="OpenAI · OpenRouter · OpenCode 또는 직접 만든 엔드포인트를 추가하세요"
              />
            ) : null}
            {data?.providers.map((provider) => {
              const accounts = data.accounts.filter((account) => account.provider_id === provider.id);
              const credentialCount = data.credentials.filter(
                (credential) => credential.provider_id === provider.id,
              ).length;
              const isSelected = provider.id === providerId;
              return (
                <div key={provider.id} className="tree-provider">
                  <button
                    type="button"
                    className={`tree-row ${isSelected ? "tree-row-active" : ""}`}
                    onClick={() => {
                      setProviderIdState(provider.id);
                      setAccountIdState(null);
                      setExpandedState(null);
                    }}
                  >
                    <span className="tree-name">{provider.display_name}</span>
                    <span className="tree-meta">
                      계정 {accounts.length} · 키 {credentialCount}
                    </span>
                  </button>
                  {isSelected
                    ? accounts.map((account) => (
                        <button
                          key={account.id}
                          type="button"
                          className={`tree-row tree-row-child ${
                            account.id === accountId ? "tree-row-active" : ""
                          }`}
                          onClick={() => {
                            setAccountIdState(account.id);
                            setExpandedState(null);
                          }}
                        >
                          <span className="tree-name">{account.name}</span>
                          <span className="tree-meta">
                            {account.plan_name ?? account.plan_type}
                            {account.monthly_cost > 0
                              ? ` · ${formatMoney(account.monthly_cost, account.currency)}`
                              : ""}
                          </span>
                        </button>
                      ))
                    : null}
                  {isSelected && accounts.length === 0 ? (
                    <p className="tree-empty">이 프로바이더에 계정이 없습니다.</p>
                  ) : null}
                </div>
              );
            })}
          </div>
        </section>

        <section className="pane pane-detail">
          {selectedProvider ? (
            <>
              <header className="detail-header">
                <div>
                  <h2>
                    {selectedProvider.display_name}{" "}
                    <Badge tone="info">{selectedProvider.kind}</Badge>
                  </h2>
                  <p className="mono">
                    {selectedProvider.base_url ?? "Base URL 없음"}
                    {selectedProvider.docs_url ? (
                      <>
                        {" · "}
                        <button
                          type="button"
                          className="link-button"
                          onClick={() => void openUrl(selectedProvider.docs_url ?? "")}
                        >
                          문서
                        </button>
                      </>
                    ) : null}
                  </p>
                </div>
                <div className="row-actions">
                  <button
                    type="button"
                    className="button button-small"
                    onClick={() =>
                      setProviderForm({
                        id: selectedProvider.id,
                        preset: "",
                        name: selectedProvider.name,
                        display_name: selectedProvider.display_name,
                        kind: selectedProvider.kind,
                        base_url: selectedProvider.base_url ?? "",
                        homepage_url: selectedProvider.homepage_url ?? "",
                        docs_url: selectedProvider.docs_url ?? "",
                        notes: selectedProvider.notes ?? "",
                      })
                    }
                  >
                    편집
                  </button>
                  <button
                    type="button"
                    className="button button-small button-danger-ghost"
                    onClick={() =>
                      setDeleteTarget({
                        kind: "provider",
                        id: selectedProvider.id,
                        label: selectedProvider.display_name,
                      })
                    }
                  >
                    삭제
                  </button>
                </div>
              </header>

              {selectedAccount ? (
                <div className="account-strip">
                  <div className="account-strip-main">
                    <strong>{selectedAccount.name}</strong>
                    <AccountStatusBadge status={selectedAccount.status} />
                    <span className="muted">
                      {selectedAccount.plan_type}
                      {selectedAccount.plan_name ? ` · ${selectedAccount.plan_name}` : ""}
                    </span>
                    <span>
                      {selectedAccount.monthly_cost > 0
                        ? `${formatMoney(selectedAccount.monthly_cost, selectedAccount.currency)} / 월`
                        : "고정비 없음"}
                    </span>
                    {selectedAccount.renewal_date ? (
                      <span className="muted">갱신 {formatDate(selectedAccount.renewal_date)}</span>
                    ) : null}
                  </div>
                  <div className="row-actions">
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() =>
                        setAccountForm({
                          id: selectedAccount.id,
                          name: selectedAccount.name,
                          plan_type: selectedAccount.plan_type,
                          plan_name: selectedAccount.plan_name ?? "",
                          monthly_cost: selectedAccount.monthly_cost.toString(),
                          currency: selectedAccount.currency,
                          billing_day: selectedAccount.billing_day?.toString() ?? "",
                          renewal_date: selectedAccount.renewal_date ?? "",
                          status: selectedAccount.status,
                          notes: selectedAccount.notes ?? "",
                        })
                      }
                    >
                      편집
                    </button>
                    <button
                      type="button"
                      className="button button-small button-danger-ghost"
                      onClick={() =>
                        setDeleteTarget({
                          kind: "account",
                          id: selectedAccount.id,
                          label: selectedAccount.name,
                        })
                      }
                    >
                      삭제
                    </button>
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() => setAccountForm({
                        id: null,
                        name: "",
                        plan_type: "subscription",
                        plan_name: "",
                        monthly_cost: "0",
                        currency: "USD",
                        billing_day: "",
                        renewal_date: "",
                        status: "active",
                        notes: "",
                      })}
                    >
                      + 계정
                    </button>
                  </div>
                </div>
              ) : (
                <div className="account-strip">
                  <div className="account-strip-main">
                    <span className="muted">
                      이 프로바이더에 계정이 없습니다. “+ API 키 추가”를 누르면 기본 계정이 자동
                      생성됩니다.
                    </span>
                  </div>
                  <div className="row-actions">
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() =>
                        setAccountForm({
                          id: null,
                          name: "기본",
                          plan_type: "subscription",
                          plan_name: "",
                          monthly_cost: "0",
                          currency: "USD",
                          billing_day: "",
                          renewal_date: "",
                          status: "active",
                          notes: "",
                        })
                      }
                    >
                      + 계정
                    </button>
                  </div>
                </div>
              )}

              <div className="credentials-header">
                <h3>인증 정보</h3>
                <button
                  type="button"
                  className="button button-small button-primary"
                  disabled={!selectedProvider}
                  onClick={() => {
                    const preset =
                      PROVIDER_PRESETS.find((item) => item.name === selectedProvider.name) ??
                      lookupProviderInfo(selectedProvider.display_name) ??
                      lookupProviderInfo(selectedProvider.name);
                    const specs = preset?.credentialFields;
                    setCredentialForm({
                      id: null,
                      name: "",
                      env_name: specs?.[0]?.envName ?? preset?.defaultEnvName ?? "",
                      status: "active",
                      expires_at: "",
                      notes: "",
                      secret: "",
                      primaryLabel: specs?.[0]?.label ?? "API Key",
                      extraFields: (specs ?? []).slice(1).map((spec) => ({
                        label: spec.label,
                        env_name: spec.envName,
                        value: "",
                      })),
                    });
                  }}
                >
                  + API 키 추가
                </button>
              </div>

              <div className="credential-list">
                {!selectedAccount ? (
                  <EmptyState
                    text={
                      providerAccounts.length === 0
                        ? "아직 계정이 없습니다"
                        : "계정을 선택하면 인증 정보가 표시됩니다"
                    }
                    hint={
                      providerAccounts.length === 0
                        ? "“+ API 키 추가”를 누르면 기본 계정이 만들어지고 키가 Vault에 저장됩니다"
                        : undefined
                    }
                  />
                ) : null}
                {selectedAccount && accountCredentials.length === 0 ? (
                  <EmptyState
                    text="이 계정에 아직 인증 정보가 없습니다"
                    hint="API 키를 추가하면 Stronghold Vault에 저장됩니다"
                  />
                ) : null}
                {accountCredentials.map((credential) => {
                  const expanded = expandedCredentialId === credential.id;
                  const credentialFields = fieldsFor(credential.id);
                  const displayEnv =
                    credential.env_name ?? credentialFields[0]?.env_name ?? null;
                  const primaryId = credentialFields[0]?.secret_id ?? credential.secret_id;
                  const usageAdapter = usageAdapterFor(
                    credential.provider_name,
                    credential.provider_display_name,
                    credential.provider_base_url,
                  );
                  const usageSnapshot = snapshotByCredential.get(credential.id) ?? null;
                  return (
                    <div
                      key={credential.id}
                      className={`credential-row ${expanded ? "credential-row-expanded" : ""}`}
                    >
                      <button
                        type="button"
                        className="credential-head"
                        onClick={() =>
                          setExpandedState(expanded ? null : credential.id)
                        }
                      >
                        <span className="credential-name">
                          <span className="credential-chevron" aria-hidden="true">
                            {expanded ? "▾" : "▸"}
                          </span>
                          {credential.name}
                        </span>
                        <span className="mono credential-env">
                          {displayEnv ?? "환경변수 이름 없음"}
                          {credentialFields.length > 1
                            ? ` · 필드 ${credentialFields.length}`
                            : ""}
                        </span>
                        <CredentialStatusBadge status={credential.status} />
                        <TestStatusBadge status={credential.last_test_status} />
                        {isSecretMissing(primaryId) ? (
                          <Badge tone="warn">키 없음</Badge>
                        ) : (
                          <code className="secret-value">
                            {masks[primaryId] ?? "••••••••"}
                            {credentialFields.length > 1
                              ? ` +${credentialFields.length - 1}`
                              : ""}
                          </code>
                        )}
                      </button>
                      {expanded ? (
                        <div className="credential-detail">
                          {isSecretMissing(primaryId) ? (
                            <p className="warn-note credential-warn">
                              일부 키의 값이 Vault에 없습니다(초기화 전에 저장된 키). 아래 필드에서
                              값을 다시 입력하면 보기 · 복사 · 테스트를 사용할 수 있습니다.
                            </p>
                          ) : null}
                          <div className="credential-detail-grid">
                            <div className="field-wide">
                              <dt>인증 필드</dt>
                              <dd>
                                <div className="credential-field-list">
                                  {(credentialFields.length > 0
                                    ? credentialFields
                                    : [
                                        {
                                          id: `legacy-${credential.id}`,
                                          credential_id: credential.id,
                                          label: "API Key",
                                          env_name: credential.env_name,
                                          secret_id: credential.secret_id,
                                          sort_order: 0,
                                          created_at: credential.created_at,
                                          updated_at: credential.updated_at,
                                        },
                                      ]
                                  ).map((field) => (
                                    <div key={field.id} className="credential-field-row">
                                      <div className="credential-field-info">
                                        <span className="credential-field-label">{field.label}</span>
                                        <span className="mono muted">
                                          {field.env_name ?? "—"}
                                        </span>
                                      </div>
                                      <SecretActions
                                        secretId={field.secret_id}
                                        mask={masks[field.secret_id] ?? null}
                                        autoHideSecs={autoHideSecs}
                                        clipboardClearSecs={clipboardClearSecs}
                                      />
                                      <div className="row-actions">
                                        <button
                                          type="button"
                                          className="button button-small"
                                          onClick={() =>
                                            setFieldForm({
                                              credentialId: credential.id,
                                              id: field.id,
                                              label: field.label,
                                              env_name: field.env_name ?? "",
                                              value: "",
                                            })
                                          }
                                        >
                                          편집
                                        </button>
                                        <button
                                          type="button"
                                          className="button button-small button-danger-ghost"
                                          disabled={credentialFields.length <= 1}
                                          title={
                                            credentialFields.length <= 1
                                              ? "마지막 필드는 삭제할 수 없습니다"
                                              : "필드 삭제"
                                          }
                                          onClick={() =>
                                            setDeleteTarget({
                                              kind: "field",
                                              id: field.id,
                                              label: field.label,
                                              secretId: field.secret_id,
                                            })
                                          }
                                        >
                                          삭제
                                        </button>
                                      </div>
                                    </div>
                                  ))}
                                  <div className="credential-field-add">
                                    <button
                                      type="button"
                                      className="button button-small button-primary"
                                      onClick={() =>
                                        setFieldForm({
                                          credentialId: credential.id,
                                          id: null,
                                          label: "",
                                          env_name: "",
                                          value: "",
                                        })
                                      }
                                    >
                                      + 필드 추가
                                    </button>
                                    <span className="muted">
                                      Client ID·Secret처럼 값이 여러 개면 필드로 추가하세요
                                    </span>
                                  </div>
                                </div>
                              </dd>
                            </div>
                            <div>
                              <dt>환경변수</dt>
                              <dd className="mono">{displayEnv ?? "—"}</dd>
                            </div>
                            <div>
                              <dt>생성일</dt>
                              <dd>{formatDateTime(credential.created_at)}</dd>
                            </div>
                            <div>
                              <dt>만료일</dt>
                              <dd>{credential.expires_at ? formatDate(credential.expires_at) : "—"}</dd>
                            </div>
                            <div>
                              <dt>최근 테스트</dt>
                              <dd>
                                {credential.last_tested_at
                                  ? `${formatDateTime(credential.last_tested_at)} · ${testStatusLabel(credential.last_test_status)}`
                                  : "없음"}
                              </dd>
                            </div>
                            <div className="field-wide">
                              <dt>사용량 · 잔액</dt>
                              <dd>
                                {usageAdapter ? (
                                  <span className="credential-usage">
                                    <span className="mono">
                                      {usageSnapshot?.summary || "미조회"}
                                    </span>
                                    {usageSnapshot ? (
                                      <Badge
                                        tone={
                                          usageSnapshot.status === "ok"
                                            ? "ok"
                                            : usageSnapshot.status === "unsupported"
                                              ? "muted"
                                              : usageSnapshot.status === "auth_required"
                                                ? "warn"
                                                : "danger"
                                        }
                                      >
                                        {usageStatusLabel(usageSnapshot.status)}
                                      </Badge>
                                    ) : null}
                                    {usageSnapshot ? (
                                      <span className="muted">
                                        {formatDateTime(usageSnapshot.fetched_at)}
                                      </span>
                                    ) : null}
                                    <button
                                      type="button"
                                      className="button button-small"
                                      disabled={usageBusy === credential.id}
                                      onClick={() => void refreshUsageFor(credential)}
                                    >
                                      {usageBusy === credential.id ? "조회 중…" : "조회"}
                                    </button>
                                  </span>
                                ) : (
                                  <span className="muted">
                                    이 프로바이더는 사용량 조회를 지원하지 않습니다
                                  </span>
                                )}
                              </dd>
                            </div>
                            {credential.notes ? (
                              <div className="field-wide">
                                <dt>메모</dt>
                                <dd>{credential.notes}</dd>
                              </div>
                            ) : null}
                          </div>
                          <div className="credential-actions">
                            <button
                              type="button"
                              className="button button-small"
                              onClick={() => void runTest(credential)}
                              disabled={testingId === credential.id}
                            >
                              {testingId === credential.id ? "테스트 중…" : "테스트"}
                            </button>
                            <button
                              type="button"
                              className="button button-small"
                              onClick={() =>
                                setCredentialForm({
                                  id: credential.id,
                                  name: credential.name,
                                  env_name: credential.env_name ?? "",
                                  status: credential.status,
                                  expires_at: credential.expires_at ?? "",
                                  notes: credential.notes ?? "",
                                  secret: "",
                                  primaryLabel: credentialFields[0]?.label ?? "API Key",
                                  extraFields: [],
                                })
                              }
                            >
                              편집
                            </button>
                            <button
                              type="button"
                              className="button button-small button-danger-ghost"
                              onClick={() =>
                                setDeleteTarget({
                                  kind: "credential",
                                  id: credential.id,
                                  label: credential.name,
                                  secretId: credential.secret_id,
                                })
                              }
                            >
                              삭제
                            </button>
                          </div>
                        </div>
                      ) : null}
                    </div>
                  );
                })}
              </div>
            </>
          ) : (
            <div className="pane-empty">
              <p>프로바이더를 선택하거나 새로 만들어 시작하세요.</p>
            </div>
          )}
        </section>
      </div>

      {providerForm ? (
        <Modal
          title={providerForm.id ? "프로바이더 편집" : "프로바이더 추가"}
          onClose={() => setProviderForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setProviderForm(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => void submitProvider()}
              >
                {providerForm.id ? "저장" : "추가"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <div className="field field-wide row-actions modal-quick-actions">
              <button type="button" className="button button-small" onClick={fillProviderFromCatalog}>
                카탈로그에서 자동 채우기
              </button>
              <span className="muted">
                {providerForm.id
                  ? "표시 이름·슬러그로 기본 정보를 찾아 채웁니다"
                  : "이름을 입력한 뒤 누르면 종류·Base URL·홈페이지·문서가 채워집니다"}
              </span>
            </div>
            {!providerForm.id ? (
              <label className="field field-wide">
                <span>프리셋</span>
                <select
                  value={providerForm.preset}
                  onChange={(event) => {
                    const chosen = [...PROVIDER_PRESETS, CUSTOM_PRESET].find(
                      (preset) => preset.name === event.target.value,
                    );
                    if (!chosen) {
                      setProviderForm({ ...providerForm, preset: "" });
                      return;
                    }
                    setProviderForm({
                      ...providerForm,
                      preset: chosen.name,
                      name: chosen.name === "custom" ? providerForm.name : chosen.name,
                      display_name:
                        chosen.name === "custom" ? providerForm.display_name : chosen.displayName,
                      kind: chosen.kind,
                      base_url: chosen.baseUrl,
                      homepage_url: chosen.homepageUrl,
                      docs_url: chosen.docsUrl,
                    });
                  }}
                >
                  <option value="">프리셋 선택…</option>
                  {PROVIDER_PRESETS.map((preset) => (
                    <option key={preset.name} value={preset.name}>
                      {preset.displayName}
                    </option>
                  ))}
                  <option value="custom">직접 입력</option>
                </select>
              </label>
            ) : null}
            <label className="field">
              <span>표시 이름</span>
              <input
                value={providerForm.display_name}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, display_name: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>슬러그</span>
              <input
                className="mono"
                value={providerForm.name}
                onChange={(event) => setProviderForm({ ...providerForm, name: event.target.value })}
              />
            </label>
            <label className="field">
              <span>어댑터 종류</span>
              <select
                value={providerForm.kind}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, kind: event.target.value as ProviderKind })
                }
              >
                <option value="openai">openai</option>
                <option value="openrouter">openrouter</option>
                <option value="anthropic">anthropic</option>
                <option value="google">google</option>
                <option value="openai-compatible">openai-compatible</option>
                <option value="custom">custom</option>
              </select>
            </label>
            <label className="field">
              <span>Base URL</span>
              <input
                className="mono"
                value={providerForm.base_url}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, base_url: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>홈페이지</span>
              <input
                value={providerForm.homepage_url}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, homepage_url: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>문서</span>
              <input
                value={providerForm.docs_url}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, docs_url: event.target.value })
                }
              />
            </label>
            <label className="field field-wide">
              <span>메모</span>
              <textarea
                rows={3}
                value={providerForm.notes}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, notes: event.target.value })
                }
              />
            </label>
          </div>
        </Modal>
      ) : null}

      {accountForm ? (
        <Modal
          title={accountForm.id ? "계정 편집" : "계정 추가"}
          subtitle={selectedProvider?.display_name}
          onClose={() => setAccountForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setAccountForm(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => void submitAccount()}
              >
                {accountForm.id ? "저장" : "추가"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>계정 이름</span>
              <input
                value={accountForm.name}
                onChange={(event) => setAccountForm({ ...accountForm, name: event.target.value })}
              />
            </label>
            <label className="field">
              <span>요금제 유형</span>
              <select
                value={accountForm.plan_type}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, plan_type: event.target.value as PlanType })
                }
              >
                {PLAN_TYPES.map((plan) => (
                  <option key={plan} value={plan}>
                    {plan}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>요금제 이름</span>
              <input
                value={accountForm.plan_name}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, plan_name: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>월 비용</span>
              <input
                value={accountForm.monthly_cost}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, monthly_cost: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>통화</span>
              <input
                value={accountForm.currency}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, currency: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>결제일</span>
              <input
                value={accountForm.billing_day}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, billing_day: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>갱신일</span>
              <input
                type="date"
                value={accountForm.renewal_date}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, renewal_date: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>상태</span>
              <select
                value={accountForm.status}
                onChange={(event) =>
                  setAccountForm({ ...accountForm, status: event.target.value as AccountStatus })
                }
              >
                {ACCOUNT_STATUSES.map((status) => (
                  <option key={status} value={status}>
                    {status}
                  </option>
                ))}
              </select>
            </label>
            <label className="field field-wide">
              <span>메모</span>
              <textarea
                rows={3}
                value={accountForm.notes}
                onChange={(event) => setAccountForm({ ...accountForm, notes: event.target.value })}
              />
            </label>
          </div>
        </Modal>
      ) : null}

      {credentialForm ? (
        <Modal
          title={credentialForm.id ? "인증 정보 편집" : "인증 정보 추가"}
          subtitle={selectedAccount ? `${selectedProvider?.display_name} / ${selectedAccount.name}` : undefined}
          onClose={() => setCredentialForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setCredentialForm(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => void submitCredential()}
              >
                {credentialForm.id ? "저장" : "Vault에 저장"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>인증 정보 이름</span>
              <input
                value={credentialForm.name}
                onChange={(event) =>
                  setCredentialForm({ ...credentialForm, name: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>환경변수 이름</span>
              <input
                className="mono"
                placeholder="OPENAI_API_KEY"
                value={credentialForm.env_name}
                onChange={(event) =>
                  setCredentialForm({ ...credentialForm, env_name: event.target.value })
                }
              />
            </label>
            <label className="field field-wide">
              <span>
                {credentialForm.primaryLabel}{" "}
                {credentialForm.id ? "(비우면 기존 값 유지)" : "(Stronghold에 저장됨)"}
              </span>
              <input
                type="password"
                autoComplete="off"
                value={credentialForm.secret}
                onChange={(event) =>
                  setCredentialForm({ ...credentialForm, secret: event.target.value })
                }
              />
            </label>
            {!credentialForm.id
              ? credentialForm.extraFields.map((extra, index) => (
                  <div key={index} className="field field-wide extra-field">
                    <div className="field-row">
                      <input
                        aria-label="필드 이름"
                        value={extra.label}
                        onChange={(event) => {
                          const next = [...credentialForm.extraFields];
                          next[index] = { ...extra, label: event.target.value };
                          setCredentialForm({ ...credentialForm, extraFields: next });
                        }}
                      />
                      <input
                        className="mono"
                        aria-label="환경변수 이름"
                        value={extra.env_name}
                        onChange={(event) => {
                          const next = [...credentialForm.extraFields];
                          next[index] = { ...extra, env_name: event.target.value };
                          setCredentialForm({ ...credentialForm, extraFields: next });
                        }}
                      />
                    </div>
                    <input
                      type="password"
                      autoComplete="off"
                      placeholder="값 (비우면 이 필드는 건너뜀)"
                      value={extra.value}
                      onChange={(event) => {
                        const next = [...credentialForm.extraFields];
                        next[index] = { ...extra, value: event.target.value };
                        setCredentialForm({ ...credentialForm, extraFields: next });
                      }}
                    />
                  </div>
                ))
              : null}
            <label className="field">
              <span>상태</span>
              <select
                value={credentialForm.status}
                onChange={(event) =>
                  setCredentialForm({
                    ...credentialForm,
                    status: event.target.value as CredentialStatus,
                  })
                }
              >
                {CREDENTIAL_STATUSES.map((status) => (
                  <option key={status} value={status}>
                    {status}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>만료일</span>
              <input
                type="date"
                value={credentialForm.expires_at}
                onChange={(event) =>
                  setCredentialForm({ ...credentialForm, expires_at: event.target.value })
                }
              />
            </label>
            <label className="field field-wide">
              <span>메모</span>
              <textarea
                rows={3}
                value={credentialForm.notes}
                onChange={(event) =>
                  setCredentialForm({ ...credentialForm, notes: event.target.value })
                }
              />
            </label>
          </div>
          <p className="form-hint">
            키는 암호화된 Stronghold Vault에 바로 기록됩니다. 화면에는 마스킹된 미리보기만
            표시됩니다. 값이 여러 개(Client ID + Secret 등)면 저장 후 상세에서 ‘+ 필드 추가’로
            넣을 수 있습니다.
          </p>
        </Modal>
      ) : null}

      {fieldForm ? (
        <Modal
          title={fieldForm.id ? "필드 편집" : "필드 추가"}
          subtitle="Client ID / Client Secret 처럼 여러 값을 한 인증 정보에 저장합니다"
          onClose={() => setFieldForm(null)}
          footer={
            <>
              <button type="button" className="button" onClick={() => setFieldForm(null)}>
                취소
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => void submitField()}
              >
                {fieldForm.id ? "저장" : "추가"}
              </button>
            </>
          }
        >
          <div className="form-grid">
            <label className="field">
              <span>필드 이름</span>
              <input
                value={fieldForm.label}
                placeholder="Client ID"
                onChange={(event) => setFieldForm({ ...fieldForm, label: event.target.value })}
              />
            </label>
            <label className="field">
              <span>환경변수 이름</span>
              <input
                className="mono"
                placeholder="NAVER_API_HUB_CLIENT_ID"
                value={fieldForm.env_name}
                onChange={(event) =>
                  setFieldForm({ ...fieldForm, env_name: event.target.value })
                }
              />
            </label>
            <label className="field field-wide">
              <span>{fieldForm.id ? "새 값 (비우면 유지)" : "값"}</span>
              <input
                type="password"
                autoComplete="off"
                value={fieldForm.value}
                onChange={(event) => setFieldForm({ ...fieldForm, value: event.target.value })}
              />
            </label>
          </div>
        </Modal>
      ) : null}

      {deleteTarget ? (
        <ConfirmDialog
          title="삭제 확인"
          danger
          confirmLabel="영구 삭제"
          message={
            <>
              <p>
                <strong>{deleteTarget.label}</strong>을(를) 삭제할까요?
              </p>
              {deleteTarget.kind === "field" ? (
                <p>이 필드와 Vault의 비밀 값이 함께 삭제됩니다.</p>
              ) : deleteTarget.kind === "credential" ? (
                <p>
                  메타데이터 행과 Vault의 암호화된 비밀 값이 함께 삭제됩니다.
                </p>
              ) : (
                <p>
                  이
                  {deleteTarget.kind === "provider" ? "프로바이더" : "계정"} 아래의 모든 계정·인증 정보와 Vault 비밀 값이 삭제됩니다. 되돌릴 수 없습니다.
                </p>
              )}
            </>
          }
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => void confirmDelete()}
        />
      ) : null}
    </div>
  );
}
