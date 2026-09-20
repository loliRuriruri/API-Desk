export function maskSecret(secret: string): string {
  const chars = Array.from(secret);
  const length = chars.length;
  if (length === 0) return "";
  if (length <= 4) return "••••";
  let prefixLength = 0;
  for (let index = 0; index < Math.min(12, length); index += 1) {
    if (chars[index] === "-") prefixLength = index + 1;
  }
  const tail = chars.slice(length - 4).join("");
  const dots = "•".repeat(Math.max(4, Math.min(12, length - prefixLength - 4)));
  const prefix = chars.slice(0, prefixLength).join("");
  return `${prefix}${dots}${tail}`;
}
