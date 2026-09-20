import type { AccountStatus, CredentialStatus, TestStatus } from "../types/domain";
import { TEST_STATUS_LABELS } from "../lib/providers/adapters";

type BadgeTone = "ok" | "warn" | "danger" | "muted" | "info";

const CREDENTIAL_STATUS_LABELS: Record<CredentialStatus, string> = {
  active: "사용 중",
  inactive: "비활성",
  expired: "만료",
  error: "오류",
  unknown: "미확인",
};

const ACCOUNT_STATUS_LABELS: Record<AccountStatus, string> = {
  active: "사용 중",
  inactive: "비활성",
  unknown: "미확인",
  expired: "만료",
};

export function Badge({ tone, children }: { tone: BadgeTone; children: React.ReactNode }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export function CredentialStatusBadge({ status }: { status: CredentialStatus }) {
  const tone: BadgeTone =
    status === "active"
      ? "ok"
      : status === "error"
        ? "danger"
        : status === "expired"
          ? "warn"
          : "muted";
  return <Badge tone={tone}>{CREDENTIAL_STATUS_LABELS[status]}</Badge>;
}

export function AccountStatusBadge({ status }: { status: AccountStatus }) {
  const tone: BadgeTone =
    status === "active" ? "ok" : status === "expired" ? "warn" : "muted";
  return <Badge tone={tone}>{ACCOUNT_STATUS_LABELS[status]}</Badge>;
}

export function TestStatusBadge({ status }: { status: TestStatus | null }) {
  if (!status) return <Badge tone="muted">미테스트</Badge>;
  const tone: BadgeTone = status === "connected" ? "ok" : status === "rate_limited" ? "warn" : "danger";
  return <Badge tone={tone}>{TEST_STATUS_LABELS[status]}</Badge>;
}
