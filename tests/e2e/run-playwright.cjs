#!/usr/bin/env node

const path = require("node:path");
const Module = require("node:module");

const appDir = path.resolve(__dirname, "..", "..", "app");
const appNodeModules = path.join(appDir, "node_modules");

const webDemoFlag = process.argv.indexOf("--web-demo");
if (webDemoFlag !== -1) {
  process.argv.splice(webDemoFlag, 1);
  process.env.VIGLA_E2E_WEB_DEMO = "1";
}

const siteFlag = process.argv.indexOf("--site");
if (siteFlag !== -1) {
  process.argv.splice(siteFlag, 1);
  process.env.VIGLA_E2E_SITE = "1";
}

process.env.NODE_PATH = [appNodeModules, process.env.NODE_PATH]
  .filter(Boolean)
  .join(path.delimiter);
Module._initPaths();

require(path.join(appNodeModules, "@playwright", "test", "cli.js"));
