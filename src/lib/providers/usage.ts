import type { CredentialField } from "../db/repo";

export type UsageAdapter = "openrouter" | "deepseek" | "tavily" | "openai" | "anthropic" | "xai";

export const USAGE_ADAPTER_LABELS: Record<UsageAdapter, string> = {
  openrouter: "OpenRouter 사용량",
  deepseek: "DeepSeek 잔액",
  tavily: "Tavily 크레딧",
  openai: "OpenAI 비용 (Admin)",
  anthropic: "Anthropic 비용 (Admin)",
  xai: "xAI 선불 크레딧",
};

export function usageAdapterFor(
  providerName: string,
  providerDisplayName: string,
  baseUrl: string | null,
): UsageAdapter | null {
  const haystack = `${providerName} ${providerDisplayName} ${baseUrl ?? ""}`.toLowerCase();
  if (haystack.includes("openrouter")) return "openrouter";
  if (haystack.includes("deepseek")) return "deepseek";
  if (haystack.includes("tavily")) return "tavily";
  if (haystack.includes("anthropic") || haystack.includes("claude")) return "anthropic";
  if (haystack.includes("openai") || haystack.includes("chatgpt")) return "openai";
  if (
    haystack.includes("xai") ||
    haystack.includes("x.ai") ||
    haystack.includes("grok")
  ) {
    return "xai";
  }
  return null;
}

export function usageExtraSecretFor(
  adapter: UsageAdapter | null,
  fields: CredentialField[],
): string | null {
  if (adapter !== "xai") return null;
  const teamField = fields.find((field) =>
    /team/i.test(`${field.label} ${field.env_name ?? ""}`),
  );
  return teamField?.secret_id ?? null;
}

export function usageStatusLabel(status: string): string {
  if (status === "ok") return "정상";
  if (status === "unsupported") return "미지원";
  if (status === "auth_required") return "인증 필요";
  return "오류";
}
