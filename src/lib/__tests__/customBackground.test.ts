import { describe, expect, it, vi } from "vitest";
import {
  setCustomBackground,
  loadCustomBackground,
  deleteCustomBackground,
} from "../customBackground";
import {
  DEFAULT_CUSTOM_BACKGROUND,
  DEFAULT_SETTINGS,
  loadSettings,
  saveSetting,
} from "../settings";
import * as repo from "../db/repo";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "custom_background_set") {
      return {
        path: "C:\\appdata\\backgrounds\\custom_background.png",
        dataUrl: "data:image/png;base64,iVBORw0KGgo=",
      };
    }
    if (cmd === "custom_background_load") {
      if (args?.path) {
        return "data:image/png;base64,loadedCustomBg==";
      }
      return null;
    }
    if (cmd === "custom_background_delete") {
      return undefined;
    }
    throw new Error(`Unhandled invoke cmd: ${cmd}`);
  }),
}));

describe("customBackground IPC client", () => {
  it("calls custom_background_set and returns payload", async () => {
    const res = await setCustomBackground("C:\\test\\my_bg.png");
    expect(res.path).toBe("C:\\appdata\\backgrounds\\custom_background.png");
    expect(res.dataUrl).toBe("data:image/png;base64,iVBORw0KGgo=");
  });

  it("calls custom_background_load with path", async () => {
    const res = await loadCustomBackground("C:\\appdata\\backgrounds\\custom_background.png");
    expect(res).toBe("data:image/png;base64,loadedCustomBg==");
  });

  it("calls custom_background_load without path", async () => {
    const res = await loadCustomBackground(null);
    expect(res).toBeNull();
  });

  it("calls custom_background_delete", async () => {
    await expect(deleteCustomBackground()).resolves.toBeUndefined();
  });
});

describe("customBackground settings persistence and fallback", () => {
  it("provides correct default custom background settings", () => {
    expect(DEFAULT_CUSTOM_BACKGROUND).toEqual({
      enabled: false,
      path: "",
      overlayOpacity: 0.45,
      blurPx: 8,
    });
    expect(DEFAULT_SETTINGS.customBackground).toEqual(DEFAULT_CUSTOM_BACKGROUND);
  });

  it("loads and parses stored custom background settings safely", async () => {
    vi.spyOn(repo, "getSetting").mockImplementation(async (key: string) => {
      if (key === "customBackground") {
        return JSON.stringify({
          enabled: true,
          path: "C:\\backgrounds\\custom.jpg",
          overlayOpacity: 0.6,
          blurPx: 12,
        });
      }
      return null;
    });

    const settings = await loadSettings();
    expect(settings.customBackground).toEqual({
      enabled: true,
      path: "C:\\backgrounds\\custom.jpg",
      overlayOpacity: 0.6,
      blurPx: 12,
    });
  });

  it("clamps invalid opacity and blur values to safe bounds", async () => {
    vi.spyOn(repo, "getSetting").mockImplementation(async (key: string) => {
      if (key === "customBackground") {
        return JSON.stringify({
          enabled: true,
          path: "C:\\bg.png",
          overlayOpacity: 1.5, // over max
          blurPx: 999, // over max
        });
      }
      return null;
    });

    const settings = await loadSettings();
    expect(settings.customBackground.overlayOpacity).toBe(0.95);
    expect(settings.customBackground.blurPx).toBe(50);
  });

  it("saves custom background settings via saveSetting", async () => {
    const setSettingSpy = vi.spyOn(repo, "setSetting").mockResolvedValue();
    const customBg = {
      enabled: true,
      path: "C:\\bg.png",
      overlayOpacity: 0.5,
      blurPx: 10,
    };
    await saveSetting("customBackground", customBg);
    expect(setSettingSpy).toHaveBeenCalledWith("customBackground", JSON.stringify(customBg));
  });
});
