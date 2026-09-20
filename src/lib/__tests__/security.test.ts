import { readdirSync, readFileSync } from "node:fs";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));

const SECRET_LIKE_PATTERNS = [
  /sk-proj-[A-Za-z0-9_-]{16,}/g,
  /sk-or-[A-Za-z0-9_-]{16,}/g,
  /sk-[A-Za-z0-9]{24,}/g,
  /AIza[0-9A-Za-z_-]{30,}/g,
  /(?:API_KEY|TOKEN|SECRET|PASSWORD)\s*[:=]\s*["']?[A-Za-z0-9_+/-]{24,}/gi,
  /Bearer\s+[A-Za-z0-9._-]{24,}/g,
];

const ALLOWED_FAKE_MARKER = /(test|fake|dummy|example|notreal|placeholder|xxxx|redacted)/i;

function walk(dir: string, extensions: string[], out: string[] = []): string[] {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return out;
  }
  for (const entry of entries) {
    if (["node_modules", "target", "dist", ".git"].includes(entry.name)) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(full, extensions, out);
    } else if (extensions.includes(extname(entry.name))) {
      out.push(full);
    }
  }
  return out;
}

describe("repository security scan", () => {
  it("contains no real-looking API keys or tokens", () => {
    const files = [
      ...walk(join(repoRoot, "src"), [".ts", ".tsx"]),
      ...walk(join(repoRoot, "src-tauri", "src"), [".rs"]),
    ];
    const violations: string[] = [];
    for (const file of files) {
      const lines = readFileSync(file, "utf8").split(/\r?\n/);
      lines.forEach((line, index) => {
        if (ALLOWED_FAKE_MARKER.test(line)) return;
        for (const pattern of SECRET_LIKE_PATTERNS) {
          pattern.lastIndex = 0;
          const match = pattern.exec(line);
          if (match) {
            violations.push(`${file}:${index + 1} matched ${match[0].slice(0, 24)}…`);
          }
        }
      });
    }
    expect(violations).toEqual([]);
  });

  it("never stores secret payloads in the SQLite schema", () => {
    const migrations = readFileSync(
      join(repoRoot, "src-tauri", "src", "migrations.rs"),
      "utf8",
    );
    const credentialsTable = /CREATE TABLE credentials\s*\(([\s\S]*?)\);/.exec(migrations);
    expect(credentialsTable).not.toBeNull();
    const body = credentialsTable?.[1] ?? "";
    expect(body).toContain("secret_id");
    expect(body).not.toMatch(/\bapi_key\b/i);
    expect(body).not.toMatch(/\bsecret_value\b/i);
    expect(body).not.toMatch(/\bvalue\s+(TEXT|BLOB)\b/i);
  });

  it("never persists secrets in web storage", () => {
    const files = walk(join(repoRoot, "src"), [".ts", ".tsx"]).filter(
      (file) => !file.endsWith("security.test.ts"),
    );
    const offenders = files.filter((file) => {
      const content = readFileSync(file, "utf8");
      return /localStorage|sessionStorage|indexedDB/.test(content);
    });
    expect(offenders).toEqual([]);
  });

  it("keeps the vault file outside of the repository sources", () => {
    const suspicious = walk(join(repoRoot, "src-tauri", "src"), [".hold", ".salt"]);
    expect(suspicious).toEqual([]);
  });
});
