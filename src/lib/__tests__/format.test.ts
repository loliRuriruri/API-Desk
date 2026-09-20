import { describe, expect, it } from "vitest";
import { formatRelativeTime } from "../format";

function ago(seconds: number): string {
  return new Date(Date.now() - seconds * 1000).toISOString();
}

describe("formatRelativeTime", () => {
  it("formats recent timestamps compactly", () => {
    expect(formatRelativeTime(ago(5))).toBe("방금 전");
    expect(formatRelativeTime(ago(5 * 60))).toBe("5분 전");
    expect(formatRelativeTime(ago(3 * 3600))).toBe("3시간 전");
    expect(formatRelativeTime(ago(2 * 86400))).toBe("2일 전");
  });

  it("falls back for invalid values", () => {
    expect(formatRelativeTime(null)).toBe("—");
    expect(formatRelativeTime("not a date")).toBe("not a date");
  });
});
