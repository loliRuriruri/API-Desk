import { describe, expect, it } from "vitest";
import {
  detectProviderFromEnvName,
  detectProviderFromText,
  findPreset,
  lookupProviderInfo,
  PROVIDER_PRESETS,
} from "../providers/presets";

describe("provider detection", () => {
  it("detects providers from environment variable names", () => {
    expect(detectProviderFromEnvName("OPENAI_API_KEY")?.name).toBe("openai");
    expect(detectProviderFromEnvName("OPENROUTER_API_KEY")?.name).toBe("openrouter");
    expect(detectProviderFromEnvName("GEMINI_API_KEY")?.name).toBe("google");
    expect(detectProviderFromEnvName("XAI_API_KEY")?.name).toBe("xai");
    expect(detectProviderFromEnvName("DEEPSEEK_API_KEY")?.name).toBe("deepseek");
    expect(detectProviderFromEnvName("SOMETHING_ELSE")).toBeUndefined();
  });

  it("detects providers from free text", () => {
    expect(detectProviderFromText("OpenRouter 계정")?.name).toBe("openrouter");
    expect(detectProviderFromText("claude pro 구독")?.name).toBe("anthropic");
    expect(detectProviderFromText("random notes")?.name).toBeUndefined();
  });

  it("finds presets by slug and alias", () => {
    expect(findPreset("openai")?.displayName).toBe("OpenAI");
    expect(findPreset("grok")?.name).toBe("xai");
  });

  it("ships unique preset names and env names", () => {
    const names = PROVIDER_PRESETS.map((preset) => preset.name);
    const envNames = PROVIDER_PRESETS.map((preset) => preset.defaultEnvName);
    expect(new Set(names).size).toBe(names.length);
    expect(new Set(envNames).size).toBe(envNames.length);
  });

  it("looks up known provider metadata for custom entries", () => {
    expect(lookupProviderInfo("Vroid HUB")?.baseUrl).toBe("https://hub.vroid.com/api");
    expect(lookupProviderInfo("키움증권")?.baseUrl).toBe("https://api.kiwoom.com");
    expect(lookupProviderInfo("티빌리")?.name).toBe("tavily");
    expect(lookupProviderInfo("Cloudflare")?.kind).toBe("openai-compatible");
    expect(lookupProviderInfo("한국은행 ECOS")?.docsUrl).toBe("https://ecos.bok.or.kr/api/#/");
    expect(lookupProviderInfo("존재하지않는서비스")).toBeUndefined();
  });

  it("fills metadata for env names of library providers", () => {
    expect(detectProviderFromEnvName("TAVILY_API_KEY")?.name).toBe("tavily");
    expect(detectProviderFromEnvName("ECOS_API_KEY")?.name).toBe("ecos");
    expect(detectProviderFromText("한국투자증권 API")?.name).toBe("koreainvestment");
  });

  it("defines multi-field credentials for two-key providers", () => {
    const naver = lookupProviderInfo("네이버 HUB");
    expect(naver?.credentialFields?.map((field) => field.label)).toEqual([
      "Client ID (X-NCP-APIGW-API-KEY-ID)",
      "Client Secret (X-NCP-APIGW-API-KEY)",
    ]);
    const kiwoom = lookupProviderInfo("키움증권");
    expect(kiwoom?.credentialFields).toHaveLength(2);
    expect(lookupProviderInfo("Tavily")?.credentialFields).toBeUndefined();
  });
});
