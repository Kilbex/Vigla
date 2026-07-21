/** Parse newline-delimited repository-relative mission scope paths. */
export function parseScopePaths(raw: string): {
  paths: string[];
  errors: string[];
} {
  const paths: string[] = [];
  const errors: string[] = [];
  const seen = new Set<string>();
  for (const rawLine of raw.split("\n")) {
    const line = rawLine.trim();
    if (line.length === 0) continue;
    if (line.startsWith("/") || /^[A-Za-z]:/.test(line) || line.includes("\\")) {
      errors.push(`absolute path not allowed: ${line}`);
      continue;
    }
    const segments = line.split("/");
    if (segments.some((segment) => segment === "..")) {
      errors.push(`parent-traversal not allowed: ${line}`);
      continue;
    }
    const normalized = segments
      .filter((segment) => segment.length > 0 && segment !== ".")
      .join("/");
    if (normalized.length === 0) {
      errors.push(`repository-root scope is not allowed: ${line}`);
      continue;
    }
    if (seen.has(normalized)) continue;
    seen.add(normalized);
    paths.push(normalized);
  }
  return { paths, errors };
}
