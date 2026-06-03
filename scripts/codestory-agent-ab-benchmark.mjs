#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import {
  buildPacketQualityDeltas,
  discoverPreviousPacketSummary,
} from "./codestory-agent-value-score.mjs";
import {
  benchmarkChildEnv,
  retrievalContractSummary,
  retrievalEnv as benchmarkRetrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedSidecarContractRequests,
} from "./codestory-benchmark-contract.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const siblingRoot = path.resolve(repoRoot, "..");
const defaultTaskRoot = path.join(repoRoot, "benchmarks", "tasks");
const defaultRepoCacheRoot = path.join(repoRoot, "target", "agent-benchmark", "repos");
const MANIFEST_REPO_NAME_PATTERN = /^[A-Za-z0-9_.-]+$/;
const MANIFEST_TASK_ID_PATTERN = /^[a-z0-9][a-z0-9.-]*$/;
const PACKET_TASK_CLASSES = new Set([
  "architecture_explanation",
  "bug_localization",
  "change_impact",
  "route_tracing",
  "symbol_ownership",
  "data_flow",
  "edit_planning",
]);

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
    "Do not use CodeStory, codestory-cli, or codestory-grounding. Use normal repository exploration only.",
  with_codestory:
    "Use CodeStory grounding first. If CODESTORY_CLI is set, use that executable; otherwise use codestory-cli on PATH. For broad repository questions, run packet first and read its sufficiency contract before ordinary source reads. Read follow-up commands from sufficiency.follow_up_commands, not a top-level field. If sufficiency.status is partial, run only the listed follow_up_commands in order and prefer targeted `search --why` commands before escalating packet budget. If a later packet becomes sufficient, stop exploration and answer. If packet status is sufficient and sufficiency.follow_up_commands is empty, answer from the packet; do not verify citations with ordinary source reads, rg, grep, or git show. Budget truncation alone is not a gap. Preserve the packet's supported-claim wording in your final answer. Include a compact 'Support files' list containing every relevant path from the packet's answer.citations and sufficiency.avoid_opening, not only the paths mentioned in your prose. Use search, context, trail, or snippet only for named gaps. The prepared full sidecar cache is mandatory; if CodeStory or its sidecars are unavailable, fail the run instead of continuing with ordinary exploration.",
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
  --codestory-cli Path to codestory-cli for packet runtime mode. Default: CODESTORY_CLI, release binary, then PATH.
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
  --prepare-codestory-cache
                  Before timed with-CodeStory runs, refresh stale or semantic-empty local caches and record indexing cost separately.
                  Packet-runtime mode enables this by default because sidecar-primary packets require prepared local indexes.
  --no-prepare-codestory-cache
                  Unsupported; sidecar preparation is mandatory.
  --prepare-codestory-timeout-ms
                  Timeout for each pre-run CodeStory index refresh. Default: 1800000.
  --max-source-reads-after-packet
                  Publishable with-CodeStory runs fail above this post-packet ordinary source-read count. Default: 0.
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
    prepareCodestoryCache: null,
    prepareCodestoryTimeoutMs: 1_800_000,
    cachePreparationByRepo: null,
    maxSourceReadsAfterPacket: 0,
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
    if (arg === "--self-test") {
      opts.selfTest = true;
      continue;
    }
    if (arg === "--reanalyze-dir") {
      opts.reanalyzeDir = argv[++i];
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
    if (arg === "--materialize-repos") {
      opts.materializeRepos = true;
      continue;
    }
    if (arg === "--packet-runtime") {
      opts.packetRuntime = true;
      continue;
    }
    if (arg === "--packet-runtime-mode") {
      opts.packetRuntimeMode = argv[++i];
      continue;
    }
    if (arg === "--repo-cache-dir") {
      opts.repoCacheDir = argv[++i];
      continue;
    }
    if (arg === "--codestory-cli") {
      opts.codestoryCli = argv[++i];
      continue;
    }
    if (arg === "--repos") {
      opts.repos = commaSeparatedList(argv[++i]);
      continue;
    }
    if (arg === "--arms") {
      opts.arms = commaSeparatedList(argv[++i]);
      continue;
    }
    if (arg === "--task-suite") {
      opts.taskSuite = argv[++i];
      continue;
    }
    if (arg === "--task-ids") {
      opts.taskIds = commaSeparatedList(argv[++i]);
      continue;
    }
    if (arg === "--task-manifest") {
      opts.taskManifest = argv[++i];
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
    if (arg === "--benchmark-run-id") {
      opts.benchmarkRunId = argv[++i];
      continue;
    }
    if (arg === "--timeout-ms") {
      opts.timeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--prepare-codestory-cache") {
      opts.prepareCodestoryCache = true;
      continue;
    }
    if (arg === "--no-prepare-codestory-cache") {
      throw new Error("--no-prepare-codestory-cache is unsupported; sidecar preparation is mandatory");
      continue;
    }
    if (arg === "--prepare-codestory-timeout-ms") {
      opts.prepareCodestoryTimeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--max-source-reads-after-packet") {
      opts.maxSourceReadsAfterPacket = Number.parseInt(argv[++i], 10);
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }

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
  if (!Number.isInteger(opts.prepareCodestoryTimeoutMs) || opts.prepareCodestoryTimeoutMs < 1000) {
    throw new Error("--prepare-codestory-timeout-ms must be an integer >= 1000");
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
  if (!Number.isInteger(opts.maxSourceReadsAfterPacket) || opts.maxSourceReadsAfterPacket < 0) {
    throw new Error("--max-source-reads-after-packet must be a non-negative integer");
  }
  opts.repoCacheDir = path.resolve(opts.repoCacheDir ?? defaultRepoCacheRoot);
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
  const expectedSymbols = textAnchorList(raw.expected_symbols ?? raw.expectedSymbols);
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

function composePrompt(repoName, repoConfig, armName, task = null) {
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
\`\`\`powershell
${packetFirstCommand}
\`\`\`

Run that answer packet before any repository search, direct source read, git command, CodeStory primitive, or help/probe command. The benchmark treats help/probe commands such as \`--help\` as not packet-first.`
    : "";
  const stopContractBlock =
    armName === "with_codestory"
      ? `
If the packet reports \`sufficiency.status: "sufficient"\` with no \`sufficiency.follow_up_commands\`, do not run ordinary source reads, \`rg\`, \`grep\`, \`git show\`, or file-open commands afterward. Those commands count as benchmark overhead unless the packet names a concrete unresolved gap.`
      : "";
  return `You are running a controlled CodeStory benchmark.

Repository: ${repoName}
${taskHeader}
Task: ${taskPrompt}

Arm: ${armName}
Instruction: ${ARMS[armName]}
${packetFirstBlock}
${stopContractBlock}

Return a concise answer with the files, symbols, and commands that support your explanation.
Do not edit source files. Use read-only inspection commands only, except CodeStory may write its cache if needed.`;
}

function packetFirstCommandForPrompt(taskPrompt, task = null) {
  const question = String(taskPrompt).replace(/\r?\n/g, " ");
  const taskClass = task?.task_class
    ? ` --task-class ${powershellSingleQuoted(validatePacketTaskClass("benchmark task", task.task_class).replace(/_/g, "-"))}`
    : "";
  return `& $env:CODESTORY_CLI packet --project . --question ${powershellSingleQuoted(question)}${taskClass} --budget compact --format json`;
}

function powershellSingleQuoted(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
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
    /&\s*\$env:CODESTORY_CLI\s+/i.test(shellText) ||
    new RegExp(`&\\s*\\$[a-z_][a-z0-9_]*\\s+${codestoryCommands}`, "i").test(shellText)
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
    /&\s*\$env:CODESTORY_CLI\s+packet\b/i.test(shellText) ||
    /&\s*\$[a-z_][a-z0-9_]*\s+packet\b/i.test(shellText)
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
    /&\s*\$env:CODESTORY_CLI\s+index\b/i.test(shellText) ||
    /&\s*\$[a-z_][a-z0-9_]*\s+index\b/i.test(shellText)
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
    .replace(/^['"]|['"]$/g, "")
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
  return /\.(rs|js|jsx|ts|tsx|py|go|java|kt|cs|cpp|c|h|hpp|rb|php|swift|md|toml|json|yaml|yml)$/i.test(normalized);
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
    /\bGet-Content\b(?:\s+-[A-Za-z]+(?:\s+\S+)?)?\s+['"]?([^'";|`\r\n]+)['"]?/gi,
    /\bcat\b\s+['"]?([^'";|`\r\n]+)['"]?/gi,
    /\btype\b\s+['"]?([^'";|`\r\n]+)['"]?/gi,
    /\bnl\b(?:\s+-[A-Za-z]+)*\s+['"]?([^'";|`\r\n]+)['"]?/gi,
    /\bsed\b\s+-n\s+['"]?[^'"]+['"]?\s+['"]?([^'";|`\r\n]+)['"]?/gi,
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

function normalizeSearchText(value) {
  return String(value ?? "")
    .toLowerCase()
    .replace(/\\/g, "/")
    .replace(/\s+/g, " ")
    .trim();
}

function redactUrlForDisplay(value) {
  if (value == null) {
    return value;
  }
  return String(value ?? "").replace(/^(https?:\/\/)([^/@\s]+)@/, "$1***@");
}

function anchorMatched(haystack, anchor) {
  const normalizedHaystack = normalizeSearchText(haystack);
  const normalizedAnchor = normalizeSearchText(anchor);
  if (!normalizedAnchor) {
    return false;
  }
  return normalizedHaystack.includes(normalizedAnchor);
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
  "only",
  "without",
]);

function claimPolarityTokens(claim) {
  return claimTokens(claim).filter((token) => FORBIDDEN_POLARITY_TERMS.has(token));
}

function forbiddenClaimMatched(haystack, claim) {
  if (!claimMatched(haystack, claim)) {
    return false;
  }
  const polarityTokens = claimPolarityTokens(claim);
  if (!polarityTokens.length) {
    const haystackTokens = new Set(claimTokens(haystack));
    return claimTokens(claim).every((token) => claimTokenMatched(token, haystackTokens));
  }
  const haystackTokens = new Set(claimTokens(haystack));
  return polarityTokens.every((token) => claimTokenMatched(token, haystackTokens));
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

async function runOne(opts, run, outDir) {
  const repoConfig = ALL_REPOS[run.repo];
  const prompt = composePrompt(run.repo, repoConfig, run.arm, run.task);
  const { command, args, stdin, killProcessTree } = runnerCommand(opts, repoConfig.path, prompt);
  const env = run.arm === "with_codestory" ? benchmarkChildEnv(process.env) : { ...process.env };
  if (run.arm === "with_codestory") {
    env.CODESTORY_CLI = path.resolve(resolveCodeStoryCli(opts));
  }
  const started = performance.now();
  const result = await runProcess(command, args, {
    cwd: repoConfig.path,
    env,
    stdin,
    timeoutMs: opts.timeoutMs,
    timeoutMessage: `Benchmark runner timed out after ${opts.timeoutMs}ms.`,
    forceKillAfterMs: 5000,
    killProcessTree,
  });

  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  const runId = benchmarkRunId([
    run.repo,
    ...(run.task ? [run.task.id] : []),
    run.arm,
    String(run.repeat).padStart(2, "0"),
  ]);
  const stdoutPath = path.join(outDir, `${runId}.stdout.jsonl`);
  const stderrPath = path.join(outDir, `${runId}.stderr.txt`);
  await writeFile(stdoutPath, result.stdout, "utf8");
  await writeFile(stderrPath, result.stderr, "utf8");

  const { parsed, malformed } = parseJsonLines(result.stdout);
  const usage = extractUsage(parsed);
  const toolCalls = parsed.filter(isToolCallStartEvent).length;
  const analysis = analyzeTranscript(parsed, repoConfig.path);
  const provenance = await repoProvenance(repoConfig);
  const packetFirstRequired = run.arm === "with_codestory";
  const packetFirstPass =
    !packetFirstRequired || Boolean(analysis.packet_was_first_context_command);
  const quality = scoreQuality(parsed, run.task);
  const cacheProvenance = run.arm === "with_codestory"
    ? await codestoryCacheProvenance(opts, repoConfig, {
        codestory_index_commands_observed: analysis.codestory_index_commands_observed,
        indexing_in_timed_run: analysis.codestory_index_commands_observed > 0,
        cache_prepared: opts.cachePreparationByRepo?.has(run.repo) ?? false,
        cache_preparation: opts.cachePreparationByRepo?.get(run.repo) ?? null,
        transport_mode: "agent_runner",
      })
    : null;

  return {
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
    status: result.timedOut ? "timeout" : result.exitCode === 0 ? "pass" : "fail",
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error,
    wall_ms: wallMs,
    usage,
    estimated_cost_usd: estimateCost(usage),
    tool_calls_observed: toolCalls,
    transcript_analysis: analysis,
    packet_first_required: packetFirstRequired,
    packet_first_pass: packetFirstPass,
    quality,
    event_types: eventTypeCounts(parsed),
    json_events: parsed.length,
    malformed_stdout_lines: malformed.length,
    stdout_path: stdoutPath,
    stderr_path: stderrPath,
  };
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
  const preparations = [];
  for (const repo of repoNames) {
    const config = ALL_REPOS[repo];
    if (!config || !existsSync(config.path)) {
      preparations.push({
        repo,
        project: config?.path ?? null,
        action: "skipped-missing-repo",
      });
      continue;
    }

    console.log(`preparing CodeStory cache for ${repo}`);
    const before = await codestoryDoctorSnapshot(codestoryCli, config.path, 60_000);
    const preparation = {
      repo,
      project: config.path,
      codestory_cli: path.resolve(codestoryCli),
      action: cachePreparationAction(before),
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

    preparations.push(preparation);
  }
  return preparations;
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
  if (backend === "onnx") {
    return {
      local_only: true,
      locality_kind: "local_model_files",
      locality_evidence: "semantic backend is onnx",
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
  const task = await loadTaskForResult(result, opts, taskCache);
  const repoConfig = ALL_REPOS[result.repo] ?? null;
  const usage = extractUsage(parsed);
  const analysis = analyzeTranscript(parsed, result.repo_path ?? repoConfig?.path ?? runDir);
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
  return {
    ...result,
    repo_provenance: result.repo_provenance ?? (repoConfig ? await repoProvenance(repoConfig) : null),
    codestory_cache_provenance: cacheProvenance,
    usage,
    estimated_cost_usd: estimateCost(usage),
    tool_calls_observed: parsed.filter(isToolCallStartEvent).length,
    transcript_analysis: analysis,
    packet_first_required: packetFirstRequired,
    packet_first_pass:
      !packetFirstRequired || Boolean(analysis.packet_was_first_context_command),
    quality: scoreQuality(parsed, task),
    reanalysis_task_source: result.task_manifest_snapshot ? "snapshot" : task ? "manifest" : null,
    event_types: eventTypeCounts(parsed),
    json_events: parsed.length,
    malformed_stdout_lines: malformed.length,
    reanalyzed_at: new Date().toISOString(),
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
  };
  await writeFile(
    path.join(runDir, "reanalyzed-runs.jsonl"),
    `${reanalyzed.map((row) => JSON.stringify(row)).join("\n")}\n`,
    "utf8",
  );
  await writeFile(path.join(runDir, "reanalyzed-summary.json"), `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  await writeFile(path.join(runDir, "reanalyzed-summary.md"), markdownSummary(summary, summaryOpts), "utf8");
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
  return `  ${result.repo} ${result.task_id ?? ""} ${result.arm} repeat ${result.repeat}: ${blocker.reasons.join("; ")}; total_tokens=${result.usage?.total_tokens ?? ""} packet_first=${result.packet_first_pass ?? ""} quality=${result.quality?.pass ?? ""}`;
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
  return exists(releaseCandidate) ? releaseCandidate : "codestory-cli";
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
  for (const path of packet.sufficiency?.avoid_opening ?? []) {
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
  const avoidOpeningPaths = (packet.sufficiency?.avoid_opening ?? [])
    .map((pathValue, index) => ({
      source: "sufficiency.avoid_opening",
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
  return {
    status,
    covered_claims_count: packet.sufficiency?.covered_claims?.length ?? 0,
    open_next_count: packet.sufficiency?.open_next?.length ?? 0,
    avoid_opening_count: packet.sufficiency?.avoid_opening?.length ?? 0,
    gaps_count: packet.sufficiency?.gaps?.length ?? 0,
    follow_up_commands_count: packet.sufficiency?.follow_up_commands?.length ?? 0,
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

function packetLatencyTelemetry(packet, wallMs) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  const retrievalTrace = packet.answer?.retrieval_trace ?? null;
  const benchmarkRetrievalTrace = packet.benchmark_trace?.retrieval_trace ?? null;
  const retrievalShadow = packetRetrievalShadowTelemetry(
    benchmarkRetrievalTrace?.retrieval_shadow ?? retrievalTrace?.retrieval_shadow ?? null,
  );
  const freshness = packet.answer?.freshness ?? null;
  const steps = Array.isArray(retrievalTrace?.steps) ? retrievalTrace.steps : [];
  const topStep = [...steps].sort((left, right) => (right.duration_ms ?? 0) - (left.duration_ms ?? 0))[0] ?? null;
  const retrievalTotalMs = finiteNumber(retrievalTrace?.total_latency_ms);
  const freshnessMs = finiteNumber(freshness?.duration_ms);
  const unaccountedMs =
    Number.isFinite(wallMs) && Number.isFinite(retrievalTotalMs) && Number.isFinite(freshnessMs)
      ? Math.max(0, wallMs - retrievalTotalMs - freshnessMs)
      : null;
  return {
    freshness_ms: Number.isFinite(freshnessMs) ? freshnessMs : null,
    retrieval_total_ms: Number.isFinite(retrievalTotalMs) ? retrievalTotalMs : null,
    sla_target_ms: finiteNumber(retrievalTrace?.sla_target_ms),
    sla_missed: retrievalTrace?.sla_missed ?? null,
    unaccounted_ms: unaccountedMs,
    top_step_kind: topStep?.kind ?? null,
    top_step_status: topStep?.status ?? null,
    top_step_duration_ms: finiteNumber(topStep?.duration_ms),
    top_step_message: topStep?.message ?? null,
    retrieval_step_count: steps.length,
    retrieval_shadow: retrievalShadow,
  };
}

async function runColdPacketRuntime(opts, task, repeat, outDir) {
  const repoConfig = ALL_REPOS[task.repo];
  const codestoryCli = resolveCodeStoryCli(opts);
  const provenance = await repoProvenance(repoConfig);
  const cacheProvenance = await codestoryCacheProvenance(opts, repoConfig, {
    codestory_index_commands_observed: 0,
    indexing_in_timed_run: false,
    transport_mode: "cold_cli_packet",
  });
  const args = [
    "packet",
    "--project",
    repoConfig.path,
    "--question",
    task.prompt,
    "--budget",
    "compact",
    "--format",
    "json",
  ];
  if (task.task_class) {
    args.push("--task-class", task.task_class.replace(/_/g, "-"));
  }
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
  const pending = [];
  let closedError = null;
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
        waiter.resolve(line);
      }
    }
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk.toString();
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
        waiter = {
          resolve: (line) => {
            clearTimeout(timer);
            resolve(line);
          },
          reject: (error) => {
            clearTimeout(timer);
            reject(error);
          },
        };
        pending.push(waiter);
        child.stdin.write(`${JSON.stringify(payload)}\n`);
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
  const cacheProvenance = await codestoryCacheProvenance(opts, repoConfig, {
    codestory_index_commands_observed: 0,
    indexing_in_timed_run: false,
    transport_mode: "warm_stdio_packet",
  });
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
        const responseLine = await client.request({
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
        const response = JSON.parse(responseLine);
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
      median_response_bytes: median(successful.map((row) => row.response_bytes)),
      median_packet_bytes: median(shapeRows.map((row) => row.packet_shape?.packet_bytes)),
      median_packet_graph_bytes: median(shapeRows.map((row) => row.packet_shape?.graph_bytes)),
      median_budget_used_output_bytes: median(shapeRows.map((row) => row.packet_shape?.budget_used_output_bytes)),
      median_packet_freshness_ms: median(latencyRows.map((row) => row.packet_latency?.freshness_ms)),
      median_packet_retrieval_total_ms: median(latencyRows.map((row) => row.packet_latency?.retrieval_total_ms)),
      median_packet_unaccounted_ms: median(latencyRows.map((row) => row.packet_latency?.unaccounted_ms)),
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

function packetRuntimeMarkdown(summary) {
  const lines = [
    "# Packet Runtime Benchmark",
    "",
    "| Repo | Task | Mode | Runs | Pass | Quality Pass | Sufficiency | Suff/quality gaps | Wall ms median | Retrieval ms median | Freshness ms median | Unaccounted ms median | Top step | Top step ms median | SLA misses | Packet-cache hits | Retrieval cache-hit runs | Stage cache-hit runs | Response bytes median | Packet bytes median | Graph bytes median | Avoid-open median | Follow-up median | File recall | Citation coverage | Packet citation recall | Packet answer-surface recall | Packet structured recall |",
    "| --- | --- | --- | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
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
    formatValue(row.median_wall_ms),
    formatValue(row.median_packet_retrieval_total_ms),
    formatValue(row.median_packet_freshness_ms),
    formatValue(row.median_packet_unaccounted_ms),
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
  if (!isPinnedRepoRef(configuredRef)) {
    reasons.push("repo ref is not pinned to an immutable commit or tag");
  }
  if (manifestRef && configuredRef && manifestRef !== configuredRef) {
    reasons.push(`manifest ref ${manifestRef} does not match configured ref ${configuredRef}`);
  }
  if (!provenance.git_head) {
    reasons.push("missing git head");
  }
  if (provenance.git_dirty !== false) {
    reasons.push(provenance.git_dirty ? "repo checkout is dirty" : "repo cleanliness is unknown");
  }
  return reasons;
}

function isPinnedRepoRef(ref) {
  const value = String(ref ?? "").trim();
  if (!value || value === "local") {
    return false;
  }
  if (/^[0-9a-f]{7,40}$/i.test(value)) {
    return true;
  }
  if (/^refs\/tags\/[^/\s]+$/i.test(value)) {
    return true;
  }
  if (/^v?\d+\.\d+(?:\.\d+)?(?:[-+][A-Za-z0-9._-]+)?$/.test(value)) {
    return true;
  }
  return false;
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
      sufficient_quality_mismatch: row.sufficiency?.sufficient_quality_mismatch ?? null,
    };
  });
  const failing = rows.filter((row) => row.quality_pass === false);
  const reasonCounts = {};
  for (const row of failing) {
    for (const reason of row.failure_reasons) {
      reasonCounts[reason] = (reasonCounts[reason] ?? 0) + 1;
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
      failure_reason_counts: reasonCounts,
    },
  };
}

function packetRuntimePublishableBlockers(results, opts = {}) {
  const enforceRepoProvenance = Boolean(opts.publishable || opts.enforceRepoProvenance);
  const enforcePacketRuntimeTelemetry = Boolean(opts.publishable || opts.enforcePacketRuntimeTelemetry);
  return results
    .map((row) => {
      const reasons = [];
      if (row.status !== "pass") {
        reasons.push(`status=${row.status}`);
      }
      if (!row.quality) {
        reasons.push("missing manifest quality score");
      } else if (!row.quality.pass) {
        reasons.push("manifest quality failed");
      }
      if (row.sufficiency?.sufficient_quality_mismatch) {
        reasons.push("packet sufficiency says sufficient but manifest quality failed");
      }
      if (enforcePacketRuntimeTelemetry) {
        if (!row.sufficiency) {
          reasons.push("missing packet sufficiency telemetry");
        } else if (row.sufficiency.status !== "sufficient") {
          reasons.push(`packet sufficiency status=${row.sufficiency.status ?? "unknown"}; expected sufficient`);
        }
        const latency = row.packet_latency;
        if (!latency) {
          reasons.push("missing packet latency telemetry");
        } else {
          if (latency.sla_missed !== false) {
            reasons.push(`packet retrieval SLA missed=${latency.sla_missed ?? "unknown"}; expected false`);
          }
          const shadow = latency.retrieval_shadow;
          if (!shadow) {
            reasons.push("missing retrieval shadow telemetry");
          } else if (shadow.retrieval_mode !== "full") {
            reasons.push(`packet retrieval shadow mode=${shadow.retrieval_mode ?? "unknown"}; expected full`);
          }
        }
      }
      if (enforceRepoProvenance) {
        reasons.push(...repoProvenanceBlockers(row));
        reasons.push(...cacheProvenanceBlockers(row));
      }
      return reasons.length ? { result: row, reasons } : null;
    })
    .filter(Boolean);
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
    for (const task of tasks) {
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        console.log(`packet-runtime cold-cli ${task.repo} ${task.id} repeat ${repeat}/${opts.repeats}`);
        results.push(await runColdPacketRuntime(opts, task, repeat, outDir));
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
  const payload = {
    generated_at: new Date().toISOString(),
    benchmark_run_id: benchmarkId,
    codestory_cli: resolveCodeStoryCli(opts),
    modes,
    repeats: opts.repeats,
    output_dir: outDir,
    retrieval_env: retrievalEnv(),
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(process.env)),
    summary,
  };
  const packetRuntimeSummaryPath = path.join(outDir, "packet-runtime-summary.json");
  await writeFile(packetRuntimeSummaryPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  await writeFile(path.join(outDir, "packet-runtime-summary.md"), packetRuntimeMarkdown(summary), "utf8");
  const baselinePacketSummaryPath = discoverPreviousPacketSummary(packetRuntimeSummaryPath, repoRoot);
  const baselinePacketSummary = baselinePacketSummaryPath
    ? JSON.parse(await readFile(baselinePacketSummaryPath, "utf8"))
    : null;
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

  const blockers = packetRuntimePublishableBlockers(results, opts);
  if (opts.publishable && blockers.length) {
    console.error("--publishable failed: packet runtime rows must pass, include passing manifest quality gates, and use pinned clean repo provenance.");
    for (const blocker of blockers) {
      const row = blocker.result;
      console.error(`  ${row.repo} ${row.task_id} ${row.mode} repeat ${row.repeat}: ${blocker.reasons.join("; ")}`);
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
    const categoryMedians = {};
    for (const category of [
      "codestory_cli",
      "shell_search",
      "direct_file_read",
      "git",
      "build_test",
      "other",
    ]) {
      categoryMedians[category] = median(
        successful.map((row) => row.transcript_analysis?.command_categories?.[category] ?? 0),
      );
    }
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
      quality_scored_runs: qualityRows.length,
      quality_pass_runs: qualityRows.filter((row) => row.quality?.pass).length,
      median_wall_ms: median(successful.map((row) => row.wall_ms)),
      median_total_tokens: median(successful.map((row) => row.usage.total_tokens)),
      median_input_tokens: median(successful.map((row) => row.usage.input_tokens)),
      median_output_tokens: median(successful.map((row) => row.usage.output_tokens)),
      median_estimated_cost_usd: median(successful.map((row) => row.estimated_cost_usd)),
      median_tool_calls_observed: median(successful.map((row) => row.tool_calls_observed)),
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
    });
  }
  return summaries;
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
  const maxSourceReadsAfterPacket = opts.maxSourceReadsAfterPacket ?? 0;
  const enforceRepoProvenance = Boolean(opts.publishable || opts.enforceRepoProvenance);
  return results
    .map((result) => {
      const reasons = [];
      if (result.status !== "pass") {
        reasons.push(`status=${result.status}`);
      }
      if (result.usage?.total_tokens == null) {
        reasons.push("missing total token usage");
      }
      if (result.packet_first_required && !result.packet_first_pass) {
        reasons.push("missing answer packet as first successful context command");
      }
      if (result.task_id && !result.quality) {
        reasons.push("missing manifest quality score");
      }
      if (result.quality && !result.quality.pass) {
        reasons.push("manifest quality failed");
      }
      const readsAfterPacket = result.transcript_analysis?.ordinary_source_reads_after_first_packet;
      if (
        result.packet_first_required &&
        readsAfterPacket != null &&
        readsAfterPacket > maxSourceReadsAfterPacket
      ) {
        reasons.push(`ordinary source reads after packet=${readsAfterPacket} > ${maxSourceReadsAfterPacket}`);
      }
      if (enforceRepoProvenance) {
        reasons.push(...repoProvenanceBlockers(result));
      }
      if (result.arm === "with_codestory" && (opts.publishable || opts.enforceCacheProvenance)) {
        reasons.push(...cacheProvenanceBlockers(result));
      }
      return reasons.length ? { result, reasons } : null;
    })
    .filter(Boolean);
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
    "| Repo | Task | Arm | Runs | Success | Packet first | Quality pass | Median wall ms | Median tokens | Median cost USD | Median tool calls | Source reads | After CodeStory | After Packet | File recall | Citation coverage | Context chars | Useful anchors / 10k context chars |",
    "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
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

function markdownSummaryRow(row) {
  const cells = [
    row.repo,
    row.task_id ?? "",
    row.arm,
    row.runs,
    row.successful_runs,
    packetFirstLabel(row),
    qualityPassLabel(row),
    formatValue(row.median_wall_ms),
    formatValue(row.median_total_tokens),
    formatValue(row.median_estimated_cost_usd),
    formatValue(row.median_tool_calls_observed),
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
      gaps: [],
      follow_up_commands: [],
    },
    answer: {
      citations: [{ display_name: "run_index" }],
      graphs: [{ id: "g", edges: [{ id: "e1" }] }],
      retrieval_trace: { steps: [] },
      sections: [{ blocks: [{ markdown: "answer" }] }],
    },
  };
  assert.equal(packetShape(packetFixture).budget_used_output_bytes, 123);
  assert.equal(packetShape(packetFixture).graph_bytes > 2, true);
  assert.equal(
    packetSufficiencyTelemetry(packetFixture, { pass: false }).sufficient_quality_mismatch,
    true,
  );
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
  if (opts.materializeRepos) {
    await materializeRepos(tasks, opts);
  }
  if (opts.publishable) {
    validatePublishableShape(opts, tasks);
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

  const results = [];
  for (const run of plannedRuns) {
    console.log(`running ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats}`);
    const result = await runOne(opts, run, outDir);
    results.push(result);
    await writeJsonlRows(path.join(outDir, "runs.jsonl"), results);
  }

  const summary = summarizeRuns(results);
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
    allow_failures: opts.allowFailures,
    timeout_ms: opts.timeoutMs,
    sandbox: opts.sandbox,
    output_dir: outDir,
    retrieval_env: retrievalEnv(),
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(process.env)),
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
    const blockers = agentPublishableBlockers(results, opts);
      if (blockers.length) {
        console.error("--publishable failed: every run must pass, report total token usage, pass manifest quality gates when present, run packet first when required, and stay within the post-packet source-read budget.");
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
  buildPacketQualityDeltas,
  buildQualityDebugPayload,
  qualityFailureReasons,
  commandCategory,
  extractCommandExecutions,
  isPathInside,
  loadTaskForResult,
  loadTasks,
  materializeRepos,
  parseArgs,
  parseJsonLines,
  packetComposition,
  packetLatencyTelemetry,
  packetRuntimePublishableBlockers,
  PACKET_COMPOSITION_WEIGHTS,
  packetCompositionFileScore,
  packetFirstCommandForPrompt,
  publicCoreCorpusAudit,
  repoProvenanceBlockers,
  repoConfigFromManifest,
  resolveCodeStoryCli,
  scoreQuality,
  summarizePacketRuntimeRuns,
  taskSnapshotForResult,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
