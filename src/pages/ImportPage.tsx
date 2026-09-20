import { useState } from "react";
import {
  createAccount,
  createCredential,
  createProvider,
  listAccounts,
  listProviders,
  updateAccount,
} from "../lib/db/repo";
import { Badge } from "../components/Badges";
import { useToast } from "../components/ToastProvider";
import { errorMessage } from "../lib/errors";
import { newId } from "../lib/format";
import { parseMemoText, type MemoParseResult } from "../lib/import/memoParser";
import { findPreset } from "../lib/providers/presets";
import { loadImportNotes, saveImportNotes } from "../lib/settings";
import { secretDelete, secretStore } from "../lib/vault";

export function ImportPage() {
  const [text, setText] = useState("");
  const [parsed, setParsed] = useState<MemoParseResult | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [noteSaved, setNoteSaved] = useState(false);
  const { notify } = useToast();

  const runParse = () => {
    const result = parseMemoText(text);
    setParsed(result);
    setSelected(new Set(result.credentials.map((credential) => credential.tempId)));
    setNoteSaved(false);
    if (result.credentials.length === 0) {
      notify("붙여넣은 텍스트에서 API 키를 찾지 못했습니다", "info");
    }
  };

  const importSelected = async () => {
    if (!parsed) return;
    const chosen = parsed.credentials.filter((credential) => selected.has(credential.tempId));
    if (chosen.length === 0) {
      notify("가져올 항목을 하나 이상 선택하세요", "error");
      return;
    }
    setBusy(true);
    try {
      const providers = await listProviders();
      let imported = 0;
      for (const entry of chosen) {
        const preset = entry.providerName ? findPreset(entry.providerName) : undefined;
        const slug = preset?.name ?? entry.providerName ?? "custom";
        const provider = providers.find((item) => item.name === slug);
        let providerId: string;
        if (provider) {
          providerId = provider.id;
        } else {
          providerId = await createProvider({
            name: slug,
            display_name: preset?.displayName ?? entry.providerDisplayName,
            kind: preset?.kind ?? "custom",
            base_url: entry.baseUrl ?? preset?.baseUrl ?? null,
            homepage_url: preset?.homepageUrl ?? null,
            docs_url: preset?.docsUrl ?? null,
            notes: "붙여넣은 텍스트에서 가져옴",
          });
        }
        const accounts = await listAccounts(providerId);
        let accountId = accounts[0]?.id;
        if (!accountId) {
          accountId = await createAccount({
            provider_id: providerId,
            name: entry.accountName ?? "기본",
            plan_type: entry.planName === "free" ? "free" : entry.monthlyCost ? "subscription" : "payg",
            plan_name: entry.planName ?? null,
            monthly_cost: entry.monthlyCost ?? 0,
            currency: "USD",
            billing_day: null,
            renewal_date: null,
            status: "active",
            notes: entry.notes.join("\n") || null,
          });
        } else if (entry.monthlyCost !== null || entry.planName) {
          const account = accounts.find((item) => item.id === accountId);
          if (account && account.monthly_cost === 0 && entry.monthlyCost !== null) {
            await updateAccount(accountId, {
              monthly_cost: entry.monthlyCost,
              plan_name: entry.planName ?? account.plan_name,
            });
          }
        }
        const secretId = newId();
        await secretStore(secretId, entry.secret);
        try {
          await createCredential({
            account_id: accountId,
            name: entry.envName ?? `${entry.providerDisplayName} 키`,
            secret_id: secretId,
            env_name: entry.envName,
            status: "unknown",
            expires_at: null,
            notes: entry.notes.join("\n") || "붙여넣은 텍스트에서 가져옴",
          });
        } catch (dbError) {
          await secretDelete(secretId);
          throw dbError;
        }
        imported += 1;
      }
      notify(`인증 정보 ${imported}개를 Vault에 가져왔습니다`, "success");
      setText("");
      setParsed(null);
      setSelected(new Set());
    } catch (err) {
      notify(errorMessage(err), "error");
    } finally {
      setBusy(false);
    }
  };

  const saveUnparsedAsNote = async () => {
    if (!parsed || parsed.unparsed.length === 0) return;
    try {
      const notes = await loadImportNotes();
      await saveImportNotes([
        ...notes,
        {
          id: newId(),
          createdAt: new Date().toISOString(),
          text: parsed.unparsed.join("\n"),
        },
      ]);
      setNoteSaved(true);
      notify("해석되지 않은 텍스트를 메모로 저장했습니다 (설정 → 가져오기 메모)", "success");
    } catch (err) {
      notify(errorMessage(err), "error");
    }
  };

  return (
    <div className="page">
      <section className="panel">
        <header className="panel-header">
          <h3>텍스트 가져오기</h3>
          <span className="panel-hint">가져오기를 누르기 전에는 아무것도 저장되지 않습니다</span>
        </header>
        <p className="toolbar-note">
          메모장, 채팅, 목록에서 복사한 텍스트를 붙여넣으세요. 프로바이더 이름, 환경변수 형식,
          단독 API 키, Base URL, 요금 정보를 로컬에서 탐지합니다.
        </p>
        <textarea
          className="import-textarea mono"
          rows={10}
          placeholder={"OpenRouter\nOPENROUTER_API_KEY=sk-or-xxxxx\n월 $20 정도\n\nOpenAI\nsk-proj-xxxxx"}
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
        <div className="row-actions">
          <button
            type="button"
            className="button button-primary"
            onClick={runParse}
            disabled={text.trim().length === 0}
          >
            텍스트 분석
          </button>
          <button
            type="button"
            className="button"
            onClick={() => {
              setText("");
              setParsed(null);
              setSelected(new Set());
            }}
          >
            지우기
          </button>
        </div>
      </section>

      {parsed ? (
        <section className="panel">
          <header className="panel-header">
            <h3>탐지 결과</h3>
            <button
              type="button"
              className="button button-primary"
              disabled={busy || selected.size === 0}
              onClick={() => void importSelected()}
            >
              {busy ? "가져오는 중…" : `선택 항목 가져오기 (${selected.size})`}
            </button>
          </header>
          <table className="table">
            <thead>
              <tr>
                <th aria-label="선택" />
                <th>프로바이더</th>
                <th>환경변수</th>
                <th>키 값</th>
                <th>요금제</th>
                <th>월 비용</th>
                <th>메모</th>
              </tr>
            </thead>
            <tbody>
              {parsed.credentials.map((credential) => (
                <tr key={credential.tempId}>
                  <td>
                    <input
                      type="checkbox"
                      checked={selected.has(credential.tempId)}
                      onChange={(event) => {
                        const next = new Set(selected);
                        if (event.target.checked) next.add(credential.tempId);
                        else next.delete(credential.tempId);
                        setSelected(next);
                      }}
                      aria-label={`${credential.masked} 선택`}
                    />
                  </td>
                  <td>
                    {credential.providerDisplayName}
                    {credential.providerName === null ? (
                      <div className="cell-sub">
                        <Badge tone="warn">프로바이더 미확정</Badge>
                      </div>
                    ) : null}
                  </td>
                  <td className="mono">{credential.envName ?? "—"}</td>
                  <td className="mono">{credential.masked}</td>
                  <td>{credential.planName ?? "—"}</td>
                  <td className="num">
                    {credential.monthlyCost !== null ? `$${credential.monthlyCost}` : "—"}
                  </td>
                  <td className="muted">{credential.notes.join(" · ") || "—"}</td>
                </tr>
              ))}
              {parsed.credentials.length === 0 ? (
                <tr>
                  <td colSpan={7} className="empty-note">
                    API 키로 보이는 항목이 없습니다.
                  </td>
                </tr>
              ) : null}
            </tbody>
          </table>
        </section>
      ) : null}

      {parsed && parsed.unparsed.length > 0 ? (
        <section className="panel">
          <header className="panel-header">
            <h3>해석되지 않은 텍스트</h3>
            <button
              type="button"
              className="button button-small"
              disabled={noteSaved}
              onClick={() => void saveUnparsedAsNote()}
            >
              {noteSaved ? "저장됨" : "메모로 저장"}
            </button>
          </header>
          <pre className="unparsed-block">{parsed.unparsed.join("\n")}</pre>
        </section>
      ) : null}
    </div>
  );
}
