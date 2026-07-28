#!/usr/bin/env node
// Prove the packaged plugin provisions its pinned CLI over the real release path.
//
// Drives plugins/codestory/scripts/codestory-mcp.cjs the way a host does: spawn it, ask for
// status, and wait for managed runtime metadata. Then verify what was actually provisioned:
// the manifest must name the pinned version, a real github_release build source, and the pin's
// own archive digest, and the provisioned binary must report the pinned version.
//
//   node scripts/prove-plugin-pinned-provision.mjs [--timeout-ms 600000]

import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const launcher = path.join(repositoryRoot, "plugins/codestory/scripts/codestory-mcp.cjs");

function fail(message) {
  console.error(`::error::${message}`);
  process.exit(1);
}

// Wait for the launcher to publish managed runtime metadata. The liveness guards run on every
// tick and never sit behind the metadata read: a launcher that resolves to a non-managed source
// (pin or archive digest drift) keeps writing a readable file, so guards nested in the read's
// catch would never fire and the proof would hang until the CI job timeout.
export function waitForManagedRuntime({ child, runtimeMetadata, timeoutMs, intervalMs = 250 }) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const poll = setInterval(() => {
      if (child.exitCode !== null) {
        clearInterval(poll);
        reject(new Error(`launcher exited ${child.exitCode} before provisioning finished.`));
        return;
      }
      if (Date.now() > deadline) {
        clearInterval(poll);
        child.kill();
        reject(new Error(`provisioning did not finish within ${timeoutMs}ms.`));
        return;
      }
      let metadata;
      try {
        metadata = JSON.parse(fs.readFileSync(runtimeMetadata, "utf8"));
      } catch {
        // Not written yet, or caught mid-write. Read again on the next tick.
        return;
      }
      if (metadata.source !== "managed") return;
      clearInterval(poll);
      resolve(metadata);
    }, intervalMs);
  });
}

function verifyProvision(dataDir, pin) {
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
  if (manifest.build_source !== "github_release") {
    fail(`expected a github_release provision, observed ${manifest.build_source}.`);
  }
  if (pin.archives?.[target] && manifest.archive_sha256 !== pin.archives[target]) {
    fail(
      `provisioned archive digest ${manifest.archive_sha256} does not match the pin's ` +
        `${target} digest ${pin.archives[target]}.`,
    );
  }
  const binary = path.join(versionDir, manifest.path);
  const reported = execFileSync(binary, ["--version"], { encoding: "utf8" }).trim();
  if (!reported.includes(pin.cli_version)) {
    fail(`provisioned binary reports "${reported}", expected ${pin.cli_version}.`);
  }
  console.log(
    `Pinned provision proven: ${target} ${pin.cli_version} from github_release, ` +
      `archive ${manifest.archive_sha256.slice(0, 12)}…, binary reports "${reported}".`,
  );
}

async function main() {
  const pin = JSON.parse(
    fs.readFileSync(path.join(repositoryRoot, "plugins/codestory/cli-version.json"), "utf8"),
  );

  const timeoutIndex = process.argv.indexOf("--timeout-ms");
  const timeoutMs = timeoutIndex >= 0 ? Number(process.argv[timeoutIndex + 1]) : 600_000;

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
    await waitForManagedRuntime({ child, runtimeMetadata, timeoutMs });
  } catch (error) {
    fail(`${error.message}\n${stderr}`);
  }
  child.kill();
  verifyProvision(dataDir, pin);
  fs.rmSync(dataDir, { recursive: true, force: true });
  process.exit(0);
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  await main();
}
