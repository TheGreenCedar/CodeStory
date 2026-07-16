#!/usr/bin/env node

import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

function fail(message) {
  throw new Error(message);
}

export function platformKey(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  const supported = new Set([
    "darwin-arm64",
    "darwin-x64",
    "linux-arm64",
    "linux-x64",
    "win32-arm64",
    "win32-x64",
  ]);
  if (!supported.has(key)) fail(`actionlint does not have a declared asset for ${key}`);
  return key;
}

function actionlintVersion(binary) {
  const result = spawnSync(binary, ["-version"], { encoding: "utf8" });
  if (result.status !== 0) return null;
  return result.stdout.trim().split(/\s+/u)[0] ?? null;
}

export function verifyArchiveChecksum(bytes, expectedSha256, archive = "actionlint archive") {
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (digest !== expectedSha256) fail(`actionlint checksum mismatch for ${archive}: ${digest}`);
  return digest;
}

export function cacheIsTrusted({
  binary,
  marker,
  expectedSha256,
  expectedVersion,
  versionReader = actionlintVersion,
}) {
  return existsSync(binary)
    && existsSync(marker)
    && readFileSync(marker, "utf8").trim() === expectedSha256
    && versionReader(binary) === expectedVersion;
}

export async function downloadPinnedActionlint(contract, key) {
  const asset = contract.assets[key];
  const cacheRoot = path.join(
    process.env.RUNNER_TEMP?.trim() || os.tmpdir(),
    "codestory-actionlint",
    contract.version,
    key,
  );
  const binary = path.join(cacheRoot, process.platform === "win32" ? "actionlint.exe" : "actionlint");
  const marker = path.join(cacheRoot, "archive.sha256");
  if (cacheIsTrusted({
    binary,
    marker,
    expectedSha256: asset.sha256,
    expectedVersion: contract.version,
  })) return binary;
  mkdirSync(cacheRoot, { recursive: true });
  const archive = path.join(cacheRoot, asset.archive);
  const url = `https://github.com/rhysd/actionlint/releases/download/v${contract.version}/${asset.archive}`;
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) fail(`actionlint download failed with HTTP ${response.status}: ${url}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  verifyArchiveChecksum(bytes, asset.sha256, asset.archive);
  writeFileSync(archive, bytes);
  const extract = spawnSync("tar", ["-xf", archive, "-C", cacheRoot], { encoding: "utf8" });
  if (extract.status !== 0) fail(`failed to extract ${asset.archive}: ${extract.stderr.trim()}`);
  rmSync(archive);
  if (!existsSync(binary)) fail(`actionlint archive did not contain ${path.basename(binary)}`);
  if (process.platform !== "win32") chmodSync(binary, 0o755);
  if (actionlintVersion(binary) !== contract.version) fail(`installed actionlint is not ${contract.version}`);
  writeFileSync(marker, `${asset.sha256}\n`, { mode: 0o600 });
  return binary;
}

export function workflowPaths() {
  const directory = path.join(root, ".github", "workflows");
  return readdirSync(directory)
    .filter((file) => /\.ya?ml$/u.test(file))
    .sort()
    .map((file) => path.join(directory, file));
}

async function main() {
  const graph = loadReleaseClaimGraph(root);
  const contract = graph.workflow_policy.actionlint;
  const key = platformKey();
  const workflows = workflowPaths();
  const invalidFixture = path.join(root, ".github", "scripts", "fixtures", "actionlint-invalid.yml");
  if (process.argv.includes("--self-test")) {
    if (!existsSync(path.join(root, contract.config))) fail(`missing actionlint config ${contract.config}`);
    if (!existsSync(invalidFixture)) fail(`missing controlled-invalid actionlint fixture ${invalidFixture}`);
    if (workflows.length === 0) fail("no workflow files found for actionlint");
    console.log(`actionlint contract passed: v${contract.version}, ${Object.keys(contract.assets).length} checksum-pinned assets, ${workflows.length} workflows`);
    return;
  }

  const configured = process.env.ACTIONLINT?.trim();
  let binary = configured || "actionlint";
  if (actionlintVersion(binary) !== contract.version) {
    binary = await downloadPinnedActionlint(contract, key);
  }
  const result = spawnSync(binary, [
    "-no-color",
    "-config-file",
    path.join(root, contract.config),
    ...workflows,
  ], { cwd: root, stdio: "inherit" });
  if (result.error) fail(`failed to execute actionlint: ${result.error.message}`);
  if (result.status !== 0) {
    process.exitCode = result.status ?? 1;
    return;
  }
  const controlledInvalid = spawnSync(binary, [
    "-no-color",
    "-config-file",
    path.join(root, contract.config),
    invalidFixture,
  ], { cwd: root, encoding: "utf8" });
  if (controlledInvalid.error) fail(`failed to execute controlled-invalid actionlint fixture: ${controlledInvalid.error.message}`);
  if (controlledInvalid.status === 0) fail("actionlint accepted the controlled-invalid syntax fixture");
  if (`${controlledInvalid.stdout}${controlledInvalid.stderr}`.trim() === "") {
    fail("actionlint rejected the controlled-invalid syntax fixture without a diagnostic");
  }
  console.log("actionlint controlled-invalid syntax fixture rejected as expected");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main();
}
