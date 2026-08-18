#!/usr/bin/env node
// Prove the packaged plugin provisions its pinned CLI over the real release path.
//
// Drives plugins/codestory/scripts/codestory-mcp.cjs the way a host does: spawn it, ask for
// status, and wait for managed runtime metadata. Then verify what was actually provisioned:
// the manifest must name the pinned version and the build source this lane expects, the archive
// digest must match the authority THIS LANE OWNS, and the provisioned binary must report the
// pinned version.
//
//   plugin lane -- the fast lane pins an already-published CLI, so the source pin carries that
//   release's archive digests and this proof holds the provision against them:
//     node scripts/prove-plugin-pinned-provision.mjs [--timeout-ms 600000]
//
//   native lane -- the pin names the release about to be built from this very tree, so it lawfully
//   carries no archive digests and the release manifest owns them instead:
//     node scripts/prove-plugin-pinned-provision.mjs --lane native \
//       --expect-build-source explicit_package --release-manifest PATH
//     node scripts/prove-plugin-pinned-provision.mjs --lane native \
//       --expect-build-source explicit_package --defer-archive-digest
//
// This file is an entry point and nothing else imports it, so its body runs unconditionally.
// The bounded wait lives in scripts/lib/wait-for-managed-runtime.mjs and the lane split lives in
// scripts/lib/provision-proof-lanes.mjs, which the tests import without running the proof.

import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { waitForManagedRuntime } from "./lib/wait-for-managed-runtime.mjs";
import {
  assertProvisionedArchiveDigest,
  parseProvisionProofArguments,
} from "./lib/provision-proof-lanes.mjs";
import { parseReleaseManifest } from "./lib/release-manifest.mjs";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const launcher = path.join(repositoryRoot, "plugins/codestory/scripts/codestory-mcp.cjs");
const pin = JSON.parse(
  fs.readFileSync(path.join(repositoryRoot, "plugins/codestory/cli-version.json"), "utf8"),
);

function fail(message) {
  console.error(`::error::${message}`);
  process.exit(1);
}

// A missing or non-numeric --timeout-ms used to yield NaN, and every `Date.now() > NaN` comparison
// is false, so the one flag that declares the bound silently removed it. Every argument is refused
// the same way now: an unbounded gate, a misspelled lane, or an unstated native digest authority
// all stop the proof rather than quietly weakening it.
let options;
try {
  options = parseProvisionProofArguments(process.argv.slice(2));
} catch (error) {
  fail(error.message);
}

let stagedReleaseManifest = null;
if (options.releaseManifestPath) {
  try {
    stagedReleaseManifest = parseReleaseManifest(
      fs.readFileSync(options.releaseManifestPath, "utf8"),
    );
  } catch (error) {
    fail(`could not use ${options.releaseManifestPath}: ${error.message}`);
  }
}

const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-pin-proof-"));
const runtimeMetadata = path.join(dataDir, ".codestory-mcp-runtime.json");

const child = spawn(process.execPath, [launcher], {
  env: { ...process.env, CODESTORY_CLI: "", PLUGIN_DATA: dataDir },
  stdio: ["pipe", "pipe", "pipe"],
});
let stderr = "";
child.stderr.on("data", (chunk) => {
  stderr += chunk;
});
child.stdout.resume();
child.stdin.write(
  `${JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: `codestory://status?project=${encodeURIComponent(repositoryRoot)}` },
  })}\n`,
);

try {
  await waitForManagedRuntime({ child, runtimeMetadata, timeoutMs: options.timeoutMs });
} catch (error) {
  fail(`${error.message}\n${stderr}`);
}
child.kill();

const versionDir = path.join(dataDir, "codestory-cli", pin.cli_version);
const manifest = JSON.parse(fs.readFileSync(path.join(versionDir, "manifest.json"), "utf8"));
const target =
  process.platform === "darwin"
    ? "macos-arm64"
    : process.platform === "win32"
      ? "windows-x64"
      : "linux-x64";
if (manifest.version !== pin.cli_version) {
  fail(`provisioned ${manifest.version}, pin names ${pin.cli_version}.`);
}
if (manifest.build_source !== options.expectBuildSource) {
  fail(`expected a ${options.expectBuildSource} provision, observed ${manifest.build_source}.`);
}
let outcome;
try {
  outcome = assertProvisionedArchiveDigest({
    lane: options.lane,
    pin,
    target,
    provisioned: { sha256: manifest.archive_sha256, bytes: manifest.archive_bytes },
    releaseManifest: stagedReleaseManifest,
    deferArchiveDigest: options.deferArchiveDigest,
  });
} catch (error) {
  fail(`${error.message}.`);
}
const binary = path.join(versionDir, manifest.path);
const reported = execFileSync(binary, ["--version"], { encoding: "utf8" }).trim();
if (!reported.includes(pin.cli_version)) {
  fail(`provisioned binary reports "${reported}", expected ${pin.cli_version}.`);
}
// A deferral is a real gap in what this run proved, so it is announced instead of being folded
// into the success line. The post-release manifest proof is the step that closes it.
if (!outcome.asserted) {
  console.log(`::warning::${outcome.claim}.`);
}
console.log(
  `Pinned provision proven (${options.lane} lane): ${target} ${pin.cli_version} from ` +
    `${manifest.build_source}, ${outcome.claim}, binary reports "${reported}".`,
);
fs.rmSync(dataDir, { recursive: true, force: true });
process.exit(0);
