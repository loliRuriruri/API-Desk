import { describe, expect, it } from "vitest";
import { gaugesForMonitor, monitorModels, usageGauges } from "../monitorGauges";

describe("usageGauges", () => {
  it("builds credit and key-limit gauges for openrouter", () => {
    const items = usageGauges(
      "openrouter",
      JSON.stringify({
        creditTotal: 10,
        creditUsed: 0.98,
        creditAvailable: 9.02,
        creditRemainingPercent: 90.2,
        key: { data: { usage: 0.92, limit: 1, limit_remaining: 0.08 } },
      }),
    );
    expect(items.map((item) => item.label)).toEqual(["크레딧", "키 한도"]);
    expect(items[0].percent).toBeCloseTo(90.2, 1);
    expect(items[0].hint).toBe("$9.02 / $10.00");
    expect(items[1].percent).toBeCloseTo(8, 1);
    expect(items[1].hint).toBe("$0.08 / $1.00");
  });

  it("ignores other adapters and bad payloads", () => {
    expect(usageGauges("deepseek", JSON.stringify({ balance: 5 }))).toEqual([]);
    expect(usageGauges("openrouter", "not json")).toEqual([]);
    expect(usageGauges("openrouter", null)).toEqual([]);
  });
});

describe("gaugesForMonitor", () => {
  it("builds remaining gauges for codex windows", () => {
    const items = gaugesForMonitor(
      "codex",
      JSON.stringify({ primaryUsedPercent: 62, secondaryUsedPercent: 10 }),
    );
    expect(items.map((item) => item.label)).toEqual(["주간", "5시간"]);
    expect(items[0].percent).toBe(38);
    expect(items[0].mode).toBe("remaining");
    expect(items[1].percent).toBe(90);
  });

  it("builds used gauges for grok bot", () => {
    const items = gaugesForMonitor("grok", JSON.stringify({ usedPercent: 71 }));
    expect(items[0].label).toBe("월간 사용");
    expect(items[0].mode).toBe("used");
    expect(items[0].percent).toBe(71);
  });

  it("uses a reference goal when grok has no monthly limit", () => {
    const details = JSON.stringify({ used: 71, monthlyLimit: 0 });
    expect(gaugesForMonitor("grok", details)).toEqual([]);
    const items = gaugesForMonitor("grok", details, 150);
    expect(items[0].label).toBe("월간 사용");
    expect(items[0].mode).toBe("used");
    expect(items[0].percent).toBe(47.3);
    expect(items[0].hint).toBe("목표 150회");
  });

  it("builds a period gauge for grok build", () => {
    const items = gaugesForMonitor(
      "grok-build",
      JSON.stringify({ periodLabel: "주간", remainingPercent: 62.5, usedPercent: 37.5 }),
    );
    expect(items[0].label).toBe("주간");
    expect(items[0].percent).toBe(62.5);
    expect(items[0].mode).toBe("remaining");
  });

  it("builds antigravity quota group gauges", () => {
    const items = gaugesForMonitor(
      "antigravity",
      JSON.stringify({
        quotaGroups: [
          {
            name: "Gemini Models",
            weeklyRemainingPercent: 73,
            fiveHourRemainingPercent: 55,
          },
          { name: "Claude and GPT models", weeklyRemainingPercent: 91 },
        ],
      }),
    );
    expect(items.map((item) => item.label)).toEqual([
      "Gemini 주간",
      "Gemini 5시간",
      "Claude 주간",
    ]);
    expect(items[0].percent).toBe(73);
    expect(items[2].percent).toBe(91);
  });

  it("builds antigravity gauges for per-account snapshot keys", () => {
    const items = gaugesForMonitor(
      "antigravity:a2katsu2@gmail.com",
      JSON.stringify({
        quotaGroups: [{ name: "Gemini Models", weeklyRemainingPercent: 0, fiveHourRemainingPercent: 100 }],
      }),
    );
    expect(items.map((item) => item.label)).toEqual(["Gemini 주간", "Gemini 5시간"]);
    expect(items[0].percent).toBe(0);
    expect(items[1].percent).toBe(100);
  });

  it("builds opencode rolling/weekly/monthly gauges", () => {
    const items = gaugesForMonitor(
      "opencode",
      JSON.stringify({
        rolling: { percent: 10, resetInSec: 3060 },
        weekly: { percent: 8, resetInSec: 111600 },
        monthly: { percent: 54, resetInSec: 1800000 },
      }),
    );
    expect(items.map((item) => item.label)).toEqual(["롤링", "주간", "월간"]);
    expect(items[0].mode).toBe("used");
    expect(items[2].percent).toBe(54);
    expect(items[0].hint).toContain("리셋");
  });

  it("returns an empty list for unknown monitors or bad json", () => {
    expect(gaugesForMonitor("nvidia", null)).toEqual([]);
    expect(gaugesForMonitor("codex", "not json")).toEqual([]);
  });

  it("extracts model lists from monitor details", () => {
    const models = monitorModels(
      JSON.stringify({
        models: [
          { label: "Gemini 3.8 Flash (High)", remainingPercent: 8 },
          { label: "deepseek-v4.1-flash", remainingPercent: null, meta: "2세션 · $1.29" },
          { notALabel: true },
        ],
      }),
    );
    expect(models.map((model) => model.label)).toEqual([
      "Gemini 3.8 Flash (High)",
      "deepseek-v4.1-flash",
    ]);
    expect(models[0].remaining).toBe(8);
    expect(models[1].meta).toBe("2세션 · $1.29");
    expect(monitorModels(null)).toEqual([]);
  });
});
