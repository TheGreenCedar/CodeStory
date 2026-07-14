#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const benchmarkScript = path.join(scriptDir, "codestory-agent-ab-benchmark.mjs");
const defaultSmokeTaskIds = "python-requests-session-flow,javascript-express-routing-flow";

function parseArgs(argv) {
  const opts = {
    taskSuite: "language-expansion-holdout",
    taskIds: defaultSmokeTaskIds,
    repeats: 1,
    sandbox: "danger-full-access",
    repoCacheDir: path.join(repoRoot, "target", "oss-language-corpus", "repos"),
    outDir: null,
    reanalyzeDir: null,
    timeoutMs: 600000,
    prepareCodestoryTimeoutMs: 1_800_000,
    prepareCodestoryCache: true,
    materializeRepos: true,
    jobs: 1,
    packetGate: false,
    allowEmptyPacketGate: false,
    packetProbeJobs: 1,
    packetProbeRepeats: 1,
    packetGateImprovedFrom: null,
    reuseBaselineFrom: null,
    prepareCodestoryJobs: 1,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
    if (arg === "--task-suite") {
      opts.taskSuite = argv[++i];
      continue;
    }
    if (arg === "--task-ids") {
      opts.taskIds = argv[++i];
      continue;
    }
    if (arg === "--repeats") {
      opts.repeats = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--sandbox") {
      opts.sandbox = argv[++i];
      continue;
    }
    if (arg === "--repo-cache-dir") {
      opts.repoCacheDir = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--out-dir") {
      opts.outDir = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--reanalyze-dir") {
      opts.reanalyzeDir = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--timeout-ms") {
      opts.timeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--prepare-codestory-timeout-ms") {
      opts.prepareCodestoryTimeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--jobs") {
      opts.jobs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--packet-gate") {
      opts.packetGate = true;
      continue;
    }
    if (arg === "--allow-empty-packet-gate") {
      opts.allowEmptyPacketGate = true;
      continue;
    }
    if (arg === "--packet-probe-jobs") {
      opts.packetProbeJobs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--packet-probe-repeats") {
      opts.packetProbeRepeats = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--packet-gate-improved-from") {
      opts.packetGateImprovedFrom = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--reuse-baseline-from") {
      opts.reuseBaselineFrom = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--prepare-codestory-jobs") {
      opts.prepareCodestoryJobs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--no-prepare-codestory-cache") {
      opts.prepareCodestoryCache = false;
      continue;
    }
    if (arg === "--no-materialize-repos") {
      opts.materializeRepos = false;
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(opts.repeats) || opts.repeats < 1) {
    throw new Error("--repeats must be a positive integer");
  }
  if (!Number.isInteger(opts.timeoutMs) || opts.timeoutMs < 1000) {
    throw new Error("--timeout-ms must be at least 1000");
  }
  if (!Number.isInteger(opts.prepareCodestoryTimeoutMs) || opts.prepareCodestoryTimeoutMs < 1000) {
    throw new Error("--prepare-codestory-timeout-ms must be at least 1000");
  }
  if (!Number.isInteger(opts.jobs) || opts.jobs < 1) {
    throw new Error("--jobs must be a positive integer");
  }
  if (!Number.isInteger(opts.packetProbeJobs) || opts.packetProbeJobs < 1) {
    throw new Error("--packet-probe-jobs must be a positive integer");
  }
  if (!Number.isInteger(opts.packetProbeRepeats) || opts.packetProbeRepeats < 1) {
    throw new Error("--packet-probe-repeats must be a positive integer");
  }
  if (!Number.isInteger(opts.prepareCodestoryJobs) || opts.prepareCodestoryJobs < 1) {
    throw new Error("--prepare-codestory-jobs must be a positive integer");
  }
  if (opts.packetGateImprovedFrom && !opts.packetGate) {
    throw new Error("--packet-gate-improved-from requires --packet-gate");
  }
  return opts;
}

function usage() {
  console.log(`Usage:
  node scripts/codestory-agent-ab-score.mjs [--task-ids ids] [--repeats n] [--out-dir dir] [--prepare-codestory-timeout-ms ms]
      [--jobs n] [--prepare-codestory-jobs n] [--packet-gate] [--allow-empty-packet-gate] [--packet-probe-jobs n]
      [--packet-gate-improved-from dir] [--reuse-baseline-from dir]
  node scripts/codestory-agent-ab-score.mjs --reanalyze-dir target/agent-benchmark/<run>

Runs the real CodeStory agent A/B harness, reanalyzes it with the current
transcript analyzer, and emits METRIC lines for Codex Autoresearch.
Packet-gate mode automatically retries transient sidecar-unavailable packet
probe rows once, serially, before selecting nested A/B tasks. It exits non-zero
when no tasks are selected unless --allow-empty-packet-gate is present for an
exploratory diagnostic run.

Default smoke task ids: ${defaultSmokeTaskIds}`);
}

function timestampId() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

async function runProcess(command, args, options = {}) {
  return await new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd: options.cwd ?? repoRoot,
      env: options.env ?? process.env,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      const text = chunk.toString();
      stdout += text;
      if (options.streamOutput) {
        process.stdout.write(text);
      }
    });
    child.stderr.on("data", (chunk) => {
      const text = chunk.toString();
      stderr += text;
      if (options.streamOutput) {
        process.stderr.write(text);
      }
    });
    child.on("error", (error) => {
      resolve({ status: "error", exitCode: null, stdout, stderr, error });
    });
    child.on("close", (exitCode, signal) => {
      resolve({
        status: exitCode === 0 ? "pass" : "fail",
        exitCode,
        signal,
        stdout,
        stderr,
        error: null,
      });
    });
  });
}

function artifactNamePart(value) {
  const normalized = String(value ?? "")
    .trim()
    .replace(/[^A-Za-z0-9_.-]+/g, "-")
    .replace(/^[.-]+|[.-]+$/g, "");
  if (!normalized || normalized === "." || normalized === "..") {
    return "unknown";
  }
  return normalized;
}

function benchmarkArtifactStem(parts) {
  return parts.map(artifactNamePart).join("-");
}

async function runBenchmark(opts, outDir) {
  const args = [
    benchmarkScript,
    "--task-suite",
    opts.taskSuite,
    "--task-ids",
    opts.taskIds,
    "--arms",
    "without_codestory,with_codestory",
    "--repeats",
    String(opts.repeats),
    "--repo-cache-dir",
    opts.repoCacheDir,
    "--sandbox",
    opts.sandbox,
    "--allow-failures",
    "--out-dir",
    outDir,
    "--timeout-ms",
    String(opts.timeoutMs),
    "--prepare-codestory-timeout-ms",
    String(opts.prepareCodestoryTimeoutMs),
    "--jobs",
    String(opts.jobs),
    "--prepare-codestory-jobs",
    String(opts.prepareCodestoryJobs),
  ];
  if (opts.materializeRepos) {
    args.push("--materialize-repos");
  }
  if (opts.prepareCodestoryCache) {
    args.push("--prepare-codestory-cache");
  }
  if (opts.reuseBaselineFrom) {
    args.push("--reuse-baseline-from", opts.reuseBaselineFrom);
  }

  const result = await runProcess(process.execPath, args, { streamOutput: true });
  if (result.status !== "pass") {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`A/B benchmark command failed with exit ${result.exitCode ?? result.status}`);
  }
}

async function reanalyze(outDir) {
  const result = await runProcess(process.execPath, [
    benchmarkScript,
    "--reanalyze-dir",
    outDir,
  ]);
  if (result.status !== "pass") {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`A/B reanalysis command failed with exit ${result.exitCode ?? result.status}`);
  }
}

async function runPacketProbeBenchmark(opts, gateDir, taskIds, jobs, prepareJobs) {
  const args = [
    benchmarkScript,
    "--packet-runtime",
    "--packet-runtime-mode",
    "cold-cli",
    "--task-suite",
    opts.taskSuite,
    "--task-ids",
    taskIds,
    "--repeats",
    String(opts.packetProbeRepeats),
    "--repo-cache-dir",
    opts.repoCacheDir,
    "--out-dir",
    gateDir,
    "--timeout-ms",
    String(opts.timeoutMs),
    "--prepare-codestory-timeout-ms",
    String(opts.prepareCodestoryTimeoutMs),
    "--jobs",
    String(jobs),
    "--prepare-codestory-jobs",
    String(prepareJobs),
    "--allow-failures",
  ];
  if (opts.materializeRepos) {
    args.push("--materialize-repos");
  }
  if (opts.prepareCodestoryCache) {
    args.push("--prepare-codestory-cache");
  }

  const result = await runProcess(process.execPath, args, { streamOutput: true });
  if (result.status !== "pass") {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`packet gate command failed with exit ${result.exitCode ?? result.status}`);
  }
}

async function runPacketGate(opts, outDir) {
  const gateDir = path.join(outDir, "packet-probes");
  mkdirSync(gateDir, { recursive: true });
  await runPacketProbeBenchmark(opts, gateDir, opts.taskIds, opts.packetProbeJobs, opts.prepareCodestoryJobs);

  const qualityDebugPath = path.join(gateDir, "quality-debug.json");
  const qualityDebug = readJsonFileIfPresent(qualityDebugPath);
  const retryableTaskIds = retryablePacketGateTaskIds(qualityDebug?.rows ?? [], gateDir);
  let selectedQualityDebugPath = qualityDebugPath;
  let selectedRows = qualityDebug?.rows ?? [];
  let retryDir = null;
  let retryQualityDebugPath = null;
  if (retryableTaskIds.length) {
    retryDir = path.join(outDir, "packet-probes-retry");
    mkdirSync(retryDir, { recursive: true });
    console.log(`packet gate retrying transient sidecar failures: ${retryableTaskIds.join(",")}`);
    await runPacketProbeBenchmark(opts, retryDir, retryableTaskIds.join(","), 1, 1);
    retryQualityDebugPath = path.join(retryDir, "quality-debug.json");
    const retryQualityDebug = readJsonFileIfPresent(retryQualityDebugPath);
    selectedRows = mergePacketGateRows(selectedRows, retryQualityDebug?.rows ?? [], retryableTaskIds);
    selectedQualityDebugPath = path.join(gateDir, "quality-debug-merged.json");
    writeFileSync(
      selectedQualityDebugPath,
      `${JSON.stringify(
        {
          ...(qualityDebug ?? {}),
          scope: "packet_runtime_quality_debug_with_retry",
          retry: {
            retry_dir: retryDir,
            retry_quality_debug: retryQualityDebugPath,
            retried_task_ids: retryableTaskIds,
          },
          rows: selectedRows,
        },
        null,
        2,
      )}\n`,
      "utf8",
    );
  }
  const byTask = rowsByTask(selectedRows);
  const baseline = opts.packetGateImprovedFrom
    ? loadPacketGateBaselineRows(opts.packetGateImprovedFrom)
    : null;
  const baselineByTask = baseline ? rowsByTask(baseline.rows) : null;
  const selected = [];
  const improved = [];
  const unchangedOrMissing = [];
  for (const [taskId, rows] of byTask) {
    if (!packetGateTaskPasses(rows)) {
      continue;
    }
    if (baselineByTask) {
      const improvement = packetGateImprovement(rows, baselineByTask.get(taskId) ?? []);
      if (!improvement.improved) {
        unchangedOrMissing.push({ taskId, reason: improvement.reason });
        continue;
      }
      improved.push({ taskId, reason: improvement.reason });
    }
    if (rows.length) {
      selected.push(taskId);
    }
  }
  selected.sort();
  improved.sort((a, b) => a.taskId.localeCompare(b.taskId));
  unchangedOrMissing.sort((a, b) => a.taskId.localeCompare(b.taskId));

  console.log(`METRIC packet_gate_scored_tasks=${byTask.size}`);
  console.log(`METRIC packet_gate_selected_tasks=${selected.length}`);
  console.log(`METRIC packet_gate_retry_tasks=${retryableTaskIds.length}`);
  if (baselineByTask) {
    console.log(`METRIC packet_gate_baseline_tasks=${baselineByTask.size}`);
    console.log(`METRIC packet_gate_improved_tasks=${improved.length}`);
  }
  console.log(`ARTIFACT packet_gate_dir=${path.relative(repoRoot, gateDir)}`);
  if (existsSync(qualityDebugPath)) {
    console.log(`ARTIFACT packet_gate_quality_debug=${path.relative(repoRoot, qualityDebugPath)}`);
  }
  if (retryDir) {
    console.log(`ARTIFACT packet_gate_retry_dir=${path.relative(repoRoot, retryDir)}`);
  }
  if (retryQualityDebugPath && existsSync(retryQualityDebugPath)) {
    console.log(`ARTIFACT packet_gate_retry_quality_debug=${path.relative(repoRoot, retryQualityDebugPath)}`);
  }
  if (selectedQualityDebugPath !== qualityDebugPath && existsSync(selectedQualityDebugPath)) {
    console.log(`ARTIFACT packet_gate_quality_debug_merged=${path.relative(repoRoot, selectedQualityDebugPath)}`);
  }
  if (baseline?.path) {
    console.log(`ARTIFACT packet_gate_improvement_baseline=${path.relative(repoRoot, baseline.path)}`);
  }
  if (!selected.length) {
    console.log("packet gate selected no tasks; skipping nested A/B run");
    if (unchangedOrMissing.length) {
      console.log(
        `packet gate skipped unchanged tasks: ${unchangedOrMissing.map((row) => `${row.taskId}:${row.reason}`).join(",")}`,
      );
    }
    return packetGateSelectionOrThrow(selected, unchangedOrMissing, opts);
  }
  if (improved.length) {
    console.log(`packet gate improved tasks: ${improved.map((row) => `${row.taskId}:${row.reason}`).join(",")}`);
  }
  console.log(`packet gate selected tasks: ${selected.join(",")}`);
  return selected;
}

function packetGateSelectionOrThrow(selected, unchangedOrMissing = [], opts = {}) {
  if (selected.length) {
    return selected;
  }
  if (opts.allowEmptyPacketGate) {
    return null;
  }
  const skipped = unchangedOrMissing.length
    ? ` Skipped tasks: ${unchangedOrMissing.map((row) => `${row.taskId}:${row.reason}`).join(",")}.`
    : "";
  throw new Error(
    `packet gate selected no nested A/B tasks; pass --allow-empty-packet-gate only for exploratory diagnostics.${skipped}`,
  );
}

function readJsonl(filePath) {
  return readFileSync(filePath, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

function readJsonFileIfPresent(filePath) {
  if (!existsSync(filePath)) {
    return null;
  }
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function resolvePacketGateBaselinePath(sourcePath) {
  if (!sourcePath) {
    return null;
  }
  const candidates = [];
  if (sourcePath.toLowerCase().endsWith(".json") || sourcePath.toLowerCase().endsWith(".jsonl")) {
    candidates.push(sourcePath);
  }
  candidates.push(
    path.join(sourcePath, "quality-debug.json"),
    path.join(sourcePath, "packet-probes", "quality-debug.json"),
    path.join(sourcePath, "reanalyzed-runs.jsonl"),
  );
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

function packetQualityDebugRowsFromAbRows(filePath) {
  return readJsonl(filePath)
    .filter((row) => row.arm === "with_codestory" && row.codestory_harness_prelude?.packet_manifest_quality)
    .map((row) => {
      const quality = row.codestory_harness_prelude.packet_manifest_quality;
      return {
        repo: row.repo,
        task_id: row.task_id,
        mode: "with_codestory_packet_prelude",
        repeat: row.repeat ?? null,
        status: row.codestory_harness_prelude.status ?? row.status ?? null,
        quality_pass: quality.pass === true,
        failure_reasons: quality.failure_reasons ?? [],
        quality_metrics: {
          expected_file_recall: quality.expected_file_recall,
          expected_symbol_recall: quality.expected_symbol_recall,
          expected_claim_recall: quality.expected_claim_recall,
          citation_coverage: quality.citation_coverage,
          expected_anchor_recall: quality.expected_anchor_recall,
          forbidden_claims_found: quality.forbidden_claims_found,
        },
        missed_anchors: quality.missed_anchors ?? {},
      };
    });
}

function loadPacketGateBaselineRows(sourcePath) {
  const resolved = resolvePacketGateBaselinePath(sourcePath);
  if (!resolved) {
    throw new Error(`--packet-gate-improved-from did not contain packet quality evidence: ${sourcePath}`);
  }
  if (resolved.endsWith(".jsonl")) {
    return { path: resolved, rows: packetQualityDebugRowsFromAbRows(resolved) };
  }
  const payload = readJsonFileIfPresent(resolved);
  return { path: resolved, rows: Array.isArray(payload?.rows) ? payload.rows : [] };
}

const transientSidecarFailurePatterns = [
  /\bretrieval_unavailable\b/i,
  /\bvector_generation_unavailable\b/i,
  /\blexical_(?:shard_unavailable|source_coverage_incomplete)\b/i,
  /\bscip_unreachable\b/i,
  /sidecar retrieval .* unavailable/i,
  /sidecar retrieval .* failed/i,
  /retrieval sidecar is mandatory/i,
  /project is not in full mode/i,
];

function packetGateStderrPath(gateDir, row) {
  const mode = String(row?.mode ?? "cold_cli_packet").replaceAll("_", "-");
  const repeat = String(row?.repeat ?? 1).padStart(2, "0");
  const stem = benchmarkArtifactStem([row?.repo, row?.task_id, mode, repeat]);
  return path.join(gateDir, `${stem}.stderr.txt`);
}

function packetGateRowHasTransientSidecarFailure(row, gateDir) {
  if (row?.status === "pass") {
    return false;
  }
  const failureReasons = Array.isArray(row?.failure_reasons) ? row.failure_reasons : [];
  if (row?.quality_pass !== null && !failureReasons.includes("missing_quality_score")) {
    return false;
  }
  const stderrPath = packetGateStderrPath(gateDir, row);
  if (!existsSync(stderrPath)) {
    return false;
  }
  const stderr = readFileSync(stderrPath, "utf8");
  return transientSidecarFailurePatterns.some((pattern) => pattern.test(stderr));
}

function retryablePacketGateTaskIds(rows, gateDir) {
  const taskIds = new Set();
  for (const row of rows ?? []) {
    if (packetGateRowHasTransientSidecarFailure(row, gateDir) && row?.task_id) {
      taskIds.add(row.task_id);
    }
  }
  return [...taskIds].sort();
}

function mergePacketGateRows(initialRows, retryRows, retriedTaskIds) {
  const retried = new Set(retriedTaskIds);
  const merged = (initialRows ?? []).filter((row) => !retried.has(row?.task_id));
  merged.push(...(retryRows ?? []).filter((row) => retried.has(row?.task_id)));
  return merged;
}

function rowsByTask(rows) {
  const byTask = new Map();
  for (const row of rows ?? []) {
    const taskId = row.task_id;
    if (!taskId) {
      continue;
    }
    if (!byTask.has(taskId)) {
      byTask.set(taskId, []);
    }
    byTask.get(taskId).push(row);
  }
  return byTask;
}

function packetGateRowPasses(row) {
  return row?.status === "pass" && row?.quality_pass === true;
}

function packetGateTaskPasses(rows) {
  return rows.length > 0 && rows.every((row) => packetGateRowPasses(row));
}

const packetGateQualityMetricNames = [
  "expected_file_recall",
  "expected_symbol_recall",
  "expected_claim_recall",
  "citation_coverage",
  "expected_anchor_recall",
];

function averageFinite(values) {
  const nums = values.filter((value) => Number.isFinite(value));
  if (!nums.length) {
    return null;
  }
  return nums.reduce((sum, value) => sum + value, 0) / nums.length;
}

function packetGateMetricAverage(rows, name) {
  return averageFinite(rows.map((row) => row.quality_metrics?.[name]));
}

function missedAnchorCount(row) {
  const missed = row?.missed_anchors;
  if (!missed || typeof missed !== "object") {
    return null;
  }
  let count = 0;
  let sawArray = false;
  for (const value of Object.values(missed)) {
    if (Array.isArray(value)) {
      sawArray = true;
      count += value.length;
    }
  }
  return sawArray ? count : null;
}

function failureReasonCount(row) {
  return Array.isArray(row?.failure_reasons) ? row.failure_reasons.length : 0;
}

function packetGateTaskProfile(rows) {
  const metrics = Object.fromEntries(
    packetGateQualityMetricNames.map((name) => [name, packetGateMetricAverage(rows, name)]),
  );
  return {
    rows: rows.length,
    passRate: rows.length ? rows.filter((row) => packetGateRowPasses(row)).length / rows.length : 0,
    metrics,
    missedAnchors: averageFinite(rows.map((row) => missedAnchorCount(row))),
    failureReasons: averageFinite(rows.map((row) => failureReasonCount(row))) ?? 0,
  };
}

function packetGateImprovement(currentRows, baselineRows) {
  if (!baselineRows?.length) {
    return { improved: false, reason: "missing_baseline_task" };
  }
  const current = packetGateTaskProfile(currentRows);
  const baseline = packetGateTaskProfile(baselineRows);
  const epsilon = 1e-9;
  if (current.passRate > baseline.passRate + epsilon) {
    return { improved: true, reason: "quality_pass_rate", current, baseline };
  }
  for (const name of packetGateQualityMetricNames) {
    const currentMetric = current.metrics[name];
    const baselineMetric = baseline.metrics[name];
    if (
      Number.isFinite(currentMetric) &&
      Number.isFinite(baselineMetric) &&
      currentMetric > baselineMetric + epsilon
    ) {
      return { improved: true, reason: name, current, baseline };
    }
  }
  if (
    Number.isFinite(current.missedAnchors) &&
    Number.isFinite(baseline.missedAnchors) &&
    current.missedAnchors < baseline.missedAnchors - epsilon
  ) {
    return { improved: true, reason: "missed_anchors", current, baseline };
  }
  if (
    Number.isFinite(current.failureReasons) &&
    Number.isFinite(baseline.failureReasons) &&
    current.failureReasons < baseline.failureReasons - epsilon
  ) {
    return { improved: true, reason: "failure_reasons", current, baseline };
  }
  return { improved: false, reason: "not_improved", current, baseline };
}

function median(values) {
  const nums = values.filter((value) => Number.isFinite(value)).sort((a, b) => a - b);
  if (!nums.length) {
    return null;
  }
  const middle = Math.floor(nums.length / 2);
  return nums.length % 2 ? nums[middle] : (nums[middle - 1] + nums[middle]) / 2;
}

function sumFinite(values) {
  return values.reduce((sum, value) => (Number.isFinite(value) ? sum + value : sum), 0);
}

function sumPresentFinite(values) {
  const nums = values.filter((value) => Number.isFinite(value));
  if (!nums.length) {
    return null;
  }
  return nums.reduce((sum, value) => sum + value, 0);
}

function cachePreparationWallMs(preparation) {
  if (!preparation) {
    return null;
  }
  if (Number.isFinite(preparation.preparation_wall_ms)) {
    return preparation.preparation_wall_ms;
  }
  const indexMs = Number.isFinite(preparation.index_wall_ms) ? preparation.index_wall_ms : 0;
  const retrievalIndexMs = Number.isFinite(preparation.retrieval_index_wall_ms)
    ? preparation.retrieval_index_wall_ms
    : 0;
  const fallback = indexMs + retrievalIndexMs;
  return fallback > 0 ? fallback : null;
}

function summarizeCachePreparation(outDir) {
  const rows = readJsonFileIfPresent(path.join(outDir, "codestory-cache-preparation.json")) ?? [];
  return {
    rows: Array.isArray(rows) ? rows.length : 0,
    preparationWallMs: Array.isArray(rows)
      ? sumFinite(rows.map((row) => cachePreparationWallMs(row)))
      : null,
    indexWallMs: Array.isArray(rows) ? sumFinite(rows.map((row) => row.index_wall_ms)) : null,
    retrievalIndexWallMs: Array.isArray(rows)
      ? sumFinite(rows.map((row) => row.retrieval_index_wall_ms))
      : null,
  };
}

function summarizeArm(rows, arm) {
  const armRows = rows.filter((row) => row.arm === arm);
  const successful = armRows.filter((row) => row.status === "pass");
  return {
    rows: armRows.length,
    successful: successful.length,
    qualityPass: successful.filter((row) => row.quality?.pass).length,
    packetFirstPass: successful.filter((row) => row.packet_first_required && row.packet_first_pass).length,
    packetFirstRequired: successful.filter((row) => row.packet_first_required).length,
    packetManifestQualityPass: successful.filter(
      (row) => row.codestory_harness_prelude?.packet_manifest_quality?.pass,
    ).length,
    packetManifestQualityScored: successful.filter(
      (row) => row.codestory_harness_prelude?.packet_manifest_quality,
    ).length,
    packetPartial: successful.filter(
      (row) => row.codestory_harness_prelude?.packet_sufficiency_status === "partial",
    ).length,
    totalWallMs: sumFinite(successful.map((row) => row.wall_ms)),
    totalInputTokens: sumFinite(successful.map((row) => row.usage?.input_tokens)),
    totalOutputTokens: sumFinite(successful.map((row) => row.usage?.output_tokens)),
    totalTokens: sumFinite(successful.map((row) => row.usage?.total_tokens)),
    totalEstimatedCostUsd: sumPresentFinite(successful.map((row) => row.estimated_cost_usd)),
    totalToolCalls: sumFinite(successful.map((row) => row.tool_calls_observed)),
    totalCommands: sumFinite(successful.map((row) => row.transcript_analysis?.command_count)),
    medianWallMs: median(successful.map((row) => row.wall_ms)),
    medianInputTokens: median(successful.map((row) => row.usage?.input_tokens)),
    medianOutputTokens: median(successful.map((row) => row.usage?.output_tokens)),
    medianTokens: median(successful.map((row) => row.usage?.total_tokens)),
    medianEstimatedCostUsd: median(successful.map((row) => row.estimated_cost_usd)),
    medianToolCalls: median(successful.map((row) => row.tool_calls_observed)),
    medianCommands: median(successful.map((row) => row.transcript_analysis?.command_count)),
    medianCodeStoryCommands: median(
      successful.map((row) => row.transcript_analysis?.command_categories?.codestory_cli ?? 0),
    ),
    medianShellSearchCommands: median(
      successful.map((row) => row.transcript_analysis?.command_categories?.shell_search ?? 0),
    ),
    medianFileReadCommands: median(
      successful.map((row) => row.transcript_analysis?.command_categories?.direct_file_read ?? 0),
    ),
    medianWebSearches: median(successful.map((row) => row.transcript_analysis?.tool_categories?.web_search ?? 0)),
    medianPostPacketReads: median(
      successful.map((row) => row.transcript_analysis?.ordinary_source_reads_after_first_packet),
    ),
  };
}

function safeRatio(numerator, denominator, fallback = 999) {
  if (!Number.isFinite(numerator) || !Number.isFinite(denominator) || denominator <= 0) {
    return fallback;
  }
  return numerator / denominator;
}

function score(rows) {
  const without = summarizeArm(rows, "without_codestory");
  const withCodeStory = summarizeArm(rows, "with_codestory");
  const tokenRatio = safeRatio(withCodeStory.medianTokens, without.medianTokens);
  const wallRatio = safeRatio(withCodeStory.medianWallMs, without.medianWallMs);
  const toolRatio = safeRatio(withCodeStory.medianToolCalls, without.medianToolCalls);
  const commandRatio = safeRatio(withCodeStory.medianCommands, without.medianCommands);

  const withQualityPenalty =
    withCodeStory.qualityPass === withCodeStory.successful && withCodeStory.successful > 0 ? 0 : 1000000;
  const packetPenalty =
    withCodeStory.packetFirstRequired > 0 && withCodeStory.packetFirstPass === withCodeStory.packetFirstRequired
      ? 0
      : 250000;
  const postPacketReadPenalty = Math.max(0, withCodeStory.medianPostPacketReads ?? 0) * 100000;
  const externalPenalty =
    Math.max(0, without.medianWebSearches ?? 0) * 100000 +
    Math.max(0, withCodeStory.medianWebSearches ?? 0) * 100000;

  const efficiencyScore =
    tokenRatio * 1000 +
    wallRatio * 1000 +
    toolRatio * 250 +
    commandRatio * 250;
  const agentAbGap =
    efficiencyScore +
    withQualityPenalty +
    packetPenalty +
    postPacketReadPenalty +
    externalPenalty;

  return {
    agentAbGap,
    tokenRatio,
    wallRatio,
    toolRatio,
    commandRatio,
    without,
    withCodeStory,
    penalties: {
      withQualityPenalty,
      packetPenalty,
      postPacketReadPenalty,
      externalPenalty,
    },
  };
}

function printMetric(name, value) {
  if (Number.isFinite(value)) {
    console.log(`METRIC ${name}=${value}`);
  }
}

function printArtifacts(outDir) {
  console.log(`ARTIFACT out_dir=${path.relative(repoRoot, outDir)}`);
  for (const name of ["reanalyzed-summary.md", "reanalyzed-runs.jsonl", "summary.md", "runs.jsonl"]) {
    const filePath = path.join(outDir, name);
    if (existsSync(filePath)) {
      console.log(`ARTIFACT ${name.replace(/[^A-Za-z0-9_]+/g, "_")}=${path.relative(repoRoot, filePath)}`);
    }
  }
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const outDir = opts.reanalyzeDir ?? opts.outDir ?? path.join(repoRoot, "target", "agent-benchmark", "autoresearch-agent-ab", timestampId());
  mkdirSync(outDir, { recursive: true });

  if (!opts.reanalyzeDir) {
    if (opts.packetGate) {
      const selectedTaskIds = await runPacketGate(opts, outDir);
      if (!selectedTaskIds) {
        return;
      }
      opts.taskIds = selectedTaskIds.join(",");
    }
    await runBenchmark(opts, outDir);
  }
  await reanalyze(outDir);

  const rowsPath = path.join(outDir, "reanalyzed-runs.jsonl");
  const rows = readJsonl(rowsPath);
  const cachePreparation = summarizeCachePreparation(outDir);
  const result = score(rows);
  const withTotalWallIncludingPreparation =
    result.withCodeStory.totalWallMs + (cachePreparation.preparationWallMs ?? 0);
  const allInWallRatio = safeRatio(withTotalWallIncludingPreparation, result.without.totalWallMs);
  const totalTokenRatio = safeRatio(result.withCodeStory.totalTokens, result.without.totalTokens);
  const totalToolRatio = safeRatio(result.withCodeStory.totalToolCalls, result.without.totalToolCalls);
  const totalCommandRatio = safeRatio(result.withCodeStory.totalCommands, result.without.totalCommands);
  const agentAbGapAllIn =
    totalTokenRatio * 1000 +
    allInWallRatio * 1000 +
    totalToolRatio * 250 +
    totalCommandRatio * 250 +
    result.penalties.withQualityPenalty +
    result.penalties.packetPenalty +
    result.penalties.postPacketReadPenalty +
    result.penalties.externalPenalty;

  printMetric("agent_ab_gap", result.agentAbGap);
  printMetric("agent_ab_gap_all_in", agentAbGapAllIn);
  printMetric("token_ratio", result.tokenRatio);
  printMetric("wall_ratio", result.wallRatio);
  printMetric("all_in_wall_ratio", allInWallRatio);
  printMetric("total_token_ratio", totalTokenRatio);
  printMetric("total_tool_ratio", totalToolRatio);
  printMetric("total_command_ratio", totalCommandRatio);
  printMetric("tool_ratio", result.toolRatio);
  printMetric("command_ratio", result.commandRatio);
  printMetric("without_quality_passes", result.without.qualityPass);
  printMetric("with_quality_passes", result.withCodeStory.qualityPass);
  printMetric("quality_pass_delta", result.withCodeStory.qualityPass - result.without.qualityPass);
  printMetric("with_packet_first_passes", result.withCodeStory.packetFirstPass);
  printMetric("with_packet_manifest_quality_passes", result.withCodeStory.packetManifestQualityPass);
  printMetric("with_packet_manifest_quality_scored", result.withCodeStory.packetManifestQualityScored);
  printMetric("with_partial_packets", result.withCodeStory.packetPartial);
  printMetric("with_post_packet_source_reads", result.withCodeStory.medianPostPacketReads ?? 0);
  printMetric("external_web_searches", (result.without.medianWebSearches ?? 0) + (result.withCodeStory.medianWebSearches ?? 0));
  printMetric("with_tokens", result.withCodeStory.medianTokens);
  printMetric("without_tokens", result.without.medianTokens);
  printMetric("with_total_tokens", result.withCodeStory.totalTokens);
  printMetric("without_total_tokens", result.without.totalTokens);
  printMetric("with_input_tokens", result.withCodeStory.medianInputTokens);
  printMetric("without_input_tokens", result.without.medianInputTokens);
  printMetric("with_total_input_tokens", result.withCodeStory.totalInputTokens);
  printMetric("without_total_input_tokens", result.without.totalInputTokens);
  printMetric("with_output_tokens", result.withCodeStory.medianOutputTokens);
  printMetric("without_output_tokens", result.without.medianOutputTokens);
  printMetric("with_total_output_tokens", result.withCodeStory.totalOutputTokens);
  printMetric("without_total_output_tokens", result.without.totalOutputTokens);
  printMetric("with_wall_ms", result.withCodeStory.medianWallMs);
  printMetric("without_wall_ms", result.without.medianWallMs);
  printMetric("with_total_wall_ms", result.withCodeStory.totalWallMs);
  printMetric("without_total_wall_ms", result.without.totalWallMs);
  printMetric("codestory_cache_preparation_repos", cachePreparation.rows);
  printMetric("codestory_cache_preparation_wall_ms", cachePreparation.preparationWallMs);
  printMetric("codestory_cache_index_wall_ms", cachePreparation.indexWallMs);
  printMetric("codestory_retrieval_index_wall_ms", cachePreparation.retrievalIndexWallMs);
  printMetric("with_total_wall_ms_including_codestory_preparation", withTotalWallIncludingPreparation);
  printMetric("with_estimated_cost_usd", result.withCodeStory.medianEstimatedCostUsd);
  printMetric("without_estimated_cost_usd", result.without.medianEstimatedCostUsd);
  printMetric("with_total_estimated_cost_usd", result.withCodeStory.totalEstimatedCostUsd);
  printMetric("without_total_estimated_cost_usd", result.without.totalEstimatedCostUsd);
  printMetric("with_tool_calls", result.withCodeStory.medianToolCalls);
  printMetric("without_tool_calls", result.without.medianToolCalls);
  printMetric("with_total_tool_calls", result.withCodeStory.totalToolCalls);
  printMetric("without_total_tool_calls", result.without.totalToolCalls);
  printMetric("with_commands", result.withCodeStory.medianCommands);
  printMetric("without_commands", result.without.medianCommands);
  printMetric("with_total_commands", result.withCodeStory.totalCommands);
  printMetric("without_total_commands", result.without.totalCommands);
  printMetric("with_codestory_commands", result.withCodeStory.medianCodeStoryCommands);
  printMetric("without_codestory_commands", result.without.medianCodeStoryCommands);
  printMetric("with_shell_search_commands", result.withCodeStory.medianShellSearchCommands);
  printMetric("without_shell_search_commands", result.without.medianShellSearchCommands);
  printMetric("with_file_read_commands", result.withCodeStory.medianFileReadCommands);
  printMetric("without_file_read_commands", result.without.medianFileReadCommands);
  printMetric("with_web_searches", result.withCodeStory.medianWebSearches);
  printMetric("without_web_searches", result.without.medianWebSearches);
  printArtifacts(outDir);

  console.log(
    `A/B score: gap=${result.agentAbGap.toFixed(3)} all_in_gap=${agentAbGapAllIn.toFixed(3)} token_ratio=${result.tokenRatio.toFixed(3)} wall_ratio=${result.wallRatio.toFixed(3)} all_in_wall_ratio=${allInWallRatio.toFixed(3)} with_quality=${result.withCodeStory.qualityPass}/${result.withCodeStory.successful}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}

export {
  mergePacketGateRows,
  packetGateSelectionOrThrow,
  packetGateStderrPath,
  packetGateRowHasTransientSidecarFailure,
  parseArgs,
  retryablePacketGateTaskIds,
};
