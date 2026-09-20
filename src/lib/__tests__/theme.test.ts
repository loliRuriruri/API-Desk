import { describe, expect, it } from "vitest";
import { resolveTheme } from "../settings";

describe("resolveTheme", () => {
  it("keeps explicit themes as-is", () => {
    expect(resolveTheme("dark")).toBe("dark");
    expect(resolveTheme("light")).toBe("light");
    expect(resolveTheme("frutiger-aero")).toBe("frutiger-aero");
  });

  it("falls back to dark for system when matchMedia is unavailable", () => {
    expect(resolveTheme("system")).toBe("dark");
  });
});
