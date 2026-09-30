import { describe, expect, it, vi, beforeEach } from "vitest";
import { onUsageRefresh, residentRefreshUsage } from "../desktop";

const mockListen = vi.fn();
const mockInvoke = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: (payload: unknown) => void) => mockListen(event, handler),
  emit: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args: unknown) => mockInvoke(cmd, args),
}));

describe("Background Usage Scheduler Client Integration", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("subscribes to both usage-refresh and usage-snapshot-updated events", async () => {
    mockListen.mockResolvedValue(() => {});
    const handler = vi.fn();

    const cleanup = onUsageRefresh(handler);

    expect(mockListen).toHaveBeenCalledWith("usage-refresh", expect.any(Function));
    expect(mockListen).toHaveBeenCalledWith("usage-snapshot-updated", expect.any(Function));

    cleanup();
  });

  it("triggers handler when usage-refresh fires", async () => {
    const handlers: Record<string, () => void> = {};
    mockListen.mockImplementation((event: string, cb: () => void) => {
      handlers[event] = cb;
      return Promise.resolve(() => {});
    });

    const handler = vi.fn();
    onUsageRefresh(handler);

    // Simulate usage-refresh event from backend resident scheduler
    handlers["usage-refresh"]?.();
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("triggers handler when usage-snapshot-updated fires", async () => {
    const handlers: Record<string, () => void> = {};
    mockListen.mockImplementation((event: string, cb: () => void) => {
      handlers[event] = cb;
      return Promise.resolve(() => {});
    });

    const handler = vi.fn();
    onUsageRefresh(handler);

    // Simulate usage-snapshot-updated event from backend resident scheduler
    handlers["usage-snapshot-updated"]?.();
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("residentRefreshUsage invokes resident_refresh_usage command", async () => {
    mockInvoke.mockResolvedValue(3);

    const updated = await residentRefreshUsage(true);

    expect(mockInvoke).toHaveBeenCalledWith("resident_refresh_usage", { force: true });
    expect(updated).toBe(3);
  });
});
