import type { CredentialStatus, ProviderKind, TestStatus } from "../../types/domain";

export interface HeaderSpec {
  name: string;
  value: string;
}

export interface AuthSpec {
  header: string;
  prefix?: string;
  secretId: string;
}

export interface TestRequestSpec {
  url: string;
  method: string;
  headers: HeaderSpec[];
  auth: AuthSpec | null;
  timeoutMs?: number;
}

export interface TestOutcome {
  status: TestStatus;
  httpStatus: number | null;
  message: string;
  latencyMs: number;
}

export const DEFAULT_TEST_TIMEOUT_MS = 15000;

export function joinUrl(base: string, path: string): string {
  const trimmedBase = base.trim().replace(/\/+$/, "");
  const suffix = path.startsWith("/") ? path : `/${path}`;
  return `${trimmedBase}${suffix}`;
}

export function buildTestRequest(
  kind: ProviderKind,
  baseUrl: string,
  secretId: string,
  timeoutMs: number = DEFAULT_TEST_TIMEOUT_MS,
): TestRequestSpec {
  const base = baseUrl.trim();
  switch (kind) {
    case "anthropic":
      return {
        url: joinUrl(base, "/v1/models"),
        method: "GET",
        headers: [{ name: "anthropic-version", value: "2023-06-01" }],
        auth: { header: "x-api-key", secretId },
        timeoutMs,
      };
    case "google":
      return {
        url: joinUrl(base, "/v1beta/models"),
        method: "GET",
        headers: [],
        auth: { header: "x-goog-api-key", secretId },
        timeoutMs,
      };
    case "openrouter":
      return {
        url: joinUrl(base, "/key"),
        method: "GET",
        headers: [],
        auth: { header: "Authorization", prefix: "Bearer ", secretId },
        timeoutMs,
      };
    case "openai":
    case "openai-compatible":
    case "custom":
    default:
      return {
        url: joinUrl(base, "/models"),
        method: "GET",
        headers: [],
        auth: { header: "Authorization", prefix: "Bearer ", secretId },
        timeoutMs,
      };
  }
}

export const TEST_STATUS_LABELS: Record<TestStatus, string> = {
  connected: "연결됨",
  unauthorized: "인증 실패",
  forbidden: "권한 없음",
  not_found: "엔드포인트 없음",
  bad_request: "잘못된 요청",
  conflict: "충돌",
  rate_limited: "호출 제한",
  server_error: "프로바이더 오류",
  timeout: "네트워크 오류",
  network_error: "네트워크 오류",
  unsupported: "지원되지 않음",
};

export function isTestSuccess(status: TestStatus): boolean {
  return status === "connected";
}

export function credentialStatusForTest(status: TestStatus): CredentialStatus {
  switch (status) {
    case "connected":
    case "rate_limited":
      return "active";
    case "unauthorized":
    case "forbidden":
      return "error";
    default:
      return "unknown";
  }
}
