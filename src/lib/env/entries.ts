export interface EnvEntrySource {
  credential_id: string;
  env_override: string | null;
}

export interface EnvCredentialRef {
  id: string;
  env_name: string | null;
  secret_id: string;
}

export interface EnvEntryItem {
  envName: string;
  secretId: string;
}

export function buildEnvEntries(
  links: EnvEntrySource[],
  credentials: EnvCredentialRef[],
): EnvEntryItem[] {
  const byId = new Map<string, EnvCredentialRef>();
  for (const credential of credentials) byId.set(credential.id, credential);
  const seen = new Set<string>();
  const entries: EnvEntryItem[] = [];
  for (const link of links) {
    const credential = byId.get(link.credential_id);
    if (!credential) continue;
    const envName = (link.env_override ?? credential.env_name ?? "").trim();
    if (envName.length === 0) continue;
    const key = envName.toUpperCase();
    if (seen.has(key)) continue;
    seen.add(key);
    entries.push({ envName, secretId: credential.secret_id });
  }
  return entries;
}
