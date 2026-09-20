import { maskSecret } from "../mask";
import { newId } from "../format";
import {
  detectProviderFromEnvName,
  detectProviderFromText,
  type ProviderPreset,
} from "../providers/presets";

export interface DetectedCredential {
  tempId: string;
  providerName: string | null;
  providerDisplayName: string;
  envName: string | null;
  secret: string;
  masked: string;
  baseUrl: string | null;
  accountName: string | null;
  planName: string | null;
  monthlyCost: number | null;
  notes: string[];
}

export interface MemoParseResult {
  credentials: DetectedCredential[];
  unparsed: string[];
}

const SECRET_NAME_HINT = /(API[_-]?KEY|TOKEN|SECRET|ACCESS[_-]?KEY)/i;
const NON_SECRET_NAMES = new Set([
  "DATABASE_URL",
  "PORT",
  "HOST",
  "NODE_ENV",
  "BASE_URL",
  "API_BASE",
  "API_URL",
  "MODEL",
  "MODEL_NAME",
  "LOG_LEVEL",
  "DEBUG",
  "REGION",
  "TIMEOUT",
  "NEXT_PUBLIC_URL",
  "VERCEL_URL",
]);

const BARE_SECRET_PATTERNS = [
  /sk-proj-[A-Za-z0-9_-]{10,}/,
  /sk-or-[A-Za-z0-9_-]{10,}/,
  /sk-[A-Za-z0-9_-]{10,}/,
  /AIza[0-9A-Za-z_-]{20,}/,
  /ghp_[A-Za-z0-9]{20,}/,
  /xai-[A-Za-z0-9]{10,}/,
  /gsk_[A-Za-z0-9]{20,}/,
];

const ASSIGNMENT = /^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)$/;
const PLAN_LINE = /^(?:plan|요금제|플랜)\s*[:=]?\s*(.+)$/i;
const ACCOUNT_LINE = /^(?:account|계정)\s*[:=]\s*(.+)$/i;
const COST = /\$\s*(\d+(?:[.,]\d+)?)/;
const PLAN_HINT = /\b(free|subscription|payg|pay-as-you-go|pay as you go|credits?)\b/i;
const NOTE_LINE = /^[^:]{1,40}:\s*.+$/;

function stripQuotes(value: string): string {
  const trimmed = value.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0];
    const last = trimmed[trimmed.length - 1];
    if ((first === '"' && last === '"') || (first === "'" && last === "'")) {
      return trimmed.slice(1, -1);
    }
  }
  return trimmed;
}

function findBareSecret(line: string): string | null {
  for (const pattern of BARE_SECRET_PATTERNS) {
    const match = pattern.exec(line);
    if (match) return match[0];
  }
  return null;
}

function looksLikeSecretName(name: string): boolean {
  if (NON_SECRET_NAMES.has(name.toUpperCase())) return false;
  return SECRET_NAME_HINT.test(name);
}

function makeCredential(
  preset: ProviderPreset | undefined,
  envName: string | null,
  secret: string,
  notes: string[],
): DetectedCredential {
  return {
    tempId: newId(),
    providerName: preset ? preset.name : null,
    providerDisplayName: preset ? preset.displayName : "Custom",
    envName,
    secret,
    masked: maskSecret(secret),
    baseUrl: preset && preset.baseUrl ? preset.baseUrl : null,
    accountName: null,
    planName: null,
    monthlyCost: null,
    notes: [...notes],
  };
}

export function parseMemoText(text: string): MemoParseResult {
  const credentials: DetectedCredential[] = [];
  const unparsed: string[] = [];
  const seenSecrets = new Set<string>();
  let providerContext: ProviderPreset | undefined;
  let pendingNotes: string[] = [];
  const state: { last: DetectedCredential | null } = { last: null };

  const push = (credential: DetectedCredential) => {
    if (seenSecrets.has(credential.secret)) return;
    seenSecrets.add(credential.secret);
    credentials.push(credential);
    state.last = credential;
  };

  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line.length === 0) continue;

    const assignment = ASSIGNMENT.exec(line);
    if (assignment) {
      const name = assignment[1];
      const value = stripQuotes(assignment[2]);
      const preset =
        detectProviderFromEnvName(name) ?? providerContext ?? detectProviderFromText(name);
      const isSecret =
        (looksLikeSecretName(name) || BARE_SECRET_PATTERNS.some((p) => p.test(value))) &&
        value.length >= 8 &&
        !value.includes(" ");
      if (isSecret) {
        push(makeCredential(preset, name, value, pendingNotes));
        pendingNotes = [];
        providerContext = providerContext ?? preset;
        continue;
      }
      unparsed.push(rawLine);
      continue;
    }

    const planLine = PLAN_LINE.exec(line);
    if (planLine && state.last) {
      state.last.planName = planLine[1].trim();
      continue;
    }

    const accountLine = ACCOUNT_LINE.exec(line);
    if (accountLine && state.last) {
      state.last.accountName = accountLine[1].trim();
      continue;
    }

    const cost = COST.exec(line);
    if (cost && state.last) {
      state.last.monthlyCost = Number(cost[1].replace(",", "."));
      state.last.notes.push(line);
      const plan = PLAN_HINT.exec(line);
      if (plan && !state.last.planName) {
        state.last.planName = plan[1];
      }
      continue;
    }

    const provider = detectProviderFromText(line);
    if (provider && line.length <= 30) {
      providerContext = provider;
      pendingNotes.push(line);
      continue;
    }

    const bare = findBareSecret(line);
    if (bare) {
      push(
        makeCredential(providerContext ?? detectProviderFromText(line), null, bare, pendingNotes),
      );
      pendingNotes = [];
      continue;
    }

    if (NOTE_LINE.test(line) && state.last && !COST.test(line)) {
      state.last.notes.push(line);
      continue;
    }

    unparsed.push(rawLine);
  }

  return { credentials, unparsed };
}
