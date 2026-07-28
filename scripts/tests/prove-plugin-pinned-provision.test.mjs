import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptsDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const proofScript = path.join(scriptsDir, "prove-plugin-pinned-provision.mjs");
const waitModuleUrl = pathToFileURL(path.join(scriptsDir, "lib/wait-for-managed-runtime.mjs")).href;

// The wait must bound itself even when the launcher keeps a readable non-managed metadata file
// on disk, so drive it in its own process and kill it if it outlives the bound. A hang here is a
// reported failure, not a wedged test run.
const KILL_AFTER_MS = 5_000;

function runNode(args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, args, { stdio: ["ignore", "pipe", "pipe"], ...options });
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
      resolve({ code, signal, stdout, stderr });
    });
  });
}

function driveWait({ runtimeMetadata, timeoutMs, exitCode }) {
  const source = `
    import { waitForManagedRuntime } from ${JSON.stringify(waitModuleUrl)};
    const exitCode = ${JSON.stringify(exitCode)};
    let killed = false;
    const child = { exitCode, kill() { killed = true; } };
    const started = Date.now();
    let outcome;
    try {
      const metadata = await waitForManagedRuntime({
        child,
        runtimeMetadata: ${JSON.stringify(runtimeMetadata)},
        timeoutMs: ${JSON.stringify(timeoutMs)},
        intervalMs: 10,
      });
      outcome = { settled: "resolved", metadata };
    } catch (error) {
      outcome = { settled: "rejected", message: error.message };
    }
    console.log(JSON.stringify({ ...outcome, killed, elapsedMs: Date.now() - started }));
  `;
  return runNode(["--input-type=module", "-e", source]).then((result) => {
    if (result.signal || result.stdout.trim() === "") {
      return { settled: "hung", code: result.code, signal: result.signal, stderr: result.stderr };
    }
    return JSON.parse(result.stdout.trim());
  });
}

function scratchDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "codestory-pin-proof-test-"));
}

function driftedRuntimeMetadata() {
  const runtimeMetadata = path.join(scratchDir(), ".codestory-mcp-runtime.json");
  // What the launcher writes when the pin's archive digest no longer resolves: readable
  // metadata that never reaches the managed source the proof is waiting for.
  fs.writeFileSync(
    runtimeMetadata,
    JSON.stringify({ source: "managed_unavailable", path: null, cliVersion: null }),
  );
  return runtimeMetadata;
}

function managedRuntimeMetadata() {
  const runtimeMetadata = path.join(scratchDir(), ".codestory-mcp-runtime.json");
  fs.writeFileSync(runtimeMetadata, JSON.stringify({ source: "managed", cliVersion: "9.9.9" }));
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
  const outcome = await driveWait({
    runtimeMetadata: managedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: null,
  });
  assert.equal(outcome.settled, "resolved", JSON.stringify(outcome));
  assert.equal(outcome.metadata.cliVersion, "9.9.9");
  assert.equal(outcome.killed, false, "a successful wait must not kill the launcher itself");
});

// The launcher hands off and exits 0 once it has published its manifest, so the exit guard must
// never outrank a completed provision: the metadata read has to settle the tick first.
test("a launcher that published managed metadata and then exited 0 is a success", async () => {
  const outcome = await driveWait({
    runtimeMetadata: managedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: 0,
  });
  assert.equal(
    outcome.settled,
    "resolved",
    `a completed provision was reported as a failure: ${JSON.stringify(outcome)}`,
  );
  assert.equal(outcome.metadata.source, "managed");
});

test("the wait refuses a timeout that is not a positive number instead of never expiring", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: null,
    exitCode: null,
  });
  assert.equal(outcome.settled, "rejected", `an unbounded wait was accepted: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /positive finite timeoutMs/u);
});

// plugin-release.yml gates the `v*` tag on this script, so every way of invoking it has to reach
// the proof. A wrong entry-point comparison used to exit 0 with no output when the path reached
// the file through a symlink, which is a vacuous pass on the release gate.
for (const invocation of ["direct", "symlinked"]) {
  test(`the gate refuses an unusable --timeout-ms when invoked ${invocation}`, async (t) => {
    let entry = proofScript;
    if (invocation === "symlinked") {
      const link = path.join(scratchDir(), "scripts");
      try {
        fs.symlinkSync(scriptsDir, link, "dir");
      } catch (error) {
        t.skip(`this platform refuses directory symlinks: ${error.code}`);
        return;
      }
      entry = path.join(link, "prove-plugin-pinned-provision.mjs");
    }
    const outcome = await runNode([entry, "--timeout-ms", "not-a-number"]);
    assert.equal(
      outcome.code,
      1,
      `the gate did not fail closed: ${JSON.stringify({ ...outcome, entry })}`,
    );
    assert.match(outcome.stderr, /::error::--timeout-ms needs a positive number/u);
  });
}

test("the gate refuses --timeout-ms with no value at all", async () => {
  const outcome = await runNode([proofScript, "--timeout-ms"]);
  assert.equal(outcome.code, 1, `the gate did not fail closed: ${JSON.stringify(outcome)}`);
  assert.match(outcome.stderr, /::error::--timeout-ms needs a positive number/u);
});
