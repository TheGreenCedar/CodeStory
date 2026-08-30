#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs as parseNodeArgs } from "node:util";

import {
  installedAgentTiming,
  installedAgentTimingCohortId,
  runProcess,
} from "./codestory-agent-ab-benchmark.mjs";

const scriptPath = fileURLToPath(import.meta.url);
const scriptDir = path.dirname(scriptPath);
const repoRoot = path.resolve(scriptDir, "..");
const harnessPath = path.join(scriptDir, "codestory-agent-ab-benchmark.mjs");
const REQUIRED_TASK_IDS = Object.freeze([
  "dart-http-client-flow",
  "c-redis-command-loop",
  "python-requests-session-flow",
  "rust-ripgrep-search-pipeline",
]);
const ARMS = Object.freeze(["published_0_17_5", "candidate_0_18"]);
const PINNED_MODEL = "gpt-5.6-sol";

function sha256Bytes(value) {
  return createHash("sha256").update(value).digest("hex");
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function abbaRunPlan(taskIds = REQUIRED_TASK_IDS, repeats = 5) {
  if (!Array.isArray(taskIds) || !taskIds.length) {
    throw new Error("focused ABBA requires at least one task");
  }
  if (!Number.isInteger(repeats) || repeats < 1) {
    throw new Error("focused ABBA repeats must be a positive integer");
  }
  const plan = [];
  for (const taskId of taskIds) {
    const armRepeats = Object.fromEntries(ARMS.map((arm) => [arm, 0]));
    const block = [
      "published_0_17_5",
      "candidate_0_18",
      "candidate_0_18",
      "published_0_17_5",
    ];
    while (ARMS.some((arm) => armRepeats[arm] < repeats)) {
      for (const arm of block) {
        if (armRepeats[arm] >= repeats) continue;
        armRepeats[arm] += 1;
        plan.push({ task_id: taskId, arm, repeat: armRepeats[arm] });
      }
    }
    const taskRows = plan.filter((row) => row.task_id === taskId);
    for (const arm of ARMS) {
      if (taskRows.filter((row) => row.arm === arm).length !== repeats) {
        throw new Error(`focused ABBA failed to schedule ${repeats} ${arm} rows for ${taskId}`);
      }
    }
  }
  return plan;
}

function focusedAbbaTiming(rawRow, dimensions) {
  const raw = rawRow?.installed_agent_timing;
  if (!raw) throw new Error("focused ABBA row has no installed agent timing");
  const timingCohortId = installedAgentTimingCohortId(dimensions);
  const prelude = rawRow.codestory_harness_prelude;
  const baselineMs = rawRow.baseline_harness_prelude?.wall_ms ?? 0;
  const agentRunnerMs = Number.isFinite(rawRow.agent_runner_wall_ms)
    ? rawRow.agent_runner_wall_ms + baselineMs
    : raw.agent_runner_ms;
  const timeToFirstPacketMs = Number.isFinite(prelude?.time_to_first_packet_ms)
    ? prelude.time_to_first_packet_ms
    : raw.time_to_first_packet_ms;
  const continuationMs = Number.isFinite(prelude?.continuation_ms)
    ? prelude.continuation_ms
    : raw.continuation_ms;
  const wholeTaskWallMs = Number.isFinite(rawRow.wall_ms)
    ? rawRow.wall_ms
    : raw.whole_task_wall_ms;
  return installedAgentTiming({
    timing_cohort_id: timingCohortId,
    agent_runner_ms: agentRunnerMs,
    time_to_first_packet_ms: timeToFirstPacketMs,
    continuation_ms: continuationMs,
    whole_task_wall_ms: wholeTaskWallMs,
  });
}

function parseArgs(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      help: { type: "boolean", short: "h" },
      "published-cli": { type: "string" },
      "candidate-cli": { type: "string" },
      "repo-cache-dir": { type: "string" },
      "state-root": { type: "string" },
      "out-dir": { type: "string" },
      "execution-window-id": { type: "string" },
      "timeout-ms": { type: "string" },
      "list-plan": { type: "boolean" },
    },
  });
  if (values.help) {
    process.stdout.write(
      "Usage: node scripts/codestory-focused-abba-preflight.mjs --published-cli PATH --candidate-cli PATH --repo-cache-dir DIR --state-root DIR --out-dir DIR [--execution-window-id ID]\n",
    );
    process.exit(0);
  }
  const opts = {
    publishedCli: values["published-cli"] ? path.resolve(values["published-cli"]) : null,
    candidateCli: values["candidate-cli"] ? path.resolve(values["candidate-cli"]) : null,
    repoCacheDir: values["repo-cache-dir"] ? path.resolve(values["repo-cache-dir"]) : null,
    stateRoot: values["state-root"] ? path.resolve(values["state-root"]) : null,
    outDir: values["out-dir"] ? path.resolve(values["out-dir"]) : null,
    executionWindowId: values["execution-window-id"] ?? null,
    timeoutMs: values["timeout-ms"] == null ? 600_000 : Number.parseInt(values["timeout-ms"], 10),
    listPlan: values["list-plan"] === true,
  };
  if (opts.listPlan) return opts;
  for (const [field, value] of Object.entries({
    publishedCli: opts.publishedCli,
    candidateCli: opts.candidateCli,
    repoCacheDir: opts.repoCacheDir,
    stateRoot: opts.stateRoot,
    outDir: opts.outDir,
  })) {
    if (!value) throw new Error(`focused ABBA requires ${field}`);
  }
  if (!Number.isInteger(opts.timeoutMs) || opts.timeoutMs < 1_000) {
    throw new Error("focused ABBA timeout must be an integer >= 1000");
  }
  return opts;
}

async function sha256File(filePath) {
  return sha256Bytes(await readFile(filePath));
}

async function readSingleJsonlRow(filePath) {
  const rows = (await readFile(filePath, "utf8"))
    .split(/\r?\n/u)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  if (rows.length !== 1) throw new Error(`expected one raw benchmark row in ${filePath}`);
  return rows[0];
}

function armStateEnv(stateRoot, arm) {
  const root = path.join(stateRoot, arm);
  return {
    CODESTORY_CACHE_ROOT: path.join(root, "cache"),
    CODESTORY_STDIO_CACHE_ROOT: path.join(root, "stdio-cache"),
    CODESTORY_PLUGIN_DATA: path.join(root, "plugin-data"),
    CODESTORY_EMBED_ALLOW_CPU: "0",
    CODESTORY_RETRIEVAL: "1",
  };
}

async function cliIdentity(cliPath, arm) {
  if (!existsSync(cliPath)) throw new Error(`${arm} CLI does not exist: ${cliPath}`);
  const version = await runProcess(cliPath, ["--version"], { timeoutMs: 10_000 });
  if (version.status !== "pass") throw new Error(`${arm} CLI version probe failed`);
  return {
    arm,
    path: cliPath,
    sha256: await sha256File(cliPath),
    version: version.stdout.trim(),
  };
}

function transientEmbeddingServerTransition(summary) {
  const failure = summary?.first_failure;
  return summary?.completed_rows === 0
    && failure?.kind === "preparation_failed"
    && String(failure?.error ?? "").includes("embedding_server_draining");
}

async function runRawRow(opts, planned, sequence, cli) {
  const rowRoot = path.join(
    opts.outDir,
    "raw",
    planned.task_id,
    `${String(sequence + 1).padStart(2, "0")}-${planned.arm}-${planned.repeat}`,
  );
  const transitionAttempts = [];
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const rowDir = path.join(rowRoot, `attempt-${attempt}`);
    const run = await runProcess(
      process.execPath,
      [
        harnessPath,
        "--task-suite", "language-expansion-holdout",
        "--task-ids", planned.task_id,
        "--arms", "with_codestory",
        "--repeats", "1",
        "--model", PINNED_MODEL,
        "--repo-cache-dir", opts.repoCacheDir,
        "--codestory-cli", cli,
        "--out-dir", rowDir,
        "--allow-failures",
      ],
      {
        cwd: repoRoot,
        env: { ...process.env, ...armStateEnv(opts.stateRoot, planned.arm) },
        timeoutMs: opts.timeoutMs,
        maxOutputBytes: 4 * 1024 * 1024,
      },
    );
    if (run.status === "pass") {
      return { rowDir, transitionAttempts };
    }
    const summaryPath = path.join(rowDir, "summary.json");
    const summary = existsSync(summaryPath)
      ? JSON.parse(await readFile(summaryPath, "utf8"))
      : null;
    const transient = transientEmbeddingServerTransition(summary);
    transitionAttempts.push({
      attempt,
      raw_directory: path.relative(opts.outDir, rowDir),
      transient_embedding_server_transition: transient,
      error: summary?.first_failure?.error ?? run.stderr ?? run.stdout,
    });
    if (!transient || attempt === 3) {
      throw new Error(
        `focused ABBA raw row failed for ${planned.task_id}/${planned.arm}/${planned.repeat}: `
        + `${summary?.first_failure?.error ?? run.stderr ?? run.stdout}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 2_000));
  }
  throw new Error("focused ABBA transition retry exhausted without a receipt");
}

async function runFocusedAbba(opts) {
  const plan = abbaRunPlan();
  if (opts.listPlan) {
    process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`);
    return null;
  }
  if (existsSync(path.join(opts.outDir, "summary.json"))) {
    throw new Error(`refusing to overwrite focused ABBA receipt: ${opts.outDir}`);
  }
  await mkdir(opts.outDir, { recursive: true });
  await mkdir(opts.stateRoot, { recursive: true });
  const executionWindowId = opts.executionWindowId
    ?? `${new Date().toISOString()}:${process.pid}:${opts.outDir}`;
  const identities = {
    published_0_17_5: await cliIdentity(opts.publishedCli, "published_0_17_5"),
    candidate_0_18: await cliIdentity(opts.candidateCli, "candidate_0_18"),
  };
  const rows = [];
  for (const [sequence, planned] of plan.entries()) {
    const cli = identities[planned.arm].path;
    process.stdout.write(
      `running ${planned.task_id} ${planned.arm} repeat ${planned.repeat}/5 (${sequence + 1}/${plan.length})\n`,
    );
    const { rowDir, transitionAttempts } = await runRawRow(opts, planned, sequence, cli);
    const rawRow = await readSingleJsonlRow(path.join(rowDir, "runs.jsonl"));
    const rawSummary = JSON.parse(await readFile(path.join(rowDir, "summary.json"), "utf8"));
    const host = rawSummary?.shard?.attestation?.host_class;
    const timing = focusedAbbaTiming(rawRow, {
      execution_window_id: executionWindowId,
      host,
      model: rawRow.model ?? rawRow.benchmark_contract?.model ?? PINNED_MODEL,
      load_policy: "fresh_cli_fresh_agent_session",
      task_id: planned.task_id,
      repeat: planned.repeat,
    });
    rows.push({
      contract: "codestory.focused-installed-abba-row/v1",
      sequence: sequence + 1,
      task_id: planned.task_id,
      arm: planned.arm,
      repeat: planned.repeat,
      raw_directory: path.relative(opts.outDir, rowDir),
      transition_attempts: transitionAttempts,
      raw_benchmark_run_id: rawRow.benchmark_run_id,
      raw_inner_timing_cohort_id: rawRow.installed_agent_timing?.timing_cohort_id ?? null,
      installed_agent_timing: timing,
      installed_agent_timing_eligible: true,
      quality: rawRow.quality,
      packet: {
        status: rawRow.codestory_harness_prelude?.packet_evidence_availability?.status ?? null,
        evidence_kind_counts:
          rawRow.codestory_harness_prelude?.packet_evidence_availability?.evidence_kind_counts ?? null,
        gap_kind_counts:
          rawRow.codestory_harness_prelude?.packet_evidence_availability?.gap_kind_counts ?? null,
        bytes: rawRow.codestory_harness_prelude?.stdout_bytes ?? null,
        transport_cell: rawRow.codestory_harness_prelude?.transport_cell ?? null,
      },
      usage: rawRow.usage,
      host,
      model: rawRow.model ?? rawRow.benchmark_contract?.model ?? PINNED_MODEL,
      source_attestation: rawSummary?.shard?.attestation ?? null,
      cli_identity: identities[planned.arm],
    });
  }
  const identityAfter = {
    published_0_17_5: await cliIdentity(opts.publishedCli, "published_0_17_5"),
    candidate_0_18: await cliIdentity(opts.candidateCli, "candidate_0_18"),
  };
  if (stableJson(identityAfter) !== stableJson(identities)) {
    throw new Error("focused ABBA CLI identity changed inside the execution window");
  }
  const receipt = {
    contract: "codestory.focused-installed-abba-preflight/v1",
    generated_at: new Date().toISOString(),
    execution_window_id: executionWindowId,
    task_ids: REQUIRED_TASK_IDS,
    repeats_per_arm: 5,
    ordering: "ABBAABBAAB per task",
    load_policy: "fresh_cli_fresh_agent_session",
    persistent_installed_mcp_measured: false,
    cli_identities: identities,
    rows,
  };
  receipt.receipt_sha256 = sha256Bytes(stableJson(receipt));
  await writeFile(path.join(opts.outDir, "summary.json"), `${JSON.stringify(receipt, null, 2)}\n`, "utf8");
  process.stdout.write(`wrote ${opts.outDir}\n`);
  return receipt;
}

export {
  ARMS,
  REQUIRED_TASK_IDS,
  abbaRunPlan,
  focusedAbbaTiming,
  parseArgs,
  runFocusedAbba,
  transientEmbeddingServerTransition,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  runFocusedAbba(parseArgs(process.argv.slice(2))).catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}
