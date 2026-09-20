import type { TestStatus } from "../types/domain";
import { TEST_STATUS_LABELS } from "./providers/adapters";

export function testStatusLabel(status: TestStatus | null | undefined): string {
  if (!status) return "미테스트";
  return TEST_STATUS_LABELS[status];
}
