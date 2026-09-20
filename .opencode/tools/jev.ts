import { tool } from "@opencode-ai/plugin"
import { experimental_evaluate as evaluate } from "ai"
import { createGateway } from "@ai-sdk/gateway"
import { appendFile, mkdir } from "node:fs/promises"
import path from "node:path"

const JEV_VERSION = "0.2.1"

type BackendId = "vercel" | "typesafe" | "openrouter"

const CHAIN: BackendId[] = ["vercel", "typesafe", "openrouter"]
const CHAIN_LABEL = "Vercel → TypeSafe → OpenRouter"

const DEFAULT_THRESHOLD = Number(process.env.JEV_CONFIDENCE_THRESHOLD ?? "0.6")
const REQUEST_TIMEOUT_MS = Number(process.env.JEV_TIMEOUT_MS ?? "20000")

const MODEL_IDS: Record<BackendId, string> = {
  vercel: process.env.JEV_MODEL ?? "typesafe-ai/jev",
  typesafe: process.env.JEV_TYPESAFE_MODEL ?? "jev-latest",
  openrouter: process.env.JEV_OPENROUTER_MODEL ?? "google/gemini-2.5-flash",
}

const TYPESAFE_URL = process.env.JEV_TYPESAFE_URL ?? "https://api.typesafe.ai/v1/systemone"
const OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"

const QUESTIONS = [
  "decide_next_action",
  "evaluate_completion",
  "classify_failure",
  "evaluate_risk",
] as const

type QuestionName = (typeof QUESTIONS)[number]

const ACTION_CHOICES = ["inspect", "implement", "fix", "test", "security_review", "build", "finish"]
const FAILURE_CHOICES = [
  "dependency",
  "typescript",
  "rust",
  "tauri",
  "database",
  "vault",
  "security",
  "test",
  "configuration",
  "environment",
  "unknown",
]

type ChoiceCriteria = Record<string, string[] | { includes: string[] } | null>

type QuestionSpec =
  | { type: "choice"; instructions: string; criteria: ChoiceCriteria }
  | { type: "boolean"; instructions: string; criteria: { true: string; false: string } }

type QuestionSet = Record<string, QuestionSpec>

interface NormalizedAnswer {
  type: "choice" | "boolean"
  choice?: string
  probability?: number
  probabilities?: Record<string, number>
  confidence?: number
}

type AnswerSet = Record<string, NormalizedAnswer>

interface BackendAttempt {
  backend: BackendId
  ok: boolean
  error?: string
}

const SECRET_PATTERNS: Array<[RegExp, string]> = [
  [/sk-proj-[A-Za-z0-9_-]{6,}/g, "[redacted]"],
  [/sk-or-[A-Za-z0-9_-]{6,}/g, "[redacted]"],
  [/sk-[A-Za-z0-9_-]{6,}/g, "[redacted]"],
  [/AIza[0-9A-Za-z_-]{10,}/g, "[redacted]"],
  [/Bearer\s+[A-Za-z0-9._-]{8,}/gi, "Bearer [redacted]"],
  [/(api[_-]?key|apikey|token|secret|password)\s*[=:]\s*["']?[^\s"',]{6,}/gi, "$1=[redacted]"],
]

function redactSecrets(text: string): string {
  let result = text
  for (const [pattern, replacement] of SECRET_PATTERNS) {
    result = result.replace(pattern, replacement)
  }
  return result
}

function clamp01(value: unknown, fallback = 0): number {
  const num = typeof value === "number" ? value : Number(value)
  if (!Number.isFinite(num)) return fallback
  return Math.min(1, Math.max(0, num))
}

function topProbability(probabilities: Record<string, number> | undefined): number {
  if (!probabilities) return 0
  const values = Object.values(probabilities)
  if (values.length === 0) return 0
  return clamp01(Math.max(...values))
}

function decisiveness(probability: number): number {
  return clamp01(Math.abs(probability - 0.5) * 2)
}

function apiKeyFor(backend: BackendId): string | undefined {
  switch (backend) {
    case "vercel":
      return process.env.AI_GATEWAY_API_KEY
    case "typesafe":
      return process.env.TYPESAFE_AI_API_KEY
    case "openrouter":
      return process.env.OPENROUTER_API_KEY
  }
}

function backendSetting(): { setting: string; order: BackendId[] } {
  const raw = (process.env.JEV_BACKEND ?? "auto").trim().toLowerCase()
  if (raw === "vercel" || raw === "typesafe" || raw === "openrouter") {
    return { setting: raw, order: [raw, ...CHAIN.filter((backend) => backend !== raw)] }
  }
  return { setting: "auto", order: CHAIN }
}

function configuredChain(order: BackendId[]): { configured: BackendId[]; missing: BackendId[] } {
  const configured: BackendId[] = []
  const missing: BackendId[] = []
  for (const backend of order) {
    if (apiKeyFor(backend)) configured.push(backend)
    else missing.push(backend)
  }
  return { configured, missing }
}

function buildState(question: string, context: string, phase: string | undefined): string {
  return [
    `Project: API Desk v0.1 (Tauri 2 + React + TypeScript, SQLite metadata + Stronghold secret vault, Local-first Windows desktop app).`,
    `Question: ${question}`,
    phase ? `Phase: ${phase}` : null,
    `State:`,
    context,
  ]
    .filter((line): line is string => line !== null)
    .join("\n")
}

function buildQuestions(question: string): QuestionSet {
  if (question === "decide_next_action") {
    return {
      next_action: {
        type: "choice",
        instructions:
          "Pick the single best next action for the coding agent. Prefer fix when failures exist, security_review when secret or vault handling changed without verification, test before finish, and finish only when every acceptance check passes with real command output.",
        criteria: {
          inspect: ["repository or state is unclear", "need to read files or logs first"],
          implement: ["next work item is clear and unblocked"],
          fix: ["lint, typecheck, test or build failure exists"],
          test: ["implementation finished but not verified by tests"],
          security_review: ["secret, vault, clipboard or .env handling changed"],
          build: ["tests pass and a release build should be verified"],
          finish: ["every acceptance criterion passes with evidence"],
        },
      },
    }
  }

  if (question === "classify_failure") {
    return {
      primary_cause: {
        type: "choice",
        instructions: "Classify the primary cause of the reported failure.",
        criteria: {
          dependency: ["package version or install problem"],
          typescript: ["type error", "tsc diagnostic"],
          rust: ["cargo or rustc diagnostic"],
          tauri: ["tauri configuration, capabilities, ipc or bundling problem"],
          database: ["sqlite, migration or query problem"],
          vault: ["stronghold, secret storage or unlock problem"],
          security: ["secret exposure or unsafe handling"],
          test: ["test assertion failure"],
          configuration: ["wrong path, env or settings value"],
          environment: ["windows, node, cargo or webview2 environment problem"],
          unknown: ["cannot determine"],
        },
      },
    }
  }

  if (question === "evaluate_risk") {
    return {
      secret_exposure_risk: {
        type: "boolean",
        instructions:
          "Does the described next action risk exposing an API key or secret value (reading or writing .env, printing keys, logging secrets, copying secrets)?",
        criteria: {
          true: "secret value could be read, printed or logged",
          false: "no secret value is touched",
        },
      },
      destructive_change: {
        type: "boolean",
        instructions:
          "Could the next action destroy data (deleting the vault, dropping tables, overwriting files, deleting credentials)?",
        criteria: { true: "data could be permanently lost", false: "no destructive write occurs" },
      },
      requires_user_confirmation: {
        type: "boolean",
        instructions:
          "Does this action require explicit user confirmation before the coding agent proceeds?",
        criteria: { true: "user confirmation is required", false: "safe to proceed automatically" },
      },
    }
  }

  return {
    requirements_satisfied: {
      type: "boolean",
      instructions:
        "Are all API Desk v0.1 functional requirements implemented (SQLite metadata, Stronghold vault, provider/account/credential management, project mapping, memo import, .env import/export, reveal/copy, API test)?",
      criteria: {
        true: "all required features exist in code",
        false: "one or more required features are missing",
      },
    },
    tests_satisfied: {
      type: "boolean",
      instructions:
        "Do the tests exist and pass for the required logic (masking, parsers, env merge, clipboard clear, adapter errors)?",
      criteria: {
        true: "tests cover the required logic and pass",
        false: "coverage or passing status is incomplete",
      },
    },
    security_satisfied: {
      type: "boolean",
      instructions:
        "Is the secret handling secure (secrets only in Stronghold, never in SQLite, logs, web storage or previews)?",
      criteria: {
        true: "no plaintext secret storage found",
        false: "a secret-handling risk exists",
      },
    },
    build_satisfied: {
      type: "boolean",
      instructions:
        "Do lint, typecheck, tests and the Windows Tauri build pass based on the reported command results?",
      criteria: {
        true: "all reported commands pass",
        false: "at least one command fails or is unverified",
      },
    },
    ready_to_finish: {
      type: "boolean",
      instructions:
        "Is the project genuinely ready to declare v0.1 complete, based only on evidence in the state? Answer true only when every area above is truly satisfied and verified with real command output.",
      criteria: {
        true: "all areas verified and complete",
        false: "any area is unverified, failing or incomplete",
      },
    },
  }
}

function normalizeFromTypedAnswers(raw: Record<string, unknown>): AnswerSet {
  const answers: AnswerSet = {}
  for (const [key, value] of Object.entries(raw)) {
    if (typeof value !== "object" || value === null) continue
    const entry = value as Record<string, unknown>
    const type = typeof entry.type === "string" ? entry.type : undefined
    if (type === "choice") {
      answers[key] = {
        type: "choice",
        choice: typeof entry.choice === "string" ? entry.choice : undefined,
        probabilities:
          typeof entry.probabilities === "object" && entry.probabilities !== null
            ? (entry.probabilities as Record<string, number>)
            : undefined,
        confidence:
          typeof entry.confidence === "number" ? clamp01(entry.confidence) : undefined,
      }
      continue
    }
    if (type === "boolean") {
      const probability =
        typeof entry.probability === "number"
          ? entry.probability
          : typeof entry.noul === "number"
            ? entry.noul
            : typeof entry.score === "number"
              ? entry.score
              : 0
      answers[key] = {
        type: "boolean",
        probability: clamp01(probability),
        confidence: typeof entry.confidence === "number" ? clamp01(entry.confidence) : undefined,
      }
      continue
    }
    if (type === "score" && typeof entry.score === "number") {
      answers[key] = { type: "boolean", probability: clamp01(entry.score) }
    }
  }
  return answers
}

async function askVercel(
  state: string,
  questions: QuestionSet,
  apiKey: string,
  signal: AbortSignal,
): Promise<AnswerSet> {
  const gateway = createGateway({ apiKey })
  const model = gateway.evaluationModel(MODEL_IDS.vercel)
  const result = await evaluate({
    model,
    state,
    abortSignal: signal,
    maxRetries: 1,
    questions: questions as Parameters<typeof evaluate>[0]["questions"],
  })
  return normalizeFromTypedAnswers(result.answers as Record<string, unknown>)
}

async function askTypesafe(
  state: string,
  questions: QuestionSet,
  apiKey: string,
  signal: AbortSignal,
): Promise<AnswerSet> {
  const response = await fetch(TYPESAFE_URL, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ model: MODEL_IDS.typesafe, state, questions }),
    signal,
  })
  if (!response.ok) {
    const detail = await response.text().catch(() => "")
    throw new Error(`TypeSafe HTTP ${response.status}: ${redactSecrets(detail).slice(0, 200)}`)
  }
  const payload = (await response.json()) as { answers?: Record<string, unknown> }
  return normalizeFromTypedAnswers(payload.answers ?? {})
}

function buildOpenRouterPrompt(question: string, state: string): string {
  const base =
    "You are JEV, a fast supervisor model for the 'API Desk v0.1' Tauri project. " +
    "You never write code. Answer exactly one structured judgment question. " +
    "Return ONLY a single JSON object. No markdown fences, no prose outside JSON."
  if (question === "decide_next_action") {
    return `${base}\nQuestion: decide_next_action\nAllowed choices: ${ACTION_CHOICES.join(", ")}\nJSON schema: {"choice": string, "confidence": number}\nPrefer fix when failures exist, security_review when secrets changed without verification, test before finish, finish only with complete evidence.\n\nState:\n${state}`
  }
  if (question === "classify_failure") {
    return `${base}\nQuestion: classify_failure\nAllowed choices: ${FAILURE_CHOICES.join(", ")}\nJSON schema: {"choice": string, "confidence": number}\n\nState:\n${state}`
  }
  if (question === "evaluate_risk") {
    return `${base}\nQuestion: evaluate_risk\nJSON schema: {"secret_exposure_risk": boolean, "destructive_change": boolean, "requires_user_confirmation": boolean, "confidence": number}\nReading/writing .env, printing keys, deleting the vault, destructive migrations, overwriting files and deleting credentials are high risk.\n\nState:\n${state}`
  }
  return `${base}\nQuestion: evaluate_completion\nScore each area from 0 (no) to 1 (yes).\nJSON schema: {"requirements_satisfied": number, "tests_satisfied": number, "security_satisfied": number, "build_satisfied": number, "ready_to_finish": number, "confidence": number}\nSet ready_to_finish high only when every area is verified with real command output.\n\nState:\n${state}`
}

function extractJson(text: string): Record<string, unknown> | null {
  const trimmed = text.trim().replace(/^```(?:json)?/i, "").replace(/```$/, "").trim()
  const start = trimmed.indexOf("{")
  const end = trimmed.lastIndexOf("}")
  if (start === -1 || end === -1 || end <= start) return null
  try {
    const parsed = JSON.parse(trimmed.slice(start, end + 1))
    return typeof parsed === "object" && parsed !== null
      ? (parsed as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

async function askOpenRouter(
  question: string,
  state: string,
  apiKey: string,
  signal: AbortSignal,
): Promise<AnswerSet> {
  const response = await fetch(OPENROUTER_URL, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      model: MODEL_IDS.openrouter,
      messages: [{ role: "user", content: buildOpenRouterPrompt(question, state) }],
      temperature: 0,
      response_format: { type: "json_object" },
    }),
    signal,
  })
  if (!response.ok) {
    const detail = await response.text().catch(() => "")
    throw new Error(`OpenRouter HTTP ${response.status}: ${redactSecrets(detail).slice(0, 200)}`)
  }
  const payload = (await response.json()) as {
    choices?: Array<{ message?: { content?: string } }>
  }
  const content = payload.choices?.[0]?.message?.content
  const parsed = content ? extractJson(content) : null
  if (!parsed) throw new Error("OpenRouter returned no parseable JSON")

  if (question === "decide_next_action") {
    const choice = typeof parsed.choice === "string" ? parsed.choice.trim().toLowerCase() : ""
    if (!ACTION_CHOICES.includes(choice)) throw new Error("OpenRouter choice out of range")
    return {
      next_action: {
        type: "choice",
        choice,
        confidence: clamp01(parsed.confidence, 0.6),
      },
    }
  }
  if (question === "classify_failure") {
    const choice = typeof parsed.choice === "string" ? parsed.choice.trim().toLowerCase() : ""
    if (!FAILURE_CHOICES.includes(choice)) throw new Error("OpenRouter choice out of range")
    return {
      primary_cause: {
        type: "choice",
        choice,
        confidence: clamp01(parsed.confidence, 0.6),
      },
    }
  }
  if (question === "evaluate_risk") {
    const toProbability = (value: unknown) =>
      value === true ? 1 : value === false ? 0 : clamp01(value)
    const reportedConfidence = clamp01(parsed.confidence, 0.6)
    return {
      secret_exposure_risk: {
        type: "boolean",
        probability: toProbability(parsed.secret_exposure_risk),
        confidence: reportedConfidence,
      },
      destructive_change: {
        type: "boolean",
        probability: toProbability(parsed.destructive_change),
        confidence: reportedConfidence,
      },
      requires_user_confirmation: {
        type: "boolean",
        probability: toProbability(parsed.requires_user_confirmation),
        confidence: reportedConfidence,
      },
    }
  }
  const reportedConfidence = clamp01(parsed.confidence, 0.6)
  return {
    requirements_satisfied: {
      type: "boolean",
      probability: clamp01(parsed.requirements_satisfied),
      confidence: reportedConfidence,
    },
    tests_satisfied: {
      type: "boolean",
      probability: clamp01(parsed.tests_satisfied),
      confidence: reportedConfidence,
    },
    security_satisfied: {
      type: "boolean",
      probability: clamp01(parsed.security_satisfied),
      confidence: reportedConfidence,
    },
    build_satisfied: {
      type: "boolean",
      probability: clamp01(parsed.build_satisfied),
      confidence: reportedConfidence,
    },
    ready_to_finish: {
      type: "boolean",
      probability: clamp01(parsed.ready_to_finish),
      confidence: reportedConfidence,
    },
  }
}

function buildDecision(
  question: string,
  answers: AnswerSet,
): { decision: Record<string, unknown>; confidence: number } {
  if (question === "decide_next_action" || question === "classify_failure") {
    const key = question === "decide_next_action" ? "next_action" : "primary_cause"
    const answer = answers[key]
    const probabilities = answer?.probabilities ?? {}
    const confidence = answer?.confidence ?? topProbability(probabilities)
    return {
      decision: {
        choice: answer?.choice ?? "unknown",
        probabilities,
        confidence,
        reason: null,
      },
      confidence,
    }
  }

  if (question === "evaluate_risk") {
    const read = (key: string): NormalizedAnswer => answers[key] ?? { type: "boolean", probability: 0 }
    const secret = read("secret_exposure_risk")
    const destructive = read("destructive_change")
    const confirm = read("requires_user_confirmation")
    const explicit = secret.confidence ?? destructive.confidence ?? confirm.confidence
    const confidence =
      explicit !== undefined
        ? explicit
        : Math.min(
            decisiveness(secret.probability ?? 0),
            decisiveness(destructive.probability ?? 0),
            decisiveness(confirm.probability ?? 0),
          )
    return {
      decision: {
        secret_exposure_risk: (secret.probability ?? 0) >= 0.5,
        destructive_change: (destructive.probability ?? 0) >= 0.5,
        requires_user_confirmation: (confirm.probability ?? 0) >= 0.5,
        probabilities: {
          secret_exposure_risk: secret.probability ?? 0,
          destructive_change: destructive.probability ?? 0,
          requires_user_confirmation: confirm.probability ?? 0,
        },
        confidence,
        reason: null,
      },
      confidence,
    }
  }

  const area = (key: string) => {
    const answer = answers[key]
    const probability = answer?.probability ?? 0
    return { boolean: probability >= 0.5, probability, confidence: probability }
  }
  const requirements = area("requirements_satisfied")
  const tests = area("tests_satisfied")
  const security = area("security_satisfied")
  const build = area("build_satisfied")
  const ready = area("ready_to_finish")
  const confidence = ready.probability
  return {
    decision: {
      requirements_satisfied: requirements,
      tests_satisfied: tests,
      security_satisfied: security,
      build_satisfied: build,
      ready_to_finish: ready,
      confidence,
      reason: null,
    },
    confidence,
  }
}

async function runWithChain(
  question: string,
  context: string,
  phase: string | undefined,
): Promise<{
  decision: Record<string, unknown>
  confidence: number
  backend: BackendId
  attempts: BackendAttempt[]
}> {
  const state = buildState(question, context, phase)
  const questions = buildQuestions(question)
  const { order } = backendSetting()
  const { configured } = configuredChain(order)
  const attempts: BackendAttempt[] = []

  for (const backend of configured) {
    const apiKey = apiKeyFor(backend)
    if (!apiKey) continue
    const signal = AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    try {
      const answers =
        backend === "vercel"
          ? await askVercel(state, questions, apiKey, signal)
          : backend === "typesafe"
            ? await askTypesafe(state, questions, apiKey, signal)
            : await askOpenRouter(question, state, apiKey, signal)
      const { decision, confidence } = buildDecision(question, answers)
      attempts.push({ backend, ok: true })
      return { decision, confidence: clamp01(confidence), backend, attempts }
    } catch (error) {
      attempts.push({ backend, ok: false, error: redactSecrets(String(error)).slice(0, 240) })
    }
  }

  const detail = attempts.map((attempt) => `${attempt.backend}: ${attempt.error ?? "unavailable"}`).join("; ")
  throw new Error(
    configured.length === 0
      ? "no backend credentials (AI_GATEWAY_API_KEY, TYPESAFE_AI_API_KEY, OPENROUTER_API_KEY)"
      : `all backends failed: ${detail}`,
  )
}

function fallbackDecision(question: string, context: string): Record<string, unknown> {
  const lower = context.toLowerCase()
  const hasFailure = /(error|failed|failure|exit code [1-9])/.test(lower)
  const hasPass = /(pass|passed|0 errors|success)/.test(lower)
  switch (question) {
    case "decide_next_action":
      return {
        choice: hasFailure ? "fix" : hasPass ? "finish" : "implement",
        probabilities: {},
        confidence: 0,
        reason: "JEV unavailable: conservative local heuristic",
      }
    case "evaluate_completion":
      return {
        requirements_satisfied: { boolean: false, probability: 0, confidence: 0 },
        tests_satisfied: { boolean: false, probability: 0, confidence: 0 },
        security_satisfied: { boolean: false, probability: 0, confidence: 0 },
        build_satisfied: { boolean: false, probability: 0, confidence: 0 },
        ready_to_finish: { boolean: false, probability: 0, confidence: 0 },
        confidence: 0,
        reason: "JEV unavailable: coding agent must verify with real command output",
      }
    case "classify_failure":
      return {
        choice: "unknown",
        probabilities: {},
        confidence: 0,
        reason: "JEV unavailable: coding agent must decide",
      }
    case "evaluate_risk":
      return {
        secret_exposure_risk: true,
        destructive_change: true,
        requires_user_confirmation: true,
        confidence: 0,
        reason: "JEV unavailable: assume risk and confirm with the user",
      }
    default:
      return { choice: "unknown", probabilities: {}, confidence: 0, reason: "JEV unavailable" }
  }
}

async function appendTrace(cwd: string, entry: Record<string, unknown>): Promise<string | null> {
  try {
    const dir = path.join(cwd, ".opencode")
    await mkdir(dir, { recursive: true })
    const file = path.join(dir, "jev-trace.jsonl")
    await appendFile(file, `${JSON.stringify(entry)}\n`, "utf8")
    return file
  } catch {
    return null
  }
}

export const status = tool({
  description:
    "Report the JEV Supervisor runtime status: version, backend setting (auto), resolved backend chain order, per-backend credential availability and model IDs. Secret values are never returned.",
  args: {},
  async execute(_args, context) {
    const { setting, order } = backendSetting()
    const { configured, missing } = configuredChain(order)
    const payload = {
      supervisor: "jev",
      version: JEV_VERSION,
      backend: setting,
      backendMode: setting === "auto" ? "auto" : "fixed",
      chain: CHAIN_LABEL,
      chainOrder: order,
      configuredBackends: configured,
      missingBackends: missing,
      models: MODEL_IDS,
      credentialsPresent: {
        vercel: Boolean(apiKeyFor("vercel")),
        typesafe: Boolean(apiKeyFor("typesafe")),
        openrouter: Boolean(apiKeyFor("openrouter")),
      },
      confidenceThreshold: DEFAULT_THRESHOLD,
      environmentOverrides: {
        JEV_BACKEND: process.env.JEV_BACKEND ?? null,
        JEV_MODEL: process.env.JEV_MODEL ?? null,
        JEV_TYPESAFE_MODEL: process.env.JEV_TYPESAFE_MODEL ?? null,
        JEV_OPENROUTER_MODEL: process.env.JEV_OPENROUTER_MODEL ?? null,
      },
      traceFile: path.join(context.worktree ?? context.directory, ".opencode/jev-trace.jsonl"),
      ok: configured.length > 0,
      note:
        configured.length > 0
          ? `auto mode will use ${configured.join(" → ")} in order.`
          : "No backend credentials found: JEV will fall back to local heuristics.",
    }
    return JSON.stringify(payload, null, 2)
  },
})

export default tool({
  description:
    "Ask the JEV Supervisor (v0.2.1) for a structured judgment: decide_next_action, evaluate_completion, " +
    "classify_failure, or evaluate_risk. Backend chain: Vercel AI Gateway → TypeSafe AI → OpenRouter " +
    "(JEV_BACKEND=auto). Never used for code generation. Falls back to local conservative heuristics when " +
    "no backend is reachable.",
  args: {
    question: tool.schema.enum(QUESTIONS).describe("Which supervisor judgment to request"),
    context: tool.schema
      .string()
      .describe(
        "Compact summary of relevant state: repo/TODO/test/build status, the next planned action, or the failure output (never include real secrets)",
      ),
    phase: tool.schema.string().optional().describe("Optional phase label, e.g. 'Phase 4 — Core UI'"),
    threshold: tool.schema
      .number()
      .optional()
      .describe("Optional confidence threshold override; defaults to JEV_CONFIDENCE_THRESHOLD or 0.6"),
  },
  async execute(args, context) {
    const started = Date.now()
    const threshold = clamp01(args.threshold ?? DEFAULT_THRESHOLD)
    const { setting, order } = backendSetting()

    let decision: Record<string, unknown> | null = null
    let confidence = 0
    let fallbackUsed = false
    let error: string | null = null
    let backendUsed: BackendId | null = null
    let attempts: BackendAttempt[] = []

    try {
      const outcome = await runWithChain(args.question as QuestionName, args.context, args.phase)
      decision = outcome.decision
      confidence = clamp01(outcome.confidence)
      backendUsed = outcome.backend
      attempts = outcome.attempts
    } catch (requestError) {
      error = redactSecrets(String(requestError)).slice(0, 300)
    }

    if (!decision) {
      fallbackUsed = true
      decision = fallbackDecision(args.question, args.context)
      confidence = clamp01(decision.confidence)
    }

    const gate = fallbackUsed || confidence < threshold ? "fallback" : "accepted"

    const result: Record<string, unknown> = {
      supervisor: "jev",
      version: JEV_VERSION,
      backendMode: setting,
      backend: backendUsed ?? "none",
      backendChain: order,
      question: args.question,
      phase: args.phase ?? null,
      threshold,
      gate,
      fallbackUsed,
      attempts,
      error,
      durationMs: Date.now() - started,
      decision,
    }

    result.traceFile = await appendTrace(context.worktree ?? context.directory, {
      timestamp: new Date().toISOString(),
      phase: args.phase ?? null,
      question: args.question,
      choice: (decision.choice as string | undefined) ?? null,
      confidence,
      durationMs: result.durationMs,
      fallbackUsed,
      gate,
      backend: backendUsed ?? "none",
      version: JEV_VERSION,
    })

    result.note =
      gate === "accepted"
        ? "JEV confidence is above the threshold: you may follow this decision, but verify with real command output."
        : "Low JEV confidence or fallback: the coding agent must make the final judgment itself."

    return JSON.stringify(result, null, 2)
  },
})
