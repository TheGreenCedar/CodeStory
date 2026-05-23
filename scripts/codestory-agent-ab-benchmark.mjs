#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const siblingRoot = path.resolve(repoRoot, "..");

const PUBLIC_REPOS = {
  codestory: {
    path: repoRoot,
    prompt:
      "Explain how full indexing flows through CLI, runtime, workspace, indexer, and store, and how that supports search, trail, and snippet.",
  },
};

const LOCAL_REPOS = {
  freelancer: {
    path: path.join(siblingRoot, "freelancer"),
    prompt:
      "Explain how lead, client, and project persistence flows through commands, repositories, and domain types.",
  },
  rootandruntime: {
    path: path.join(siblingRoot, "rootandruntime"),
    prompt:
      "Explain how public writing and social surfaces connect to Payload collections, comment auth, and the elsewhere feed.",
  },
  traderotate: {
    path: path.join(siblingRoot, "traderotate"),
    prompt:
      "Explain how runtime config, wallet context, executor setup, and the hunt loop connect.",
  },
};

const ALL_REPOS = { ...PUBLIC_REPOS, ...LOCAL_REPOS };

const ARMS = {
  without_codestory:
    "Do not use CodeStory, codestory-cli, or codestory-grounding. Use normal repository exploration only.",
  with_codestory:
    "Use CodeStory grounding first if available. If CODESTORY_CLI is set, use that executable; otherwise try codestory-cli on PATH. Run doctor, ground, and focused search, trail, or snippet commands before ordinary source reads. Run index only if the cache is missing and writes are allowed. If CodeStory is unavailable, say so explicitly and continue.",
};

function usage() {
  console.log(`Usage:
  node scripts/codestory-agent-ab-benchmark.mjs --list
  node scripts/codestory-agent-ab-benchmark.mjs [--quick] [--repos names] [--include-local-repos] [--repeats n] [--runner codex] [--model model] [--sandbox mode] [--out-dir path] [--timeout-ms ms] [--allow-failures] [--publishable]

Options:
  --list          Print configured benchmark repositories and exit.
  --quick         Default to repo=codestory and repeats=1 unless explicitly set.
  --repos         Comma-separated repo names. Public: ${Object.keys(PUBLIC_REPOS).join(", ")}. Local optional: ${Object.keys(LOCAL_REPOS).join(", ")}
  --include-local-repos
                  Include local sibling repos in the default non-quick run.
  --repeats       Repeats per repo/arm. Default: 3, or 1 with --quick.
  --runner        Runner command family. Default: codex.
  --model         Optional model passed to codex exec.
  --sandbox       Codex sandbox mode. Default: workspace-write.
  --out-dir       Output directory. Default: target/agent-benchmark/<timestamp>.
  --timeout-ms    Timeout per runner invocation. Default: 600000.
  --allow-failures Exit 0 even when a run fails. Intended only for exploratory dry runs.
  --publishable   Fail unless every run succeeds and reports token usage.
`);
}

function parseArgs(argv) {
  const opts = {
    list: false,
    quick: false,
    repos: null,
    includeLocalRepos: false,
    repeats: null,
    runner: "codex",
    model: null,
    sandbox: "workspace-write",
    outDir: null,
    timeoutMs: 600000,
    allowFailures: false,
    publishable: false,
  };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
    if (arg === "--list") {
      opts.list = true;
      continue;
    }
    if (arg === "--quick") {
      opts.quick = true;
      continue;
    }
    if (arg === "--publishable") {
      opts.publishable = true;
      continue;
    }
    if (arg === "--allow-failures") {
      opts.allowFailures = true;
      continue;
    }
    if (arg === "--include-local-repos") {
      opts.includeLocalRepos = true;
      continue;
    }
    if (arg === "--repos") {
      opts.repos = argv[++i]?.split(",").map((name) => name.trim()).filter(Boolean);
      continue;
    }
    if (arg === "--repeats") {
      opts.repeats = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--runner") {
      opts.runner = argv[++i];
      continue;
    }
    if (arg === "--model") {
      opts.model = argv[++i];
      continue;
    }
    if (arg === "--sandbox") {
      opts.sandbox = argv[++i];
      continue;
    }
    if (arg === "--out-dir") {
      opts.outDir = argv[++i];
      continue;
    }
    if (arg === "--timeout-ms") {
      opts.timeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }

  if (!opts.repos) {
    opts.repos = opts.quick
      ? ["codestory"]
      : [
          ...Object.keys(PUBLIC_REPOS),
          ...(opts.includeLocalRepos ? Object.keys(LOCAL_REPOS) : []),
        ];
  }
  if (!opts.repeats) {
    opts.repeats = opts.quick ? 1 : 3;
  }
  if (!Number.isInteger(opts.repeats) || opts.repeats < 1) {
    throw new Error("--repeats must be a positive integer");
  }
  if (!Number.isInteger(opts.timeoutMs) || opts.timeoutMs < 1000) {
    throw new Error("--timeout-ms must be an integer >= 1000");
  }
  if (!["read-only", "workspace-write", "danger-full-access"].includes(opts.sandbox)) {
    throw new Error("--sandbox must be one of: read-only, workspace-write, danger-full-access");
  }
  for (const name of opts.repos) {
    if (!ALL_REPOS[name]) {
      throw new Error(`Unknown repo '${name}'. Known: ${Object.keys(ALL_REPOS).join(", ")}`);
    }
  }
  return opts;
}

function runnerCommand(opts, repoPath, prompt) {
  if (opts.runner !== "codex") {
    return {
      command: opts.runner,
      args: [prompt],
      stdin: null,
    };
  }

  const command = process.platform === "win32" ? "cmd.exe" : "codex";
  const codexArgs = [
    "exec",
    "--json",
    "--ephemeral",
    "--sandbox",
    opts.sandbox,
    "--cd",
    repoPath,
  ];
  if (opts.model) {
    codexArgs.push("--model", opts.model);
  }
  codexArgs.push("-");
  const args = process.platform === "win32" ? ["/d", "/s", "/c", "codex.cmd", ...codexArgs] : codexArgs;
  return { command, args, stdin: prompt };
}

function composePrompt(repoName, repoConfig, armName) {
  return `You are running a controlled CodeStory benchmark.

Repository: ${repoName}
Task: ${repoConfig.prompt}

Arm: ${armName}
Instruction: ${ARMS[armName]}

Return a concise answer with the files, symbols, and commands that support your explanation.
Do not edit source files. Use read-only inspection commands only, except CodeStory may write its cache if needed.`;
}

function parseJsonLines(stdout) {
  const parsed = [];
  const malformed = [];
  for (const line of stdout.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }
    try {
      parsed.push(JSON.parse(trimmed));
    } catch {
      malformed.push(trimmed);
    }
  }
  return { parsed, malformed };
}

function isToolType(text) {
  const lower = String(text ?? "").toLowerCase();
  return (
    lower.includes("command_execution") ||
    lower.includes("mcp_tool_call") ||
    lower.includes("tool_call") ||
    lower.includes("function_call") ||
    lower.includes("tool_use") ||
    lower.includes("web_search") ||
    lower.includes("exec_command")
  );
}

function isToolCallStartEvent(event) {
  if (!event || typeof event !== "object") {
    return false;
  }

  const eventType = String(event.type ?? event.event ?? "").toLowerCase();
  const item = event.item && typeof event.item === "object" ? event.item : {};
  const itemType = String(item.type ?? event.item_type ?? event.kind ?? event.name ?? "").toLowerCase();

  if (eventType === "item.started" || eventType.endsWith(".started")) {
    return isToolType(itemType) || isToolType(eventType);
  }

  if (eventType.includes("started") && isToolType(eventType)) {
    return true;
  }

  return false;
}

function normalizeTokenKey(key) {
  const lower = key.toLowerCase();
  if (lower === "prompt_tokens") {
    return "input_tokens";
  }
  if (lower === "completion_tokens") {
    return "output_tokens";
  }
  if (
    lower === "input_tokens" ||
    lower === "output_tokens" ||
    lower === "total_tokens" ||
    lower === "cached_input_tokens" ||
    lower === "reasoning_tokens"
  ) {
    return lower;
  }
  return null;
}

function mergeUsage(value, usage) {
  if (!value || typeof value !== "object") {
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      mergeUsage(item, usage);
    }
    return;
  }

  for (const [key, raw] of Object.entries(value)) {
    const normalized = normalizeTokenKey(key);
    if (normalized && Number.isFinite(Number(raw))) {
      usage[normalized] = Math.max(usage[normalized] ?? 0, Number(raw));
    }
    if (raw && typeof raw === "object") {
      mergeUsage(raw, usage);
    }
  }
}

function extractUsage(events) {
  const usage = {};
  for (const event of events) {
    mergeUsage(event, usage);
  }
  if (usage.total_tokens == null) {
    const input = usage.input_tokens ?? 0;
    const output = usage.output_tokens ?? 0;
    if (input || output) {
      usage.total_tokens = input + output;
    }
  }
  return {
    input_tokens: usage.input_tokens ?? null,
    output_tokens: usage.output_tokens ?? null,
    total_tokens: usage.total_tokens ?? null,
    cached_input_tokens: usage.cached_input_tokens ?? null,
    reasoning_tokens: usage.reasoning_tokens ?? null,
  };
}

function estimateCost(usage) {
  const inputCost = Number.parseFloat(process.env.CODESTORY_BENCH_INPUT_COST_PER_MTOK ?? "");
  const outputCost = Number.parseFloat(process.env.CODESTORY_BENCH_OUTPUT_COST_PER_MTOK ?? "");
  if (
    !Number.isFinite(inputCost) ||
    !Number.isFinite(outputCost) ||
    usage.input_tokens == null ||
    usage.output_tokens == null
  ) {
    return null;
  }
  return (usage.input_tokens / 1_000_000) * inputCost + (usage.output_tokens / 1_000_000) * outputCost;
}

async function runOne(opts, run, outDir) {
  const repoConfig = ALL_REPOS[run.repo];
  const prompt = composePrompt(run.repo, repoConfig, run.arm);
  const { command, args, stdin } = runnerCommand(opts, repoConfig.path, prompt);
  const started = performance.now();

  const result = await new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd: repoConfig.path,
      env: process.env,
      shell: false,
      stdio: [stdin == null ? "ignore" : "pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let settled = false;
    let forceKillTimer = null;
    const timeoutTimer = setTimeout(() => {
      timedOut = true;
      stderr += `\nBenchmark runner timed out after ${opts.timeoutMs}ms.\n`;
      child.kill("SIGTERM");
      forceKillTimer = setTimeout(() => child.kill("SIGKILL"), 5000);
    }, opts.timeoutMs);

    function finish(payload) {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeoutTimer);
      if (forceKillTimer) {
        clearTimeout(forceKillTimer);
      }
      resolve({ timedOut, ...payload });
    }

    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    if (stdin != null) {
      child.stdin.end(stdin);
    }
    child.on("error", (error) => {
      finish({ exitCode: null, signal: null, error: error.message, stdout, stderr });
    });
    child.on("close", (exitCode, signal) => {
      finish({ exitCode, signal, error: null, stdout, stderr });
    });
  });

  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  const runId = `${run.repo}-${run.arm}-${String(run.repeat).padStart(2, "0")}`;
  const stdoutPath = path.join(outDir, `${runId}.stdout.jsonl`);
  const stderrPath = path.join(outDir, `${runId}.stderr.txt`);
  await writeFile(stdoutPath, result.stdout, "utf8");
  await writeFile(stderrPath, result.stderr, "utf8");

  const { parsed, malformed } = parseJsonLines(result.stdout);
  const usage = extractUsage(parsed);
  const toolCalls = parsed.filter(isToolCallStartEvent).length;
  const eventTypes = {};
  for (const event of parsed) {
    const type = String(event.type ?? event.event ?? "unknown");
    eventTypes[type] = (eventTypes[type] ?? 0) + 1;
  }

  return {
    repo: run.repo,
    arm: run.arm,
    repeat: run.repeat,
    runner: opts.runner,
    model: opts.model,
    sandbox: opts.sandbox,
    command,
    args,
    stdin: stdin == null ? null : "<prompt>",
    repo_path: repoConfig.path,
    status: result.timedOut ? "timeout" : result.exitCode === 0 ? "pass" : "fail",
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error,
    wall_ms: wallMs,
    usage,
    estimated_cost_usd: estimateCost(usage),
    tool_calls_observed: toolCalls,
    event_types: eventTypes,
    json_events: parsed.length,
    malformed_stdout_lines: malformed.length,
    stdout_path: stdoutPath,
    stderr_path: stderrPath,
  };
}

function median(values) {
  const sorted = values.filter((value) => value != null).sort((a, b) => a - b);
  if (!sorted.length) {
    return null;
  }
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function summarizeRuns(results) {
  const groups = new Map();
  for (const result of results) {
    const key = `${result.repo}\t${result.arm}`;
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(result);
  }

  const summaries = [];
  for (const [key, rows] of groups) {
    const [repo, arm] = key.split("\t");
    const successful = rows.filter((row) => row.status === "pass");
    summaries.push({
      repo,
      arm,
      runs: rows.length,
      successful_runs: successful.length,
      median_wall_ms: median(successful.map((row) => row.wall_ms)),
      median_total_tokens: median(successful.map((row) => row.usage.total_tokens)),
      median_input_tokens: median(successful.map((row) => row.usage.input_tokens)),
      median_output_tokens: median(successful.map((row) => row.usage.output_tokens)),
      median_estimated_cost_usd: median(successful.map((row) => row.estimated_cost_usd)),
      median_tool_calls_observed: median(successful.map((row) => row.tool_calls_observed)),
    });
  }
  return summaries;
}

function markdownSummary(summary, opts) {
  const lines = [
    "# CodeStory Agent A/B Benchmark",
    "",
    `Runner: \`${opts.runner}\``,
    opts.model ? `Model: \`${opts.model}\`` : "Model: runner default",
    `Sandbox: \`${opts.sandbox}\``,
    `Host: \`${os.hostname()}\``,
    "",
    "| Repo | Arm | Runs | Success | Median wall ms | Median tokens | Median cost USD | Median tool calls |",
    "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const row of summary) {
    lines.push(
      `| ${row.repo} | ${row.arm} | ${row.runs} | ${row.successful_runs} | ${formatValue(row.median_wall_ms)} | ${formatValue(row.median_total_tokens)} | ${formatValue(row.median_estimated_cost_usd)} | ${formatValue(row.median_tool_calls_observed)} |`,
    );
  }
  lines.push(
    "",
    "Raw stdout/stderr files and the JSONL run ledger in this directory are the source of truth.",
    "Do not promote token or cost claims when token usage is blank.",
    "",
  );
  return lines.join("\n");
}

function formatValue(value) {
  if (value == null) {
    return "";
  }
  if (Number.isInteger(value)) {
    return String(value);
  }
  return String(Math.round(value * 1000) / 1000);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.list) {
    for (const [name, config] of Object.entries(ALL_REPOS)) {
      const availability = existsSync(config.path) ? "available" : "missing";
      const scope = PUBLIC_REPOS[name] ? "public" : "local";
      console.log(`${name}\t${scope}\t${availability}\t${config.path}\t${config.prompt}`);
    }
    return;
  }

  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const outDir = path.resolve(opts.outDir ?? path.join(repoRoot, "target", "agent-benchmark", timestamp));
  await mkdir(outDir, { recursive: true });

  const plannedRuns = [];
  for (const repo of opts.repos) {
    for (const arm of Object.keys(ARMS)) {
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        plannedRuns.push({ repo, arm, repeat });
      }
    }
  }

  const results = [];
  for (const run of plannedRuns) {
    console.log(`running ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats}`);
    const result = await runOne(opts, run, outDir);
    results.push(result);
    await writeFile(path.join(outDir, "runs.jsonl"), `${results.map((row) => JSON.stringify(row)).join("\n")}\n`, "utf8");
  }

  const summary = summarizeRuns(results);
  const summaryPayload = {
    generated_at: new Date().toISOString(),
    runner: opts.runner,
    model: opts.model,
    repos: opts.repos,
    repeats: opts.repeats,
    publishable: opts.publishable,
    allow_failures: opts.allowFailures,
    timeout_ms: opts.timeoutMs,
    sandbox: opts.sandbox,
    output_dir: outDir,
    summary,
  };
  await writeFile(path.join(outDir, "summary.json"), `${JSON.stringify(summaryPayload, null, 2)}\n`, "utf8");
  await writeFile(path.join(outDir, "summary.md"), markdownSummary(summary, opts), "utf8");

  const failedRuns = results.filter((result) => result.status !== "pass");
  let exitCode = 0;
  if (failedRuns.length && !opts.allowFailures) {
    console.error("benchmark failed: every run must pass unless --allow-failures is set.");
    for (const failed of failedRuns) {
      console.error(`  ${failed.repo} ${failed.arm} repeat ${failed.repeat}: status=${failed.status} exit=${failed.exit_code} signal=${failed.signal ?? ""}`);
    }
    exitCode = 1;
  }

  if (opts.publishable) {
    const blockers = results.filter((result) => result.status !== "pass" || result.usage.total_tokens == null);
    if (blockers.length) {
      console.error("--publishable failed: every run must pass and report total token usage.");
      for (const blocker of blockers) {
        console.error(`  ${blocker.repo} ${blocker.arm} repeat ${blocker.repeat}: status=${blocker.status} total_tokens=${blocker.usage.total_tokens}`);
      }
      exitCode = 1;
    }
  }

  console.log(`wrote ${outDir}`);
  if (exitCode) {
    process.exit(exitCode);
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
