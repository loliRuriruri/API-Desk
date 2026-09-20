import * as repo from "./db/repo";

interface MetadataExport {
  app: string;
  version: string;
  exportedAt: string;
  note: string;
  providers: unknown[];
  accounts: unknown[];
  credentials: unknown[];
  models: unknown[];
  projects: unknown[];
  project_links: Record<string, unknown[]>;
  tags: unknown[];
}

export async function buildMetadataExport(): Promise<string> {
  const [providers, accounts, credentials, models, projects, tags] = await Promise.all([
    repo.listProviders(),
    repo.listAccounts(),
    repo.listCredentialsWithContext(),
    repo.listModels(),
    repo.listProjects(),
    repo.listTags(),
  ]);

  const projectLinks: Record<string, unknown[]> = {};
  for (const project of projects) {
    projectLinks[project.id] = await repo.listProjectLinks(project.id);
  }

  const payload: MetadataExport = {
    app: "API Desk",
    version: "0.1.0",
    exportedAt: new Date().toISOString(),
    note:
      "메타데이터 내보내기입니다. API 키 값은 암호화된 Stronghold Vault에 있으며 secret_id로 참조됩니다.",
    providers,
    accounts,
    credentials,
    models,
    projects,
    project_links: projectLinks,
    tags,
  };
  return JSON.stringify(payload, null, 2);
}
