import { useCustomBackground, type BackgroundSurface } from "../hooks/useCustomBackground";
import type { CustomBackgroundSettings } from "../lib/settings";

interface CustomBackgroundLayerProps {
  surface?: BackgroundSurface;
  settings?: CustomBackgroundSettings;
  dataUrl?: string | null;
}

export function CustomBackgroundLayer({
  surface = "main",
  settings: propSettings,
  dataUrl: propDataUrl,
}: CustomBackgroundLayerProps) {
  const hook = useCustomBackground(surface);
  const settings = propSettings ?? hook.settings;
  const dataUrl = propDataUrl !== undefined ? propDataUrl : hook.dataUrl;

  const shouldRender =
    settings.enabled &&
    Boolean(dataUrl) &&
    (surface === "main"
      ? settings.applyToMain
      : surface === "mini"
        ? settings.applyToMini
        : settings.applyToMonitor);

  if (!shouldRender || !dataUrl) {
    return null;
  }

  return (
    <div className="app-custom-bg" aria-hidden="true">
      <div
        className="app-custom-bg-image"
        style={{
          backgroundImage: `url(${dataUrl})`,
          filter: `blur(${settings.blurPx}px)`,
        }}
      />
      <div
        className="app-custom-bg-overlay"
        style={{ opacity: settings.overlayOpacity }}
      />
    </div>
  );
}
