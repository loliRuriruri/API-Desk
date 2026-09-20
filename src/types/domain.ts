export type ProviderKind =
  | "openai"
  | "openrouter"
  | "anthropic"
  | "google"
  | "openai-compatible"
  | "custom";

export type PlanType = "free" | "subscription" | "payg" | "credits" | "custom";
export type AccountStatus = "active" | "inactive" | "unknown" | "expired";
export type CredentialStatus = "active" | "inactive" | "expired" | "error" | "unknown";
export type TestStatus =
  | "connected"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "bad_request"
  | "conflict"
  | "rate_limited"
  | "server_error"
  | "timeout"
  | "network_error"
  | "unsupported";

export type ModelCategory =
  | "LLM"
  | "Coding"
  | "Vision"
  | "Embedding"
  | "Image"
  | "TTS"
  | "STT"
  | "Other";

export interface Provider {
  id: string;
  name: string;
  display_name: string;
  kind: ProviderKind;
  base_url: string | null;
  homepage_url: string | null;
  docs_url: string | null;
  icon: string | null;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface Account {
  id: string;
  provider_id: string;
  name: string;
  plan_type: PlanType;
  plan_name: string | null;
  monthly_cost: number;
  currency: string;
  billing_day: number | null;
  renewal_date: string | null;
  status: AccountStatus;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface Credential {
  id: string;
  account_id: string;
  name: string;
  secret_id: string;
  env_name: string | null;
  status: CredentialStatus;
  created_at: string;
  updated_at: string;
  expires_at: string | null;
  last_tested_at: string | null;
  last_test_status: TestStatus | null;
  notes: string | null;
}

export interface ModelInfo {
  id: string;
  provider_id: string;
  name: string;
  display_name: string;
  category: ModelCategory;
  context_length: number | null;
  input_price: number | null;
  output_price: number | null;
  notes: string | null;
  is_active: number;
  created_at: string;
  updated_at: string;
}

export interface Project {
  id: string;
  name: string;
  path: string | null;
  description: string | null;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface ProjectCredential {
  id: string;
  project_id: string;
  credential_id: string;
  model_id: string | null;
  purpose: string;
  priority: number;
  env_override: string | null;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

export interface Tag {
  id: string;
  name: string;
  color: string | null;
  created_at: string;
}

export interface EntityTag {
  id: string;
  tag_id: string;
  entity_type: "provider" | "account" | "credential" | "model" | "project";
  entity_id: string;
}

export interface SearchResult {
  kind: "provider" | "account" | "credential" | "model" | "project" | "tag";
  id: string;
  label: string;
  sublabel: string;
  extra: string | null;
}
