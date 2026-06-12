#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
    prepareCodestoryCache: true,
    materializeRepos: true,
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
  return opts;
}

function usage() {
  console.log(`Usage:
  node scripts/codestory-agent-ab-score.mjs [--task-ids ids] [--repeats n] [--out-dir dir]
  node scripts/codestory-agent-ab-score.mjs --reanalyze-dir target/agent-benchmark/<run>

Runs the real CodeStory agent A/B harness, reanalyzes it with the current
transcript analyzer, and emits METRIC lines for Codex Autoresearch.

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
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
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
  ];
  if (opts.materializeRepos) {
    args.push("--materialize-repos");
  }
  if (opts.prepareCodestoryCache) {
    args.push("--prepare-codestory-cache");
  }

  const result = await runProcess(process.execPath, args);
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

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
