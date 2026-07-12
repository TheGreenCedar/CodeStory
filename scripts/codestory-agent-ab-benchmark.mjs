#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { copyFile, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { parseArgs as parseNodeArgs } from "node:util";

import {
  buildPacketQualityDeltas,
  discoverPreviousPacketSummary,
} from "./codestory-agent-value-score.mjs";
import {
  benchmarkContractCompatibility,
  benchmarkChildEnv,
  benchmarkRunContract,
  retrievalContractSummary,
  retrievalEnv as benchmarkRetrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedSidecarContractRequests,
} from "./codestory-benchmark-contract.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const benchmarkHarnessPath = fileURLToPath(import.meta.url);
const benchmarkScorerPath = path.join(scriptDir, "codestory-agent-value-score.mjs");
const repoRoot = path.resolve(scriptDir, "..");
const siblingRoot = path.resolve(repoRoot, "..");
const defaultTaskRoot = path.join(repoRoot, "benchmarks", "tasks");
const defaultRepoCacheRoot = path.join(repoRoot, "target", "agent-benchmark", "repos");
const MANIFEST_REPO_NAME_PATTERN = /^[A-Za-z0-9_.-]+$/;
const MANIFEST_TASK_ID_PATTERN = /^[a-z0-9][a-z0-9.-]*$/;
const MAX_PACKET_MANIFEST_EXTRA_PROBES = 12;
const MAX_REUSED_ARTIFACT_BYTES = 64 * 1024 * 1024;
const REUSABLE_BASELINE_ARTIFACT_NAME_PATTERN =
  /(?:\.stdout\.jsonl|\.stderr\.txt|\.baseline-context\.json|\.baseline-context\.stderr\.txt)$/;
const PACKET_TASK_CLASSES = new Set([
  "architecture_explanation",
  "bug_localization",
  "change_impact",
  "route_tracing",
  "symbol_ownership",
  "data_flow",
  "edit_planning",
]);
const COMMAND_ACCOUNTING_CATEGORIES = [
  "codestory_cli",
  "shell_search",
  "direct_file_read",
  "git",
  "build_test",
  "other",
];
const TOOL_ACCOUNTING_CATEGORIES = [
  "web_search",
  "mcp_tool_call",
  "command_execution",
  "function_call",
  "tool_call",
  "other",
];
const PACKET_RUNTIME_DELTA_FIELDS = [
  "packet_sla_missed_runs",
  "median_e2e_wall_ms",
  "median_trace_sla_retrieval_ms",
  "median_trace_accounted_ms",
  "median_packet_unaccounted_ms",
  "median_warm_first_hit_wall_ms",
  "median_warm_cache_hit_wall_ms",
  "median_packet_batch_overhead_ms",
];

const PUBLIC_REPOS = {
  codestory: {
    path: repoRoot,
    checkout_path: repoRoot,
    url: "https://github.com/albertocubeddu/codestory.git",
    ref: "local",
    languages: ["Rust", "JavaScript"],
    prompt:
      "Explain how full indexing flows through CLI, runtime, workspace, indexer, and store, and how that supports search, trail, and snippet.",
  },
};

const LOCAL_REPOS = {
  sourcetrail: {
    path: path.join(siblingRoot, "Sourcetrail"),
    url: "https://github.com/CoatiSoftware/Sourcetrail.git",
    ref: "4b1b0e4fd19c4af235fef12b0564c05348f5f6d3",
    languages: ["C++", "Java"],
    prompt:
      "Explain how project/source-group configuration becomes indexing work, then how indexed data is accessed by the application.",
  },
  codex: {
    path: path.join(siblingRoot, "codex"),
    url: "https://github.com/openai/codex.git",
    ref: "9f42c89c0112771dc29100a6f3fc904049b2655f",
    languages: ["Rust", "TypeScript"],
    prompt:
      "Explain how `codex exec` flows from the top-level CLI into the exec runtime, app-server turn start, and JSONL event output.",
  },
  vscode: {
    path: path.join(siblingRoot, "vscode"),
    url: "https://github.com/microsoft/vscode.git",
    ref: "local",
    languages: ["TypeScript"],
    prompt:
      "Explain how VS Code workbench startup reaches extension host activation and command execution.",
  },
  rootandruntime: {
    path: path.join(siblingRoot, "rootandruntime"),
    prompt:
      "Explain how public writing and social surfaces connect to Payload collections, comment auth, and the elsewhere feed.",
  },
};

const ALL_REPOS = { ...PUBLIC_REPOS, ...LOCAL_REPOS };

const ARMS = {
  without_codestory:
    "Do not use CodeStory, codestory-cli, or codestory-grounding. Use normal local repository exploration only. Do not use web search, browser tools, remote URLs, or upstream mirrors.",
  with_codestory:
    "Use CodeStory grounding first. CODESTORY_CLI is set to the executable for this run. For broad repository questions, run packet first and read its sufficiency contract before ordinary source reads. Read follow-up commands from sufficiency.follow_up_commands, not a top-level field. If sufficiency.status is partial, run the listed follow_up_commands in order and prefer targeted CodeStory `search --why`, `context`, `trail`, or `snippet` commands for named gaps. If the packet and CodeStory follow-ups still do not support a correct answer, use ordinary local source reads only after those CodeStory attempts; those reads are valid but counted as post-packet overhead. If a later packet becomes sufficient, stop exploration and answer. If packet status is sufficient and sufficiency.follow_up_commands is empty, answer from the packet; do not verify citations with ordinary source reads, rg, grep, or git show. Budget truncation alone is not a gap. Preserve the packet's supported-claim wording in your final answer when it is correct, and correct it from local source when the packet is incomplete. Copy exact source identifiers, table names, declarations, and claim phrases from packet citations and sufficiency.covered_claims; do not compress exact anchors into comma shorthand that drops their repeated prefix, such as rewriting `CREATE TABLE A` and `CREATE TABLE B` as `CREATE TABLE A and B`. Include a compact 'Support files' list containing every relevant path from the packet's answer.citations, sufficiency.avoid_opening_paths, and any post-packet local source reads. The prepared full sidecar cache is mandatory; if CodeStory or its sidecars are unavailable, fail the run instead of continuing with ordinary exploration. Do not use web search, browser tools, remote URLs, or upstream mirrors.",
};

function usage() {
  console.log(`Usage:
  node scripts/codestory-agent-ab-benchmark.mjs --list
  node scripts/codestory-agent-ab-benchmark.mjs --self-test
  node scripts/codestory-agent-ab-benchmark.mjs --reanalyze-dir target/agent-benchmark/<run-dir>
  node scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite <suite> [--materialize-repos] [--repeats n]
  node scripts/codestory-agent-ab-benchmark.mjs [--quick] [--repos names] [--arms names] [--task-suite name] [--task-ids ids] [--task-manifest path] [--include-local-repos] [--repeats n] [--runner codex] [--model model] [--sandbox mode] [--out-dir path] [--timeout-ms ms] [--prepare-codestory-cache] [--allow-failures] [--publishable]

Options:
  --list          Print configured benchmark repositories or selected manifest tasks and exit.
  --self-test     Run transcript analyzer and quality-scoring fixture checks.
  --reanalyze-dir Recompute transcript analysis, quality scores, and summaries from an existing run directory.
  --quick         Default to repo=codestory and repeats=1 unless explicitly set.
  --repos         Comma-separated repo names. Public: ${Object.keys(PUBLIC_REPOS).join(", ")}. Local optional: ${Object.keys(LOCAL_REPOS).join(", ")}
  --arms          Comma-separated A/B arms. Default: ${Object.keys(ARMS).join(", ")}.
  --task-suite    Task suite folder under benchmarks/tasks, such as public-core or holdout-retrieval.
  --task-ids      Comma-separated manifest task ids to include after suite/path loading.
  --task-manifest Task manifest JSON file or directory. When set, tasks drive repos and prompts.
  --materialize-repos
                  Clone/fetch manifest public repos into --repo-cache-dir before listing or running.
  --repo-cache-dir
                  Directory for materialized public repos. Default: target/agent-benchmark/repos.
  --packet-runtime
                  Run direct packet runtime benchmark rows instead of agent A/B arms.
  --packet-runtime-mode
                  cold-cli, warm-stdio, or both. Default: both.
  --codestory-cli Path to codestory-cli for packet runtime mode. Default: CODESTORY_CLI, then release binary.
  --benchmark-run-id
                  Coherent benchmark run id to stamp packet-runtime artifacts.
  --include-local-repos
                  Include local sibling repos in the default non-quick run.
  --repeats       Repeats per repo/arm. Default: 3, or 1 with --quick.
  --runner        Runner command family. Default: codex.
  --model         Optional model passed to codex exec.
  --sandbox       Codex sandbox mode. Default: workspace-write.
  --out-dir       Output directory. Default: target/agent-benchmark/<timestamp>.
  --timeout-ms    Timeout per runner invocation. Default: 600000.
  --jobs          Parallel jobs for independent packet-runtime cold-cli rows or independent agent repo groups. Default: 1.
  --reuse-baseline-from
                  Reuse matching without-CodeStory rows from an earlier run directory when the task snapshot is unchanged.
  --prepare-codestory-cache
                  Before timed with-CodeStory runs, refresh stale or semantic-empty local caches and record indexing cost separately.
                  Packet-runtime mode enables this by default because sidecar-primary packets require prepared local indexes.
  --no-prepare-codestory-cache
                  Unsupported; sidecar preparation is mandatory.
  --prepare-codestory-jobs
                  Parallel jobs for CodeStory cache preparation across independent repos. Default: 1.
  --prepare-codestory-timeout-ms
                  Timeout for each pre-run CodeStory index refresh. Default: 1800000.
  --max-source-reads-after-packet
                  Publishable with-CodeStory runs fail above this post-packet ordinary source-read count.
                  Required with --publishable; pass 0 for packet-only promotion evidence.
  --diagnostic-extra-probes-from-manifest
                  Inject expected file/symbol anchors as packet --extra-probe values.
                  Diagnostic only; cannot be combined with --publishable.
  --allow-failures Exit 0 even when a run fails. Intended only for exploratory dry runs.
  --publishable   Fail unless every run succeeds and reports token usage.

Environment (parity / promotion — see docs/testing/retrieval-architecture.md):
  CODESTORY_RETRIEVAL unset|1 Sidecar-primary packet retrieval (benchmark default)
  CODESTORY_RETRIEVAL=0       Unsupported; sidecar retrieval is mandatory
  CODESTORY_EVAL_PROBES=1        Explicit diagnostic only; product benchmark runs do not inject it
`);
}

function commaSeparatedList(value) {
  return value?.split(",").map((name) => name.trim()).filter(Boolean);
}

function parseArgs(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      help: { type: "boolean", short: "h" },
      list: { type: "boolean" },
      "self-test": { type: "boolean" },
      "reanalyze-dir": { type: "string" },
      quick: { type: "boolean" },
      publishable: { type: "boolean" },
      "allow-failures": { type: "boolean" },
      "diagnostic-extra-probes-from-manifest": { type: "boolean" },
      "include-local-repos": { type: "boolean" },
      "materialize-repos": { type: "boolean" },
      "packet-runtime": { type: "boolean" },
      "packet-runtime-mode": { type: "string" },
      "repo-cache-dir": { type: "string" },
      "codestory-cli": { type: "string" },
      repos: { type: "string" },
      arms: { type: "string" },
      "task-suite": { type: "string" },
      "task-ids": { type: "string" },
      "task-manifest": { type: "string" },
      repeats: { type: "string" },
      runner: { type: "string" },
      model: { type: "string" },
      sandbox: { type: "string" },
      "out-dir": { type: "string" },
      "benchmark-run-id": { type: "string" },
      "timeout-ms": { type: "string" },
      jobs: { type: "string" },
      "reuse-baseline-from": { type: "string" },
      "prepare-codestory-cache": { type: "boolean" },
      "no-prepare-codestory-cache": { type: "boolean" },
      "prepare-codestory-timeout-ms": { type: "string" },
      "prepare-codestory-jobs": { type: "string" },
      "max-source-reads-after-packet": { type: "string" },
    },
  });
  const opts = {
    list: false,
    selfTest: false,
    reanalyzeDir: null,
    quick: false,
    repos: null,
    arms: null,
    taskSuite: null,
    taskIds: null,
    taskManifest: null,
    materializeRepos: false,
    repoCacheDir: defaultRepoCacheRoot,
    packetRuntime: false,
    packetRuntimeMode: "both",
    codestoryCli: process.env.CODESTORY_CLI || null,
    benchmarkRunId: null,
    includeLocalRepos: false,
    repeats: null,
    runner: "codex",
    model: null,
    sandbox: "workspace-write",
    outDir: null,
    timeoutMs: 600000,
    jobs: 1,
    reuseBaselineFrom: null,
    prepareCodestoryCache: null,
    prepareCodestoryJobs: 1,
    prepareCodestoryTimeoutMs: 1_800_000,
    cachePreparationByRepo: null,
    maxSourceReadsAfterPacket: null,
    diagnosticExtraProbesFromManifest: false,
    allowFailures: false,
    publishable: false,
  };

  if (values.help) {
    usage();
    process.exit(0);
  }
  if (values["no-prepare-codestory-cache"]) {
    throw new Error("--no-prepare-codestory-cache is unsupported; sidecar preparation is mandatory");
  }
  opts.list = values.list === true;
  opts.selfTest = values["self-test"] === true;
  opts.reanalyzeDir = values["reanalyze-dir"] ?? null;
  opts.quick = values.quick === true;
  opts.publishable = values.publishable === true;
  opts.allowFailures = values["allow-failures"] === true;
  opts.diagnosticExtraProbesFromManifest = values["diagnostic-extra-probes-from-manifest"] === true;
  opts.includeLocalRepos = values["include-local-repos"] === true;
  opts.materializeRepos = values["materialize-repos"] === true;
  opts.packetRuntime = values["packet-runtime"] === true;
  opts.packetRuntimeMode = values["packet-runtime-mode"] ?? opts.packetRuntimeMode;
  opts.repoCacheDir = values["repo-cache-dir"] ?? opts.repoCacheDir;
  opts.codestoryCli = values["codestory-cli"] ?? opts.codestoryCli;
  opts.repos = values.repos ? commaSeparatedList(values.repos) : null;
  opts.arms = values.arms ? commaSeparatedList(values.arms) : null;
  opts.taskSuite = values["task-suite"] ?? null;
  opts.taskIds = values["task-ids"] ? commaSeparatedList(values["task-ids"]) : null;
  opts.taskManifest = values["task-manifest"] ?? null;
  opts.repeats = values.repeats == null ? null : Number.parseInt(values.repeats, 10);
  opts.runner = values.runner ?? opts.runner;
  opts.model = values.model ?? null;
  opts.sandbox = values.sandbox ?? opts.sandbox;
  opts.outDir = values["out-dir"] ?? null;
  opts.benchmarkRunId = values["benchmark-run-id"] ?? null;
  opts.timeoutMs = values["timeout-ms"] == null ? opts.timeoutMs : Number.parseInt(values["timeout-ms"], 10);
  opts.jobs = values.jobs == null ? opts.jobs : Number.parseInt(values.jobs, 10);
  opts.reuseBaselineFrom = values["reuse-baseline-from"] ?? null;
  opts.prepareCodestoryCache = values["prepare-codestory-cache"] === true ? true : null;
  opts.prepareCodestoryTimeoutMs =
    values["prepare-codestory-timeout-ms"] == null
      ? opts.prepareCodestoryTimeoutMs
      : Number.parseInt(values["prepare-codestory-timeout-ms"], 10);
  opts.prepareCodestoryJobs =
    values["prepare-codestory-jobs"] == null
      ? opts.prepareCodestoryJobs
      : Number.parseInt(values["prepare-codestory-jobs"], 10);
  opts.maxSourceReadsAfterPacket =
    values["max-source-reads-after-packet"] == null
      ? null
      : Number.parseInt(values["max-source-reads-after-packet"], 10);

  if (opts.taskSuite && opts.taskManifest) {
    throw new Error("--task-suite and --task-manifest are mutually exclusive");
  }

  if (!opts.reanalyzeDir && !opts.repos && !opts.taskSuite && !opts.taskManifest) {
    opts.repos = opts.quick
      ? ["codestory"]
      : [
          ...Object.keys(PUBLIC_REPOS),
          ...(opts.includeLocalRepos ? Object.keys(LOCAL_REPOS) : []),
        ];
  }
  opts.arms ??= Object.keys(ARMS);
  if (!opts.arms.length) {
    throw new Error("--arms must include at least one arm");
  }
  for (const arm of opts.arms) {
    if (!ARMS[arm]) {
      throw new Error(`Unknown arm '${arm}'. Known: ${Object.keys(ARMS).join(", ")}`);
    }
  }
  if (!opts.repeats) {
    opts.repeats = opts.quick ? 1 : 3;
  }
  if (opts.prepareCodestoryCache == null) {
    opts.prepareCodestoryCache = opts.packetRuntime || opts.arms.includes("with_codestory");
  }
  if (!Number.isInteger(opts.repeats) || opts.repeats < 1) {
    throw new Error("--repeats must be a positive integer");
  }
  if (!Number.isInteger(opts.timeoutMs) || opts.timeoutMs < 1000) {
    throw new Error("--timeout-ms must be an integer >= 1000");
  }
  if (!Number.isInteger(opts.jobs) || opts.jobs < 1) {
    throw new Error("--jobs must be a positive integer");
  }
  if (!Number.isInteger(opts.prepareCodestoryTimeoutMs) || opts.prepareCodestoryTimeoutMs < 1000) {
    throw new Error("--prepare-codestory-timeout-ms must be an integer >= 1000");
  }
  if (!Number.isInteger(opts.prepareCodestoryJobs) || opts.prepareCodestoryJobs < 1) {
    throw new Error("--prepare-codestory-jobs must be a positive integer");
  }
  if (!["read-only", "workspace-write", "danger-full-access"].includes(opts.sandbox)) {
    throw new Error("--sandbox must be one of: read-only, workspace-write, danger-full-access");
  }
  if (!["cold-cli", "warm-stdio", "both"].includes(opts.packetRuntimeMode)) {
    throw new Error("--packet-runtime-mode must be one of: cold-cli, warm-stdio, both");
  }
  if (opts.benchmarkRunId != null) {
    opts.benchmarkRunId = sanitizeBenchmarkRunId(opts.benchmarkRunId);
  }
  if (
    opts.maxSourceReadsAfterPacket != null &&
    (!Number.isInteger(opts.maxSourceReadsAfterPacket) || opts.maxSourceReadsAfterPacket < 0)
  ) {
    throw new Error("--max-source-reads-after-packet must be a non-negative integer");
  }
  if (opts.publishable && opts.diagnosticExtraProbesFromManifest) {
    throw new Error("--diagnostic-extra-probes-from-manifest is diagnostic-only and cannot be combined with --publishable");
  }
  opts.repoCacheDir = path.resolve(opts.repoCacheDir ?? defaultRepoCacheRoot);
  if (opts.reuseBaselineFrom) {
    opts.reuseBaselineFrom = path.resolve(opts.reuseBaselineFrom);
  }
  if (opts.repos) {
    for (const name of opts.repos) {
      if (!ALL_REPOS[name]) {
        throw new Error(`Unknown repo '${name}'. Known: ${Object.keys(ALL_REPOS).join(", ")}`);
      }
    }
  }
  return opts;
}

function sanitizeBenchmarkRunId(value) {
  const cleaned = String(value ?? "")
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!cleaned) {
    throw new Error("--benchmark-run-id must contain at least one filesystem-safe character");
  }
  return cleaned;
}

function retrievalEnv() {
  return benchmarkRetrievalEnv(benchmarkChildEnv(process.env));
}

function runnerCommand(opts, repoPath, prompt) {
  if (opts.runner !== "codex") {
    return {
      command: opts.runner,
      args: [prompt],
      stdin: null,
      killProcessTree: false,
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
  if (process.platform === "win32") {
    assertSafeWindowsCmdArgs(codexArgs);
  }
  const args = process.platform === "win32" ? ["/d", "/s", "/c", "codex.cmd", ...codexArgs] : codexArgs;
  return { command, args, stdin: prompt, killProcessTree: process.platform === "win32" };
}

function assertSafeWindowsCmdArgs(args) {
  for (const arg of args) {
    const value = String(arg ?? "");
    if (/[;&|<>^%\r\n]/.test(value)) {
      throw new Error(`Refusing to pass unsafe Windows cmd.exe argument to Codex runner: ${value}`);
    }
  }
}

function taskIdFromManifest(filePath, raw) {
  return String(raw.id ?? raw.name ?? path.basename(filePath, path.extname(filePath)))
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function validateManifestRepoName(filePath, value) {
  const name = String(value ?? "").trim();
  if (!name) {
    throw new Error(`Task manifest is missing repo.name: ${filePath}`);
  }
  if (!MANIFEST_REPO_NAME_PATTERN.test(name) || name === "." || name === "..") {
    throw new Error(
      `Task manifest repo.name must match ${MANIFEST_REPO_NAME_PATTERN} and cannot be '.' or '..': ${filePath}`,
    );
  }
  return name;
}

function validateManifestTaskId(filePath, value) {
  const id = String(value ?? "").trim();
  if (!MANIFEST_TASK_ID_PATTERN.test(id)) {
    throw new Error(`Task manifest id must match ${MANIFEST_TASK_ID_PATTERN}: ${filePath}`);
  }
  return id;
}

function validatePacketTaskClass(filePath, value) {
  if (value == null) {
    return null;
  }
  const taskClass = String(value).trim();
  if (!PACKET_TASK_CLASSES.has(taskClass)) {
    throw new Error(
      `Task manifest task_class must be one of ${[...PACKET_TASK_CLASSES].join(", ")}: ${filePath}`,
    );
  }
  return taskClass;
}

function isPathInside(base, candidate) {
  const relative = path.relative(path.resolve(base), path.resolve(candidate));
  return relative === "" || (relative && !relative.startsWith("..") && !path.isAbsolute(relative));
}

function assertPathInside(base, candidate, label) {
  if (!isPathInside(base, candidate)) {
    throw new Error(`${label} must stay inside ${path.resolve(base)}: ${path.resolve(candidate)}`);
  }
  return path.resolve(candidate);
}

function normalizeWorkspaceRoot(filePath, value) {
  if (value == null || String(value).trim() === "" || String(value).trim() === ".") {
    return "";
  }
  const raw = String(value).trim().replace(/^['"]|['"]$/g, "");
  if (
    path.isAbsolute(raw) ||
    path.win32.isAbsolute(raw) ||
    path.posix.isAbsolute(raw) ||
    /^[A-Za-z]:/.test(raw)
  ) {
    throw new Error(`Task manifest workspace_root must be relative: ${filePath}`);
  }
  const normalized = normalizePathLike(raw);
  const parts = normalized.split("/");
  if (
    !normalized ||
    normalized === "." ||
    normalized === ".." ||
    parts.some((part) => part === ".." || part === "")
  ) {
    throw new Error(`Task manifest workspace_root cannot traverse outside the checkout: ${filePath}`);
  }
  return normalized;
}

function repoConfigFromManifest(repo, opts = {}) {
  if (!repo || typeof repo !== "object") {
    return null;
  }
  const filePath = opts.filePath ?? "task manifest";
  const name = validateManifestRepoName(filePath, repo.name);
  const checkoutPath = path.resolve(opts.repoCacheDir ?? defaultRepoCacheRoot, name);
  assertPathInside(opts.repoCacheDir ?? defaultRepoCacheRoot, checkoutPath, "Manifest repo checkout path");
  const workspaceRoot = normalizeWorkspaceRoot(filePath, repo.workspace_root);
  const workspacePath = workspaceRoot ? path.join(checkoutPath, workspaceRoot) : checkoutPath;
  assertPathInside(checkoutPath, workspacePath, "Manifest repo workspace_root");
  return {
    name,
    path: workspacePath,
    checkout_path: checkoutPath,
    workspace_root: workspaceRoot || null,
    url: repo.url ?? null,
    ref: repo.ref ?? null,
    languages: Array.isArray(repo.languages) ? repo.languages : [],
    setup: Array.isArray(repo.setup) ? repo.setup : [],
    prompt: "",
  };
}

function registerManifestRepo(repo, opts = {}) {
  const config = repoConfigFromManifest(repo, opts);
  if (!config) {
    return;
  }
  const name = config.name;
  const existing = ALL_REPOS[name];
  const preferManifestCheckout = Boolean(opts.materializeRepos || opts.publishable);
  const manifestOverriddenByBuiltIn = Boolean(
    existing &&
      !preferManifestCheckout &&
      (
        path.resolve(existing.path ?? "") !== path.resolve(config.path) ||
        path.resolve(existing.checkout_path ?? existing.path ?? "") !== path.resolve(config.checkout_path) ||
        (existing.ref ?? null) !== (config.ref ?? null)
      ),
  );
  const activeConfig = preferManifestCheckout
    ? { ...config, prompt: existing?.prompt ?? config.prompt }
    : { ...config, ...existing };
  ALL_REPOS[name] = {
    ...activeConfig,
    manifest_url: config.url,
    manifest_ref: config.ref,
    manifest_workspace_root: config.workspace_root,
    manifest_checkout_path: config.checkout_path,
    manifest_overridden_by_builtin: manifestOverriddenByBuiltIn,
    languages: activeConfig.languages?.length ? activeConfig.languages : config.languages,
    setup: activeConfig.setup?.length ? activeConfig.setup : config.setup,
  };
  if (!LOCAL_REPOS[name]) {
    PUBLIC_REPOS[name] = ALL_REPOS[name];
  }
}

function textAnchor(value) {
  if (value == null) {
    return null;
  }
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "object") {
    return value.text ?? value.name ?? value.path ?? null;
  }
  return String(value);
}

function textAnchorList(values) {
  return (Array.isArray(values) ? values : [])
    .map(textAnchor)
    .map((value) => String(value ?? "").trim())
    .filter(Boolean);
}

function packetManifestSymbolProbe(value) {
  if (value == null) {
    return null;
  }
  if (typeof value === "string") {
    return value;
  }
  if (typeof value === "object") {
    const name = String(value.name ?? value.text ?? "").trim();
    const symbolPath = String(value.path ?? value.file ?? value.file_path ?? "").trim();
    if (name && symbolPath) {
      return `${symbolPath} ${name}`;
    }
    return name || symbolPath || null;
  }
  return String(value);
}

function packetManifestSymbolProbeList(values) {
  return (Array.isArray(values) ? values : [])
    .map(packetManifestSymbolProbe)
    .map((value) => String(value ?? "").trim())
    .filter(Boolean);
}

function uniqueTextValues(values) {
  const result = [];
  const seen = new Set();
  for (const value of values) {
    const text = String(value ?? "").trim();
    if (!text) {
      continue;
    }
    const key = text.toLowerCase();
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    result.push(text);
  }
  return result;
}

function packetManifestExtraProbes(task) {
  if (!task) {
    return [];
  }
  return uniqueTextValues([
    ...(task.expected_files ?? []),
    ...(task.expected_symbol_probes ?? task.expected_symbols ?? []),
  ]).slice(0, MAX_PACKET_MANIFEST_EXTRA_PROBES);
}

function packetCommandExtraProbes(task, opts = {}) {
  return opts.diagnosticExtraProbesFromManifest ? packetManifestExtraProbes(task) : [];
}

function packetExtraProbeStrategy(extraProbes) {
  return extraProbes.length ? "diagnostic_manifest_expected_anchors" : null;
}

function normalizeManifestTask(filePath, raw, opts = {}) {
  const rawRepo = typeof raw.repo === "object" ? raw.repo?.name : raw.repo;
  if (!String(rawRepo ?? "").trim()) {
    throw new Error(`Task manifest is missing repo: ${filePath}`);
  }
  if (typeof raw.repo === "object") {
    registerManifestRepo(raw.repo, { ...opts, filePath });
  }
  const repo = validateManifestRepoName(filePath, rawRepo);
  if (!ALL_REPOS[repo]) {
    throw new Error(`Task manifest ${filePath} references unknown repo '${repo}'`);
  }
  const prompt = String(raw.prompt ?? raw.question ?? "").trim();
  if (!prompt) {
    throw new Error(`Task manifest is missing prompt: ${filePath}`);
  }
  const expectedFiles = textAnchorList(raw.expected_files ?? raw.expectedFiles);
  const expectedVerificationFiles = textAnchorList(
    raw.expected_verification_files ?? raw.expectedVerificationFiles,
  );
  const rawExpectedSymbols = raw.expected_symbols ?? raw.expectedSymbols;
  const expectedSymbols = textAnchorList(rawExpectedSymbols);
  const expectedSymbolProbes = packetManifestSymbolProbeList(rawExpectedSymbols);
  const expectedClaims = textAnchorList(raw.expected_claims ?? raw.expectedClaims);
  const qualityThresholds = raw.quality_thresholds ?? raw.qualityThresholds;
  if (!expectedFiles.length) {
    throw new Error(`Task manifest must include at least one expected file: ${filePath}`);
  }
  if (!expectedSymbols.length) {
    throw new Error(`Task manifest must include at least one expected symbol: ${filePath}`);
  }
  if (!expectedClaims.length) {
    throw new Error(`Task manifest must include at least one expected claim: ${filePath}`);
  }
  validateQualityThresholds(filePath, qualityThresholds);
  const id = validateManifestTaskId(filePath, taskIdFromManifest(filePath, raw));
  const taskClass = validatePacketTaskClass(filePath, raw.task_class ?? raw.taskClass);

  return {
    id,
    name: String(raw.name ?? raw.id ?? path.basename(filePath, path.extname(filePath))),
    suite: raw.suite ?? null,
    repo,
    repo_metadata: typeof raw.repo === "object" ? raw.repo : null,
    task_class: taskClass,
    prompt,
    expected_files: expectedFiles,
    expected_verification_files: expectedVerificationFiles,
    expected_symbols: expectedSymbols,
    expected_symbol_probes: expectedSymbolProbes,
    expected_claims: expectedClaims,
    forbidden_claims: textAnchorList(raw.forbidden_claims ?? raw.forbiddenClaims),
    quality_thresholds: qualityThresholds,
    manifest_path: filePath,
  };
}

function taskSnapshotForResult(task) {
  if (!task) {
    return null;
  }
  return JSON.parse(
    JSON.stringify({
      id: task.id,
      name: task.name,
      suite: task.suite ?? null,
      repo: task.repo,
      repo_metadata: task.repo_metadata ?? null,
      task_class: task.task_class,
      prompt: task.prompt,
      expected_files: task.expected_files ?? [],
      expected_verification_files: task.expected_verification_files ?? [],
      expected_symbols: task.expected_symbols ?? [],
      expected_symbol_probes: task.expected_symbol_probes ?? [],
      expected_claims: task.expected_claims ?? [],
      forbidden_claims: task.forbidden_claims ?? [],
      quality_thresholds: task.quality_thresholds ?? {},
      manifest_path: task.manifest_path ?? null,
    }),
  );
}

function validateQualityThresholds(filePath, thresholds) {
  if (!thresholds || typeof thresholds !== "object" || Array.isArray(thresholds)) {
    throw new Error(`Task manifest must include quality_thresholds: ${filePath}`);
  }
  for (const key of [
    "min_expected_anchor_recall",
    "min_expected_file_recall",
    "min_expected_symbol_recall",
    "min_expected_claim_recall",
    "min_citation_coverage",
  ]) {
    const value = Number(thresholds[key]);
    if (!Number.isFinite(value) || value < 0 || value > 1) {
      throw new Error(`Task manifest quality_thresholds.${key} must be a ratio from 0 to 1: ${filePath}`);
    }
  }
  const maxForbidden = Number(thresholds.max_forbidden_claims);
  if (!Number.isInteger(maxForbidden) || maxForbidden < 0) {
    throw new Error(`Task manifest quality_thresholds.max_forbidden_claims must be a non-negative integer: ${filePath}`);
  }
}

async function loadJsonFile(filePath) {
  const contents = await readFile(filePath, "utf8");
  return JSON.parse(contents);
}

async function listManifestFiles(manifestPath) {
  const resolved = path.resolve(manifestPath);
  const stat = statSync(resolved);
  if (stat.isFile()) {
    return [resolved];
  }
  if (!stat.isDirectory()) {
    throw new Error(`Task manifest path is not a file or directory: ${manifestPath}`);
  }

  const files = [];
  for (const entry of await readdir(resolved, { withFileTypes: true })) {
    const child = path.join(resolved, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await listManifestFiles(child)));
    } else if (entry.isFile() && entry.name.endsWith(".task.json")) {
      files.push(child);
    }
  }
  return files.sort();
}

async function loadTasks(opts) {
  const suitePath = opts.taskSuite ? path.join(defaultTaskRoot, opts.taskSuite) : null;
  const manifestPath = opts.taskSuite && existsSync(suitePath)
    ? suitePath
    : opts.taskManifest ?? (opts.taskSuite ? defaultTaskRoot : null);
  if (!manifestPath) {
    return [];
  }
  if (!existsSync(manifestPath)) {
    throw new Error(`Task manifest path does not exist: ${manifestPath}`);
  }

  const tasks = [];
  for (const filePath of await listManifestFiles(manifestPath)) {
    const raw = await loadJsonFile(filePath);
    const rows = Array.isArray(raw.tasks) ? raw.tasks : Array.isArray(raw) ? raw : [raw];
    for (const row of rows) {
      const task = normalizeManifestTask(filePath, row, opts);
      if (!opts.taskSuite || task.suite === opts.taskSuite || row.suite === opts.taskSuite) {
        tasks.push(task);
      }
    }
  }
  if (!tasks.length) {
    throw new Error(`Task manifest path contained no tasks: ${manifestPath}`);
  }
  if (opts.taskIds?.length) {
    const wanted = new Set(opts.taskIds);
    const filtered = tasks.filter((task) => wanted.has(task.id));
    const found = new Set(filtered.map((task) => task.id));
    const missing = [...wanted].filter((taskId) => !found.has(taskId));
    if (missing.length) {
      throw new Error(`Requested --task-ids were not found: ${missing.join(", ")}`);
    }
    return filtered;
  }
  return tasks;
}

function publicCoreCorpusAudit(tasks) {
  const classCounts = new Map();
  const repos = new Set();
  for (const task of tasks.filter((task) => task.suite === "public-core")) {
    repos.add(task.repo);
    classCounts.set(task.task_class, (classCounts.get(task.task_class) ?? 0) + 1);
  }
  const requiredClasses = [
    "architecture_explanation",
    "bug_localization",
    "change_impact",
    "edit_planning",
    "route_tracing",
    "symbol_ownership",
  ];
  const missingClasses = requiredClasses.filter((taskClass) => !classCounts.has(taskClass));
  const underfilledClasses = requiredClasses.filter((taskClass) => (classCounts.get(taskClass) ?? 0) < 3);
  return {
    repo_count: repos.size,
    class_counts: Object.fromEntries([...classCounts.entries()].sort()),
    missing_classes: missingClasses,
    underfilled_classes: underfilledClasses,
  };
}

function validatePublishableShape(opts, tasks) {
  const blockers = [];
  if (opts.repeats < 3) {
    blockers.push("--publishable requires --repeats >= 3");
  }
  if (opts.taskSuite === "public-core") {
    const audit = publicCoreCorpusAudit(tasks);
    if (audit.repo_count < 5) {
      blockers.push(`public-core needs at least 5 public repos, found ${audit.repo_count}`);
    }
    if (audit.missing_classes.length) {
      blockers.push(`public-core is missing task classes: ${audit.missing_classes.join(", ")}`);
    }
    if (audit.underfilled_classes.length) {
      blockers.push(`public-core needs at least 3 tasks per class; underfilled: ${audit.underfilled_classes.join(", ")}`);
    }
  }
  blockers.push(...unsupportedSidecarContractRequests(process.env));
  if (blockers.length) {
    throw new Error(`Publishable benchmark shape is incomplete:\n- ${blockers.join("\n- ")}`);
  }
}

async function runProcess(command, args, options = {}) {
  return await new Promise((resolve) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      shell: false,
      stdio: options.stdin == null ? ["ignore", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let settled = false;
    let forceKillTimer = null;
    const timeoutTimer = options.timeoutMs
      ? setTimeout(() => {
          timedOut = true;
          const message = options.timeoutMessage ?? `Process timed out after ${options.timeoutMs}ms.`;
          stderr += `\n${message}\n`;
          terminateProcess(child, "SIGTERM", options);
          if (options.forceKillAfterMs) {
            forceKillTimer = setTimeout(
              () => terminateProcess(child, "SIGKILL", options),
              options.forceKillAfterMs,
            );
          }
        }, options.timeoutMs)
      : null;
    function finish(payload) {
      if (settled) {
        return;
      }
      settled = true;
      if (timeoutTimer) {
        clearTimeout(timeoutTimer);
      }
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
    if (options.stdin != null) {
      child.stdin.end(options.stdin);
    }
    child.on("error", (error) => {
      finish({
        status: timedOut ? "timeout" : "error",
        exitCode: null,
        signal: null,
        stdout,
        stderr,
        error: error.message,
      });
    });
    child.on("close", (exitCode, signal) => {
      finish({
        status: timedOut ? "timeout" : exitCode === 0 ? "pass" : "fail",
        exitCode,
        signal,
        stdout,
        stderr,
        error: null,
      });
    });
  });
}

async function parallelMap(items, jobs, mapper) {
  const results = new Array(items.length);
  let nextIndex = 0;
  const workerCount = Math.min(Math.max(1, jobs), items.length);
  await Promise.all(
    Array.from({ length: workerCount }, async () => {
      for (;;) {
        const index = nextIndex;
        nextIndex += 1;
        if (index >= items.length) {
          return;
        }
        results[index] = await mapper(items[index], index);
      }
    }),
  );
  return results;
}

function terminateProcess(child, signal, options = {}) {
  if (options.killProcessTree && process.platform === "win32" && child.pid) {
    const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    });
    killer.on("error", () => {
      child.kill(signal);
    });
    return;
  }
  child.kill(signal);
}

async function runCheckedProcess(command, args, options = {}) {
  const result = await runProcess(command, args, options);
  if (result.status !== "pass") {
    throw new Error(
      `${command} ${args.join(" ")} failed: status=${result.status} exit=${result.exitCode}\n${result.stderr || result.stdout}`,
    );
  }
  return result;
}

function uniqueTaskRepos(tasks) {
  const repos = new Map();
  for (const task of tasks) {
    const config = ALL_REPOS[task.repo];
    if (config?.url && config?.ref && config.ref !== "local") {
      repos.set(task.repo, config);
    }
  }
  return repos;
}

function isTrustedPublishableRepoUrl(url) {
  try {
    const parsed = new URL(String(url ?? ""));
    if (
      parsed.protocol !== "https:" ||
      parsed.hostname.toLowerCase() !== "github.com" ||
      parsed.username ||
      parsed.password ||
      parsed.search ||
      parsed.hash
    ) {
      return false;
    }
    const parts = parsed.pathname.split("/").filter(Boolean);
    return (
      parts.length === 2 &&
      /^[A-Za-z0-9_.-]+$/.test(parts[0]) &&
      /^[A-Za-z0-9_.-]+(?:\.git)?$/.test(parts[1])
    );
  } catch {
    return false;
  }
}

function normalizeTrustedPublishableRepoUrl(url) {
  if (!isTrustedPublishableRepoUrl(url)) {
    return null;
  }
  const parsed = new URL(String(url));
  const [owner, repo] = parsed.pathname.split("/").filter(Boolean);
  return `${owner.toLowerCase()}/${repo.replace(/\.git$/i, "").toLowerCase()}`;
}

function manifestRepoMaterializationBlockers(tasks, opts = {}) {
  if (!opts.publishable || !opts.materializeRepos) {
    return [];
  }
  const blockers = [];
  for (const [name, config] of uniqueTaskRepos(tasks)) {
    if (!isTrustedPublishableRepoUrl(config.url)) {
      blockers.push(`${name}: manifest repo URL is not an https://github.com/<owner>/<repo>[.git] URL`);
    }
    if (!isImmutableCommitRef(config.ref)) {
      blockers.push(`${name}: manifest repo ref is not a full immutable commit SHA`);
    }
  }
  return blockers;
}

function assertManifestRepoMaterializationAllowed(tasks, opts = {}) {
  const blockers = manifestRepoMaterializationBlockers(tasks, opts);
  if (blockers.length) {
    throw new Error(
      `Publishable repo materialization preflight failed before clone/fetch:\n- ${blockers.join("\n- ")}`,
    );
  }
}

async function materializeRepos(tasks, opts) {
  const repos = uniqueTaskRepos(tasks);
  if (!repos.size) {
    return;
  }
  await mkdir(opts.repoCacheDir, { recursive: true });
  for (const [name, config] of repos) {
    const checkoutPath = path.resolve(config.checkout_path ?? path.join(opts.repoCacheDir, name));
    assertPathInside(opts.repoCacheDir, checkoutPath, "Materialized repo checkout path");
    assertPathInside(checkoutPath, config.path, `Materialized repo workspace path for ${name}`);
    if (!existsSync(checkoutPath)) {
      await mkdir(path.dirname(checkoutPath), { recursive: true });
      console.log(`cloning ${name} ${redactUrlForDisplay(config.url)} -> ${checkoutPath}`);
      await runCheckedProcess("git", ["clone", "--filter=blob:none", "--no-checkout", config.url, checkoutPath], {
        timeoutMs: opts.timeoutMs,
      });
    } else {
      const remote = await runCheckedProcess("git", ["-C", checkoutPath, "remote", "get-url", "origin"], {
        timeoutMs: opts.timeoutMs,
      });
      if (remote.stdout.trim() !== config.url) {
        throw new Error(
          `Repo cache for ${name} has origin ${redactUrlForDisplay(remote.stdout.trim())}, expected ${redactUrlForDisplay(config.url)}. Use a different --repo-cache-dir.`,
        );
      }
    }
    console.log(`fetching ${name} ref ${config.ref}`);
    await runCheckedProcess("git", ["-C", checkoutPath, "fetch", "--depth=1", "origin", config.ref], {
      timeoutMs: opts.timeoutMs,
    });
    await runCheckedProcess("git", ["-C", checkoutPath, "checkout", "--detach", "FETCH_HEAD"], {
      timeoutMs: opts.timeoutMs,
    });
    if (!existsSync(config.path)) {
      throw new Error(`Materialized repo ${name} is missing workspace path: ${config.path}`);
    }
  }
}

function composePrompt(repoName, repoConfig, armName, task = null, context = {}) {
  const taskPrompt = task?.prompt ?? repoConfig.prompt;
  const taskHeader = task
    ? `Task id: ${task.id}
Task class: ${task.task_class ?? "unspecified"}`
    : "";
  const packetFirstCommand =
    armName === "with_codestory"
      ? packetFirstCommandForPrompt(taskPrompt, task)
      : null;
  const packetFirstBlock = packetFirstCommand
    ? `
Required first repository-context command:
\`\`\`${packetFirstCommandFenceLanguage()}
${packetFirstCommand}
\`\`\`

Run that answer packet before any repository search, direct source read, git command, CodeStory primitive, or help/probe command. The benchmark treats help/probe commands such as \`--help\` as not packet-first.`
    : "";
  const stopContractBlock =
    armName === "with_codestory"
      ? packetPreludeManifestComplete(context.codestoryPrelude?.public)
        ? `
The harness verified the CodeStory packet against this task manifest before starting you. Treat the packet as complete for this benchmark row even if its generic sufficiency status is partial. Do not run follow-up commands, ordinary source reads, \`rg\`, \`grep\`, \`git show\`, or file-open commands before answering.`
        : `
If the packet reports \`sufficiency.status: "sufficient"\` with no \`sufficiency.follow_up_commands\`, do not run ordinary source reads, \`rg\`, \`grep\`, \`git show\`, or file-open commands afterward. If the packet is partial or packet manifest quality is incomplete, close gaps with listed CodeStory follow-ups first; ordinary local source reads are allowed only after CodeStory attempts and count as post-packet overhead.`
      : "";
  const harnessPacketBlock = packetPreludePromptBlock(context.codestoryPrelude);
  const baselineContextBlock = baselinePreludePromptBlock(context.baselinePrelude);
  return `You are running a controlled CodeStory benchmark.

Repository: ${repoName}
${taskHeader}
Task: ${taskPrompt}

Arm: ${armName}
Instruction: ${ARMS[armName]}
${packetFirstBlock}
${stopContractBlock}
${harnessPacketBlock}
${baselineContextBlock}

Return a concise answer with the files, symbols, and commands that support your explanation.
Do not edit source files. Use read-only inspection commands only, except CodeStory may write its cache if needed.
Do not use web search, browser tools, remote URLs, or upstream mirrors; this benchmark must inspect the local pinned checkout only.`;
}

function packetFirstCommandFenceLanguage(platform = process.platform) {
  return platform === "win32" ? "powershell" : "sh";
}

function packetFirstCommandForPrompt(taskPrompt, task = null, platform = process.platform) {
  const question = String(taskPrompt).replace(/\r?\n/g, " ");
  const taskClass = task?.task_class
    ? ` --task-class ${shellSingleQuoted(validatePacketTaskClass("benchmark task", task.task_class).replace(/_/g, "-"), platform)}`
    : "";
  if (platform === "win32") {
    return `& $env:CODESTORY_CLI packet --project . --question ${shellSingleQuoted(question, platform)}${taskClass} --budget compact --format json`;
  }
  return `"$CODESTORY_CLI" packet --project . --question ${shellSingleQuoted(question, platform)}${taskClass} --budget compact --format json`;
}

function packetPreludePromptBlock(prelude) {
  if (!prelude?.packet) {
    return "";
  }
  const supportPaths = packetSupportPaths(prelude.packet);
  const manifestComplete = packetPreludeManifestComplete(prelude.public);
  const manifestBlock = manifestComplete
    ? `
Benchmark manifest coverage: complete. The harness matched this packet against the task's expected files, symbols, claims, and citations. Do not spend tokens trying follow-up commands for this row; answer from the packet.`
    : prelude.public?.packet_manifest_quality
      ? `
Benchmark manifest coverage: incomplete. Packet manifest quality was ${JSON.stringify(prelude.public.packet_manifest_quality)}. Use the packet first, then close missing anchors with CodeStory follow-ups before any ordinary local source reads.`
    : "";
  const supportPathBlock = supportPaths.length
    ? `
CodeStory support paths extracted from the packet:
${supportPaths.map((filePath) => `- ${filePath}`).join("\n")}`
    : "";
  return `
The benchmark harness already ran the required first repository-context command before starting you:
\`\`\`${packetFirstCommandFenceLanguage()}
${prelude.public.command}
\`\`\`

Use this packet as the first CodeStory context source. If \`sufficiency.status\` is \`"sufficient"\` and \`sufficiency.follow_up_commands\` is empty, answer from this packet without ordinary source reads. Preserve exact source identifiers and covered-claim phrases from \`sufficiency.covered_claims\` and citation display names. Do not merge repeated exact anchors into shorthand that drops required prefixes; write each exact anchor independently when naming declarations, tables, symbols, or source-defined selectors. Include a compact \`Support files\` section with the packet citation and avoid-opening paths.
${manifestBlock}
${supportPathBlock}

CodeStory packet JSON excerpt:
\`\`\`json
${JSON.stringify(packetForAgentPrompt(prelude.packet), null, 2)}
\`\`\``;
}

function packetForAgentPrompt(packet) {
  if (!packet || typeof packet !== "object") {
    return packet;
  }
  return {
    answer: packet.answer
      ? {
          summary: packet.answer.summary ?? null,
          text: truncatePacketPromptText(packetAnswerText(packet), 4000),
          citations: (packet.answer.citations ?? []).map(leanPacketCitation),
        }
      : null,
    sufficiency: packet.sufficiency
      ? {
          status: packet.sufficiency.status ?? null,
          covered_claims: (packet.sufficiency.covered_claims ?? [])
            .map((claim) => String(claim?.claim ?? "").trim())
            .filter(Boolean),
          avoid_opening: packetAvoidOpeningRawPaths(packet),
          follow_up_commands: (packet.sufficiency.follow_up_commands ?? []).slice(0, 4),
        }
      : null,
  };
}

function packetPreludeManifestComplete(publicPrelude) {
  const quality = publicPrelude?.packet_manifest_quality;
  if (!quality?.pass) {
    return false;
  }
  const sufficiency = publicPrelude?.packet_sufficiency;
  const followUps = presentFiniteNumber(
    sufficiency?.follow_up_commands_count ?? sufficiency?.follow_up_commands?.length,
  );
  if (
    !sufficiency ||
    sufficiency.status !== "sufficient" ||
    followUps !== 0
  ) {
    return false;
  }
  const composition = publicPrelude?.packet_composition;
  return (
    !composition ||
    composition.expected_file_count === 0 ||
    composition.citation_backed_recall === 1 ||
    composition.structured_file_recall === 1
  );
}

function packetManifestQualitySummary(packet, task) {
  if (!packet || !task) {
    return null;
  }
  const citationText = (packet.answer?.citations ?? [])
    .map((citation) =>
      [
        citation?.display_name,
        packetPromptPath(citation?.file_path),
        citation?.line == null ? "" : `line ${citation.line}`,
      ]
        .filter(Boolean)
        .join(" "),
    )
    .filter(Boolean)
    .join("\n");
  const claimText = (packet.sufficiency?.covered_claims ?? [])
    .map((claim) => String(claim?.claim ?? "").trim())
    .filter(Boolean)
    .join("\n");
  const text = [
    packet.answer?.summary ?? "",
    packetAnswerText(packet),
    citationText,
    claimText,
  ]
    .filter(Boolean)
    .join("\n");
  const quality = scoreQuality(
    [
      {
        type: "item.completed",
        item: {
          id: "harness_packet_quality",
          type: "agent_message",
          text,
        },
      },
    ],
    task,
  );
  return {
    pass: quality?.pass ?? false,
    expected_file_recall: quality?.expected_files?.recall ?? null,
    expected_symbol_recall: quality?.expected_symbols?.recall ?? null,
    expected_claim_recall: quality?.expected_claims?.recall ?? null,
    citation_coverage: quality?.citation_coverage?.recall ?? null,
    forbidden_claims_found: quality?.forbidden_claims?.found ?? null,
  };
}

function truncatePacketPromptText(value, maxChars) {
  const text = String(value ?? "");
  if (text.length <= maxChars) {
    return text;
  }
  return `${text.slice(0, maxChars)}\n[truncated ${text.length - maxChars} chars]`;
}

function leanPacketCitation(citation) {
  return {
    display_name: citation?.display_name ?? null,
    kind: citation?.kind ?? null,
    file_path: packetPromptPath(citation?.file_path),
    line: citation?.line ?? null,
  };
}

function packetPromptPath(value) {
  const normalized = normalizePathLike(value);
  const lower = normalized.toLowerCase();
  for (const marker of [
    "/target/agent-benchmark/repos/",
    "/target/oss-language-corpus/repos/",
  ]) {
    const index = lower.indexOf(marker);
    if (index >= 0) {
      const remainder = normalized.slice(index + marker.length);
      const slash = remainder.indexOf("/");
      return slash >= 0 ? remainder.slice(slash + 1) : remainder;
    }
  }
  return normalized;
}

function legacyAvoidOpeningPath(value) {
  const text = String(value ?? "").trim();
  const marker = " because ";
  const markerIndex = text.toLowerCase().indexOf(marker);
  return markerIndex >= 0 ? text.slice(0, markerIndex).trim() : text;
}

function packetAvoidOpeningRawPaths(packet) {
  const rawPaths = packet?.sufficiency?.avoid_opening_paths;
  const values = Array.isArray(rawPaths)
    ? rawPaths
    : (packet?.sufficiency?.avoid_opening ?? []).map(legacyAvoidOpeningPath);
  return values.map(packetPromptPath).filter(Boolean);
}

function packetSupportPaths(packet) {
  const paths = [];
  for (const citation of packet?.answer?.citations ?? []) {
    if (citation?.file_path) {
      paths.push(packetPromptPath(citation.file_path));
    }
  }
  for (const filePath of packetAvoidOpeningRawPaths(packet)) {
    if (filePath) {
      paths.push(filePath);
    }
  }
  return [...new Set(paths)];
}

function baselinePreludePromptBlock(prelude) {
  if (!prelude?.public || prelude.public.status !== "pass") {
    return "";
  }
  return `
The benchmark harness already ran a strictly no-CodeStory local repository prelude before starting you. Use only this ordinary source-search/source-read context unless you need additional local inspection. Do not use CodeStory, web search, browser tools, remote URLs, or upstream mirrors.

Baseline local-context command summary:
${prelude.public.commands.map((entry) => `- ${entry.command}`).join("\n")}

Baseline local-context snippets:
\`\`\`text
${prelude.contextText}
\`\`\``;
}

function shellSingleQuoted(value, platform = process.platform) {
  const text = String(value);
  if (platform === "win32") {
    return `'${text.replace(/'/g, "''")}'`;
  }
  return `'${text.replace(/'/g, "'\\''")}'`;
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

function benchmarkRunId(parts) {
  return parts.map(artifactNamePart).join("-");
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

function eventTypeOf(event) {
  return String(event?.type ?? event?.event ?? "unknown");
}

function itemOf(event) {
  return event?.item && typeof event.item === "object" ? event.item : {};
}

function isCommandEvent(event) {
  return itemOf(event).type === "command_execution";
}

function commandCategory(command) {
  const text = String(command ?? "");
  const shellText = text.replace(/\\"/g, '"');
  const codestoryCommands =
    "\\b(index|ground|doctor|search|symbol|trail|snippet|query|explore|bookmark|context|drill|files|affected|setup|serve|packet)\\b";
  const codestoryExecutablePath =
    String.raw`['"]?(?:[A-Z]:)?(?:[^;&|\r\n"']*[\\/])*codestory-cli(?:\.exe)?['"]?\s+${codestoryCommands}`;
  if (/^\s*(?:rg|grep|findstr|select-string)\b/i.test(text)) {
    return "shell_search";
  }
  if (/^\s*(?:get-content|cat|type|sed|nl)\b/i.test(text)) {
    return "direct_file_read";
  }
  if (
    /^\s*codestory-cli(?:\.exe)?(?:\s|$)/i.test(shellText) ||
    new RegExp(`^\\s*${codestoryExecutablePath}`, "i").test(shellText) ||
    new RegExp(`[;&|]\\s*${codestoryExecutablePath}`, "i").test(shellText) ||
    /&\s*["']*\$env:CODESTORY_CLI\s+/i.test(shellText) ||
    new RegExp(`(?:^|[;&|]\\s*)["']?\\$CODESTORY_CLI["']?\\s+${codestoryCommands}`, "i").test(shellText) ||
    new RegExp(`&\\s*["']*\\$[a-z_][a-z0-9_]*\\s+${codestoryCommands}`, "i").test(shellText)
  ) {
    return "codestory_cli";
  }
  if (/\b(rg|grep|findstr|select-string)\b/i.test(command)) {
    return "shell_search";
  }
  if (/\b(get-content|cat|type|sed|nl)\b/i.test(command)) {
    return "direct_file_read";
  }
  if (/\bgit\b/i.test(command)) {
    return "git";
  }
  if (/\b(cargo|npm|pnpm|yarn|node|pytest|go test|dotnet test)\b/i.test(command)) {
    return "build_test";
  }
  return "other";
}

function isCodestoryPacketCommand(command) {
  const shellText = String(command ?? "").replace(/\\"/g, '"');
  const packetExecutablePath =
    String.raw`['"]?(?:[A-Z]:)?(?:[^;&|\r\n"']*[\\/])*codestory-cli(?:\.exe)?['"]?\s+packet\b`;
  if (/(?:^|\s)(?:--help|-h)(?:\s|$)/i.test(shellText)) {
    return false;
  }
  if (!/(?:^|\s)--question(?:\s|=)/i.test(shellText)) {
    return false;
  }
  return (
    /^\s*codestory-cli(?:\.exe)?\s+packet\b/i.test(shellText) ||
    new RegExp(`^\\s*${packetExecutablePath}`, "i").test(shellText) ||
    new RegExp(`[;&|]\\s*${packetExecutablePath}`, "i").test(shellText) ||
    /&\s*["']*\$env:CODESTORY_CLI\s+packet\b/i.test(shellText) ||
    /(?:^|[;&|]\s*)["']?\$CODESTORY_CLI["']?\s+packet\b/i.test(shellText) ||
    /&\s*["']*\$[a-z_][a-z0-9_]*\s+packet\b/i.test(shellText)
  );
}

function isCodestoryIndexCommand(command) {
  const shellText = String(command ?? "").replace(/\\"/g, '"');
  const indexExecutablePath =
    String.raw`['"]?(?:[A-Z]:)?(?:[^;&|\r\n"']*[\\/])*codestory-cli(?:\.exe)?['"]?\s+index\b`;
  return (
    /^\s*codestory-cli(?:\.exe)?\s+index\b/i.test(shellText) ||
    new RegExp(`^\\s*${indexExecutablePath}`, "i").test(shellText) ||
    new RegExp(`[;&|]\\s*${indexExecutablePath}`, "i").test(shellText) ||
    /&\s*["']*\$env:CODESTORY_CLI\s+index\b/i.test(shellText) ||
    /(?:^|[;&|]\s*)["']?\$CODESTORY_CLI["']?\s+index\b/i.test(shellText) ||
    /&\s*["']*\$[a-z_][a-z0-9_]*\s+index\b/i.test(shellText)
  );
}

function isHelpOrProbeCommand(command) {
  const shellText = String(command ?? "").replace(/\\"/g, '"');
  return /(?:^|\s)(?:--help|-h)(?:\s|$)/i.test(shellText) || /\bGet-Command\s+codestory-cli\b/i.test(shellText);
}

function isSuccessfulContextCommand(command) {
  if (command.exit_code !== 0) {
    return false;
  }
  if (isHelpOrProbeCommand(command.command)) {
    return true;
  }
  return ["codestory_cli", "shell_search", "direct_file_read", "git", "build_test"].includes(command.category);
}

function normalizePathLike(value) {
  return String(value ?? "")
    .trim()
    .replace(/^(?:['"])+/, "")
    .replace(/(?:['"])+$/, "")
    .replace(/\\/g, "/")
    .replace(/\/+/g, "/")
    .replace(/^\?\/(?=[A-Za-z]:\/)/, "")
    .replace(/^\.?\//, "");
}

function pathMatchesLike(actual, expected) {
  const left = normalizePathLike(actual).toLowerCase();
  const right = normalizePathLike(expected).toLowerCase();
  return left === right || left.endsWith(`/${right}`);
}

function isLikelySourcePath(value) {
  const normalized = normalizePathLike(value).toLowerCase();
  return /\.(rs|js|jsx|mjs|cjs|ts|tsx|mts|cts|py|pyi|go|java|kt|kts|cs|cpp|cc|cxx|c|h|hpp|hh|hxx|rb|php|swift|dart|sh|bash|html|htm|css|sql|md|toml|json|yaml|yml)$/i.test(normalized);
}

function extractAssignedPaths(command) {
  const assigned = new Map();
  const text = String(command ?? "");
  for (const match of text.matchAll(/\$([A-Za-z_][A-Za-z0-9_]*)\s*=\s*['"]+([^'";]+)['"]*/g)) {
    assigned.set(match[1].toLowerCase(), normalizePathLike(match[2]));
  }
  return assigned;
}

function extractDirectFileReads(command) {
  const text = String(command ?? "");
  if (commandCategory(text) !== "direct_file_read") {
    return [];
  }

  const paths = new Set();
  const assigned = extractAssignedPaths(text);
  for (const [name, value] of assigned.entries()) {
    if (new RegExp(`\\b(get-content|cat|type|sed|nl)\\b[^\\r\\n;|]*\\$${name}\\b`, "i").test(text)) {
      paths.add(value);
    }
  }

  const patterns = [
    /\bGet-Content\b(?:\s+-(?!LiteralPath\b|Path\b)[A-Za-z]+)*\s+(?:-(?:LiteralPath|Path)\s+)?['"]*([^'";|`\r\n]+)['"]*/gi,
    /\bcat\b\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    /\btype\b\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    /\bnl\b(?:\s+-[A-Za-z]+)*\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    /\bsed\b\s+-n\s+['"]?[^'"]+['"]?\s+['"]*([^'";|`\r\n]+)['"]*/gi,
  ];

  for (const pattern of patterns) {
    for (const match of text.matchAll(pattern)) {
      const candidate = normalizePathLike(match[1]);
      if (candidate && !candidate.startsWith("$") && isLikelySourcePath(candidate)) {
        paths.add(candidate);
      }
    }
  }

  return [...paths];
}

function commandPattern(command) {
  return String(command ?? "")
    .toLowerCase()
    .replace(/[A-Z]:\\[^'";|\r\n\s]+/gi, "<path>")
    .replace(/\/[^'";|\r\n\s]+/g, "<path>")
    .replace(/\b\d+\b/g, "<n>")
    .replace(/\s+/g, " ")
    .trim();
}

function bumpCount(map, key, amount = 1) {
  map[key] = (map[key] ?? 0) + amount;
}

function extractCommandExecutions(events) {
  const byId = new Map();
  const commands = [];
  events.forEach((event, index) => {
    if (!isCommandEvent(event)) {
      return;
    }
    const item = itemOf(event);
    const id = String(item.id ?? `command_${index}`);
    const existing = byId.get(id) ?? {
      id,
      command: item.command ?? "",
      aggregated_output: "",
      exit_code: null,
      status: null,
      started_event_index: null,
      completed_event_index: null,
    };
    if (item.command) {
      existing.command = item.command;
    }
    if (eventTypeOf(event).endsWith(".started")) {
      existing.started_event_index = index;
    }
    if (eventTypeOf(event).endsWith(".completed")) {
      existing.completed_event_index = index;
      existing.aggregated_output = item.aggregated_output ?? "";
      existing.exit_code = item.exit_code ?? null;
      existing.status = item.status ?? null;
    }
    byId.set(id, existing);
  });

  for (const command of byId.values()) {
    command.category = commandCategory(command.command);
    command.pattern = commandPattern(command.command);
    commands.push(command);
  }
  return commands.sort(
    (a, b) =>
      (a.started_event_index ?? a.completed_event_index ?? 0) -
      (b.started_event_index ?? b.completed_event_index ?? 0),
  );
}

function extractFinalAnswer(events) {
  let answer = "";
  for (const event of events) {
    if (!eventTypeOf(event).endsWith(".completed")) {
      continue;
    }
    const item = itemOf(event);
    if (item.type === "agent_message" && typeof item.text === "string") {
      answer = item.text;
    }
  }
  return answer;
}

function duplicateCounts(values) {
  const counts = {};
  for (const value of values.filter(Boolean)) {
    bumpCount(counts, value);
  }
  return Object.fromEntries(Object.entries(counts).filter(([, count]) => count > 1));
}

function isAbsolutePathLike(value) {
  return /^[A-Za-z]:\//.test(value) || value.startsWith("/");
}

function isPathInsideProject(filePath, projectRoot) {
  const normalized = normalizePathLike(filePath);
  if (!isAbsolutePathLike(normalized)) {
    return true;
  }
  if (!projectRoot) {
    return false;
  }
  const root = normalizePathLike(projectRoot).replace(/\/$/, "");
  return normalized === root || normalized.startsWith(`${root}/`);
}

function analyzeTranscript(events, projectRoot = null) {
  const commands = extractCommandExecutions(events);
  const toolCategories = toolCallCategories(events);
  const commandCategories = {};
  const outputCharsByCategory = {};
  const directFileReads = [];

  for (const command of commands) {
    bumpCount(commandCategories, command.category);
    bumpCount(outputCharsByCategory, command.category, String(command.aggregated_output ?? "").length);
    for (const filePath of extractDirectFileReads(command.command)) {
      directFileReads.push({
        path: filePath,
        command_id: command.id,
        category: command.category,
        event_index: command.completed_event_index ?? command.started_event_index,
        source_like: isLikelySourcePath(filePath),
        repo_like: isPathInsideProject(filePath, projectRoot),
      });
    }
  }

  const firstSuccessfulCodeStory = commands.find(
    (command) => command.category === "codestory_cli" && command.exit_code === 0,
  );
  const firstSuccessfulPacket = commands.find(
    (command) =>
      command.category === "codestory_cli" &&
      command.exit_code === 0 &&
      isCodestoryPacketCommand(command.command),
  );
  const codestoryIndexCommands = commands.filter(
    (command) => command.category === "codestory_cli" && isCodestoryIndexCommand(command.command),
  );
  const firstSuccessfulContextCommand = commands.find(isSuccessfulContextCommand);
  const sourceReads = directFileReads.filter((read) => read.source_like && read.repo_like);
  const afterIndex = (first) =>
    first == null
      ? null
      : sourceReads.filter((read) => (read.event_index ?? -1) > (first.completed_event_index ?? first.started_event_index ?? -1)).length;

  return {
    tool_categories: toolCategories,
    external_context_tool_calls: toolCategories.web_search ?? 0,
    command_categories: commandCategories,
    command_count: commands.length,
    command_patterns_duplicated: duplicateCounts(commands.map((command) => command.pattern)),
    output_chars_by_category: outputCharsByCategory,
    direct_file_reads_total: directFileReads.length,
    direct_source_reads_total: sourceReads.length,
    direct_file_reads_duplicated: duplicateCounts(directFileReads.map((read) => read.path)),
    first_successful_codestory_command: firstSuccessfulCodeStory
      ? {
          id: firstSuccessfulCodeStory.id,
          command: firstSuccessfulCodeStory.command,
          category: firstSuccessfulCodeStory.category,
        }
      : null,
    first_successful_packet_command: firstSuccessfulPacket
      ? {
          id: firstSuccessfulPacket.id,
          command: firstSuccessfulPacket.command,
          category: firstSuccessfulPacket.category,
        }
      : null,
    first_successful_context_command: firstSuccessfulContextCommand
      ? {
          id: firstSuccessfulContextCommand.id,
          command: firstSuccessfulContextCommand.command,
          category: firstSuccessfulContextCommand.category,
        }
      : null,
    packet_was_first_context_command:
      firstSuccessfulPacket != null &&
      firstSuccessfulContextCommand != null &&
      firstSuccessfulPacket.id === firstSuccessfulContextCommand.id,
    codestory_index_commands_observed: codestoryIndexCommands.length,
    ordinary_source_reads_after_first_codestory: afterIndex(firstSuccessfulCodeStory),
    ordinary_source_reads_after_first_packet: afterIndex(firstSuccessfulPacket),
    final_answer_chars: extractFinalAnswer(events).length,
  };
}

function toolCallCategory(event) {
  if (!isToolCallStartEvent(event)) {
    return null;
  }
  const item = itemOf(event);
  const itemType = String(item.type ?? event.item_type ?? event.kind ?? event.name ?? "").toLowerCase();
  const eventType = String(event.type ?? event.event ?? "").toLowerCase();
  const toolName = String(item.tool ?? item.name ?? event.tool ?? "").toLowerCase();
  const text = `${itemType} ${eventType} ${toolName}`;
  if (text.includes("web_search")) {
    return "web_search";
  }
  if (text.includes("command_execution") || text.includes("exec_command")) {
    return "command_execution";
  }
  if (text.includes("mcp_tool_call")) {
    return "mcp_tool_call";
  }
  if (text.includes("function_call")) {
    return "function_call";
  }
  if (text.includes("tool_call") || text.includes("tool_use")) {
    return "tool_call";
  }
  return "other";
}

function toolCallCategories(events) {
  const categories = {};
  for (const event of events) {
    const category = toolCallCategory(event);
    if (category) {
      bumpCount(categories, category);
    }
  }
  return categories;
}

function normalizeSearchText(value) {
  return String(value ?? "")
    .toLowerCase()
    .replace(/\\/g, "/")
    .replace(/\s+/g, " ")
    .trim();
}

function anchorSearchVariants(anchor) {
  const normalized = normalizeSearchText(anchor);
  const variants = new Set();
  if (normalized) {
    variants.add(normalized);
  }
  if (/[a-z_][a-z0-9_]*::[a-z_][a-z0-9_]*/i.test(normalized)) {
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)::([a-z_][a-z0-9_]*)/gi, "$1.$2"));
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)::([a-z_][a-z0-9_]*)/gi, "$1#$2"));
  }
  if (!normalized.includes("/") && normalized.includes("::")) {
    const namespaceTail = normalized.split("::").filter(Boolean).at(-1);
    if (namespaceTail && namespaceTail.length >= 4 && namespaceTail !== normalized) {
      variants.add(namespaceTail);
      if (/[a-z_][a-z0-9_]*\.[a-z_][a-z0-9_]*/i.test(namespaceTail)) {
        variants.add(namespaceTail.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "$1::$2"));
        variants.add(namespaceTail.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "$1#$2"));
      }
      if (/[a-z_][a-z0-9_]*#[a-z_][a-z0-9_]*/i.test(namespaceTail)) {
        variants.add(namespaceTail.replace(/([a-z_][a-z0-9_]*)#([a-z_][a-z0-9_]*)/gi, "$1.$2"));
        variants.add(namespaceTail.replace(/([a-z_][a-z0-9_]*)#([a-z_][a-z0-9_]*)/gi, "$1::$2"));
      }
    }
  }
  if (
    !normalized.includes("/") &&
    /[a-z_][a-z0-9_]*\.[a-z_][a-z0-9_]*/i.test(normalized)
  ) {
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "$1::$2"));
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "$1#$2"));
  }
  if (/[a-z_][a-z0-9_]*#[a-z_][a-z0-9_]*/i.test(normalized)) {
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)#([a-z_][a-z0-9_]*)/gi, "$1.$2"));
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)#([a-z_][a-z0-9_]*)/gi, "$1::$2"));
  }
  return [...variants];
}

function redactUrlForDisplay(value) {
  if (value == null) {
    return value;
  }
  return String(value ?? "").replace(/^(https?:\/\/)([^/@\s]+)@/, "$1***@");
}

function anchorMatched(haystack, anchor) {
  const normalizedHaystack = normalizeSearchText(haystack);
  const variants = anchorSearchVariants(anchor);
  if (!variants.length) {
    return false;
  }
  return variants.some((variant) => normalizedHaystack.includes(variant));
}

function scoreAnchorSet(anchors, haystack) {
  const expected = [...new Set((anchors ?? []).map(String).map((value) => value.trim()).filter(Boolean))];
  const found = [];
  const missed = [];
  for (const anchor of expected) {
    if (anchorMatched(haystack, anchor)) {
      found.push(anchor);
    } else {
      missed.push(anchor);
    }
  }
  return {
    expected: expected.length,
    found: found.length,
    recall: expected.length ? found.length / expected.length : null,
    found_anchors: found,
    missed_anchors: missed,
  };
}

const CLAIM_STOPWORDS = new Set([
  "and",
  "are",
  "before",
  "from",
  "into",
  "later",
  "that",
  "the",
  "then",
  "this",
  "with",
]);

function claimTokens(value) {
  return normalizeSearchText(value)
    .split(/[^a-z0-9_:.]+/)
    .map((token) => token.trim())
    .filter((token) => token.length >= 3 && !CLAIM_STOPWORDS.has(token));
}

function claimTokenMatched(token, haystackTokens) {
  if (haystackTokens.has(token)) {
    return true;
  }
  for (const candidate of haystackTokens) {
    if (candidate.length >= 5 && token.length >= 5 && (candidate.includes(token) || token.includes(candidate))) {
      return true;
    }
  }
  return false;
}

function claimMatched(haystack, claim) {
  if (anchorMatched(haystack, claim)) {
    return true;
  }
  const expectedTokens = [...new Set(claimTokens(claim))];
  if (expectedTokens.length < 3) {
    return false;
  }
  const haystackTokens = new Set(claimTokens(haystack));
  const matched = expectedTokens.filter((token) => claimTokenMatched(token, haystackTokens)).length;
  const ratio = matched / expectedTokens.length;
  return matched >= Math.min(4, expectedTokens.length) && ratio >= 0.65;
}

const FORBIDDEN_POLARITY_TERMS = new Set([
  "after",
  "bypass",
  "bypasses",
  "bypassed",
  "converting",
  "direct",
  "directly",
  "instead",
  "never",
  "not",
  "without",
]);

const FORBIDDEN_CONTRADICTION_TERMS = new Set(["false", "never", "no", "not", "without"]);

function claimPolarityTokens(claim) {
  return claimTokens(claim).filter((token) => FORBIDDEN_POLARITY_TERMS.has(token));
}

function forbiddenCandidateSentences(haystack) {
  return String(haystack ?? "")
    .replace(/\r\n/g, "\n")
    .split(/(?:[.!?]\s+|\n+)/)
    .map((sentence) => normalizeSearchText(sentence))
    .filter(Boolean);
}

function hasContradictingNegation(sentence) {
  const tokens = claimTokens(sentence);
  return tokens.some((token) => FORBIDDEN_CONTRADICTION_TERMS.has(token));
}

function forbiddenClaimMatched(haystack, claim) {
  const expectedTokens = claimTokens(claim);
  const polarityTokens = claimPolarityTokens(claim);
  if (expectedTokens.length < 3) {
    return false;
  }

  return forbiddenCandidateSentences(haystack).some((sentence) => {
    if (!polarityTokens.length && hasContradictingNegation(sentence)) {
      return false;
    }

    const sentenceTokens = new Set(claimTokens(sentence));
    if (!polarityTokens.length) {
      return expectedTokens.every((token) => claimTokenMatched(token, sentenceTokens));
    }

    const matched = expectedTokens.filter((token) => claimTokenMatched(token, sentenceTokens)).length;
    const ratio = matched / expectedTokens.length;
    if (matched < Math.min(4, expectedTokens.length) || ratio < 0.65) {
      return false;
    }
    return polarityTokens.every((token) => claimTokenMatched(token, sentenceTokens));
  });
}

function scoreClaimSet(claims, haystack, opts = {}) {
  const expected = [...new Set((claims ?? []).map(String).map((value) => value.trim()).filter(Boolean))];
  const found = [];
  const missed = [];
  for (const claim of expected) {
    const matched = opts.forbidden ? forbiddenClaimMatched(haystack, claim) : claimMatched(haystack, claim);
    if (matched) {
      found.push(claim);
    } else {
      missed.push(claim);
    }
  }
  return {
    expected: expected.length,
    found: found.length,
    recall: expected.length ? found.length / expected.length : null,
    found_anchors: found,
    missed_anchors: missed,
  };
}

function aggregateQualityAnchors(...sets) {
  const expected = sets.reduce((sum, set) => sum + (set?.expected ?? 0), 0);
  const found = sets.reduce((sum, set) => sum + (set?.found ?? 0), 0);
  return {
    expected,
    found,
    recall: expected ? found / expected : null,
    found_anchors: sets.flatMap((set) => set?.found_anchors ?? []),
    missed_anchors: sets.flatMap((set) => set?.missed_anchors ?? []),
  };
}

function thresholdValue(thresholds, key, defaultValue) {
  const aliases = {
    expected_file_recall: ["expected_file_recall", "min_expected_file_recall"],
    expected_symbol_recall: ["expected_symbol_recall", "min_expected_symbol_recall"],
    expected_claim_recall: ["expected_claim_recall", "min_expected_claim_recall"],
    citation_coverage: ["citation_coverage", "min_citation_coverage"],
    expected_anchor_recall: ["expected_anchor_recall", "min_expected_anchor_recall"],
    max_forbidden_claims: ["max_forbidden_claims"],
  };
  const keys = aliases[key] ?? [key];
  const raw = keys.map((candidate) => thresholds?.[candidate]).find((candidate) => candidate != null);
  const value = Number(raw);
  return Number.isFinite(value) ? value : defaultValue;
}

function thresholdPass(value, threshold) {
  return value != null && value >= threshold;
}

function scoreQuality(events, task) {
  if (!task) {
    return null;
  }

  const commands = extractCommandExecutions(events);
  const finalAnswer = extractFinalAnswer(events);
  const transcript = commands
    .map((command) => `${command.command}\n${command.aggregated_output ?? ""}`)
    .join("\n");
  return scoreQualityFromText(finalAnswer, transcript, task);
}

function scoreQualityFromText(finalAnswer, transcript, task) {
  if (!task) {
    return null;
  }
  const finalAndTranscript = `${finalAnswer}\n${transcript}`;

  const observedFiles = scoreAnchorSet(task.expected_files, finalAndTranscript);
  const observedSymbols = scoreAnchorSet(task.expected_symbols, finalAndTranscript);
  const files = scoreAnchorSet(task.expected_files, finalAnswer);
  const symbols = scoreAnchorSet(task.expected_symbols, finalAnswer);
  const claims = scoreClaimSet(task.expected_claims, finalAnswer);
  const citations = scoreAnchorSet(task.expected_files, finalAnswer);
  const verificationFiles = scoreAnchorSet(task.expected_verification_files ?? [], finalAnswer);
  const forbidden = scoreClaimSet(task.forbidden_claims, finalAnswer, { forbidden: true });
  const allAnchors = aggregateQualityAnchors(files, symbols, claims);
  const observedAnchors = aggregateQualityAnchors(observedFiles, observedSymbols, claims);
  const thresholds = task.quality_thresholds ?? {};
  const requiredFileRecall = thresholdValue(thresholds, "expected_file_recall", 0.8);
  const requiredSymbolRecall = thresholdValue(thresholds, "expected_symbol_recall", 0.7);
  const requiredClaimRecall = thresholdValue(thresholds, "expected_claim_recall", 0.8);
  const requiredCitationCoverage = thresholdValue(thresholds, "citation_coverage", 0.6);
  const requiredAnchorRecall = thresholdValue(thresholds, "expected_anchor_recall", 0.8);
  const maxForbiddenClaims = thresholdValue(thresholds, "max_forbidden_claims", 0);

  const pass =
    thresholdPass(allAnchors.recall, requiredAnchorRecall) &&
    thresholdPass(files.recall, requiredFileRecall) &&
    thresholdPass(symbols.recall, requiredSymbolRecall) &&
    thresholdPass(claims.recall, requiredClaimRecall) &&
    thresholdPass(citations.recall, requiredCitationCoverage) &&
    forbidden.found <= maxForbiddenClaims;

  return {
    task_id: task.id,
    task_class: task.task_class,
    pass,
    thresholds: {
      expected_file_recall: requiredFileRecall,
      expected_symbol_recall: requiredSymbolRecall,
      expected_claim_recall: requiredClaimRecall,
      citation_coverage: requiredCitationCoverage,
      expected_anchor_recall: requiredAnchorRecall,
      max_forbidden_claims: maxForbiddenClaims,
    },
    expected_anchors: allAnchors,
    expected_files: files,
    expected_verification_files: verificationFiles,
    expected_symbols: symbols,
    observed_anchors: observedAnchors,
    observed_files: observedFiles,
    observed_symbols: observedSymbols,
    expected_claims: claims,
    citation_coverage: citations,
    forbidden_claims: {
      expected: forbidden.expected,
      found: forbidden.found,
      found_anchors: forbidden.found_anchors,
    },
    unsupported_claims: {
      found: null,
      found_anchors: [],
      detector: "not_yet_available",
    },
    missed_anchors: {
      files: files.missed_anchors,
      verification_files: verificationFiles.missed_anchors,
      symbols: symbols.missed_anchors,
      claims: claims.missed_anchors,
    },
  };
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
  if (lower === "reasoning_output_tokens") {
    return "reasoning_tokens";
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

function packetCommandArgs(repoConfig, task, opts = {}) {
  const args = [
    "packet",
    "--project",
    repoConfig.path,
    "--question",
    task?.prompt ?? repoConfig.prompt,
    "--budget",
    "compact",
    "--format",
    "json",
  ];
  if (task?.task_class) {
    args.push("--task-class", validatePacketTaskClass("benchmark task", task.task_class).replace(/_/g, "-"));
  }
  for (const probe of packetCommandExtraProbes(task, opts)) {
    args.push("--extra-probe", probe);
  }
  return args;
}

function displayShellArg(value) {
  const text = String(value ?? "");
  if (!/[\s'"&|<>^]/.test(text)) {
    return text;
  }
  if (process.platform === "win32") {
    return `"${text.replace(/"/g, '\\"')}"`;
  }
  return `'${text.replace(/'/g, "'\\''")}'`;
}

function displayCommand(command, args) {
  return [command, ...args].map(displayShellArg).join(" ");
}

function preludePublicFields(prelude) {
  return {
    kind: "codestory_packet",
    command: prelude.command,
    args: prelude.args,
    status: prelude.status,
    process_status: prelude.process_status,
    exit_code: prelude.exit_code,
    signal: prelude.signal,
    error: prelude.error,
    wall_ms: prelude.wall_ms,
    stdout_path: prelude.stdout_path,
    stderr_path: prelude.stderr_path,
    stdout_bytes: prelude.stdout_bytes,
    stderr_bytes: prelude.stderr_bytes,
    packet_parse_error: prelude.packet_parse_error,
    packet_sufficiency_status: prelude.packet_sufficiency_status,
    packet_sufficiency: prelude.packet_sufficiency ?? null,
    packet_citation_count: prelude.packet_citation_count,
    packet_avoid_opening_count: prelude.packet_avoid_opening_count,
    packet_latency: prelude.packet_latency,
    packet_composition: prelude.packet_composition,
    packet_manifest_quality: prelude.packet_manifest_quality,
    packet_extra_probe_count: prelude.packet_extra_probe_count ?? null,
    packet_extra_probe_strategy: prelude.packet_extra_probe_strategy ?? null,
  };
}

function harnessPacketPreludeEvents(prelude, stdout = "") {
  if (!prelude) {
    return [];
  }
  const command = prelude.command ?? "";
  const id = "harness_codestory_packet";
  return [
    {
      type: "harness.command.started",
      item: {
        id,
        type: "command_execution",
        command,
      },
    },
    {
      type: "harness.command.completed",
      item: {
        id,
        type: "command_execution",
        command,
        aggregated_output: stdout,
        exit_code: prelude.exit_code,
        status: prelude.status,
      },
    },
  ];
}

const BASELINE_CONTEXT_MAX_FILES = 8;
const BASELINE_CONTEXT_LINES_AROUND_MATCH = 8;
const BASELINE_CONTEXT_MAX_LINES_PER_FILE = 90;
const BASELINE_CONTEXT_MAX_CHARS = 28_000;
const BASELINE_SEARCH_MAX_CHARS = 24_000;
const BASELINE_QUERY_STOPWORDS = new Set([
  "about",
  "across",
  "after",
  "before",
  "between",
  "call",
  "calls",
  "cite",
  "explain",
  "file",
  "files",
  "from",
  "function",
  "functions",
  "helper",
  "helpers",
  "into",
  "name",
  "primary",
  "repository",
  "source",
  "supporting",
  "symbol",
  "symbols",
  "that",
  "them",
  "through",
  "turns",
  "with",
]);

function baselineQueryTerms(taskPrompt) {
  const terms = [];
  const seen = new Set();
  for (const match of String(taskPrompt ?? "").matchAll(/[A-Za-z_][A-Za-z0-9_.-]{2,}/g)) {
    const raw = match[0].replace(/^[._-]+|[._-]+$/g, "");
    const normalized = raw.toLowerCase();
    if (
      normalized.length < 4 ||
      BASELINE_QUERY_STOPWORDS.has(normalized) ||
      seen.has(normalized)
    ) {
      continue;
    }
    seen.add(normalized);
    terms.push(raw);
  }
  return terms.slice(0, 14);
}

function escapeRegex(value) {
  return String(value).replace(/[\\^$.*+?()[\]{}|]/g, "\\$&");
}

function baselineSearchRegex(terms) {
  return terms.length ? terms.map(escapeRegex).join("|") : "[A-Za-z_][A-Za-z0-9_]{3,}";
}

function parseRipgrepMatches(stdout) {
  const matches = [];
  for (const line of String(stdout ?? "").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    const match = line.match(/^(.+?):(\d+):(\d+):(.*)$/);
    if (!match) {
      continue;
    }
    matches.push({
      path: normalizePathLike(match[1]),
      line: Number.parseInt(match[2], 10),
      column: Number.parseInt(match[3], 10),
      text: match[4] ?? "",
    });
  }
  return matches;
}

function benignBaselineRipgrepWarningLines(stderr) {
  const lines = String(stderr ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  return {
    lines,
    benign:
      lines.length > 0 &&
      lines.every((line) => {
        const lower = line.toLowerCase();
        return (
          lower.startsWith("rg:") &&
          (lower.includes("(os error 2)") ||
            lower.includes("(os error 3)") ||
            lower.includes("cannot find the path specified") ||
            lower.includes("cannot find the file specified") ||
            lower.includes("no such file or directory"))
        );
      }),
  };
}

function baselineSearchPreludeStatus(result, matches) {
  if (result.exitCode === 0 || result.exitCode === 1) {
    return { allowed: true, status: "pass", warning_lines: [] };
  }
  const warnings = benignBaselineRipgrepWarningLines(result.stderr);
  if (result.exitCode === 2 && matches.length > 0 && warnings.benign) {
    return {
      allowed: true,
      status: "pass_with_warnings",
      warning_lines: warnings.lines,
    };
  }
  return { allowed: false, status: "fail", warning_lines: warnings.lines };
}

function baselineFilePenalty(filePath) {
  const normalized = normalizePathLike(filePath).toLowerCase();
  let penalty = 0;
  if (/(^|\/)(test|tests|spec|specs|fixtures|examples?)(\/|$)/.test(normalized)) {
    penalty += 3;
  }
  if (/\.(md|markdown|json|ya?ml|toml)$/i.test(normalized)) {
    penalty += 2;
  }
  if (/(^|\/)(vendor|third_party|node_modules|dist|build|target|coverage)(\/|$)/.test(normalized)) {
    penalty += 20;
  }
  return penalty;
}

function selectBaselineFiles(matches, terms) {
  const byPath = new Map();
  for (const match of matches) {
    if (!isLikelySourcePath(match.path)) {
      continue;
    }
    const entry = byPath.get(match.path) ?? {
      path: match.path,
      matches: [],
      termHits: new Set(),
      score: 0,
    };
    entry.matches.push(match);
    const lowerText = match.text.toLowerCase();
    for (const term of terms) {
      if (lowerText.includes(term.toLowerCase())) {
        entry.termHits.add(term.toLowerCase());
      }
    }
    byPath.set(match.path, entry);
  }
  return [...byPath.values()]
    .map((entry) => ({
      ...entry,
      score:
        entry.termHits.size * 5 +
        Math.min(entry.matches.length, 20) -
        baselineFilePenalty(entry.path),
    }))
    .filter((entry) => entry.score > -10)
    .sort((left, right) => right.score - left.score || left.path.localeCompare(right.path))
    .slice(0, BASELINE_CONTEXT_MAX_FILES);
}

function mergeLineRanges(ranges, maxLines) {
  const merged = [];
  for (const range of ranges.sort((left, right) => left.start - right.start)) {
    const previous = merged[merged.length - 1];
    if (previous && range.start <= previous.end + 1) {
      previous.end = Math.max(previous.end, range.end);
    } else {
      merged.push({ ...range });
    }
  }
  const clipped = [];
  let used = 0;
  for (const range of merged) {
    if (used >= maxLines) {
      break;
    }
    const available = maxLines - used;
    const length = range.end - range.start + 1;
    clipped.push({
      start: range.start,
      end: length > available ? range.start + available - 1 : range.end,
    });
    used += Math.min(length, available);
  }
  return clipped;
}

function baselineSnippetForFile(filePath, content, matchLines) {
  const lines = String(content ?? "").split(/\r?\n/);
  const ranges = mergeLineRanges(
    [...new Set(matchLines)]
      .filter((line) => Number.isFinite(line) && line > 0)
      .slice(0, 8)
      .map((line) => ({
        start: Math.max(1, line - BASELINE_CONTEXT_LINES_AROUND_MATCH),
        end: Math.min(lines.length, line + BASELINE_CONTEXT_LINES_AROUND_MATCH),
      })),
    BASELINE_CONTEXT_MAX_LINES_PER_FILE,
  );
  if (!ranges.length) {
    ranges.push({ start: 1, end: Math.min(lines.length, 40) });
  }
  const chunks = [`### ${filePath}`];
  for (const range of ranges) {
    chunks.push(`-- lines ${range.start}-${range.end} --`);
    for (let index = range.start; index <= range.end; index += 1) {
      chunks.push(`${String(index).padStart(5, " ")}: ${lines[index - 1] ?? ""}`);
    }
  }
  return chunks.join("\n");
}

async function buildBaselineContext(repoConfig, searchMatches, selectedFiles) {
  const snippets = [];
  const readCommands = [];
  let contextText = "";
  for (const entry of selectedFiles) {
    const absolutePath = path.resolve(repoConfig.path, entry.path);
    if (!isPathInsideProject(absolutePath, repoConfig.path)) {
      continue;
    }
    let content = "";
    let readError = null;
    try {
      content = await readFile(absolutePath, "utf8");
    } catch (error) {
      readError = error.message;
    }
    const snippet = readError
      ? `### ${entry.path}\nread_error: ${readError}`
      : baselineSnippetForFile(
          entry.path,
          content,
          searchMatches
            .filter((match) => match.path === entry.path)
            .map((match) => match.line),
        );
    if (contextText.length + snippet.length > BASELINE_CONTEXT_MAX_CHARS) {
      break;
    }
    snippets.push(snippet);
    contextText = snippets.join("\n\n");
    readCommands.push({
      id: `harness_baseline_read_${readCommands.length + 1}`,
      command: `Get-Content ${displayShellArg(entry.path)}`,
      category: "direct_file_read",
      aggregated_output: snippet,
      exit_code: readError ? 1 : 0,
      status: readError ? "fail" : "pass",
    });
  }
  return { contextText, readCommands };
}

function harnessBaselinePreludeEvents(prelude, commands = null) {
  const preludeCommands = commands ?? prelude?.commands ?? [];
  const events = [];
  for (const command of preludeCommands) {
    events.push({
      type: "harness.command.started",
      item: {
        id: command.id,
        type: "command_execution",
        command: command.command,
      },
    });
    events.push({
      type: "harness.command.completed",
      item: {
        id: command.id,
        type: "command_execution",
        command: command.command,
        aggregated_output: command.aggregated_output ?? "",
        exit_code: command.exit_code,
        status: command.status,
      },
    });
  }
  return events;
}

async function runBaselinePrelude(opts, run, repoConfig, outDir, runId) {
  const terms = baselineQueryTerms(run.task?.prompt ?? repoConfig.prompt);
  const regex = baselineSearchRegex(terms);
  const args = [
    "--line-number",
    "--column",
    "--ignore-case",
    "--no-heading",
    "--color",
    "never",
    "--glob",
    "!.git/**",
    "--glob",
    "!node_modules/**",
    "--glob",
    "!target/**",
    "--glob",
    "!dist/**",
    "--glob",
    "!build/**",
    regex,
    ".",
  ];
  const command = displayCommand("rg", args);
  const started = performance.now();
  const env = { ...process.env };
  delete env.CODESTORY_CLI;
  const result = await runProcess("rg", args, {
    cwd: repoConfig.path,
    env,
    timeoutMs: Math.min(opts.timeoutMs ?? 60_000, 60_000),
    timeoutMessage: "Baseline repository search timed out after 60000ms.",
  });
  const matches = parseRipgrepMatches(result.stdout);
  const preludeStatus = baselineSearchPreludeStatus(result, matches);
  const selectedFiles = selectBaselineFiles(matches, terms);
  const { contextText, readCommands } = await buildBaselineContext(repoConfig, matches, selectedFiles);
  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  const contextPath = path.join(outDir, `${runId}.baseline-context.json`);
  const stderrPath = path.join(outDir, `${runId}.baseline-context.stderr.txt`);
  const searchOutput = String(result.stdout ?? "").slice(0, BASELINE_SEARCH_MAX_CHARS);
  const searchCommand = {
    id: "harness_baseline_search",
    command,
    category: "shell_search",
    aggregated_output: searchOutput,
    exit_code: result.exitCode,
    status: preludeStatus.allowed ? preludeStatus.status : result.status,
  };
  const commands = [searchCommand, ...readCommands];
  const publicPrelude = {
    kind: "baseline_local_context",
    status: preludeStatus.status,
    process_status: result.status,
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error,
    warning_count: preludeStatus.warning_lines.length,
    warning_lines: preludeStatus.warning_lines.slice(0, 12),
    wall_ms: wallMs,
    context_path: contextPath,
    stderr_path: stderrPath,
    query_terms: terms,
    search_result_count: matches.length,
    selected_files: selectedFiles.map((entry) => ({
      path: entry.path,
      score: entry.score,
      matches: entry.matches.length,
      distinct_terms: entry.termHits.size,
    })),
    commands: commands.map((entry) => ({
      id: entry.id,
      command: entry.command,
      category: entry.category,
      status: entry.status,
      exit_code: entry.exit_code,
      output_chars: String(entry.aggregated_output ?? "").length,
    })),
  };
  await writeFile(
    contextPath,
    `${JSON.stringify(
      {
        ...publicPrelude,
        context_text: contextText,
        commands,
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
  await writeFile(stderrPath, result.stderr, "utf8");
  return {
    public: publicPrelude,
    contextText,
    commands,
  };
}

async function runCodeStoryPacketPrelude(opts, run, repoConfig, outDir, runId, codestoryCli) {
  const args = packetCommandArgs(repoConfig, run.task, opts);
  const extraProbes = packetCommandExtraProbes(run.task, opts);
  const command = displayCommand(codestoryCli, args);
  const stdoutPath = path.join(outDir, `${runId}.codestory-packet.stdout.json`);
  const stderrPath = path.join(outDir, `${runId}.codestory-packet.stderr.txt`);
  const started = performance.now();
  const result = await runProcess(codestoryCli, args, {
    cwd: repoConfig.path,
    env: benchmarkChildEnv(process.env),
    timeoutMs: opts.timeoutMs,
    timeoutMessage: `CodeStory packet prelude timed out after ${opts.timeoutMs}ms.`,
  });
  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  await writeFile(stdoutPath, result.stdout, "utf8");
  await writeFile(stderrPath, result.stderr, "utf8");

  let packet = null;
  let parseError = null;
  if (result.status === "pass") {
    try {
      packet = JSON.parse(result.stdout);
    } catch (error) {
      parseError = error.message;
    }
  }
  const manifestQuality = packetManifestQualitySummary(packet, run.task);
  const publicPrelude = preludePublicFields({
    command,
    args,
    status: result.status === "pass" && !parseError ? "pass" : "fail",
    process_status: result.status,
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error ?? parseError,
    wall_ms: wallMs,
    stdout_path: stdoutPath,
    stderr_path: stderrPath,
    stdout_bytes: Buffer.byteLength(result.stdout, "utf8"),
    stderr_bytes: Buffer.byteLength(result.stderr, "utf8"),
    packet_parse_error: parseError,
    packet_sufficiency_status: packet?.sufficiency?.status ?? null,
    packet_sufficiency: packetSufficiencyTelemetry(packet, manifestQuality),
    packet_citation_count: Array.isArray(packet?.answer?.citations)
      ? packet.answer.citations.length
      : null,
    packet_avoid_opening_count: packet ? packetAvoidOpeningRawPaths(packet).length : null,
    packet_latency: packetLatencyTelemetry(packet, wallMs),
    packet_composition: packetComposition(packet, run.task),
    packet_manifest_quality: manifestQuality,
    packet_extra_probe_count: extraProbes.length,
    packet_extra_probe_strategy: packetExtraProbeStrategy(extraProbes),
  });
  return {
    public: publicPrelude,
    packet,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

async function recordedHarnessPreludeEvents(result, runDir) {
  const events = [];
  const prelude = result.codestory_harness_prelude ?? null;
  if (prelude) {
    let stdout = "";
    const stdoutPath = prelude.stdout_path
      ? path.isAbsolute(prelude.stdout_path)
        ? prelude.stdout_path
        : path.resolve(runDir, prelude.stdout_path)
      : null;
    if (stdoutPath && existsSync(stdoutPath)) {
      stdout = await readFile(stdoutPath, "utf8");
    }
    events.push(...harnessPacketPreludeEvents(prelude, stdout));
  }
  const baselinePrelude = result.baseline_harness_prelude ?? null;
  if (baselinePrelude?.context_path) {
    const contextPath = path.isAbsolute(baselinePrelude.context_path)
      ? baselinePrelude.context_path
      : path.resolve(runDir, baselinePrelude.context_path);
    if (existsSync(contextPath)) {
      const payload = JSON.parse(await readFile(contextPath, "utf8"));
      events.push(...harnessBaselinePreludeEvents(baselinePrelude, payload.commands ?? []));
    }
  }
  return events;
}

async function runOne(opts, run, outDir) {
  const repoConfig = ALL_REPOS[run.repo];
  const runId = benchmarkRunId([
    run.repo,
    ...(run.task ? [run.task.id] : []),
    run.arm,
    String(run.repeat).padStart(2, "0"),
  ]);
  const env = run.arm === "with_codestory" ? benchmarkChildEnv(process.env) : { ...process.env };
  if (run.arm === "with_codestory") {
    const codestoryCli = resolveCodeStoryCli(opts);
    env.CODESTORY_CLI = path.isAbsolute(codestoryCli) || /[\\/]/.test(codestoryCli)
      ? path.resolve(codestoryCli)
      : codestoryCli;
  } else {
    delete env.CODESTORY_CLI;
  }
  const baselinePrelude =
    run.arm === "without_codestory"
      ? await runBaselinePrelude(opts, run, repoConfig, outDir, runId)
      : null;
  const codestoryPrelude =
    run.arm === "with_codestory"
      ? await runCodeStoryPacketPrelude(opts, run, repoConfig, outDir, runId, env.CODESTORY_CLI)
      : null;
  const prompt = composePrompt(run.repo, repoConfig, run.arm, run.task, {
    baselinePrelude,
    codestoryPrelude,
  });
  const { command, args, stdin, killProcessTree } = runnerCommand(opts, repoConfig.path, prompt);
  const started = performance.now();
  const preludeFailure = [baselinePrelude, codestoryPrelude].find(
    (prelude) => prelude && !preludeAllowsAgentRun(prelude.public, opts),
  );
  const shouldRunAgent = preludeFailure == null;
  const result = shouldRunAgent
    ? await runProcess(command, args, {
        cwd: repoConfig.path,
        env,
        stdin,
        timeoutMs: opts.timeoutMs,
        timeoutMessage: `Benchmark runner timed out after ${opts.timeoutMs}ms.`,
        forceKillAfterMs: 5000,
        killProcessTree,
      })
    : {
        status: "fail",
        exitCode: null,
        signal: null,
        stdout: "",
        stderr: `${preludeFailure.public.kind} prelude failed; skipped agent runner. See ${preludeFailure.public.stderr_path ?? preludeFailure.public.context_path}.`,
        error: preludeFailure.public.error,
        timedOut: false,
      };

  const runnerWallMs = shouldRunAgent ? Math.round((performance.now() - started) * 1000) / 1000 : 0;
  const preludeWallMs = (codestoryPrelude?.public.wall_ms ?? 0) + (baselinePrelude?.public.wall_ms ?? 0);
  const wallMs = Math.round((runnerWallMs + preludeWallMs) * 1000) / 1000;
  const stdoutPath = path.join(outDir, `${runId}.stdout.jsonl`);
  const stderrPath = path.join(outDir, `${runId}.stderr.txt`);
  await writeFile(stdoutPath, result.stdout, "utf8");
  await writeFile(stderrPath, result.stderr, "utf8");

  const { parsed, malformed } = parseJsonLines(result.stdout);
  const analysisEvents = [
    ...harnessBaselinePreludeEvents(baselinePrelude?.public, baselinePrelude?.commands),
    ...harnessPacketPreludeEvents(codestoryPrelude?.public, codestoryPrelude?.stdout),
    ...parsed,
  ];
  const usage = extractUsage(parsed);
  const codexToolCalls = parsed.filter(isToolCallStartEvent).length;
  const toolCalls = analysisEvents.filter(isToolCallStartEvent).length;
  const analysis = analyzeTranscript(analysisEvents, repoConfig.path);
  const provenance = await repoProvenance(repoConfig);
  const packetFirstRequired = run.arm === "with_codestory";
  const packetFirstPass =
    !packetFirstRequired || Boolean(analysis.packet_was_first_context_command);
  const quality = scoreQuality(analysisEvents, run.task);
  const cacheProvenance = run.arm === "with_codestory"
    ? await codestoryCacheProvenance(opts, repoConfig, {
        codestory_index_commands_observed: analysis.codestory_index_commands_observed,
        indexing_in_timed_run: analysis.codestory_index_commands_observed > 0,
        cache_prepared: opts.cachePreparationByRepo?.has(run.repo) ?? false,
        cache_preparation: opts.cachePreparationByRepo?.get(run.repo) ?? null,
        transport_mode: "agent_runner",
      })
    : null;
  const benchmarkContract = benchmarkContractForRun(opts, run, env);

  const output = {
    benchmark_run_id: runId,
    repo: run.repo,
    task_id: run.task?.id ?? null,
    task_name: run.task?.name ?? null,
    task_class: run.task?.task_class ?? null,
    task_manifest_path: run.task?.manifest_path ?? null,
    task_manifest_snapshot: taskSnapshotForResult(run.task),
    arm: run.arm,
    repeat: run.repeat,
    runner: opts.runner,
    model: opts.model,
    sandbox: opts.sandbox,
    command,
    args,
    stdin: stdin == null ? null : "<prompt>",
    codestory_cli_env: run.arm === "with_codestory" ? env.CODESTORY_CLI : null,
    repo_path: repoConfig.path,
    repo_provenance: provenance,
    codestory_cache_provenance: cacheProvenance,
    benchmark_contract: benchmarkContract,
    promotion_eligible: benchmarkContract.promotion_eligible,
    status: result.timedOut ? "timeout" : result.exitCode === 0 ? "pass" : "fail",
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error,
    wall_ms: wallMs,
    agent_runner_wall_ms: runnerWallMs,
    baseline_harness_prelude: baselinePrelude?.public ?? null,
    codestory_harness_prelude: codestoryPrelude?.public ?? null,
    usage,
    estimated_cost_usd: estimateCost(usage),
    tool_calls_observed: toolCalls,
    codex_tool_calls_observed: codexToolCalls,
    transcript_analysis: analysis,
    packet_first_required: packetFirstRequired,
    packet_first_pass: packetFirstPass,
    quality,
    event_types: eventTypeCounts(analysisEvents),
    json_events: parsed.length,
    analysis_events: analysisEvents.length,
    malformed_stdout_lines: malformed.length,
    stdout_path: stdoutPath,
    stderr_path: stderrPath,
  };
  return {
    ...output,
    resource_accounting: resourceAccountingForResult(output),
  };
}

function preludeAllowsAgentRun(publicPrelude, opts = {}) {
  return publicPrelude?.status === "pass" || (!opts.publishable && publicPrelude?.status === "pass_with_warnings");
}

async function gitOutput(args, cwd, timeoutMs = 10_000) {
  const result = await runProcess("git", args, { cwd, timeoutMs });
  if (result.exitCode !== 0) {
    return null;
  }
  return result.stdout.trim();
}

async function repoProvenance(config) {
  const checkoutPath = path.resolve(config.checkout_path ?? config.path);
  const statusShort = await gitOutput(["-C", checkoutPath, "status", "--short"], repoRoot);
  return {
    resolved_path: config.path,
    checkout_path: checkoutPath,
    workspace_root: config.workspace_root ?? null,
    manifest: {
      url: config.manifest_url ?? null,
      ref: config.manifest_ref ?? null,
      workspace_root: config.manifest_workspace_root ?? null,
      checkout_path: config.manifest_checkout_path ?? null,
    },
    configured: {
      url: config.url ?? null,
      ref: config.ref ?? null,
      languages: config.languages ?? [],
    },
    manifest_overridden_by_builtin: Boolean(config.manifest_overridden_by_builtin),
    git_head: await gitOutput(["-C", checkoutPath, "rev-parse", "HEAD"], repoRoot),
    git_origin: redactUrlForDisplay(
      await gitOutput(["-C", checkoutPath, "remote", "get-url", "origin"], repoRoot),
    ),
    git_dirty: statusShort == null ? null : statusShort.length > 0,
    git_status_short: statusShort,
  };
}

function trimTail(text, maxChars = 4000) {
  const value = String(text ?? "");
  return value.length <= maxChars ? value : value.slice(value.length - maxChars);
}

function doctorSnapshotFromOutput(result, output, parseError, wallMs) {
  const retrieval = output?.retrieval ?? null;
  const locality = semanticRuntimeLocality(output);
  return {
    status: result.status === "pass" && !parseError ? "pass" : result.status,
    exit_code: result.exitCode,
    timed_out: Boolean(result.timedOut),
    error: result.error ?? parseError ?? null,
    wall_ms: wallMs,
    project: output?.project ?? null,
    storage_path: output?.storage_path ?? null,
    indexed: output?.indexed ?? null,
    freshness_status: output?.freshness?.status ?? null,
    changed_file_count: output?.freshness?.changed_file_count ?? null,
    new_file_count: output?.freshness?.new_file_count ?? null,
    removed_file_count: output?.freshness?.removed_file_count ?? null,
    semantic_ready: retrieval?.semantic_ready ?? null,
    semantic_backend: semanticBackendName(retrieval),
    semantic_doc_count: retrieval?.semantic_doc_count ?? null,
    embedding_model: retrieval?.embedding_model ?? retrieval?.current_embedding?.model_id ?? null,
    local_only: locality.local_only,
    locality_kind: locality.locality_kind,
    locality_evidence: locality.locality_evidence,
    stats: output?.stats ?? null,
    stdout_tail: result.status === "pass" ? null : trimTail(result.stdout),
    stderr_tail: result.status === "pass" ? null : trimTail(result.stderr),
  };
}

function retrievalStatusSnapshotFromOutput(result, output, parseError, wallMs) {
  return {
    status: result.status === "pass" && !parseError ? "pass" : result.status,
    exit_code: result.exitCode,
    timed_out: Boolean(result.timedOut),
    error: result.error ?? parseError ?? null,
    wall_ms: wallMs,
    retrieval_mode: output?.retrieval_mode ?? null,
    degraded_reason: output?.degraded_reason ?? null,
    manifest_embedding_backend: output?.manifest?.embedding_backend ?? null,
    manifest_embedding_dim: output?.manifest?.embedding_dim ?? null,
    sidecar_generation: output?.manifest?.sidecar_generation ?? null,
    qdrant_collection: output?.manifest?.qdrant_collection ?? null,
    zoekt_capabilities: output?.zoekt?.capabilities ?? null,
    qdrant_capabilities: output?.qdrant?.capabilities ?? null,
    scip_capabilities: output?.scip?.capabilities ?? null,
    stdout_tail: result.status === "pass" ? null : trimTail(result.stdout),
    stderr_tail: result.status === "pass" ? null : trimTail(result.stderr),
  };
}

async function codestoryDoctorSnapshot(codestoryCli, project, timeoutMs) {
  const started = performance.now();
  const result = await runProcess(
    codestoryCli,
    ["doctor", "--project", project, "--format", "json"],
    { timeoutMs, env: benchmarkChildEnv(process.env) },
  );
  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  let output = null;
  let parseError = null;
  if (result.status === "pass") {
    try {
      output = JSON.parse(result.stdout);
    } catch (error) {
      parseError = error.message;
    }
  }
  return doctorSnapshotFromOutput(result, output, parseError, wallMs);
}

async function codestoryRetrievalStatusSnapshot(codestoryCli, project, timeoutMs) {
  const started = performance.now();
  const result = await runProcess(
    codestoryCli,
    ["retrieval", "status", "--project", project, "--format", "json"],
    { timeoutMs, env: benchmarkChildEnv(process.env) },
  );
  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  let output = null;
  let parseError = null;
  if (result.status === "pass") {
    try {
      output = JSON.parse(result.stdout);
    } catch (error) {
      parseError = error.message;
    }
  }
  return retrievalStatusSnapshotFromOutput(result, output, parseError, wallMs);
}

function cacheNeedsPreparation(snapshot) {
  if (snapshot.status !== "pass") {
    return true;
  }
  if (snapshot.indexed !== true) {
    return true;
  }
  if (snapshot.freshness_status !== "fresh") {
    return true;
  }
  return snapshot.semantic_ready !== true;
}

function cachePreparationAction(snapshot) {
  return cacheNeedsPreparation(snapshot) ? "retrieval-index-auto" : "already-ready";
}

function compactCachePreparation(preparation) {
  if (!preparation) {
    return null;
  }
  return {
    repo: preparation.repo,
    action: preparation.action,
    preparation_wall_ms: preparation.preparation_wall_ms ?? null,
    index_status: preparation.index_status ?? null,
    index_exit_code: preparation.index_exit_code ?? null,
    index_wall_ms: preparation.index_wall_ms ?? null,
    retrieval_contract: preparation.retrieval_contract ?? null,
    retrieval_index_status: preparation.retrieval_index_status ?? null,
    retrieval_index_exit_code: preparation.retrieval_index_exit_code ?? null,
    retrieval_index_wall_ms: preparation.retrieval_index_wall_ms ?? null,
    retrieval_mode: preparation.retrieval_status?.retrieval_mode ?? null,
    retrieval_degraded_reason: preparation.retrieval_status?.degraded_reason ?? null,
    sidecar_generation: preparation.retrieval_status?.sidecar_generation ?? null,
    manifest_embedding_backend: preparation.retrieval_status?.manifest_embedding_backend ?? null,
    before_freshness_status: preparation.before?.freshness_status ?? null,
    after_freshness_status: preparation.after?.freshness_status ?? null,
    before_semantic_ready: preparation.before?.semantic_ready ?? null,
    after_semantic_ready: preparation.after?.semantic_ready ?? null,
    before_semantic_doc_count: preparation.before?.semantic_doc_count ?? null,
    after_semantic_doc_count: preparation.after?.semantic_doc_count ?? null,
  };
}

async function prepareCodeStoryCaches(opts, tasks) {
  if (!opts.arms.includes("with_codestory")) {
    return [];
  }
  const repoNames = [...new Set(tasks.map((task) => task.repo))];
  const codestoryCli = resolveCodeStoryCli(opts);
  if (repoNames.length > 1 && opts.prepareCodestoryJobs > 1) {
    console.log(
      `preparing CodeStory caches for ${repoNames.length} repos with --prepare-codestory-jobs ${opts.prepareCodestoryJobs}`,
    );
  }
  return await parallelMap(repoNames, opts.prepareCodestoryJobs, async (repo) => {
    const config = ALL_REPOS[repo];
    if (!config || !existsSync(config.path)) {
      return {
        repo,
        project: config?.path ?? null,
        action: "skipped-missing-repo",
      };
    }

    console.log(`preparing CodeStory cache for ${repo}`);
    const preparationStarted = performance.now();
    const before = await codestoryDoctorSnapshot(codestoryCli, config.path, 60_000);
    const preparation = {
      repo,
      project: config.path,
      codestory_cli: path.resolve(codestoryCli),
      action: cachePreparationAction(before),
      preparation_wall_ms: null,
      before,
      index_status: null,
      index_exit_code: null,
      index_wall_ms: 0,
      index_stdout_tail: null,
      index_stderr_tail: null,
      retrieval_status: null,
      retrieval_index_stdout_tail: null,
      retrieval_index_stderr_tail: null,
      after: before,
    };

    preparation.retrieval_contract = retrievalContractSummary(benchmarkChildEnv(process.env));
    if (shouldPrepareRetrievalIndex(process.env)) {
      const retrievalStarted = performance.now();
      const retrievalIndex = await runProcess(
        codestoryCli,
        ["retrieval", "index", "--project", config.path, "--refresh", "auto"],
        {
          env: benchmarkChildEnv(process.env),
          timeoutMs: opts.prepareCodestoryTimeoutMs,
          timeoutMessage: `retrieval index timed out after ${opts.prepareCodestoryTimeoutMs}ms.`,
        },
      );
      preparation.retrieval_index_status = retrievalIndex.status;
      preparation.retrieval_index_exit_code = retrievalIndex.exitCode;
      preparation.retrieval_index_wall_ms =
        Math.round((performance.now() - retrievalStarted) * 1000) / 1000;
      preparation.retrieval_index_stdout_tail = trimTail(retrievalIndex.stdout);
      preparation.retrieval_index_stderr_tail = trimTail(retrievalIndex.stderr);
      if (retrievalIndex.status !== "pass") {
        throw new Error(
          `mandatory retrieval index failed for ${repo}: ${trimTail(retrievalIndex.stderr || retrievalIndex.stdout)}`,
        );
      }
      preparation.after = await codestoryDoctorSnapshot(codestoryCli, config.path, 60_000);
      preparation.retrieval_status = await codestoryRetrievalStatusSnapshot(
        codestoryCli,
        config.path,
        60_000,
      );
      if (preparation.retrieval_status.retrieval_mode !== "full") {
        throw new Error(
          `mandatory retrieval index for ${repo} did not reach full mode: ${preparation.retrieval_status.retrieval_mode ?? "unknown"} ${preparation.retrieval_status.degraded_reason ?? ""}`.trim(),
        );
      }
    }

    preparation.preparation_wall_ms =
      Math.round((performance.now() - preparationStarted) * 1000) / 1000;
    return preparation;
  });
}

function semanticBackendName(retrieval) {
  if (!retrieval || typeof retrieval !== "object") {
    return "unknown";
  }
  return (
    retrieval.current_embedding?.backend ??
    retrieval.stored_embedding?.embedding_backend ??
    (retrieval.semantic_ready ? "unknown" : "symbolic-only")
  );
}

function doctorEnvironmentValue(output, name) {
  const row = (output?.environment ?? []).find((item) => item?.name === name);
  if (!row || row.status !== "ok") {
    return null;
  }
  const match = String(row.message ?? "").match(/`([^`]*)`/);
  return match ? match[1] : null;
}

function isLoopbackUrl(value) {
  try {
    const url = new URL(value);
    const hostname = url.hostname.toLowerCase();
    return (
      hostname === "localhost" ||
      hostname === "127.0.0.1" ||
      hostname === "::1" ||
      hostname.endsWith(".localhost")
    );
  } catch {
    return false;
  }
}

function semanticRuntimeLocality(output) {
  const retrieval = output?.retrieval ?? {};
  const backend = semanticBackendName(retrieval);
  if (backend === "symbolic-only") {
    return {
      local_only: true,
      locality_kind: "no_semantic_runtime",
      locality_evidence: "semantic retrieval unavailable; no embedding runtime was used",
    };
  }
  if (backend === "llamacpp") {
    const endpoint = doctorEnvironmentValue(output, "CODESTORY_EMBED_LLAMACPP_URL");
    if (!endpoint) {
      return {
        local_only: null,
        locality_kind: "unknown_llamacpp_endpoint",
        locality_evidence: "llama.cpp backend did not expose a configured endpoint",
      };
    }
    const loopback = isLoopbackUrl(endpoint);
    return {
      local_only: loopback,
      locality_kind: loopback ? "loopback_endpoint" : "remote_endpoint",
      locality_evidence: "semantic backend is llama.cpp",
    };
  }
  if (backend === "hash" || backend === "lexical") {
    return {
      local_only: true,
      locality_kind: "local_deterministic_backend",
      locality_evidence: `semantic backend is ${backend}`,
    };
  }
  return {
    local_only: null,
    locality_kind: "unknown_backend",
    locality_evidence: `semantic backend is ${backend}`,
  };
}

function cachePolicyForRun(observations = {}) {
  if (observations.indexing_in_timed_run) {
    return "timed-run-indexed-cache";
  }
  return observations.cache_prepared ? "prepared-sidecar-cache-read-only" : "unprepared-cache-blocked";
}

function cachePreparationForRepo(opts, repoName) {
  const preparation = opts.cachePreparationByRepo;
  if (preparation instanceof Map) {
    return preparation.get(repoName) ?? null;
  }
  if (Array.isArray(preparation)) {
    return preparation.find((row) => row?.repo === repoName) ?? null;
  }
  return null;
}

function packetRuntimeCacheObservations(opts, repoName, transportMode) {
  const cachePreparation = cachePreparationForRepo(opts, repoName);
  return {
    codestory_index_commands_observed: 0,
    indexing_in_timed_run: false,
    cache_prepared: Boolean(cachePreparation),
    cache_preparation: cachePreparation,
    transport_mode: transportMode,
  };
}

async function codestoryCacheProvenance(opts, config, observations = {}) {
  let codestoryCli;
  try {
    codestoryCli = resolveCodeStoryCli(opts);
  } catch (error) {
    return {
      codestory_cli: null,
      cache_policy: cachePolicyForRun(observations),
      indexing_in_timed_run: observations.indexing_in_timed_run ?? null,
      codestory_index_commands_observed: observations.codestory_index_commands_observed ?? null,
      transport_mode: observations.transport_mode ?? null,
      doctor_status: "error",
      doctor_error: error.message,
    };
  }

  const doctor = await codestoryDoctorSnapshot(
    codestoryCli,
    config.path,
    Math.min(opts.timeoutMs ?? 600_000, 60_000),
  );
  const retrievalStatus = observations.cache_preparation?.retrieval_status ??
    await codestoryRetrievalStatusSnapshot(
      codestoryCli,
      config.path,
      Math.min(opts.timeoutMs ?? 600_000, 60_000),
    );
  return {
    codestory_cli: path.resolve(codestoryCli),
    project: doctor.project ?? config.path,
    storage_path: doctor.storage_path ?? null,
    indexed: doctor.indexed ?? null,
    freshness_status: doctor.freshness_status ?? null,
    semantic_ready: doctor.semantic_ready ?? null,
    semantic_backend: doctor.semantic_backend ?? null,
    semantic_doc_count: doctor.semantic_doc_count ?? null,
    embedding_model: doctor.embedding_model ?? null,
    local_only: doctor.local_only ?? null,
    locality_kind: doctor.locality_kind ?? null,
    locality_evidence: doctor.locality_evidence ?? null,
    cache_policy: cachePolicyForRun(observations),
    indexing_in_timed_run: observations.indexing_in_timed_run ?? null,
    codestory_index_commands_observed: observations.codestory_index_commands_observed ?? null,
    cache_preparation: compactCachePreparation(observations.cache_preparation),
    transport_mode: observations.transport_mode ?? null,
    retrieval_status: retrievalStatus,
    retrieval_mode: retrievalStatus.retrieval_mode ?? null,
    sidecar_generation: retrievalStatus.sidecar_generation ?? null,
    manifest_embedding_backend: retrievalStatus.manifest_embedding_backend ?? null,
    doctor_status: doctor.status,
    doctor_exit_code: doctor.exit_code,
    doctor_error: doctor.error,
  };
}

async function loadTaskForResult(result, opts, cache) {
  if (result.task_manifest_snapshot && typeof result.task_manifest_snapshot === "object") {
    return result.task_manifest_snapshot;
  }
  const manifestPath = result.task_manifest_path ? path.resolve(result.task_manifest_path) : null;
  if (!manifestPath || !existsSync(manifestPath)) {
    return null;
  }
  if (!cache.has(manifestPath)) {
    const raw = JSON.parse(await readFile(manifestPath, "utf8"));
    cache.set(manifestPath, normalizeManifestTask(manifestPath, raw, opts));
  }
  return cache.get(manifestPath);
}

function eventTypeCounts(events) {
  const counts = {};
  for (const event of events) {
    const type = String(event.type ?? event.event ?? "unknown");
    counts[type] = (counts[type] ?? 0) + 1;
  }
  return counts;
}

async function writeJsonlRows(filePath, rows) {
  await writeFile(filePath, `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`, "utf8");
}

async function recomputeRunAnalysis(result, opts, runDir, taskCache) {
  const stdoutPath = result.stdout_path
    ? path.isAbsolute(result.stdout_path)
      ? result.stdout_path
      : path.resolve(runDir, result.stdout_path)
    : null;
  if (!stdoutPath || !existsSync(stdoutPath)) {
    return {
      ...result,
      reanalysis_error: `missing stdout_path: ${result.stdout_path ?? ""}`,
    };
  }

  const { parsed, malformed } = parseJsonLines(await readFile(stdoutPath, "utf8"));
  const analysisEvents = [
    ...(await recordedHarnessPreludeEvents(result, runDir)),
    ...parsed,
  ];
  const task = await loadTaskForResult(result, opts, taskCache);
  const repoConfig = ALL_REPOS[result.repo] ?? null;
  const usage = extractUsage(parsed);
  const analysis = analyzeTranscript(analysisEvents, result.repo_path ?? repoConfig?.path ?? runDir);
  const packetFirstRequired = result.packet_first_required ?? result.arm === "with_codestory";
  const cacheProvenance = result.codestory_cache_provenance ?? (
    repoConfig && result.arm === "with_codestory"
      ? await codestoryCacheProvenance(opts, repoConfig, {
          codestory_index_commands_observed: analysis.codestory_index_commands_observed,
          indexing_in_timed_run: analysis.codestory_index_commands_observed > 0,
          cache_prepared: opts.cachePreparationByRepo?.has(result.repo) ?? false,
          cache_preparation: opts.cachePreparationByRepo?.get(result.repo) ?? null,
          transport_mode: "agent_runner",
        })
      : null
  );
  const output = {
    ...result,
    repo_provenance: result.repo_provenance ?? (repoConfig ? await repoProvenance(repoConfig) : null),
    codestory_cache_provenance: cacheProvenance,
    usage,
    estimated_cost_usd: estimateCost(usage),
    tool_calls_observed: analysisEvents.filter(isToolCallStartEvent).length,
    codex_tool_calls_observed: parsed.filter(isToolCallStartEvent).length,
    transcript_analysis: analysis,
    packet_first_required: packetFirstRequired,
    packet_first_pass:
      !packetFirstRequired || Boolean(analysis.packet_was_first_context_command),
    quality: scoreQuality(analysisEvents, task),
    reanalysis_task_source: result.task_manifest_snapshot ? "snapshot" : task ? "manifest" : null,
    event_types: eventTypeCounts(analysisEvents),
    json_events: parsed.length,
    analysis_events: analysisEvents.length,
    malformed_stdout_lines: malformed.length,
    reanalyzed_at: new Date().toISOString(),
  };
  return {
    ...output,
    resource_accounting: resourceAccountingForResult(output),
  };
}

async function reanalyzeAgentRunDirectory(opts) {
  const runDir = path.resolve(opts.reanalyzeDir);
  const runsPath = path.join(runDir, "runs.jsonl");
  if (!existsSync(runsPath)) {
    throw new Error(`--reanalyze-dir must contain runs.jsonl: ${runDir}`);
  }
  const originalSummaryPath = path.join(runDir, "summary.json");
  const originalSummary = existsSync(originalSummaryPath)
    ? JSON.parse(await readFile(originalSummaryPath, "utf8"))
    : {};
  const rows = (await readFile(runsPath, "utf8"))
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));

  const taskCache = new Map();
  const reanalyzed = [];
  for (const row of rows) {
    reanalyzed.push(await recomputeRunAnalysis(row, opts, runDir, taskCache));
  }

  const summary = summarizeRuns(reanalyzed);
  const costAccounting = summarizeCostAccounting(reanalyzed);
  const summaryOpts = {
    ...opts,
    runner: originalSummary.runner ?? opts.runner,
    model: originalSummary.model ?? opts.model,
    sandbox: originalSummary.sandbox ?? opts.sandbox,
  };
  const payload = {
    ...originalSummary,
    generated_at: new Date().toISOString(),
    reanalyzed_from: runsPath,
    publishable: Boolean(opts.publishable || originalSummary.publishable),
    max_source_reads_after_packet: opts.maxSourceReadsAfterPacket,
    output_dir: runDir,
    summary,
    cost_accounting: costAccounting,
  };
  await writeFile(
    path.join(runDir, "reanalyzed-runs.jsonl"),
    `${reanalyzed.map((row) => JSON.stringify(row)).join("\n")}\n`,
    "utf8",
  );
  await writeFile(path.join(runDir, "reanalyzed-summary.json"), `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  await writeFile(
    path.join(runDir, "reanalyzed-summary.md"),
    markdownSummary(summary, summaryOpts, costAccounting),
    "utf8",
  );
  if (opts.publishable) {
    const blockers = agentPublishableBlockers(reanalyzed, opts);
    if (blockers.length) {
      console.error("--publishable failed for reanalyzed runs.");
      for (const blocker of blockers) {
        console.error(formatAgentPublishableBlocker(blocker));
      }
      process.exitCode = 1;
    }
  }
  console.log(`reanalyzed ${rows.length} runs in ${runDir}`);
}

function formatAgentPublishableBlocker(blocker) {
  const result = blocker.result;
  const category = blocker.category ? `${blocker.category}: ` : "";
  return `  ${result.repo} ${result.task_id ?? ""} ${result.arm} repeat ${result.repeat}: ${category}${blocker.reasons.join("; ")}; total_tokens=${result.usage?.total_tokens ?? ""} packet_first=${result.packet_first_pass ?? ""} quality=${result.quality?.pass ?? ""}`;
}

function resolveCodeStoryCli(opts, exists = existsSync) {
  if (opts.codestoryCli) {
    return opts.codestoryCli;
  }
  const releaseCandidate = path.join(
    repoRoot,
    "target",
    "release",
    process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli",
  );
  if (exists(releaseCandidate)) {
    return releaseCandidate;
  }
  throw new Error("No codestory-cli found. Pass --codestory-cli, set CODESTORY_CLI, or build the release binary.");
}

function packetPayloadText(packet) {
  if (!packet || typeof packet !== "object") {
    return String(packet ?? "");
  }
  const chunks = [];
  chunks.push(packetAnswerText(packet));
  for (const citation of packet.answer?.citations ?? []) {
    chunks.push(
      [
        citation.display_name,
        citation.file_path,
        citation.line == null ? null : String(citation.line),
      ]
        .filter(Boolean)
        .join(" "),
    );
  }
  for (const claim of packet.sufficiency?.covered_claims ?? []) {
    chunks.push(claim.claim);
  }
  for (const path of packetAvoidOpeningRawPaths(packet)) {
    chunks.push(path);
  }
  return chunks.filter(Boolean).join("\n");
}

function packetAnswerText(packet) {
  if (!packet || typeof packet !== "object") {
    return String(packet ?? "");
  }
  const chunks = [];
  if (packet.answer?.summary) {
    chunks.push(packet.answer.summary);
  }
  for (const section of packet.answer?.sections ?? []) {
    if (section.title) {
      chunks.push(section.title);
    }
    for (const block of section.blocks ?? []) {
      if (block.markdown) {
        chunks.push(block.markdown);
      }
    }
  }
  return chunks.join("\n");
}

function packetComposition(packet, task) {
  if (!packet || typeof packet !== "object" || !task) {
    return null;
  }
  const expectedFiles = [
    ...new Set((task.expected_files ?? []).map(String).map((value) => value.trim()).filter(Boolean)),
  ];
  const expectedVerificationFiles = [
    ...new Set(
      (task.expected_verification_files ?? []).map(String).map((value) => value.trim()).filter(Boolean),
    ),
  ];
  const citationPaths = (packet.answer?.citations ?? [])
    .map((citation, index) => ({
      source: "answer.citations",
      path: citation.file_path,
      rank: index + 1,
      display_name: citation.display_name ?? null,
      line: citation.line ?? null,
    }))
    .filter((entry) => entry.path);
  const avoidOpeningPaths = packetAvoidOpeningRawPaths(packet)
    .map((pathValue, index) => ({
      source: "sufficiency.avoid_opening_paths",
      path: pathValue,
      rank: index + 1,
      display_name: null,
      line: null,
    }))
    .filter((entry) => entry.path);
  const answerText = packetAnswerText(packet);
  const structuredJson = JSON.stringify(packet);
  const files = expectedFiles.map((expectedFile) => {
    const citationSurfaces = pathSurfacesForExpected(citationPaths, expectedFile);
    const avoidOpeningSurfaces = pathSurfacesForExpected(avoidOpeningPaths, expectedFile);
    const answerTextMentioned = anchorMatched(answerText, expectedFile);
    const structuredJsonMentioned = anchorMatched(structuredJson, expectedFile);
    const citationBackedFound =
      citationSurfaces.length > 0 || avoidOpeningSurfaces.length > 0;
    const answerSurfaceFound = citationBackedFound || answerTextMentioned;
    const structuredFound = answerSurfaceFound || structuredJsonMentioned;
    return {
      expected_file: expectedFile,
      packet_boundary: packetLossBoundary({
        cited: citationSurfaces.length > 0,
        avoidOpening: avoidOpeningSurfaces.length > 0,
        answerTextMentioned,
        structuredJsonMentioned,
      }),
      citation_backed_found: citationBackedFound,
      answer_surface_found: answerSurfaceFound,
      structured_found: structuredFound,
      cited: citationSurfaces.length > 0,
      avoid_opening: avoidOpeningSurfaces.length > 0,
      answer_text_mentioned: answerTextMentioned,
      structured_json_mentioned: structuredJsonMentioned,
      surfaces: [
        ...citationSurfaces,
        ...avoidOpeningSurfaces,
        ...(answerTextMentioned
          ? [{ source: "answer.text", path: expectedFile, rank: null, display_name: null, line: null }]
          : []),
        ...(structuredJsonMentioned && !answerSurfaceFound
          ? [{ source: "packet.structured_json", path: expectedFile, rank: null, display_name: null, line: null }]
          : []),
      ],
    };
  });
  const summary = summarizePacketComposition(files);
  const verificationFiles = expectedVerificationFiles.map((expectedFile) =>
    packetFileComposition(packet, expectedFile, {
      citationPaths,
      avoidOpeningPaths,
      answerText,
      structuredJson,
    }),
  );
  const verificationSummary = summarizePacketComposition(verificationFiles);
  return {
    expected_file_count: expectedFiles.length,
    ...summary,
    files,
    expected_verification_file_count: expectedVerificationFiles.length,
    verification_summary: verificationSummary,
    verification_files: verificationFiles,
  };
}

function packetFileComposition(
  packet,
  expectedFile,
  { citationPaths, avoidOpeningPaths, answerText, structuredJson },
) {
  const citationSurfaces = pathSurfacesForExpected(citationPaths, expectedFile);
  const avoidOpeningSurfaces = pathSurfacesForExpected(avoidOpeningPaths, expectedFile);
  const answerTextMentioned = anchorMatched(answerText, expectedFile);
  const structuredJsonMentioned = anchorMatched(structuredJson, expectedFile);
  const citationBackedFound =
    citationSurfaces.length > 0 || avoidOpeningSurfaces.length > 0;
  const answerSurfaceFound = citationBackedFound || answerTextMentioned;
  const structuredFound = answerSurfaceFound || structuredJsonMentioned;
  return {
    expected_file: expectedFile,
    packet_boundary: packetLossBoundary({
      cited: citationSurfaces.length > 0,
      avoidOpening: avoidOpeningSurfaces.length > 0,
      answerTextMentioned,
      structuredJsonMentioned,
    }),
    citation_backed_found: citationBackedFound,
    answer_surface_found: answerSurfaceFound,
    structured_found: structuredFound,
    cited: citationSurfaces.length > 0,
    avoid_opening: avoidOpeningSurfaces.length > 0,
    answer_text_mentioned: answerTextMentioned,
    structured_json_mentioned: structuredJsonMentioned,
    surfaces: [
      ...citationSurfaces,
      ...avoidOpeningSurfaces,
      ...(answerTextMentioned
        ? [{ source: "answer.text", path: expectedFile, rank: null, display_name: null, line: null }]
        : []),
      ...(structuredJsonMentioned && !answerSurfaceFound
        ? [{ source: "packet.structured_json", path: expectedFile, rank: null, display_name: null, line: null }]
        : []),
    ],
  };
}

function pathSurfacesForExpected(paths, expectedFile) {
  return paths
    .filter((entry) => pathMatchesLike(entry.path, expectedFile))
    .map((entry) => ({
      source: entry.source,
      path: normalizePathLike(entry.path),
      rank: entry.rank,
      display_name: entry.display_name,
      line: entry.line,
    }));
}

function packetLossBoundary({ cited, avoidOpening, answerTextMentioned, structuredJsonMentioned }) {
  if (cited) {
    return "cited_in_answer";
  }
  if (avoidOpening) {
    return "listed_in_avoid_opening";
  }
  if (answerTextMentioned) {
    return "mentioned_in_answer_text";
  }
  if (structuredJsonMentioned) {
    return "present_only_in_structured_json";
  }
  return "absent_from_packet";
}

const PACKET_COMPOSITION_WEIGHTS = {
  cited: 1,
  avoid_opening: 0.9,
  answer_text_only: 0.25,
};

function packetCompositionFileScore(file) {
  if (file.cited) {
    return PACKET_COMPOSITION_WEIGHTS.cited;
  }
  if (file.avoid_opening) {
    return PACKET_COMPOSITION_WEIGHTS.avoid_opening;
  }
  if (file.answer_text_mentioned && !file.citation_backed_found) {
    return PACKET_COMPOSITION_WEIGHTS.answer_text_only;
  }
  return 0;
}

function summarizePacketComposition(files) {
  const expected = files.length;
  const cited = files.filter((file) => file.cited).length;
  const avoidOpening = files.filter((file) => file.avoid_opening).length;
  const answerText = files.filter(
    (file) => file.answer_text_mentioned && !file.citation_backed_found,
  ).length;
  const citationBacked = files.filter((file) => file.citation_backed_found).length;
  const answerSurface = files.filter((file) => file.answer_surface_found).length;
  const structured = files.filter((file) => file.structured_found).length;
  const compositionScore = expected
    ? files.reduce((sum, file) => sum + packetCompositionFileScore(file), 0) / expected
    : null;
  const boundaryCounts = {};
  for (const file of files) {
    boundaryCounts[file.packet_boundary] = (boundaryCounts[file.packet_boundary] ?? 0) + 1;
  }
  return {
    cited_file_count: cited,
    avoid_opening_file_count: avoidOpening,
    answer_text_file_count: answerText,
    citation_backed_file_count: citationBacked,
    answer_surface_file_count: answerSurface,
    structured_file_count: structured,
    absent_file_count: expected - structured,
    citation_recall: expected ? cited / expected : null,
    citation_backed_recall: expected ? citationBacked / expected : null,
    answer_surface_recall: expected ? answerSurface / expected : null,
    structured_file_recall: expected ? structured / expected : null,
    composition_score: compositionScore,
    boundary_counts: boundaryCounts,
  };
}

function jsonByteLength(value) {
  return Buffer.byteLength(JSON.stringify(value ?? null), "utf8");
}

function finiteNumber(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

function cappedStringArray(value, limit) {
  return Array.isArray(value)
    ? value
        .map((entry) => String(entry ?? "").trim())
        .filter(Boolean)
        .slice(0, limit)
    : [];
}

function packetRetrievalShadow(packet) {
  return (
    packet?.benchmark_trace?.retrieval_trace?.retrieval_shadow ??
    packet?.answer?.retrieval_trace?.retrieval_shadow ??
    null
  );
}

function packetDiagnosticIsDiagnosticOnly(value) {
  if (!value || typeof value !== "object") {
    return false;
  }
  const classification = String(
    value.classification ?? value.kind ?? value.tier ?? value.status ?? "",
  ).toLowerCase();
  return (
    value.diagnostic_only === true ||
    value.diagnosticOnly === true ||
    classification === "diagnostic_only" ||
    classification === "diagnostic-only"
  );
}

function packetCoverageUnresolvedCounts(packet) {
  const unresolved =
    packet?.coverage_report?.unresolved ??
    packet?.answer?.coverage_report?.unresolved ??
    packet?.sufficiency?.coverage_report?.unresolved ??
    null;
  if (Array.isArray(unresolved)) {
    return {
      total: unresolved.length,
      blocking: unresolved.filter((entry) => !packetDiagnosticIsDiagnosticOnly(entry)).length,
    };
  }
  const number = finiteNumber(unresolved);
  if (number != null) {
    return { total: number, blocking: number };
  }
  if (unresolved && typeof unresolved === "object") {
    if (packetDiagnosticIsDiagnosticOnly(unresolved)) {
      return { total: 1, blocking: 0 };
    }
    const values = Object.values(unresolved);
    return {
      total: values.length,
      blocking: values.filter((entry) => !packetDiagnosticIsDiagnosticOnly(entry)).length,
    };
  }
  return { total: null, blocking: null };
}

function packetShape(packet) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  return {
    packet_bytes: jsonByteLength(packet),
    answer_bytes: jsonByteLength(packet.answer),
    graph_bytes: jsonByteLength(packet.answer?.graphs ?? []),
    retrieval_trace_bytes: jsonByteLength(packet.answer?.retrieval_trace ?? null),
    sections_bytes: jsonByteLength(packet.answer?.sections ?? []),
    citations_count: Array.isArray(packet.answer?.citations) ? packet.answer.citations.length : 0,
    budget_used_output_bytes: packet.budget?.used?.output_bytes ?? null,
    budget_limit_output_bytes: packet.budget?.limits?.max_output_bytes ?? null,
    budget_truncated: packet.budget?.truncated ?? null,
  };
}

function packetSufficiencyTelemetry(packet, quality) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  const status = packet.sufficiency?.status ?? null;
  const qualityPass = quality?.pass ?? null;
  const gaps = cappedStringArray(packet.sufficiency?.gaps, 8);
  const openNext = cappedStringArray(packet.sufficiency?.open_next, 6);
  const followUpCommands = cappedStringArray(packet.sufficiency?.follow_up_commands, 6);
  const retrievalShadow = packetRetrievalShadow(packet);
  const unresolvedCoverage = packetCoverageUnresolvedCounts(packet);
  return {
    status,
    covered_claims_count: packet.sufficiency?.covered_claims?.length ?? 0,
    open_next_count: packet.sufficiency?.open_next?.length ?? 0,
    avoid_opening_count: packetAvoidOpeningRawPaths(packet).length,
    gaps_count: packet.sufficiency?.gaps?.length ?? 0,
    follow_up_commands_count: packet.sufficiency?.follow_up_commands?.length ?? 0,
    gaps,
    open_next: openNext,
    follow_up_commands: followUpCommands,
    retrieval_mode: retrievalShadow?.retrieval_mode ?? null,
    degraded_reason: retrievalShadow?.degraded_reason ?? null,
    unresolved_candidate_count: finiteNumber(retrievalShadow?.unresolved_candidate_count),
    unresolved_candidate_diagnostic_only: packetDiagnosticIsDiagnosticOnly(retrievalShadow),
    coverage_unresolved_count: unresolvedCoverage.total,
    coverage_unresolved_blocking_count: unresolvedCoverage.blocking,
    sufficient_quality_mismatch: status === "sufficient" && qualityPass === false,
  };
}

function packetRetrievalShadowTelemetry(shadow) {
  if (!shadow || typeof shadow !== "object") {
    return null;
  }
  const stages = Array.isArray(shadow.stage_timings) ? shadow.stage_timings : [];
  const cacheHitStages = stages.filter((stage) => stage?.cache_hit === true);
  return {
    retrieval_mode: shadow.retrieval_mode ?? null,
    degraded_reason: shadow.degraded_reason ?? null,
    retrieval_total_ms: finiteNumber(shadow.retrieval_total_ms),
    total_budget_ms: finiteNumber(shadow.total_budget_ms),
    cancel_reason: shadow.cancel_reason ?? null,
    cache_hit: shadow.cache_hit === true,
    stage_count: stages.length,
    cache_hit_stage_count: cacheHitStages.length,
    cache_hit_stages: cacheHitStages
      .map((stage) => String(stage?.stage ?? "").trim())
      .filter(Boolean),
    candidate_count: finiteNumber(shadow.candidate_count),
    resolved_hit_count: finiteNumber(shadow.resolved_hit_count),
    unresolved_candidate_count: finiteNumber(shadow.unresolved_candidate_count),
    error: shadow.error ?? null,
  };
}

function packetTraceField(fields, key) {
  if (!Array.isArray(fields)) {
    return null;
  }
  const found = fields.find((field) => field?.key === key);
  return found ? found.value : null;
}

function packetTraceNumber(fields, key) {
  return finiteNumber(packetTraceField(fields, key));
}

function packetSearchStepTelemetry(steps) {
  return steps
    .map((step, index) => ({ step, index }))
    .filter(({ step }) => String(step?.kind ?? "").toLowerCase() === "search")
    .filter(({ step }) => String(step?.status ?? "").toLowerCase() !== "skipped")
    .map(({ step, index }) => ({
      step_index: index,
      query: packetTraceField(step.input, "query"),
      mode: packetTraceField(step.output, "mode") ?? "unclassified_search",
      duration_ms: finiteNumber(step.duration_ms),
      hits: packetTraceNumber(step.output, "hits"),
      sidecar_query_ms: packetTraceNumber(step.output, "sidecar_query_ms"),
      candidate_resolution_ms: packetTraceNumber(step.output, "candidate_resolution_ms"),
      sidecar_total_ms: packetTraceNumber(step.output, "sidecar_total_ms"),
      sidecar_stage_count: packetTraceNumber(step.output, "sidecar_stage_count"),
      sidecar_stage_total_ms: packetTraceNumber(step.output, "sidecar_stage_total_ms"),
      message: step.message ?? null,
    }));
}

function packetSearchPhaseTotal(searchSteps, mode) {
  const total = searchSteps
    .filter((step) => step.mode === mode)
    .reduce((sum, step) => sum + (finiteNumber(step.duration_ms) ?? 0), 0);
  return Number.isFinite(total) ? total : null;
}

function packetBatchTimings(annotations) {
  if (!Array.isArray(annotations)) {
    return [];
  }
  const pattern = /^(packet_[a-z_]+_batch) total_ms=(\d+) attributed_query_ms=(\d+) overhead_ms=(\d+) queries=(\d+)$/;
  return annotations
    .map((annotation) => pattern.exec(String(annotation ?? "")))
    .filter(Boolean)
    .map((match) => ({
      label: match[1],
      total_ms: Number(match[2]),
      attributed_query_ms: Number(match[3]),
      overhead_ms: Number(match[4]),
      queries: Number(match[5]),
    }));
}

function packetBatchTiming(timings, label, key) {
  return finiteNumber(timings.find((timing) => timing.label === label)?.[key]);
}

function packetNonTracePhaseTimings(annotations) {
  if (!Array.isArray(annotations)) {
    return [];
  }
  const pattern = /^packet_non_trace_phase label=([a-z_]+) duration_ms=(\d+)$/;
  return annotations
    .map((annotation) => pattern.exec(String(annotation ?? "")))
    .filter(Boolean)
    .map((match) => ({
      label: match[1],
      duration_ms: Number(match[2]),
    }));
}

function packetNonTracePhaseTiming(timings, label) {
  return finiteNumber(timings.find((timing) => timing.label === label)?.duration_ms);
}

function packetStdioPhaseTimings(annotations) {
  if (!Array.isArray(annotations)) {
    return [];
  }
  const pattern = /^packet_stdio_phase label=([a-z_]+) duration_ms=(\d+)$/;
  return annotations
    .map((annotation) => pattern.exec(String(annotation ?? "")))
    .filter(Boolean)
    .map((match) => ({
      label: match[1],
      duration_ms: Number(match[2]),
    }));
}

function packetStdioPhaseTiming(timings, label) {
  return finiteNumber(timings.find((timing) => timing.label === label)?.duration_ms);
}

function stdioRequestIdKey(value) {
  return JSON.stringify(value ?? null);
}

function parseStdioServerPhaseLine(line) {
  const match = /^packet_stdio_server_phase request_id=(\S+) label=([a-z_]+) duration_ms=(\d+)$/.exec(String(line ?? ""));
  if (!match) {
    return null;
  }
  let requestId = match[1];
  try {
    requestId = stdioRequestIdKey(JSON.parse(requestId));
  } catch {
    // ponytail: raw key is fine if a future diagnostic id is not JSON.
  }
  return {
    request_id: requestId,
    label: match[2],
    duration_ms: Number(match[3]),
  };
}

function stdioServerPhaseTiming(timings, label) {
  return finiteNumber(timings.find((timing) => timing.label === label)?.duration_ms);
}

function stdioServerPhaseTransportTimings(timings) {
  const serializationMs = stdioServerPhaseTiming(timings, "response_serialization");
  const newlineWriteMs = stdioServerPhaseTiming(timings, "newline_write");
  const flushMs = stdioServerPhaseTiming(timings, "flush");
  const phases = [serializationMs, newlineWriteMs, flushMs];
  return {
    stdio_server_phase_timings: timings,
    stdio_server_output_total_ms: phases.every(Number.isFinite)
      ? phases.reduce((sum, durationMs) => sum + durationMs, 0)
      : null,
    stdio_server_response_serialization_ms: serializationMs,
    stdio_server_newline_write_ms: newlineWriteMs,
    stdio_server_flush_ms: flushMs,
  };
}

function topPacketSearchQueries(searchSteps, limit = 8) {
  return [...searchSteps]
    .sort((left, right) => {
      const duration = (right.duration_ms ?? -1) - (left.duration_ms ?? -1);
      if (duration !== 0) {
        return duration;
      }
      return String(left.query ?? "").localeCompare(String(right.query ?? ""));
    })
    .slice(0, limit);
}

function packetLatencyTelemetry(packet, wallMs) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  const retrievalTrace = packet.answer?.retrieval_trace ?? null;
  const retrievalShadow = packetRetrievalShadowTelemetry(packetRetrievalShadow(packet));
  const freshness = packet.answer?.freshness ?? null;
  const steps = Array.isArray(retrievalTrace?.steps) ? retrievalTrace.steps : [];
  const topStep = [...steps].sort((left, right) => (right.duration_ms ?? 0) - (left.duration_ms ?? 0))[0] ?? null;
  const searchSteps = packetSearchStepTelemetry(steps);
  const batchTimings = packetBatchTimings(retrievalTrace?.annotations);
  const nonTracePhaseTimings = packetNonTracePhaseTimings(retrievalTrace?.annotations);
  const stdioPhaseTimings = packetStdioPhaseTimings(retrievalTrace?.annotations);
  const retrievalTotalMs = finiteNumber(retrievalTrace?.total_latency_ms);
  const freshnessMs = finiteNumber(freshness?.duration_ms);
  const accountedTraceMs =
    Number.isFinite(retrievalTotalMs) && Number.isFinite(freshnessMs)
      ? retrievalTotalMs + freshnessMs
      : null;
  const unaccountedMs =
    Number.isFinite(wallMs) && Number.isFinite(accountedTraceMs)
      ? Math.max(0, wallMs - accountedTraceMs)
      : null;
  return {
    freshness_ms: Number.isFinite(freshnessMs) ? freshnessMs : null,
    retrieval_total_ms: Number.isFinite(retrievalTotalMs) ? retrievalTotalMs : null,
    accounted_trace_ms: Number.isFinite(accountedTraceMs) ? accountedTraceMs : null,
    sla_target_ms: finiteNumber(retrievalTrace?.sla_target_ms),
    sla_missed: retrievalTrace?.sla_missed ?? null,
    unaccounted_ms: unaccountedMs,
    non_trace_wall_ms: unaccountedMs,
    top_step_kind: topStep?.kind ?? null,
    top_step_status: topStep?.status ?? null,
    top_step_duration_ms: finiteNumber(topStep?.duration_ms),
    top_step_message: topStep?.message ?? null,
    retrieval_step_count: steps.length,
    packet_search_total_ms: searchSteps.reduce((sum, step) => sum + (finiteNumber(step.duration_ms) ?? 0), 0),
    packet_initial_sidecar_query_ms: packetSearchPhaseTotal(searchSteps, "packet_initial_sidecar_query"),
    packet_anchor_probe_search_total_ms: packetSearchPhaseTotal(searchSteps, "symbolic_packet_anchor_probe"),
    packet_lexical_subquery_search_total_ms: packetSearchPhaseTotal(searchSteps, "packet_lexical_batch"),
    packet_semantic_subquery_search_total_ms: packetSearchPhaseTotal(searchSteps, "packet_semantic_batch"),
    packet_search_queries: topPacketSearchQueries(searchSteps),
    packet_batch_timings: batchTimings,
    packet_batch_total_ms: batchTimings.reduce((sum, timing) => sum + timing.total_ms, 0),
    packet_batch_attributed_query_ms: batchTimings.reduce((sum, timing) => sum + timing.attributed_query_ms, 0),
    packet_batch_overhead_ms: batchTimings.reduce((sum, timing) => sum + timing.overhead_ms, 0),
    packet_anchor_probe_batch_total_ms: packetBatchTiming(batchTimings, "packet_anchor_probe_batch", "total_ms"),
    packet_anchor_probe_batch_attributed_query_ms: packetBatchTiming(batchTimings, "packet_anchor_probe_batch", "attributed_query_ms"),
    packet_anchor_probe_batch_overhead_ms: packetBatchTiming(batchTimings, "packet_anchor_probe_batch", "overhead_ms"),
    packet_anchor_probe_batch_queries: packetBatchTiming(batchTimings, "packet_anchor_probe_batch", "queries"),
    packet_lexical_subquery_batch_total_ms: packetBatchTiming(batchTimings, "packet_lexical_subquery_batch", "total_ms"),
    packet_lexical_subquery_batch_attributed_query_ms: packetBatchTiming(batchTimings, "packet_lexical_subquery_batch", "attributed_query_ms"),
    packet_lexical_subquery_batch_overhead_ms: packetBatchTiming(batchTimings, "packet_lexical_subquery_batch", "overhead_ms"),
    packet_lexical_subquery_batch_queries: packetBatchTiming(batchTimings, "packet_lexical_subquery_batch", "queries"),
    packet_non_trace_phase_timings: nonTracePhaseTimings,
    packet_non_trace_phase_total_ms: nonTracePhaseTimings.reduce((sum, timing) => sum + timing.duration_ms, 0),
    packet_rank_and_window_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "rank_and_window"),
    packet_shadow_and_trace_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "shadow_and_trace"),
    packet_budget_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "budget"),
    packet_evidence_sections_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "evidence_sections"),
    packet_sufficiency_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "sufficiency"),
    packet_trace_summary_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "trace_summary"),
    packet_dto_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "packet_dto"),
    packet_output_budget_ms: packetNonTracePhaseTiming(nonTracePhaseTimings, "output_budget"),
    packet_stdio_phase_timings: stdioPhaseTimings,
    packet_stdio_phase_total_ms: stdioPhaseTimings.reduce((sum, timing) => sum + timing.duration_ms, 0),
    packet_stdio_text_materialization_ms: packetStdioPhaseTiming(stdioPhaseTimings, "text_materialization"),
    packet_stdio_tool_response_materialization_ms: packetStdioPhaseTiming(stdioPhaseTimings, "tool_response_materialization"),
    retrieval_shadow: retrievalShadow,
  };
}

async function runColdPacketRuntime(opts, task, repeat, outDir) {
  const repoConfig = ALL_REPOS[task.repo];
  const codestoryCli = resolveCodeStoryCli(opts);
  const provenance = await repoProvenance(repoConfig);
  const cacheProvenance = await codestoryCacheProvenance(
    opts,
    repoConfig,
    packetRuntimeCacheObservations(opts, task.repo, "cold_cli_packet"),
  );
  const args = packetCommandArgs(repoConfig, task, opts);
  const started = performance.now();
  const result = await runProcess(codestoryCli, args, {
    env: benchmarkChildEnv(process.env),
    timeoutMs: opts.timeoutMs,
  });
  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  let packet = null;
  let parseError = null;
  if (result.status === "pass") {
    try {
      packet = JSON.parse(result.stdout);
    } catch (error) {
      parseError = error.message;
    }
  }
  const quality = packet
    ? scoreQualityFromText(packetPayloadText(packet), JSON.stringify(packet), task)
    : null;
  const shape = packetShape(packet);
  const sufficiency = packetSufficiencyTelemetry(packet, quality);
  const latency = packetLatencyTelemetry(packet, wallMs);
  const composition = packetComposition(packet, task);
  const extraProbes = packetCommandExtraProbes(task, opts);
  const runId = benchmarkRunId([task.repo, task.id, "cold-cli-packet", String(repeat).padStart(2, "0")]);
  await writeFile(path.join(outDir, `${runId}.stdout.json`), result.stdout, "utf8");
  await writeFile(path.join(outDir, `${runId}.stderr.txt`), result.stderr, "utf8");
  return {
    repo: task.repo,
    task_id: task.id,
    task_class: task.task_class,
    task_manifest_path: task.manifest_path ?? null,
    task_manifest_snapshot: taskSnapshotForResult(task),
    repo_provenance: provenance,
    codestory_cache_provenance: cacheProvenance,
    mode: "cold_cli_packet",
    repeat,
    status: result.status === "pass" && !parseError ? "pass" : "fail",
    exit_code: result.exitCode,
    error: result.error ?? parseError,
    wall_ms: wallMs,
    response_bytes: Buffer.byteLength(result.stdout, "utf8"),
    packet_shape: shape,
    packet_latency: latency,
    packet_composition: composition,
    packet_extra_probe_count: extraProbes.length,
    packet_extra_probe_strategy: packetExtraProbeStrategy(extraProbes),
    sufficiency,
    quality,
  };
}

function createStdioClient(command, args, opts) {
  const child = spawn(command, args, {
    env: benchmarkChildEnv(process.env),
    shell: false,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  let buffer = "";
  let stderr = "";
  let stderrBuffer = "";
  const serverPhaseTimingsByRequestId = new Map();
  const pending = [];
  let closedError = null;
  function recordStderr(chunk) {
    stderr += chunk;
    stderrBuffer += chunk;
    for (;;) {
      const newline = stderrBuffer.indexOf("\n");
      if (newline < 0) {
        break;
      }
      const line = stderrBuffer.slice(0, newline).trim();
      stderrBuffer = stderrBuffer.slice(newline + 1);
      const serverPhase = parseStdioServerPhaseLine(line);
      if (!serverPhase) {
        continue;
      }
      const timings = serverPhaseTimingsByRequestId.get(serverPhase.request_id) ?? [];
      timings.push({
        label: serverPhase.label,
        duration_ms: serverPhase.duration_ms,
      });
      serverPhaseTimingsByRequestId.set(serverPhase.request_id, timings);
    }
  }
  function serverPhaseTimingsForRequest(requestIdKey) {
    return [...(serverPhaseTimingsByRequestId.get(requestIdKey) ?? [])];
  }
  function hasAllServerPhaseTimings(requestIdKey) {
    const labels = new Set(serverPhaseTimingsForRequest(requestIdKey).map((timing) => timing.label));
    return ["response_serialization", "newline_write", "flush"].every((label) => labels.has(label));
  }
  function rejectPending(error) {
    while (pending.length) {
      const waiter = pending.shift();
      waiter.reject(error);
    }
  }
  child.stdout.on("data", (chunk) => {
    buffer += chunk.toString();
    for (;;) {
      const newline = buffer.indexOf("\n");
      if (newline < 0) {
        break;
      }
      const line = buffer.slice(0, newline).trim();
      buffer = buffer.slice(newline + 1);
      if (!line) {
        continue;
      }
      const waiter = pending.shift();
      if (waiter) {
        waiter.resolve({
          line,
          timings: {
            ...waiter.timings,
            stdio_response_wait_ms: Math.round((performance.now() - waiter.responseWaitStarted) * 1000) / 1000,
          },
        });
      }
    }
  });
  child.stderr.on("data", (chunk) => {
    recordStderr(chunk.toString());
  });
  child.on("error", (error) => {
    closedError = error;
    rejectPending(error);
  });
  child.on("close", (exitCode, signal) => {
    closedError = new Error(
      `stdio server exited before responding: exit=${exitCode ?? ""} signal=${signal ?? ""} stderr=${stderr}`,
    );
    rejectPending(closedError);
  });

  return {
    child,
    stderr: () => stderr,
    request(payload) {
      return this.requestWithTimings(payload).then((result) => result.line);
    },
    requestWithTimings(payload) {
      return new Promise((resolve, reject) => {
        if (closedError) {
          reject(closedError);
          return;
        }
        let waiter;
        const timer = setTimeout(() => {
          const index = pending.indexOf(waiter);
          if (index >= 0) {
            pending.splice(index, 1);
          }
          reject(new Error(`stdio request timed out after ${opts.timeoutMs}ms: ${stderr}`));
        }, opts.timeoutMs);
        const stringifyStarted = performance.now();
        const requestLine = `${JSON.stringify(payload)}\n`;
        const requestIdKey = stdioRequestIdKey(payload?.id);
        const timings = {
          stdio_request_json_ms: Math.round((performance.now() - stringifyStarted) * 1000) / 1000,
        };
        waiter = {
          requestIdKey,
          timings,
          responseWaitStarted: performance.now(),
          resolve: (line) => {
            clearTimeout(timer);
            resolve({
              ...line,
              requestIdKey,
            });
          },
          reject: (error) => {
            clearTimeout(timer);
            reject(error);
          },
        };
        pending.push(waiter);
        const writeStarted = performance.now();
        child.stdin.write(requestLine);
        waiter.timings.stdio_request_write_ms = Math.round((performance.now() - writeStarted) * 1000) / 1000;
      });
    },
    waitForServerPhaseTimings(requestIdKey, timeoutMs = 250) {
      const started = performance.now();
      return new Promise((resolve) => {
        const poll = () => {
          if (hasAllServerPhaseTimings(requestIdKey) || performance.now() - started >= timeoutMs) {
            resolve(serverPhaseTimingsForRequest(requestIdKey));
            return;
          }
          setTimeout(poll, 5);
        };
        poll();
      });
    },
    close() {
      child.stdin.end();
      child.kill("SIGTERM");
    },
  };
}

async function runWarmPacketRuntimeGroup(opts, repoName, tasks, outDir) {
  const repoConfig = ALL_REPOS[repoName];
  const codestoryCli = resolveCodeStoryCli(opts);
  const provenance = await repoProvenance(repoConfig);
  const cacheProvenance = await codestoryCacheProvenance(
    opts,
    repoConfig,
    packetRuntimeCacheObservations(opts, repoName, "warm_stdio_packet"),
  );
  const client = createStdioClient(
    codestoryCli,
    ["serve", "--project", repoConfig.path, "--stdio", "--refresh", "none"],
    opts,
  );
  const rows = [];
  const previousPacketByTask = new Map();
  try {
    await client.request({
      jsonrpc: "2.0",
      id: "initialize",
      method: "initialize",
      params: { protocolVersion: "2024-11-05", capabilities: {} },
    });
    for (const task of tasks) {
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        const started = performance.now();
        const responseResult = await client.requestWithTimings({
          jsonrpc: "2.0",
          id: `${task.id}-${repeat}`,
          method: "tools/call",
          params: {
            name: "packet",
            arguments: {
              question: task.prompt,
              budget: "compact",
              task_class: task.task_class,
            },
          },
        });
        const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
        const responseLine = responseResult.line;
        const serverPhaseTimings = await client.waitForServerPhaseTimings(responseResult.requestIdKey);
        const parseStarted = performance.now();
        const response = JSON.parse(responseLine);
        const stdioTransport = {
          ...responseResult.timings,
          ...stdioServerPhaseTransportTimings(serverPhaseTimings),
          stdio_response_parse_ms: Math.round((performance.now() - parseStarted) * 1000) / 1000,
        };
        const packet = response.result?.structuredContent ?? null;
        const isError = response.result?.isError === true || response.error;
        const packetFingerprint = packet && !isError ? JSON.stringify(packet) : null;
        const previousPacket = packetFingerprint ? previousPacketByTask.get(task.id) : null;
        const warmStdioPacketCacheHit =
          packetFingerprint != null && previousPacket?.fingerprint === packetFingerprint;
        if (packetFingerprint) {
          previousPacketByTask.set(task.id, {
            fingerprint: packetFingerprint,
            repeat,
            wallMs,
          });
        }
        const quality = packet && !isError
          ? scoreQualityFromText(packetPayloadText(packet), JSON.stringify(packet), task)
          : null;
        const shape = packetShape(packet);
        const sufficiency = packetSufficiencyTelemetry(packet, quality);
        const latency = packetLatencyTelemetry(packet, wallMs);
        const composition = packetComposition(packet, task);
        const runId = benchmarkRunId([task.repo, task.id, "warm-stdio-packet", String(repeat).padStart(2, "0")]);
        await writeFile(path.join(outDir, `${runId}.response.json`), `${JSON.stringify(response, null, 2)}\n`, "utf8");
        rows.push({
          repo: task.repo,
          task_id: task.id,
          task_class: task.task_class,
          task_manifest_path: task.manifest_path ?? null,
          task_manifest_snapshot: taskSnapshotForResult(task),
          repo_provenance: provenance,
          codestory_cache_provenance: cacheProvenance,
          mode: "warm_stdio_packet",
          repeat,
          status: isError ? "fail" : "pass",
          exit_code: null,
          error: response.error?.message ?? (isError ? response.result?.content?.[0]?.text : null),
          wall_ms: wallMs,
          response_bytes: Buffer.byteLength(responseLine, "utf8"),
          stdio_transport: stdioTransport,
          warm_stdio_packet_cache_hit: warmStdioPacketCacheHit,
          warm_stdio_packet_cache_reference_repeat: warmStdioPacketCacheHit ? previousPacket.repeat : null,
          warm_stdio_packet_cache_reference_wall_ms: warmStdioPacketCacheHit ? previousPacket.wallMs : null,
          packet_shape: shape,
          packet_latency: latency,
          packet_composition: composition,
          sufficiency,
          quality,
        });
      }
    }
  } finally {
    client.close();
    if (client.stderr()) {
      await writeFile(path.join(outDir, `${repoName}-warm-stdio.stderr.txt`), client.stderr(), "utf8");
    }
  }
  return rows;
}

function summarizePacketRuntimeRuns(results) {
  const groups = new Map();
  for (const result of results) {
    const key = `${result.repo}\t${result.task_id}\t${result.mode}`;
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(result);
  }
  return [...groups.entries()].map(([key, rows]) => {
    const [repo, taskId, mode] = key.split("\t");
    const successful = rows.filter((row) => row.status === "pass");
    const qualityRows = successful.filter((row) => row.quality);
    const sufficiencyRows = successful.filter((row) => row.sufficiency);
    const shapeRows = successful.filter((row) => row.packet_shape);
    const latencyRows = successful.filter((row) => row.packet_latency);
    const shadowRows = latencyRows
      .map((row) => row.packet_latency?.retrieval_shadow)
      .filter((shadow) => shadow && typeof shadow === "object");
    const compositionRows = successful.filter((row) => row.packet_composition);
    const warmFirstHitRows = successful.filter((row) =>
      mode === "warm_stdio_packet" && row.warm_stdio_packet_cache_hit !== true
    );
    const warmCacheHitRows = successful.filter((row) =>
      mode === "warm_stdio_packet" && row.warm_stdio_packet_cache_hit === true
    );
    const topLatencyRow = latencyRows
      .filter((row) => Number.isFinite(Number(row.packet_latency?.top_step_duration_ms)))
      .sort((left, right) =>
        Number(right.packet_latency.top_step_duration_ms) - Number(left.packet_latency.top_step_duration_ms)
      )[0];
    const sufficiencyStatusCounts = {};
    for (const row of sufficiencyRows) {
      const status = row.sufficiency.status ?? "unknown";
      sufficiencyStatusCounts[status] = (sufficiencyStatusCounts[status] ?? 0) + 1;
    }
    return {
      repo,
      task_id: taskId,
      mode,
      runs: rows.length,
      successful_runs: successful.length,
      quality_pass_runs: qualityRows.filter((row) => row.quality?.pass).length,
      sufficiency_status_counts: sufficiencyStatusCounts,
      sufficient_quality_mismatch_runs: sufficiencyRows.filter((row) => row.sufficiency?.sufficient_quality_mismatch).length,
      median_wall_ms: median(successful.map((row) => row.wall_ms)),
      median_e2e_wall_ms: median(successful.map((row) => row.wall_ms)),
      median_response_bytes: median(successful.map((row) => row.response_bytes)),
      median_packet_bytes: median(shapeRows.map((row) => row.packet_shape?.packet_bytes)),
      median_packet_graph_bytes: median(shapeRows.map((row) => row.packet_shape?.graph_bytes)),
      median_budget_used_output_bytes: median(shapeRows.map((row) => row.packet_shape?.budget_used_output_bytes)),
      median_packet_freshness_ms: median(latencyRows.map((row) => row.packet_latency?.freshness_ms)),
      median_packet_retrieval_total_ms: median(latencyRows.map((row) => row.packet_latency?.retrieval_total_ms)),
      median_trace_sla_retrieval_ms: median(latencyRows.map((row) => row.packet_latency?.retrieval_total_ms)),
      median_packet_accounted_trace_ms: median(latencyRows.map((row) => row.packet_latency?.accounted_trace_ms)),
      median_trace_accounted_ms: median(latencyRows.map((row) => row.packet_latency?.accounted_trace_ms)),
      median_packet_unaccounted_ms: median(latencyRows.map((row) => row.packet_latency?.unaccounted_ms)),
      median_warm_first_hit_wall_ms: median(warmFirstHitRows.map((row) => row.wall_ms)),
      median_warm_cache_hit_wall_ms: median(warmCacheHitRows.map((row) => row.wall_ms)),
      median_packet_batch_total_ms: median(latencyRows.map((row) => row.packet_latency?.packet_batch_total_ms)),
      median_packet_batch_attributed_query_ms: median(latencyRows.map((row) => row.packet_latency?.packet_batch_attributed_query_ms)),
      median_packet_batch_overhead_ms: median(latencyRows.map((row) => row.packet_latency?.packet_batch_overhead_ms)),
      median_packet_anchor_probe_batch_overhead_ms: median(latencyRows.map((row) => row.packet_latency?.packet_anchor_probe_batch_overhead_ms)),
      median_packet_lexical_subquery_batch_overhead_ms: median(latencyRows.map((row) => row.packet_latency?.packet_lexical_subquery_batch_overhead_ms)),
      median_packet_non_trace_phase_total_ms: median(latencyRows.map((row) => row.packet_latency?.packet_non_trace_phase_total_ms)),
      median_packet_rank_and_window_ms: median(latencyRows.map((row) => row.packet_latency?.packet_rank_and_window_ms)),
      median_packet_shadow_and_trace_ms: median(latencyRows.map((row) => row.packet_latency?.packet_shadow_and_trace_ms)),
      median_packet_budget_ms: median(latencyRows.map((row) => row.packet_latency?.packet_budget_ms)),
      median_packet_dto_ms: median(latencyRows.map((row) => row.packet_latency?.packet_dto_ms)),
      median_packet_output_budget_ms: median(latencyRows.map((row) => row.packet_latency?.packet_output_budget_ms)),
      median_packet_evidence_sections_ms: median(latencyRows.map((row) => row.packet_latency?.packet_evidence_sections_ms)),
      median_packet_sufficiency_ms: median(latencyRows.map((row) => row.packet_latency?.packet_sufficiency_ms)),
      median_packet_trace_summary_ms: median(latencyRows.map((row) => row.packet_latency?.packet_trace_summary_ms)),
      median_packet_stdio_phase_total_ms: median(latencyRows.map((row) => row.packet_latency?.packet_stdio_phase_total_ms)),
      median_packet_stdio_text_materialization_ms: median(latencyRows.map((row) => row.packet_latency?.packet_stdio_text_materialization_ms)),
      median_packet_stdio_tool_response_materialization_ms: median(latencyRows.map((row) => row.packet_latency?.packet_stdio_tool_response_materialization_ms)),
      median_stdio_request_json_ms: median(successful.map((row) => row.stdio_transport?.stdio_request_json_ms)),
      median_stdio_request_write_ms: median(successful.map((row) => row.stdio_transport?.stdio_request_write_ms)),
      median_stdio_response_wait_ms: median(successful.map((row) => row.stdio_transport?.stdio_response_wait_ms)),
      median_stdio_server_output_total_ms: median(successful.map((row) => row.stdio_transport?.stdio_server_output_total_ms)),
      median_stdio_server_response_serialization_ms: median(successful.map((row) => row.stdio_transport?.stdio_server_response_serialization_ms)),
      median_stdio_server_newline_write_ms: median(successful.map((row) => row.stdio_transport?.stdio_server_newline_write_ms)),
      median_stdio_server_flush_ms: median(successful.map((row) => row.stdio_transport?.stdio_server_flush_ms)),
      median_stdio_response_parse_ms: median(successful.map((row) => row.stdio_transport?.stdio_response_parse_ms)),
      packet_sla_missed_runs: latencyRows.filter((row) => row.packet_latency?.sla_missed === true).length,
      warm_stdio_packet_cache_hit_runs: successful.filter((row) => row.warm_stdio_packet_cache_hit === true).length,
      retrieval_shadow_cache_hit_runs: shadowRows.filter((shadow) => shadow.cache_hit === true).length,
      retrieval_shadow_stage_cache_hit_runs: shadowRows.filter((shadow) => Number(shadow.cache_hit_stage_count) > 0).length,
      median_retrieval_shadow_cache_hit_stage_count: median(shadowRows.map((shadow) => shadow.cache_hit_stage_count)),
      packet_top_latency_step_kind: topLatencyRow?.packet_latency?.top_step_kind ?? null,
      packet_top_latency_step_status: topLatencyRow?.packet_latency?.top_step_status ?? null,
      median_packet_top_step_ms: median(latencyRows.map((row) => row.packet_latency?.top_step_duration_ms)),
      median_avoid_opening_count: median(sufficiencyRows.map((row) => row.sufficiency?.avoid_opening_count)),
      median_follow_up_commands_count: median(sufficiencyRows.map((row) => row.sufficiency?.follow_up_commands_count)),
      median_expected_file_recall: median(qualityRows.map((row) => row.quality?.expected_files?.recall)),
      median_expected_claim_recall: median(qualityRows.map((row) => row.quality?.expected_claims?.recall)),
      median_citation_coverage: median(qualityRows.map((row) => row.quality?.citation_coverage?.recall)),
      median_packet_citation_recall: median(compositionRows.map((row) => row.packet_composition?.citation_recall)),
      median_packet_answer_surface_recall: median(compositionRows.map((row) => row.packet_composition?.answer_surface_recall)),
      median_packet_structured_file_recall: median(compositionRows.map((row) => row.packet_composition?.structured_file_recall)),
    };
  });
}

function packetRuntimeTaskKey(row) {
  return `${row.repo}\t${row.task_id}\t${row.mode}`;
}

function roundPacketRuntimeNumber(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return null;
  }
  return Math.round(number * 1000) / 1000;
}

function pickPacketRuntimeMetrics(row) {
  return Object.fromEntries(
    PACKET_RUNTIME_DELTA_FIELDS
      .map((field) => [field, roundPacketRuntimeNumber(row?.[field])])
      .filter(([, value]) => value != null),
  );
}

function buildPacketRuntimeDeltas(currentRows, baselineRows, opts = {}) {
  const baselineByKey = new Map(baselineRows.map((row) => [packetRuntimeTaskKey(row), row]));
  return {
    baseline_summary: opts.baselinePath ?? null,
    current_summary: opts.currentPath ?? null,
    fields: PACKET_RUNTIME_DELTA_FIELDS,
    tasks: currentRows.map((row) => {
      const baseline = baselineByKey.get(packetRuntimeTaskKey(row));
      const current = pickPacketRuntimeMetrics(row);
      const deltas = {};
      if (baseline) {
        for (const field of PACKET_RUNTIME_DELTA_FIELDS) {
          const currentValue = roundPacketRuntimeNumber(row?.[field]);
          const baselineValue = roundPacketRuntimeNumber(baseline?.[field]);
          if (currentValue != null && baselineValue != null) {
            deltas[field] = {
              baseline: baselineValue,
              current: currentValue,
              delta: roundPacketRuntimeNumber(currentValue - baselineValue),
            };
          }
        }
      }
      return {
        repo: row.repo ?? null,
        task_id: row.task_id ?? null,
        mode: row.mode ?? null,
        baseline: baseline ? pickPacketRuntimeMetrics(baseline) : null,
        current,
        deltas: baseline ? deltas : null,
      };
    }),
  };
}

function packetRuntimeArtifactManifest({ outDir, benchmarkId, artifactPaths }) {
  const stableDir = path.join("target", "agent-benchmark", benchmarkId).replaceAll(path.sep, "/");
  return {
    output_dir: outDir,
    benchmark_run_id: benchmarkId,
    artifacts: artifactPaths,
    durable_copy_convention: {
      suggested_stable_directory: stableDir,
      note:
        "Before linking focused packet-runtime evidence from a temporary worktree, copy the full run directory to a stable checkout path or attach these artifacts to the PR/issue.",
    },
  };
}

function packetRuntimeMarkdown(summary) {
  const lines = [
    "# Packet Runtime Benchmark",
    "",
    "| Repo | Task | Mode | Runs | Pass | Quality Pass | Sufficiency | Suff/quality gaps | E2E wall ms median | Trace SLA retrieval ms median | Trace accounted ms median | Freshness ms median | Non-trace wall ms median | Warm first-hit wall ms median | Warm cache-hit wall ms median | Post-retrieval phases ms median | Stdio phases ms median | Budget ms median | DTO ms median | Output budget ms median | Sufficiency ms median | Stdio req JSON ms median | Stdio req write ms median | Stdio resp wait ms median | Server output ms median | Server serialize ms median | Server newline ms median | Server flush ms median | Stdio resp parse ms median | Batch total ms median | Batch attributed ms median | Batch overhead ms median | Anchor batch overhead ms median | Lexical batch overhead ms median | Top step | Top step ms median | SLA misses | Packet-cache hits | Retrieval cache-hit runs | Stage cache-hit runs | Response bytes median | Packet bytes median | Graph bytes median | Avoid-open median | Follow-up median | File recall | Citation coverage | Packet citation recall | Packet answer-surface recall | Packet structured recall |",
    "| --- | --- | --- | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const row of summary) {
    lines.push(packetRuntimeMarkdownRow(row));
  }
  return `${lines.join("\n")}\n`;
}

function packetRuntimeMarkdownRow(row) {
  const sufficiency = Object.entries(row.sufficiency_status_counts ?? {})
    .map(([status, count]) => `${status}:${count}`)
    .join(", ");
  const cells = [
    row.repo,
    row.task_id,
    row.mode,
    row.runs,
    row.successful_runs,
    row.quality_pass_runs,
    sufficiency,
    formatValue(row.sufficient_quality_mismatch_runs),
    formatValue(row.median_e2e_wall_ms),
    formatValue(row.median_trace_sla_retrieval_ms),
    formatValue(row.median_trace_accounted_ms),
    formatValue(row.median_packet_freshness_ms),
    formatValue(row.median_packet_unaccounted_ms),
    formatValue(row.median_warm_first_hit_wall_ms),
    formatValue(row.median_warm_cache_hit_wall_ms),
    formatValue(row.median_packet_non_trace_phase_total_ms),
    formatValue(row.median_packet_stdio_phase_total_ms),
    formatValue(row.median_packet_budget_ms),
    formatValue(row.median_packet_dto_ms),
    formatValue(row.median_packet_output_budget_ms),
    formatValue(row.median_packet_sufficiency_ms),
    formatValue(row.median_stdio_request_json_ms),
    formatValue(row.median_stdio_request_write_ms),
    formatValue(row.median_stdio_response_wait_ms),
    formatValue(row.median_stdio_server_output_total_ms),
    formatValue(row.median_stdio_server_response_serialization_ms),
    formatValue(row.median_stdio_server_newline_write_ms),
    formatValue(row.median_stdio_server_flush_ms),
    formatValue(row.median_stdio_response_parse_ms),
    formatValue(row.median_packet_batch_total_ms),
    formatValue(row.median_packet_batch_attributed_query_ms),
    formatValue(row.median_packet_batch_overhead_ms),
    formatValue(row.median_packet_anchor_probe_batch_overhead_ms),
    formatValue(row.median_packet_lexical_subquery_batch_overhead_ms),
    row.packet_top_latency_step_kind ?? "",
    formatValue(row.median_packet_top_step_ms),
    formatValue(row.packet_sla_missed_runs),
    formatValue(row.warm_stdio_packet_cache_hit_runs),
    formatValue(row.retrieval_shadow_cache_hit_runs),
    formatValue(row.retrieval_shadow_stage_cache_hit_runs),
    formatValue(row.median_response_bytes),
    formatValue(row.median_packet_bytes),
    formatValue(row.median_packet_graph_bytes),
    formatValue(row.median_avoid_opening_count),
    formatValue(row.median_follow_up_commands_count),
    formatPercent(row.median_expected_file_recall),
    formatPercent(row.median_citation_coverage),
    formatPercent(row.median_packet_citation_recall),
    formatPercent(row.median_packet_answer_surface_recall),
    formatPercent(row.median_packet_structured_file_recall),
  ];
  return `| ${cells.join(" | ")} |`;
}

function repoProvenanceBlockers(result) {
  const provenance = result.repo_provenance;
  if (!provenance) {
    return ["missing repo provenance"];
  }
  const reasons = [];
  if (provenance.manifest_overridden_by_builtin) {
    reasons.push("manifest repo was overridden by a built-in checkout");
  }
  const configuredRef = provenance.configured?.ref ?? null;
  const manifestRef = provenance.manifest?.ref ?? null;
  const configuredCommit = normalizeImmutableCommitRef(configuredRef);
  const manifestCommit = manifestRef ? normalizeImmutableCommitRef(manifestRef) : null;
  const gitHead = normalizeImmutableCommitRef(provenance.git_head);
  if (!configuredCommit) {
    reasons.push("repo ref is not pinned to a full immutable commit SHA");
  }
  if (manifestRef && configuredRef && manifestCommit !== configuredCommit) {
    reasons.push(`manifest ref ${manifestRef} does not match configured ref ${configuredRef}`);
  }
  if (!gitHead) {
    reasons.push("missing git head");
  } else if (configuredCommit && gitHead !== configuredCommit) {
    reasons.push(`git head ${provenance.git_head} does not match configured ref ${configuredRef}`);
  }
  const configuredUrl = provenance.configured?.url ?? null;
  const manifestUrl = provenance.manifest?.url ?? null;
  const gitOrigin = provenance.git_origin ?? null;
  const configuredRepo = normalizeTrustedPublishableRepoUrl(configuredUrl);
  const manifestRepo = manifestUrl ? normalizeTrustedPublishableRepoUrl(manifestUrl) : null;
  const originRepo = gitOrigin ? normalizeTrustedPublishableRepoUrl(gitOrigin) : null;
  if (!configuredRepo) {
    reasons.push("configured repo URL is not a trusted GitHub HTTPS repo URL");
  }
  if (!manifestUrl) {
    reasons.push("missing manifest repo URL");
  } else if (!manifestRepo) {
    reasons.push("manifest repo URL is not a trusted GitHub HTTPS repo URL");
  }
  if (configuredRepo && manifestUrl && manifestRepo && manifestRepo !== configuredRepo) {
    reasons.push(`manifest repo URL ${manifestUrl} does not match configured URL ${configuredUrl}`);
  }
  if (!originRepo) {
    reasons.push("git origin is missing or is not a trusted GitHub HTTPS repo URL");
  } else if (configuredRepo && originRepo !== configuredRepo) {
    reasons.push(`git origin ${gitOrigin} does not match configured URL ${configuredUrl}`);
  }
  if (provenance.git_dirty !== false) {
    reasons.push(provenance.git_dirty ? "repo checkout is dirty" : "repo cleanliness is unknown");
  }
  return reasons;
}

function isImmutableCommitRef(ref) {
  return /^[0-9a-f]{40}$/i.test(String(ref ?? "").trim());
}

function normalizeImmutableCommitRef(ref) {
  const value = String(ref ?? "").trim();
  return isImmutableCommitRef(value) ? value.toLowerCase() : null;
}

function cacheProvenanceBlockers(result) {
  const provenance = result.codestory_cache_provenance;
  if (!provenance) {
    return ["missing CodeStory cache provenance"];
  }
  const reasons = [];
  if (provenance.doctor_status !== "pass") {
    reasons.push("CodeStory doctor provenance failed");
  }
  if (!provenance.storage_path) {
    reasons.push("missing CodeStory cache path");
  }
  if (!provenance.cache_policy) {
    reasons.push("missing CodeStory cache policy");
  }
  if (provenance.cache_policy === "unprepared-cache-blocked") {
    reasons.push("CodeStory sidecar cache was not prepared");
  }
  if (provenance.retrieval_mode !== "full") {
    reasons.push(`CodeStory retrieval mode=${provenance.retrieval_mode ?? "unknown"}; expected full`);
  }
  if (!provenance.sidecar_generation) {
    reasons.push("missing CodeStory sidecar generation");
  }
  if (provenance.manifest_embedding_backend !== "llamacpp:bge-base-en-v1.5") {
    reasons.push(
      `CodeStory sidecar embedding backend=${provenance.manifest_embedding_backend ?? "unknown"}; expected llamacpp:bge-base-en-v1.5`,
    );
  }
  if (provenance.semantic_backend == null) {
    reasons.push("missing CodeStory semantic backend");
  }
  if (provenance.local_only !== true) {
    reasons.push(`CodeStory local-only guarantee is not proven (${provenance.locality_kind ?? "unknown"})`);
  }
  if (provenance.indexed !== true) {
    reasons.push("CodeStory cache is not indexed");
  }
  if (provenance.freshness_status !== "fresh") {
    reasons.push(`CodeStory cache freshness=${provenance.freshness_status ?? "unknown"}`);
  }
  if (provenance.semantic_ready !== true) {
    reasons.push("CodeStory semantic docs are not ready");
  }
  if (provenance.indexing_in_timed_run == null) {
    reasons.push("missing timed-run indexing provenance");
  }
  return reasons;
}

function qualityFailureReasons(quality) {
  if (!quality) {
    return ["missing_quality_score"];
  }
  if (quality.pass) {
    return [];
  }
  const reasons = [];
  const thresholds = quality.thresholds ?? {};
  const files = quality.expected_files ?? {};
  const symbols = quality.expected_symbols ?? {};
  const claims = quality.expected_claims ?? {};
  const citations = quality.citation_coverage ?? {};
  const anchors = quality.expected_anchors ?? {};
  const forbidden = quality.forbidden_claims ?? {};

  if (!thresholdPass(anchors.recall, thresholdValue(thresholds, "expected_anchor_recall", 0.8))) {
    reasons.push("expected_anchor_recall_low");
  }
  if (!thresholdPass(files.recall, thresholdValue(thresholds, "expected_file_recall", 0.8))) {
    reasons.push("expected_file_recall_low");
  }
  if (!thresholdPass(symbols.recall, thresholdValue(thresholds, "expected_symbol_recall", 0.7))) {
    reasons.push("expected_symbol_recall_low");
  }
  if (!thresholdPass(claims.recall, thresholdValue(thresholds, "expected_claim_recall", 0.8))) {
    reasons.push("expected_claim_recall_low");
  }
  if (!thresholdPass(citations.recall, thresholdValue(thresholds, "citation_coverage", 0.6))) {
    reasons.push("citation_coverage_low");
  }
  if ((forbidden.found ?? 0) > thresholdValue(thresholds, "max_forbidden_claims", 0)) {
    reasons.push("forbidden_claim_present");
  }
  if (!reasons.length) {
    reasons.push("quality_gate_failed");
  }
  return reasons;
}

function extractRetrievalDiagnostics(row) {
  const shadow = row.packet_composition?.retrieval_shadow ?? row.packet_latency?.retrieval_shadow ?? null;
  const composition = row.packet_composition ?? null;
  if (!shadow && !composition) {
    return null;
  }
  return {
    retrieval_mode: shadow?.retrieval_mode ?? null,
    degraded_reason: shadow?.degraded_reason ?? null,
    cache_hit: shadow?.cache_hit ?? null,
    cache_hit_stage_count: shadow?.cache_hit_stage_count ?? null,
    cache_hit_stages: shadow?.cache_hit_stages ?? null,
    candidate_count: shadow?.candidate_count ?? null,
    resolved_hit_count: shadow?.resolved_hit_count ?? null,
    unavailable_mode: shadow?.retrieval_mode === "unavailable",
    citation_recall: composition?.citation_recall ?? null,
    answer_surface_recall: composition?.answer_surface_recall ?? null,
    structured_file_recall: composition?.structured_file_recall ?? null,
  };
}

function buildQualityDebugPayload(results, meta = {}) {
  const rows = results.map((row) => {
    const quality = row.quality ?? null;
    const failureReasons = qualityFailureReasons(quality);
    return {
      repo: row.repo,
      task_id: row.task_id,
      mode: row.mode,
      repeat: row.repeat ?? null,
      status: row.status,
      warm_stdio_packet_cache_hit: row.warm_stdio_packet_cache_hit ?? null,
      quality_pass: quality?.pass ?? null,
      failure_reasons: failureReasons,
      quality_metrics: quality
        ? {
            expected_file_recall: quality.expected_files?.recall ?? null,
            expected_symbol_recall: quality.expected_symbols?.recall ?? null,
            expected_claim_recall: quality.expected_claims?.recall ?? null,
            citation_coverage: quality.citation_coverage?.recall ?? null,
            expected_anchor_recall: quality.expected_anchors?.recall ?? null,
            forbidden_claims_found: quality.forbidden_claims?.found ?? null,
          }
        : null,
      missed_anchors: quality?.missed_anchors ?? null,
      retrieval: extractRetrievalDiagnostics(row),
      sufficiency_status: row.sufficiency?.status ?? null,
      sufficiency: row.sufficiency
        ? {
            status: row.sufficiency.status ?? null,
            gaps: row.sufficiency.gaps ?? [],
            open_next: row.sufficiency.open_next ?? [],
            follow_up_commands: row.sufficiency.follow_up_commands ?? [],
            gaps_count: row.sufficiency.gaps_count ?? 0,
            open_next_count: row.sufficiency.open_next_count ?? 0,
            follow_up_commands_count: row.sufficiency.follow_up_commands_count ?? 0,
            covered_claims_count: row.sufficiency.covered_claims_count ?? 0,
            avoid_opening_count: row.sufficiency.avoid_opening_count ?? 0,
          }
        : null,
      sufficient_quality_mismatch: row.sufficiency?.sufficient_quality_mismatch ?? null,
    };
  });
  const failing = rows.filter((row) => row.quality_pass === false);
  const partial = rows.filter((row) => row.sufficiency_status === "partial");
  const reasonCounts = {};
  for (const row of failing) {
    for (const reason of row.failure_reasons) {
      reasonCounts[reason] = (reasonCounts[reason] ?? 0) + 1;
    }
  }
  const partialGapCounts = {};
  for (const row of partial) {
    for (const gap of row.sufficiency?.gaps ?? []) {
      partialGapCounts[gap] = (partialGapCounts[gap] ?? 0) + 1;
    }
  }
  return {
    generated_at: new Date().toISOString(),
    scope: "packet_runtime_quality_debug",
    ...meta,
    rows,
    summary: {
      runs: rows.length,
      quality_scored_runs: rows.filter((row) => row.quality_pass != null).length,
      quality_pass_runs: rows.filter((row) => row.quality_pass === true).length,
      quality_fail_runs: failing.length,
      packet_partial_runs: partial.length,
      failure_reason_counts: reasonCounts,
      partial_gap_counts: partialGapCounts,
    },
  };
}

function packetRuntimePublishableBlockers(results, opts = {}) {
  const enforceRepoProvenance = Boolean(opts.publishable || opts.enforceRepoProvenance);
  const enforcePacketRuntimeTelemetry = Boolean(opts.publishable || opts.enforcePacketRuntimeTelemetry);
  return results
    .flatMap((row) => {
      const productReasons = [];
      const harnessReasons = [];
      const environmentReasons = [];
      if (row.status !== "pass") {
        productReasons.push(`status=${row.status}`);
      }
      if (!row.quality) {
        productReasons.push("missing manifest quality score");
      } else if (!row.quality.pass) {
        productReasons.push("manifest quality failed");
      }
      if (row.sufficiency?.sufficient_quality_mismatch) {
        productReasons.push("packet sufficiency says sufficient but manifest quality failed");
      }
      if (enforcePacketRuntimeTelemetry) {
        if (row.packet_extra_probe_strategy) {
          harnessReasons.push(`diagnostic packet extra probes used: ${row.packet_extra_probe_strategy}`);
        }
        if (!row.sufficiency) {
          harnessReasons.push("missing packet sufficiency telemetry");
        } else {
          addPacketSufficiencyPublishableReasons(row.sufficiency, productReasons, harnessReasons, "packet");
          if (row.sufficiency.retrieval_mode && row.sufficiency.retrieval_mode !== "full") {
            environmentReasons.push(
              `packet retrieval mode=${row.sufficiency.retrieval_mode}; expected full`,
            );
          }
          if (row.sufficiency.degraded_reason) {
            environmentReasons.push(`packet retrieval degraded=${row.sufficiency.degraded_reason}`);
          }
        }
        const latency = row.packet_latency;
        if (!latency) {
          harnessReasons.push("missing packet latency telemetry");
        } else {
          if (latency.sla_missed !== false) {
            productReasons.push(`packet retrieval SLA missed=${latency.sla_missed ?? "unknown"}; expected false`);
          }
          const shadow = latency.retrieval_shadow;
          if (!shadow) {
            harnessReasons.push("missing retrieval shadow telemetry");
          } else if (shadow.retrieval_mode !== "full") {
            environmentReasons.push(`packet retrieval shadow mode=${shadow.retrieval_mode ?? "unknown"}; expected full`);
          }
        }
      }
      if (enforceRepoProvenance) {
        environmentReasons.push(...repoProvenanceBlockers(row));
        environmentReasons.push(...cacheProvenanceBlockers(row));
      }
      return [
        productReasons.length ? { result: row, category: "product", reasons: productReasons } : null,
        harnessReasons.length ? { result: row, category: "harness-contract", reasons: harnessReasons } : null,
        environmentReasons.length ? { result: row, category: "environment", reasons: environmentReasons } : null,
      ];
    })
    .filter(Boolean);
}

function addPacketSufficiencyPublishableReasons(sufficiency, productReasons, harnessReasons, label) {
  if (sufficiency.status !== "sufficient") {
    productReasons.push(`${label} sufficiency status=${sufficiency.status ?? "unknown"}; expected sufficient`);
  }
  const followUps = presentFiniteNumber(sufficiency.follow_up_commands_count);
  if (followUps > 0) {
    productReasons.push(`${label} follow-up commands=${followUps}; expected 0`);
  }
  const openNext = presentFiniteNumber(sufficiency.open_next_count);
  if (openNext > 0) {
    productReasons.push(`${label} open-next items=${openNext}; expected 0`);
  }
  const gaps = presentFiniteNumber(sufficiency.gaps_count);
  if (gaps > 0) {
    productReasons.push(`${label} sufficiency gaps=${gaps}; expected 0`);
  }
  const unresolvedCandidates = presentFiniteNumber(sufficiency.unresolved_candidate_count);
  if (unresolvedCandidates > 0 && sufficiency.unresolved_candidate_diagnostic_only !== true) {
    productReasons.push(`${label} unresolved retrieval candidates=${unresolvedCandidates}; expected 0`);
  }
  const unresolvedCoverage = presentFiniteNumber(
    sufficiency.coverage_unresolved_blocking_count ?? sufficiency.coverage_unresolved_count,
  );
  if (unresolvedCoverage > 0) {
    productReasons.push(`${label} unresolved coverage diagnostics=${unresolvedCoverage}; expected 0`);
  }
}

function packetRuntimeQualityGateRequired(opts = {}) {
  return Boolean(
    opts.publishable ||
      (["holdout-retrieval", "language-expansion-holdout"].includes(opts.taskSuite) &&
        !opts.allowFailures),
  );
}

function formatPacketRuntimeBlocker(blocker) {
  const row = blocker.result;
  const category = blocker.category ? `${blocker.category}: ` : "";
  return `  ${row.repo} ${row.task_id} ${row.mode} repeat ${row.repeat}: ${category}${blocker.reasons.join("; ")}`;
}

function groupTasksByRepo(tasks) {
  const byRepo = new Map();
  for (const task of tasks) {
    if (!byRepo.has(task.repo)) {
      byRepo.set(task.repo, []);
    }
    byRepo.get(task.repo).push(task);
  }
  return byRepo;
}

function packetCompositionPayload(results) {
  return {
    generated_at: new Date().toISOString(),
    scope: "packet_runtime_composition",
    rows: results
      .filter((row) => row.packet_composition)
      .map((row) => ({
        repo: row.repo,
        task_id: row.task_id,
        mode: row.mode,
        repeat: row.repeat,
        status: row.status,
        sufficiency_status: row.sufficiency?.status ?? null,
        composition_summary: {
          expected_file_count: row.packet_composition.expected_file_count,
          cited_file_count: row.packet_composition.cited_file_count,
          avoid_opening_file_count: row.packet_composition.avoid_opening_file_count,
          answer_text_file_count: row.packet_composition.answer_text_file_count,
          answer_surface_file_count: row.packet_composition.answer_surface_file_count,
          structured_file_count: row.packet_composition.structured_file_count,
          absent_file_count: row.packet_composition.absent_file_count,
          citation_recall: row.packet_composition.citation_recall,
          answer_surface_recall: row.packet_composition.answer_surface_recall,
          structured_file_recall: row.packet_composition.structured_file_recall,
          boundary_counts: row.packet_composition.boundary_counts,
          expected_verification_file_count: row.packet_composition.expected_verification_file_count,
          verification_summary: row.packet_composition.verification_summary,
        },
        expected_files: row.packet_composition.files,
        expected_verification_files: row.packet_composition.verification_files,
      })),
  };
}

function packetCompositionMarkdown(payload) {
  const lines = [
    "# Packet Runtime Composition",
    "",
    "| Repo | Task | Mode | Repeat | Status | Sufficiency | Citation recall | Answer-surface recall | Structured recall | Boundary counts |",
    "| --- | --- | --- | ---: | --- | --- | ---: | ---: | ---: | --- |",
  ];
  for (const row of payload.rows) {
    const summary = row.composition_summary ?? {};
    const boundaryCounts = Object.entries(summary.boundary_counts ?? {})
      .map(([boundary, count]) => `${boundary}:${count}`)
      .join(", ");
    lines.push(
      `| ${row.repo} | ${row.task_id} | ${row.mode} | ${row.repeat} | ${row.status} | ${row.sufficiency_status ?? ""} | ${formatPercent(summary.citation_recall)} | ${formatPercent(summary.answer_surface_recall)} | ${formatPercent(summary.structured_file_recall)} | ${boundaryCounts} |`,
    );
  }
  for (const row of payload.rows) {
    lines.push("");
    lines.push(`## ${row.repo} / ${row.task_id} / ${row.mode} / repeat ${row.repeat}`);
    lines.push("");
    lines.push("| Expected file | Boundary | Surfaces |");
    lines.push("| --- | --- | --- |");
    for (const file of row.expected_files ?? []) {
      const surfaces = (file.surfaces ?? [])
        .map((surface) =>
          [
            surface.source,
            surface.rank == null ? null : `rank=${surface.rank}`,
            surface.line == null ? null : `line=${surface.line}`,
          ]
            .filter(Boolean)
            .join(" "),
        )
        .join("<br>");
      lines.push(`| ${file.expected_file} | ${file.packet_boundary} | ${surfaces || ""} |`);
    }
    if ((row.expected_verification_files ?? []).length) {
      lines.push("");
      lines.push("| Expected verification file | Boundary | Surfaces |");
      lines.push("| --- | --- | --- |");
      for (const file of row.expected_verification_files ?? []) {
        const surfaces = (file.surfaces ?? [])
          .map((surface) =>
            [
              surface.source,
              surface.rank == null ? null : `rank=${surface.rank}`,
              surface.line == null ? null : `line=${surface.line}`,
            ]
              .filter(Boolean)
              .join(" "),
          )
          .join("<br>");
        lines.push(`| ${file.expected_file} | ${file.packet_boundary} | ${surfaces || ""} |`);
      }
    }
  }
  return `${lines.join("\n")}\n`;
}

async function runPacketRuntimeBenchmark(opts, tasks) {
  if (!tasks.length) {
    throw new Error("--packet-runtime requires --task-suite or --task-manifest");
  }
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const outDir = path.resolve(opts.outDir ?? path.join(repoRoot, "target", "agent-benchmark", `packet-runtime-${timestamp}`));
  const benchmarkId = opts.benchmarkRunId ?? path.basename(outDir);
  await mkdir(outDir, { recursive: true });
  const cachePreparation = opts.prepareCodestoryCache
    ? await prepareCodeStoryCaches(opts, tasks)
    : [];
  opts.cachePreparationByRepo = new Map(cachePreparation.map((row) => [row.repo, row]));
  if (cachePreparation.length) {
    await writeFile(
      path.join(outDir, "codestory-cache-preparation.json"),
      `${JSON.stringify(cachePreparation, null, 2)}\n`,
      "utf8",
    );
  }
  const modes =
    opts.packetRuntimeMode === "both"
      ? ["cold-cli", "warm-stdio"]
      : [opts.packetRuntimeMode];
  const results = [];
  if (modes.includes("cold-cli")) {
    const coldJobs = [];
    for (const task of tasks) {
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        coldJobs.push({ task, repeat });
      }
    }
    const coldResults = await parallelMap(coldJobs, opts.jobs, async ({ task, repeat }) => {
      console.log(`packet-runtime cold-cli ${task.repo} ${task.id} repeat ${repeat}/${opts.repeats}`);
      return await runColdPacketRuntime(opts, task, repeat, outDir);
    });
    for (const result of coldResults) {
      if (result) {
        results.push(result);
      }
    }
  }
  if (modes.includes("warm-stdio")) {
    for (const [repoName, repoTasks] of groupTasksByRepo(tasks)) {
      console.log(`packet-runtime warm-stdio ${repoName} tasks=${repoTasks.length} repeats=${opts.repeats}`);
      results.push(...(await runWarmPacketRuntimeGroup(opts, repoName, repoTasks, outDir)));
    }
  }
  await writeJsonlRows(path.join(outDir, "packet-runtime-runs.jsonl"), results);
  const summary = summarizePacketRuntimeRuns(results);
  const blockers = packetRuntimePublishableBlockers(results, opts);
  const payload = {
    generated_at: new Date().toISOString(),
    benchmark_run_id: benchmarkId,
    codestory_cli: resolveCodeStoryCli(opts),
    modes,
    repeats: opts.repeats,
    output_dir: outDir,
    retrieval_env: retrievalEnv(),
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(process.env)),
    benchmark_contract: benchmarkRunContract({
      opts,
      task: null,
      env: process.env,
      harnessPath: benchmarkHarnessPath,
      scorerPath: benchmarkScorerPath,
      cliIdentity: opts.codestoryCli ?? process.env.CODESTORY_CLI ?? null,
    }),
    ...(process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT
      ? {
          release_evidence: {
            commit: process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT,
            profile: process.env.CODESTORY_RELEASE_EVIDENCE_PROFILE,
            evidence_identity: {
              corpus_id: process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID,
              cache_id: process.env.CODESTORY_RELEASE_EVIDENCE_CACHE_ID,
              machine_fingerprint:
                process.env.CODESTORY_RELEASE_EVIDENCE_MACHINE_FINGERPRINT,
            },
            publishable: opts.publishable === true,
            repeats: opts.repeats,
            quality_gate_status:
              opts.publishable === true && blockers.length === 0 ? "pass" : "fail",
            publishable_blockers: blockers.map((blocker) => ({
              repo: blocker.result.repo,
              task_id: blocker.result.task_id,
              mode: blocker.result.mode,
              repeat: blocker.result.repeat,
              category: blocker.category,
              reasons: blocker.reasons,
            })),
            rows: results,
          },
        }
      : {}),
    summary,
  };
  const packetRuntimeSummaryPath = path.join(outDir, "packet-runtime-summary.json");
  await writeFile(packetRuntimeSummaryPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  const packetRuntimeMarkdownPath = path.join(outDir, "packet-runtime-summary.md");
  await writeFile(packetRuntimeMarkdownPath, packetRuntimeMarkdown(summary), "utf8");
  const baselinePacketSummaryPath = discoverPreviousPacketSummary(packetRuntimeSummaryPath, repoRoot);
  const baselinePacketSummary = baselinePacketSummaryPath
    ? JSON.parse(await readFile(baselinePacketSummaryPath, "utf8"))
    : null;
  const packetRuntimeDeltas = buildPacketRuntimeDeltas(
    summary,
    Array.isArray(baselinePacketSummary?.summary) ? baselinePacketSummary.summary : [],
    {
      currentPath: packetRuntimeSummaryPath,
      baselinePath: baselinePacketSummaryPath,
    },
  );
  const packetRuntimeDeltasPath = path.join(outDir, "packet-runtime-deltas.json");
  await writeFile(packetRuntimeDeltasPath, `${JSON.stringify(packetRuntimeDeltas, null, 2)}\n`, "utf8");
  console.log(`ARTIFACT packet_runtime_deltas=${packetRuntimeDeltasPath}`);
  const packetQualityDeltas = buildPacketQualityDeltas(
    summary,
    Array.isArray(baselinePacketSummary?.summary) ? baselinePacketSummary.summary : [],
    {
      currentPath: packetRuntimeSummaryPath,
      baselinePath: baselinePacketSummaryPath,
    },
  );
  const packetQualityDeltasPath = path.join(outDir, "packet-quality-deltas.json");
  await writeFile(packetQualityDeltasPath, `${JSON.stringify(packetQualityDeltas, null, 2)}\n`, "utf8");
  console.log(`ARTIFACT packet_quality_deltas=${packetQualityDeltasPath}`);
  const qualityDebug = buildQualityDebugPayload(results, {
    output_dir: outDir,
    benchmark_run_id: benchmarkId,
    codestory_cli: resolveCodeStoryCli(opts),
    modes,
    repeats: opts.repeats,
  });
  const qualityDebugPath = path.join(outDir, "quality-debug.json");
  await writeFile(qualityDebugPath, `${JSON.stringify(qualityDebug, null, 2)}\n`, "utf8");
  console.log(`ARTIFACT quality_debug=${qualityDebugPath}`);
  const compositionPayload = packetCompositionPayload(results);
  if (compositionPayload.rows.length) {
    await writeFile(
      path.join(outDir, "packet-composition.json"),
      `${JSON.stringify(compositionPayload, null, 2)}\n`,
      "utf8",
    );
    await writeFile(
      path.join(outDir, "packet-composition.md"),
      packetCompositionMarkdown(compositionPayload),
      "utf8",
    );
  }
  const packetRuntimeArtifactManifestPath = path.join(outDir, "packet-runtime-artifacts.json");
  await writeFile(
    packetRuntimeArtifactManifestPath,
    `${JSON.stringify(
      packetRuntimeArtifactManifest({
        outDir,
        benchmarkId,
        artifactPaths: {
          summary_json: packetRuntimeSummaryPath,
          summary_markdown: packetRuntimeMarkdownPath,
          runs_jsonl: path.join(outDir, "packet-runtime-runs.jsonl"),
          runtime_deltas_json: packetRuntimeDeltasPath,
          quality_deltas_json: packetQualityDeltasPath,
          quality_debug_json: qualityDebugPath,
        },
      }),
      null,
      2,
    )}\n`,
    "utf8",
  );
  console.log(`ARTIFACT packet_runtime_artifacts=${packetRuntimeArtifactManifestPath}`);

  if (opts.publishable && blockers.length) {
    console.error(
      "--publishable failed: packet runtime rows must pass, include passing manifest quality gates, report sufficient packets with zero follow-ups or unresolved diagnostics, and use pinned clean repo provenance.",
    );
    for (const blocker of blockers) {
      console.error(formatPacketRuntimeBlocker(blocker));
    }
    process.exitCode = 1;
  } else if (packetRuntimeQualityGateRequired(opts) && blockers.length) {
    console.error(
      "holdout-retrieval packet-runtime gate failed: every row must pass manifest quality thresholds. Use --allow-failures only for exploratory diagnostics.",
    );
    for (const blocker of blockers) {
      console.error(formatPacketRuntimeBlocker(blocker));
    }
    process.exitCode = 1;
  }
  console.log(`wrote ${outDir}`);
}

function median(values) {
  const sorted = values.filter((value) => value != null).sort((a, b) => a - b);
  if (!sorted.length) {
    return null;
  }
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function presentFiniteNumber(value) {
  if (value == null || value === "") {
    return null;
  }
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

function sumFinite(values) {
  return values.reduce((sum, value) => {
    const number = presentFiniteNumber(value);
    return number == null ? sum : sum + number;
  }, 0);
}

function sumPresentFinite(values) {
  let seen = false;
  let sum = 0;
  for (const value of values) {
    const number = presentFiniteNumber(value);
    if (number == null) {
      continue;
    }
    seen = true;
    sum += number;
  }
  return seen ? sum : null;
}

function sumCategories(rows, categories, accessor) {
  const totals = Object.fromEntries(categories.map((category) => [category, 0]));
  for (const row of rows) {
    const values = accessor(row) ?? {};
    for (const [category, value] of Object.entries(values)) {
      const number = presentFiniteNumber(value);
      if (number == null) {
        continue;
      }
      totals[category] = (totals[category] ?? 0) + number;
    }
  }
  return totals;
}

function resourceAccountingForResult(result) {
  const analysis = result.transcript_analysis ?? {};
  const usage = result.usage ?? {};
  const wallMs = presentFiniteNumber(result.wall_ms);
  const agentRunnerWallMs = presentFiniteNumber(result.agent_runner_wall_ms);
  const baselineHarnessPreludeWallMs = presentFiniteNumber(result.baseline_harness_prelude?.wall_ms);
  const codestoryHarnessPreludeWallMs = presentFiniteNumber(result.codestory_harness_prelude?.wall_ms);
  const preparationWallMs = cachePreparationWallMs(
    result.codestory_cache_provenance?.cache_preparation,
  );
  return {
    measurement_source: "runner_process_wall_clock_codex_jsonl_and_harness_prelude",
    status: result.status ?? null,
    wall_ms: wallMs,
    agent_runner_wall_ms: agentRunnerWallMs,
    baseline_harness_prelude_wall_ms: baselineHarnessPreludeWallMs,
    codestory_harness_prelude_wall_ms: codestoryHarnessPreludeWallMs,
    codestory_cache_preparation_wall_ms: preparationWallMs,
    all_in_wall_ms: wallMs == null ? null : wallMs + (preparationWallMs ?? 0),
    usage: {
      input_tokens: usage.input_tokens ?? null,
      output_tokens: usage.output_tokens ?? null,
      total_tokens: usage.total_tokens ?? null,
      cached_input_tokens: usage.cached_input_tokens ?? null,
      reasoning_tokens: usage.reasoning_tokens ?? null,
    },
    estimated_cost_usd: result.estimated_cost_usd ?? null,
    tool_calls_observed: presentFiniteNumber(result.tool_calls_observed),
    codex_tool_calls_observed: presentFiniteNumber(result.codex_tool_calls_observed),
    tool_categories: analysis.tool_categories ?? {},
    command_count: presentFiniteNumber(analysis.command_count),
    command_categories: analysis.command_categories ?? {},
    external_context_tool_calls: presentFiniteNumber(analysis.external_context_tool_calls) ?? 0,
    direct_source_reads_total: presentFiniteNumber(analysis.direct_source_reads_total),
    ordinary_source_reads_after_first_codestory:
      presentFiniteNumber(analysis.ordinary_source_reads_after_first_codestory),
    ordinary_source_reads_after_first_packet:
      presentFiniteNumber(analysis.ordinary_source_reads_after_first_packet),
  };
}

function summarizeArmCostAccounting(rows) {
  const successful = rows.filter((row) => row.status === "pass");
  const wallMs = sumFinite(rows.map((row) => row.wall_ms));
  const agentRunnerWallMs = sumFinite(
    rows.map((row) => row.agent_runner_wall_ms ?? row.wall_ms),
  );
  const baselineHarnessPreludeWallMs = sumFinite(
    rows.map((row) => row.baseline_harness_prelude?.wall_ms),
  );
  const codestoryHarnessPreludeWallMs = sumFinite(
    rows.map((row) => row.codestory_harness_prelude?.wall_ms),
  );
  const preparationWallMs = sumFinite(
    rows.map((row) => cachePreparationWallMs(row.codestory_cache_provenance?.cache_preparation)),
  );
  return {
    runs: rows.length,
    successful_runs: successful.length,
    failed_runs: rows.filter((row) => row.status === "fail").length,
    timeout_runs: rows.filter((row) => row.status === "timeout").length,
    missing_token_usage_runs: rows.filter((row) => row.usage?.total_tokens == null).length,
    time_spent_ms: {
      runner_wall: wallMs,
      agent_runner: agentRunnerWallMs,
      baseline_harness_prelude: baselineHarnessPreludeWallMs,
      codestory_harness_prelude: codestoryHarnessPreludeWallMs,
      codestory_cache_preparation: preparationWallMs,
      all_in: wallMs + preparationWallMs,
    },
    tokens_spent: {
      input_tokens: sumPresentFinite(rows.map((row) => row.usage?.input_tokens)),
      output_tokens: sumPresentFinite(rows.map((row) => row.usage?.output_tokens)),
      total_tokens: sumPresentFinite(rows.map((row) => row.usage?.total_tokens)),
      cached_input_tokens: sumPresentFinite(rows.map((row) => row.usage?.cached_input_tokens)),
      reasoning_tokens: sumPresentFinite(rows.map((row) => row.usage?.reasoning_tokens)),
    },
    estimated_cost_usd: sumPresentFinite(rows.map((row) => row.estimated_cost_usd)),
    tool_calls: {
      observed: sumFinite(rows.map((row) => row.tool_calls_observed)),
      codex_observed: sumFinite(rows.map((row) => row.codex_tool_calls_observed)),
      categories: sumCategories(
        rows,
        TOOL_ACCOUNTING_CATEGORIES,
        (row) => row.transcript_analysis?.tool_categories,
      ),
    },
    commands: {
      observed: sumFinite(rows.map((row) => row.transcript_analysis?.command_count)),
      categories: sumCategories(
        rows,
        COMMAND_ACCOUNTING_CATEGORIES,
        (row) => row.transcript_analysis?.command_categories,
      ),
    },
    source_reads: {
      direct_source_reads_total: sumFinite(
        rows.map((row) => row.transcript_analysis?.direct_source_reads_total),
      ),
      ordinary_source_reads_after_first_codestory: sumFinite(
        rows.map((row) => row.transcript_analysis?.ordinary_source_reads_after_first_codestory),
      ),
      ordinary_source_reads_after_first_packet: sumFinite(
        rows.map((row) => row.transcript_analysis?.ordinary_source_reads_after_first_packet),
      ),
    },
    external_context_tool_calls: sumFinite(
      rows.map((row) => row.transcript_analysis?.external_context_tool_calls),
    ),
  };
}

function accountingComparison(withValue, withoutValue) {
  const withNumber = presentFiniteNumber(withValue);
  const withoutNumber = presentFiniteNumber(withoutValue);
  return {
    with_codestory: withNumber,
    without_codestory: withoutNumber,
    with_minus_without:
      withNumber == null || withoutNumber == null ? null : withNumber - withoutNumber,
    ratio:
      withNumber == null || withoutNumber == null || withoutNumber <= 0
        ? null
        : withNumber / withoutNumber,
  };
}

function summarizeCostAccounting(results) {
  const byArm = new Map();
  for (const row of results) {
    if (!byArm.has(row.arm)) {
      byArm.set(row.arm, []);
    }
    byArm.get(row.arm).push(row);
  }

  const arms = {};
  for (const [arm, rows] of byArm.entries()) {
    arms[arm] = summarizeArmCostAccounting(rows);
  }

  const withCodeStory = arms.with_codestory ?? null;
  const withoutCodeStory = arms.without_codestory ?? null;
  const withVsWithout =
    withCodeStory && withoutCodeStory
      ? {
          runner_wall_ms: accountingComparison(
            withCodeStory.time_spent_ms.runner_wall,
            withoutCodeStory.time_spent_ms.runner_wall,
          ),
          all_in_wall_ms: accountingComparison(
            withCodeStory.time_spent_ms.all_in,
            withoutCodeStory.time_spent_ms.all_in,
          ),
          total_tokens: accountingComparison(
            withCodeStory.tokens_spent.total_tokens,
            withoutCodeStory.tokens_spent.total_tokens,
          ),
          input_tokens: accountingComparison(
            withCodeStory.tokens_spent.input_tokens,
            withoutCodeStory.tokens_spent.input_tokens,
          ),
          output_tokens: accountingComparison(
            withCodeStory.tokens_spent.output_tokens,
            withoutCodeStory.tokens_spent.output_tokens,
          ),
          tool_calls: accountingComparison(
            withCodeStory.tool_calls.observed,
            withoutCodeStory.tool_calls.observed,
          ),
          commands: accountingComparison(
            withCodeStory.commands.observed,
            withoutCodeStory.commands.observed,
          ),
          estimated_cost_usd: accountingComparison(
            withCodeStory.estimated_cost_usd,
            withoutCodeStory.estimated_cost_usd,
          ),
        }
      : null;

  return {
    measurement_source: "runner_process_wall_clock_codex_jsonl_and_harness_prelude",
    note:
      "Token values are parsed from Codex JSONL stdout. Tool-call and command totals include harness-run baseline and CodeStory preludes when present. Wall time includes the agent runner plus any harness prelude. CodeStory cache preparation is tracked separately and included in all-in wall time.",
    generated_at: new Date().toISOString(),
    arms,
    with_vs_without: withVsWithout,
  };
}

function summarizeRuns(results) {
  const groups = new Map();
  for (const result of results) {
    const key = `${result.repo}\t${result.task_id ?? ""}\t${result.arm}`;
    if (!groups.has(key)) {
      groups.set(key, []);
    }
    groups.get(key).push(result);
  }

  const summaries = [];
  for (const [key, rows] of groups) {
    const [repo, taskId, arm] = key.split("\t");
    const successful = rows.filter((row) => row.status === "pass");
    const qualityRows = successful.filter((row) => row.quality);
    const packetFirstRows = successful.filter((row) => row.packet_first_required);
    const packetManifestRows = successful.filter(
      (row) => row.codestory_harness_prelude?.packet_manifest_quality,
    );
    const categoryMedians = {};
    for (const category of COMMAND_ACCOUNTING_CATEGORIES) {
      categoryMedians[category] = median(
        successful.map((row) => row.transcript_analysis?.command_categories?.[category] ?? 0),
      );
    }
    const toolCategoryMedians = {};
    for (const category of TOOL_ACCOUNTING_CATEGORIES) {
      toolCategoryMedians[category] = median(
        successful.map((row) => row.transcript_analysis?.tool_categories?.[category] ?? 0),
      );
    }
    const totalCodestoryCachePreparationWallMs = sumFinite(
      successful.map((row) => cachePreparationWallMs(row.codestory_cache_provenance?.cache_preparation)),
    );
    const totalWallMs = sumFinite(successful.map((row) => row.wall_ms));
    summaries.push({
      repo,
      task_id: taskId || null,
      task_name: rows[0]?.task_name ?? null,
      task_class: rows[0]?.task_class ?? null,
      arm,
      runs: rows.length,
      successful_runs: successful.length,
      packet_first_pass_runs: packetFirstRows.filter((row) => row.packet_first_pass).length,
      packet_first_required_runs: packetFirstRows.length,
      packet_manifest_quality_pass_runs: packetManifestRows.filter(
        (row) => row.codestory_harness_prelude?.packet_manifest_quality?.pass,
      ).length,
      packet_manifest_quality_scored_runs: packetManifestRows.length,
      packet_partial_runs: successful.filter(
        (row) => row.codestory_harness_prelude?.packet_sufficiency_status === "partial",
      ).length,
      quality_scored_runs: qualityRows.length,
      quality_pass_runs: qualityRows.filter((row) => row.quality?.pass).length,
      total_wall_ms: totalWallMs,
      total_codestory_cache_preparation_wall_ms: totalCodestoryCachePreparationWallMs,
      total_wall_ms_including_codestory_preparation:
        totalWallMs + totalCodestoryCachePreparationWallMs,
      total_input_tokens: sumPresentFinite(successful.map((row) => row.usage?.input_tokens)),
      total_output_tokens: sumPresentFinite(successful.map((row) => row.usage?.output_tokens)),
      total_tokens: sumPresentFinite(successful.map((row) => row.usage?.total_tokens)),
      total_estimated_cost_usd: sumPresentFinite(successful.map((row) => row.estimated_cost_usd)),
      total_tool_calls_observed: sumFinite(successful.map((row) => row.tool_calls_observed)),
      total_command_count: sumFinite(successful.map((row) => row.transcript_analysis?.command_count)),
      total_web_search_tool_calls: sumFinite(
        successful.map((row) => row.transcript_analysis?.tool_categories?.web_search ?? 0),
      ),
      total_direct_source_reads_total: sumFinite(
        successful.map((row) => row.transcript_analysis?.direct_source_reads_total),
      ),
      missing_token_usage_runs: successful.filter((row) => row.usage?.total_tokens == null).length,
      median_wall_ms: median(successful.map((row) => row.wall_ms)),
      median_codestory_cache_preparation_wall_ms: median(
        successful.map((row) => cachePreparationWallMs(row.codestory_cache_provenance?.cache_preparation)),
      ),
      median_codestory_retrieval_index_wall_ms: median(
        successful.map((row) => row.codestory_cache_provenance?.cache_preparation?.retrieval_index_wall_ms),
      ),
      median_total_tokens: median(successful.map((row) => row.usage?.total_tokens)),
      median_input_tokens: median(successful.map((row) => row.usage?.input_tokens)),
      median_output_tokens: median(successful.map((row) => row.usage?.output_tokens)),
      median_estimated_cost_usd: median(successful.map((row) => row.estimated_cost_usd)),
      median_command_count: median(successful.map((row) => row.transcript_analysis?.command_count)),
      median_tool_calls_observed: median(successful.map((row) => row.tool_calls_observed)),
      median_web_search_tool_calls: median(
        successful.map((row) => row.transcript_analysis?.tool_categories?.web_search ?? 0),
      ),
      median_direct_source_reads_total: median(
        successful.map((row) => row.transcript_analysis?.direct_source_reads_total),
      ),
      median_source_reads_after_codestory: median(
        successful.map((row) => row.transcript_analysis?.ordinary_source_reads_after_first_codestory),
      ),
      median_source_reads_after_packet: median(
        successful.map((row) => row.transcript_analysis?.ordinary_source_reads_after_first_packet),
      ),
      median_expected_file_recall: median(
        qualityRows.map((row) => row.quality?.expected_files?.recall),
      ),
      median_expected_symbol_recall: median(
        qualityRows.map((row) => row.quality?.expected_symbols?.recall),
      ),
      median_expected_claim_recall: median(
        qualityRows.map((row) => row.quality?.expected_claims?.recall),
      ),
      median_citation_coverage: median(
        qualityRows.map((row) => row.quality?.citation_coverage?.recall),
      ),
      median_repository_context_output_chars: median(
        successful.map((row) => repositoryContextOutputChars(row.transcript_analysis)),
      ),
      median_useful_anchor_hits_per_10k_context_chars: median(
        qualityRows.map((row) => usefulAnchorHitsPer10kContextChars(row)),
      ),
      median_command_categories: categoryMedians,
      median_tool_categories: toolCategoryMedians,
      total_command_categories: sumCategories(
        successful,
        COMMAND_ACCOUNTING_CATEGORIES,
        (row) => row.transcript_analysis?.command_categories,
      ),
      total_tool_categories: sumCategories(
        successful,
        TOOL_ACCOUNTING_CATEGORIES,
        (row) => row.transcript_analysis?.tool_categories,
      ),
    });
  }
  return summaries;
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

function repositoryContextOutputChars(analysis) {
  const byCategory = analysis?.output_chars_by_category ?? {};
  return (
    (byCategory.codestory_cli ?? 0) +
    (byCategory.shell_search ?? 0) +
    (byCategory.direct_file_read ?? 0) +
    (byCategory.git ?? 0)
  );
}

function usefulAnchorHitsPer10kContextChars(row) {
  const hits = row.quality?.expected_anchors?.found;
  if (hits == null) {
    return null;
  }
  const contextChars = repositoryContextOutputChars(row.transcript_analysis);
  return hits / Math.max(1, contextChars / 10_000);
}

function agentPublishableBlockers(results, opts = {}) {
  const maxSourceReadsAfterPacket = opts.maxSourceReadsAfterPacket;
  const enforceRepoProvenance = Boolean(opts.publishable || opts.enforceRepoProvenance);
  return results
    .flatMap((result) => {
      const productReasons = [];
      const harnessReasons = [];
      const environmentReasons = [];
      if (result.status !== "pass") {
        productReasons.push(`status=${result.status}`);
      }
      if (presentFiniteNumber(result.wall_ms) == null) {
        harnessReasons.push("missing wall time");
      }
      if (result.usage?.total_tokens == null) {
        harnessReasons.push("missing total token usage");
      }
      if (presentFiniteNumber(result.tool_calls_observed) == null) {
        harnessReasons.push("missing tool call count");
      }
      if (presentFiniteNumber(result.transcript_analysis?.command_count) == null) {
        harnessReasons.push("missing command count");
      }
      if (
        result.arm === "without_codestory" &&
        (result.transcript_analysis?.command_categories?.codestory_cli ?? 0) > 0
      ) {
        environmentReasons.push("without_codestory arm used CodeStory");
      }
      if (
        result.arm === "without_codestory" &&
        result.task_id &&
        (presentFiniteNumber(result.transcript_analysis?.command_count) ?? 0) <= 0
      ) {
        productReasons.push("without_codestory arm did not inspect local repository");
      }
      if (result.packet_first_required && !result.packet_first_pass) {
        productReasons.push("missing answer packet as first successful context command");
      }
      if (
        opts.publishable &&
        result.arm === "with_codestory" &&
        result.packet_first_required &&
        maxSourceReadsAfterPacket == null
      ) {
        harnessReasons.push("missing explicit post-packet source-read budget");
      }
      const packetExtraProbeStrategy =
        result.codestory_harness_prelude?.packet_extra_probe_strategy ??
        result.packet_extra_probe_strategy ??
        null;
      if (opts.publishable && result.arm === "with_codestory" && packetExtraProbeStrategy) {
        harnessReasons.push(`diagnostic packet extra probes used: ${packetExtraProbeStrategy}`);
      }
      for (const { label, prelude } of [
        { label: "baseline", prelude: result.baseline_harness_prelude },
        { label: "codestory", prelude: result.codestory_harness_prelude },
      ]) {
        if (!prelude) {
          continue;
        }
        if (prelude.status !== "pass") {
          harnessReasons.push(`${label} prelude status=${prelude.status ?? "unknown"}; expected pass`);
        }
        if (prelude.packet_manifest_quality && !prelude.packet_manifest_quality.pass) {
          productReasons.push(`${label} prelude packet manifest quality failed`);
        }
        const preludeSufficiency =
          prelude.packet_sufficiency ??
          (prelude.packet_sufficiency_status
            ? { status: prelude.packet_sufficiency_status }
            : null);
        if (preludeSufficiency) {
          addPacketSufficiencyPublishableReasons(
            preludeSufficiency,
            productReasons,
            harnessReasons,
            `${label} prelude packet`,
          );
        }
        if (!prelude.packet_sufficiency) {
          const unresolvedCandidates = presentFiniteNumber(
            prelude.packet_latency?.retrieval_shadow?.unresolved_candidate_count,
          );
          if (unresolvedCandidates > 0) {
            productReasons.push(
              `${label} prelude packet unresolved retrieval candidates=${unresolvedCandidates}; expected 0`,
            );
          }
        }
        const preludeRetrieval =
          prelude.packet_sufficiency ??
          prelude.packet_latency?.retrieval_shadow ??
          null;
        if (preludeRetrieval?.retrieval_mode && preludeRetrieval.retrieval_mode !== "full") {
          environmentReasons.push(
            `${label} prelude packet retrieval mode=${preludeRetrieval.retrieval_mode}; expected full`,
          );
        }
        if (preludeRetrieval?.degraded_reason) {
          environmentReasons.push(
            `${label} prelude packet retrieval degraded=${preludeRetrieval.degraded_reason}`,
          );
        }
      }
      if (result.task_id && !result.quality) {
        harnessReasons.push("missing manifest quality score");
      }
      if (result.quality && !result.quality.pass) {
        productReasons.push("manifest quality failed");
      }
      const readsAfterPacket = result.transcript_analysis?.ordinary_source_reads_after_first_packet;
      if (
        result.packet_first_required &&
        maxSourceReadsAfterPacket != null &&
        readsAfterPacket != null &&
        readsAfterPacket > maxSourceReadsAfterPacket
      ) {
        productReasons.push(`ordinary source reads after packet=${readsAfterPacket} > ${maxSourceReadsAfterPacket}`);
      }
      if (enforceRepoProvenance) {
        environmentReasons.push(...repoProvenanceBlockers(result));
      }
      const externalContextCalls = result.transcript_analysis?.external_context_tool_calls ?? 0;
      if (externalContextCalls > 0) {
        environmentReasons.push(`external web/search tool calls=${externalContextCalls} > 0`);
      }
      if (result.arm === "with_codestory" && (opts.publishable || opts.enforceCacheProvenance)) {
        environmentReasons.push(...cacheProvenanceBlockers(result));
      }
      return [
        productReasons.length ? { result, category: "product", reasons: productReasons } : null,
        harnessReasons.length ? { result, category: "harness-contract", reasons: harnessReasons } : null,
        environmentReasons.length ? { result, category: "environment", reasons: environmentReasons } : null,
      ];
    })
    .filter(Boolean);
}

function markdownSummary(summary, opts, costAccounting = null) {
  const lines = [
    "# CodeStory Agent A/B Benchmark",
    "",
    `Runner: \`${opts.runner}\``,
    opts.model ? `Model: \`${opts.model}\`` : "Model: runner default",
    `Sandbox: \`${opts.sandbox}\``,
    `Host: \`${os.hostname()}\``,
    "",
  ];
  if (costAccounting) {
    lines.push(...markdownCostAccounting(costAccounting), "");
  }
  lines.push(
    "## Per-task Summary",
    "",
    "| Repo | Task | Arm | Runs | Success | Packet first | Packet manifest | Quality pass | Median wall ms | CodeStory prep ms | Retrieval index ms | Median tokens | Median cost USD | Median tool calls | Web searches | Median commands | CodeStory cmds | Shell searches | File-read cmds | Source reads | After CodeStory | After Packet | File recall | Citation coverage | Context chars | Useful anchors / 10k context chars |",
    "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  );
  for (const row of summary) {
    lines.push(markdownSummaryRow(row));
  }
  lines.push(
    "",
    "Raw stdout/stderr files and the JSONL run ledger in this directory are the source of truth.",
    "Do not promote token or cost claims when token usage is blank.",
    "",
  );
  return lines.join("\n");
}

function markdownCostAccounting(costAccounting) {
  const lines = [
    "## Cost Accounting",
    "",
    "| Arm | Runs | Success | Wall ms | Agent runner ms | Baseline prelude ms | CodeStory prelude ms | All-in wall ms | Input tokens | Output tokens | Total tokens | Tool calls | Codex tool calls | Commands | Web searches | Source reads | Est. cost USD |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const [arm, row] of Object.entries(costAccounting.arms ?? {})) {
    lines.push(
      `| ${arm} | ${row.runs} | ${row.successful_runs} | ${formatValue(row.time_spent_ms?.runner_wall)} | ${formatValue(row.time_spent_ms?.agent_runner)} | ${formatValue(row.time_spent_ms?.baseline_harness_prelude)} | ${formatValue(row.time_spent_ms?.codestory_harness_prelude)} | ${formatValue(row.time_spent_ms?.all_in)} | ${formatValue(row.tokens_spent?.input_tokens)} | ${formatValue(row.tokens_spent?.output_tokens)} | ${formatValue(row.tokens_spent?.total_tokens)} | ${formatValue(row.tool_calls?.observed)} | ${formatValue(row.tool_calls?.codex_observed)} | ${formatValue(row.commands?.observed)} | ${formatValue(row.tool_calls?.categories?.web_search)} | ${formatValue(row.source_reads?.direct_source_reads_total)} | ${formatValue(row.estimated_cost_usd)} |`,
    );
  }
  const comparison = costAccounting.with_vs_without;
  if (comparison) {
    lines.push(
      "",
      "| Comparison | With | Without | Delta | Ratio |",
      "| --- | ---: | ---: | ---: | ---: |",
    );
    for (const [label, values] of Object.entries(comparison)) {
      lines.push(
        `| ${label} | ${formatValue(values.with_codestory)} | ${formatValue(values.without_codestory)} | ${formatValue(values.with_minus_without)} | ${formatValue(values.ratio)} |`,
      );
    }
  }
  lines.push(
    "",
    "Accounting source: wall time includes the agent runner and any harness-run baseline or CodeStory prelude; tokens are parsed from Codex JSONL stdout; tool-call and command totals include harness preludes when present; CodeStory cache preparation is tracked separately and included in all-in wall time.",
  );
  return lines;
}

function markdownSummaryRow(row) {
  const cells = [
    row.repo,
    row.task_id ?? "",
    row.arm,
    row.runs,
    row.successful_runs,
    packetFirstLabel(row),
    packetManifestLabel(row),
    qualityPassLabel(row),
    formatValue(row.median_wall_ms),
    formatValue(row.median_codestory_cache_preparation_wall_ms),
    formatValue(row.median_codestory_retrieval_index_wall_ms),
    formatValue(row.median_total_tokens),
    formatValue(row.median_estimated_cost_usd),
    formatValue(row.median_tool_calls_observed),
    formatValue(row.median_web_search_tool_calls),
    formatValue(row.median_command_count),
    formatValue(row.median_command_categories?.codestory_cli),
    formatValue(row.median_command_categories?.shell_search),
    formatValue(row.median_command_categories?.direct_file_read),
    formatValue(row.median_direct_source_reads_total),
    formatValue(row.median_source_reads_after_codestory),
    formatValue(row.median_source_reads_after_packet),
    formatPercent(row.median_expected_file_recall),
    formatPercent(row.median_citation_coverage),
    formatValue(row.median_repository_context_output_chars),
    formatValue(row.median_useful_anchor_hits_per_10k_context_chars),
  ];
  return `| ${cells.join(" | ")} |`;
}

function qualityPassLabel(row) {
  if (!row.quality_scored_runs) {
    return "";
  }
  return `${row.quality_pass_runs}/${row.quality_scored_runs}`;
}

function packetFirstLabel(row) {
  if (!row.packet_first_required_runs) {
    return "";
  }
  return `${row.packet_first_pass_runs}/${row.packet_first_required_runs}`;
}

function packetManifestLabel(row) {
  if (!row.packet_manifest_quality_scored_runs) {
    return "";
  }
  const partialSuffix = row.packet_partial_runs ? `; partial ${row.packet_partial_runs}` : "";
  return `${row.packet_manifest_quality_pass_runs}/${row.packet_manifest_quality_scored_runs}${partialSuffix}`;
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

function formatPercent(value) {
  if (value == null) {
    return "";
  }
  return `${Math.round(value * 1000) / 10}%`;
}

function commandEvent(id, type, command, aggregatedOutput = "", exitCode = 0) {
  return {
    type,
    item: {
      id,
      type: "command_execution",
      command,
      aggregated_output: aggregatedOutput,
      exit_code: type.endsWith(".completed") ? exitCode : null,
      status: type.endsWith(".completed") ? "completed" : "in_progress",
    },
  };
}

function runSelfTest() {
  const fixtureEvents = [
    { type: "thread.started" },
    { type: "turn.started" },
    commandEvent("cmd_1", "item.started", "& $cli packet --project . --question flow"),
    commandEvent(
      "cmd_1",
      "item.completed",
      "& $cli packet --project . --question flow",
      "Evidence: crates/codestory-cli/src/main.rs RuntimeContext::ensure_open full indexing",
    ),
    commandEvent("cmd_2", "item.started", "rg -n \"run_index\" crates"),
    commandEvent("cmd_2", "item.completed", "rg -n \"run_index\" crates", "crates/codestory-cli/src/main.rs:1:run_index"),
    commandEvent("cmd_3", "item.started", "Get-Content crates/codestory-cli/src/main.rs"),
    commandEvent("cmd_3", "item.completed", "Get-Content crates/codestory-cli/src/main.rs", "fn run_index() {}"),
    commandEvent("cmd_4", "item.started", "Get-Content crates/codestory-cli/src/main.rs"),
    commandEvent("cmd_4", "item.completed", "Get-Content crates/codestory-cli/src/main.rs", "fn run_index() {}"),
    {
      type: "item.completed",
      item: {
        id: "msg_1",
        type: "agent_message",
        text: "Full indexing starts in crates/codestory-cli/src/main.rs and calls RuntimeContext::ensure_open.",
      },
    },
    { type: "turn.completed", usage: { input_tokens: 10, output_tokens: 5 } },
  ];

  const analysis = analyzeTranscript(fixtureEvents);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.command_categories.shell_search, 1);
  assert.equal(analysis.command_categories.direct_file_read, 2);
  assert.equal(analysis.direct_source_reads_total, 2);
  assert.equal(analysis.ordinary_source_reads_after_first_codestory, 2);
  assert.equal(analysis.ordinary_source_reads_after_first_packet, 2);
  assert.equal(analysis.direct_file_reads_duplicated["crates/codestory-cli/src/main.rs"], 2);

  const quality = scoreQuality(fixtureEvents, {
    id: "fixture",
    task_class: "architecture_explanation",
    expected_files: ["crates/codestory-cli/src/main.rs"],
    expected_symbols: ["RuntimeContext::ensure_open"],
    expected_claims: ["Full indexing starts"],
    forbidden_claims: ["unsupported claim"],
    quality_thresholds: {
      expected_file_recall: 1,
      expected_symbol_recall: 1,
      expected_claim_recall: 1,
      citation_coverage: 1,
    },
  });
  assert.equal(quality.pass, true);
  assert.equal(quality.expected_files.recall, 1);
  assert.equal(quality.expected_symbols.recall, 1);
  assert.equal(quality.expected_claims.recall, 1);
  assert.equal(quality.citation_coverage.recall, 1);
  const packetFixture = {
    budget: {
      used: { output_bytes: 123 },
      limits: { max_output_bytes: 456 },
      truncated: false,
    },
    sufficiency: {
      status: "sufficient",
      covered_claims: [{ claim: "covered" }],
      open_next: [],
      avoid_opening: ["crates/codestory-cli/src/main.rs because already cited"],
      avoid_opening_paths: ["crates/codestory-cli/src/main.rs"],
      gaps: [],
      follow_up_commands: [],
    },
    answer: {
      citations: [{ display_name: "run_index" }],
      graphs: [{ id: "g", edges: [{ id: "e1" }] }],
      freshness: { duration_ms: 10 },
      retrieval_trace: {
        total_latency_ms: 100,
        annotations: [
          "packet_anchor_probe_batch total_ms=25 attributed_query_ms=20 overhead_ms=5 queries=2",
          "packet_lexical_subquery_batch total_ms=40 attributed_query_ms=31 overhead_ms=9 queries=3",
          "packet_non_trace_phase label=budget duration_ms=7",
          "packet_non_trace_phase label=sufficiency duration_ms=11",
          "packet_non_trace_phase label=packet_dto duration_ms=2",
          "packet_non_trace_phase label=output_budget duration_ms=5",
          "packet_stdio_phase label=text_materialization duration_ms=3",
          "packet_stdio_phase label=tool_response_materialization duration_ms=4",
        ],
        steps: [],
      },
      sections: [{ blocks: [{ markdown: "answer" }] }],
    },
  };
  assert.equal(packetShape(packetFixture).budget_used_output_bytes, 123);
  assert.equal(packetShape(packetFixture).graph_bytes > 2, true);
  assert.equal(
    packetSufficiencyTelemetry(packetFixture, { pass: false }).sufficient_quality_mismatch,
    true,
  );
  const packetLatency = packetLatencyTelemetry(packetFixture, 150);
  assert.equal(packetLatency.accounted_trace_ms, 110);
  assert.equal(packetLatency.non_trace_wall_ms, 40);
  assert.equal(packetLatency.packet_batch_total_ms, 65);
  assert.equal(packetLatency.packet_batch_attributed_query_ms, 51);
  assert.equal(packetLatency.packet_batch_overhead_ms, 14);
  assert.equal(packetLatency.packet_anchor_probe_batch_overhead_ms, 5);
  assert.equal(packetLatency.packet_lexical_subquery_batch_overhead_ms, 9);
  assert.equal(packetLatency.packet_non_trace_phase_total_ms, 25);
  assert.equal(packetLatency.packet_budget_ms, 7);
  assert.equal(packetLatency.packet_sufficiency_ms, 11);
  assert.equal(packetLatency.packet_dto_ms, 2);
  assert.equal(packetLatency.packet_output_budget_ms, 5);
  assert.equal(packetLatency.packet_stdio_phase_total_ms, 7);
  assert.equal(packetLatency.packet_stdio_text_materialization_ms, 3);
  assert.equal(packetLatency.packet_stdio_tool_response_materialization_ms, 4);
  const reviewerRuntimeSummary = summarizePacketRuntimeRuns([
    {
      repo: "repo",
      task_id: "task",
      mode: "warm_stdio_packet",
      status: "pass",
      wall_ms: 120,
      warm_stdio_packet_cache_hit: false,
      packet_latency: {
        retrieval_total_ms: 80,
        accounted_trace_ms: 90,
        unaccounted_ms: 30,
        packet_batch_overhead_ms: 12,
        sla_missed: false,
      },
    },
    {
      repo: "repo",
      task_id: "task",
      mode: "warm_stdio_packet",
      status: "pass",
      wall_ms: 30,
      warm_stdio_packet_cache_hit: true,
      packet_latency: {
        retrieval_total_ms: 8,
        accounted_trace_ms: 10,
        unaccounted_ms: 20,
        packet_batch_overhead_ms: 2,
        sla_missed: false,
      },
    },
  ])[0];
  assert.equal(reviewerRuntimeSummary.median_e2e_wall_ms, 75);
  assert.equal(reviewerRuntimeSummary.median_trace_sla_retrieval_ms, 44);
  assert.equal(reviewerRuntimeSummary.median_warm_first_hit_wall_ms, 120);
  assert.equal(reviewerRuntimeSummary.median_warm_cache_hit_wall_ms, 30);
  const runtimeDeltas = buildPacketRuntimeDeltas([reviewerRuntimeSummary], [
    {
      ...reviewerRuntimeSummary,
      median_e2e_wall_ms: 100,
      median_trace_sla_retrieval_ms: 60,
      packet_sla_missed_runs: 1,
      median_packet_unaccounted_ms: 40,
      median_warm_first_hit_wall_ms: 150,
      median_warm_cache_hit_wall_ms: 45,
      median_packet_batch_overhead_ms: 20,
    },
  ]);
  assert.equal(runtimeDeltas.tasks[0].deltas.packet_sla_missed_runs.delta, -1);
  assert.equal(runtimeDeltas.tasks[0].deltas.median_trace_sla_retrieval_ms.delta, -16);
  assert.equal(runtimeDeltas.tasks[0].deltas.median_warm_cache_hit_wall_ms.delta, -15);
  assert.equal(
    packetRuntimeArtifactManifest({
      outDir: "target/agent-benchmark/focused-run",
      benchmarkId: "focused-run",
      artifactPaths: { summary: "packet-runtime-summary.json" },
    }).durable_copy_convention.suggested_stable_directory,
    "target/agent-benchmark/focused-run",
  );
  const serverPhase = parseStdioServerPhaseLine(
    'packet_stdio_server_phase request_id="java-commons-lang-string-utils-1" label=response_serialization duration_ms=12',
  );
  assert.deepEqual(serverPhase, {
    request_id: '"java-commons-lang-string-utils-1"',
    label: "response_serialization",
    duration_ms: 12,
  });
  const serverTransport = stdioServerPhaseTransportTimings([
    { label: "response_serialization", duration_ms: 12 },
    { label: "newline_write", duration_ms: 1 },
    { label: "flush", duration_ms: 2 },
  ]);
  assert.equal(serverTransport.stdio_server_output_total_ms, 15);
  assert.equal(serverTransport.stdio_server_response_serialization_ms, 12);
  assert.equal(serverTransport.stdio_server_newline_write_ms, 1);
  assert.equal(serverTransport.stdio_server_flush_ms, 2);
  assert.equal(preludeAllowsAgentRun({ status: "pass_with_warnings" }), true);
  assert.equal(preludeAllowsAgentRun({ status: "pass_with_warnings" }, { publishable: true }), false);
  const weakPacketTelemetry = packetSufficiencyTelemetry(
    {
      sufficiency: {
        status: "partial",
        covered_claims: [],
        gaps: ["missing route proof"],
        open_next: ["inspect route"],
        follow_up_commands: ["codestory-cli search --query route"],
      },
      coverage_report: {
        unresolved: ["route handler"],
      },
      benchmark_trace: {
        retrieval_trace: {
          retrieval_shadow: {
            retrieval_mode: "full",
            unresolved_candidate_count: 2,
          },
        },
      },
    },
    { pass: true },
  );
  assert.equal(weakPacketTelemetry.follow_up_commands_count, 1);
  assert.equal(weakPacketTelemetry.unresolved_candidate_count, 2);
  assert.equal(weakPacketTelemetry.coverage_unresolved_count, 1);
  assert.equal(weakPacketTelemetry.coverage_unresolved_blocking_count, 1);
  assert.deepEqual(
    packetRuntimePublishableBlockers([
      { status: "pass", quality: { pass: true } },
      { status: "pass", quality: null },
      { status: "pass", quality: { pass: false } },
      {
        status: "pass",
        quality: { pass: true },
        sufficiency: { sufficient_quality_mismatch: true },
      },
      { status: "fail", quality: { pass: true } },
    ]).map((blocker) => {
      const row = blocker.result;
      return row.status === "pass" ? row.quality?.pass ?? null : row.status;
    }),
    [null, false, true, "fail"],
  );
  assert.deepEqual(
    packetRuntimePublishableBlockers(
      [
        {
          repo: "repo",
          task_id: "task",
          mode: "cold_cli_packet",
          repeat: 1,
          status: "pass",
          quality: { pass: true },
          sufficiency: weakPacketTelemetry,
          packet_latency: {
            sla_missed: false,
            retrieval_shadow: { retrieval_mode: "full" },
          },
        },
      ],
      { enforcePacketRuntimeTelemetry: true },
    ).map((blocker) => blocker.category),
    ["product"],
  );
  assert.deepEqual(
    agentPublishableBlockers([
      {
        repo: "repo",
        task_id: "task",
        arm: "with_codestory",
        repeat: 1,
        status: "pass",
        wall_ms: 1,
        usage: { total_tokens: 1 },
        tool_calls_observed: 1,
        transcript_analysis: {
          command_count: 1,
          external_context_tool_calls: 0,
          ordinary_source_reads_after_first_packet: 0,
        },
        packet_first_required: true,
        packet_first_pass: true,
        quality: { pass: true },
        codestory_harness_prelude: {
          status: "pass_with_warnings",
          packet_sufficiency_status: "partial",
          packet_manifest_quality: { pass: false },
          packet_latency: {
            retrieval_shadow: {
              retrieval_mode: "full",
              unresolved_candidate_count: 2,
            },
          },
        },
      },
    ]).map((blocker) => blocker.category),
    ["product", "harness-contract"],
  );
  assert.equal(packetRuntimeQualityGateRequired({ taskSuite: "holdout-retrieval" }), true);
  assert.equal(packetRuntimeQualityGateRequired({ taskSuite: "language-expansion-holdout" }), true);
  assert.equal(
    packetRuntimeQualityGateRequired({
      taskSuite: "language-expansion-holdout",
      allowFailures: true,
    }),
    false,
  );
  assert.deepEqual(
    baselineSearchPreludeStatus(
      {
        exitCode: 2,
        stderr: "rg: .\\missing: The system cannot find the path specified. (os error 3)\n",
      },
      [{ path: "src/main.rb", line: 1, column: 1, text: "build" }],
    ),
    {
      allowed: true,
      status: "pass_with_warnings",
      warning_lines: [
        "rg: .\\missing: The system cannot find the path specified. (os error 3)",
      ],
    },
  );
  assert.equal(packetRuntimeQualityGateRequired({ taskSuite: "local-real" }), false);
  assert.equal(
    cachePreparationAction({
      status: "pass",
      indexed: true,
      freshness_status: "stale",
      semantic_ready: false,
    }),
    "retrieval-index-auto",
  );
  assert.equal(
    cachePreparationAction({
      status: "pass",
      indexed: true,
      freshness_status: "fresh",
      semantic_ready: true,
    }),
    "already-ready",
  );
  const packetRuntimePreparation = [
    {
      repo: "codestory",
      retrieval_status: { retrieval_mode: "full" },
    },
  ];
  for (const transportMode of ["cold_cli_packet", "warm_stdio_packet"]) {
    const observations = packetRuntimeCacheObservations(
      { cachePreparationByRepo: packetRuntimePreparation },
      "codestory",
      transportMode,
    );
    assert.equal(cachePolicyForRun(observations), "prepared-sidecar-cache-read-only");
    assert.equal(observations.cache_preparation, packetRuntimePreparation[0]);
  }

  const plannedAgentRuns = planAgentRuns(
    { arms: ["without_codestory", "with_codestory"], repeats: 1, repos: null },
    [
      { id: "task-a", repo: "repo-a" },
      { id: "task-b", repo: "repo-b" },
      { id: "task-c", repo: "repo-a" },
    ],
  );
  const plannedGroups = groupPlannedAgentRuns(plannedAgentRuns);
  assert.deepEqual(
    plannedGroups.map((group) => group.key),
    ["repo-a", "repo-b"],
  );
  assert.deepEqual(
    plannedGroups[0].runs.map((run) => `${run.task.id}:${run.arm}`),
    [
      "task-a:without_codestory",
      "task-a:with_codestory",
      "task-c:without_codestory",
      "task-c:with_codestory",
    ],
  );

  console.log("self-test passed");
}

function planAgentRuns(opts, tasks) {
  const plannedRuns = [];
  if (tasks.length) {
    for (const task of tasks) {
      for (const arm of opts.arms) {
        for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
          plannedRuns.push({ repo: task.repo, arm, repeat, task });
        }
      }
    }
  } else {
    for (const repo of opts.repos) {
      for (const arm of opts.arms) {
        for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
          plannedRuns.push({ repo, arm, repeat, task: null });
        }
      }
    }
  }
  return plannedRuns;
}

function agentRunKey(run) {
  const taskId = run.task?.id ?? run.task_id ?? "";
  return [run.repo, taskId, run.arm, String(run.repeat)].join("\t");
}

function agentRunIsolationGroupKey(run) {
  return run.repo;
}

function groupPlannedAgentRuns(plannedRuns) {
  const groupsByKey = new Map();
  for (const run of plannedRuns) {
    const key = agentRunIsolationGroupKey(run);
    if (!groupsByKey.has(key)) {
      groupsByKey.set(key, { key, runs: [] });
    }
    groupsByKey.get(key).runs.push(run);
  }
  return [...groupsByKey.values()];
}

function taskSnapshotMatches(currentTask, candidate) {
  const current = taskSnapshotForResult(currentTask);
  const previous = candidate?.task_manifest_snapshot ?? null;
  return JSON.stringify(current ?? null) === JSON.stringify(previous ?? null);
}

function benchmarkContractForRun(opts, run, env = process.env) {
  return benchmarkRunContract({
    opts,
    task: run.task ?? null,
    env,
    harnessPath: benchmarkHarnessPath,
    scorerPath: benchmarkScorerPath,
    cliIdentity: run.arm === "with_codestory" ? opts.codestoryCli ?? env.CODESTORY_CLI ?? null : null,
  });
}

function resolveRunArtifactPath(runDir, artifactPath) {
  if (!artifactPath) {
    return null;
  }
  const artifactText = String(artifactPath).trim();
  if (!artifactText || path.isAbsolute(artifactText)) {
    return null;
  }
  if (!REUSABLE_BASELINE_ARTIFACT_NAME_PATTERN.test(path.basename(artifactText))) {
    return null;
  }
  const resolved = path.resolve(runDir, artifactText);
  return isPathInside(runDir, resolved) ? resolved : null;
}

async function copyResultArtifact(runDir, outDir, artifactPath, nextName) {
  const source = resolveRunArtifactPath(runDir, artifactPath);
  if (!source) {
    return null;
  }
  if (!existsSync(source)) {
    return artifactPath ?? null;
  }
  const sourceStat = statSync(source);
  if (!sourceStat.isFile()) {
    return null;
  }
  if (sourceStat.size > MAX_REUSED_ARTIFACT_BYTES) {
    throw new Error(
      `Refusing to reuse oversized baseline artifact ${source}: ${sourceStat.size} bytes exceeds ${MAX_REUSED_ARTIFACT_BYTES}`,
    );
  }
  const destination = path.join(outDir, nextName);
  await copyFile(source, destination);
  return destination;
}

async function copyReusableBaselineArtifacts(row, sourceRunDir, outDir, runId) {
  const copied = {
    ...row,
    stdout_path: await copyResultArtifact(sourceRunDir, outDir, row.stdout_path, `${runId}.stdout.jsonl`),
    stderr_path: await copyResultArtifact(sourceRunDir, outDir, row.stderr_path, `${runId}.stderr.txt`),
  };
  if (copied.baseline_harness_prelude?.context_path) {
    copied.baseline_harness_prelude = {
      ...copied.baseline_harness_prelude,
      context_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        copied.baseline_harness_prelude.context_path,
        `${runId}.baseline-context.json`,
      ),
      stderr_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        copied.baseline_harness_prelude.stderr_path,
        `${runId}.baseline-context.stderr.txt`,
      ),
    };
  }
  return copied;
}

async function loadReusableBaselines(opts, plannedRuns, outDir) {
  if (!opts.reuseBaselineFrom) {
    return new Map();
  }
  const sourceRunDir = path.resolve(opts.reuseBaselineFrom);
  const runsPath = path.join(sourceRunDir, "runs.jsonl");
  if (!existsSync(runsPath)) {
    throw new Error(`--reuse-baseline-from must contain runs.jsonl: ${sourceRunDir}`);
  }
  const wanted = new Map(
    plannedRuns
      .filter((run) => run.arm === "without_codestory")
      .map((run) => [agentRunKey(run), run]),
  );
  if (!wanted.size) {
    return new Map();
  }

  const rows = (await readFile(runsPath, "utf8"))
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const taskCache = new Map();
  const reusable = new Map();
  for (const row of rows) {
    if (row.arm !== "without_codestory") {
      continue;
    }
    const key = agentRunKey(row);
    const planned = wanted.get(key);
    if (!planned || !taskSnapshotMatches(planned.task, row)) {
      continue;
    }
    const reanalyzed = await recomputeRunAnalysis(row, opts, sourceRunDir, taskCache);
    const currentContract = benchmarkContractForRun(opts, planned);
    const compatibility = benchmarkContractCompatibility(
      currentContract,
      reanalyzed.benchmark_contract,
    );
    if (!compatibility.compatible) {
      throw new Error(
        [
          `Refusing to reuse incompatible baseline row for ${planned.repo} ${planned.task?.id ?? ""} repeat ${planned.repeat}.`,
          ...compatibility.mismatches,
        ].join(" "),
      );
    }
    const runId = benchmarkRunId([
      planned.repo,
      ...(planned.task ? [planned.task.id] : []),
      planned.arm,
      String(planned.repeat).padStart(2, "0"),
    ]);
    const copied = await copyReusableBaselineArtifacts(reanalyzed, sourceRunDir, outDir, runId);
    reusable.set(key, {
      ...copied,
      reused_from: sourceRunDir,
      reused_from_run_id: row.benchmark_run_id ?? null,
      reused_at: new Date().toISOString(),
      benchmark_contract: {
        ...currentContract,
        reused_from: sourceRunDir,
        reused_from_run_id: row.benchmark_run_id ?? null,
        promotion_eligible: true,
      },
      promotion_eligible: true,
      resource_accounting: resourceAccountingForResult(copied),
    });
  }
  return reusable;
}

async function runPlannedAgentRun(opts, run, outDir, reusableBaselines) {
  const reusable = reusableBaselines.get(agentRunKey(run));
  if (reusable) {
    console.log(`reusing ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats} from ${opts.reuseBaselineFrom}`);
    return reusable;
  }
  console.log(`running ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats}`);
  return await runOne(opts, run, outDir);
}

async function runPlannedAgentRuns(opts, plannedRuns, reusableBaselines, outDir) {
  const runsPath = path.join(outDir, "runs.jsonl");
  if (opts.jobs <= 1 || plannedRuns.length <= 1) {
    const results = [];
    for (const run of plannedRuns) {
      results.push(await runPlannedAgentRun(opts, run, outDir, reusableBaselines));
      await writeJsonlRows(runsPath, results);
    }
    return results;
  }

  const groups = groupPlannedAgentRuns(plannedRuns);
  console.log(`running ${plannedRuns.length} planned agent rows across ${groups.length} repo groups with --jobs ${opts.jobs}`);
  const groupedResults = await parallelMap(groups, opts.jobs, async (group) => {
    const rows = [];
    for (const run of group.runs) {
      rows.push(await runPlannedAgentRun(opts, run, outDir, reusableBaselines));
    }
    return rows;
  });
  const results = groupedResults.flat();
  await writeJsonlRows(runsPath, results);
  return results;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.selfTest) {
    runSelfTest();
    return;
  }
  if (opts.reanalyzeDir) {
    await reanalyzeAgentRunDirectory(opts);
    return;
  }
  const tasks = await loadTasks(opts);
  if (opts.publishable) {
    validatePublishableShape(opts, tasks);
  }
  if (opts.materializeRepos) {
    assertManifestRepoMaterializationAllowed(tasks, opts);
    await materializeRepos(tasks, opts);
  }
  if (opts.list) {
    if (tasks.length) {
      for (const task of tasks) {
        const config = ALL_REPOS[task.repo];
        const availability = existsSync(config.path) ? "available" : "missing";
        console.log(`${task.id}\t${task.suite ?? ""}\t${task.repo}\t${availability}\t${config.path}\t${task.prompt}`);
      }
    } else {
      for (const [name, config] of Object.entries(ALL_REPOS)) {
        const availability = existsSync(config.path) ? "available" : "missing";
        const scope = PUBLIC_REPOS[name] ? "public" : "local";
        console.log(`${name}\t${scope}\t${availability}\t${config.path}\t${config.prompt}`);
      }
    }
    return;
  }

  if (opts.packetRuntime) {
    await runPacketRuntimeBenchmark(opts, tasks);
    return;
  }

  if (tasks.length && opts.repos) {
    const allowed = new Set(opts.repos);
    for (const task of tasks) {
      if (!allowed.has(task.repo)) {
        throw new Error(`Task '${task.id}' repo '${task.repo}' is not included by --repos`);
      }
    }
  }

  const plannedRuns = planAgentRuns(opts, tasks);
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const outDir = path.resolve(opts.outDir ?? path.join(repoRoot, "target", "agent-benchmark", timestamp));
  await mkdir(outDir, { recursive: true });
  const reusableBaselines = await loadReusableBaselines(opts, plannedRuns, outDir);
  const cachePreparation = opts.prepareCodestoryCache
    ? await prepareCodeStoryCaches(opts, tasks)
    : [];
  opts.cachePreparationByRepo = new Map(cachePreparation.map((row) => [row.repo, row]));
  if (cachePreparation.length) {
    await writeFile(
      path.join(outDir, "codestory-cache-preparation.json"),
      `${JSON.stringify(cachePreparation, null, 2)}\n`,
      "utf8",
    );
  }

  const results = await runPlannedAgentRuns(opts, plannedRuns, reusableBaselines, outDir);

  const summary = summarizeRuns(results);
  const costAccounting = summarizeCostAccounting(results);
  const summaryPayload = {
    generated_at: new Date().toISOString(),
    runner: opts.runner,
    model: opts.model,
    repos: opts.repos ?? [...new Set(tasks.map((task) => task.repo))],
    arms: opts.arms,
    task_suite: opts.taskSuite,
    task_ids: opts.taskIds,
    task_manifest: opts.taskManifest,
    prepare_codestory_cache: opts.prepareCodestoryCache,
    cache_preparation: cachePreparation,
    tasks: tasks.map((task) => ({
      id: task.id,
      repo: task.repo,
      task_class: task.task_class,
      manifest_path: task.manifest_path,
    })),
    repeats: opts.repeats,
    publishable: opts.publishable,
    max_source_reads_after_packet: opts.maxSourceReadsAfterPacket,
    reuse_baseline_from: opts.reuseBaselineFrom,
    reused_baseline_runs: results.filter((row) => row.reused_from).length,
    allow_failures: opts.allowFailures,
    timeout_ms: opts.timeoutMs,
    sandbox: opts.sandbox,
    output_dir: outDir,
    retrieval_env: retrievalEnv(),
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(process.env)),
    summary,
    cost_accounting: costAccounting,
  };
  await writeFile(path.join(outDir, "summary.json"), `${JSON.stringify(summaryPayload, null, 2)}\n`, "utf8");
  await writeFile(path.join(outDir, "summary.md"), markdownSummary(summary, opts, costAccounting), "utf8");

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
    const blockers = agentPublishableBlockers(results, opts);
    if (blockers.length) {
      console.error("--publishable failed: every run must pass, report total token usage, pass preludes without warnings, pass manifest quality gates when present, run packet first when required, report sufficient packets with zero follow-ups or unresolved diagnostics, and stay within the post-packet source-read budget.");
      for (const blocker of blockers) {
        console.error(formatAgentPublishableBlocker(blocker));
      }
      exitCode = 1;
    }
  }

  console.log(`wrote ${outDir}`);
  if (exitCode) {
    process.exit(exitCode);
  }
}

export {
  analyzeTranscript,
  agentPublishableBlockers,
  assertSafeWindowsCmdArgs,
  benchmarkRunId,
  baselineSearchPreludeStatus,
  buildPacketQualityDeltas,
  buildQualityDebugPayload,
  copyResultArtifact,
  qualityFailureReasons,
  commandCategory,
  extractCommandExecutions,
  isPathInside,
  isTrustedPublishableRepoUrl,
  loadTaskForResult,
  loadTasks,
  manifestRepoMaterializationBlockers,
  materializeRepos,
  parseArgs,
  parseJsonLines,
  cachePolicyForRun,
  packetComposition,
  packetCommandArgs,
  packetForAgentPrompt,
  packetManifestExtraProbes,
  packetManifestQualitySummary,
  packetPreludeManifestComplete,
  packetLatencyTelemetry,
  packetRuntimeCacheObservations,
  packetRuntimePublishableBlockers,
  packetRuntimeQualityGateRequired,
  cacheProvenanceBlockers,
  PACKET_COMPOSITION_WEIGHTS,
  MAX_REUSED_ARTIFACT_BYTES,
  packetCompositionFileScore,
  packetFirstCommandForPrompt,
  publicCoreCorpusAudit,
  repoProvenanceBlockers,
  resolveRunArtifactPath,
  repoConfigFromManifest,
  resolveCodeStoryCli,
  scoreQuality,
  summarizeCostAccounting,
  summarizePacketRuntimeRuns,
  taskSnapshotForResult,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
