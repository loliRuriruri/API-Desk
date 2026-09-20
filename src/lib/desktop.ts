import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import {
  disable as autostartDisable,
  enable as autostartEnable,
  isEnabled as autostartIsEnabled,
} from "@tauri-apps/plugin-autostart";

export async function openMiniWindow(): Promise<void> {
  await invoke("open_mini_window_command");
}

export async function showMainWindow(): Promise<void> {
  await invoke("show_main_window");
}

export async function setTrayResident(resident: boolean): Promise<void> {
  await invoke("set_tray_resident", { resident });
}

export async function launchedFromAutostart(): Promise<boolean> {
  try {
    return await invoke<boolean>("launch_startup");
  } catch {
    return false;
  }
}

export function onUsageRefresh(handler: () => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen("usage-refresh", () => handler()).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export async function openMainPage(page: string): Promise<void> {
  await emit("main-navigate", page);
  await invoke("show_main_window");
}

export function onMainNavigate(handler: (page: string) => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen<string>("main-navigate", (event) => handler(event.payload)).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export async function notifySettingsChanged(): Promise<void> {
  await emit("settings-changed");
}

export function onSettingsChanged(handler: () => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen("settings-changed", () => handler()).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export async function notifyVaultChanged(): Promise<void> {
  await emit("vault-changed");
}

export function onVaultChanged(handler: () => void): () => void {
  let unlisten: (() => void) | null = null;
  let disposed = false;
  void listen("vault-changed", () => handler()).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export async function notifyDesktop(title: string, body: string): Promise<void> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      const permission = await requestPermission();
      granted = permission === "granted";
    }
    if (granted) {
      sendNotification({ title, body });
    }
  } catch {
    // notifications are best-effort only
  }
}

export async function getAutostart(): Promise<boolean> {
  try {
    return await autostartIsEnabled();
  } catch {
    return false;
  }
}

export async function setAutostart(enabled: boolean): Promise<void> {
  try {
    if (enabled) {
      await autostartEnable();
    } else {
      await autostartDisable();
    }
  } catch {
    // autostart failures must not break the app
  }
}
