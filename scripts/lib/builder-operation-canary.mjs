import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import { BUILDER_ABLATION_ARMS } from "./evidence-compiler-ablation.mjs";

const CONTRACT = "codestory.builder-operation-canary/v1";
const SOURCE = "pub fn seed() -> usize { leaf() }\npub fn leaf() -> usize { 7 }\n";
const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const REQUIRED = Object.freeze({
  native_tools: ["native_search", "native_source"],
  exact_identity_source: ["search", "snippet"],
  exact_plus_relations: ["search", "snippet", "callees"],
  packet_semantic_off: ["packet"],
  packet_semantic_on: ["packet"],
});

// Use Codex's own OS sandbox, with the same built-in policy as the agent runner.
// Unsupported hosts/policies fail closed; a direct CLI fallback is not equivalent.
function sandboxCommand(sandbox, project, command, args) {
  if (process.platform !== "darwin" || sandbox !== "workspace-write") {
    throw new Error("operation canary currently requires the pinned macOS workspace-write cell");
  }
  return {
    command: "codex",
    args: ["sandbox", "--permission-profile", ":workspace", "--cd", project, "--", command, ...args],
  };
}

function canaryBlockers(receipt, cliSha256 = null) {
  const reasons = [];
  if (receipt?.contract !== CONTRACT || receipt?.status !== "pass") {
    reasons.push("operation canary did not pass");
  }
  if (cliSha256 && receipt?.cli_sha256 !== cliSha256) {
    reasons.push("operation canary binary differs from the model-row binary");
  }
  for (const arm of BUILDER_ABLATION_ARMS) {
    for (const operation of REQUIRED[arm]) {
      const rows = receipt?.operations?.filter((row) => row.arm === arm && row.operation === operation) ?? [];
      if (rows.length !== 1 || rows[0].exit_code !== 0 || rows[0].shape_valid !== true ||
          !/^[a-f0-9]{64}$/u.test(rows[0].stdout_sha256 ?? "") ||
          !/^[a-f0-9]{64}$/u.test(rows[0].stderr_sha256 ?? "") ||
          !rows[0].sandboxed || !Number.isFinite(rows[0].wall_ms)) {
        reasons.push(`${arm}/${operation} lacks successful sandboxed operation telemetry`);
      }
    }
  }
  for (const row of receipt?.operations ?? []) {
    if (row.exit_code !== 0 || row.shape_valid !== true) reasons.push(`${row.arm}/${row.operation} failed`);
  }
  return reasons;
}

async function runBuilderOperationCanary({
  root, outDir, cli, sandbox, envForArm, runProcess,
  scopeArgs, retrievalArgs, packetArgs, continuationArgs, validatePacket,
}) {
  await mkdir(root, { recursive: true });
  const project = await mkdtemp(path.join(root, "operation-canary-"));
  await mkdir(path.join(project, "src"));
  await writeFile(path.join(project, "Cargo.toml"), '[package]\nname = "canary"\nversion = "0.0.0"\nedition = "2021"\n');
  await writeFile(path.join(project, "src", "lib.rs"), SOURCE);
  const receipt = {
    contract: CONTRACT,
    status: "fail",
    project,
    source_sha256: sha256(SOURCE),
    cli_sha256: sha256(await readFile(cli)),
    sandbox,
    operations: [],
  };
  const persist = async () => {
    await writeFile(path.join(outDir, "operation-canary.json"), JSON.stringify(receipt, null, 2) + "\n");
    return receipt;
  };
  try {
    // Preparation is outside the agent sandbox, exactly as task preparation is.
    receipt.preparation = [];
    for (const args of [["index", "--project", project, "--format", "json"], retrievalArgs(project)]) {
      const prepared = await runProcess(cli, args, {
        cwd: project, env: envForArm("exact_identity_source"), timeoutMs: 120_000,
      });
      receipt.preparation.push({
        arguments: args, exit_code: prepared.exitCode,
        stdout_sha256: sha256(prepared.stdout), stderr_sha256: sha256(prepared.stderr),
      });
      await writeFile(path.join(outDir, `canary-preparation-${args[0]}.stdout`), prepared.stdout);
      await writeFile(path.join(outDir, `canary-preparation-${args[0]}.stderr`), prepared.stderr);
      if (prepared.exitCode !== 0) throw new Error("canary preparation failed");
    }

    async function execute(arm, operation, command, args, validate) {
      const wrapped = sandboxCommand(sandbox, project, command, args);
      const env = envForArm(arm);
      if (!env.CODEX_HOME) throw new Error(`${arm} lacks the isolated timed-agent home`);
      const started = performance.now();
      const result = await runProcess(wrapped.command, wrapped.args, {
        cwd: project, env, timeoutMs: 60_000,
      });
      let shapeValid = false;
      let output = null;
      if (result.exitCode === 0) {
        try {
          output = operation.startsWith("native_") ? result.stdout : JSON.parse(result.stdout);
          shapeValid = await validate(output) === true;
        } catch { /* Wrong JSON shape is an invalid intervention, never a quality failure. */ }
      }
      receipt.operations.push({
        arm, operation, command: wrapped.command, arguments: wrapped.args,
        sandboxed: true, codex_home: env.CODEX_HOME,
        exit_code: result.exitCode, wall_ms: performance.now() - started,
        stdout_sha256: sha256(result.stdout), stderr_sha256: sha256(result.stderr),
        shape_valid: shapeValid,
      });
      await writeFile(path.join(outDir, `canary-${arm}-${operation}.stdout`), result.stdout);
      await writeFile(path.join(outDir, `canary-${arm}-${operation}.stderr`), result.stderr);
      if (result.exitCode !== 0 || !shapeValid) throw new Error(`${arm}/${operation} failed its operation canary`);
      return output;
    }
    for (const arm of BUILDER_ABLATION_ARMS) {
      if (arm === "native_tools") {
        await execute(arm, "native_search", "rg", ["pub fn seed", "src/lib.rs"], (value) => value.includes("pub fn seed"));
        await execute(arm, "native_source", "sed", ["-n", "1,2p", "src/lib.rs"], (value) => value === SOURCE);
      } else if (arm.startsWith("exact_")) {
        const common = ["--project", project, "--refresh", "none", "--format", "json"];
        const search = await execute(arm, "search", cli,
          ["search", ...common, ...scopeArgs(), "--query", "seed", "--repo-text", "off"],
          (value) => value.indexed_symbol_hits?.some((hit) => hit.display_name === "seed" && typeof hit.node_id === "string"));
        const id = search.indexed_symbol_hits.find((hit) => hit.display_name === "seed").node_id;
        await execute(arm, "snippet", cli, ["snippet", ...common, "--id", id],
          (value) => value.snippet?.snippet?.includes("leaf()") === true);
        if (arm === "exact_plus_relations") {
          await execute(arm, "callees", cli, ["callees", ...common, "--id", id, "--depth", "1"],
            (value) => value.trail?.trail?.edges?.length > 0 || value.trail?.edges?.length > 0);
        }
      } else {
        const args = packetArgs(project, arm);
        const packet = await execute(arm, "packet", cli, args, (value) => validatePacket(value, arm));
        const next = continuationArgs(project, arm, packet);
        if (next) await execute(arm, "continuation", cli, next, (value) => validatePacket(value, arm, packet));
      }
    }
    receipt.status = "pass";
  } catch (error) {
    receipt.error = error.message;
  }
  return await persist();
}

export { CONTRACT, REQUIRED, canaryBlockers, runBuilderOperationCanary, sandboxCommand };
