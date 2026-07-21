#!/usr/bin/env node

import { appendFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const API_ROOT = "https://api.github.com";
const API_VERSION = "2022-11-28";

function redact(value, secret) {
  const text = String(value);
  return secret ? text.split(secret).join("[REDACTED]") : text;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export async function githubRequest(
  endpoint,
  {
    token,
    fetchImpl = fetch,
    attempts = 3,
    delayImpl = delay,
    requestTimeoutMs = 15_000,
  } = {},
) {
  if (!token) {
    throw new Error("GITHUB_TOKEN is required");
  }

  const url = endpoint.startsWith("http") ? endpoint : `${API_ROOT}${endpoint}`;
  let response;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      response = await fetchImpl(url, {
        headers: {
          Accept: "application/vnd.github+json",
          Authorization: `Bearer ${token}`,
          "X-GitHub-Api-Version": API_VERSION,
          "User-Agent": "vigla-traffic-snapshot",
        },
        signal: AbortSignal.timeout(requestTimeoutMs),
      });
    } catch (error) {
      if (attempt === attempts) {
        const message = error instanceof Error ? error.message : String(error);
        throw new Error(
          `GitHub API network failure for ${endpoint}: ${redact(message, token)}`,
        );
      }
      await delayImpl(attempt * 500);
      continue;
    }

    if (response.ok) {
      return response;
    }

    const retryable = response.status === 429 || response.status >= 500;
    if (!retryable || attempt === attempts) {
      const body = redact(await response.text(), token).slice(0, 500);
      throw new Error(`GitHub API ${response.status} for ${endpoint}: ${body}`);
    }

    const retryAfterHeader = response.headers.get("retry-after");
    const retryAfterSeconds =
      retryAfterHeader === null || retryAfterHeader.trim() === ""
        ? Number.NaN
        : Number(retryAfterHeader);
    await delayImpl(
      Number.isFinite(retryAfterSeconds) && retryAfterSeconds >= 0
        ? retryAfterSeconds * 1_000
        : attempt * 500,
    );
  }

  throw new Error(`GitHub API request failed for ${endpoint}`);
}

async function githubJson(endpoint, options) {
  const response = await githubRequest(endpoint, options);
  return { response, data: await response.json() };
}

async function collectReleases(repository, options) {
  const releases = [];
  for (let page = 1; ; page += 1) {
    const { data } = await githubJson(
      `/repos/${repository}/releases?per_page=100&page=${page}`,
      options,
    );
    releases.push(...data);
    if (data.length < 100) break;
  }

  return releases.map((release) => ({
    id: release.id,
    tag_name: release.tag_name,
    published_at: release.published_at,
    assets: release.assets.map((asset) => ({
      id: asset.id,
      name: asset.name,
      download_count: asset.download_count,
      updated_at: asset.updated_at,
    })),
  }));
}

export async function collectSnapshot({ repository, token, fetchImpl = fetch, now = new Date() }) {
  if (!/^[^/\s]+\/[^/\s]+$/.test(repository)) {
    throw new Error("repository must use the owner/name form");
  }

  const options = { token, fetchImpl };
  const [views, clones, referrers, paths, repositoryData, releases] = await Promise.all([
    githubJson(`/repos/${repository}/traffic/views?per=day`, options).then(({ data }) => data),
    githubJson(`/repos/${repository}/traffic/clones?per=day`, options).then(({ data }) => data),
    githubJson(`/repos/${repository}/traffic/popular/referrers`, options).then(({ data }) => data),
    githubJson(`/repos/${repository}/traffic/popular/paths`, options).then(({ data }) => data),
    githubJson(`/repos/${repository}`, options).then(({ data }) => data),
    collectReleases(repository, options),
  ]);

  return {
    schema_version: 1,
    captured_at: now.toISOString(),
    repository,
    traffic: {
      views,
      clones,
      popular_referrers: referrers,
      popular_paths: paths,
    },
    repository_stats: {
      stargazers_count: repositoryData.stargazers_count,
      forks_count: repositoryData.forks_count,
      watchers_count: repositoryData.subscribers_count,
      open_issues_count: repositoryData.open_issues_count,
    },
    releases,
  };
}

export async function appendSnapshot(outputPath, snapshot) {
  await mkdir(path.dirname(outputPath), { recursive: true });
  await appendFile(outputPath, `${JSON.stringify(snapshot)}\n`, { encoding: "utf8", mode: 0o600 });
}

function mergedDaily(records, key) {
  const byTimestamp = new Map();
  for (const record of [...records].sort((a, b) => a.captured_at.localeCompare(b.captured_at))) {
    for (const point of record.traffic[key][key] ?? []) {
      byTimestamp.set(point.timestamp, point);
    }
  }
  return [...byTimestamp.values()].sort((a, b) => a.timestamp.localeCompare(b.timestamp));
}

function total(points, field) {
  return points.reduce((sum, point) => sum + Number(point[field] ?? 0), 0);
}

function calendarWindowEndingAt(timestamp, days) {
  const end = new Date(timestamp);
  return Array.from({ length: days }, (_, index) => {
    const day = new Date(end);
    day.setUTCDate(end.getUTCDate() - (days - index - 1));
    return day.toISOString().replace(/\d{2}:\d{2}:\d{2}\.\d{3}Z$/, "00:00:00Z");
  });
}

function markdownTable(headers, rows) {
  if (rows.length === 0) return "_No data returned._";
  const separator = headers.map(() => "---");
  return [headers, separator, ...rows]
    .map((row) =>
      `| ${row
        .map((cell) => String(cell).replaceAll("|", "\\|").replaceAll("\n", " "))
        .join(" | ")} |`,
    )
    .join("\n");
}

export function buildSummary(records) {
  if (records.length === 0) {
    throw new Error("no traffic snapshots found");
  }

  const sorted = [...records].sort((a, b) => a.captured_at.localeCompare(b.captured_at));
  const latest = sorted.at(-1);
  const allViews = mergedDaily(sorted, "views");
  const allClones = mergedDaily(sorted, "clones");
  const dailyTimestamps = calendarWindowEndingAt(latest.captured_at, 7);
  const timestampSet = new Set(dailyTimestamps);
  const views = allViews.filter((point) => timestampSet.has(point.timestamp));
  const clones = allClones.filter((point) => timestampSet.has(point.timestamp));
  const downloads = latest.releases
    .flatMap((release) => release.assets.map((asset) => ({ release: release.tag_name, ...asset })))
    .sort((a, b) => b.download_count - a.download_count);

  const startDate = dailyTimestamps[0]?.slice(0, 10) ?? "n/a";
  const endDate = dailyTimestamps.at(-1)?.slice(0, 10) ?? "n/a";

  return `# Vigla GitHub traffic snapshot

Captured: ${latest.captured_at}<br>
Repository: ${latest.repository}<br>
Daily window: ${startDate} to ${endDate} (seven consecutive UTC calendar days)

## Seven-day totals

| Metric | Total | Unique |
| --- | ---: | ---: |
| Repository views | ${total(views, "count")} | ${total(views, "uniques")} |
| Git clones | ${total(clones, "count")} | ${total(clones, "uniques")} |
| Current stars | ${latest.repository_stats.stargazers_count} | — |

Daily points are deduplicated by timestamp before totals are calculated, so overlapping 14-day API responses are not double-counted. Daily unique counts are summed and should not be interpreted as de-duplicated people across the full week.

## Daily traffic

${markdownTable(
    ["Date", "Views", "Unique views", "Clones", "Unique clones"],
    dailyTimestamps
      .map((timestamp) => {
        const view = views.find((point) => point.timestamp === timestamp);
        const clone = clones.find((point) => point.timestamp === timestamp);
        return [
          timestamp.slice(0, 10),
          String(view?.count ?? 0),
          String(view?.uniques ?? 0),
          String(clone?.count ?? 0),
          String(clone?.uniques ?? 0),
        ];
      }),
  )}

## Top referrers (latest rolling 14-day response)

${markdownTable(
    ["Referrer", "Views", "Unique"],
    latest.traffic.popular_referrers.map((item) => [item.referrer, item.count, item.uniques]),
  )}

## Popular content (latest rolling 14-day response)

${markdownTable(
    ["Path", "Title", "Views", "Unique"],
    latest.traffic.popular_paths.map((item) => [item.path, item.title, item.count, item.uniques]),
  )}

## Release asset downloads (lifetime counters)

${markdownTable(
    ["Release", "Asset", "Downloads"],
    downloads.map((asset) => [asset.release, asset.name, asset.download_count]),
  )}
`;
}

export async function readSnapshots(inputDirectory) {
  const files = (await readdir(inputDirectory))
    .filter((name) => /^traffic-\d{4}\.jsonl$/.test(name))
    .sort();
  const records = [];
  for (const file of files) {
    const contents = await readFile(path.join(inputDirectory, file), "utf8");
    for (const [index, line] of contents.split("\n").entries()) {
      if (!line.trim()) continue;
      try {
        records.push(JSON.parse(line));
      } catch (error) {
        throw new Error(`${file}:${index + 1}: invalid JSON: ${error.message}`);
      }
    }
  }
  return records;
}

function parseArguments(argv) {
  const [command, ...rest] = argv;
  const options = {};
  for (let index = 0; index < rest.length; index += 2) {
    const flag = rest[index];
    const value = rest[index + 1];
    if (!flag?.startsWith("--") || value === undefined) {
      throw new Error(`invalid argument: ${flag ?? "<missing>"}`);
    }
    options[flag.slice(2)] = value;
  }
  return { command, options };
}

async function main() {
  const { command, options } = parseArguments(process.argv.slice(2));
  if (command === "snapshot") {
    if (!options.repo || !options.output) {
      throw new Error("usage: github-traffic.mjs snapshot --repo owner/name --output file.jsonl");
    }
    const snapshot = await collectSnapshot({
      repository: options.repo,
      token: process.env.GITHUB_TOKEN || process.env.GH_TOKEN,
    });
    await appendSnapshot(options.output, snapshot);
    console.log(`Captured ${snapshot.repository} traffic at ${snapshot.captured_at}`);
    return;
  }

  if (command === "summary") {
    if (!options["input-dir"] || !options.output) {
      throw new Error("usage: github-traffic.mjs summary --input-dir data --output report.md");
    }
    const records = await readSnapshots(options["input-dir"]);
    const summary = buildSummary(records);
    await mkdir(path.dirname(options.output), { recursive: true });
    await writeFile(options.output, summary, { encoding: "utf8", mode: 0o600 });
    console.log(`Wrote ${options.output}`);
    return;
  }

  throw new Error("usage: github-traffic.mjs <snapshot|summary> [options]");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(`traffic snapshot failed: ${error.message}`);
    process.exitCode = 1;
  });
}
