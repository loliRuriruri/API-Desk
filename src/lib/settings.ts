import { getSetting, setSetting } from "./db/repo";

export type ThemeSetting = "system" | "frutiger-aero" | "dark" | "light";

export interface CustomBackgroundSettings {
  enabled: boolean;
  path: string;
  overlayOpacity: number;
  blurPx: number;
  applyToMain: boolean;
  applyToMini: boolean;
  applyToMonitor: boolean;
}

export const DEFAULT_CUSTOM_BACKGROUND: CustomBackgroundSettings = {
  enabled: false,
  path: "",
  overlayOpacity: 0.45,
  blurPx: 8,
  applyToMain: true,
  applyToMini: true,
  applyToMonitor: true,
};

export interface AppSettings {
  theme: ThemeSetting;
  clipboardClearSecs: number;
  autoHideSecs: number;
  defaultProjectDir: string;
  trayResident: boolean;
  alertsEnabled: boolean;
  alertThresholdPercent: number;
  openMiniOnStart: boolean;
  autoLockMinutes: number;
  customBackground: CustomBackgroundSettings;
}

export const DEFAULT_SETTINGS: AppSettings = {
  theme: "dark",
  clipboardClearSecs: 30,
  autoHideSecs: 30,
  defaultProjectDir: "",
  trayResident: true,
  alertsEnabled: true,
  alertThresholdPercent: 20,
  openMiniOnStart: false,
  autoLockMinutes: 0,
  customBackground: DEFAULT_CUSTOM_BACKGROUND,
};

const KEYS: Record<keyof AppSettings, string> = {
  theme: "theme",
  clipboardClearSecs: "clipboardClearSecs",
  autoHideSecs: "autoHideSecs",
  defaultProjectDir: "defaultProjectDir",
  trayResident: "trayResident",
  alertsEnabled: "alertsEnabled",
  alertThresholdPercent: "alertThresholdPercent",
  openMiniOnStart: "openMiniOnStart",
  autoLockMinutes: "autoLockMinutes",
  customBackground: "customBackground",
};

export async function loadSettings(): Promise<AppSettings> {
  const settings: AppSettings = { ...DEFAULT_SETTINGS };
  await Promise.all(
    (Object.keys(KEYS) as Array<keyof AppSettings>).map(async (field) => {
      const raw = await getSetting(KEYS[field]);
      if (raw === null) return;
      try {
        const parsed: unknown = JSON.parse(raw);
        if (field === "theme") {
          if (
            parsed === "system" ||
            parsed === "dark" ||
            parsed === "light" ||
            parsed === "frutiger-aero"
          ) {
            settings.theme = parsed;
          }
        } else if (field === "clipboardClearSecs" || field === "autoHideSecs") {
          if (typeof parsed === "number" && Number.isFinite(parsed)) {
            settings[field] = parsed;
          }
        } else if (field === "defaultProjectDir" && typeof parsed === "string") {
          settings.defaultProjectDir = parsed;
        } else if (
          (field === "trayResident" ||
            field === "alertsEnabled" ||
            field === "openMiniOnStart") &&
          typeof parsed === "boolean"
        ) {
          settings[field] = parsed;
        } else if (field === "alertThresholdPercent") {
          if (typeof parsed === "number" && Number.isFinite(parsed)) {
            settings.alertThresholdPercent = Math.min(90, Math.max(1, parsed));
          }
        } else if (field === "autoLockMinutes") {
          if (typeof parsed === "number" && Number.isFinite(parsed)) {
            settings.autoLockMinutes = Math.max(0, Math.min(240, parsed));
          }
        } else if (field === "customBackground") {
          if (typeof parsed === "object" && parsed !== null) {
            const p = parsed as Partial<CustomBackgroundSettings>;
            settings.customBackground = {
              enabled:
                typeof p.enabled === "boolean"
                  ? p.enabled
                  : DEFAULT_CUSTOM_BACKGROUND.enabled,
              path:
                typeof p.path === "string"
                  ? p.path
                  : DEFAULT_CUSTOM_BACKGROUND.path,
              overlayOpacity:
                typeof p.overlayOpacity === "number" && Number.isFinite(p.overlayOpacity)
                  ? Math.min(0.95, Math.max(0, p.overlayOpacity))
                  : DEFAULT_CUSTOM_BACKGROUND.overlayOpacity,
              blurPx:
                typeof p.blurPx === "number" && Number.isFinite(p.blurPx)
                  ? Math.min(50, Math.max(0, Math.round(p.blurPx)))
                  : DEFAULT_CUSTOM_BACKGROUND.blurPx,
              applyToMain:
                typeof p.applyToMain === "boolean"
                  ? p.applyToMain
                  : DEFAULT_CUSTOM_BACKGROUND.applyToMain,
              applyToMini:
                typeof p.applyToMini === "boolean"
                  ? p.applyToMini
                  : DEFAULT_CUSTOM_BACKGROUND.applyToMini,
              applyToMonitor:
                typeof p.applyToMonitor === "boolean"
                  ? p.applyToMonitor
                  : DEFAULT_CUSTOM_BACKGROUND.applyToMonitor,
            };
          }
        }
      } catch {
        // ignore malformed stored values and keep the default
      }
    }),
  );
  return settings;
}

export async function saveSetting<K extends keyof AppSettings>(
  field: K,
  value: AppSettings[K],
): Promise<void> {
  await setSetting(KEYS[field], JSON.stringify(value));
}

export function resolveTheme(theme: ThemeSetting): "dark" | "light" | "frutiger-aero" {
  if (theme !== "system") return theme;
  if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return "dark";
}

export function applyTheme(theme: ThemeSetting): void {
  document.documentElement.dataset.theme = resolveTheme(theme);
}

export interface ImportNote {
  id: string;
  createdAt: string;
  text: string;
}

const NOTES_KEY = "import_notes";
const THRESHOLDS_KEY = "alert_thresholds";
const MONITOR_CAPS_KEY = "monitor_caps";

export type AlertThresholds = Record<string, number>;

export async function loadAlertThresholds(): Promise<AlertThresholds> {
  const raw = await getSetting(THRESHOLDS_KEY);
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    const result: AlertThresholds = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      const num = typeof value === "number" ? value : Number(value);
      if (Number.isFinite(num) && num >= 1 && num <= 90) {
        result[key] = Math.round(num);
      }
    }
    return result;
  } catch {
    return {};
  }
}

export async function saveAlertThresholds(thresholds: AlertThresholds): Promise<void> {
  await setSetting(THRESHOLDS_KEY, JSON.stringify(thresholds));
}

export type MonitorCaps = Record<string, number>;

export async function loadMonitorCaps(): Promise<MonitorCaps> {
  const raw = await getSetting(MONITOR_CAPS_KEY);
  if (!raw) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return {};
    const result: MonitorCaps = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      const num = typeof value === "number" ? value : Number(value);
      if (Number.isFinite(num) && num > 0) {
        result[key] = Math.round(num);
      }
    }
    return result;
  } catch {
    return {};
  }
}

export async function saveMonitorCap(monitor: string, cap: number | null): Promise<MonitorCaps> {
  const caps = await loadMonitorCaps();
  if (cap === null || !Number.isFinite(cap) || cap <= 0) {
    delete caps[monitor];
  } else {
    caps[monitor] = Math.round(cap);
  }
  await setSetting(MONITOR_CAPS_KEY, JSON.stringify(caps));
  return caps;
}

export async function loadImportNotes(): Promise<ImportNote[]> {
  const raw = await getSetting(NOTES_KEY);
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return parsed.filter(
        (item): item is ImportNote =>
          typeof item === "object" &&
          item !== null &&
          typeof (item as ImportNote).id === "string" &&
          typeof (item as ImportNote).text === "string",
      );
    }
  } catch {
    // ignore malformed notes
  }
  return [];
}

export async function saveImportNotes(notes: ImportNote[]): Promise<void> {
  await setSetting(NOTES_KEY, JSON.stringify(notes));
}
