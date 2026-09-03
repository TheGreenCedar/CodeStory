import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { canaryBlockers, runBuilderOperationCanary, sandboxCommand } from "../lib/builder-operation-canary.mjs";

test("operation canary executes required surfaces in the timed-agent sandbox and fails closed", { skip: process.platform !== "darwin" }, async () => {
  for (const fault of [null, "exit", "shape", "missing_exit"]) {
    const root = await mkdtemp(path.join(os.tmpdir(), "codestory-canary-test-"));
    const calls = [];
    const receipt = await runBuilderOperationCanary({
      root, outDir: root, cli: process.execPath, sandbox: "workspace-write",
      envForArm: (arm) => ({ CODEX_HOME: `/isolated/${arm}`, CODESTORY_CACHE_ROOT: "/prepared/cache" }),
      scopeArgs: () => ["--profile", "agent", "--run-id", "shared-agent"],
      retrievalArgs: (project) => ["retrieval", "index", "--project", project],
      packetArgs: (project) => ["packet", "--project", project],
      continuationArgs: () => null,
      validatePacket: (value) => value.schema_version === 3,
      runProcess: async (command, args, options) => {
        calls.push({ command, args, options });
        if (command !== "codex") return { exitCode: 0, stdout: "{}", stderr: "" };
        assert.deepEqual(args.slice(0, 3), ["sandbox", "--permission-profile", ":workspace"]);
        const operation = args[7] === "--project" ? args[6] : args[7];
        const source = "pub fn seed() -> usize { leaf() }\npub fn leaf() -> usize { 7 }\n";
        let stdout;
        if (args[6] === "rg" || args[6] === "sed") stdout = source;
        else if (operation === "search") stdout = JSON.stringify({ indexed_symbol_hits: [{ display_name: "seed", node_id: "node-a" }] });
        else if (operation === "snippet") stdout = JSON.stringify({ snippet: { snippet: source } });
        else if (operation === "callees") stdout = JSON.stringify({ trail: { trail: { edges: [{}] } } });
        else stdout = JSON.stringify({ schema_version: 3 });
        if (fault === "shape") stdout = "{}";
        return { exitCode: fault === "missing_exit" ? null : fault === "exit" ? 1 : 0, stdout, stderr: fault ? "fault" : "" };
      },
    });
    if (fault) {
      assert.equal(receipt.status, "fail");
      assert.equal(receipt.operations.length, 1);
      assert.ok(canaryBlockers(receipt).length > 0);
    } else {
      assert.equal(receipt.status, "pass", receipt.error);
      assert.deepEqual(canaryBlockers(receipt, receipt.cli_sha256), []);
      assert.equal(receipt.operations.length, 9);
      assert.ok(canaryBlockers(receipt, "0".repeat(64)).length > 0);
      assert.ok(calls.filter((call) => call.command === "codex").every((call) => call.options.env.CODESTORY_CACHE_ROOT === "/prepared/cache"));
    }
    assert.deepEqual(JSON.parse(await readFile(path.join(root, "operation-canary.json"), "utf8")), receipt);
    assert.equal(calls.some((call) => call.args[0] === "exec"), false, "canary never starts a model");
  }
});

test("unsupported canary policies never fall back to an unsandboxed command", () => {
  assert.throws(() => sandboxCommand("danger-full-access", "/repo", "cli", []), /pinned macOS/);
});
