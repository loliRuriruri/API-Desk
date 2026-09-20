import type { ProviderKind } from "../../types/domain";

export interface CredentialFieldSpec {
  label: string;
  envName: string;
}

export interface ProviderPreset {
  name: string;
  displayName: string;
  kind: ProviderKind;
  baseUrl: string;
  homepageUrl: string;
  docsUrl: string;
  defaultEnvName: string;
  aliases: string[];
  credentialFields?: CredentialFieldSpec[];
}

export const CUSTOM_PRESET: ProviderPreset = {
  name: "custom",
  displayName: "Custom",
  kind: "custom",
  baseUrl: "",
  homepageUrl: "",
  docsUrl: "",
  defaultEnvName: "",
  aliases: ["custom"],
};

export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    name: "opencode",
    displayName: "OpenCode",
    kind: "openai-compatible",
    baseUrl: "https://opencode.ai/zen/v1",
    homepageUrl: "https://opencode.ai",
    docsUrl: "https://opencode.ai/docs",
    defaultEnvName: "OPENCODE_API_KEY",
    aliases: ["opencode", "open code", "opencode zen", "zen"],
  },
  {
    name: "openrouter",
    displayName: "OpenRouter",
    kind: "openrouter",
    baseUrl: "https://openrouter.ai/api/v1",
    homepageUrl: "https://openrouter.ai",
    docsUrl: "https://openrouter.ai/docs",
    defaultEnvName: "OPENROUTER_API_KEY",
    aliases: ["openrouter", "open router"],
  },
  {
    name: "openai",
    displayName: "OpenAI",
    kind: "openai",
    baseUrl: "https://api.openai.com/v1",
    homepageUrl: "https://openai.com",
    docsUrl: "https://platform.openai.com/docs",
    defaultEnvName: "OPENAI_API_KEY",
    aliases: ["openai", "open ai", "chatgpt"],
  },
  {
    name: "anthropic",
    displayName: "Anthropic",
    kind: "anthropic",
    baseUrl: "https://api.anthropic.com",
    homepageUrl: "https://www.anthropic.com",
    docsUrl: "https://docs.anthropic.com",
    defaultEnvName: "ANTHROPIC_API_KEY",
    aliases: ["anthropic", "claude"],
  },
  {
    name: "google",
    displayName: "Google / Gemini",
    kind: "google",
    baseUrl: "https://generativelanguage.googleapis.com",
    homepageUrl: "https://ai.google.dev",
    docsUrl: "https://ai.google.dev/gemini-api/docs",
    defaultEnvName: "GEMINI_API_KEY",
    aliases: ["google", "gemini", "google ai", "vertex"],
  },
  {
    name: "xai",
    displayName: "xAI",
    kind: "openai-compatible",
    baseUrl: "https://api.x.ai/v1",
    homepageUrl: "https://x.ai",
    docsUrl: "https://docs.x.ai",
    defaultEnvName: "XAI_API_KEY",
    aliases: ["xai", "x.ai", "grok"],
  },
  {
    name: "vercel",
    displayName: "Vercel",
    kind: "openai-compatible",
    baseUrl: "https://ai-gateway.vercel.sh/v1",
    homepageUrl: "https://vercel.com",
    docsUrl: "https://vercel.com/docs/ai-gateway",
    defaultEnvName: "AI_GATEWAY_API_KEY",
    aliases: ["vercel", "ai gateway", "vercel ai"],
  },
  {
    name: "groq",
    displayName: "Groq",
    kind: "openai-compatible",
    baseUrl: "https://api.groq.com/openai/v1",
    homepageUrl: "https://groq.com",
    docsUrl: "https://console.groq.com/docs",
    defaultEnvName: "GROQ_API_KEY",
    aliases: ["groq"],
  },
  {
    name: "together",
    displayName: "Together",
    kind: "openai-compatible",
    baseUrl: "https://api.together.xyz/v1",
    homepageUrl: "https://www.together.ai",
    docsUrl: "https://docs.together.ai",
    defaultEnvName: "TOGETHER_API_KEY",
    aliases: ["together", "together ai"],
  },
  {
    name: "fireworks",
    displayName: "Fireworks",
    kind: "openai-compatible",
    baseUrl: "https://api.fireworks.ai/inference/v1",
    homepageUrl: "https://fireworks.ai",
    docsUrl: "https://docs.fireworks.ai",
    defaultEnvName: "FIREWORKS_API_KEY",
    aliases: ["fireworks", "fireworks ai"],
  },
  {
    name: "deepseek",
    displayName: "DeepSeek",
    kind: "openai-compatible",
    baseUrl: "https://api.deepseek.com/v1",
    homepageUrl: "https://www.deepseek.com",
    docsUrl: "https://api-docs.deepseek.com",
    defaultEnvName: "DEEPSEEK_API_KEY",
    aliases: ["deepseek", "deep seek"],
  },
  {
    name: "mistral",
    displayName: "Mistral",
    kind: "openai-compatible",
    baseUrl: "https://api.mistral.ai/v1",
    homepageUrl: "https://mistral.ai",
    docsUrl: "https://docs.mistral.ai",
    defaultEnvName: "MISTRAL_API_KEY",
    aliases: ["mistral", "mistral ai"],
  },
  {
    name: "nvidia",
    displayName: "NVIDIA",
    kind: "openai-compatible",
    baseUrl: "https://integrate.api.nvidia.com/v1",
    homepageUrl: "https://build.nvidia.com",
    docsUrl: "https://docs.nvidia.com/nim",
    defaultEnvName: "NVIDIA_API_KEY",
    aliases: ["nvidia", "nim", "build.nvidia"],
  },
];

export function findPreset(name: string): ProviderPreset | undefined {
  const lower = name.trim().toLowerCase();
  return (
    PROVIDER_PRESETS.find((preset) => preset.name === lower) ??
    PROVIDER_PRESETS.find((preset) =>
      preset.aliases.some((alias) => alias === lower),
    )
  );
}

export const KNOWN_PROVIDER_LIBRARY: ProviderPreset[] = [
  {
    name: "cloudflare",
    displayName: "Cloudflare",
    kind: "openai-compatible",
    baseUrl: "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1",
    homepageUrl: "https://www.cloudflare.com",
    docsUrl: "https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility",
    defaultEnvName: "CLOUDFLARE_API_TOKEN",
    aliases: ["cloudflare", "workers ai", "cloudflare workers ai"],
    credentialFields: [
      { label: "API Token", envName: "CLOUDFLARE_API_TOKEN" },
      { label: "Account ID", envName: "CLOUDFLARE_ACCOUNT_ID" },
    ],
  },
  {
    name: "fish-audio",
    displayName: "Fish Audio",
    kind: "custom",
    baseUrl: "https://api.fish.audio",
    homepageUrl: "https://fish.audio",
    docsUrl: "https://docs.fish.audio",
    defaultEnvName: "FISH_API_KEY",
    aliases: ["fish audio", "fish-audio", "fishop", "fish"],
  },
  {
    name: "fred",
    displayName: "FRED 경제지표",
    kind: "custom",
    baseUrl: "https://api.stlouisfed.org/fred",
    homepageUrl: "https://fred.stlouisfed.org",
    docsUrl: "https://fred.stlouisfed.org/docs/api/fred/",
    defaultEnvName: "FRED_API_KEY",
    aliases: ["fred", "fred 경제지표", "stlouisfed"],
  },
  {
    name: "tavily",
    displayName: "Tavily",
    kind: "custom",
    baseUrl: "https://api.tavily.com",
    homepageUrl: "https://tavily.com",
    docsUrl: "https://docs.tavily.com",
    defaultEnvName: "TAVILY_API_KEY",
    aliases: ["tavily", "티빌리"],
  },
  {
    name: "typesafe-ai",
    displayName: "typesafe.ai",
    kind: "custom",
    baseUrl: "https://api.typesafe.ai/v1",
    homepageUrl: "https://typesafe.ai",
    docsUrl: "https://docs.typesafe.ai",
    defaultEnvName: "TYPESAFE_AI_API_KEY",
    aliases: ["typesafe", "typesafe.ai", "typesafe ai"],
  },
  {
    name: "vroid-hub",
    displayName: "Vroid HUB",
    kind: "custom",
    baseUrl: "https://hub.vroid.com/api",
    homepageUrl: "https://hub.vroid.com",
    docsUrl: "https://developer.vroid.com/en/api",
    defaultEnvName: "VROID_CLIENT_SECRET",
    aliases: ["vroid hub", "vroid", "브로이드"],
    credentialFields: [
      { label: "Client ID", envName: "VROID_CLIENT_ID" },
      { label: "Client Secret", envName: "VROID_CLIENT_SECRET" },
    ],
  },
  {
    name: "naver-api-hub",
    displayName: "네이버 HUB",
    kind: "custom",
    baseUrl: "https://naverapihub.apigw.ntruss.com",
    homepageUrl: "https://www.ncloud.com",
    docsUrl: "https://api.ncloud-docs.com/docs/naver-api-hub-overview",
    defaultEnvName: "NAVER_API_HUB_CLIENT_SECRET",
    aliases: ["naver api hub", "네이버 hub", "네이버 api hub", "naver api hub"],
    credentialFields: [
      { label: "Client ID (X-NCP-APIGW-API-KEY-ID)", envName: "NAVER_API_HUB_CLIENT_ID" },
      { label: "Client Secret (X-NCP-APIGW-API-KEY)", envName: "NAVER_API_HUB_CLIENT_SECRET" },
    ],
  },
  {
    name: "naver-cloud",
    displayName: "네이버 클라우드",
    kind: "custom",
    baseUrl: "https://clovastudio.stream.ntruss.com",
    homepageUrl: "https://www.ncloud.com/product/aiService/clovaStudio",
    docsUrl: "https://api.ncloud-docs.com/docs/ai-naver-clovastudio-summary",
    defaultEnvName: "NCP_CLOVASTUDIO_API_KEY",
    aliases: ["네이버 클라우드", "naver cloud", "ncloud", "clova studio", "clovastudio", "하이퍼클로바", "hyperclova"],
    credentialFields: [
      { label: "API Key (X-NCP-CLOVASTUDIO-API-KEY)", envName: "NCP_CLOVASTUDIO_API_KEY" },
      { label: "API Gateway Key (X-NCP-APIGW-API-KEY)", envName: "NCP_APIGW_API_KEY" },
    ],
  },
  {
    name: "naver-maps",
    displayName: "네이버맵",
    kind: "custom",
    baseUrl: "https://naveropenapi.apigw.ntruss.com",
    homepageUrl: "https://www.ncloud.com/product/applicationService/maps",
    docsUrl: "https://api.ncloud-docs.com/docs/ai-naver-mapsgeocoding",
    defaultEnvName: "NAVER_MAPS_CLIENT_SECRET",
    aliases: ["네이버맵", "네이버 지도", "naver maps", "naver map", "maps"],
    credentialFields: [
      { label: "Client ID (X-NCP-APIGW-API-KEY-ID)", envName: "NAVER_MAPS_CLIENT_ID" },
      { label: "Client Secret (X-NCP-APIGW-API-KEY)", envName: "NAVER_MAPS_CLIENT_SECRET" },
    ],
  },
  {
    name: "opendart",
    displayName: "전자공시 OPENDART",
    kind: "custom",
    baseUrl: "https://opendart.fss.or.kr/api",
    homepageUrl: "https://opendart.fss.or.kr",
    docsUrl: "https://opendart.fss.or.kr/guide/main.do",
    defaultEnvName: "OPENDART_API_KEY",
    aliases: ["opendart", "전자공시 opendart", "전자공시", "dart"],
  },
  {
    name: "kiwoom",
    displayName: "키움증권",
    kind: "custom",
    baseUrl: "https://api.kiwoom.com",
    homepageUrl: "https://openapi.kiwoom.com",
    docsUrl: "https://openapi.kiwoom.com/guide/apiguide",
    defaultEnvName: "KIWOOM_APP_SECRET",
    aliases: ["키움증권", "키움", "kiwoom"],
    credentialFields: [
      { label: "App Key", envName: "KIWOOM_APP_KEY" },
      { label: "App Secret", envName: "KIWOOM_APP_SECRET" },
    ],
  },
  {
    name: "tossinvest",
    displayName: "토스증권",
    kind: "custom",
    baseUrl: "https://openapi.tossinvest.com",
    homepageUrl: "https://corp.tossinvest.com/open-api",
    docsUrl: "https://developers.tossinvest.com/docs",
    defaultEnvName: "TOSSINVEST_CLIENT_SECRET",
    aliases: ["토스증권", "토스", "tossinvest", "toss invest"],
    credentialFields: [
      { label: "Client ID", envName: "TOSSINVEST_CLIENT_ID" },
      { label: "Client Secret", envName: "TOSSINVEST_CLIENT_SECRET" },
    ],
  },
  {
    name: "krx",
    displayName: "한국거래소 KRX",
    kind: "custom",
    baseUrl: "https://data.krx.co.kr",
    homepageUrl: "https://data.krx.co.kr",
    docsUrl: "https://data.krx.co.kr/contents/MDC/MAIN/main/index.cmd",
    defaultEnvName: "KRX_API_KEY",
    aliases: ["한국거래소 krx", "한국거래소", "krx", "data.krx"],
  },
  {
    name: "ecos",
    displayName: "한국은행 ECOS",
    kind: "custom",
    baseUrl: "https://ecos.bok.or.kr/api",
    homepageUrl: "https://ecos.bok.or.kr",
    docsUrl: "https://ecos.bok.or.kr/api/#/",
    defaultEnvName: "ECOS_API_KEY",
    aliases: ["한국은행 ecos", "한국은행", "ecos", "bok"],
  },
  {
    name: "koreainvestment",
    displayName: "한국투자증권",
    kind: "custom",
    baseUrl: "https://openapi.koreainvestment.com:9443",
    homepageUrl: "https://securities.koreainvestment.com",
    docsUrl: "https://apiportal.koreainvestment.com",
    defaultEnvName: "KIS_APP_SECRET",
    aliases: ["한국투자증권", "한국투자", "kis", "koreainvestment"],
    credentialFields: [
      { label: "App Key", envName: "KIS_APP_KEY" },
      { label: "App Secret", envName: "KIS_APP_SECRET" },
    ],
  },
];

const CATALOG: ProviderPreset[] = [...PROVIDER_PRESETS, ...KNOWN_PROVIDER_LIBRARY];

export function lookupProviderInfo(nameOrSlug: string): ProviderPreset | undefined {
  const lower = nameOrSlug.trim().toLowerCase();
  if (!lower) return undefined;
  return (
    CATALOG.find((preset) => preset.name === lower) ??
    CATALOG.find((preset) => preset.displayName.toLowerCase() === lower) ??
    CATALOG.find((preset) => preset.aliases.some((alias) => alias === lower))
  );
}

export function detectProviderFromEnvName(envName: string): ProviderPreset | undefined {
  const upper = envName.trim().toUpperCase();
  if (!upper) return undefined;
  const exact = CATALOG.find((preset) => preset.defaultEnvName === upper);
  if (exact) return exact;
  return CATALOG.find((preset) => {
    const stem = preset.defaultEnvName.replace(/_API_KEY$/, "");
    return stem.length > 2 && upper.includes(stem);
  });
}

export function detectProviderFromText(text: string): ProviderPreset | undefined {
  const lower = ` ${text.toLowerCase()} `;
  let best: { preset: ProviderPreset; index: number } | null = null;
  for (const preset of CATALOG) {
    for (const alias of preset.aliases) {
      const index = lower.indexOf(` ${alias} `);
      if (index !== -1 && (best === null || index < best.index)) {
        best = { preset, index };
      }
    }
  }
  return best?.preset;
}
