export interface AppErrorShape {
  code: string;
  message: string;
}

export function toAppError(error: unknown): AppErrorShape {
  if (typeof error === "object" && error !== null) {
    const candidate = error as Record<string, unknown>;
    if (typeof candidate.code === "string" && typeof candidate.message === "string") {
      return { code: candidate.code, message: candidate.message };
    }
    if (typeof candidate.message === "string") {
      return { code: "UNKNOWN", message: candidate.message };
    }
  }
  return { code: "UNKNOWN", message: String(error) };
}

export function errorMessage(error: unknown): string {
  return toAppError(error).message;
}
