#!/usr/bin/env node

/* Capture the real React web replay as a compact looping WebP.
 *
 * Start the browser harness first:
 *   VITE_VIGLA_E2E=1 VITE_VIGLA_WEB_DEMO=1 VITE_VIGLA_BASE=/ \
 *     pnpm -C app exec vite --strictPort --host 127.0.0.1 --port 5180
 *
 * Then, from another terminal:
 *   node scripts/capture-web-demo.cjs docs/media/vigla-demo.webp
 *
 * Requires Playwright Chromium and ffmpeg. The three scenes are canonical
 * recorded events; no vendor CLI, credential, or network request is used. */

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { createRequire } = require("node:module");

const appRequire = createRequire(path.join(__dirname, "..", "app", "package.json"));
const { chromium } = appRequire("@playwright/test");

const BASE = process.env.VIGLA_CAPTURE_URL || "http://127.0.0.1:5180";
const outputPath = path.resolve(process.argv[2] || "docs/media/vigla-demo.webp");
const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "vigla-web-demo-"));
const sourceVideo = path.join(tempDir, "capture.webm");
const FRAME_RATE = 15;
const FRAME_DELAY_MS = Math.round(1_000 / FRAME_RATE);
const WEBP_QUALITY = 76;
const CAPTURE_TRIM_SECONDS = "1.0";

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

function supportsFfmpegWebp() {
  const result = spawnSync("ffmpeg", ["-hide_banner", "-encoders"], {
    encoding: "utf8",
  });
  return result.status === 0 && /\blibwebp\b/.test(result.stdout || "");
}

function renderWebp(source, output) {
  const filter = `fps=${FRAME_RATE},scale=1280:-2:flags=lanczos`;
  if (supportsFfmpegWebp()) {
    run("ffmpeg", [
      "-hide_banner",
      "-loglevel",
      "error",
      "-y",
      "-ss",
      CAPTURE_TRIM_SECONDS,
      "-i",
      source,
      "-vf",
      filter,
      "-an",
      "-c:v",
      "libwebp",
      "-loop",
      "0",
      "-preset",
      "picture",
      "-quality",
      String(WEBP_QUALITY),
      "-compression_level",
      "6",
      output,
    ]);
    return;
  }

  if (spawnSync("img2webp", ["-version"], { stdio: "ignore" }).status !== 0) {
    throw new Error("ffmpeg lacks libwebp and img2webp is not installed");
  }
  const framesDir = path.join(tempDir, "frames");
  fs.mkdirSync(framesDir);
  run("ffmpeg", [
    "-hide_banner",
    "-loglevel",
    "error",
    "-y",
    "-ss",
    CAPTURE_TRIM_SECONDS,
    "-i",
    source,
    "-vf",
    filter,
    path.join(framesDir, "frame-%04d.png"),
  ]);
  const frames = fs
    .readdirSync(framesDir)
    .sort()
    .map((name) => path.join(framesDir, name));
  if (frames.length === 0) throw new Error("ffmpeg produced no animation frames");
  run("img2webp", [
    "-loop",
    "0",
    "-mixed",
    "-lossy",
    "-q",
    String(WEBP_QUALITY),
    "-m",
    "4",
    "-d",
    String(FRAME_DELAY_MS),
    ...frames,
    "-o",
    output,
  ]);
}

async function choose(page, name, waitMs) {
  await page.getByRole("button", { name, exact: true }).click();
  await page.getByRole("button", { name: "1×", exact: true }).click();
  await page.waitForTimeout(waitMs);
}

(async () => {
  if (spawnSync("ffmpeg", ["-version"], { stdio: "ignore" }).status !== 0) {
    throw new Error("ffmpeg is required to render docs/media/vigla-demo.webp");
  }

  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  const browser = await chromium.launch();
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: "dark",
    recordVideo: {
      dir: tempDir,
      size: { width: 1440, height: 900 },
    },
  });
  const page = await context.newPage();
  const video = page.video();

  await page.goto(BASE, { waitUntil: "networkidle" });
  await page.waitForSelector(".web-demo-bar");
  await page.waitForSelector(".station");
  await choose(page, "Accepted", 6_700);
  await choose(page, "Bound tripped", 5_200);
  await choose(page, "Quota paused", 2_400);
  await page.getByRole("button", { name: "Accepted", exact: true }).click();
  await page.waitForTimeout(500);

  await context.close();
  if (!video) throw new Error("Playwright did not create a video handle");
  await video.saveAs(sourceVideo);
  await browser.close();

  renderWebp(sourceVideo, outputPath);

  const bytes = fs.statSync(outputPath).size;
  const mib = bytes / (1024 * 1024);
  if (mib < 2 || mib > 8) {
    throw new Error(
      `animated WebP is ${mib.toFixed(2)} MiB; expected the launch budget of 2–8 MiB`,
    );
  }

  fs.rmSync(tempDir, { recursive: true, force: true });
  process.stdout.write(`${outputPath} (${mib.toFixed(2)} MiB)\n`);
})().catch((error) => {
  fs.rmSync(tempDir, { recursive: true, force: true });
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
