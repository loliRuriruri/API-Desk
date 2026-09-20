import { describe, expect, it } from "vitest";
import { maskSecret } from "../mask";

describe("maskSecret", () => {
  it("keeps the provider prefix and the last four characters", () => {
    expect(maskSecret("sk-proj-testfakeabcdefghA82F")).toBe("sk-proj-••••••••••••A82F");
    expect(maskSecret("sk-1234567890abcd4821")).toBe("sk-••••••••••••4821");
  });

  it("never reveals more than the last four characters", () => {
    const secret = "sk-or-testfake-notreal-0001";
    const masked = maskSecret(secret);
    expect(masked.endsWith("0001")).toBe(true);
    expect(masked).not.toContain("testfake");
    expect(masked).not.toContain("notreal");
  });

  it("handles short and empty values", () => {
    expect(maskSecret("")).toBe("");
    expect(maskSecret("a")).toBe("••••");
    expect(maskSecret("abcd")).toBe("••••");
  });

  it("handles keys without a dash", () => {
    expect(maskSecret("AIzaTESTFAKE0000000000000000")).toBe("••••••••••••0000");
  });
});
