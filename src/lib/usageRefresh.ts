import {
  listAllCredentialFields,
  listCredentialsWithContext,
  listUsageSnapshots,
  upsertUsageSnapshot,
} from "./db/repo";
import { usageAdapterFor, usageExtraSecretFor } from "./providers/usage";
import { fetchUsage } from "./system";

export async function refreshStaleUsageSnapshots(maxAgeMs: number): Promise<number> {
  if (maxAgeMs <= 0) return 0;
  const [snapshots, credentials, fields] = await Promise.all([
    listUsageSnapshots(),
    listCredentialsWithContext(),
    listAllCredentialFields(),
  ]);
  const byId = new Map(credentials.map((credential) => [credential.id, credential]));
  const now = Date.now();
  let updated = 0;
  for (const snapshot of snapshots) {
    const fetched = new Date(snapshot.fetched_at).getTime();
    const age = Number.isNaN(fetched) ? Number.POSITIVE_INFINITY : now - fetched;
    if (age < maxAgeMs) continue;
    const credential = byId.get(snapshot.credential_id);
    if (!credential) continue;
    const adapter = usageAdapterFor(
      credential.provider_name,
      credential.provider_display_name,
      credential.provider_base_url,
    );
    if (!adapter) continue;
    try {
      const credentialFields = fields.filter((field) => field.credential_id === credential.id);
      const outcome = await fetchUsage(
        adapter,
        credential.provider_base_url,
        credential.secret_id,
        usageExtraSecretFor(adapter, credentialFields),
      );
      await upsertUsageSnapshot({
        credential_id: credential.id,
        adapter,
        status: outcome.status,
        summary: outcome.summary || outcome.message || "",
        used: outcome.used,
        limit_total: outcome.limit,
        remaining: outcome.remaining,
        currency: outcome.currency,
        details_json: outcome.details ? JSON.stringify(outcome.details) : null,
        fetched_at: new Date().toISOString(),
      });
      updated += 1;
    } catch {
      // best effort: a locked vault or a provider error must not break startup
    }
  }
  return updated;
}
