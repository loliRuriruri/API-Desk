// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";
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
import { CustomBackgroundLayer } from "../../components/CustomBackgroundLayer";

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

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
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
      applyToMain: true,
      applyToMini: true,
      applyToMonitor: true,
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
          applyToMain: true,
          applyToMini: false,
          applyToMonitor: true,
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
      applyToMain: true,
      applyToMini: false,
      applyToMonitor: true,
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
    expect(settings.customBackground.applyToMain).toBe(true);
    expect(settings.customBackground.applyToMini).toBe(true);
    expect(settings.customBackground.applyToMonitor).toBe(true);
  });

  it("saves custom background settings via saveSetting", async () => {
    const setSettingSpy = vi.spyOn(repo, "setSetting").mockResolvedValue();
    const customBg = {
      enabled: true,
      path: "C:\\bg.png",
      overlayOpacity: 0.5,
      blurPx: 10,
      applyToMain: true,
      applyToMini: true,
      applyToMonitor: false,
    };
    await saveSetting("customBackground", customBg);
    expect(setSettingSpy).toHaveBeenCalledWith("customBackground", JSON.stringify(customBg));
  });
});

describe("CustomBackgroundLayer surface rendering", () => {
  const baseBg = {
    enabled: true,
    path: "C:\\bg.png",
    overlayOpacity: 0.45,
    blurPx: 8,
    applyToMain: true,
    applyToMini: true,
    applyToMonitor: true,
  };
  const testDataUrl = "data:image/png;base64,mock";

  it("renders on main when applyToMain is true", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="main" settings={baseBg} dataUrl={testDataUrl} />
    );
    expect(container.querySelector(".app-custom-bg")).not.toBeNull();
  });

  it("does not render on main when applyToMain is false", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="main" settings={{ ...baseBg, applyToMain: false }} dataUrl={testDataUrl} />
    );
    expect(container.querySelector(".app-custom-bg")).toBeNull();
  });

  it("renders on mini when applyToMini is true", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="mini" settings={baseBg} dataUrl={testDataUrl} />
    );
    expect(container.querySelector(".app-custom-bg")).not.toBeNull();
  });

  it("does not render on mini when applyToMini is false", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="mini" settings={{ ...baseBg, applyToMini: false }} dataUrl={testDataUrl} />
    );
    expect(container.querySelector(".app-custom-bg")).toBeNull();
  });

  it("does not render when enabled is false even if surface flags are true", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="main" settings={{ ...baseBg, enabled: false }} dataUrl={testDataUrl} />
    );
    expect(container.querySelector(".app-custom-bg")).toBeNull();
  });

  it("gracefully falls back and does not render when dataUrl is null", () => {
    const { container } = render(
      <CustomBackgroundLayer surface="main" settings={baseBg} dataUrl={null} />
    );
    expect(container.querySelector(".app-custom-bg")).toBeNull();
  });
});
