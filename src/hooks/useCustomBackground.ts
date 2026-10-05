import { useEffect, useState } from "react";
import { loadCustomBackground } from "../lib/customBackground";
import {
  DEFAULT_CUSTOM_BACKGROUND,
  loadSettings,
  type CustomBackgroundSettings,
} from "../lib/settings";
import { onSettingsChanged } from "../lib/desktop";

export type BackgroundSurface = "main" | "mini" | "monitor";

export function useCustomBackground(surface: BackgroundSurface = "main") {
  const [bgSettings, setBgSettings] =
    useState<CustomBackgroundSettings>(DEFAULT_CUSTOM_BACKGROUND);
  const [dataUrl, setDataUrl] = useState<string | null>(null);

  const reload = () => {
    let active = true;
    loadSettings()
      .then((settings) => {
        if (!active) return;
        const bg = settings.customBackground ?? DEFAULT_CUSTOM_BACKGROUND;
        setBgSettings(bg);
        const shouldApply =
          bg.enabled &&
          Boolean(bg.path) &&
          (surface === "main"
            ? bg.applyToMain
            : surface === "mini"
              ? bg.applyToMini
              : bg.applyToMonitor);

        if (shouldApply) {
          loadCustomBackground(bg.path)
            .then((url) => {
              if (active) setDataUrl(url);
            })
            .catch(() => {
              if (active) setDataUrl(null);
            });
        } else {
          setDataUrl(null);
        }
      })
      .catch(() => {
        if (active) {
          setBgSettings(DEFAULT_CUSTOM_BACKGROUND);
          setDataUrl(null);
        }
      });
    return () => {
      active = false;
    };
  };

  useEffect(() => {
    const cleanup = reload();
    const unlisten = onSettingsChanged(() => {
      reload();
    });
    return () => {
      cleanup?.();
      unlisten();
    };
  }, [surface]);

  const active = Boolean(
    bgSettings.enabled &&
      dataUrl &&
      (surface === "main"
        ? bgSettings.applyToMain
        : surface === "mini"
          ? bgSettings.applyToMini
          : bgSettings.applyToMonitor),
  );

  useEffect(() => {
    if (active) {
      document.documentElement.dataset.customBg = "true";
    } else {
      delete document.documentElement.dataset.customBg;
    }
  }, [active]);

  return {
    settings: bgSettings,
    dataUrl,
    active,
    reload,
  };
}
