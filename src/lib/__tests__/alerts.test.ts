import { describe, expect, it } from "vitest";
import {
  collectAlerts,
  credentialRemainingPercent,
  monitorRemainingPercent,
} from "../alerts";

describe("collectAlerts", () => {
  it("alerts only when remaining is at or below the threshold", () => {
    const alerts = collectAlerts(
      [
        { key: "a", label: "OpenRouter", remainingPercent: 15 },
        { key: "b", label: "DeepSeek", remainingPercent: 40 },
        { key: "c", label: "Tavily", remainingPercent: 20 },
        { key: "d", label: "Codex", remainingPercent: null },
      ],
      20,
    );
    expect(alerts.map((alert) => alert.key)).toEqual(["a", "c"]);
    expect(alerts[0].message).toContain("잔여 15%");
  });

  it("sorts by most critical first", () => {
    const alerts = collectAlerts(
      [
        { key: "a", label: "A", remainingPercent: 19 },
        { key: "b", label: "B", remainingPercent: 3 },
      ],
      20,
    );
    expect(alerts.map((alert) => alert.key)).toEqual(["b", "a"]);
  });

  it("honors per-item threshold overrides", () => {
    const alerts = collectAlerts(
      [
        { key: "a", label: "A", remainingPercent: 35, thresholdPercent: 40 },
        { key: "b", label: "B", remainingPercent: 35 },
        { key: "c", label: "C", remainingPercent: 55, thresholdPercent: 50 },
      ],
      20,
    );
    expect(alerts.map((alert) => alert.key)).toEqual(["a"]);
    expect(alerts[0].message).toContain("임계치 40%");
  });
});

describe("percent helpers", () => {
  it("computes credential remaining percent from limit", () => {
    expect(credentialRemainingPercent(8, 10)).toBe(80);
    expect(credentialRemainingPercent(8, 0)).toBeNull();
    expect(credentialRemainingPercent(null, 10)).toBeNull();
  });

  it("computes monitor remaining percent from details", () => {
    expect(
      monitorRemainingPercent(
        "codex",
        JSON.stringify({ primaryUsedPercent: 62, secondaryUsedPercent: 10 }),
      ),
    ).toBe(38);
    expect(
      monitorRemainingPercent("grok", JSON.stringify({ usedPercent: 71 })),
    ).toBe(29);
    expect(
      monitorRemainingPercent("grok", JSON.stringify({ used: 70, monthlyLimit: 0 }), 150),
    ).toBeCloseTo(53.3, 1);
    expect(
      monitorRemainingPercent("grok", JSON.stringify({ used: 70, monthlyLimit: 0 })),
    ).toBeNull();
    expect(
      monitorRemainingPercent(
        "antigravity",
        JSON.stringify({ geminiRemainingPercent: 73, claudeRemainingPercent: 91 }),
      ),
    ).toBe(73);
    expect(
      monitorRemainingPercent(
        "antigravity:a2katsu2@gmail.com",
        JSON.stringify({ geminiRemainingPercent: 12, claudeRemainingPercent: 40 }),
      ),
    ).toBe(12);
    expect(
      monitorRemainingPercent("opencode", JSON.stringify({ remainingPercent: 46 })),
    ).toBe(46);
    expect(monitorRemainingPercent("codex", null)).toBeNull();
    expect(monitorRemainingPercent("codex", "not json")).toBeNull();
  });
});
