import { invoke } from "@tauri-apps/api/core";

export type VaultStateName = "uninitialized" | "locked" | "unlocked";

export interface VaultStatus {
  state: VaultStateName;
  vaultPath: string;
  saltPath: string;
}

export const CLEAR_CLIPBOARD_OFF = 0;

export async function vaultStatus(): Promise<VaultStatus> {
  return invoke<VaultStatus>("vault_status");
}

export async function vaultInit(password: string): Promise<void> {
  await invoke("vault_init", { password });
}

export async function vaultUnlock(password: string): Promise<void> {
  await invoke("vault_unlock", { password });
}

export async function vaultLock(): Promise<void> {
  await invoke("vault_lock");
}

export async function vaultReset(confirm: string): Promise<void> {
  await invoke("vault_reset", { confirm });
}

export async function secretStore(secretId: string, value: string): Promise<void> {
  await invoke("secret_store", { secretId, value });
}

export async function secretDelete(secretId: string): Promise<void> {
  await invoke("secret_delete", { secretId });
}

export async function secretReveal(secretId: string): Promise<string> {
  return invoke<string>("secret_reveal", { secretId });
}

export async function secretMask(secretId: string): Promise<string> {
  return invoke<string>("secret_mask", { secretId });
}

export async function secretMasks(secretIds: string[]): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("secret_masks", { secretIds });
}

export async function secretExists(secretId: string): Promise<boolean> {
  return invoke<boolean>("secret_exists", { secretId });
}

export async function secretCopy(secretId: string, clearAfterSecs: number): Promise<void> {
  await invoke("secret_copy", { secretId, clearAfterSecs });
}
