import { describe, expect, it } from "vitest";
import {
  buildTestRequest,
  credentialStatusForTest,
  isTestSuccess,
  joinUrl,
  TEST_STATUS_LABELS,
} from "../providers/adapters";

const FAKE_SECRET_ID = "00000000-0000-0000-0000-000000000000";

describe("provider adapters", () => {
  it("builds OpenAI-compatible /models requests with bearer auth", () => {
    const request = buildTestRequest("openai", "https://api.openai.com/v1", FAKE_SECRET_ID);
    expect(request.url).toBe("https://api.openai.com/v1/models");
    expect(request.method).toBe("GET");
    expect(request.auth).toEqual({
      header: "Authorization",
      prefix: "Bearer ",
      secretId: FAKE_SECRET_ID,
    });
    expect(request.url).not.toContain(FAKE_SECRET_ID);
  });

  it("uses the OpenRouter key endpoint", () => {
    const request = buildTestRequest("openrouter", "https://openrouter.ai/api/v1", FAKE_SECRET_ID);
    expect(request.url).toBe("https://openrouter.ai/api/v1/key");
  });

  it("uses the anthropic version header and x-api-key", () => {
    const request = buildTestRequest("anthropic", "https://api.anthropic.com", FAKE_SECRET_ID);
    expect(request.url).toBe("https://api.anthropic.com/v1/models");
    expect(request.headers).toContainEqual({
      name: "anthropic-version",
      value: "2023-06-01",
    });
    expect(request.auth?.header).toBe("x-api-key");
  });

  it("keeps the Google key out of the URL", () => {
    const request = buildTestRequest(
      "google",
      "https://generativelanguage.googleapis.com",
      FAKE_SECRET_ID,
    );
    expect(request.url).toBe("https://generativelanguage.googleapis.com/v1beta/models");
    expect(request.url).not.toContain("key=");
    expect(request.auth?.header).toBe("x-goog-api-key");
  });

  it("normalizes trailing slashes", () => {
    expect(joinUrl("https://api.example.com/v1/", "/models")).toBe(
      "https://api.example.com/v1/models",
    );
    expect(joinUrl("https://api.example.com/v1", "models")).toBe(
      "https://api.example.com/v1/models",
    );
  });

  it("normalizes outcomes into credential statuses", () => {
    expect(credentialStatusForTest("connected")).toBe("active");
    expect(credentialStatusForTest("rate_limited")).toBe("active");
    expect(credentialStatusForTest("unauthorized")).toBe("error");
    expect(credentialStatusForTest("forbidden")).toBe("error");
    expect(credentialStatusForTest("network_error")).toBe("unknown");
    expect(credentialStatusForTest("timeout")).toBe("unknown");
  });

  it("labels every test status", () => {
    expect(TEST_STATUS_LABELS.connected).toBe("연결됨");
    expect(TEST_STATUS_LABELS.unauthorized).toBe("인증 실패");
    expect(TEST_STATUS_LABELS.rate_limited).toBe("호출 제한");
    expect(TEST_STATUS_LABELS.timeout).toBe("네트워크 오류");
    expect(TEST_STATUS_LABELS.unsupported).toBe("지원되지 않음");
    expect(isTestSuccess("connected")).toBe(true);
    expect(isTestSuccess("unauthorized")).toBe(false);
  });
});
