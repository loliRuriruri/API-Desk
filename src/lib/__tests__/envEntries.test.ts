import { describe, expect, it } from "vitest";
import { buildEnvEntries } from "../env/entries";

describe("buildEnvEntries", () => {
  const credentials = [
    { id: "cred-1", env_name: "OPENAI_API_KEY", secret_id: "secret-1" },
    { id: "cred-2", env_name: "OPENROUTER_API_KEY", secret_id: "secret-2" },
    { id: "cred-3", env_name: null, secret_id: "secret-3" },
  ];

  it("maps project links to env entries", () => {
    const entries = buildEnvEntries(
      [
        { credential_id: "cred-1", env_override: null },
        { credential_id: "cred-2", env_override: null },
      ],
      credentials,
    );
    expect(entries).toEqual([
      { envName: "OPENAI_API_KEY", secretId: "secret-1" },
      { envName: "OPENROUTER_API_KEY", secretId: "secret-2" },
    ]);
  });

  it("applies env overrides and de-duplicates case-insensitively", () => {
    const entries = buildEnvEntries(
      [
        { credential_id: "cred-1", env_override: "PRIMARY_LLM_KEY" },
        { credential_id: "cred-2", env_override: "primary_llm_key" },
      ],
      credentials,
    );
    expect(entries).toEqual([{ envName: "PRIMARY_LLM_KEY", secretId: "secret-1" }]);
  });

  it("skips links without an environment name", () => {
    const entries = buildEnvEntries([{ credential_id: "cred-3", env_override: null }], credentials);
    expect(entries).toEqual([]);
  });

  it("skips links whose credential no longer exists", () => {
    const entries = buildEnvEntries(
      [{ credential_id: "missing", env_override: "SOME_KEY" }],
      credentials,
    );
    expect(entries).toEqual([]);
  });
});
