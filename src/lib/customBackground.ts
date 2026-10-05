import { invoke } from "@tauri-apps/api/core";

export interface CustomBackgroundPayload {
  path: string;
  dataUrl: string;
}

export async function setCustomBackground(sourcePath: string): Promise<CustomBackgroundPayload> {
  return invoke<CustomBackgroundPayload>("custom_background_set", { sourcePath });
}

export async function loadCustomBackground(path?: string | null): Promise<string | null> {
  return invoke<string | null>("custom_background_load", { path: path ?? null });
}

export async function deleteCustomBackground(): Promise<void> {
  await invoke("custom_background_delete");
}
