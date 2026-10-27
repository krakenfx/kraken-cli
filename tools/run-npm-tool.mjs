#!/usr/bin/env node
// Runs one pinned npm-owned formatter.
//
// Formatters must not drift between a developer's machine and CI: a different
// minor version reformats the tree and every later diff inherits the noise.
// This refuses to execute unless the installed bytes match the exact pin in
// tools/npm/<tool>/package.json.
//
// Usage: node tools/run-npm-tool.mjs <tool> [args...]
//        node tools/run-npm-tool.mjs <tool> --verify-install

import { spawnSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const TOOLS = Object.freeze({
  markdownlint: { binary: "markdownlint", package: "markdownlint-cli" },
  prettier: { binary: "prettier", package: "prettier" },
});

// Ranges defeat the point: only a literal version is a pin.
const EXACT_VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

const TOOLS_DIR = path.dirname(fileURLToPath(import.meta.url));

function fail(message) {
  console.error(`run-npm-tool: ${message}`);
  process.exit(2);
}

function readJson(file) {
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch (error) {
    fail(`cannot read ${file}: ${error.message}`);
  }
}

const [name, ...args] = process.argv.slice(2);

if (!name) {
  fail(`no tool named. Available: ${Object.keys(TOOLS).join(", ")}`);
}

const tool = TOOLS[name];
if (!tool) {
  fail(`unknown tool '${name}'. Available: ${Object.keys(TOOLS).join(", ")}`);
}

// CI installs exactly one profile per job. A job asking for a different tool
// than it installed would otherwise run whatever happens to be on disk.
const ciTool = process.env.NPM_TOOL;
if (ciTool && ciTool !== name) {
  fail(`CI installed the '${ciTool}' profile, but '${name}' was requested`);
}

const profileDir = path.join(TOOLS_DIR, "npm", name);
const declared = readJson(path.join(profileDir, "package.json"))
  .devDependencies?.[tool.package];

if (!declared) {
  fail(`${tool.package} is not in tools/npm/${name}/package.json`);
}
if (!EXACT_VERSION.test(declared)) {
  fail(`${tool.package} must be pinned to an exact version, found '${declared}'`);
}

const installedManifest = path.join(
  profileDir,
  "node_modules",
  tool.package,
  "package.json",
);
const installed = readJson(installedManifest).version;

if (installed !== declared) {
  fail(
    `${tool.package} ${installed} is installed but ${declared} is pinned. ` +
      `Run 'just install-tools'.`,
  );
}

const binary = path.join(profileDir, "node_modules", ".bin", tool.binary);
try {
  statSync(binary);
} catch {
  fail(`${binary} is missing. Run 'just install-tools'.`);
}

if (args[0] === "--verify-install") {
  console.log(`${name}: ${tool.package}@${installed} ok`);
  process.exit(0);
}

const result = spawnSync(binary, args, { stdio: "inherit" });
if (result.error) {
  fail(`could not run ${binary}: ${result.error.message}`);
}
if (result.signal) {
  fail(`${tool.binary} was killed by ${result.signal}`);
}
process.exit(result.status ?? 1);
