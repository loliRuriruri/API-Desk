import { describe, expect, it } from "vitest";
import { parseMemoText } from "../import/memoParser";

const MEMO = `OpenRouter
OPENROUTER_API_KEY=sk-or-testfake-notreal-0001
월 $20 정도

OpenAI
sk-proj-testfake-notreal-0002
`;

describe("parseMemoText", () => {
  it("detects env-style assignments with provider context", () => {
    const result = parseMemoText(MEMO);
    expect(result.credentials).toHaveLength(2);

    const openrouter = result.credentials[0];
    expect(openrouter.providerName).toBe("openrouter");
    expect(openrouter.envName).toBe("OPENROUTER_API_KEY");
    expect(openrouter.monthlyCost).toBe(20);
    expect(openrouter.notes.join(" ")).toContain("OpenRouter");
    expect(openrouter.notes.join(" ")).toContain("월 $20");
    expect(openrouter.secret).toBe("sk-or-testfake-notreal-0001");
    expect(openrouter.masked).not.toContain("testfake");
  });

  it("detects bare keys that follow a provider heading", () => {
    const result = parseMemoText(MEMO);
    const openai = result.credentials[1];
    expect(openai.providerName).toBe("openai");
    expect(openai.envName).toBeNull();
    expect(openai.secret).toBe("sk-proj-testfake-notreal-0002");
  });

  it("never auto-saves and never marks text as parsed when no key exists", () => {
    const result = parseMemoText("DATABASE_URL=postgres://localhost\nPORT=3000\n");
    expect(result.credentials).toHaveLength(0);
    expect(result.unparsed.length).toBeGreaterThan(0);
  });

  it("de-duplicates identical secrets", () => {
    const result = parseMemoText(
      "OPENAI_API_KEY=sk-testfake-notreal-0003\nOPENAI_API_KEY=sk-testfake-notreal-0003\n",
    );
    expect(result.credentials).toHaveLength(1);
  });

  it("does not treat plain config values as secrets", () => {
    const result = parseMemoText("BASE_URL=https://example.com/v1\nMODEL=gpt-4o\n");
    expect(result.credentials).toHaveLength(0);
  });
});
