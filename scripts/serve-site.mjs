#!/usr/bin/env node

import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const siteRoot = path.join(repoRoot, "site", "dist");
const port = Number.parseInt(process.env.PORT ?? "5190", 10);

if (!Number.isInteger(port) || port < 1 || port > 65_535) {
  throw new Error(`invalid PORT: ${process.env.PORT}`);
}

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".png", "image/png"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
  [".webp", "image/webp"],
  [".woff2", "font/woff2"],
]);

function resolveRequestPath(requestUrl) {
  const url = new URL(requestUrl ?? "/", `http://127.0.0.1:${port}`);
  let pathname = decodeURIComponent(url.pathname);
  if (pathname === "/Vigla") pathname = "/";
  if (pathname.startsWith("/Vigla/")) pathname = pathname.slice("/Vigla".length);

  const candidate = path.resolve(siteRoot, `.${pathname}`);
  if (candidate !== siteRoot && !candidate.startsWith(`${siteRoot}${path.sep}`)) {
    return null;
  }
  return candidate;
}

const server = createServer(async (request, response) => {
  if (request.method !== "GET" && request.method !== "HEAD") {
    response.writeHead(405, { Allow: "GET, HEAD" });
    response.end();
    return;
  }

  try {
    let filePath = resolveRequestPath(request.url);
    if (!filePath) {
      response.writeHead(403);
      response.end("Forbidden\n");
      return;
    }

    let metadata = await stat(filePath);
    if (metadata.isDirectory()) {
      filePath = path.join(filePath, "index.html");
      metadata = await stat(filePath);
    }
    if (!metadata.isFile()) throw new Error("not a file");

    response.writeHead(200, {
      "Content-Length": metadata.size,
      "Content-Type": contentTypes.get(path.extname(filePath)) ?? "application/octet-stream",
      "X-Content-Type-Options": "nosniff",
    });
    if (request.method === "HEAD") {
      response.end();
      return;
    }
    createReadStream(filePath).pipe(response);
  } catch {
    response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Not found\n");
  }
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`site preview: http://127.0.0.1:${port}/Vigla/\n`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
