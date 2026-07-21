import assert from "node:assert/strict";
import test from "node:test";

import { buildSummary, collectSnapshot, githubRequest } from "./github-traffic.mjs";

function jsonResponse(data, status = 200, headers = {}) {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

test("collectSnapshot captures every launch metric without storing credentials", async () => {
  const requested = [];
  const fetchImpl = async (url, options) => {
    requested.push(url);
    assert.equal(options.headers.Authorization, "Bearer secret-token");
    if (url.includes("traffic/views")) {
      return jsonResponse({ count: 3, uniques: 2, views: [] });
    }
    if (url.includes("traffic/clones")) {
      return jsonResponse({ count: 1, uniques: 1, clones: [] });
    }
    if (url.includes("popular/referrers")) return jsonResponse([]);
    if (url.includes("popular/paths")) return jsonResponse([]);
    if (url.includes("/releases?")) return jsonResponse([]);
    return jsonResponse({
      stargazers_count: 4,
      forks_count: 2,
      subscribers_count: 1,
      open_issues_count: 3,
    });
  };

  const snapshot = await collectSnapshot({
    repository: "Kilbex/Vigla",
    token: "secret-token",
    fetchImpl,
    now: new Date("2026-07-21T12:00:00Z"),
  });

  assert.equal(snapshot.repository, "Kilbex/Vigla");
  assert.equal(snapshot.repository_stats.stargazers_count, 4);
  assert.equal(JSON.stringify(snapshot).includes("secret-token"), false);
  assert.equal(requested.length, 6);
});

test("githubRequest redacts a credential returned in an API error", async () => {
  await assert.rejects(
    githubRequest("/broken", {
      token: "do-not-log-me",
      attempts: 1,
      fetchImpl: async () => new Response("bad do-not-log-me", { status: 403 }),
    }),
    (error) => {
      assert.equal(error.message.includes("do-not-log-me"), false);
      assert.match(error.message, /\[REDACTED\]/);
      return true;
    },
  );
});

test("githubRequest retries a transient network failure without logging the token", async () => {
  let attempts = 0;
  const response = await githubRequest("/eventually-ready", {
    token: "retry-secret",
    attempts: 2,
    delayImpl: async () => {},
    fetchImpl: async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("socket closed near retry-secret");
      return jsonResponse({ ok: true });
    },
  });

  assert.equal(response.status, 200);
  assert.equal(attempts, 2);
});

test("githubRequest uses fallback backoff when Retry-After is absent", async () => {
  const delays = [];
  let attempts = 0;
  const response = await githubRequest("/eventually-ready", {
    token: "retry-secret",
    attempts: 2,
    delayImpl: async (milliseconds) => delays.push(milliseconds),
    fetchImpl: async () => {
      attempts += 1;
      return attempts === 1
        ? new Response("try later", { status: 503 })
        : jsonResponse({ ok: true });
    },
  });

  assert.equal(response.status, 200);
  assert.deepEqual(delays, [500]);
});

test("githubRequest aborts a stalled request", async () => {
  await assert.rejects(
    githubRequest("/stalled", {
      token: "timeout-secret",
      attempts: 1,
      requestTimeoutMs: 5,
      fetchImpl: async (_url, { signal }) =>
        new Promise((_resolve, reject) => {
          const keepAlive = setTimeout(() => {
            reject(new Error("request timeout signal did not fire"));
          }, 100);
          signal.addEventListener(
            "abort",
            () => {
              clearTimeout(keepAlive);
              reject(signal.reason);
            },
            { once: true },
          );
        }),
    }),
    /GitHub API network failure/,
  );
});

test("summary deduplicates overlapping daily snapshots", () => {
  const base = {
    schema_version: 1,
    repository: "Kilbex/Vigla",
    traffic: {
      popular_referrers: [],
      popular_paths: [],
      views: { views: [] },
      clones: { clones: [] },
    },
    repository_stats: { stargazers_count: 1 },
    releases: [],
  };
  const first = structuredClone(base);
  first.captured_at = "2026-07-20T08:00:00Z";
  first.traffic.views.views = [
    { timestamp: "2026-07-19T00:00:00Z", count: 10, uniques: 8 },
    { timestamp: "2026-07-20T00:00:00Z", count: 20, uniques: 15 },
  ];
  first.traffic.clones.clones = [
    { timestamp: "2026-07-20T00:00:00Z", count: 2, uniques: 2 },
  ];

  const second = structuredClone(base);
  second.captured_at = "2026-07-21T08:00:00Z";
  second.repository_stats.stargazers_count = 3;
  second.traffic.views.views = [
    { timestamp: "2026-07-20T00:00:00Z", count: 21, uniques: 16 },
    { timestamp: "2026-07-21T00:00:00Z", count: 7, uniques: 6 },
  ];
  second.traffic.clones.clones = [
    { timestamp: "2026-07-20T00:00:00Z", count: 2, uniques: 2 },
    { timestamp: "2026-07-21T00:00:00Z", count: 1, uniques: 1 },
  ];

  const report = buildSummary([first, second]);
  assert.match(report, /\| Repository views \| 38 \| 30 \|/);
  assert.match(report, /\| Git clones \| 3 \| 3 \|/);
  assert.match(report, /\| Current stars \| 3 \| — \|/);
  assert.doesNotMatch(report, /48/);
});

test("summary uses seven consecutive calendar days when traffic is sparse", () => {
  const record = {
    schema_version: 1,
    captured_at: "2026-07-21T08:00:00Z",
    repository: "Kilbex/Vigla",
    traffic: {
      popular_referrers: [],
      popular_paths: [],
      views: {
        views: [
          { timestamp: "2026-07-01T00:00:00Z", count: 100, uniques: 50 },
          { timestamp: "2026-07-10T00:00:00Z", count: 4, uniques: 3 },
        ],
      },
      clones: {
        clones: [{ timestamp: "2026-07-10T00:00:00Z", count: 2, uniques: 1 }],
      },
    },
    repository_stats: { stargazers_count: 2 },
    releases: [],
  };

  const report = buildSummary([record]);
  assert.match(report, /Daily window: 2026-07-15 to 2026-07-21/);
  assert.match(report, /\| Repository views \| 0 \| 0 \|/);
  assert.match(report, /\| Git clones \| 0 \| 0 \|/);
  assert.doesNotMatch(report, /100/);
  assert.equal((report.match(/^\| 2026-07-/gm) ?? []).length, 7);
});
