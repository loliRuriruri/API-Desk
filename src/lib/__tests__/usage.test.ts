import { describe, expect, it } from "vitest";
import { usageAdapterFor, usageExtraSecretFor, usageStatusLabel } from "../providers/usage";

describe("usage adapters", () => {
  it("detects supported providers by name or base url", () => {
    expect(usageAdapterFor("openrouter", "OpenRouter", null)).toBe("openrouter");
    expect(usageAdapterFor("deepseek", "DeepSeek", "https://api.deepseek.com/v1")).toBe("deepseek");
    expect(usageAdapterFor("tavily", "Tavily", null)).toBe("tavily");
    expect(usageAdapterFor("openai", "OpenAI", "https://api.openai.com/v1")).toBe("openai");
    expect(usageAdapterFor("anthropic", "Anthropic", null)).toBe("anthropic");
    expect(usageAdapterFor("xai", "xAI", "https://api.x.ai/v1")).toBe("xai");
    expect(
      usageAdapterFor("custom", "내 서버", "https://openrouter.ai/api/v1"),
    ).toBe("openrouter");
  });

  it("returns null for providers without a usage endpoint", () => {
    expect(usageAdapterFor("nvidia", "NVIDIA", null)).toBeNull();
    expect(usageAdapterFor("custom", "한국은행 ECOS", null)).toBeNull();
  });

  it("picks the Team ID field as the extra secret for xAI", () => {
    const fields = [
      { id: "f1", credential_id: "c1", label: "Management Key", env_name: "XAI_MGMT_KEY", secret_id: "s1", sort_order: 0, created_at: "", updated_at: "" },
      { id: "f2", credential_id: "c1", label: "Team ID", env_name: "XAI_TEAM_ID", secret_id: "s2", sort_order: 1, created_at: "", updated_at: "" },
    ];
    expect(usageExtraSecretFor("xai", fields)).toBe("s2");
    expect(usageExtraSecretFor("openrouter", fields)).toBeNull();
  });

  it("labels usage statuses", () => {
    expect(usageStatusLabel("ok")).toBe("정상");
    expect(usageStatusLabel("unsupported")).toBe("미지원");
    expect(usageStatusLabel("auth_required")).toBe("인증 필요");
    expect(usageStatusLabel("error")).toBe("오류");
  });
});
