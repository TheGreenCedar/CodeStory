import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptUrl = pathToFileURL(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../prove-plugin-pinned-provision.mjs"),
).href;

// The wait must bound itself even when the launcher keeps a readable non-managed metadata file
// on disk, so drive it in its own process and kill it if it outlives the bound. A hang here is a
// reported failure, not a wedged test run.
const KILL_AFTER_MS = 5_000;

function driveWait({ runtimeMetadata, timeoutMs, exitCode }) {
  const source = `
    import { waitForManagedRuntime } from ${JSON.stringify(scriptUrl)};
    const exitCode = ${JSON.stringify(exitCode)};
    let killed = false;
    const child = { exitCode, kill() { killed = true; } };
    const started = Date.now();
    let outcome;
    try {
      await waitForManagedRuntime({
        child,
        runtimeMetadata: ${JSON.stringify(runtimeMetadata)},
        timeoutMs: ${JSON.stringify(timeoutMs)},
        intervalMs: 10,
      });
      outcome = { settled: "resolved" };
    } catch (error) {
      outcome = { settled: "rejected", message: error.message };
    }
    console.log(JSON.stringify({ ...outcome, killed, elapsedMs: Date.now() - started }));
  `;
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", source], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const killer = setTimeout(() => child.kill("SIGKILL"), KILL_AFTER_MS);
    child.on("close", (code, signal) => {
      clearTimeout(killer);
      if (signal || stdout.trim() === "") {
        resolve({ settled: "hung", code, signal, stderr });
        return;
      }
      resolve(JSON.parse(stdout.trim()));
    });
  });
}

function driftedRuntimeMetadata() {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-pin-proof-test-"));
  const runtimeMetadata = path.join(dataDir, ".codestory-mcp-runtime.json");
  // What the launcher writes when the pin's archive digest no longer resolves: readable
  // metadata that never reaches the managed source the proof is waiting for.
  fs.writeFileSync(
    runtimeMetadata,
    JSON.stringify({ source: "managed_unavailable", path: null, cliVersion: null }),
  );
  return runtimeMetadata;
}

test("pin drift that keeps runtime metadata readable still times out within the bound", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: 300,
    exitCode: null,
  });
  assert.equal(outcome.settled, "rejected", `wait did not bound itself: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /provisioning did not finish within 300ms/u);
  assert.equal(outcome.killed, true, "the timed-out wait must kill the launcher");
  assert.ok(outcome.elapsedMs < KILL_AFTER_MS, `waited ${outcome.elapsedMs}ms`);
});

test("a launcher that exits while runtime metadata stays readable fails fast", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: 3,
  });
  assert.equal(outcome.settled, "rejected", `wait did not notice the exit: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /launcher exited 3 before provisioning finished/u);
  assert.ok(outcome.elapsedMs < KILL_AFTER_MS, `waited ${outcome.elapsedMs}ms`);
});

test("managed runtime metadata resolves the wait with the published metadata", async () => {
  const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-pin-proof-test-"));
  const runtimeMetadata = path.join(dataDir, ".codestory-mcp-runtime.json");
  fs.writeFileSync(runtimeMetadata, JSON.stringify({ source: "managed", cliVersion: "9.9.9" }));
  const outcome = await driveWait({ runtimeMetadata, timeoutMs: 600_000, exitCode: null });
  assert.equal(outcome.settled, "resolved", JSON.stringify(outcome));
  assert.equal(outcome.killed, false, "a successful wait must not kill the launcher itself");
});
