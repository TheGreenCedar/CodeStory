#!/usr/bin/env node
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { existsSync, realpathSync, statSync } from "node:fs";
import { chmod, copyFile, mkdir, mkdtemp, open, readdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { StringDecoder } from "node:string_decoder";
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
  unsupportedRetrievalContractRequests,
} from "./codestory-benchmark-contract.mjs";
import {
  cacheProvenanceBlockers,
  isImmutableCommitRef,
  isTrustedPublishableRepoUrl,
  repoProvenanceBlockers,
} from "./codestory-evidence-provenance.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const benchmarkHarnessPath = fileURLToPath(import.meta.url);
const benchmarkScorerPath = path.join(scriptDir, "codestory-agent-value-score.mjs");
const repoRoot = path.resolve(scriptDir, "..");
const siblingRoot = path.resolve(repoRoot, "..");
const defaultTaskRoot = path.join(repoRoot, "benchmarks", "tasks");
const defaultRepoCacheRoot = path.join(repoRoot, "target", "agent-benchmark", "repos");
const MANIFEST_REPO_NAME_PATTERN = /^[A-Za-z0-9_.-]+$/;
const MANIFEST_TASK_ID_PATTERN = /^[a-z0-9][a-z0-9.-]*$/;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const MAX_PACKET_MANIFEST_EXTRA_PROBES = 12;
const PUBLIC_PACKET_MAX_ANCHORS = 16;
const PUBLIC_PACKET_MAX_TRAIL_EDGES = 60;
const PUBLIC_PACKET_MAX_OUTPUT_BYTES = 128 * 1024;
const PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES = 16 * 1024;
const MAX_REUSED_ARTIFACT_BYTES = 64 * 1024 * 1024;
const DEFAULT_BENCHMARK_MODEL = "gpt-5.6-sol";
const EXACT_CANDIDATE_ARMS = Object.freeze([
  "without_codestory",
  "published_0_17_4",
  "candidate_0_18",
]);
const EXACT_CANDIDATE_TASK_IDS = Object.freeze([
  "python-requests-session-flow",
  "java-commons-lang-string-utils",
  "rust-ripgrep-search-pipeline",
  "javascript-express-routing-flow",
  "typescript-swr-hook-flow",
  "cpp-fmt-formatting-flow",
  "c-redis-command-loop",
  "go-gin-route-dispatch",
  "ruby-jekyll-site-build",
  "php-monolog-record-flow",
  "csharp-automapper-map-flow",
  "kotlin-okio-buffer-flow",
  "swift-alamofire-request-flow",
  "dart-http-client-flow",
  "bash-nvm-install-dispatch",
  "html-mdn-form-validation",
  "css-animate-base-and-keyframes",
  "sql-chinook-schema-relations",
]);
const EXACT_CANDIDATE_TASK_REPOS = Object.freeze({
  "python-requests-session-flow": "psf-requests",
  "java-commons-lang-string-utils": "apache-commons-lang",
  "rust-ripgrep-search-pipeline": "BurntSushi-ripgrep",
  "javascript-express-routing-flow": "expressjs-express",
  "typescript-swr-hook-flow": "vercel-swr",
  "cpp-fmt-formatting-flow": "fmtlib-fmt",
  "c-redis-command-loop": "redis-redis",
  "go-gin-route-dispatch": "gin-gonic-gin",
  "ruby-jekyll-site-build": "jekyll-jekyll",
  "php-monolog-record-flow": "Seldaek-monolog",
  "csharp-automapper-map-flow": "AutoMapper-AutoMapper",
  "kotlin-okio-buffer-flow": "square-okio",
  "swift-alamofire-request-flow": "Alamofire-Alamofire",
  "dart-http-client-flow": "dart-lang-http",
  "bash-nvm-install-dispatch": "nvm-sh-nvm",
  "html-mdn-form-validation": "mdn-learning-area",
  "css-animate-base-and-keyframes": "animate-css-animate-css",
  "sql-chinook-schema-relations": "lerocha-chinook-database",
});
const EXACT_CANDIDATE_PACKAGE_CONTRACT = "codestory.agent-benchmark-package/v2";
const EXACT_CANDIDATE_SOURCE_CLI_CONTRACT = "codestory.agent-benchmark-source-cli/v1";
const MAX_EXACT_RECEIPT_BYTES = 64 * 1024;
const MAX_EXACT_CHECKSUM_MANIFEST_BYTES = 1024 * 1024;
const MAX_EXACT_ARCHIVE_BYTES = 8 * 1024 * 1024 * 1024;
const MAX_EXACT_CLI_BYTES = 1024 * 1024 * 1024;
const MAX_EXACT_SOURCE_IDENTITY_BYTES = 16 * 1024 * 1024;
const MAX_EXACT_ARCHIVE_ENTRIES = 16_384;
const MAX_EXACT_ARCHIVE_LISTING_BYTES = 2 * 1024 * 1024;
const EXACT_CANDIDATE_TASK_CONTRACT_SHA256 =
  "601092667e757421c4cadafdaf0661a64f7066ea0aa8365f19706264a6931173";
const MCP_PROTOCOL_REVISIONS = new Set([
  "2024-11-05",
  "2025-03-26",
  "2025-06-18",
  "2025-11-25",
]);
const PINNED_CODEX_RUNNER_CONFIG = [
  'model_reasoning_effort="xhigh"',
  'service_tier="default"',
  'personality="pragmatic"',
  'model_verbosity="low"',
];
const REUSABLE_BASELINE_ARTIFACT_NAME_PATTERN =
  /(?:\.stdout\.jsonl|\.stderr\.txt|\.baseline-context\.json|\.baseline-context\.stderr\.txt)$/;
const EXACT_COMPARATOR_ARTIFACT_NAME_PATTERN =
  /(?:\.stdout\.jsonl|\.stderr\.txt|\.baseline-context\.json|\.baseline-context\.stderr\.txt|\.codestory-packet(?:-drill)?\.stdout\.json|\.codestory-packet(?:-drill)?\.stderr\.txt)$/;
const MAX_EXACT_COMPARATOR_LEDGER_BYTES = 64 * 1024 * 1024;
const PACKET_TASK_CLASSES = new Set([
  "architecture_explanation",
  "bug_localization",
  "change_impact",
  "route_tracing",
  "symbol_ownership",
  "data_flow",
  "edit_planning",
]);
const PACKET_DISPOSITIONS = new Set([
  "supported",
  "drill_once",
  "not_established",
  "unavailable",
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

const CODESTORY_ARM_INSTRUCTION =
  "Use the CodeStory packet supplied by the harness as the only repository context. Judge its compiled support units directly. Supported, not_established, and unavailable are terminal. For supported, answer from support. For not_established, answer every directly established part and explicitly name the material gaps without inferring missing links. For unavailable, report the typed availability reason. Drill_once permits exactly one MCP packet continuation with the original question, parent_packet_id, listed option_ids, and the declared core_generation_id/retrieval_generation pins; after that result, apply the same terminal rules and stop. Do not use search, context, trail, snippet, shell, git, or direct source reads as packet recovery. Preserve exact source identifiers and paths from support and citations. Do not use web search, browser tools, remote URLs, or upstream mirrors.";

const CODESTORY_V3_ARM_INSTRUCTION =
  "Use the CodeStory packet supplied by the harness as repository evidence, not as proof or an answer-authority verdict. Answer only from its evidence rows. For every requested material stage that an evidence row establishes, state a direct subject-verb claim naming the subject and established action before describing any gaps. Do not substitute a heading, symbol inventory, or adjacent partial observation, and make the claim no broader than the cited evidence row. When a higher-level action and its mechanism are established by the same evidence rows, name both: the subject performs, drives, or handles the action by calling the mechanism. Avoid weak role labels such as `is the ... symbol` or `participates in`; do not report only that the subject `calls` a downstream target. Then state every material gap, scoped only to requested links or stages the rows do not establish. Follow at most one declared continuation using the packet's continuation and publication identities, then reassess its returned gaps without another retrieval call. An exact focused source read is allowed only for a file-local task where the user named that exact file, or when a material evidence-missing, Unknown, or Unavailable boundary authenticates that exact path. A file mentioned inside a broad flow question does not authorize a read. Perform at most one bounded read per authorized path. An output_budget_exceeded gap is descriptive and does not authorize a source read or another repository tool by itself. A packet-cited path or range alone is not read authorization, and an unrelated gap does not authorize arbitrary files. Do not use shell search, Git, or free-form repository recovery. Do not call search or context as packet recovery in this controlled comparison. Do not turn missing evidence into absence or runtime behavior. Do not repeat the initial packet call or use web search, browser tools, remote URLs, or upstream mirrors.";

const ARMS = {
  without_codestory:
    "Do not use CodeStory, codestory-cli, or codestory-grounding. Use normal local repository exploration only. Do not use web search, browser tools, remote URLs, or upstream mirrors.",
  with_codestory:
    CODESTORY_ARM_INSTRUCTION,
  published_0_17_4: CODESTORY_ARM_INSTRUCTION,
  candidate_0_18: CODESTORY_ARM_INSTRUCTION,
};

function isCodeStoryArm(arm) {
  return arm === "with_codestory" || arm === "published_0_17_4" || arm === "candidate_0_18";
}

function isPacketProjectionV3(packet) {
  return packet?.schema_version === 3;
}

function codeStoryArmInstruction(packet) {
  return isPacketProjectionV3(packet)
    ? CODESTORY_V3_ARM_INSTRUCTION
    : CODESTORY_ARM_INSTRUCTION;
}

function usage() {
  console.log(`Usage:
  node scripts/codestory-agent-ab-benchmark.mjs --list
  node scripts/codestory-agent-ab-benchmark.mjs --self-test
  node scripts/codestory-agent-ab-benchmark.mjs --reanalyze-dir target/agent-benchmark/<run-dir>
  node scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite <suite> [--materialize-repos] [--repeats n]
  node scripts/codestory-agent-ab-benchmark.mjs [--quick] [--repos names] [--arms names] [--task-suite name] [--task-ids ids] [--task-manifest path] [--include-local-repos] [--repeats n] [--runner codex] [--model model] [--sandbox mode] [--out-dir path] [--timeout-ms ms] [--prepare-codestory-cache] [--canary-task-id id] [--shard-count n --shard-index n] [--allow-failures] [--publishable]
  node scripts/codestory-agent-ab-benchmark.mjs --exact-candidate --task-suite language-expansion-holdout --published-archive <path> --published-checksum-manifest <path> --published-checksum-sha256 <sha256> --candidate-source-root <path>

Options:
  --list          Print configured benchmark repositories or selected manifest tasks and exit.
  --self-test     Run transcript analyzer and quality-scoring fixture checks.
  --reanalyze-dir Recompute transcript analysis, quality scores, and summaries from an existing run directory.
  --quick         Default to repo=codestory and repeats=1 unless explicitly set.
  --repos         Comma-separated repo names. Public: ${Object.keys(PUBLIC_REPOS).join(", ")}. Local optional: ${Object.keys(LOCAL_REPOS).join(", ")}
  --arms          Comma-separated A/B arms. Default: without_codestory, with_codestory.
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
  --model         Model passed to codex exec. Default: ${DEFAULT_BENCHMARK_MODEL}.
  --sandbox       Codex sandbox mode. Default: workspace-write.
  --out-dir       Output directory. Default: target/agent-benchmark/<timestamp>.
  --timeout-ms    Timeout per runner invocation. Default: 600000.
  --jobs          Parallel jobs for independent packet-runtime cold-cli rows or independent agent repo groups. Default: 1.
  --reuse-baseline-from
                  Reuse matching without-CodeStory rows from an earlier run directory when the task snapshot is unchanged.
  --resume-prefix-from
                  Exact-candidate only: authenticate and reanalyze one complete-task prefix, then run only the remaining tasks.
  --reuse-comparators-from
                  Exact-candidate only: reuse authenticated no-CodeStory and published-0.17.4 triplets while rerunning every candidate row.
  --reuse-comparators-ledger-sha256
                  External SHA-256 binding for the comparator source runs.jsonl.
  --reuse-comparators-artifacts-sha256
                  External SHA-256 binding for the comparator source artifact bundle.
  --exact-candidate
                  Run the fresh 18-task, three-repeat comparison of no CodeStory, published 0.17.4, and the frozen 0.18 candidate.
  --published-archive
                  Published CodeStory 0.17.4 native archive named by the authenticated checksum manifest.
  --published-checksum-manifest
                  Official published SHA256SUMS.txt containing the selected archive digest.
  --published-checksum-sha256
                  External SHA-256 binding for the official published checksum manifest.
  --candidate-source-root
                  Clean Git root whose checked-in plugin and MCP catalog define the 0.18 candidate. Exact mode builds its CLI with Cargo --locked --release.
  --prepare-codestory-cache
                  Before timed with-CodeStory runs, refresh stale or semantic-empty local caches and record indexing cost separately.
                  Packet-runtime mode enables this by default because packets require prepared local indexes.
  --no-prepare-codestory-cache
                  Unsupported; retrieval preparation is mandatory.
  --prepare-codestory-jobs
                  Parallel jobs for CodeStory cache preparation across independent repos. Default: 2.
  --canary-task-id
                  Task to prepare and run first as a real with-CodeStory repeat-1 row. A manifest can declare canary_task_id.
  --shard-count   Number of deterministic whole-task shards. Default: 1.
  --shard-index   Zero-based shard index. Default: 0.
  --aggregate-shards
                  Comma-separated completed shard directories to validate and aggregate without running agents.
  --candidate-package-sha256
                  SHA-256 of the exact installed candidate archive; required for publishable multi-host shards.
  --collect-all-failures
                  Diagnostic mode only. Continue after deterministic row failures; incompatible with --publishable.
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
  CODESTORY_RETRIEVAL unset|1 Full packet retrieval (benchmark default)
  CODESTORY_RETRIEVAL=0       Unsupported; full retrieval is mandatory
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
      "resume-prefix-from": { type: "string" },
      "reuse-comparators-from": { type: "string" },
      "reuse-comparators-ledger-sha256": { type: "string" },
      "reuse-comparators-artifacts-sha256": { type: "string" },
      "exact-candidate": { type: "boolean" },
      "published-archive": { type: "string" },
      "published-checksum-manifest": { type: "string" },
      "published-checksum-sha256": { type: "string" },
      "candidate-source-root": { type: "string" },
      "prepare-codestory-cache": { type: "boolean" },
      "no-prepare-codestory-cache": { type: "boolean" },
      "prepare-codestory-timeout-ms": { type: "string" },
      "prepare-codestory-jobs": { type: "string" },
      "canary-task-id": { type: "string" },
      "shard-count": { type: "string" },
      "shard-index": { type: "string" },
      "aggregate-shards": { type: "string" },
      "candidate-package-sha256": { type: "string" },
      "collect-all-failures": { type: "boolean" },
      "max-source-reads-after-packet": { type: "string" },
    },
  });
  const providedOptions = new Set(Object.keys(values));
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
    model: DEFAULT_BENCHMARK_MODEL,
    sandbox: "workspace-write",
    outDir: null,
    timeoutMs: 600000,
    jobs: 1,
    reuseBaselineFrom: null,
    resumePrefixFrom: null,
    reuseComparatorsFrom: null,
    reuseComparatorsLedgerSha256: null,
    reuseComparatorsArtifactsSha256: null,
    exactCandidate: false,
    publishedArchive: null,
    publishedChecksumManifest: null,
    publishedChecksumSha256: null,
    candidateSourceRoot: null,
    exactCandidatePackageByArm: null,
    prepareCodestoryCache: null,
    prepareCodestoryJobs: 2,
    prepareCodestoryTimeoutMs: 1_800_000,
    cachePreparationByRepo: null,
    maxSourceReadsAfterPacket: null,
    diagnosticExtraProbesFromManifest: false,
    canaryTaskId: null,
    manifestCanaryTaskId: null,
    shardCount: 1,
    shardIndex: 0,
    aggregateShards: null,
    candidatePackageSha256: null,
    collectAllFailures: false,
    allowFailures: false,
    publishable: false,
  };

  if (values.help) {
    usage();
    process.exit(0);
  }
  if (values["no-prepare-codestory-cache"]) {
    throw new Error("--no-prepare-codestory-cache is unsupported; retrieval preparation is mandatory");
  }
  opts.list = values.list === true;
  opts.selfTest = values["self-test"] === true;
  opts.reanalyzeDir = values["reanalyze-dir"] ?? null;
  opts.quick = values.quick === true;
  opts.publishable = values.publishable === true;
  opts.allowFailures = values["allow-failures"] === true;
  opts.diagnosticExtraProbesFromManifest = values["diagnostic-extra-probes-from-manifest"] === true;
  opts.canaryTaskId = values["canary-task-id"] ?? null;
  opts.shardCount = values["shard-count"] == null ? 1 : Number.parseInt(values["shard-count"], 10);
  opts.shardIndex = values["shard-index"] == null ? 0 : Number.parseInt(values["shard-index"], 10);
  opts.aggregateShards = values["aggregate-shards"]
    ? commaSeparatedList(values["aggregate-shards"]).map((entry) => path.resolve(entry))
    : null;
  opts.candidatePackageSha256 = values["candidate-package-sha256"] ?? null;
  opts.collectAllFailures = values["collect-all-failures"] === true;
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
  opts.resumePrefixFrom = values["resume-prefix-from"] ?? null;
  opts.reuseComparatorsFrom = values["reuse-comparators-from"] ?? null;
  opts.reuseComparatorsLedgerSha256 = values["reuse-comparators-ledger-sha256"] ?? null;
  opts.reuseComparatorsArtifactsSha256 = values["reuse-comparators-artifacts-sha256"] ?? null;
  opts.exactCandidate = values["exact-candidate"] === true;
  opts.publishedArchive = values["published-archive"] ?? null;
  opts.publishedChecksumManifest = values["published-checksum-manifest"] ?? null;
  opts.publishedChecksumSha256 = values["published-checksum-sha256"] ?? null;
  opts.candidateSourceRoot = values["candidate-source-root"] ?? null;
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

  if (!opts.reanalyzeDir && !opts.repos && !opts.taskSuite && !opts.taskManifest && !opts.exactCandidate) {
    opts.repos = opts.quick
      ? ["codestory"]
      : [
          ...Object.keys(PUBLIC_REPOS),
          ...(opts.includeLocalRepos ? Object.keys(LOCAL_REPOS) : []),
        ];
  }
  if (opts.exactCandidate) {
    const exactOptionAllowlist = new Set([
      "exact-candidate",
      "task-suite",
      "materialize-repos",
      "repo-cache-dir",
      "out-dir",
      "published-archive",
      "published-checksum-manifest",
      "published-checksum-sha256",
      "candidate-source-root",
      "resume-prefix-from",
      "reuse-comparators-from",
      "reuse-comparators-ledger-sha256",
      "reuse-comparators-artifacts-sha256",
    ]);
    const forbiddenOptions = [...providedOptions].filter((name) => !exactOptionAllowlist.has(name));
    if (forbiddenOptions.length) {
      throw new Error(`exact-candidate mode forbids option(s): ${forbiddenOptions.sort().join(", ")}`);
    }
    if (opts.reuseBaselineFrom) {
      throw new Error("baseline reuse is forbidden in exact-candidate mode");
    }
    if (opts.quick || opts.packetRuntime || opts.aggregateShards || opts.shardCount !== 1 || opts.shardIndex !== 0) {
      throw new Error("exact-candidate mode forbids quick, packet-runtime, aggregation, and sharding");
    }
    if (opts.runner !== "codex" || (opts.model != null && opts.model !== DEFAULT_BENCHMARK_MODEL)) {
      throw new Error(`exact-candidate mode requires runner=codex and model=${DEFAULT_BENCHMARK_MODEL}`);
    }
    if (opts.sandbox !== "workspace-write" || opts.jobs !== 1 || opts.timeoutMs !== 600_000) {
      throw new Error("exact-candidate mode requires the pinned workspace-write, single-job, 600000ms run window");
    }
    if (opts.taskIds || opts.repos) {
      throw new Error("exact-candidate mode forbids task and repository subsets");
    }
    opts.taskSuite ??= "language-expansion-holdout";
    if (opts.taskSuite !== "language-expansion-holdout") {
      throw new Error("exact-candidate mode requires the language-expansion-holdout task suite");
    }
    if (
      !opts.publishedArchive ||
      !opts.publishedChecksumManifest ||
      !opts.publishedChecksumSha256 ||
      !opts.candidateSourceRoot
    ) {
      throw new Error("exact-candidate mode requires authenticated published archive and candidate source input");
    }
    if (opts.arms && opts.arms.join(",") !== EXACT_CANDIDATE_ARMS.join(",")) {
      throw new Error(`exact-candidate arms are frozen as ${EXACT_CANDIDATE_ARMS.join(",")}`);
    }
    if (opts.repeats != null && opts.repeats !== 3) {
      throw new Error("exact-candidate mode requires exactly 3 repeats");
    }
    opts.arms = [...EXACT_CANDIDATE_ARMS];
    opts.repeats = 3;
    opts.model = DEFAULT_BENCHMARK_MODEL;
    opts.prepareCodestoryCache = true;
    opts.publishedChecksumSha256 = normalizeExternalSha256(
      opts.publishedChecksumSha256,
      "--published-checksum-sha256",
    );
  }
  opts.arms ??= ["without_codestory", "with_codestory"];
  if (!opts.arms.length) {
    throw new Error("--arms must include at least one arm");
  }
  for (const arm of opts.arms) {
    if (!ARMS[arm]) {
      throw new Error(`Unknown arm '${arm}'. Known: ${Object.keys(ARMS).join(", ")}`);
    }
    if (!opts.exactCandidate && EXACT_CANDIDATE_ARMS.includes(arm) && arm !== "without_codestory") {
      throw new Error(`Arm '${arm}' is available only with --exact-candidate`);
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
  if (!Number.isInteger(opts.shardCount) || opts.shardCount < 1) {
    throw new Error("--shard-count must be a positive integer");
  }
  if (!Number.isInteger(opts.shardIndex) || opts.shardIndex < 0 || opts.shardIndex >= opts.shardCount) {
    throw new Error("--shard-index must be zero-based and smaller than --shard-count");
  }
  if (opts.publishable && opts.collectAllFailures) {
    throw new Error("--collect-all-failures is diagnostic-only and cannot be combined with --publishable");
  }
  if (opts.candidatePackageSha256 != null) {
    opts.candidatePackageSha256 = normalizeSha256(
      opts.candidatePackageSha256,
      "--candidate-package-sha256",
    );
  }
  if (opts.publishable && opts.shardCount > 1 && !opts.candidatePackageSha256) {
    throw new Error("publishable multi-host shards require --candidate-package-sha256");
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
  if (opts.resumePrefixFrom) {
    if (!opts.exactCandidate) {
      throw new Error("--resume-prefix-from is available only in exact-candidate mode");
    }
    opts.resumePrefixFrom = path.resolve(opts.resumePrefixFrom);
  }
  const comparatorReuseInputs = [
    opts.reuseComparatorsFrom,
    opts.reuseComparatorsLedgerSha256,
    opts.reuseComparatorsArtifactsSha256,
  ];
  if (comparatorReuseInputs.some(Boolean)) {
    if (!opts.exactCandidate) {
      throw new Error("--reuse-comparators-from is available only in exact-candidate mode");
    }
    if (!comparatorReuseInputs.every(Boolean)) {
      throw new Error(
        "comparator reuse requires --reuse-comparators-from, --reuse-comparators-ledger-sha256, and --reuse-comparators-artifacts-sha256",
      );
    }
    if (opts.resumePrefixFrom) {
      throw new Error("--reuse-comparators-from and --resume-prefix-from are mutually exclusive");
    }
    opts.reuseComparatorsFrom = path.resolve(opts.reuseComparatorsFrom);
    opts.reuseComparatorsLedgerSha256 = normalizeExternalSha256(
      opts.reuseComparatorsLedgerSha256,
      "--reuse-comparators-ledger-sha256",
    );
    opts.reuseComparatorsArtifactsSha256 = normalizeExternalSha256(
      opts.reuseComparatorsArtifactsSha256,
      "--reuse-comparators-artifacts-sha256",
    );
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

function exactCandidateArmEnv(opts, arm) {
  if (!opts.exactCandidate || !isCodeStoryArm(arm)) return {};
  const armRoot = path.join(opts.exactCandidateStateRoot, arm);
  return {
    CODESTORY_CACHE_ROOT: path.join(armRoot, "cache"),
    CODESTORY_STDIO_CACHE_ROOT: path.join(armRoot, "stdio-cache"),
    CODESTORY_PLUGIN_DATA: path.join(armRoot, "plugin-data"),
    CODESTORY_EMBED_QUALIFICATION_DIR: path.join(armRoot, "embedding-qualification"),
    CODESTORY_EMBED_QUALIFICATION_NONCE: `agent-benchmark-${arm}`,
  };
}

const EXACT_CANDIDATE_ARM_DIRECTORY_ENV_KEYS = Object.freeze([
  "CODESTORY_CACHE_ROOT",
  "CODESTORY_STDIO_CACHE_ROOT",
  "CODESTORY_PLUGIN_DATA",
  "CODESTORY_EMBED_QUALIFICATION_DIR",
]);

const EXACT_CANDIDATE_ARM_SCALAR_ENV_KEYS = Object.freeze([
  "CODESTORY_EMBED_QUALIFICATION_NONCE",
]);

function exactCandidateArmEnvironmentGroups(opts, arm) {
  const env = exactCandidateArmEnv(opts, arm);
  const select = (keys) => Object.fromEntries(
    keys.filter((key) => Object.hasOwn(env, key)).map((key) => [key, env[key]]),
  );
  return {
    directories: select(EXACT_CANDIDATE_ARM_DIRECTORY_ENV_KEYS),
    scalars: select(EXACT_CANDIDATE_ARM_SCALAR_ENV_KEYS),
  };
}

async function createExactCandidatePrivateStateRoot(prefix) {
  const created = await mkdtemp(path.join(os.tmpdir(), prefix));
  try {
    return realpathSync(created);
  } catch (error) {
    await rm(created, { recursive: true, force: true });
    throw error;
  }
}

const OWNED_BENCHMARK_ROOT_REMOVE_OPTIONS = Object.freeze({
  recursive: true,
  force: true,
  maxRetries: 12,
  retryDelay: 250,
});

function benchmarkResourceFailure(resource, error, resourcePath = null) {
  return {
    resource,
    ...(resourcePath ? { path: resourcePath } : {}),
    code: typeof error?.code === "string" ? error.code : null,
    message: error instanceof Error ? error.message : String(error),
  };
}

async function removeOwnedBenchmarkRoot(remove, resource, root) {
  try {
    await remove(root, OWNED_BENCHMARK_ROOT_REMOVE_OPTIONS);
    return null;
  } catch (error) {
    return benchmarkResourceFailure(resource, error, root);
  }
}

async function initializeExactCandidateState(opts, dependencies = {}) {
  const createPrivateStateRoot = dependencies.createPrivateStateRoot
    ?? createExactCandidatePrivateStateRoot;
  const makeDirectory = dependencies.makeDirectory ?? mkdir;
  const authenticatePackages = dependencies.authenticatePackages
    ?? authenticateExactCandidatePackages;
  const remove = dependencies.remove ?? rm;
  const allocated = [];
  try {
    opts.exactCandidateStateRoot = await createPrivateStateRoot(
      "codestory-agent-exact-candidate-",
    );
    allocated.push({
      resource: "exact_candidate_state",
      root: opts.exactCandidateStateRoot,
      clear: () => delete opts.exactCandidateStateRoot,
    });
    opts.exactCandidateBaselineContainerRoot = await createPrivateStateRoot(
      "agent-exact-baseline-",
    );
    allocated.push({
      resource: "exact_candidate_baseline_state",
      root: opts.exactCandidateBaselineContainerRoot,
      clear: () => {
        delete opts.exactCandidateBaselineContainerRoot;
        delete opts.exactCandidateBaselineStateRoot;
      },
    });
    opts.exactCandidateBaselineStateRoot = path.join(
      opts.exactCandidateBaselineContainerRoot,
      "private-state",
    );
    await makeDirectory(opts.exactCandidateBaselineStateRoot, {
      recursive: true,
      mode: 0o700,
    });
    const authenticated = await authenticatePackages(opts);
    opts.exactCandidatePackageByArm = authenticated.packages;
    opts.exactCandidateLifecycle = authenticated.lifecycle;
  } catch (error) {
    const cleanupFailures = [];
    for (const allocation of allocated.reverse()) {
      const failure = await removeOwnedBenchmarkRoot(
        remove,
        allocation.resource,
        allocation.root,
      );
      if (failure) {
        cleanupFailures.push(failure);
      } else {
        allocation.clear();
      }
    }
    if (cleanupFailures.length) {
      opts.exactCandidateInitializationCleanupFailures = cleanupFailures;
    } else {
      delete opts.exactCandidateInitializationCleanupFailures;
    }
    delete opts.exactCandidatePackageByArm;
    delete opts.exactCandidateLifecycle;
    throw error;
  }
}

async function finalizeBenchmarkResources(opts, ledger, preparationLedger, dependencies = {}) {
  const remove = dependencies.remove ?? rm;
  const failures = [...(opts.exactCandidateInitializationCleanupFailures ?? [])];
  const attempt = async (resource, action, resourcePath = null) => {
    try {
      await action();
    } catch (error) {
      failures.push(benchmarkResourceFailure(resource, error, resourcePath));
      for (const secondary of error?.benchmarkSecondaryFailures ?? []) {
        failures.push({
          ...secondary,
          resource: `${resource}.${secondary.resource}`,
        });
      }
    }
  };

  await attempt("runs_ledger", () => ledger.close());
  await attempt("preparations_ledger", () => preparationLedger.close());
  for (const [resource, root] of [
    ["exact_candidate_state", opts.exactCandidateStateRoot],
    ["exact_candidate_baseline_state", opts.exactCandidateBaselineContainerRoot],
  ]) {
    if (!root) continue;
    const failure = await removeOwnedBenchmarkRoot(remove, resource, root);
    if (failure) failures.push(failure);
  }
  delete opts.exactCandidateStateRoot;
  delete opts.exactCandidateBaselineContainerRoot;
  delete opts.exactCandidateBaselineStateRoot;
  delete opts.exactCandidateInitializationCleanupFailures;
  return failures;
}

function finalBenchmarkFailure(primaryFailure, finalizationFailures) {
  if (primaryFailure) return primaryFailure;
  if (!finalizationFailures.length) return null;
  const reason = finalizationFailures
    .map((failure) => `${failure.resource}: ${failure.message}`)
    .join("; ");
  return pipelineStageFailure("cleanup", null, new Error(reason));
}

const EXACT_BASELINE_ENV_ALLOWLIST = Object.freeze([
  "ALL_PROXY", "COMSPEC", "HTTPS_PROXY", "HTTP_PROXY", "LANG", "LC_ALL", "LC_CTYPE",
  "LOGNAME", "NODE_EXTRA_CA_CERTS", "NO_PROXY", "PATH", "PATHEXT", "SHELL",
  "SSL_CERT_DIR", "SSL_CERT_FILE", "SYSTEMROOT", "TERM", "TZ", "USER", "WINDIR",
]);

function pathEntryExposesCodeStory(entry) {
  const normalized = normalizePathLike(entry).toLowerCase();
  if (normalized.includes("codestory")) return true;
  return ["codestory", "codestory-cli", "codestory.exe", "codestory-cli.exe", "codestory.cmd"]
    .some((name) => existsSync(path.join(entry, name)));
}

function exactCandidateBaselineEnv(opts, baseEnv = process.env) {
  const root = opts.exactCandidateBaselineStateRoot;
  if (!opts.exactCandidate || !root) throw new Error("exact baseline environment requires its disjoint private state root");
  const env = {};
  for (const key of EXACT_BASELINE_ENV_ALLOWLIST) {
    if (baseEnv[key] != null) env[key] = baseEnv[key];
  }
  env.PATH = String(env.PATH ?? "")
    .split(path.delimiter)
    .filter(Boolean)
    .filter((entry) => !pathEntryExposesCodeStory(entry))
    .join(path.delimiter);
  const home = path.join(root, "home");
  const temporary = path.join(root, "tmp");
  env.HOME = home;
  env.USERPROFILE = home;
  env.XDG_CACHE_HOME = path.join(root, "xdg-cache");
  env.XDG_CONFIG_HOME = path.join(root, "xdg-config");
  env.XDG_DATA_HOME = path.join(root, "xdg-data");
  env.TMPDIR = temporary;
  env.TMP = temporary;
  env.TEMP = temporary;
  return env;
}

function selectedBenchmarkChildEnv(opts = {}, arm = null) {
  if (opts.exactCandidate && arm === "without_codestory") {
    return exactCandidateBaselineEnv(opts);
  }
  return {
    ...(opts.packetRuntimeChildEnv ?? benchmarkChildEnv(process.env)),
    ...exactCandidateArmEnv(opts, arm),
  };
}

function exactCandidatePackageIdentity(receipt, arm) {
  if (arm !== "published_0_17_4") {
    return null;
  }
  const identity = Object.fromEntries([
    "contract", "arm", "package_version", "package_sha256", "cli_sha256",
    "source_commit", "source_tree", "schema_version", "protocol_revision",
    "discovery_contract_sha256",
  ].map((field) => [field, receipt?.[field] ?? null]));
  identity.trust_root_kind = receipt?.trust_root?.kind ?? null;
  identity.trust_root_sha256 = receipt?.trust_root?.sha256 ?? null;
  return identity;
}

function exactCandidateSourceCliIdentity(receipt, arm) {
  if (arm !== "candidate_0_18") return null;
  return Object.fromEntries([
    "contract", "arm", "package_version", "cli_sha256", "source_commit",
    "source_tree", "schema_version", "protocol_revision",
    "discovery_contract_sha256", "plugin_manifest_sha256", "catalog_sha256",
  ].map((field) => [field, receipt?.[field] ?? null]));
}

function exactCandidateResultIdentity(result) {
  return result?.arm === "candidate_0_18"
    ? result?.source_cli_identity
    : result?.package_identity;
}

function resolveCodeStoryCliForArm(opts, arm) {
  return opts.exactCandidatePackageByArm?.get(arm)?.cli_path ?? resolveCodeStoryCli(opts);
}

function runnerCommand(opts, repoPath, prompt, arm = null) {
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
    "--ignore-user-config",
    "--config",
    'approval_policy="never"',
    ...PINNED_CODEX_RUNNER_CONFIG.flatMap((value) => ["--config", value]),
    "--json",
    "--ephemeral",
    "--sandbox",
    opts.sandbox,
    "--cd",
    repoPath,
  ];
  codexArgs.push("--model", opts.model ?? DEFAULT_BENCHMARK_MODEL);
  codexArgs.push("-");
  if (process.platform === "win32") {
    assertSafeWindowsCmdArgs(codexArgs);
  }
  const args = process.platform === "win32" ? ["/d", "/s", "/c", "codex.cmd", ...codexArgs] : codexArgs;
  return { command, args, stdin: prompt, killProcessTree: process.platform === "win32" };
}

function agentRunnerEnv(baseEnv = process.env, codexHome = null, allowCodeStory = true) {
  const env = allowCodeStory ? benchmarkChildEnv(baseEnv) : { ...baseEnv };
  if (allowCodeStory) {
    delete env.CODESTORY_CLI;
  } else {
    for (const key of Object.keys(env)) {
      if (key.startsWith("CODESTORY_")) delete env[key];
    }
  }
  if (codexHome) {
    env.CODEX_HOME = codexHome;
  }
  return env;
}

async function prepareAgentCodexIsolation(outDir, opts = {}) {
  const sourceCodexHome = path.resolve(
    process.env.CODEX_HOME ?? path.join(os.homedir(), ".codex"),
  );
  const authPath = path.join(sourceCodexHome, "auth.json");
  if (!existsSync(authPath)) {
    throw new Error(`Codex benchmark isolation requires auth.json under ${sourceCodexHome}`);
  }

  const receipt = {
    contract: "codestory.agent-benchmark-codex-isolation/v2",
    codestory_surface: "managed_cli_packet_prelude",
    with_codestory_config: "normal_user_config",
    without_codestory_config: "ignore_user_config",
    shared_auth: true,
    output_settings_source: "explicit_runner_args",
    model: opts.model ?? DEFAULT_BENCHMARK_MODEL,
    runner_config: PINNED_CODEX_RUNNER_CONFIG,
  };
  let homes = null;
  if (opts.exactCandidate) {
    homes = {};
    for (const arm of EXACT_CANDIDATE_ARMS) {
      const armRoot = arm === "without_codestory"
        ? opts.exactCandidateBaselineStateRoot
        : path.join(opts.exactCandidateStateRoot, arm);
      const codexHome = path.join(armRoot, "host");
      await mkdir(codexHome, { recursive: true });
      if (arm === "without_codestory") {
        for (const directory of [
          "home", "tmp", "xdg-cache", "xdg-config", "xdg-data",
        ]) {
          await mkdir(path.join(armRoot, directory), { recursive: true });
        }
      }
      await copyFile(authPath, path.join(codexHome, "auth.json"));
      homes[arm] = codexHome;
      for (const value of Object.values(
        exactCandidateArmEnvironmentGroups(opts, arm).directories,
      )) {
        await mkdir(value, { recursive: true, mode: 0o700 });
      }
      if (isCodeStoryArm(arm)) {
        const cli = resolveCodeStoryCliForArm(opts, arm);
        await writeFile(
          path.join(codexHome, "config.toml"),
          `[mcp_servers.codestory]\ncommand = ${JSON.stringify(cli)}\nargs = ["serve", "--stdio", "--multi-project"]\n`,
          "utf8",
        );
      }
    }
    receipt.contract = "codestory.agent-benchmark-codex-isolation/v3";
    receipt.with_codestory_config = "arm_specific_checksum_bound_cli";
    receipt.homes = Object.fromEntries(
      EXACT_CANDIDATE_ARMS.map((arm) => [arm, path.relative(outDir, homes[arm])]),
    );
    receipt.cache_roots = Object.fromEntries(
      EXACT_CANDIDATE_ARMS.map((arm) => [
        arm,
        Object.fromEntries(Object.entries(
          exactCandidateArmEnvironmentGroups(opts, arm).directories,
        ).map(([key, value]) => [
          key,
          path.relative(outDir, value),
        ])),
      ]),
    );
    receipt.embedding_server_namespaces = Object.fromEntries(
      EXACT_CANDIDATE_ARMS.filter(isCodeStoryArm).map((arm) => {
        const groups = exactCandidateArmEnvironmentGroups(opts, arm);
        return [arm, {
          qualification_directory: path.relative(
            outDir,
            groups.directories.CODESTORY_EMBED_QUALIFICATION_DIR,
          ),
          nonce: groups.scalars.CODESTORY_EMBED_QUALIFICATION_NONCE,
        }];
      }),
    );
  }
  await writeFile(
    path.join(outDir, "codex-agent-isolation.json"),
    `${JSON.stringify(receipt, null, 2)}\n`,
    "utf8",
  );
  return {
    root: opts.exactCandidate ? opts.exactCandidateStateRoot : null,
    homes,
    receipt,
  };
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

function sha256Bytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function normalizeSha256(value, label) {
  const normalized = String(value ?? "").trim().toLowerCase();
  if (!SHA256_PATTERN.test(normalized)) {
    throw new Error(`${label} must be a lowercase SHA-256 digest`);
  }
  return normalized;
}

function normalizeExternalSha256(value, label) {
  const digest = normalizeSha256(value, label);
  if (/^0{64}$/.test(digest)) throw new Error(`${label} cannot be the all-zero digest`);
  return digest;
}

async function sha256FileBounded(filePath, maxBytes, label) {
  const handle = await open(filePath, "r");
  try {
    return await streamOpenedFileBounded(handle, maxBytes, label);
  } finally {
    await handle.close();
  }
}

async function readBoundedFile(filePath, maxBytes, label) {
  const handle = await open(filePath, "r");
  try {
    return await streamOpenedFileBounded(handle, maxBytes, label, { capture: true });
  } finally {
    await handle.close();
  }
}

async function writeAll(handle, bytes) {
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesWritten } = await handle.write(bytes, offset, bytes.length - offset);
    if (bytesWritten <= 0) throw new Error("immutable input staging stopped before all bytes were written");
    offset += bytesWritten;
  }
}

async function streamOpenedFileBounded(handle, maxBytes, label, options = {}) {
  const before = await handle.stat();
  if (!before.isFile() || !Number.isSafeInteger(before.size) || before.size < 0 || before.size > maxBytes) {
    throw new Error(`${label} exceeds the ${maxBytes}-byte bound or is not a regular file`);
  }
  const hash = createHash("sha256");
  const chunks = options.capture ? [] : null;
  const buffer = Buffer.allocUnsafe(Math.min(64 * 1024, maxBytes + 1));
  let observed = 0;
  while (true) {
    const remaining = maxBytes + 1 - observed;
    const { bytesRead } = await handle.read(buffer, 0, Math.min(buffer.length, remaining), null);
    if (bytesRead === 0) break;
    observed += bytesRead;
    if (observed > maxBytes) throw new Error(`${label} exceeds the ${maxBytes}-byte bound`);
    const chunk = buffer.subarray(0, bytesRead);
    hash.update(chunk);
    if (options.destination) await writeAll(options.destination, chunk);
    if (chunks) chunks.push(Buffer.from(chunk));
  }
  const after = await handle.stat();
  if (after.size !== observed) throw new Error(`${label} changed while it was being ingested`);
  return {
    sha256: hash.digest("hex"),
    byte_length: observed,
    ...(chunks ? { bytes: Buffer.concat(chunks, observed) } : {}),
  };
}

async function ingestExactInput(opts, kind, sourcePath, maxBytes) {
  const source = path.resolve(sourcePath);
  const stagingRoot = path.join(opts.exactCandidateStateRoot, "authenticated-inputs");
  await mkdir(stagingRoot, { recursive: true, mode: 0o700 });
  const stagedPath = path.join(stagingRoot, `${kind}.bin`);
  const sourceHandle = await open(source, "r");
  let destinationHandle = null;
  try {
    destinationHandle = await open(stagedPath, "wx", 0o600);
    const identity = await streamOpenedFileBounded(
      sourceHandle,
      maxBytes,
      kind.replaceAll("_", " "),
      { destination: destinationHandle },
    );
    await destinationHandle.sync();
    await destinationHandle.chmod(0o400);
    await destinationHandle.close();
    destinationHandle = null;
    await sourceHandle.close();
    await opts.exactCandidateAfterInputIngest?.({
      kind,
      source_path: source,
      staged_path: stagedPath,
      sha256: identity.sha256,
      byte_length: identity.byte_length,
    });
    return { ...identity, source_path: source, staged_path: stagedPath };
  } catch (error) {
    await destinationHandle?.close().catch(() => {});
    await sourceHandle.close().catch(() => {});
    await rm(stagedPath, { force: true });
    throw error;
  }
}

function exactArchiveEntryIsSafe(entry) {
  const normalized = normalizePathLike(entry).replace(/\/$/, "");
  return Boolean(normalized) &&
    !isAbsolutePathLike(normalized) &&
    !normalized.split("/").some((part) => part === "" || part === "." || part === "..");
}

async function walkExactArchive(root) {
  const files = [];
  const pending = [root];
  while (pending.length) {
    const current = pending.pop();
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const absolute = path.join(current, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`exact package archive contains a symlink: ${entry.name}`);
      if (entry.isDirectory()) pending.push(absolute);
      else if (entry.isFile()) files.push(absolute);
      else throw new Error(`exact package archive contains a non-file entry: ${entry.name}`);
      if (files.length + pending.length > MAX_EXACT_ARCHIVE_ENTRIES) {
        throw new Error("exact package archive contains too many entries");
      }
    }
  }
  return files;
}

async function probeExactPackageRuntime(cliPath, expected, env) {
  const session = createSequencedStdioSession(
    cliPath,
    ["serve", "--stdio", "--multi-project", "--refresh", "none"],
    { env, timeoutMs: 30_000 },
  );
  try {
    const response = await session.request({
      jsonrpc: "2.0",
      id: "exact-candidate-authentication",
      method: "initialize",
      params: {
        protocolVersion: expected.protocol_revision,
        capabilities: {},
        clientInfo: { name: "codestory-exact-candidate-authenticator", version: "1" },
      },
    });
    await session.close();
    if (response.error) throw new Error(`exact package initialize failed: ${JSON.stringify(response.error)}`);
    const result = response.result;
    const observed = {
      package_version: result?.serverInfo?.version ?? null,
      schema_version: result?._meta?.codestory_publication?.schema_version ?? null,
      protocol_revision: result?.protocolVersion ?? null,
      discovery_contract_sha256:
        result?._meta?.codestory_protocol?.discovery_contract_sha256 ?? null,
    };
    for (const field of ["package_version", "schema_version", "protocol_revision"]) {
      if (observed[field] !== expected[field]) {
        throw new Error(`exact package runtime ${field}=${observed[field] ?? "missing"}; expected ${expected[field]}`);
      }
    }
    if (expected.discovery_contract_sha256) {
      observed.discovery_contract_sha256 = normalizeExternalSha256(
        observed.discovery_contract_sha256,
        "runtime discovery_contract_sha256",
      );
      if (observed.discovery_contract_sha256 !== expected.discovery_contract_sha256) {
        throw new Error("exact package runtime discovery contract does not match its authenticated receipt");
      }
    } else if (observed.discovery_contract_sha256 !== null) {
      throw new Error("legacy exact package unexpectedly declared a discovery contract digest");
    }
    return observed;
  } catch (error) {
    await session.stop();
    throw error;
  }
}

async function authenticateExactArchive({ arm, archivePath, archiveSha256, archiveIdentity, expected, trustRoot }, root, env) {
  if (archiveIdentity.sha256 !== archiveSha256) throw new Error(`${arm} archive checksum mismatch`);
  const unpackRoot = path.join(root, "packages", arm);
  await mkdir(unpackRoot, { recursive: true });
  const listing = await runProcess("tar", ["-tf", archivePath], {
    timeoutMs: 30_000,
    maxOutputBytes: MAX_EXACT_ARCHIVE_LISTING_BYTES,
  });
  if (listing.status !== "pass") throw new Error(`${arm} archive listing failed: ${trimTail(listing.stderr)}`);
  const entries = listing.stdout.split(/\r?\n/).filter(Boolean);
  if (!entries.length || entries.length > MAX_EXACT_ARCHIVE_ENTRIES || entries.some((entry) => !exactArchiveEntryIsSafe(entry))) {
    throw new Error(`${arm} archive contains an invalid entry set`);
  }
  const unpack = await runProcess("tar", ["-xf", archivePath, "-C", unpackRoot], {
    timeoutMs: 120_000,
    maxOutputBytes: MAX_EXACT_ARCHIVE_LISTING_BYTES,
  });
  if (unpack.status !== "pass") throw new Error(`${arm} archive extraction failed: ${trimTail(unpack.stderr)}`);
  const files = await walkExactArchive(unpackRoot);
  const manifestPaths = files.filter((file) => path.basename(file) === "codestory-native-manifest.json");
  if (manifestPaths.length !== 1) throw new Error(`${arm} archive must contain one native manifest`);
  const manifestRead = await readBoundedFile(manifestPaths[0], MAX_EXACT_RECEIPT_BYTES, `${arm} native manifest`);
  const manifest = JSON.parse(manifestRead.bytes.toString("utf8"));
  const sourceCommit = String(manifest?.source?.commit ?? "");
  const sourceTree = String(manifest?.source?.tree ?? "");
  if (!/^[0-9a-f]{40}$/.test(sourceCommit) || !/^[0-9a-f]{40}$/.test(sourceTree) || manifest?.source?.tracked_dirty !== false) {
    throw new Error(`${arm} native manifest has invalid source identity`);
  }
  if (manifest?.release_version !== expected.package_version) {
    throw new Error(`${arm} native manifest version drifted from authenticated identity`);
  }
  if (expected.source_commit && sourceCommit !== expected.source_commit) throw new Error(`${arm} source commit drifted`);
  if (expected.source_tree && sourceTree !== expected.source_tree) throw new Error(`${arm} source tree drifted`);
  const cliName = String(manifest?.binary?.name ?? "");
  const cliCandidates = files.filter((file) => path.basename(file) === cliName);
  if (!cliName || cliCandidates.length !== 1) throw new Error(`${arm} archive does not contain its declared CLI exactly once`);
  const cliIdentity = await sha256FileBounded(cliCandidates[0], MAX_EXACT_ARCHIVE_BYTES, `${arm} CLI`);
  if (cliIdentity.sha256 !== normalizeSha256(manifest?.binary?.sha256, `${arm} native manifest binary sha256`)) {
    throw new Error(`${arm} unpacked CLI does not belong to the authenticated archive manifest`);
  }
  const runtime = await probeExactPackageRuntime(cliCandidates[0], expected, env);
  return {
    contract: EXACT_CANDIDATE_PACKAGE_CONTRACT,
    arm,
    package_version: expected.package_version,
    package_path: archivePath,
    package_sha256: archiveIdentity.sha256,
    cli_path: cliCandidates[0],
    cli_sha256: cliIdentity.sha256,
    source_commit: sourceCommit,
    source_tree: sourceTree,
    schema_version: runtime.schema_version,
    protocol_revision: runtime.protocol_revision,
    discovery_contract_sha256: runtime.discovery_contract_sha256,
    trust_root: trustRoot,
  };
}

async function readCandidateTrackedFile(sourceRoot, relativePath, label) {
  const result = await runProcess(
    "git",
    ["-C", sourceRoot, "show", `HEAD:${relativePath}`],
    { timeoutMs: 10_000, maxOutputBytes: MAX_EXACT_SOURCE_IDENTITY_BYTES },
  );
  if (result.status !== "pass") {
    throw new Error(`candidate source is missing checked-in ${label}: ${trimTail(result.stderr || result.stdout)}`);
  }
  const bytes = Buffer.from(result.stdout, "utf8");
  if (bytes.length > MAX_EXACT_SOURCE_IDENTITY_BYTES) {
    throw new Error(`candidate checked-in ${label} exceeds the identity byte bound`);
  }
  return { bytes, sha256: sha256Bytes(bytes) };
}

async function candidateSourceSnapshot(sourcePath) {
  const sourceRoot = realpathSync(path.resolve(sourcePath));
  const topLevel = await gitCheckedOutput(
    ["-C", sourceRoot, "rev-parse", "--show-toplevel"],
    repoRoot,
    { timeoutMs: 10_000, maxOutputBytes: 1024 * 1024 },
  );
  if (realpathSync(topLevel) !== sourceRoot) {
    throw new Error("--candidate-source-root must name the Git worktree root exactly");
  }
  const status = await gitCheckedOutput(
    ["-C", sourceRoot, "status", "--porcelain=v1", "--untracked-files=all"],
    repoRoot,
    { timeoutMs: 10_000, maxOutputBytes: 1024 * 1024 },
  );
  if (status) throw new Error("candidate source root must have a clean tracked and untracked worktree");
  const sourceCommit = await gitCheckedOutput(
    ["-C", sourceRoot, "rev-parse", "HEAD"],
    repoRoot,
    { timeoutMs: 10_000, maxOutputBytes: 1024 },
  );
  const sourceTree = await gitCheckedOutput(
    ["-C", sourceRoot, "rev-parse", "HEAD^{tree}"],
    repoRoot,
    { timeoutMs: 10_000, maxOutputBytes: 1024 },
  );
  if (
    !/^[0-9a-f]{40}$/.test(sourceCommit) || /^0{40}$/.test(sourceCommit) ||
    !/^[0-9a-f]{40}$/.test(sourceTree) || /^0{40}$/.test(sourceTree)
  ) {
    throw new Error("candidate source HEAD/tree identity is invalid");
  }
  const [cargo, plugin, catalog] = await Promise.all([
    readCandidateTrackedFile(sourceRoot, "crates/codestory-cli/Cargo.toml", "CLI manifest"),
    readCandidateTrackedFile(sourceRoot, "plugins/codestory/plugin.json", "plugin manifest"),
    readCandidateTrackedFile(sourceRoot, "plugins/codestory/generated-mcp-catalog.json", "MCP catalog"),
  ]);
  const versionMatch = /^version\s*=\s*"([^"]+)"\s*$/mu.exec(cargo.bytes.toString("utf8"));
  const pluginJson = JSON.parse(plugin.bytes.toString("utf8"));
  const catalogJson = JSON.parse(catalog.bytes.toString("utf8"));
  const wire = catalogJson?.wireContract;
  const packageVersion = versionMatch?.[1] ?? null;
  const preferredProtocol = wire?.preferredMcpProtocolVersion ?? null;
  const discoveryContractSha256 = normalizeExternalSha256(
    wire?.discoveryContracts?.[preferredProtocol],
    "candidate catalog preferred discovery contract",
  );
  if (
    pluginJson?.name !== "codestory" || !packageVersion || pluginJson?.version !== packageVersion ||
    wire?.publicationStampSchemaVersion !== 3 ||
    wire?.minimumCompatiblePublicationStampSchemaVersion !== 3 ||
    preferredProtocol !== "2025-11-25" ||
    !Array.isArray(wire?.supportedMcpProtocolVersions) ||
    !wire.supportedMcpProtocolVersions.includes(preferredProtocol)
  ) {
    throw new Error("candidate checked-in plugin/catalog identity is not the schema-3 preferred-profile source contract");
  }
  return {
    source_root: sourceRoot,
    source_commit: sourceCommit,
    source_tree: sourceTree,
    package_version: packageVersion,
    schema_version: 3,
    protocol_revision: preferredProtocol,
    discovery_contract_sha256: discoveryContractSha256,
    plugin_manifest_sha256: plugin.sha256,
    catalog_sha256: catalog.sha256,
  };
}

async function buildExactCandidateCli({ sourceRoot, targetDir, cliPath }) {
  const result = await runProcess(
    "cargo",
    [
      "build", "--locked", "--release", "-p", "codestory-cli",
      "--target-dir", targetDir,
    ],
    {
      cwd: sourceRoot,
      timeoutMs: 30 * 60 * 1000,
      maxOutputBytes: 16 * 1024 * 1024,
    },
  );
  if (result.status !== "pass") {
    throw new Error(`candidate locked release CLI build failed: ${trimTail(result.stderr || result.stdout)}`);
  }
  if (!existsSync(cliPath)) throw new Error("candidate locked release CLI build did not produce codestory-cli");
}

async function authenticateExactCandidateSourceCli(opts) {
  const before = await candidateSourceSnapshot(opts.candidateSourceRoot);
  const targetDir = path.join(before.source_root, "target", "codestory-mission-candidate");
  const cliPath = path.join(
    targetDir,
    "release",
    process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli",
  );
  const buildCli = opts.exactCandidateBuildCli ?? buildExactCandidateCli;
  await buildCli({ sourceRoot: before.source_root, targetDir, cliPath });
  const cliInput = await ingestExactInput(
    opts,
    "candidate_cli",
    cliPath,
    MAX_EXACT_CLI_BYTES,
  );
  await chmod(cliInput.staged_path, 0o500);
  const runtime = await probeExactPackageRuntime(cliInput.staged_path, before, selectedBenchmarkChildEnv(opts, "candidate_0_18"));
  const afterCli = await sha256FileBounded(cliInput.staged_path, MAX_EXACT_CLI_BYTES, "authenticated candidate CLI");
  if (afterCli.sha256 !== cliInput.sha256 || afterCli.byte_length !== cliInput.byte_length) {
    throw new Error("authenticated candidate CLI bytes changed during live initialize");
  }
  const after = await candidateSourceSnapshot(opts.candidateSourceRoot);
  for (const field of [
    "source_root", "source_commit", "source_tree", "package_version", "schema_version",
    "protocol_revision", "discovery_contract_sha256", "plugin_manifest_sha256", "catalog_sha256",
  ]) {
    if (after[field] !== before[field]) throw new Error(`candidate source identity changed during authentication: ${field}`);
  }
  return {
    contract: EXACT_CANDIDATE_SOURCE_CLI_CONTRACT,
    arm: "candidate_0_18",
    package_version: before.package_version,
    cli_path: cliInput.staged_path,
    cli_sha256: cliInput.sha256,
    source_commit: before.source_commit,
    source_tree: before.source_tree,
    schema_version: runtime.schema_version,
    protocol_revision: runtime.protocol_revision,
    discovery_contract_sha256: runtime.discovery_contract_sha256,
    plugin_manifest_sha256: before.plugin_manifest_sha256,
    catalog_sha256: before.catalog_sha256,
  };
}

async function authenticateExactCandidatePackages(opts) {
  const started = performance.now();
  const publishedManifestInput = await ingestExactInput(
    opts,
    "published_checksum_manifest",
    opts.publishedChecksumManifest,
    MAX_EXACT_CHECKSUM_MANIFEST_BYTES,
  );
  const publishedManifest = await readBoundedFile(
    publishedManifestInput.staged_path,
    MAX_EXACT_CHECKSUM_MANIFEST_BYTES,
    "staged published checksum manifest",
  );
  if (
    publishedManifest.sha256 !== normalizeExternalSha256(
      opts.publishedChecksumSha256,
      "published checksum manifest external digest",
    )
  ) {
    throw new Error("published checksum manifest does not match its external digest");
  }
  const publishedArchive = path.resolve(opts.publishedArchive);
  const publishedName = path.basename(publishedArchive);
  const publishedMatches = publishedManifest.bytes.toString("utf8").split(/\r?\n/).flatMap((line) => {
    const match = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line.trim());
    return match && path.basename(match[2]) === publishedName ? [match[1]] : [];
  });
  if (publishedMatches.length !== 1) throw new Error("official checksum data must name the published archive exactly once");

  const packageRoot = opts.exactCandidateStateRoot;
  const order = opts.exactCandidatePackageAuthenticationOrder ?? ["published_0_17_4", "candidate_0_18"];
  const publishedDefinition = {
    arm: "published_0_17_4",
    sourceArchivePath: publishedArchive,
    archiveSha256: normalizeExternalSha256(publishedMatches[0], "official published archive sha256"),
    expected: { package_version: "0.17.4", schema_version: 2, protocol_revision: "2024-11-05" },
    trustRoot: { kind: "official_published_checksum", sha256: publishedManifest.sha256 },
  };
  const packages = new Map();
  const armTimings = {};
  for (const arm of order) {
    if (!EXACT_CANDIDATE_ARMS.includes(arm) || arm === "without_codestory" || packages.has(arm)) {
      throw new Error("package authentication order must contain each exact CodeStory arm once");
    }
    const armStarted = performance.now();
    if (arm === "published_0_17_4") {
      const archiveInput = await ingestExactInput(
        opts,
        `${arm}_archive`,
        publishedDefinition.sourceArchivePath,
        MAX_EXACT_ARCHIVE_BYTES,
      );
      packages.set(arm, await authenticateExactArchive(
        {
          ...publishedDefinition,
          archivePath: archiveInput.staged_path,
          archiveIdentity: archiveInput,
        },
        packageRoot,
        selectedBenchmarkChildEnv(opts, arm),
      ));
    } else {
      packages.set(arm, await authenticateExactCandidateSourceCli(opts));
    }
    armTimings[arm] = Math.round((performance.now() - armStarted) * 1000) / 1000;
  }
  if (packages.size !== 2) throw new Error("package authentication order omitted an exact CodeStory arm");
  return {
    packages,
    lifecycle: {
      contract: "codestory.agent-benchmark-exact-lifecycle/v1",
      package_authentication_order: order,
      package_authentication_ms: armTimings,
      total_package_authentication_ms: Math.round((performance.now() - started) * 1000) / 1000,
    },
  };
}

function validateExactCandidateShape(opts, tasks) {
  if (!opts.exactCandidate) return;
  if (tasks.length !== 18 || new Set(tasks.map((task) => task.id)).size !== 18) {
    throw new Error("exact-candidate mode requires exactly 18 pinned tasks");
  }
  if (
    tasks.some((task) => task.suite != null) &&
    tasks.map((task) => task.id).join(",") !== EXACT_CANDIDATE_TASK_IDS.join(",")
  ) {
    throw new Error("exact-candidate task ids or order differ from the pinned 18-task window");
  }
  if (tasks.some((task) => task.suite != null)) {
    const taskContract = tasks.map((task) => {
      const { manifest_path: _manifestPath, ...snapshot } = taskSnapshotForResult(task);
      return snapshot;
    });
    if (sha256Bytes(stableJsonForHash(taskContract)) !== EXACT_CANDIDATE_TASK_CONTRACT_SHA256) {
      throw new Error("exact-candidate prompts or qualification inputs differ from the pinned task window");
    }
  }
  if (opts.taskSuite !== "language-expansion-holdout") {
    throw new Error("exact-candidate mode requires the pinned language-expansion-holdout suite");
  }
  if (opts.repeats !== 3 || opts.arms.join(",") !== EXACT_CANDIDATE_ARMS.join(",")) {
    throw new Error("exact-candidate mode requires the frozen three arms and exactly 3 repeats");
  }
  if (opts.reuseBaselineFrom) {
    throw new Error("baseline reuse is forbidden in exact-candidate mode");
  }
}

function normalizeCodestoryProjectManifest(filePath, value) {
  if (value == null) {
    return null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`Task manifest codestory_project_manifest must be an object: ${filePath}`);
  }
  const declaredPath = String(value.path ?? "").trim();
  if (!declaredPath || path.isAbsolute(declaredPath) || path.win32.isAbsolute(declaredPath)) {
    throw new Error(`Task manifest codestory_project_manifest.path must be relative: ${filePath}`);
  }
  const sourcePath = assertPathInside(
    path.dirname(filePath),
    path.resolve(path.dirname(filePath), declaredPath),
    "Task manifest codestory_project_manifest.path",
  );
  if (!existsSync(sourcePath) || !statSync(sourcePath).isFile()) {
    throw new Error(`Task manifest codestory_project_manifest.path does not name a file: ${filePath}`);
  }
  return {
    declared_path: declaredPath.replaceAll(path.sep, "/"),
    source_path: sourcePath,
    sha256: normalizeSha256(value.sha256, `Task manifest codestory_project_manifest.sha256: ${filePath}`),
  };
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
    codestory_project_manifest: normalizeCodestoryProjectManifest(filePath, repo.codestory_project_manifest),
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
    manifest_codestory_project_manifest: config.codestory_project_manifest,
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
  if (raw.file_local != null && typeof raw.file_local !== "boolean") {
    throw new Error(`Task manifest file_local must be a boolean: ${filePath}`);
  }

  return {
    id,
    name: String(raw.name ?? raw.id ?? path.basename(filePath, path.extname(filePath))),
    suite: raw.suite ?? null,
    repo,
    repo_metadata: typeof raw.repo === "object" ? raw.repo : null,
    task_class: taskClass,
    file_local: raw.file_local === true,
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
      file_local: task.file_local === true,
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
  let manifestCanaryTaskId = null;
  for (const filePath of await listManifestFiles(manifestPath)) {
    const raw = await loadJsonFile(filePath);
    const declaredCanary = !Array.isArray(raw)
      ? String(raw?.canary_task_id ?? raw?.canaryTaskId ?? "").trim()
      : "";
    const rows = Array.isArray(raw.tasks) ? raw.tasks : Array.isArray(raw) ? raw : [raw];
    let selectedRows = 0;
    for (const row of rows) {
      const task = normalizeManifestTask(filePath, row, opts);
      if (!opts.taskSuite || task.suite === opts.taskSuite || row.suite === opts.taskSuite) {
        tasks.push(task);
        selectedRows += 1;
      }
    }
    if (declaredCanary && selectedRows > 0) {
      validateManifestTaskId(filePath, declaredCanary);
      if (manifestCanaryTaskId && manifestCanaryTaskId !== declaredCanary) {
        throw new Error(
          `Task manifests declare conflicting canary_task_id values: ${manifestCanaryTaskId}, ${declaredCanary}`,
        );
      }
      manifestCanaryTaskId = declaredCanary;
    }
  }
  if (!tasks.length) {
    throw new Error(`Task manifest path contained no tasks: ${manifestPath}`);
  }
  opts.manifestCanaryTaskId = manifestCanaryTaskId;
  opts.canaryTaskId ??= manifestCanaryTaskId;
  if (opts.canaryTaskId && !tasks.some((task) => task.id === opts.canaryTaskId)) {
    throw new Error(`Canary task '${opts.canaryTaskId}' was not found in the selected task manifest`);
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

function taskShardIndex(taskId, shardCount) {
  if (!Number.isInteger(shardCount) || shardCount < 1) {
    throw new Error("shardCount must be a positive integer");
  }
  const prefix = createHash("sha256").update(String(taskId)).digest("hex").slice(0, 12);
  return Number.parseInt(prefix, 16) % shardCount;
}

function tasksForShard(tasks, shardCount, shardIndex) {
  return tasks.filter((task) => taskShardIndex(task.id, shardCount) === shardIndex);
}

function sortedUniqueStrings(values, label) {
  if (!Array.isArray(values) || values.length === 0) {
    throw new Error(`${label} must be a non-empty array`);
  }
  const normalized = values.map((value) => String(value ?? "").trim());
  if (normalized.some((value) => !MANIFEST_TASK_ID_PATTERN.test(value))) {
    throw new Error(`${label} must contain benchmark task IDs`);
  }
  const sorted = [...new Set(normalized)].sort();
  if (sorted.length !== normalized.length || JSON.stringify(sorted) !== JSON.stringify(normalized)) {
    throw new Error(`${label} must be sorted and unique`);
  }
  return sorted;
}

async function loadReleaseEvidenceCorpusContract(tasks, opts) {
  const declaredPath = process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT?.trim();
  if (!process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT) {
    if (declaredPath) {
      throw new Error("CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT requires CODESTORY_RELEASE_EVIDENCE_COMMIT");
    }
    return null;
  }
  if (!declaredPath) {
    throw new Error("release evidence requires CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT");
  }
  const contractPath = assertPathInside(repoRoot, path.resolve(repoRoot, declaredPath), "Release evidence corpus contract");
  if (!existsSync(contractPath) || !statSync(contractPath).isFile()) {
    throw new Error(`Release evidence corpus contract does not exist: ${declaredPath}`);
  }
  const bytes = await readFile(contractPath);
  const contract = JSON.parse(bytes.toString("utf8"));
  if (contract?.schema_version !== 1 || typeof contract?.corpus_id !== "string") {
    throw new Error("Release evidence corpus contract must use schema_version 1 and name a corpus_id");
  }
  if (contract.corpus_id !== process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID) {
    throw new Error("Release evidence corpus contract corpus_id does not match CODESTORY_RELEASE_EVIDENCE_CORPUS_ID");
  }
  const taskIds = sortedUniqueStrings(contract.task_ids, "Release evidence corpus contract task_ids");
  const loadedTaskIds = [...new Set(tasks.map((task) => task.id))].sort();
  if (JSON.stringify(taskIds) !== JSON.stringify(loadedTaskIds)) {
    throw new Error(
      `Release evidence task selection does not match corpus contract: expected ${taskIds.join(", ")}, got ${loadedTaskIds.join(", ")}`,
    );
  }
  const selectedRuntimeModes = opts.packetRuntimeMode === "both"
    ? ["cold_cli_packet", "warm_stdio_packet"]
    : [`${opts.packetRuntimeMode.replaceAll("-", "_")}_packet`];
  if (JSON.stringify(contract.runtime_modes) !== JSON.stringify(selectedRuntimeModes)) {
    throw new Error("Release evidence packet runtime modes do not match corpus contract");
  }
  if (!Number.isInteger(contract.repeats) || contract.repeats < 1 || contract.repeats !== opts.repeats) {
    throw new Error("Release evidence repeat count does not match corpus contract");
  }
  if (!contract.task_manifests || typeof contract.task_manifests !== "object" || Array.isArray(contract.task_manifests)) {
    throw new Error("Release evidence corpus contract must bind task_manifests");
  }
  const taskManifests = {};
  const taskRepositories = {};
  for (const task of tasks) {
    const declaration = contract.task_manifests[task.id];
    if (!declaration || typeof declaration !== "object") {
      throw new Error(`Release evidence corpus contract is missing task manifest for ${task.id}`);
    }
    const manifestPath = assertPathInside(repoRoot, path.resolve(repoRoot, declaration.path ?? ""), `Task manifest contract path for ${task.id}`);
    if (path.resolve(task.manifest_path) !== manifestPath) {
      throw new Error(`Release evidence task manifest path does not match loaded task for ${task.id}`);
    }
    const manifestBytes = await readFile(manifestPath);
    const manifestSha256 = sha256Bytes(manifestBytes);
    if (manifestSha256 !== normalizeSha256(declaration.sha256, `Task manifest contract hash for ${task.id}`)) {
      throw new Error(`Release evidence task manifest hash does not match for ${task.id}`);
    }
    taskManifests[task.id] = {
      path: path.relative(repoRoot, manifestPath).replaceAll(path.sep, "/"),
      sha256: manifestSha256,
    };
    taskRepositories[task.id] = task.repo;
  }
  if (Object.keys(contract.task_manifests).some((taskId) => !taskIds.includes(taskId))) {
    throw new Error("Release evidence corpus contract binds an unselected task manifest");
  }
  const projectManifests = {};
  for (const task of tasks) {
    const manifest = task.repo_metadata?.codestory_project_manifest;
    if (!manifest) continue;
    const config = ALL_REPOS[task.repo];
    const normalized = config?.manifest_codestory_project_manifest;
    if (!normalized || normalized.sha256 !== manifest.sha256) {
      throw new Error(`Release evidence project manifest declaration is inconsistent for ${task.id}`);
    }
    projectManifests[task.id] = {
      path: path.relative(repoRoot, normalized.source_path).replaceAll(path.sep, "/"),
      sha256: normalized.sha256,
    };
  }
  if (JSON.stringify(contract.project_manifests ?? {}) !== JSON.stringify(projectManifests)) {
    throw new Error("Release evidence corpus contract project manifest bindings do not match selected tasks");
  }
  return {
    path: path.relative(repoRoot, contractPath).replaceAll(path.sep, "/"),
    sha256: sha256Bytes(bytes),
    corpus_id: contract.corpus_id,
    task_ids: taskIds,
    runtime_modes: selectedRuntimeModes,
    repeats: contract.repeats,
    task_manifests: taskManifests,
    task_repositories: taskRepositories,
    project_manifests: projectManifests,
  };
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
  blockers.push(...unsupportedRetrievalContractRequests(process.env));
  if (blockers.length) {
    throw new Error(`Publishable benchmark shape is incomplete:\n- ${blockers.join("\n- ")}`);
  }
}

async function runProcess(command, args, options = {}) {
  if (options.signal?.aborted) {
    return {
      status: "aborted",
      exitCode: null,
      signal: null,
      stdout: "",
      stderr: `${options.abortMessage ?? "Process aborted by benchmark fail-fast."}\n`,
      error: null,
      timedOut: false,
      aborted: true,
    };
  }
  return await new Promise((resolve) => {
    const child = (options.spawnProcess ?? spawn)(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      shell: false,
      stdio: options.stdin == null ? ["ignore", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    let aborted = false;
    let settled = false;
    let forceKillTimer = null;
    let outputBytes = 0;
    const terminate = (signal, message = null) => {
      if (message) {
        stderr += `\n${message}\n`;
      }
      terminateProcess(child, signal, options);
      const forceKillAfterMs = options.forceKillAfterMs ?? 5000;
      if (forceKillAfterMs > 0 && signal !== "SIGKILL") {
        forceKillTimer ??= setTimeout(
          () => terminateProcess(child, "SIGKILL", options),
          forceKillAfterMs,
        );
      }
    };
    const onAbort = () => {
      if (settled || aborted || timedOut) {
        return;
      }
      aborted = true;
      terminate("SIGTERM", options.abortMessage ?? "Process aborted by benchmark fail-fast.");
    };
    options.signal?.addEventListener("abort", onAbort, { once: true });
    if (options.signal?.aborted) {
      onAbort();
    }
    const timeoutTimer = options.timeoutMs
      ? setTimeout(() => {
          if (settled || aborted) {
            return;
          }
          timedOut = true;
          const message = options.timeoutMessage ?? `Process timed out after ${options.timeoutMs}ms.`;
          terminate("SIGTERM", message);
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
      options.signal?.removeEventListener("abort", onAbort);
      resolve({ timedOut, aborted, ...payload });
    }
    child.stdout.on("data", (chunk) => {
      outputBytes += chunk.length;
      if (options.maxOutputBytes && outputBytes > options.maxOutputBytes) {
        terminate("SIGTERM", `Process output exceeded ${options.maxOutputBytes} bytes.`);
        return;
      }
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      outputBytes += chunk.length;
      if (options.maxOutputBytes && outputBytes > options.maxOutputBytes) {
        terminate("SIGTERM", `Process output exceeded ${options.maxOutputBytes} bytes.`);
        return;
      }
      stderr += chunk.toString();
    });
    if (options.stdin != null) {
      child.stdin.end(options.stdin);
    }
    child.on("error", (error) => {
      finish({
        status: timedOut ? "timeout" : aborted ? "aborted" : "error",
        exitCode: null,
        signal: null,
        stdout,
        stderr,
        error: error.message,
      });
    });
    child.on("close", (exitCode, signal) => {
      finish({
        status: timedOut ? "timeout" : aborted ? "aborted" : exitCode === 0 ? "pass" : "fail",
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

function createAsyncQueue() {
  const values = [];
  const waiters = [];
  let closed = false;
  return {
    push(value) {
      if (closed) {
        throw new Error("cannot push to a closed queue");
      }
      const waiter = waiters.shift();
      if (waiter) {
        waiter(value);
      } else {
        values.push(value);
      }
    },
    async shift() {
      if (values.length) {
        return values.shift();
      }
      if (closed) {
        return null;
      }
      return await new Promise((resolve) => waiters.push(resolve));
    },
    close() {
      closed = true;
      while (waiters.length) {
        waiters.shift()(null);
      }
    },
    remaining() {
      return [...values];
    },
  };
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

async function gitCheckedOutput(args, cwd, options = {}) {
  const normalizedOptions = typeof options === "number"
    ? { timeoutMs: options }
    : options;
  const result = await runCheckedProcess("git", args, {
    ...normalizedOptions,
    cwd,
  });
  return result.stdout.trim();
}

async function installCodestoryProjectManifest(config, checkoutPath, opts) {
  opts.signal?.throwIfAborted();
  const manifest = config.manifest_codestory_project_manifest ?? null;
  if (!manifest) {
    return null;
  }
  const workspacePath = path.resolve(config.path);
  assertPathInside(checkoutPath, workspacePath, "CodeStory project manifest workspace path");
  const destination = assertPathInside(
    workspacePath,
    path.join(workspacePath, "codestory_project.json"),
    "CodeStory project manifest destination",
  );
  const relativeDestination = path.relative(checkoutPath, destination).replaceAll(path.sep, "/");
  if (!relativeDestination || relativeDestination.startsWith("../") || path.isAbsolute(relativeDestination)) {
    throw new Error(`CodeStory project manifest destination escapes checkout: ${destination}`);
  }
  const tracked = await runProcess("git", ["-C", checkoutPath, "ls-files", "--error-unmatch", "--", relativeDestination], {
    timeoutMs: opts.timeoutMs,
    signal: opts.signal,
    spawnProcess: opts.spawnProcess,
    forceKillAfterMs: opts.forceKillAfterMs,
  });
  opts.signal?.throwIfAborted();
  if (tracked.exitCode === 0) {
    throw new Error(`Refusing to replace upstream-tracked ${relativeDestination} in ${config.name}`);
  }
  const sourceBytes = await readFile(manifest.source_path);
  const sourceSha256 = sha256Bytes(sourceBytes);
  if (sourceSha256 !== manifest.sha256) {
    throw new Error(
      `CodeStory project manifest hash mismatch for ${config.name}: expected ${manifest.sha256}, got ${sourceSha256}`,
    );
  }
  const infoExclude = path.join(checkoutPath, ".git", "info", "exclude");
  const ignoreEntry = `/${relativeDestination}`;
  const currentExclude = existsSync(infoExclude) ? await readFile(infoExclude, "utf8") : "";
  if (!currentExclude.split(/\r?\n/u).includes(ignoreEntry)) {
    await writeFile(infoExclude, `${currentExclude}${currentExclude.endsWith("\n") || !currentExclude ? "" : "\n"}${ignoreEntry}\n`, "utf8");
  }
  await writeFile(destination, sourceBytes);
  const installedBytes = await readFile(destination);
  const installedSha256 = sha256Bytes(installedBytes);
  if (installedSha256 !== manifest.sha256) {
    throw new Error(`Installed CodeStory project manifest hash mismatch for ${config.name}`);
  }
  const gitOptions = {
    timeoutMs: opts.timeoutMs,
    signal: opts.signal,
    spawnProcess: opts.spawnProcess,
    forceKillAfterMs: opts.forceKillAfterMs,
  };
  await gitCheckedOutput(["-C", checkoutPath, "check-ignore", "-q", "--", relativeDestination], repoRoot, gitOptions);
  const dirty = await gitCheckedOutput(["-C", checkoutPath, "status", "--porcelain"], repoRoot, gitOptions);
  if (dirty) {
    throw new Error(`Installing CodeStory project manifest dirtied ${config.name}: ${dirty}`);
  }
  return {
    source_path: path.relative(repoRoot, manifest.source_path).replaceAll(path.sep, "/"),
    declared_sha256: manifest.sha256,
    installed_path: relativeDestination,
    installed_sha256: installedSha256,
    ignored: true,
  };
}

async function scrubMaterializedCheckout(config, checkoutPath, opts) {
  const processOptions = {
    timeoutMs: opts.timeoutMs,
    signal: opts.signal,
    spawnProcess: opts.spawnProcess,
    forceKillAfterMs: opts.forceKillAfterMs,
  };
  await runCheckedProcess("git", ["-C", checkoutPath, "reset", "--hard", config.ref], {
    ...processOptions,
  });
  await runCheckedProcess("git", ["-C", checkoutPath, "clean", "-ffdqx"], {
    ...processOptions,
  });
  const head = (await gitCheckedOutput(["-C", checkoutPath, "rev-parse", "HEAD"], repoRoot, processOptions)).toLowerCase();
  if (head !== String(config.ref).toLowerCase()) {
    throw new Error(`Materialized repo ${config.name} HEAD ${head} does not match pinned ref ${config.ref}`);
  }
  const dirty = await gitCheckedOutput(["-C", checkoutPath, "status", "--porcelain"], repoRoot, processOptions);
  if (dirty) {
    throw new Error(`Materialized repo ${config.name} is dirty after scrub: ${dirty}`);
  }
  const remaining = await gitCheckedOutput(["-C", checkoutPath, "clean", "-ffdqx", "-n"], repoRoot, processOptions);
  if (remaining) {
    throw new Error(`Materialized repo ${config.name} retains untracked or ignored files after scrub: ${remaining}`);
  }
  config.installed_codestory_project_manifest = null;
}

async function materializeRepos(tasks, opts) {
  const repos = uniqueTaskRepos(tasks);
  if (!repos.size) {
    return;
  }
  await mkdir(opts.repoCacheDir, { recursive: true });
  for (const [name, config] of repos) {
    opts.signal?.throwIfAborted();
    const processOptions = {
      timeoutMs: opts.timeoutMs,
      signal: opts.signal,
      spawnProcess: opts.spawnProcess,
      forceKillAfterMs: opts.forceKillAfterMs,
    };
    const checkoutPath = path.resolve(config.checkout_path ?? path.join(opts.repoCacheDir, name));
    assertPathInside(opts.repoCacheDir, checkoutPath, "Materialized repo checkout path");
    assertPathInside(checkoutPath, config.path, `Materialized repo workspace path for ${name}`);
    if (!existsSync(checkoutPath)) {
      await mkdir(path.dirname(checkoutPath), { recursive: true });
      console.log(`cloning ${name} ${redactUrlForDisplay(config.url)} -> ${checkoutPath}`);
      await runCheckedProcess("git", ["clone", "--filter=blob:none", "--no-checkout", config.url, checkoutPath], {
        ...processOptions,
      });
    } else {
      const remote = await runCheckedProcess("git", ["-C", checkoutPath, "remote", "get-url", "origin"], {
        ...processOptions,
      });
      if (remote.stdout.trim() !== config.url) {
        throw new Error(
          `Repo cache for ${name} has origin ${redactUrlForDisplay(remote.stdout.trim())}, expected ${redactUrlForDisplay(config.url)}. Use a different --repo-cache-dir.`,
        );
      }
    }
    console.log(`fetching ${name} ref ${config.ref}`);
    await runCheckedProcess("git", ["-C", checkoutPath, "fetch", "--depth=1", "origin", config.ref], {
      ...processOptions,
    });
    await runCheckedProcess("git", ["-C", checkoutPath, "checkout", "--detach", "FETCH_HEAD"], {
      ...processOptions,
    });
    await scrubMaterializedCheckout(config, checkoutPath, opts);
    if (!existsSync(config.path)) {
      throw new Error(`Materialized repo ${name} is missing workspace path: ${config.path}`);
    }
    config.installed_codestory_project_manifest = await installCodestoryProjectManifest(config, checkoutPath, opts);
  }
}

function composePrompt(repoName, repoConfig, armName, task = null, context = {}) {
  const taskPrompt = task?.prompt ?? repoConfig.prompt;
  const taskHeader = task
    ? `Task id: ${task.id}
Task class: ${task.task_class ?? "unspecified"}`
    : "";
  const packetFirstCommand =
    isCodeStoryArm(armName)
      ? packetFirstCommandForPrompt(taskPrompt, task)
      : null;
  const packetFirstBlock = packetFirstCommand && !context.codestoryPrelude?.packet
    ? `
Required first repository-context command:
\`\`\`${packetFirstCommandFenceLanguage()}
${packetFirstCommand}
\`\`\`

Run that answer packet before any repository search, direct source read, git command, CodeStory primitive, or help/probe command. The benchmark treats help/probe commands such as \`--help\` as not packet-first.`
    : "";
  const stopContractBlock =
    isCodeStoryArm(armName)
      ? isPacketProjectionV3(context.codestoryPrelude?.packet)
        ? `
The packet is an evidence-only projection. Use its evidence rows directly. For every requested material stage that a row establishes, state a direct subject-verb claim naming the subject and established action before describing any gaps. Do not substitute a heading, symbol inventory, or adjacent partial observation, and make the claim no broader than the cited row. When a higher-level action and its mechanism are established by the same evidence rows, name both: the subject performs, drives, or handles the action by calling the mechanism. Avoid weak role labels such as \`is the ... symbol\` or \`participates in\`; do not report only that the subject \`calls\` a downstream target. Then name every gap, scoped only to requested links or stages the rows do not establish. Do not infer proof, completeness, runtime behavior, or absence from availability. If status is \`continuation_available\`, execute exactly the declared one-shot continuation, then reassess its returned gaps without another retrieval call. An exact focused source read is allowed only for a file-local task where the user named that exact file, or when a material \`evidence_missing\`, \`Unknown\`, or \`Unavailable\` boundary authenticates that exact path. A file mentioned inside this broad flow question does not authorize a read. Perform at most one bounded read per authorized path. An \`output_budget_exceeded\` gap is descriptive and does not authorize a source read or another repository tool by itself. A packet-cited path or range alone is not authorization, and an unrelated gap does not authorize arbitrary files.`
        : `
The packet's own \`disposition\` is the complete control contract. The benchmark's expected-answer manifest is never shown to you and does not authorize extra retrieval. Stop on \`supported\`, \`not_established\`, or \`unavailable\`. A \`not_established\` packet can still contain directly useful support: answer those established parts, identify the material gaps, and do not infer the missing links. On \`drill_once\`, execute exactly the declared one-shot packet continuation, apply the same terminal answer rule, and then stop regardless of its result.`
      : "";
  const harnessPacketBlock = packetPreludePromptBlock(context.codestoryPrelude);
  const baselineContextBlock = baselinePreludePromptBlock(context.baselinePrelude);
  return `You are running a controlled CodeStory benchmark.

Repository: ${repoName}
${taskHeader}
Task: ${taskPrompt}

Arm: ${armName}
Instruction: ${isCodeStoryArm(armName)
    ? codeStoryArmInstruction(context.codestoryPrelude?.packet)
    : ARMS[armName]}
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
    return `& $env:CODESTORY_CLI packet --project . --question ${shellSingleQuoted(question, platform)}${taskClass} --budget standard --format json`;
  }
  return `"$CODESTORY_CLI" packet --project . --question ${shellSingleQuoted(question, platform)}${taskClass} --budget standard --format json`;
}

function packetPreludePromptBlock(prelude) {
  if (!prelude?.packet) {
    return "";
  }
  if (isPacketProjectionV3(prelude.packet)) {
    return `
The benchmark harness already ran the required first repository-context command before starting you:
\`\`\`${packetFirstCommandFenceLanguage()}
${prelude.public.command}
\`\`\`

Use the evidence rows as bounded repository evidence and preserve every gap and
availability boundary. This packet does not prove claims. Do not repeat the
initial packet call or synthesize legacy support/disposition semantics.

CodeStory packet JSON excerpt:
\`\`\`json
${JSON.stringify(packetForAgentPrompt(prelude.packet), null, 2)}
\`\`\``;
  }
  return `
The benchmark harness already ran the required first repository-context command before starting you:
\`\`\`${packetFirstCommandFenceLanguage()}
${prelude.public.command}
\`\`\`

Use the compiled \`support\` units as evidence and obey only \`disposition\` for
control flow. The task manifest is withheld and has no effect on whether you
continue. Do not repeat the initial packet call. Preserve exact source
identifiers and paths from support summaries and citations.

CodeStory packet JSON excerpt:
\`\`\`json
${JSON.stringify(packetForAgentPrompt(prelude.packet), null, 2)}
\`\`\``;
}

function packetForAgentPrompt(packet) {
  if (!packet || typeof packet !== "object") {
    return packet;
  }
  if (isPacketProjectionV3(packet)) {
    return {
      schema_version: 3,
      kind: packet.kind ?? null,
      status: packet.status ?? null,
      identity: packet.identity ?? null,
      publication: packet.publication ?? null,
      retrieval: packet.retrieval ?? null,
      evidence: (packet.evidence ?? []).map((row) => ({
        identity: row?.identity ?? null,
        kind: row?.kind ?? null,
        path: row?.path ?? null,
        symbol_id: row?.symbol_id ?? null,
        start_line: row?.start_line ?? null,
        end_line: row?.end_line ?? null,
        summary: row?.summary == null
          ? null
          : truncatePacketPromptText(row.summary, 1_200),
      })),
      gaps: (packet.gaps ?? []).map((row) => ({
        identity: row?.identity ?? null,
        kind: row?.kind ?? null,
        message: row?.message == null
          ? null
          : truncatePacketPromptText(row.message, 800),
      })),
      continuation: packet.continuation ?? null,
      diagnostics: packet.diagnostics ?? null,
      ...(packet.kind === "budget_exceeded"
        ? {
            maximum_bytes: packet.maximum_bytes ?? null,
            required_complete_bytes: packet.required_complete_bytes ?? null,
          }
        : {}),
    };
  }
  return {
    packet_id: packet.packet_id ?? null,
    question: packet.question ?? null,
    support: Array.isArray(packet.support) ? packet.support : [],
    disposition: packet.disposition ?? null,
    answer: packet.answer
      ? {
          summary: packet.answer.summary ?? null,
          text: truncatePacketPromptText(packetAnswerText(packet), 4000),
          citations: (packet.answer.citations ?? []).map(leanPacketCitation),
        }
      : null,
  };
}

function packetPreludeManifestComplete(publicPrelude) {
  const quality = publicPrelude?.packet_manifest_quality;
  if (!quality?.pass) {
    return false;
  }
  if (publicPrelude?.packet_schema_version === 3) {
    return (
      publicPrelude?.packet_projection_kind === "complete" &&
      publicPrelude?.packet_evidence_availability?.status === "available" &&
      (presentFiniteNumber(publicPrelude?.packet_evidence_count) ?? 0) > 0
    );
  }
  if (publicPrelude?.packet_disposition_kind !== "supported") {
    return false;
  }
  if ((presentFiniteNumber(publicPrelude?.packet_support_count) ?? 0) <= 0) {
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
  const claimText = isPacketProjectionV3(packet)
    ? (packet.evidence ?? [])
      .map((row) => [
        row?.path,
        row?.symbol_id,
        row?.start_line == null ? null : `line ${row.start_line}`,
        row?.summary,
      ].filter(Boolean).join(" "))
      .filter(Boolean)
      .join("\n")
    : (packet.support ?? [])
    .map((support) => String(support?.summary ?? "").trim())
    .filter(Boolean)
    .join("\n");
  const gapText = isPacketProjectionV3(packet)
    ? (packet.gaps ?? [])
      .map((gap) => [gap?.kind, gap?.message].filter(Boolean).join(" "))
      .filter(Boolean)
      .join("\n")
    : "";
  const text = [
    packet.answer?.summary ?? "",
    packetAnswerText(packet),
    citationText,
    claimText,
    gapText,
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
    .replace(/^\/\?\/(?=[A-Za-z]:\/)/, "")
    .replace(/^\.\//, "");
}

function pathMatchesLike(actual, expected) {
  const left = normalizePathLike(actual).toLowerCase();
  const right = normalizePathLike(expected).toLowerCase();
  return left === right || left.endsWith(`/${right}`);
}

function sourceReadPathIdentity(value, projectRoot) {
  const normalized = normalizePathLike(value);
  if (!normalized) return null;
  const normalizedRoot = normalizePathLike(projectRoot);
  const windowsPath = /^[A-Za-z]:\//.test(normalized) || /^[A-Za-z]:\//.test(normalizedRoot);
  let relative = normalized;
  if (isAbsolutePathLike(normalized)) {
    if (!normalizedRoot || !isAbsolutePathLike(normalizedRoot)) return null;
    relative = normalizePathLike(
      windowsPath
        ? path.win32.relative(normalizedRoot.replaceAll("/", "\\"), normalized.replaceAll("/", "\\"))
        : path.relative(normalizedRoot, normalized),
    );
  }
  const components = relative.split("/");
  if (
    !relative ||
    isAbsolutePathLike(relative) ||
    components.some((component) =>
      !component || component === "." || component === ".." || component.includes("\0")
    )
  ) {
    return null;
  }
  const caseInsensitive = windowsPath || process.platform === "win32";
  return {
    relative,
    key: caseInsensitive ? relative.toLowerCase() : relative,
  };
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

  const shellArgument = String.raw`(?:'[^'\r\n]*'|"[^"\r\n]*"|[^\s;|]+)`;
  const patterns = [
    /\bGet-Content\b(?:\s+-(?!LiteralPath\b|Path\b)[A-Za-z]+)*\s+(?:-(?:LiteralPath|Path)\s+)?['"]*([^'";|`\r\n]+)['"]*/gi,
    /\bcat\b\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    /\btype\b\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    /\bnl\b(?:\s+-[A-Za-z]+)*\s+['"]*([^'";|`\r\n]+)['"]*/gi,
    new RegExp(
      String.raw`\bsed\b[ \t]+-n[ \t]+${shellArgument}[ \t]+['"]*([^'";|\x60\r\n]+)['"]*`,
      "gi",
    ),
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
      harness_semantics: null,
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
    if (
      eventTypeOf(event).startsWith("harness.command.") &&
      item.harness_semantics?.source === "codestory_packet_prelude_v1" &&
      item.harness_semantics?.category === "codestory_cli" &&
      item.harness_semantics?.operation === "packet"
    ) {
      existing.harness_semantics = item.harness_semantics;
    }
    byId.set(id, existing);
  });

  for (const command of byId.values()) {
    command.category = command.harness_semantics?.category ?? commandCategory(command.command);
    command.codestory_operation = command.harness_semantics?.operation ?? null;
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

function interactionTurnTelemetry(events) {
  let modelMessages = 0;
  let toolActions = 0;
  let failedToolActions = 0;
  let reasoningItemsExcluded = 0;
  let errorItemsExcluded = 0;
  for (const event of events) {
    const eventType = String(event?.type ?? event?.event ?? "").toLowerCase();
    if (!(eventType === "item.completed" || eventType.endsWith(".completed"))) {
      continue;
    }
    const item = itemOf(event);
    const itemType = String(item.type ?? "").toLowerCase();
    if (itemType === "reasoning") {
      reasoningItemsExcluded += 1;
      continue;
    }
    if (itemType === "error") {
      errorItemsExcluded += 1;
      continue;
    }
    if (itemType === "agent_message") {
      modelMessages += 1;
      continue;
    }
    if (isToolType(itemType)) {
      toolActions += 1;
      if (
        item.error != null ||
        event.error != null ||
        String(item.status ?? "completed").toLowerCase() === "failed"
      ) {
        failedToolActions += 1;
      }
    }
  }
  return {
    total: modelMessages + toolActions,
    model_messages: modelMessages,
    tool_actions: toolActions,
    failed_tool_actions: failedToolActions,
    reasoning_items_excluded: reasoningItemsExcluded,
    error_items_excluded: errorItemsExcluded,
    taxonomy: "completed_agent_messages_plus_tool_actions_v1",
  };
}

const MATERIAL_SOURCE_GAP_CODES = new Set([
  "unknown",
  "unavailable",
  "not_established",
  "evidence_missing",
  "retrieval_unavailable",
  "source_unavailable",
]);

const MATERIAL_SOURCE_GAP_TEXT =
  /\b(?:unknown|unavailable|not_established|evidence gap|material gap|missing evidence|evidence_missing|retrieval_unavailable|source_unavailable)\b/i;
const NON_AUTHORIZING_GAP_TEXT = /\b(?:output_budget_exceeded|continuation_required)\b/i;

function textMentionsExactPath(value, normalizedPath) {
  if (!normalizedPath) return false;
  const text = String(value ?? "").replaceAll("\\", "/");
  let offset = text.indexOf(normalizedPath);
  while (offset >= 0) {
    const before = offset === 0 ? "" : text[offset - 1];
    const afterOffset = offset + normalizedPath.length;
    const after = afterOffset >= text.length ? "" : text[afterOffset];
    const pathCharacter = /[A-Za-z0-9._~:/-]/;
    if ((!before || !pathCharacter.test(before)) && (!after || !pathCharacter.test(after))) {
      return true;
    }
    offset = text.indexOf(normalizedPath, offset + 1);
  }
  return false;
}

function gapTextUniquelyCorrelatesPath(value, normalizedPath) {
  if (!textMentionsExactPath(value, normalizedPath)) return false;
  const text = String(value ?? "").replaceAll("\\", "/");
  const paths = new Set(
    (text.match(/(?:[A-Za-z0-9_@+.-]+\/)+[A-Za-z0-9_@+.-]*[A-Za-z0-9_@+-]|[A-Za-z0-9_@+-]+\.[A-Za-z0-9_@+.-]*[A-Za-z0-9_@+-]/gu) ?? [])
      .map(normalizePathLike),
  );
  paths.add(normalizedPath);
  return paths.size === 1;
}

function explicitEvidenceGapAuthorizesPath(value, normalizedPath, seen = new Set()) {
  if (typeof value === "string") {
    const trimmed = value.trim();
    if ((trimmed.startsWith("{") && trimmed.endsWith("}")) ||
        (trimmed.startsWith("[") && trimmed.endsWith("]"))) {
      try {
        return explicitEvidenceGapAuthorizesPath(JSON.parse(trimmed), normalizedPath, seen);
      } catch {
        // Fall through to bounded line correlation for non-JSON tool output.
      }
    }
    return trimmed.split(/\r?\n/u).some((line) =>
      MATERIAL_SOURCE_GAP_TEXT.test(line) &&
      !NON_AUTHORIZING_GAP_TEXT.test(line) &&
      gapTextUniquelyCorrelatesPath(line, normalizedPath)
    );
  }
  if (Array.isArray(value)) {
    return value.some((entry) => explicitEvidenceGapAuthorizesPath(entry, normalizedPath, seen));
  }
  if (!value || typeof value !== "object" || seen.has(value)) return false;
  seen.add(value);

  const code = String(value.kind ?? value.status ?? "").trim().toLowerCase();
  const classificationText = [value.kind, value.status, value.message, value.reason, value.reasons]
    .flat()
    .filter((entry) => entry != null)
    .join(" ");
  if (MATERIAL_SOURCE_GAP_CODES.has(code) && !NON_AUTHORIZING_GAP_TEXT.test(classificationText)) {
    const correlationText = ["message", "reason", "reasons", "detail", "details", "path", "source_path", "source_paths"]
      .flatMap((field) => Array.isArray(value[field]) ? value[field] : [value[field]])
      .filter((entry) => entry != null)
      .join("\n");
    if (gapTextUniquelyCorrelatesPath(correlationText, normalizedPath)) return true;
  }

  return ["gaps", "gap", "disposition", "result", "structuredContent", "structured_content", "content", "text"]
    .some((field) => explicitEvidenceGapAuthorizesPath(value[field], normalizedPath, seen));
}

function canonicalMcpEvidenceResult(result) {
  if (!result || typeof result !== "object") return null;
  const structured = result.structuredContent ?? result.structured_content ?? null;
  const textItems = Array.isArray(result.content)
    ? result.content.filter((entry) => entry?.type === "text" && typeof entry.text === "string")
    : [];
  const parsedText = [];
  for (const item of textItems) {
    try {
      parsedText.push(JSON.parse(item.text));
    } catch {
      return null;
    }
  }
  if (structured != null) {
    if (parsedText.some((value) => stableJsonForHash(value) !== stableJsonForHash(structured))) return null;
    return structured;
  }
  if (parsedText.length === 1) return parsedText[0];
  if (parsedText.length > 1) {
    const expected = stableJsonForHash(parsedText[0]);
    return parsedText.every((value) => stableJsonForHash(value) === expected) ? parsedText[0] : null;
  }
  return result;
}

function directSourceReadAuthorization(read, commands, events, projectRoot, context) {
  if (context.arm === "without_codestory") {
    return { status: "baseline_local_exploration", reason: "without_codestory" };
  }
  const prompt = String(context.task?.prompt ?? "").replaceAll("\\", "/");
  const readIdentity = sourceReadPathIdentity(read.path, projectRoot);
  if (!readIdentity) {
    return { status: "unauthorized", reason: null };
  }
  const relativePath = readIdentity.relative;
  const readEventIndex = read.event_index ?? -1;
  const repeatedRead = commands.some((command) =>
    command.category === "direct_file_read" &&
    (command.started_event_index ?? command.completed_event_index ?? -1) < readEventIndex &&
    extractDirectFileReads(command.command).some((candidate) =>
      sourceReadPathIdentity(candidate, projectRoot)?.key === readIdentity.key
    )
  );
  if (repeatedRead) {
    return { status: "unauthorized", reason: "repeated_source_read" };
  }
  if (
    context.task?.file_local === true &&
    relativePath &&
    textMentionsExactPath(prompt, relativePath)
  ) {
    return { status: "authorized", reason: "user_named_file" };
  }
  const priorCommand = [...commands].reverse().find((command) =>
    command.category === "codestory_cli" &&
    command.exit_code === 0 &&
    String(command.status ?? "completed").toLowerCase() !== "failed" &&
    (command.completed_event_index ?? -1) < (read.event_index ?? -1) &&
    explicitEvidenceGapAuthorizesPath(command.aggregated_output, relativePath)
  );
  if (priorCommand) {
    return {
      status: "authorized",
      reason: "explicit_evidence_gap",
      evidence_command_id: priorCommand.id,
    };
  }
  const priorMcpEventIndex = events.findLastIndex((event, index) =>
    index < (read.event_index ?? -1) &&
    isSuccessfulCodeStoryMcpToolCallEvent(event) &&
    explicitEvidenceGapAuthorizesPath(
      canonicalMcpEvidenceResult(itemOf(event).result ?? event.result),
      relativePath,
    ),
  );
  if (priorMcpEventIndex >= 0) {
    return {
      status: "authorized",
      reason: "explicit_evidence_gap",
      evidence_event_index: priorMcpEventIndex,
    };
  }
  return { status: "unauthorized", reason: null };
}

function analyzeTranscript(events, projectRoot = null, context = {}) {
  const commands = extractCommandExecutions(events);
  const toolCategories = toolCallCategories(events);
  const codestoryMcpToolCalls = events.filter(isCodeStoryMcpToolCallStartEvent);
  const codestoryMcpCompletedCalls = events.filter(isSuccessfulCodeStoryMcpToolCallEvent);
  const codestoryMcpRuntimeIdentities = codestoryMcpCompletedCalls
    .map(codeStoryMcpRuntimeIdentity)
    .filter(Boolean);
  const commandCategories = {};
  const outputCharsByCategory = {};
  const directFileReads = [];

  for (const command of commands) {
    bumpCount(commandCategories, command.category);
    bumpCount(outputCharsByCategory, command.category, String(command.aggregated_output ?? "").length);
    const directReads = command.category === "direct_file_read"
      ? extractDirectFileReads(command.command)
      : [];
    for (const filePath of directReads) {
      directFileReads.push({
        path: filePath,
        command_id: command.id,
        category: command.category,
        event_index: command.started_event_index ?? command.completed_event_index,
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
      (command.codestory_operation === "packet" || isCodestoryPacketCommand(command.command)),
  );
  const codestoryIndexCommands = commands.filter(
    (command) => command.category === "codestory_cli" && isCodestoryIndexCommand(command.command),
  );
  const firstSuccessfulContextCommand = commands.find(isSuccessfulContextCommand);
  const sourceReads = directFileReads.filter((read) => read.source_like && read.repo_like);
  const authorizedSourceReads = sourceReads.map((read) => ({
    ...read,
    authorization: directSourceReadAuthorization(read, commands, events, projectRoot, context),
  }));
  const afterIndex = (first) =>
    first == null
      ? null
      : sourceReads.filter((read) => (read.event_index ?? -1) > (first.completed_event_index ?? first.started_event_index ?? -1)).length;

  return {
    interaction_turns: interactionTurnTelemetry(events),
    tool_categories: toolCategories,
    codestory_mcp_tool_calls_observed: codestoryMcpToolCalls.length,
    codestory_mcp_completed_calls_observed: codestoryMcpCompletedCalls.length,
    codestory_mcp_runtime_identities: codestoryMcpRuntimeIdentities,
    external_context_tool_calls: toolCategories.web_search ?? 0,
    command_categories: commandCategories,
    command_count: commands.length,
    command_patterns_duplicated: duplicateCounts(commands.map((command) => command.pattern)),
    output_chars_by_category: outputCharsByCategory,
    direct_file_reads_total: directFileReads.length,
    direct_source_reads_total: sourceReads.length,
    direct_source_reads: authorizedSourceReads,
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

function isCodeStoryMcpToolCallEvent(event) {
  const item = itemOf(event);
  return (
    String(item.type ?? "").toLowerCase() === "mcp_tool_call" &&
    String(item.server ?? event?.server ?? "").trim().toLowerCase() === "codestory"
  );
}

function isCodeStoryMcpToolCallStartEvent(event) {
  return isToolCallStartEvent(event) && isCodeStoryMcpToolCallEvent(event);
}

function isSuccessfulCodeStoryMcpToolCallEvent(event) {
  if (!isCodeStoryMcpToolCallEvent(event)) {
    return false;
  }
  const eventType = String(event.type ?? event.event ?? "").toLowerCase();
  if (!(eventType === "item.completed" || eventType.endsWith(".completed"))) {
    return false;
  }
  const item = itemOf(event);
  const result = item.result ?? event.result ?? null;
  return (
    result != null &&
    item.error == null &&
    event.error == null &&
    String(item.status ?? "completed").toLowerCase() !== "failed" &&
    !(result && typeof result === "object" && result.isError === true)
  );
}

function codeStoryMcpRuntimeIdentity(event) {
  const item = itemOf(event);
  const runtime = findCodeStoryRuntimeIdentity(item.result ?? event.result ?? null);
  if (!runtime) {
    return null;
  }
  return {
    plugin_version: runtime.plugin_version ?? null,
    plugin_cli_version: runtime.plugin_cli_version ?? null,
    cli_version: runtime.cli_version ?? null,
    cli_sha256: runtime.cli_sha256 ?? null,
    cli_source: runtime.cli_source ?? null,
    pinned_pair_matches: runtime.pinned_pair_matches ?? null,
    known_override_skew_channel: runtime.known_override_skew_channel ?? null,
  };
}

function codeStoryBinaryIdentity(preludeCliSha256, analysis) {
  const preludeSha = String(preludeCliSha256 ?? "").trim().toLowerCase();
  const completedCalls = analysis?.codestory_mcp_completed_calls_observed ?? 0;
  const identities = analysis?.codestory_mcp_runtime_identities ?? [];
  const declaredMcpShas = identities
    .map((identity) => String(identity?.cli_sha256 ?? "").trim().toLowerCase())
    .filter(Boolean);
  const uniqueMcpShas = [...new Set(declaredMcpShas)].sort();
  let status;
  if (!SHA256_PATTERN.test(preludeSha)) {
    status = "prelude_sha_missing_or_invalid";
  } else if (completedCalls === 0) {
    status = "prelude_only";
  } else if (
    declaredMcpShas.length < completedCalls ||
    declaredMcpShas.some((sha) => !SHA256_PATTERN.test(sha))
  ) {
    status = "mcp_sha_missing_or_invalid";
  } else if (uniqueMcpShas.length === 1 && uniqueMcpShas[0] === preludeSha) {
    status = "exact_match";
  } else {
    status = "mismatch";
  }
  return {
    status,
    exact_match: status === "exact_match",
    prelude_cli_sha256: SHA256_PATTERN.test(preludeSha) ? preludeSha : null,
    completed_mcp_calls: completedCalls,
    mcp_identities_observed: identities.length,
    mcp_cli_sha256_values: uniqueMcpShas,
  };
}

function findCodeStoryRuntimeIdentity(value, depth = 0) {
  if (depth > 6 || value == null) {
    return null;
  }
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!(trimmed.startsWith("{") || trimmed.startsWith("["))) {
      return null;
    }
    try {
      return findCodeStoryRuntimeIdentity(JSON.parse(trimmed), depth + 1);
    } catch {
      return null;
    }
  }
  if (Array.isArray(value)) {
    for (const entry of value) {
      const identity = findCodeStoryRuntimeIdentity(entry, depth + 1);
      if (identity) {
        return identity;
      }
    }
    return null;
  }
  if (typeof value !== "object") {
    return null;
  }
  const direct = value?._meta?.codestory_publication?.contract_runtime;
  if (direct && typeof direct === "object") {
    return direct;
  }
  for (const key of ["structuredContent", "structured_content", "content", "data", "output", "text"]) {
    if (!(key in value)) {
      continue;
    }
    const identity = findCodeStoryRuntimeIdentity(value[key], depth + 1);
    if (identity) {
      return identity;
    }
  }
  return null;
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
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "(*$1).$2"));
    variants.add(normalized.replace(/([a-z_][a-z0-9_]*)\.([a-z_][a-z0-9_]*)/gi, "($1).$2"));
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

function scoreAnchorSet(anchors, haystack, opts = {}) {
  const expected = [...new Set((anchors ?? []).map(String).map((value) => value.trim()).filter(Boolean))];
  const found = [];
  const missed = [];
  for (const anchor of expected) {
    const matched = opts.affirmative === true
      ? affirmativeQualityClauses(haystack).some((clause) => anchorMatched(clause, anchor))
      : anchorMatched(haystack, anchor);
    if (matched) {
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
  "for",
  "from",
  "into",
  "is",
  "later",
  "or",
  "that",
  "the",
  "then",
  "this",
  "with",
]);

function claimTokens(value, { expandQualified = false } = {}) {
  const raw = String(value ?? "");
  const inputs = expandQualified
    ? [raw, raw.replace(/([a-z0-9])([A-Z])/g, "$1 $2")]
    : [raw];
  return inputs
    .flatMap((input) => normalizeSearchText(input).split(/[^a-z0-9_:.]+/))
    .map((token) => token.trim().replace(/^[.:]+|[.:]+$/g, ""))
    .flatMap((token) => expandQualified ? [token, ...token.split(/(?:::|[.#_])/g)] : [token])
    .filter((token) => token.length >= 3 && !CLAIM_STOPWORDS.has(token));
}

const POSITIVE_CLAIM_EQUIVALENCE_CLASSES = [
  new Set(["call", "delegate", "forward", "invoke"]),
  new Set(["choose", "determine", "dispatch", "route", "select"]),
  new Set(["admission", "admit", "check", "guard", "reject", "validate"]),
];

function positiveClaimTokenVariants(token) {
  const variants = new Set();
  const splitComponents = token.split(/(?:::|[.:#_])/g);
  const alternatives = token.includes(":") ? splitComponents.slice(-1) : splitComponents;
  const components = [token, ...alternatives]
    .filter((component, index) =>
      component.length >= (index === 0 ? 3 : 2) && !CLAIM_STOPWORDS.has(component)
    );
  for (const component of components) {
    variants.add(component);
    if (component.length >= 5 && component.endsWith("ies")) {
      variants.add(`${component.slice(0, -3)}y`);
    }
    if (component.length >= 5 && component.endsWith("ing")) {
      variants.add(component.slice(0, -3));
      variants.add(`${component.slice(0, -3)}e`);
    }
    if (component.length >= 4 && component.endsWith("ed")) {
      variants.add(component.slice(0, -2));
      variants.add(component.slice(0, -1));
    }
    if (component.length >= 4 && component.endsWith("es")) {
      variants.add(component.slice(0, -2));
      variants.add(component.slice(0, -1));
    }
    if (component.length >= 4 && component.endsWith("s")) {
      variants.add(component.slice(0, -1));
    }
    if (component.length >= 6 && component.endsWith("er")) {
      const roleBase = component.slice(0, -2);
      if (roleBase.length >= 4) variants.add(roleBase);
    }
  }
  for (const equivalents of POSITIVE_CLAIM_EQUIVALENCE_CLASSES) {
    if ([...variants].some((variant) => equivalents.has(variant))) {
      for (const equivalent of equivalents) variants.add(equivalent);
    }
  }
  return variants;
}

function positiveClaimTokenMatched(token, haystackTokens) {
  const expectedVariants = positiveClaimTokenVariants(token);
  for (const candidate of haystackTokens) {
    for (const expected of expectedVariants) {
      for (const observed of positiveClaimTokenVariants(candidate)) {
        if (expected === observed) {
          return true;
        }
      }
    }
  }
  return false;
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

function claimMatchedInSupportUnit(unit, claim) {
  if (anchorMatched(unit, claim)) {
    return true;
  }
  const expectedTokens = [...new Set(claimTokens(claim))];
  if (expectedTokens.length < 3) {
    return false;
  }
  const haystackTokens = new Set(claimTokens(unit, { expandQualified: true }));
  const matched = expectedTokens.filter((token) =>
    positiveClaimTokenMatched(token, haystackTokens),
  ).length;
  const ratio = matched / expectedTokens.length;
  // Positive quality claims are short semantic summaries, not exact-string fixtures. Three
  // independently matched content words plus 60% coverage accepts ordinary inflection and
  // paraphrase while still rejecting a sentence that merely repeats one symbol name. Forbidden
  // claims retain the stricter polarity-aware matcher below.
  return matched >= Math.min(3, expectedTokens.length) && ratio >= 0.6;
}

function claimMatched(haystack, claim, subjectAnchors = []) {
  const subject = claimSubjectAnchor(claim, subjectAnchors);
  return qualitySupportUnits(haystack).some((unit) => {
    const clauses = qualityClausesInUnit(unit);
    if (!subject) {
      const affirmative = clauses.filter(qualityClauseIsAffirmative);
      return affirmative.length > 0 && claimMatchedInSupportUnit(affirmative.join(" "), claim);
    }
    return clauses.some((clause, index) => {
      if (!qualityClauseIsAffirmative(clause) || !anchorMatched(clause, subject)) {
        return false;
      }
      const candidate = [];
      if (
        index > 0 &&
        qualityClauseIsAffirmative(clauses[index - 1]) &&
        claimTokens(clauses[index - 1]).length <= 4 &&
        !leadingClaimSubject(clauses[index - 1], subjectAnchors)
      ) {
        candidate.push(clauses[index - 1]);
      }
      candidate.push(clause);
      for (let next = index + 1; next < clauses.length; next += 1) {
        if (!qualityClauseIsAffirmative(clauses[next])) {
          break;
        }
        const nextSubject = leadingClaimSubject(clauses[next], subjectAnchors);
        if (nextSubject && nextSubject !== subject) break;
        candidate.push(clauses[next]);
      }
      return claimMatchedInSupportUnit(candidate.join(" "), claim);
    });
  });
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

const NON_AFFIRMATIVE_QUALITY_TEXT =
  /\b(?:unknown|unavailable|unproven|unsupported|missing|gaps?|not_established|evidence_missing|unresolved|(?:no|without)\s+evidence|(?:lacks?|lacked|lacking)\s+(?:evidence|support)|(?:does|do|did|can|could)\s+not\b|cannot\b|doesn't\b|don't\b|didn't\b|can't\b|couldn't\b|never\b|fails? to (?:establish|show|support|prove|verify|demonstrate))\b/i;

function qualitySupportUnits(haystack) {
  const units = [];
  let current = [];
  let currentListIndent = null;
  const listParents = [];
  let inCodeFence = false;
  let inGapSection = false;
  const flush = () => {
    const unit = current.join(" ").trim();
    if (unit) units.push(unit);
    current = [];
    const listIndent = currentListIndent;
    currentListIndent = null;
    return unit ? { unit, listIndent } : null;
  };
  for (const rawLine of String(haystack ?? "").replace(/\r\n/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (/^```/.test(line)) {
      flush();
      inCodeFence = !inCodeFence;
      continue;
    }
    if (inCodeFence) continue;

    const heading = line.match(/^#{1,6}\s+(.+)$/);
    if (heading) {
      flush();
      listParents.length = 0;
      inGapSection = /\b(?:gaps?|unknown|unavailable|limitations?)\b/i.test(heading[1]);
      continue;
    }
    if (/^(?:material\s+)?gaps?\s*:/i.test(line)) {
      flush();
      listParents.length = 0;
      inGapSection = true;
      continue;
    }
    if (!line) {
      flush();
      listParents.length = 0;
      continue;
    }
    if (inGapSection) continue;
    if (/^(?:supporting|repository evidence) command\s*:/i.test(line)) {
      flush();
      continue;
    }

    const listItem = line.match(/^(?:[-*+]\s+|\d+[.)]\s+)(.*)$/);
    if (listItem) {
      const indent = rawLine.match(/^\s*/u)?.[0].replace(/\t/gu, "  ").length ?? 0;
      const previous = flush();
      while (listParents.at(-1)?.listIndent >= indent) listParents.pop();
      if (
        previous?.listIndent != null &&
        previous.listIndent < indent &&
        previous.unit.endsWith(":")
      ) {
        listParents.push(previous);
      }
      const parent = listParents.at(-1);
      current.push(
        parent && parent.listIndent < indent
          ? `${parent.unit.replace(/:\s*$/u, "")} ${listItem[1]}`
          : listItem[1],
      );
      currentListIndent = indent;
    } else {
      current.push(line);
    }
  }
  flush();
  return units;
}

function qualityClausesInUnit(unit) {
  return String(unit ?? "")
    .split(/(?:[.!?](?:["'`)]*)\s+|;\s*|\s+[—–]\s+|\s+(?:but|however|although|though|yet)\b[,:]?\s*)/iu)
    .map((clause) => clause.trim())
    .filter(Boolean);
}

function qualityClauseIsAffirmative(clause) {
  return !NON_AFFIRMATIVE_QUALITY_TEXT.test(clause);
}

function affirmativeQualityClausesInUnit(unit) {
  return qualityClausesInUnit(unit).filter(qualityClauseIsAffirmative);
}

function affirmativeQualityClauses(haystack) {
  return qualitySupportUnits(haystack).flatMap(affirmativeQualityClausesInUnit);
}

function claimSubjectAnchor(claim, subjectAnchors = []) {
  const leadingKnown = leadingClaimSubject(claim, subjectAnchors);
  if (leadingKnown) return leadingKnown;

  const normalized = String(claim ?? "")
    .trim()
    .replace(/^[`*_]+|[`*_]+$/g, "");
  const subject = normalized.match(
    /^([A-Za-z_$][A-Za-z0-9_$]*(?:(?:::|[.#])[A-Za-z_$][A-Za-z0-9_$]*)*)\b/,
  )?.[1] ?? null;
  if (!subject || /^(?:a|an|the|this|these|those)$/i.test(subject)) return null;
  return /(?:::|[.#])/.test(subject) || /[A-Z_]/.test(subject.slice(1)) ? subject : null;
}

function leadingClaimSubject(clause, subjectAnchors) {
  const normalized = normalizeSearchText(clause)
    .replace(/^[^a-z0-9_$]+/i, "")
    .replace(/^(?:a|an|the)\s+/i, "");
  return [...new Set(subjectAnchors.map(String).filter(Boolean))]
    .sort((left, right) => right.length - left.length || left.localeCompare(right))
    .find((anchor) => anchorSearchVariants(anchor).some((variant) => {
      if (!normalized.startsWith(variant)) return false;
      const next = normalized[variant.length];
      return next == null || !/[a-z0-9_$]/i.test(next);
    })) ?? null;
}

function hasContradictingNegation(sentence) {
  const tokens = normalizeSearchText(sentence)
    .split(/[^a-z0-9_:.]+/)
    .map((token) => token.trim())
    .filter(Boolean);
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
    // Polarity-bearing forbidden claims need near-complete semantic coverage. A lower ratio
    // confuses an explicit evidence-gap sentence with the opposite claim when both name the same
    // subsystem, binary route, and dispatch vocabulary.
    if (matched < Math.min(4, expectedTokens.length) || ratio < 0.8) {
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
    const matched = opts.forbidden
      ? forbiddenClaimMatched(haystack, claim)
      : claimMatched(haystack, claim, opts.subjectAnchors ?? []);
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
  const quality = scoreQualityFromText(finalAnswer, transcript, task);
  const proofClaims = forbiddenCandidateSentences(finalAnswer).filter((sentence) =>
    /\b(?:contractproven|contractrefuted)\b/i.test(sentence) ||
    /\bcodestory\s+(?:proved|proves|refuted|refutes|verified)\b/i.test(sentence)
  );
  const proofEvidence = [
    ...commands
      .filter((command) => command.category === "codestory_cli" && command.exit_code === 0)
      .map((command) => command.aggregated_output ?? ""),
    ...events.filter(isSuccessfulCodeStoryMcpToolCallEvent).map((event) => JSON.stringify(event)),
  ].join("\n");
  const unsupportedProofClaims = proofClaims.filter((claim) => {
    const refutationClaim = /\b(?:contractrefuted|refuted|refutes)\b/i.test(claim);
    return refutationClaim
      ? !/\bContractRefuted\b/.test(proofEvidence)
      : !/\bContractProven\b/.test(proofEvidence);
  });
  return {
    ...quality,
    material_factual_errors: {
      found: quality.forbidden_claims.found,
      found_anchors: quality.forbidden_claims.found_anchors,
    },
    unsupported_proof_claims: {
      found: unsupportedProofClaims.length,
      found_claims: unsupportedProofClaims,
    },
  };
}

function scoreQualityFromText(finalAnswer, transcript, task) {
  if (!task) {
    return null;
  }
  const finalAndTranscript = `${finalAnswer}\n${transcript}`;

  const observedFiles = scoreAnchorSet(task.expected_files, finalAndTranscript);
  const observedSymbols = scoreAnchorSet(task.expected_symbols, finalAndTranscript, {
    affirmative: true,
  });
  const files = scoreAnchorSet(task.expected_files, finalAnswer);
  const symbols = scoreAnchorSet(task.expected_symbols, finalAnswer, { affirmative: true });
  const claims = scoreClaimSet(task.expected_claims, finalAnswer, {
    subjectAnchors: task.expected_symbols ?? [],
  });
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

function estimateCost(usage, rates = null) {
  const inputCost = Number.parseFloat(
    rates?.input_per_mtok ?? process.env.CODESTORY_BENCH_INPUT_COST_PER_MTOK ?? "",
  );
  const outputCost = Number.parseFloat(
    rates?.output_per_mtok ?? process.env.CODESTORY_BENCH_OUTPUT_COST_PER_MTOK ?? "",
  );
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

function exactCandidateCostRates(env = process.env) {
  const inputPerMtok = Number.parseFloat(env.CODESTORY_BENCH_INPUT_COST_PER_MTOK ?? "");
  const outputPerMtok = Number.parseFloat(env.CODESTORY_BENCH_OUTPUT_COST_PER_MTOK ?? "");
  if (
    !Number.isFinite(inputPerMtok) ||
    inputPerMtok <= 0 ||
    !Number.isFinite(outputPerMtok) ||
    outputPerMtok <= 0
  ) {
    throw new Error(
      "exact-candidate mode requires positive CODESTORY_BENCH_INPUT_COST_PER_MTOK and CODESTORY_BENCH_OUTPUT_COST_PER_MTOK before package authentication or repository materialization",
    );
  }
  return {
    currency: "USD",
    model: DEFAULT_BENCHMARK_MODEL,
    input_per_mtok: inputPerMtok,
    output_per_mtok: outputPerMtok,
    source: "configured_environment",
  };
}

const BENCHMARK_AGENT_RUN_ID = "shared-agent";

function benchmarkAgentScopeArgs() {
  return ["--profile", "agent", "--run-id", BENCHMARK_AGENT_RUN_ID];
}

function retrievalStatusCommandArgs(project) {
  return [
    "retrieval",
    "status",
    "--project",
    project,
    ...benchmarkAgentScopeArgs(),
    "--format",
    "json",
  ];
}

function retrievalIndexCommandArgs(project) {
  return [
    "retrieval",
    "index",
    "--project",
    project,
    ...benchmarkAgentScopeArgs(),
    "--refresh",
    "auto",
    "--format",
    "json",
  ];
}

function packetCommandArgs(repoConfig, task, opts = {}) {
  const args = [
    "packet",
    "--project",
    repoConfig.path,
    ...benchmarkAgentScopeArgs(),
    "--question",
    task?.prompt ?? repoConfig.prompt,
    "--budget",
    "standard",
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

function drillPacketCommandArgs(repoConfig, task, opts, packet) {
  const drill = packet?.disposition?.drill;
  const continuation = isPacketProjectionV3(packet) ? packet.continuation : null;
  const parentPacketId = String(
    continuation?.continuation_id ?? drill?.parent_packet_id ?? "",
  ).trim();
  const options = continuation
    ? (Array.isArray(continuation.gap_ids) ? continuation.gap_ids : [])
    : (Array.isArray(drill?.options) ? drill.options : []);
  const optionIds = options
    .map((option) => String(option?.gap_id ?? option?.id ?? "").trim())
    .filter(Boolean);
  if (!parentPacketId || optionIds.length === 0) {
    return null;
  }
  const args = packetCommandArgs(repoConfig, task, opts);
  args.push("--parent-packet-id", parentPacketId);
  for (const optionId of optionIds) {
    args.push("--option-id", optionId);
  }
  const coreGenerationId = String(
    packet?.publication?.core?.generation_id ?? drill?.core_generation_id ?? "",
  ).trim();
  if (coreGenerationId) {
    args.push("--core-generation-id", coreGenerationId);
  }
  const retrievalGeneration = String(
    packet?.publication?.retrieval?.retrieval_generation ??
      drill?.retrieval_generation ??
      "",
  ).trim();
  if (retrievalGeneration) {
    args.push("--retrieval-generation", retrievalGeneration);
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
    packet_schema_version: prelude.packet_schema_version ?? null,
    packet_projection_kind: prelude.packet_projection_kind ?? null,
    packet_evidence_availability: prelude.packet_evidence_availability ?? null,
    packet_evidence_gap_accounting: prelude.packet_evidence_gap_accounting ?? null,
    packet_evidence_count: prelude.packet_evidence_count ?? null,
    packet_gap_count: prelude.packet_gap_count ?? null,
    packet_disposition_kind: prelude.packet_disposition_kind ?? null,
    packet_disposition: prelude.packet_disposition ?? null,
    packet_sufficiency_status: prelude.packet_sufficiency_status ?? null,
    packet_sufficiency: prelude.packet_sufficiency ?? null,
    packet_support_count: prelude.packet_support_count ?? null,
    packet_support_kind_counts: prelude.packet_support_kind_counts ?? null,
    packet_citation_count: prelude.packet_citation_count,
    packet_avoid_opening_count: prelude.packet_avoid_opening_count,
    packet_latency: prelude.packet_latency,
    packet_composition: prelude.packet_composition,
    packet_manifest_quality: prelude.packet_manifest_quality,
    packet_contract_runtime: prelude.packet_contract_runtime ?? null,
    packet_extra_probe_count: prelude.packet_extra_probe_count ?? null,
    packet_extra_probe_strategy: prelude.packet_extra_probe_strategy ?? null,
    packet_drill_continuation: prelude.packet_drill_continuation === true,
    packet_contract_blockers: prelude.packet_contract_blockers ?? [],
    ...(prelude.packet_command_failure == null
      ? {}
      : { packet_command_failure: prelude.packet_command_failure }),
  };
}

function harnessPacketPreludeEvents(prelude, stdout = "") {
  if (!prelude) {
    return [];
  }
  const command = prelude.command ?? "";
  const id = "harness_codestory_packet";
  const harnessSemantics = {
    source: "codestory_packet_prelude_v1",
    category: "codestory_cli",
    operation: "packet",
  };
  return [
    {
      type: "harness.command.started",
      item: {
        id,
        type: "command_execution",
        command,
        harness_semantics: harnessSemantics,
      },
    },
    {
      type: "harness.command.completed",
      item: {
        id,
        type: "command_execution",
        command,
        harness_semantics: harnessSemantics,
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
  const env = opts.exactCandidate
    ? selectedBenchmarkChildEnv(opts, "without_codestory")
    : { ...process.env };
  delete env.CODESTORY_CLI;
  const result = await runProcess("rg", args, {
    cwd: repoConfig.path,
    env,
    signal: opts.signal,
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

function packetGraphEdgeOccurrences(packet) {
  return (packet?.answer?.graphs ?? []).reduce(
    (count, artifact) => count + (Array.isArray(artifact?.graph?.edges) ? artifact.graph.edges.length : 0),
    0,
  );
}

function packetUniqueCitationFileCount(packet) {
  return new Set(
    (packet?.answer?.citations ?? [])
      .filter((citation) => typeof citation?.file_path === "string")
      .map((citation) => citation.file_path),
  ).size;
}

function packetSourceReadSnippetCount(packet) {
  return (packet?.answer?.retrieval_trace?.steps ?? []).filter(
    (step) => step?.kind === "source_read" && step?.status === "ok",
  ).length;
}

function packetFollowUpCount(packet) {
  const invocations = packet?.sufficiency?.follow_up_invocations;
  const commands = packet?.sufficiency?.follow_up_commands;
  return Math.max(
    Array.isArray(invocations) ? invocations.length : 0,
    Array.isArray(commands) ? commands.length : 0,
  );
}

function managedRuntimeIdentityBlockers(identity, label, expectedVersion = null) {
  const versions = [
    identity?.plugin_version,
    identity?.plugin_cli_version,
    identity?.cli_version,
  ];
  const selectedVersion = expectedVersion ?? versions[0];
  const versionMatches = Boolean(selectedVersion) && versions.every((version) => version === selectedVersion);
  return versionMatches &&
    identity?.pinned_pair_matches === true &&
    identity?.cli_source === "managed" &&
    identity?.known_override_skew_channel === false
    ? []
    : [`${label} runtime identity is not managed ${selectedVersion ?? "a bound version"}: ${JSON.stringify(identity ?? null)}`];
}

function packetMaterialObligationBucket(obligation) {
  if (String(obligation?.proof_status ?? "").trim().toLowerCase() === "proven") {
    return "proven";
  }
  const reason = String(obligation?.reason ?? "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  if (!reason) {
    return "missing_reason";
  }
  if (reason.startsWith("requested_claim_binding_limit_exceeded")) {
    return "requested_claim_binding_limit_exceeded";
  }
  if ([
    "packet_budget_truncated",
    "carrier_not_sufficiency_eligible",
    "carrier_does_not_satisfy_role_contract",
    "required_evidence_edge_missing",
  ].includes(reason)) {
    return reason;
  }
  return `unclassified_reason:${reason}`;
}

function packetObligationAccounting(packet) {
  const obligations = packet?.plan?.obligations?.claim_obligations;
  if (!Array.isArray(obligations)) {
    return null;
  }
  let material = 0;
  let nonmaterial = 0;
  const materialStatusBuckets = new Map();
  for (const obligation of obligations) {
    if (obligation?.material === true) {
      material += 1;
      const bucket = packetMaterialObligationBucket(obligation);
      materialStatusBuckets.set(bucket, (materialStatusBuckets.get(bucket) ?? 0) + 1);
    } else if (obligation?.material === false) {
      nonmaterial += 1;
    }
  }
  return {
    total: obligations.length,
    material,
    nonmaterial,
    material_status_buckets: Object.fromEntries(
      [...materialStatusBuckets.entries()].sort(([left], [right]) =>
        left < right ? -1 : left > right ? 1 : 0
      ),
    ),
  };
}

function packetObligationAccountingError(accounting, label = "packet obligations") {
  if (!accounting || typeof accounting !== "object" || Array.isArray(accounting)) {
    return `${label} accounting is missing`;
  }
  if (["total", "material", "nonmaterial"].some(
    (field) => !Number.isInteger(accounting[field]) || accounting[field] < 0
  )) {
    return `${label} counts are missing or invalid`;
  }
  const buckets = accounting.material_status_buckets;
  if (!buckets || typeof buckets !== "object" || Array.isArray(buckets)) {
    return `${label} material status buckets are missing or invalid`;
  }
  const bucketEntries = Object.entries(buckets);
  if (bucketEntries.some(([status, count]) =>
    !status || !Number.isInteger(count) || count < 0
  )) {
    return `${label} material status buckets are invalid`;
  }
  if (accounting.total !== accounting.material + accounting.nonmaterial) {
    return `${label} total=${accounting.total} does not reconcile with material=${accounting.material} + nonmaterial=${accounting.nonmaterial}`;
  }
  const materialStatusTotal = bucketEntries.reduce((sum, [, count]) => sum + count, 0);
  return accounting.material === materialStatusTotal
    ? null
    : `${label} material=${accounting.material} does not reconcile with material status buckets=${materialStatusTotal}`;
}

function resultPacketObligationAccounting(result) {
  return result?.sufficiency?.obligation_accounting ??
    result?.codestory_harness_prelude?.packet_sufficiency?.obligation_accounting ??
    null;
}

function resultRequiresPacketObligationAccounting(result) {
  if (!isCodeStoryArm(result?.arm) && result?.mode == null) {
    return false;
  }
  const prelude = result?.codestory_harness_prelude;
  if (prelude?.packet_schema_version === 3) {
    return false;
  }
  if (
    result?.disposition != null ||
    prelude?.packet_disposition != null ||
    prelude?.packet_disposition_kind != null
  ) {
    return false;
  }
  const packetEvidencePresent =
    result?.sufficiency != null ||
    result?.packet_shape != null ||
    result?.packet_latency != null ||
    result?.packet_composition != null ||
    prelude?.packet_sufficiency != null;
  if (packetEvidencePresent) {
    return true;
  }
  // Missing legacy status stays fail-closed because it cannot prove that no packet was emitted.
  // Only an explicitly failed/cancelled row with no bounded packet evidence is exempt: fail-fast
  // siblings can be stopped before their packet process begins and therefore own no accounting.
  return result?.status == null || result.status === "pass";
}

function summarizePacketObligationAccounting(results, label) {
  const aggregate = {
    packets: 0,
    total: 0,
    material: 0,
    nonmaterial: 0,
    material_status_buckets: {},
  };
  for (const result of results) {
    const accounting = resultPacketObligationAccounting(result);
    if (!accounting) {
      if (resultRequiresPacketObligationAccounting(result)) {
        throw new Error(
          `${label} ${result.repo ?? "unknown"}/${result.task_id ?? "unknown"}/${result.arm ?? result.mode ?? "unknown"}/repeat-${result.repeat ?? "unknown"} packet obligation accounting is missing`,
        );
      }
      continue;
    }
    const rowLabel = `${label} ${result.repo ?? "unknown"}/${result.task_id ?? "unknown"}/${result.arm ?? result.mode ?? "unknown"}/repeat-${result.repeat ?? "unknown"}`;
    const error = packetObligationAccountingError(accounting, rowLabel);
    if (error) {
      throw new Error(error);
    }
    aggregate.packets += 1;
    aggregate.total += accounting.total;
    aggregate.material += accounting.material;
    aggregate.nonmaterial += accounting.nonmaterial;
    for (const [status, count] of Object.entries(accounting.material_status_buckets)) {
      aggregate.material_status_buckets[status] =
        (aggregate.material_status_buckets[status] ?? 0) + count;
    }
  }
  if (aggregate.packets === 0) {
    return null;
  }
  aggregate.material_status_buckets = Object.fromEntries(
    Object.entries(aggregate.material_status_buckets).sort(([left], [right]) =>
      left < right ? -1 : left > right ? 1 : 0
    ),
  );
  const error = packetObligationAccountingError(aggregate, label);
  if (error) {
    throw new Error(error);
  }
  return aggregate;
}

function publicPacketPreludeContractPasses(packet, stdout) {
  return packetPreludeContractBlockers(packet, stdout, {
    requireSupported: false,
    requireManagedRuntime: false,
  }).length === 0;
}

function nonemptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function packetV3EvidenceGapAccounting(packet) {
  if (!isPacketProjectionV3(packet)) {
    return null;
  }
  const evidence = Array.isArray(packet.evidence) ? packet.evidence : [];
  const gaps = Array.isArray(packet.gaps) ? packet.gaps : [];
  const evidenceIds = evidence
    .map((row) => row?.identity?.evidence_id)
    .filter(nonemptyString);
  const gapIds = gaps
    .map((row) => row?.identity?.gap_id)
    .filter(nonemptyString);
  const continuationGapIds = Array.isArray(packet.continuation?.gap_ids)
    ? packet.continuation.gap_ids
      .map((identity) => identity?.gap_id)
      .filter(nonemptyString)
    : [];
  const kindCounts = (rows) => Object.fromEntries(
    [...rows.reduce((counts, row) => {
      const kind = String(row?.kind ?? "unknown");
      counts.set(kind, (counts.get(kind) ?? 0) + 1);
      return counts;
    }, new Map()).entries()].sort(([left], [right]) => left.localeCompare(right)),
  );
  return {
    contract: "codestory.packet-v3-evidence-gap-accounting/v1",
    kind: packet.kind ?? null,
    status: packet.status ?? null,
    evidence_count: evidence.length,
    unique_evidence_id_count: new Set(evidenceIds).size,
    evidence_kind_counts: kindCounts(evidence),
    gap_count: gaps.length,
    unique_gap_id_count: new Set(gapIds).size,
    gap_kind_counts: kindCounts(gaps),
    continuation_gap_count: continuationGapIds.length,
    unique_continuation_gap_id_count: new Set(continuationGapIds).size,
    continuation_gap_ids_bound:
      continuationGapIds.every((gapId) => new Set(gapIds).has(gapId)),
  };
}

function packetV3EvidenceAvailabilityTelemetry(packet, quality) {
  if (!isPacketProjectionV3(packet)) {
    return null;
  }
  const accounting = packetV3EvidenceGapAccounting(packet);
  return {
    kind: packet.kind ?? null,
    status: packet.status ?? null,
    terminal: packet.status !== "continuation_available",
    retrieval_state: packet.retrieval?.state ?? null,
    evidence_count: accounting.evidence_count,
    evidence_kind_counts: accounting.evidence_kind_counts,
    gap_count: accounting.gap_count,
    gap_kind_counts: accounting.gap_kind_counts,
    continuation_id: packet.continuation?.continuation_id ?? null,
    continuation_gap_count: accounting.continuation_gap_count,
    remaining_rounds: presentFiniteNumber(packet.continuation?.remaining_rounds),
    available_quality_mismatch:
      packet.status === "available" && quality?.pass === false,
  };
}

function packetV3ContractBlockers(packet, options = {}) {
  const blockers = [];
  const isSha256 = (value) =>
    SHA256_PATTERN.test(String(value ?? "")) && !/^0{64}$/.test(String(value));
  const identity = packet.identity;
  const publication = packet.publication;
  const core = publication?.core;
  const retrievalPublication = publication?.retrieval ?? null;
  const retrieval = packet.retrieval;
  const evidence = packet.evidence;
  const gaps = packet.gaps;
  const allowedStatuses = new Set([
    "available",
    "continuation_available",
    "no_useful_evidence",
    "unavailable",
  ]);
  const allowedRetrievalStates = new Set(["full", "degraded", "unavailable"]);
  const allowedEvidenceKinds = new Set([
    "exact_source",
    "structural_source",
    "graph_relation",
    "retrieval_excerpt",
  ]);
  const allowedGapKinds = new Set([
    "evidence_missing",
    "retrieval_unavailable",
    "source_unavailable",
    "continuation_required",
    "output_budget_exceeded",
  ]);

  if (!identity || typeof identity !== "object" || Array.isArray(identity)) {
    blockers.push("packet v3 identity is missing or invalid");
  } else {
    for (const field of ["packet_id", "request_id"]) {
      if (!nonemptyString(identity[field])) {
        blockers.push(`packet v3 identity.${field} is missing`);
      }
    }
    if (!isSha256(identity.question_sha256)) {
      blockers.push("packet v3 identity.question_sha256 is invalid");
    } else if (
      options.expectedQuestion != null &&
      identity.question_sha256 !== sha256Bytes(String(options.expectedQuestion))
    ) {
      blockers.push("packet v3 question digest does not match the benchmark task");
    }
  }
  if (!core || typeof core !== "object" || Array.isArray(core)) {
    blockers.push("packet v3 core publication is missing or invalid");
  } else {
    for (const field of ["project_id", "generation_id", "run_id"]) {
      if (!nonemptyString(core[field])) {
        blockers.push(`packet v3 publication.core.${field} is missing`);
      }
    }
  }
  if (!retrieval || typeof retrieval !== "object" || Array.isArray(retrieval)) {
    blockers.push("packet v3 retrieval state is missing or invalid");
  } else if (!allowedRetrievalStates.has(retrieval.state)) {
    blockers.push(`packet v3 retrieval state=${retrieval.state ?? "missing"} is invalid`);
  }
  if (retrievalPublication != null) {
    for (const field of [
      "core_generation_id",
      "core_run_id",
      "retrieval_generation",
      "semantic_generation",
    ]) {
      if (!nonemptyString(retrievalPublication[field])) {
        blockers.push(`packet v3 publication.retrieval.${field} is missing`);
      }
    }
    if (!isSha256(retrievalPublication.retrieval_input_sha256)) {
      blockers.push("packet v3 publication.retrieval.retrieval_input_sha256 is invalid");
    }
    if (
      core &&
      (
        retrievalPublication.core_generation_id !== core.generation_id ||
        retrievalPublication.core_run_id !== core.run_id
      )
    ) {
      blockers.push("packet v3 retrieval publication is not bound to the core publication");
    }
    if (retrieval?.generation_id !== retrievalPublication.retrieval_generation) {
      blockers.push("packet v3 retrieval state generation does not match its publication");
    }
  }
  if (retrieval?.state === "full" && !retrievalPublication) {
    blockers.push("packet v3 full retrieval has no retrieval publication");
  }
  if (retrieval?.state === "unavailable" && retrievalPublication != null) {
    blockers.push("packet v3 unavailable retrieval unexpectedly has a publication");
  }
  if (
    ["degraded", "unavailable"].includes(retrieval?.state) &&
    retrievalPublication == null &&
    retrieval?.generation_id != null
  ) {
    blockers.push("packet v3 retrieval state has a generation without a publication");
  }

  if (!Array.isArray(gaps) || gaps.length > 256) {
    blockers.push("packet v3 gaps are missing, invalid, or over the public row cap");
  }
  const gapIds = new Set();
  for (const gap of Array.isArray(gaps) ? gaps : []) {
    const gapId = gap?.identity?.gap_id;
    if (!nonemptyString(gapId)) {
      blockers.push("packet v3 gap identity is missing");
    } else if (gapIds.has(gapId)) {
      blockers.push(`packet v3 gap identity=${gapId} is duplicated`);
    } else {
      gapIds.add(gapId);
    }
    if (!allowedGapKinds.has(gap?.kind)) {
      blockers.push(`packet v3 gap kind=${gap?.kind ?? "missing"} is invalid`);
    }
  }

  const diagnostics = packet.diagnostics;
  if (!diagnostics || !["available", "unavailable"].includes(diagnostics.availability)) {
    blockers.push("packet v3 diagnostics capability is missing or invalid");
  } else if (diagnostics.availability === "available") {
    const reference = diagnostics.reference;
    if (
      !nonemptyString(reference?.artifact_id) ||
      !isSha256(reference?.sha256) ||
      !Number.isInteger(reference?.byte_length) ||
      reference.byte_length < 0
    ) {
      blockers.push("packet v3 diagnostics reference is invalid");
    }
  } else if (diagnostics.reference != null) {
    blockers.push("packet v3 unavailable diagnostics unexpectedly include a reference");
  }

  const compactBytes = Buffer.byteLength(JSON.stringify(packet), "utf8");
  if (compactBytes > PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES) {
    blockers.push(
      `packet v3 compact bytes=${compactBytes} exceeds public cap=${PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES}`,
    );
  }

  if (packet.kind === "complete") {
    if (!Array.isArray(evidence) || evidence.length > 256) {
      blockers.push("packet v3 evidence is missing, invalid, or over the public row cap");
    }
    const evidenceIds = new Set();
    for (const row of Array.isArray(evidence) ? evidence : []) {
      const evidenceId = row?.identity?.evidence_id;
      if (!nonemptyString(evidenceId)) {
        blockers.push("packet v3 evidence identity is missing");
      } else if (evidenceIds.has(evidenceId)) {
        blockers.push(`packet v3 evidence identity=${evidenceId} is duplicated`);
      } else {
        evidenceIds.add(evidenceId);
      }
      if (!allowedEvidenceKinds.has(row?.kind)) {
        blockers.push(`packet v3 evidence kind=${row?.kind ?? "missing"} is invalid`);
      }
      if (
        row?.start_line != null &&
        (!Number.isInteger(row.start_line) || row.start_line < 1)
      ) {
        blockers.push(`packet v3 evidence start_line=${row.start_line} is invalid`);
      }
      if (
        row?.end_line != null &&
        (!Number.isInteger(row.end_line) || row.end_line < 1)
      ) {
        blockers.push(`packet v3 evidence end_line=${row.end_line} is invalid`);
      }
      if (
        Number.isInteger(row?.start_line) &&
        Number.isInteger(row?.end_line) &&
        row.end_line < row.start_line
      ) {
        blockers.push("packet v3 evidence line range is reversed");
      }
    }
    if (!allowedStatuses.has(packet.status)) {
      blockers.push(`packet v3 status=${packet.status ?? "missing"} is invalid`);
    }
    const expectedStatus = packet.continuation != null
      ? "continuation_available"
      : Array.isArray(evidence) && evidence.length > 0
        ? "available"
        : retrieval?.state === "unavailable" ||
            (Array.isArray(gaps) && gaps.some((gap) =>
              ["retrieval_unavailable", "source_unavailable"].includes(gap?.kind)))
          ? "unavailable"
          : "no_useful_evidence";
    if (allowedStatuses.has(packet.status) && packet.status !== expectedStatus) {
      blockers.push(
        `packet v3 status=${packet.status} does not match evidence availability=${expectedStatus}`,
      );
    }
    const continuation = packet.continuation;
    if (packet.status === "continuation_available") {
      const references = continuation?.gap_ids;
      if (
        !nonemptyString(continuation?.continuation_id) ||
        continuation?.remaining_rounds !== 1 ||
        !Array.isArray(references) ||
        references.length === 0
      ) {
        blockers.push("packet v3 continuation_available state has an invalid continuation");
      } else {
        const referenceIds = references.map((entry) => entry?.gap_id);
        if (
          referenceIds.some((gapId) => !nonemptyString(gapId) || !gapIds.has(gapId)) ||
          new Set(referenceIds).size !== referenceIds.length
        ) {
          blockers.push("packet v3 continuation gap identities are missing, duplicated, or unbound");
        }
      }
    } else if (continuation != null) {
      blockers.push(`packet v3 status=${packet.status ?? "missing"} unexpectedly includes continuation`);
    }
    if (packet.status === "available" && (!Array.isArray(evidence) || evidence.length === 0)) {
      blockers.push("packet v3 available status has no evidence");
    }
    if (
      ["no_useful_evidence", "unavailable"].includes(packet.status) &&
      Array.isArray(evidence) &&
      evidence.length > 0
    ) {
      blockers.push(`packet v3 status=${packet.status} unexpectedly includes evidence`);
    }
    if (packet.maximum_bytes != null || packet.required_complete_bytes != null) {
      blockers.push("packet v3 complete result unexpectedly includes budget fallback fields");
    }
  } else if (packet.kind === "budget_exceeded") {
    if (packet.status !== "unavailable") {
      blockers.push(`packet v3 budget fallback status=${packet.status ?? "missing"}; expected unavailable`);
    }
    if (packet.evidence != null || packet.continuation != null) {
      blockers.push("packet v3 budget fallback contains partial evidence or continuation");
    }
    if (packet.maximum_bytes !== PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES) {
      blockers.push(
        `packet v3 budget fallback maximum_bytes=${packet.maximum_bytes ?? "missing"}; expected ${PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES}`,
      );
    }
    if (
      !Number.isInteger(packet.required_complete_bytes) ||
      packet.required_complete_bytes <= PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES
    ) {
      blockers.push("packet v3 budget fallback required_complete_bytes is invalid");
    }
    if (
      !Array.isArray(gaps) ||
      gaps.length === 0 ||
      gaps.some((gap) => gap?.kind !== "output_budget_exceeded")
    ) {
      blockers.push("packet v3 budget fallback lacks its output_budget_exceeded gap");
    }
  } else {
    blockers.push(`packet v3 kind=${packet.kind ?? "missing"} is invalid`);
  }

  if (options.requireSupported) {
    if (
      packet.kind !== "complete" ||
      packet.status !== "available" ||
      packet.retrieval?.state !== "full" ||
      !Array.isArray(packet.evidence) ||
      packet.evidence.length === 0
    ) {
      blockers.push("packet v3 does not contain available full-retrieval evidence");
    }
  }
  if (options.requireManagedRuntime) {
    blockers.push(
      ...managedRuntimeIdentityBlockers(
        packet?._meta?.codestory_publication?.contract_runtime,
        "packet",
      ),
    );
  }
  return blockers;
}

function packetPreludeContractBlockers(packet, stdout, options = {}) {
  if (!packet || typeof packet !== "object") {
    return ["packet JSON is missing"];
  }
  if (isPacketProjectionV3(packet)) {
    return packetV3ContractBlockers(packet, options);
  }
  const blockers = [];
  const limits = packet.budget?.limits ?? {};
  const used = packet.budget?.used ?? {};
  const actualStdoutBytes = Buffer.byteLength(String(stdout ?? ""), "utf8");
  const citationCount = Array.isArray(packet.answer?.citations)
    ? packet.answer.citations.length
    : 0;
  const fileCount = packetUniqueCitationFileCount(packet);
  const snippetCount = packetSourceReadSnippetCount(packet);
  const graphEdgeOccurrences = packetGraphEdgeOccurrences(packet);
  const support = packet.support;
  const disposition = packet.disposition;
  const dispositionKind = disposition?.kind;
  if (!Array.isArray(support)) {
    blockers.push("packet support is missing or invalid");
  }
  if (!PACKET_DISPOSITIONS.has(dispositionKind)) {
    blockers.push(`packet disposition=${dispositionKind ?? "missing"} is invalid`);
  }
  if (dispositionKind === "supported" && Array.isArray(support) && support.length === 0) {
    blockers.push("supported packet has no support units");
  }
  if (dispositionKind === "drill_once") {
    const drill = disposition?.drill;
    if (!drill || typeof drill !== "object") {
      blockers.push("drill_once packet has no drill plan");
    } else {
      if (drill.parent_packet_id !== packet.packet_id) {
        blockers.push("drill_once parent_packet_id does not match packet_id");
      }
      if (!Array.isArray(drill.options) || drill.options.length === 0 || drill.options.length > 8) {
        blockers.push(`drill_once option count=${drill.options?.length ?? "missing"}; expected 1..8`);
      }
      if (drill.remaining_rounds !== 1) {
        blockers.push(`drill_once remaining_rounds=${drill.remaining_rounds ?? "missing"}; expected 1`);
      }
      if (!String(drill.core_generation_id ?? "").trim()) {
        blockers.push("drill_once core_generation_id is missing");
      }
    }
  } else if (disposition?.drill != null) {
    blockers.push(`${dispositionKind ?? "unknown"} packet unexpectedly includes a drill plan`);
  }
  const boundedCounters = [
    ["anchors", used.anchors, limits.max_anchors],
    ["files", used.files, limits.max_files],
    ["snippets", used.snippets, limits.max_snippets],
    ["trail_edges", used.trail_edges, limits.max_trail_edges],
    ["output_bytes", used.output_bytes, limits.max_output_bytes],
  ];
  for (const [name, observed, maximum] of boundedCounters) {
    if (!Number.isInteger(observed) || observed < 0) {
      blockers.push(`budget.used.${name} is missing or invalid`);
    }
    if (!Number.isInteger(maximum) || maximum < 0) {
      blockers.push(`budget.limits.max_${name} is missing or invalid`);
    }
    if (Number.isInteger(observed) && Number.isInteger(maximum) && observed > maximum) {
      blockers.push(`budget.used.${name}=${observed} exceeds ${maximum}`);
    }
  }
  for (const [name, observed, declared, publicMaximum] of [
    ["anchors", used.anchors, limits.max_anchors, PUBLIC_PACKET_MAX_ANCHORS],
    ["trail_edges", used.trail_edges, limits.max_trail_edges, PUBLIC_PACKET_MAX_TRAIL_EDGES],
    ["output_bytes", used.output_bytes, limits.max_output_bytes, PUBLIC_PACKET_MAX_OUTPUT_BYTES],
  ]) {
    if (Number.isInteger(declared) && declared !== publicMaximum) {
      blockers.push(
        `budget.limits.max_${name}=${declared} does not equal public cap=${publicMaximum}`,
      );
    }
    if (Number.isInteger(observed) && observed > publicMaximum) {
      blockers.push(`budget.used.${name}=${observed} exceeds public cap=${publicMaximum}`);
    }
  }
  if (used.output_bytes !== actualStdoutBytes) {
    blockers.push(
      `budget.used.output_bytes=${used.output_bytes ?? "missing"} does not match CLI stdout bytes=${actualStdoutBytes}`,
    );
  }
  if (used.anchors !== citationCount) {
    blockers.push(
      `budget.used.anchors=${used.anchors ?? "missing"} does not match citation count=${citationCount}`,
    );
  }
  if (used.files !== fileCount) {
    blockers.push(
      `budget.used.files=${used.files ?? "missing"} does not match unique citation files=${fileCount}`,
    );
  }
  if (used.snippets !== snippetCount) {
    blockers.push(
      `budget.used.snippets=${used.snippets ?? "missing"} does not match successful source reads=${snippetCount}`,
    );
  }
  if (used.trail_edges !== graphEdgeOccurrences) {
    blockers.push(
      `budget.used.trail_edges=${used.trail_edges ?? "missing"} does not match serialized graph edges=${graphEdgeOccurrences}`,
    );
  }
  if (options.requireSupported) {
    if (dispositionKind !== "supported") {
      blockers.push(`packet disposition=${dispositionKind ?? "missing"}; expected supported`);
    }
    const retrievalShadow = packetRetrievalShadow(packet);
    if (!retrievalShadow) {
      blockers.push("packet retrieval shadow is missing");
    } else {
      if (retrievalShadow.retrieval_mode !== "full") {
        blockers.push(
          `packet retrieval shadow mode=${retrievalShadow.retrieval_mode ?? "missing"}; expected full`,
        );
      }
      for (const [field, value] of [
        ["degraded_reason", retrievalShadow.degraded_reason],
        ["error", retrievalShadow.error],
        ["cancel_reason", retrievalShadow.cancel_reason],
      ]) {
        if (value) {
          blockers.push(`packet retrieval shadow ${field}=${value}`);
        }
      }
    }
  }
  if (options.requireManagedRuntime) {
    blockers.push(
      ...managedRuntimeIdentityBlockers(
        packet?._meta?.codestory_publication?.contract_runtime,
        "packet",
      ),
    );
  }
  return blockers;
}

function packetCommandFailureEnvelope(value) {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    value.schema_version !== 1 ||
    !value.error ||
    typeof value.error !== "object" ||
    Array.isArray(value.error) ||
    !nonemptyString(value.error.code) ||
    !nonemptyString(value.error.message)
  ) {
    return null;
  }
  return value;
}

function packetCommandFailureReason(envelope) {
  if (!envelope) {
    return null;
  }
  const failedLayer = envelope.error?.details?.failed_layer;
  const context = envelope.context;
  return [
    `${envelope.error.code}: ${envelope.error.message}`,
    nonemptyString(failedLayer) ? `failed_layer=${failedLayer}` : null,
    context == null ? null : `context=${JSON.stringify(context)}`,
  ].filter(Boolean).join("; ");
}

async function runCodeStoryPacketPrelude(opts, run, repoConfig, outDir, runId, codestoryCli, env) {
  const args = packetCommandArgs(repoConfig, run.task, opts);
  const extraProbes = packetCommandExtraProbes(run.task, opts);
  let command = displayCommand(codestoryCli, args);
  let activeArgs = args;
  const stdoutPath = path.join(outDir, `${runId}.codestory-packet.stdout.json`);
  const stderrPath = path.join(outDir, `${runId}.codestory-packet.stderr.txt`);
  const started = performance.now();
  let result = await runProcess(codestoryCli, args, {
    cwd: repoConfig.path,
    env,
    signal: opts.signal,
    timeoutMs: opts.timeoutMs,
    timeoutMessage: `CodeStory packet prelude timed out after ${opts.timeoutMs}ms.`,
  });
  await writeFile(stdoutPath, result.stdout, "utf8");
  await writeFile(stderrPath, result.stderr, "utf8");

  let parsedOutput = null;
  let parseError = null;
  if (result.stdout.trim()) {
    try {
      parsedOutput = JSON.parse(result.stdout);
    } catch (error) {
      parseError = error.message;
    }
  }
  let commandFailure = packetCommandFailureEnvelope(parsedOutput);
  let commandFailureReason = packetCommandFailureReason(commandFailure);
  let packet = commandFailure ? null : parsedOutput;

  let activeStdoutPath = stdoutPath;
  let activeStderrPath = stderrPath;
  let drillContinuation = false;
  if (
    result.status === "pass" &&
    !parseError &&
    (
      packet?.disposition?.kind === "drill_once" ||
      (isPacketProjectionV3(packet) && packet?.status === "continuation_available")
    )
  ) {
    const drillArgs = drillPacketCommandArgs(repoConfig, run.task, opts, packet);
    if (drillArgs) {
      const drillStdoutPath = path.join(outDir, `${runId}.codestory-packet-drill.stdout.json`);
      const drillStderrPath = path.join(outDir, `${runId}.codestory-packet-drill.stderr.txt`);
      const drillResult = await runProcess(codestoryCli, drillArgs, {
        cwd: repoConfig.path,
        env,
        signal: opts.signal,
        timeoutMs: opts.timeoutMs,
        timeoutMessage: `CodeStory packet drill continuation timed out after ${opts.timeoutMs}ms.`,
      });
      await writeFile(drillStdoutPath, drillResult.stdout, "utf8");
      await writeFile(drillStderrPath, drillResult.stderr, "utf8");
      if (drillResult.status === "pass") {
        try {
          const continuationPacket = JSON.parse(drillResult.stdout);
          if (publicPacketPreludeContractPasses(continuationPacket, drillResult.stdout)) {
            packet = continuationPacket;
            result = drillResult;
            activeArgs = drillArgs;
            command = displayCommand(codestoryCli, drillArgs);
            activeStdoutPath = drillStdoutPath;
            activeStderrPath = drillStderrPath;
            drillContinuation = true;
            parseError = null;
            commandFailure = null;
            commandFailureReason = null;
          }
        } catch (error) {
          parseError = error.message;
        }
      }
    }
  }

  const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
  const manifestQuality = packetManifestQualitySummary(packet, run.task);
  const contractBlockers = parseError
    ? [`packet JSON parse failed: ${parseError}`]
    : commandFailureReason
      ? [commandFailureReason]
      : packetPreludeContractBlockers(packet, result.stdout, {
          requireSupported: opts.publishable,
          requireManagedRuntime: opts.publishable,
          expectedQuestion: run.task?.prompt ?? repoConfig.prompt,
        });
  const dispositionTelemetry = packetDispositionTelemetry(packet, manifestQuality);
  const evidenceAvailabilityTelemetry = packetV3EvidenceAvailabilityTelemetry(
    packet,
    manifestQuality,
  );
  const evidenceGapAccounting = packetV3EvidenceGapAccounting(packet);
  const sufficiencyTelemetry = packetSufficiencyTelemetry(packet, manifestQuality);
  const publicPrelude = preludePublicFields({
    command,
    args: activeArgs,
    status: result.status === "pass" && !parseError && !commandFailure && !contractBlockers.length
      ? "pass"
      : "fail",
    process_status: result.status,
    exit_code: result.exitCode,
    signal: result.signal,
    error: result.error ?? parseError ?? commandFailureReason ?? contractBlockers[0] ?? null,
    wall_ms: wallMs,
    stdout_path: activeStdoutPath,
    stderr_path: activeStderrPath,
    stdout_bytes: Buffer.byteLength(result.stdout, "utf8"),
    stderr_bytes: Buffer.byteLength(result.stderr, "utf8"),
    packet_parse_error: parseError,
    packet_schema_version: packet?.schema_version ?? null,
    packet_projection_kind: isPacketProjectionV3(packet) ? packet?.kind ?? null : null,
    packet_evidence_availability: evidenceAvailabilityTelemetry,
    packet_evidence_gap_accounting: evidenceGapAccounting,
    packet_evidence_count: evidenceGapAccounting?.evidence_count ?? null,
    packet_gap_count: evidenceGapAccounting?.gap_count ?? null,
    packet_disposition_kind: isPacketProjectionV3(packet)
      ? null
      : dispositionTelemetry?.kind ?? null,
    packet_disposition: isPacketProjectionV3(packet) ? null : dispositionTelemetry,
    packet_sufficiency_status: sufficiencyTelemetry?.status ?? null,
    packet_sufficiency: sufficiencyTelemetry,
    packet_support_count: dispositionTelemetry?.support_count ?? null,
    packet_support_kind_counts: dispositionTelemetry?.support_kind_counts ?? null,
    packet_citation_count: Array.isArray(packet?.answer?.citations)
      ? packet.answer.citations.length
      : null,
    packet_avoid_opening_count: packet ? packetAvoidOpeningRawPaths(packet).length : null,
    packet_latency: packetLatencyTelemetry(packet, wallMs),
    packet_composition: packetComposition(packet, run.task),
    packet_manifest_quality: manifestQuality,
    packet_contract_runtime: packet?._meta?.codestory_publication?.contract_runtime ?? null,
    packet_extra_probe_count: extraProbes.length,
    packet_extra_probe_strategy: packetExtraProbeStrategy(extraProbes),
    packet_drill_continuation: drillContinuation,
    packet_contract_blockers: contractBlockers,
    packet_command_failure: commandFailure,
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
  const resolvedCodeStoryCli = isCodeStoryArm(run.arm)
    ? resolveCodeStoryCliForArm(opts, run.arm)
    : null;
  const codestoryPreludeCli = resolvedCodeStoryCli
    ? path.isAbsolute(resolvedCodeStoryCli) || /[\\/]/.test(resolvedCodeStoryCli)
      ? path.resolve(resolvedCodeStoryCli)
      : resolvedCodeStoryCli
    : null;
  const codestoryPreludeCliSha256 = codestoryPreludeCli && existsSync(codestoryPreludeCli)
    ? (await sha256FileBounded(
      codestoryPreludeCli,
      MAX_EXACT_ARCHIVE_BYTES,
      "CodeStory prelude CLI",
    )).sha256
    : null;
  const env = agentRunnerEnv(
    selectedBenchmarkChildEnv(opts, run.arm),
    opts.agentCodexHomes?.[run.arm] ?? null,
    !opts.exactCandidate || isCodeStoryArm(run.arm),
  );
  const baselinePrelude =
    run.arm === "without_codestory"
      ? await runBaselinePrelude(opts, run, repoConfig, outDir, runId)
      : null;
  const codestoryPrelude =
    isCodeStoryArm(run.arm)
      ? await runCodeStoryPacketPrelude(
          opts,
          run,
          repoConfig,
          outDir,
          runId,
          codestoryPreludeCli,
          env,
        )
      : null;
  const prompt = composePrompt(run.repo, repoConfig, run.arm, run.task, {
    baselinePrelude,
    codestoryPrelude,
  });
  const { command, args, stdin, killProcessTree } = runnerCommand(
    opts,
    repoConfig.path,
    prompt,
    run.arm,
  );
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
        signal: opts.signal,
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
  const analysis = analyzeTranscript(analysisEvents, repoConfig.path, { task: run.task, arm: run.arm });
  const codestoryBinaryIdentity = isCodeStoryArm(run.arm)
    ? codeStoryBinaryIdentity(codestoryPreludeCliSha256, analysis)
    : null;
  const binaryIdentityFailed = codestoryBinaryIdentity != null && ![
    "prelude_only",
    "exact_match",
  ].includes(codestoryBinaryIdentity.status);
  const provenance = await repoProvenance(repoConfig, opts.signal);
  const packetFirstRequired = isCodeStoryArm(run.arm);
  const packetFirstPass =
    !packetFirstRequired || Boolean(analysis.packet_was_first_context_command);
  const quality = scoreQuality(analysisEvents, run.task);
  const cacheProvenance = isCodeStoryArm(run.arm)
    ? await codestoryCacheProvenance(
        opts,
        repoConfig,
        agentPacketPreludeCacheObservations(
          opts,
          run.repo,
          codestoryPrelude?.packet ?? null,
          analysis,
          run.arm,
        ),
        run.arm,
      )
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
    canary: run.canary === true,
    preparation_overlap: run.preparation_overlap === true,
    comparative_wall_time_eligible: run.comparative_wall_time_eligible !== false,
    runner: opts.runner,
    model: opts.model,
    sandbox: opts.sandbox,
    command,
    args,
    stdin: stdin == null ? null : "<prompt>",
    codestory_cli_env: null,
    codestory_prelude_cli: isCodeStoryArm(run.arm) ? codestoryPreludeCli : null,
    codestory_prelude_cli_sha256: isCodeStoryArm(run.arm)
      ? codestoryPreludeCliSha256
      : null,
    package_identity: opts.exactCandidate
      ? exactCandidatePackageIdentity(opts.exactCandidatePackageByArm?.get(run.arm), run.arm)
      : null,
    source_cli_identity: opts.exactCandidate
      ? exactCandidateSourceCliIdentity(opts.exactCandidatePackageByArm?.get(run.arm), run.arm)
      : null,
    codestory_binary_identity: codestoryBinaryIdentity,
    repo_path: repoConfig.path,
    repo_provenance: provenance,
    codestory_cache_provenance: cacheProvenance,
    benchmark_contract: benchmarkContract,
    promotion_eligible: benchmarkContract.promotion_eligible,
    status: opts.signal?.aborted || result.status === "aborted"
      ? "cancelled"
      : binaryIdentityFailed
        ? "fail"
        : result.status,
    exit_code: result.exitCode,
    signal: result.signal,
    error: binaryIdentityFailed
      ? `CodeStory binary identity ${codestoryBinaryIdentity.status}`
      : result.error,
    wall_ms: wallMs,
    exact_candidate_timing: opts.exactCandidate
      ? {
          cold_ms: cachePreparationForRepo(opts, run.repo, run.arm)?.preparation_wall_ms ?? 0,
          warm_ms: wallMs,
          incremental_ms: cachePreparationForRepo(opts, run.repo, run.arm)?.incremental_wall_ms ?? 0,
          all_in_ms: wallMs,
        }
      : null,
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

async function gitOutput(
  args,
  cwd,
  timeoutMs = 10_000,
  signal = null,
  processOptions = {},
) {
  const result = await runProcess("git", args, {
    ...processOptions,
    cwd,
    timeoutMs,
    signal,
  });
  if (result.exitCode !== 0) {
    return null;
  }
  return result.stdout.trim();
}

async function installedCodestoryProjectManifestProvenance(
  config,
  checkoutPath,
  signal = null,
  processOptions = {},
) {
  const manifest = config.manifest_codestory_project_manifest ?? null;
  if (!manifest) {
    return config.installed_codestory_project_manifest ?? null;
  }
  const workspacePath = path.resolve(config.path);
  assertPathInside(checkoutPath, workspacePath, "CodeStory project manifest workspace path");
  const destination = assertPathInside(
    workspacePath,
    path.join(workspacePath, "codestory_project.json"),
    "CodeStory project manifest destination",
  );
  const relativeDestination = path.relative(checkoutPath, destination).replaceAll(path.sep, "/");
  if (!relativeDestination || relativeDestination.startsWith("../") || path.isAbsolute(relativeDestination)) {
    return null;
  }
  let installedBytes;
  try {
    installedBytes = await readFile(destination);
  } catch {
    return null;
  }
  const ignored = await runProcess(
    "git",
    ["-C", checkoutPath, "check-ignore", "-q", "--", relativeDestination],
    {
      ...processOptions,
      cwd: repoRoot,
      timeoutMs: 10_000,
      signal,
    },
  );
  return {
    source_path: path.relative(repoRoot, manifest.source_path).replaceAll(path.sep, "/"),
    declared_sha256: manifest.sha256,
    installed_path: relativeDestination,
    installed_sha256: sha256Bytes(installedBytes),
    ignored: ignored.exitCode === 0,
  };
}

async function repoProvenance(config, signal = null, processOptions = {}) {
  const checkoutPath = path.resolve(config.checkout_path ?? config.path);
  const statusShort = await gitOutput(
    ["-C", checkoutPath, "status", "--short"],
    repoRoot,
    10_000,
    signal,
    processOptions,
  );
  return {
    resolved_path: config.path,
    checkout_path: checkoutPath,
    workspace_root: config.workspace_root ?? null,
    manifest: {
      url: config.manifest_url ?? null,
      ref: config.manifest_ref ?? null,
      workspace_root: config.manifest_workspace_root ?? null,
      checkout_path: config.manifest_checkout_path ?? null,
      codestory_project_manifest: config.manifest_codestory_project_manifest
        ? {
            path: config.manifest_codestory_project_manifest.declared_path,
            sha256: config.manifest_codestory_project_manifest.sha256,
          }
        : null,
    },
    configured: {
      url: config.url ?? null,
      ref: config.ref ?? null,
      languages: config.languages ?? [],
    },
    manifest_overridden_by_builtin: Boolean(config.manifest_overridden_by_builtin),
    git_head: await gitOutput(
      ["-C", checkoutPath, "rev-parse", "HEAD"],
      repoRoot,
      10_000,
      signal,
      processOptions,
    ),
    git_origin: redactUrlForDisplay(
      await gitOutput(
        ["-C", checkoutPath, "remote", "get-url", "origin"],
        repoRoot,
        10_000,
        signal,
        processOptions,
      ),
    ),
    git_dirty: statusShort == null ? null : statusShort.length > 0,
    git_status_short: statusShort,
    installed_codestory_project_manifest: await installedCodestoryProjectManifestProvenance(
      config,
      checkoutPath,
      signal,
      processOptions,
    ),
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
    semantic_generation: output?.manifest?.semantic_generation ?? null,
    embedding_device_policy: output?.embedding_device_policy ?? null,
    embedding_device_state: output?.embedding_device_state ?? null,
    embedding_device_observation_source: output?.embedding_device_observation_source ?? null,
    embedding_detected_provider: output?.embedding_detected_provider ?? null,
    embedding_detected_gpu: output?.embedding_detected_gpu ?? null,
    embedding_accelerator_requested: output?.embedding_accelerator_requested ?? null,
    embedding_accelerator_request_provider: output?.embedding_accelerator_request_provider ?? null,
    embedding_accelerator_request_device: output?.embedding_accelerator_request_device ?? null,
    embedding_cpu_allowed: output?.embedding_cpu_allowed ?? null,
    embedding_model_sha256: null,
    embedding_ggml_build_identity: null,
    embedding_backend: null,
    embedding_adapter: null,
    embedding_policy: null,
    embedding_engine_instance_id: null,
    embedding_model_load_count: null,
    embedding_smoke_ms: null,
    embedding_initialization_ms: null,
    embedding_materialized_reused: null,
    embedding_accelerator_execution_verified: null,
    local_only: null,
    locality_kind: null,
    locality_evidence: null,
    lexical_capabilities: output?.lexical?.capabilities ?? null,
    semantic_capabilities: output?.semantic?.capabilities ?? null,
    scip_capabilities: output?.scip?.capabilities ?? null,
    stdout_tail: result.status === "pass" ? null : trimTail(result.stdout),
    stderr_tail: result.status === "pass" ? null : trimTail(result.stderr),
  };
}

const RETRIEVAL_ENGINE_DIAGNOSTICS_URI = "codestory://diagnostics/retrieval-engine";
const RETRIEVAL_ENGINE_IDENTITY_FIELDS = [
  "embedding_model_sha256",
  "embedding_ggml_build_identity",
  "embedding_backend",
  "embedding_adapter",
  "embedding_adapter_description",
  "embedding_policy",
  "embedding_engine_instance_id",
  "embedding_engine_residency",
  "embedding_engine_load_generation",
  "embedding_engine_load_error",
  "embedding_model_load_count",
  "embedding_smoke_ms",
  "embedding_initialization_ms",
  "embedding_materialized_reused",
  "embedding_accelerator_execution_verified",
  "embedding_execution_devices",
  "embedding_execution_backends",
  "embedding_execution_observation_source",
  "embedding_encode_count",
  "embedding_execution_node_count",
  "embedding_resident_accelerator_tensor_count",
  "embedding_resident_accelerator_tensor_bytes",
  "embedding_model_layer_count",
  "embedding_offloaded_layer_count",
];

function strictUriComponent(value) {
  return encodeURIComponent(value).replace(/[!'()*]/g, (character) =>
    `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

function projectResourcePath(project, platform = process.platform) {
  let value = String(project);
  if (platform === "win32") {
    value = value.replaceAll("\\", "/");
    if (value.startsWith("//?/UNC/")) {
      value = `//${value.slice("//?/UNC/".length)}`;
    } else if (value.startsWith("//?/")) {
      value = value.slice("//?/".length);
    }
  }
  return value;
}

function projectResourceUri(baseUri, project, platform = process.platform) {
  return `${baseUri}?project=${strictUriComponent(projectResourcePath(project, platform))}`;
}

function projectResourceUriParts(uri) {
  const marker = "?project=";
  const index = String(uri).indexOf(marker);
  if (index <= 0 || String(uri).indexOf(marker, index + marker.length) >= 0) return null;
  const baseUri = String(uri).slice(0, index);
  const encodedProject = String(uri).slice(index + marker.length);
  if (!encodedProject) return null;
  try {
    const project = decodeURIComponent(encodedProject);
    if (strictUriComponent(project) !== encodedProject) return null;
    return { baseUri, project };
  } catch {
    return null;
  }
}

function resourceUriMatches(expectedUri, actualUri, platform = process.platform, sameFile = null) {
  if (actualUri === expectedUri) return true;
  const expected = projectResourceUriParts(expectedUri);
  const actual = projectResourceUriParts(actualUri);
  if (!expected || !actual || expected.baseUri !== actual.baseUri) return false;
  const pathApi = platform === "win32" ? path.win32 : path.posix;
  if (!pathApi.isAbsolute(expected.project) || !pathApi.isAbsolute(actual.project)) return false;
  const identityProbe = sameFile ?? ((left, right) => {
    const leftStat = statSync(left, { bigint: true });
    const rightStat = statSync(right, { bigint: true });
    if (leftStat.ino !== 0n || rightStat.ino !== 0n) {
      return leftStat.dev === rightStat.dev && leftStat.ino === rightStat.ino;
    }
    const leftReal = realpathSync(left);
    const rightReal = realpathSync(right);
    return platform === "win32"
      ? leftReal.toLowerCase() === rightReal.toLowerCase()
      : leftReal === rightReal;
  });
  try {
    return identityProbe(expected.project, actual.project) === true;
  } catch {
    return false;
  }
}

function retrievalEngineDiagnosticsSnapshotFromOutput(
  response,
  expectedUri,
  wallMs,
) {
  const snapshot = {
    status: "fail",
    error: null,
    wall_ms: wallMs,
    resource_uri: expectedUri,
    retrieval_mode: null,
    degraded_reason: null,
    engine: null,
    server: null,
  };

  try {
    if (response.error) {
      throw new Error(`retrieval-engine resource failed: ${JSON.stringify(response.error)}`);
    }
    const contents = response?.result?.contents;
    if (!Array.isArray(contents)) {
      throw new Error("retrieval-engine resource response lacks contents");
    }
    const matching = contents.filter((content) =>
      typeof content?.uri === "string" && resourceUriMatches(expectedUri, content.uri),
    );
    if (matching.length !== 1 || contents.length !== 1) {
      throw new Error(
        `retrieval-engine resource content mismatch: matching=${matching.length} total=${contents.length}`,
      );
    }
    if (matching[0].mimeType !== "application/json") {
      throw new Error(`retrieval-engine resource MIME type=${matching[0].mimeType ?? "missing"}`);
    }
    const diagnostics = JSON.parse(matching[0].text);
    if (!diagnostics || typeof diagnostics !== "object" || Array.isArray(diagnostics)) {
      throw new Error("retrieval-engine resource text is not an object");
    }
    if (!diagnostics.engine || typeof diagnostics.engine !== "object" || Array.isArray(diagnostics.engine)) {
      throw new Error("retrieval-engine resource lacks engine diagnostics");
    }
    snapshot.status = "pass";
    snapshot.retrieval_mode = diagnostics.retrieval_mode ?? null;
    snapshot.degraded_reason = diagnostics.degraded_reason ?? null;
    snapshot.engine = Object.fromEntries(
      RETRIEVAL_ENGINE_IDENTITY_FIELDS.map((field) => [
        field,
        field === "embedding_engine_load_error"
          ? (diagnostics.engine[field] == null ? null : "present")
          : diagnostics.engine[field] ?? null,
      ]),
    );
    const server = diagnostics.embedding_server;
    snapshot.server = server && typeof server === "object" && !Array.isArray(server)
      ? {
          lifecycle: server.lifecycle ?? null,
          peer_verified: server.authority?.peer_verified ?? null,
          server_instance_id: server.process?.server_instance_id ?? null,
          executable_sha256: server.process?.executable_sha256 ?? null,
          executable_version: server.process?.executable_version ?? null,
          load_generation: server.engine?.load_generation ?? null,
          model_load_count: server.engine?.model_load_count ?? null,
          successful_encode_count: server.engine?.successful_encode_count ?? null,
        }
      : null;
  } catch (error) {
    snapshot.status = "fail";
    snapshot.error = error instanceof Error ? error.message : String(error);
  }
  return snapshot;
}

function mergeRetrievalStatusWithEngineDiagnostics(retrievalStatus, diagnostics) {
  const merged = {
    ...retrievalStatus,
    engine_diagnostics_status: diagnostics?.status ?? "missing",
    engine_diagnostics_error: diagnostics?.error ?? null,
    engine_diagnostics_wall_ms: diagnostics?.wall_ms ?? null,
  };
  if (diagnostics?.status !== "pass") {
    return merged;
  }
  const publicMode = retrievalStatus?.retrieval_mode ?? null;
  const diagnosticMode = diagnostics.retrieval_mode ?? null;
  const publicDegraded = retrievalStatus?.degraded_reason ?? null;
  const diagnosticDegraded = diagnostics.degraded_reason ?? null;
  if (publicMode !== diagnosticMode || publicDegraded !== diagnosticDegraded) {
    merged.engine_diagnostics_status = "fail";
    merged.engine_diagnostics_error =
      `retrieval status/engine diagnostics disagree: mode=${publicMode ?? "missing"}/${diagnosticMode ?? "missing"} `
      + `degraded=${publicDegraded ?? "none"}/${diagnosticDegraded ?? "none"}`;
    return merged;
  }
  for (const field of RETRIEVAL_ENGINE_IDENTITY_FIELDS) {
    merged[field] = diagnostics.engine?.[field] ?? null;
  }
  merged.embedding_server_identity = diagnostics.server ?? null;
  merged.local_only = diagnostics.server?.peer_verified === true;
  merged.locality_kind = merged.local_only ? "same_user_local_ipc" : null;
  merged.locality_evidence = merged.local_only
    ? "retrieval embeddings execute in the peer-verified per-user CodeStory server"
    : null;
  return merged;
}

function createSequencedStdioSession(command, args, options) {
  const child = (options.spawnProcess ?? spawn)(command, args, {
    env: options.env,
    shell: false,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  let stdoutBuffer = "";
  let stdoutBytes = 0;
  const stdoutDecoder = new StringDecoder("utf8");
  let stderr = "";
  let terminalError = null;
  let exited = false;
  let terminationStarted = false;
  let forceKillTimer = null;
  const queuedResponses = [];
  const responseWaiters = [];
  let exitResolve;
  const exitPromise = new Promise((resolve) => {
    exitResolve = resolve;
  });

  function rejectWaiters(error) {
    while (responseWaiters.length) {
      responseWaiters.shift().reject(error);
    }
  }
  function terminate(signal) {
    if (exited) return;
    if (signal !== "SIGKILL" && terminationStarted) return;
    if (signal !== "SIGKILL") terminationStarted = true;
    terminateProcess(child, signal, options);
    if (signal !== "SIGKILL" && (options.forceKillAfterMs ?? 5000) > 0) {
      forceKillTimer ??= setTimeout(
        () => terminateProcess(child, "SIGKILL", options),
        options.forceKillAfterMs ?? 5000,
      );
    }
  }
  function fail(status, message) {
    if (terminalError) return;
    terminalError = Object.assign(new Error(message), { status });
    rejectWaiters(terminalError);
    terminate("SIGTERM");
  }
  function dispatchResponse(response) {
    if (queuedResponses.length >= 4) {
      fail("fail", "retrieval-engine stdio emitted too many unmatched responses");
      return;
    }
    const waiter = responseWaiters.shift();
    if (waiter) waiter.resolve(response);
    else queuedResponses.push(response);
  }

  child.stdout.on("data", (chunk) => {
    stdoutBytes += chunk.length;
    if (stdoutBytes > 1_048_576) {
      stdoutBuffer = trimTail(stdoutBuffer, 4096);
      child.stdout.pause();
      fail("fail", "retrieval-engine stdio response exceeded 1 MiB");
      return;
    }
    stdoutBuffer += stdoutDecoder.write(chunk);
    for (;;) {
      const newline = stdoutBuffer.indexOf("\n");
      if (newline < 0) break;
      const line = stdoutBuffer.slice(0, newline).trim();
      stdoutBuffer = stdoutBuffer.slice(newline + 1);
      if (!line) continue;
      try {
        dispatchResponse(JSON.parse(line));
      } catch (error) {
        fail("fail", `retrieval-engine stdio emitted malformed JSON: ${error.message}`);
      }
    }
  });
  child.stdout.on("end", () => {
    stdoutBuffer += stdoutDecoder.end();
  });
  child.stdout.on("error", (error) => {
    fail("error", `retrieval-engine stdio stdout error: ${error.message}`);
  });
  child.stderr.on("data", (chunk) => {
    stderr = trimTail(stderr + chunk.toString(), 65_536);
  });
  child.stderr.on("error", (error) => {
    fail("error", `retrieval-engine stdio stderr error: ${error.message}`);
  });
  child.stdin.on("error", (error) => {
    fail("error", `retrieval-engine stdio stdin error: ${error.message}`);
  });
  child.on("error", (error) => {
    fail("error", `retrieval-engine stdio process error: ${error.message}`);
  });
  child.on("close", (exitCode, signal) => {
    exited = true;
    if (forceKillTimer) clearTimeout(forceKillTimer);
    exitResolve({ exitCode, signal });
    if (!terminalError && (exitCode !== 0 || signal || responseWaiters.length)) {
      terminalError = Object.assign(
        new Error(
          `retrieval-engine stdio exited before completing responses: code=${exitCode ?? ""} signal=${signal ?? ""} stderr=${trimTail(stderr)}`,
        ),
        { status: "fail" },
      );
      rejectWaiters(terminalError);
    }
  });

  const onAbort = () => fail("aborted", "retrieval-engine stdio aborted by benchmark fail-fast");
  options.signal?.addEventListener("abort", onAbort, { once: true });
  if (options.signal?.aborted) onAbort();
  const timeoutTimer = setTimeout(
    () => fail("timeout", `retrieval-engine stdio timed out after ${options.timeoutMs}ms`),
    options.timeoutMs,
  );

  async function nextResponse() {
    if (terminalError) throw terminalError;
    if (queuedResponses.length) return queuedResponses.shift();
    if (exited) throw Object.assign(new Error("retrieval-engine stdio already exited"), { status: "fail" });
    return await new Promise((resolve, reject) => responseWaiters.push({ resolve, reject }));
  }
  function send(payload) {
    if (terminalError) throw terminalError;
    if (exited) throw Object.assign(new Error("retrieval-engine stdio already exited"), { status: "fail" });
    child.stdin.write(`${JSON.stringify(payload)}\n`);
  }
  async function request(payload) {
    send(payload);
    const response = await nextResponse();
    const ownsResult = Object.prototype.hasOwnProperty.call(response ?? {}, "result");
    const ownsError = Object.prototype.hasOwnProperty.call(response ?? {}, "error");
    if (response?.jsonrpc !== "2.0" || response?.id !== payload.id || ownsResult === ownsError) {
      fail(
        "fail",
        `retrieval-engine stdio response envelope mismatch for id=${JSON.stringify(payload.id)}`,
      );
      throw terminalError;
    }
    return response;
  }
  async function close() {
    if (!terminalError) child.stdin.end();
    else terminate("SIGTERM");
    const exit = await exitPromise;
    clearTimeout(timeoutTimer);
    options.signal?.removeEventListener("abort", onAbort);
    if (terminalError) throw terminalError;
    if (stdoutBuffer.trim() || queuedResponses.length || responseWaiters.length) {
      throw Object.assign(new Error("retrieval-engine stdio retained unmatched output"), { status: "fail" });
    }
    if (exit.exitCode !== 0 || exit.signal) {
      throw Object.assign(new Error("retrieval-engine stdio did not exit cleanly"), { status: "fail" });
    }
  }
  async function stop() {
    if (!exited) terminate("SIGTERM");
    await exitPromise;
    clearTimeout(timeoutTimer);
    options.signal?.removeEventListener("abort", onAbort);
  }
  return { request, send, close, stop, stderr: () => stderr };
}

async function codestoryDoctorSnapshot(
  codestoryCli,
  project,
  timeoutMs,
  env = benchmarkChildEnv(process.env),
  signal = null,
  processOptions = {},
) {
  const started = performance.now();
  const result = await runProcess(
    codestoryCli,
    ["doctor", "--project", project, "--format", "json"],
    { ...processOptions, timeoutMs, env, signal },
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

async function codestoryRetrievalStatusSnapshot(
  codestoryCli,
  project,
  timeoutMs,
  env = benchmarkChildEnv(process.env),
  signal = null,
  processOptions = {},
) {
  const started = performance.now();
  const result = await runProcess(
    codestoryCli,
    retrievalStatusCommandArgs(project),
    { ...processOptions, timeoutMs, env, signal },
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

async function codestoryRetrievalEngineDiagnosticsSnapshot(
  codestoryCli,
  project,
  timeoutMs,
  env = benchmarkChildEnv(process.env),
  signal = null,
  processOptions = {},
) {
  const uri = projectResourceUri(RETRIEVAL_ENGINE_DIAGNOSTICS_URI, project);
  const started = performance.now();
  const session = createSequencedStdioSession(
    codestoryCli,
    ["serve", "--stdio", "--multi-project", "--refresh", "none"],
    {
      ...processOptions,
      timeoutMs,
      env: {
        ...env,
        CODESTORY_RETRIEVAL_PROFILE: "agent",
        CODESTORY_RETRIEVAL_RUN_ID: BENCHMARK_AGENT_RUN_ID,
      },
      signal,
    },
  );
  try {
    const initialize = await session.request({
      jsonrpc: "2.0",
      id: "benchmark-initialize",
      method: "initialize",
      params: {
        protocolVersion: "2024-11-05",
        capabilities: {},
        clientInfo: { name: "codestory-benchmark", version: "1" },
      },
    });
    if (initialize.error) {
      throw new Error(`retrieval-engine stdio initialize failed: ${JSON.stringify(initialize.error)}`);
    }
    const negotiation = initialize?.result?._meta?.codestory_protocol;
    if (
      initialize?.result?.protocolVersion !== "2024-11-05"
      || negotiation?.status !== "agreed"
      || negotiation?.compatible !== true
    ) {
      throw new Error(
        `retrieval-engine stdio protocol negotiation failed: ${JSON.stringify(negotiation ?? null)}`,
      );
    }
    session.send({ jsonrpc: "2.0", method: "notifications/initialized" });
    const response = await session.request({
      jsonrpc: "2.0",
      id: "benchmark-retrieval-engine",
      method: "resources/read",
      params: { uri },
    });
    await session.close();
    const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
    return retrievalEngineDiagnosticsSnapshotFromOutput(response, uri, wallMs);
  } catch (error) {
    await session.stop();
    return {
      status: error?.status ?? "fail",
      error: error instanceof Error ? error.message : String(error),
      wall_ms: Math.round((performance.now() - started) * 1000) / 1000,
      resource_uri: uri,
      retrieval_mode: null,
      degraded_reason: null,
      engine: null,
      server: null,
      stderr_tail: trimTail(session.stderr()),
    };
  }
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

function retrievalIndexWorkEvidence(stdout) {
  let payload;
  try {
    payload = JSON.parse(String(stdout ?? ""));
  } catch {
    return null;
  }
  const evidence = {
    core_phase_timings: payload?.core_phase_timings ?? null,
    retrieval_phase_timings: payload?.retrieval_phase_timings ?? null,
    retrieval_component_work: payload?.retrieval_component_work ?? null,
  };
  return Object.values(evidence).some((value) => value != null) ? evidence : null;
}

function retrievalWorkEvidenceShapeBlockers(evidence, label) {
  if (!evidence || typeof evidence !== "object") {
    return [`missing ${label} retrieval work evidence`];
  }
  const blockers = [];
  const coreTimings = evidence.core_phase_timings;
  if (!coreTimings || typeof coreTimings !== "object" || Array.isArray(coreTimings) || Object.keys(coreTimings).length === 0) {
    blockers.push(`${label} core phase timings are missing`);
  }
  const phaseTimings = evidence.retrieval_phase_timings;
  if (
    !Array.isArray(phaseTimings) || phaseTimings.length === 0 ||
    phaseTimings.some((phase) =>
      typeof phase?.phase !== "string" || !phase.phase ||
      typeof phase?.elapsed_ms !== "number" || !Number.isFinite(phase.elapsed_ms) || phase.elapsed_ms < 0
    )
  ) {
    blockers.push(`${label} retrieval phase timings are missing or invalid`);
  }
  const componentWork = evidence.retrieval_component_work;
  const expectedComponents = ["graph", "lexical", "vectors"];
  const observedComponents = Array.isArray(componentWork)
    ? componentWork.map((entry) => entry?.component).sort()
    : [];
  if (
    observedComponents.length !== expectedComponents.length ||
    observedComponents.some((component, index) => component !== expectedComponents[index])
  ) {
    blockers.push(`${label} retrieval component roster must contain graph, lexical, and vectors exactly once`);
  } else if (componentWork.some((entry) =>
    !["complete", "copy_on_write", "reused"].includes(entry.mode) ||
    ![entry.retained, entry.inserted, entry.removed].every((value) =>
      typeof value === "number" && Number.isInteger(value) && value >= 0
    )
  )) {
    blockers.push(`${label} retrieval component work is missing or invalid`);
  }
  return blockers;
}

function candidateIncrementalRetrievalWorkBlockers(evidence) {
  const label = "candidate incremental";
  const blockers = retrievalWorkEvidenceShapeBlockers(evidence, label);
  if (blockers.length === 0) {
    for (const entry of evidence.retrieval_component_work) {
      if (entry.mode === "complete") {
        blockers.push(`${label} retrieval rebuilt ${entry.component} completely`);
      }
    }
  }
  return blockers;
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
    incremental_status: preparation.incremental_status ?? null,
    incremental_exit_code: preparation.incremental_exit_code ?? null,
    incremental_wall_ms: preparation.incremental_wall_ms ?? null,
    incremental_source_mutation: preparation.incremental_source_mutation ?? null,
    cold_retrieval_work_evidence: preparation.cold_retrieval_work_evidence ?? null,
    incremental_retrieval_work_evidence: preparation.incremental_retrieval_work_evidence ?? null,
    coherence_refresh_status: preparation.coherence_refresh_status ?? null,
    coherence_refresh_exit_code: preparation.coherence_refresh_exit_code ?? null,
    coherence_semantic_generation: preparation.coherence_semantic_generation ?? null,
    retrieval_mode: preparation.retrieval_status?.retrieval_mode ?? null,
    retrieval_degraded_reason: preparation.retrieval_status?.degraded_reason ?? null,
    semantic_generation: preparation.retrieval_status?.semantic_generation ?? null,
    embedding_engine_instance_id: preparation.retrieval_status?.embedding_engine_instance_id ?? null,
    embedding_policy: preparation.retrieval_status?.embedding_policy ?? null,
    manifest_embedding_backend: preparation.retrieval_status?.manifest_embedding_backend ?? null,
    before_freshness_status: preparation.before?.freshness_status ?? null,
    after_freshness_status: preparation.after?.freshness_status ?? null,
    before_semantic_ready: preparation.before?.semantic_ready ?? null,
    after_semantic_ready: preparation.after?.semantic_ready ?? null,
    before_semantic_doc_count: preparation.before?.semantic_doc_count ?? null,
    after_semantic_doc_count: preparation.after?.semantic_doc_count ?? null,
  };
}

function cachePreparationCanaryBlockers(preparation, env = process.env) {
  const blockers = [];
  if (!preparation) {
    return ["canary cache preparation is missing"];
  }
  const retrieval = preparation.retrieval_status ?? {};
  if (retrieval.status !== "pass") {
    blockers.push(`retrieval status=${retrieval.status ?? "missing"}; expected pass`);
  }
  if (preparation.retrieval_index_status !== "pass") {
    blockers.push(`retrieval preparation status=${preparation.retrieval_index_status ?? "missing"}`);
  }
  if (retrieval.retrieval_mode !== "full") {
    blockers.push(`retrieval mode=${retrieval.retrieval_mode ?? "missing"}; expected full`);
  }
  if (!retrieval.semantic_generation) {
    blockers.push("semantic generation identity is missing");
  }
  if (retrieval.degraded_reason != null) {
    blockers.push(`retrieval is degraded: ${retrieval.degraded_reason}`);
  }
  if (retrieval.engine_diagnostics_status !== "pass") {
    blockers.push(
      `retrieval engine diagnostics=${retrieval.engine_diagnostics_status ?? "missing"}: ${retrieval.engine_diagnostics_error ?? "identity unavailable"}`,
    );
  } else if (retrieval.engine_diagnostics_error != null) {
    blockers.push(`retrieval engine diagnostics retained an error: ${retrieval.engine_diagnostics_error}`);
  }
  if (retrieval.embedding_device_policy !== "accelerator_required") {
    blockers.push(
      `embedding device policy=${retrieval.embedding_device_policy ?? "missing"}; expected accelerator_required`,
    );
  }
  if (retrieval.embedding_device_state !== "accelerated") {
    blockers.push(
      `embedding device state=${retrieval.embedding_device_state ?? "missing"}; expected accelerated`,
    );
  }
  if (retrieval.embedding_cpu_allowed !== false) {
    blockers.push("runtime did not prove CPU embeddings disabled");
  }
  if (retrieval.embedding_policy !== "accelerated") {
    blockers.push(`embedding policy=${retrieval.embedding_policy ?? "missing"}; expected accelerated`);
  }
  if (retrieval.embedding_accelerator_execution_verified !== true) {
    blockers.push("accelerator execution was not verified");
  }
  if (!retrieval.embedding_backend) {
    blockers.push("accelerator backend identity is missing");
  }
  const modelSha = String(retrieval.embedding_model_sha256 ?? "");
  if (!/^[0-9a-f]{64}$/i.test(modelSha)) {
    blockers.push("embedding model digest is missing or malformed");
  } else if (!String(retrieval.manifest_embedding_backend ?? "")
    .startsWith(`per-user-server:coderank-embed:q8_0:sha256-${modelSha}:`)) {
    blockers.push("live embedding model identity does not match the retrieval manifest");
  }
  if (!retrieval.embedding_ggml_build_identity) {
    blockers.push("linked ggml build identity is missing");
  }
  const adapter = String(retrieval.embedding_adapter ?? "");
  const adapterEvidence = `${retrieval.embedding_backend ?? ""} ${adapter} ${retrieval.embedding_adapter_description ?? ""}`;
  if (!adapter) {
    blockers.push("physical accelerator adapter identity is missing");
  } else if (["llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader", "microsoft basic render driver"]
    .some((token) => adapterEvidence.toLowerCase().includes(token))) {
    blockers.push(`software accelerator adapter is not eligible: ${adapter}`);
  }
  if (!retrieval.embedding_engine_instance_id) {
    blockers.push("embedding engine instance identity is missing");
  }
  if (retrieval.embedding_engine_residency !== "resident") {
    blockers.push(`embedding engine residency=${retrieval.embedding_engine_residency ?? "missing"}; expected resident`);
  }
  if (!Number.isInteger(retrieval.embedding_engine_load_generation)
      || retrieval.embedding_engine_load_generation <= 0) {
    blockers.push("embedding engine load generation is missing");
  }
  if (!Number.isInteger(retrieval.embedding_model_load_count)
      || retrieval.embedding_model_load_count <= 0) {
    blockers.push("embedding model load count is missing");
  }
  if (retrieval.embedding_engine_load_error != null) {
    blockers.push(`embedding engine retained a load error: ${retrieval.embedding_engine_load_error}`);
  }
  if (typeof retrieval.embedding_materialized_reused !== "boolean") {
    blockers.push("embedding model materialization reuse identity is missing");
  }
  for (const [field, label] of [
    ["embedding_smoke_ms", "embedding smoke timing"],
    ["embedding_initialization_ms", "embedding initialization timing"],
  ]) {
    if (!Number.isFinite(retrieval[field]) || retrieval[field] < 0) {
      blockers.push(`${label} is missing`);
    }
  }
  if (retrieval.embedding_execution_observation_source !== "ggml_eval_callback") {
    blockers.push("accelerator execution observation is not backend-measured");
  }
  for (const [field, label] of [
    ["embedding_execution_devices", "execution devices"],
    ["embedding_execution_backends", "execution backends"],
  ]) {
    if (!Array.isArray(retrieval[field]) || retrieval[field].length === 0) {
      blockers.push(`${label} are missing`);
    }
  }
  for (const [field, label] of [
    ["embedding_encode_count", "successful encode count"],
    ["embedding_execution_node_count", "accelerator execution node count"],
    ["embedding_resident_accelerator_tensor_count", "resident accelerator tensor count"],
    ["embedding_resident_accelerator_tensor_bytes", "resident accelerator tensor bytes"],
  ]) {
    if (!Number.isInteger(retrieval[field]) || retrieval[field] <= 0) {
      blockers.push(`${label} is missing`);
    }
  }
  if (!Number.isInteger(retrieval.embedding_model_layer_count)
      || retrieval.embedding_model_layer_count <= 0) {
    blockers.push("embedding model layer count is missing");
  } else if (retrieval.embedding_offloaded_layer_count !== retrieval.embedding_model_layer_count) {
    blockers.push("not every embedding model layer was offloaded");
  }
  const server = retrieval.embedding_server_identity;
  if (server?.peer_verified !== true) blockers.push("embedding server peer identity is not verified");
  if (server?.lifecycle !== "resident") blockers.push("embedding server is not resident");
  if (!/^[0-9a-f]{64}$/i.test(String(server?.executable_sha256 ?? ""))) {
    blockers.push("embedding server executable digest is missing or malformed");
  }
  const expectedServerVersion = exactCandidateResultIdentity(preparation)?.package_version ?? null;
  if (expectedServerVersion && server?.executable_version !== expectedServerVersion) {
    blockers.push(`embedding server version=${server?.executable_version ?? "missing"}; expected ${expectedServerVersion}`);
  }
  if (!server?.server_instance_id || server.server_instance_id !== retrieval.embedding_engine_instance_id) {
    blockers.push("embedding server and engine instance identities disagree");
  }
  if (server?.load_generation !== retrieval.embedding_engine_load_generation
      || server?.model_load_count !== retrieval.embedding_model_load_count) {
    blockers.push("embedding server and engine load identities disagree");
  }
  if (!Number.isInteger(server?.successful_encode_count)
      || server.successful_encode_count < retrieval.embedding_encode_count) {
    blockers.push("embedding server successful encode count is missing");
  }
  if (retrieval.local_only !== true) {
    blockers.push("same-user local embedding execution is not proven");
  }
  if (String(env.CODESTORY_EMBED_ALLOW_CPU ?? "0") !== "0") {
    blockers.push("CPU embeddings are enabled");
  }
  return blockers;
}

function benchmarkHostClass(cachePreparations) {
  const successful = (cachePreparations ?? []).filter((row) => row?.retrieval_status);
  const reference = successful[0] ?? null;
  if (reference) {
    for (const preparation of successful.slice(1)) {
      const blockers = cachePreparationIdentityBlockers(reference, preparation);
      if (blockers.length) {
        throw new Error(
          `Benchmark preparations do not share one retrieval host class: ${blockers.join("; ")}`,
        );
      }
    }
  }
  const retrieval = reference?.retrieval_status ?? {};
  const cpus = os.cpus();
  const cpuModel = String(cpus[0]?.model ?? "")
    .normalize("NFKC")
    .trim()
    .replace(/\s+/g, " ");
  return {
    platform: process.platform,
    arch: process.arch,
    cpu_model: cpuModel || null,
    logical_cpu_count: cpus.length,
    total_memory_bytes: os.totalmem(),
    accelerator_backend: retrieval.embedding_backend ?? null,
    accelerator_adapter: retrieval.embedding_adapter ?? null,
    embedding_policy: retrieval.embedding_policy ?? null,
    model_sha256: retrieval.embedding_model_sha256 ?? null,
  };
}

const BENCHMARK_PREPARATION_IDENTITY_FIELDS = [
  "embedding_model_sha256",
  "embedding_backend",
  "embedding_adapter",
  "embedding_policy",
];

function cachePreparationIdentity(preparation) {
  const retrieval = preparation?.retrieval_status ?? {};
  return Object.fromEntries(
    BENCHMARK_PREPARATION_IDENTITY_FIELDS.map((field) => [field, retrieval[field] ?? null]),
  );
}

function cachePreparationIdentityBlockers(referencePreparation, preparation) {
  const referenceArms = referencePreparation?.arm_preparations;
  const observedArms = preparation?.arm_preparations;
  if (referenceArms || observedArms) {
    return ["published_0_17_4", "candidate_0_18"].flatMap((arm) => {
      const expected = referenceArms?.[arm];
      const observed = observedArms?.[arm];
      if (!expected || !observed) {
        return [`${arm} retrieval preparation identity is missing`];
      }
      return cachePreparationIdentityBlockers(expected, observed).map(
        (blocker) => `${arm}: ${blocker}`,
      );
    });
  }
  const expected = cachePreparationIdentity(referencePreparation);
  const observed = cachePreparationIdentity(preparation);
  return BENCHMARK_PREPARATION_IDENTITY_FIELDS.flatMap((field) =>
    observed[field] === expected[field]
      ? []
      : [
          `retrieval preparation identity changed for ${field}: `
          + `${observed[field] ?? "missing"}; expected ${expected[field] ?? "missing"}`,
        ]
  );
}

async function withExactSourceMutation(sourcePath, whileMutated, afterRestore) {
  const original = await readBoundedFile(sourcePath, 16 * 1024 * 1024, "incremental source file");
  const mutation = Buffer.concat([original.bytes, Buffer.from("\n", "utf8")]);
  const mutatedSha256 = sha256Bytes(mutation);
  if (mutatedSha256 === original.sha256) throw new Error("incremental source mutation did not change bytes");
  let result;
  try {
    await writeFile(sourcePath, mutation);
    const observedMutation = await sha256FileBounded(sourcePath, mutation.length, "mutated source file");
    if (observedMutation.sha256 !== mutatedSha256) throw new Error("incremental source mutation was not observed");
    result = await whileMutated({ original_sha256: original.sha256, mutated_sha256: mutatedSha256 });
  } finally {
    await writeFile(sourcePath, original.bytes);
    const restored = await sha256FileBounded(sourcePath, original.bytes.length, "restored source file");
    if (restored.sha256 !== original.sha256) throw new Error("incremental source restoration did not restore exact bytes");
    await afterRestore({ original_sha256: original.sha256, restored_sha256: restored.sha256 });
  }
  return {
    result,
    original_sha256: original.sha256,
    mutated_sha256: mutatedSha256,
    restored_sha256: original.sha256,
  };
}

async function measureExactIncrementalRefresh(opts, task, arm, row, childEnv) {
  const projectRoot = realpathSync(ALL_REPOS[task.repo].path);
  const candidates = (task.expected_files ?? []).map((file) => path.resolve(projectRoot, file));
  const sourcePath = candidates.find((candidate) =>
    existsSync(candidate) &&
    statSync(candidate).isFile() &&
    path.resolve(candidate) === realpathSync(candidate) &&
    isPathInside(projectRoot, realpathSync(candidate))
  );
  if (!sourcePath) throw new Error(`no pinned task source file is available for incremental timing: ${task.id}`);
  let incrementalWallMs = null;
  const mutation = await withExactSourceMutation(sourcePath, async () => {
    const incrementalStarted = performance.now();
    const incrementalArgs = retrievalIndexCommandArgs(row.project);
    incrementalArgs[incrementalArgs.indexOf("auto")] = "incremental";
    const incremental = await runProcess(resolveCodeStoryCliForArm(opts, arm), incrementalArgs, {
      env: childEnv,
      signal: opts.signal,
      timeoutMs: opts.prepareCodestoryTimeoutMs,
      timeoutMessage: `incremental timing run timed out after ${opts.prepareCodestoryTimeoutMs}ms.`,
    });
    incrementalWallMs = Math.round((performance.now() - incrementalStarted) * 1000) / 1000;
    return incremental;
  }, async () => {
    const restoreArgs = retrievalIndexCommandArgs(row.project);
    restoreArgs[restoreArgs.indexOf("auto")] = "incremental";
    const restoreIndex = await runProcess(resolveCodeStoryCliForArm(opts, arm), restoreArgs, {
      env: childEnv,
      signal: opts.signal,
      timeoutMs: opts.prepareCodestoryTimeoutMs,
      timeoutMessage: `incremental restoration timed out after ${opts.prepareCodestoryTimeoutMs}ms.`,
    });
    if (restoreIndex.status !== "pass") throw new Error(`incremental cache restoration failed for ${row.repo}/${arm}`);
    const restoredDoctor = await codestoryDoctorSnapshot(
      resolveCodeStoryCliForArm(opts, arm),
      row.project,
      60_000,
      childEnv,
      opts.signal,
    );
    if (restoredDoctor.freshness_status !== "fresh") {
      throw new Error(`incremental cache restoration stayed ${restoredDoctor.freshness_status ?? "unknown"}`);
    }
    const refreshed = await exactCandidateRetrievalSnapshot(opts, task, arm);
    row.retrieval_status = refreshed.retrieval_status;
    row.retrieval_engine_diagnostics = refreshed.retrieval_engine_diagnostics;
    row.after = refreshed.doctor;
  });
  const incremental = mutation.result;
  row.incremental_wall_ms = incrementalWallMs;
  row.incremental_status = incremental?.status ?? "error";
  row.incremental_exit_code = incremental?.exitCode ?? null;
  row.incremental_retrieval_work_evidence = retrievalIndexWorkEvidence(incremental?.stdout);
  row.incremental_source_mutation = {
    path: normalizePathLike(path.relative(projectRoot, sourcePath)),
    original_sha256: mutation.original_sha256,
    mutated_sha256: mutation.mutated_sha256,
    restored_sha256: mutation.restored_sha256,
    mutation: "append_one_lf_v1",
  };
  if (incremental?.status !== "pass") {
    throw new Error(`incremental timing failed for ${row.repo}/${arm}: ${trimTail(incremental?.stderr || incremental?.stdout)}`);
  }
  if (arm === "candidate_0_18") {
    const blockers = candidateIncrementalRetrievalWorkBlockers(
      row.incremental_retrieval_work_evidence,
    );
    if (blockers.length) {
      throw new Error(
        `candidate incremental retrieval work is not admissible for ${row.repo}: ${blockers.join("; ")}`,
      );
    }
  }
}

async function exactCandidateRetrievalSnapshot(opts, task, arm, dependencies = {}) {
  const statusSnapshot = dependencies.statusSnapshot ?? codestoryRetrievalStatusSnapshot;
  const engineSnapshot = dependencies.engineSnapshot
    ?? codestoryRetrievalEngineDiagnosticsSnapshot;
  const doctorSnapshot = dependencies.doctorSnapshot ?? codestoryDoctorSnapshot;
  const codestoryCli = resolveCodeStoryCliForArm(opts, arm);
  const project = task.project ?? ALL_REPOS[task.repo]?.path;
  if (!project) {
    throw new Error(`exact candidate repository ${task.repo} has no project path`);
  }
  const childEnv = selectedBenchmarkChildEnv(opts, arm);
  const timeoutMs = opts.prepareCodestoryTimeoutMs;
  const publicRetrievalStatus = await statusSnapshot(
    codestoryCli,
    project,
    timeoutMs,
    childEnv,
    opts.signal,
  );
  const retrievalEngineDiagnostics = await engineSnapshot(
    codestoryCli,
    project,
    timeoutMs,
    childEnv,
    opts.signal,
  );
  const retrievalStatus = mergeRetrievalStatusWithEngineDiagnostics(
    publicRetrievalStatus,
    retrievalEngineDiagnostics,
  );
  const doctor = await doctorSnapshot(
    codestoryCli,
    project,
    timeoutMs,
    childEnv,
    opts.signal,
  );
  if (
    publicRetrievalStatus.status !== "pass" ||
    retrievalEngineDiagnostics.status !== "pass" ||
    retrievalStatus.retrieval_mode !== "full" ||
    doctor.freshness_status !== "fresh"
  ) {
    throw new Error(
      `exact candidate retrieval snapshot is not coherent for ${task.repo}/${arm}`,
    );
  }
  return { retrieval_status: retrievalStatus, retrieval_engine_diagnostics: retrievalEngineDiagnostics, doctor };
}

async function refreshExactCandidatePreparation(
  opts,
  task,
  arm,
  row,
  dependencies = {},
) {
  const run = dependencies.run ?? runProcess;
  const codestoryCli = resolveCodeStoryCliForArm(opts, arm);
  const childEnv = selectedBenchmarkChildEnv(opts, arm);
  const refresh = await run(codestoryCli, retrievalIndexCommandArgs(row.project), {
    env: childEnv,
    signal: opts.signal,
    timeoutMs: opts.prepareCodestoryTimeoutMs,
    timeoutMessage: `final exact-candidate coherence refresh timed out after ${opts.prepareCodestoryTimeoutMs}ms.`,
  });
  if (refresh.status !== "pass") {
    throw new Error(
      `final exact-candidate coherence refresh failed for ${task.repo}/${arm}: ${trimTail(refresh.stderr || refresh.stdout)}`,
    );
  }
  const snapshot = await exactCandidateRetrievalSnapshot(
    opts,
    task,
    arm,
    dependencies,
  );
  row.retrieval_status = snapshot.retrieval_status;
  row.retrieval_engine_diagnostics = snapshot.retrieval_engine_diagnostics;
  row.after = snapshot.doctor;
  row.coherence_refresh_status = "pass";
  row.coherence_refresh_exit_code = refresh.exitCode ?? 0;
  row.coherence_semantic_generation = snapshot.retrieval_status.semantic_generation ?? null;
  return row;
}

function exactCandidatePreparationArmOrder(index) {
  const arms = ["published_0_17_4", "candidate_0_18"];
  return index % 2 === 0 ? arms : [...arms].reverse();
}

async function prepareCodeStoryCaches(opts, tasks) {
  if (opts.exactCandidate) {
    if (tasks.length !== 1) throw new Error("exact-candidate preparation owns one pinned repository at a time");
    const task = tasks[0];
    const sequence = opts.exactCandidateLifecycle.preparation_order?.length ?? 0;
    const armOrder = exactCandidatePreparationArmOrder(sequence);
    opts.exactCandidateLifecycle.preparation_order ??= [];
    opts.exactCandidateLifecycle.preparation_order.push({ repo: task.repo, arms: armOrder });
    const preparedByArm = new Map();
    for (const arm of armOrder) {
      for (const value of Object.values(
        exactCandidateArmEnvironmentGroups(opts, arm).directories,
      )) {
        await mkdir(value, { recursive: true, mode: 0o700 });
      }
      const childEnv = selectedBenchmarkChildEnv(opts, arm);
      const rows = await prepareCodeStoryCaches(
        {
          ...opts,
          exactCandidate: false,
          arms: ["with_codestory"],
          codestoryCli: resolveCodeStoryCliForArm(opts, arm),
          packetRuntimeChildEnv: childEnv,
        },
        [task],
      );
      for (const row of rows) {
        row.arm = arm;
        row.package_identity = exactCandidatePackageIdentity(
          opts.exactCandidatePackageByArm.get(arm),
          arm,
        );
        row.source_cli_identity = exactCandidateSourceCliIdentity(
          opts.exactCandidatePackageByArm.get(arm),
          arm,
        );
        await measureExactIncrementalRefresh(opts, task, arm, row, childEnv);
        opts.exactCandidateLifecycle.model_initialization_ms ??= {};
        if (!Object.hasOwn(opts.exactCandidateLifecycle.model_initialization_ms, arm)) {
          const initializationMs = row.retrieval_status?.embedding_initialization_ms;
          if (typeof initializationMs !== "number" || !Number.isFinite(initializationMs) || initializationMs < 0) {
            throw new Error(`missing one-time model initialization timing for ${arm}`);
          }
          opts.exactCandidateLifecycle.model_initialization_ms[arm] = initializationMs;
        }
      }
      preparedByArm.set(arm, rows);
    }
    for (const arm of ["published_0_17_4", "candidate_0_18"]) {
      for (const row of preparedByArm.get(arm)) {
        await refreshExactCandidatePreparation(opts, task, arm, row);
      }
    }
    const publishedByRepo = new Map(preparedByArm.get("published_0_17_4").map((row) => [row.repo, row]));
    const candidateByRepo = new Map(preparedByArm.get("candidate_0_18").map((row) => [row.repo, row]));
    return [task.repo].map((repo) => ({
      ...candidateByRepo.get(repo),
      arm: "candidate_0_18",
      arm_preparations: {
        published_0_17_4: publishedByRepo.get(repo),
        candidate_0_18: candidateByRepo.get(repo),
      },
    }));
  }
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
    const childEnv = selectedBenchmarkChildEnv(opts);
    const preparation = {
      repo,
      project: config.path,
      codestory_cli: path.resolve(codestoryCli),
      action: null,
      preparation_wall_ms: null,
      before: null,
      index_status: null,
      index_exit_code: null,
      index_wall_ms: 0,
      index_stdout_tail: null,
      index_stderr_tail: null,
      retrieval_status: null,
      retrieval_engine_diagnostics: null,
      cold_retrieval_work_evidence: null,
      retrieval_index_stdout_tail: null,
      retrieval_index_stderr_tail: null,
      after: null,
    };
    try {
      const before = await codestoryDoctorSnapshot(
        codestoryCli,
        config.path,
        60_000,
        childEnv,
        opts.signal,
      );
      preparation.before = before;
      preparation.after = before;
      preparation.action = cachePreparationAction(before);
      preparation.retrieval_contract = retrievalContractSummary(childEnv);
      if (shouldPrepareRetrievalIndex(childEnv)) {
        const retrievalStarted = performance.now();
        const retrievalIndex = await runProcess(
          codestoryCli,
          retrievalIndexCommandArgs(config.path),
          {
            env: childEnv,
            signal: opts.signal,
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
        preparation.cold_retrieval_work_evidence = retrievalIndexWorkEvidence(
          retrievalIndex.stdout,
        );
        if (retrievalIndex.status !== "pass") {
          throw new Error(
            `mandatory retrieval index failed for ${repo}: ${trimTail(retrievalIndex.stderr || retrievalIndex.stdout)}`,
          );
        }
        preparation.after = await codestoryDoctorSnapshot(
          codestoryCli,
          config.path,
          60_000,
          childEnv,
          opts.signal,
        );
        const publicRetrievalStatus = await codestoryRetrievalStatusSnapshot(
          codestoryCli,
          config.path,
          60_000,
          childEnv,
          opts.signal,
        );
        preparation.retrieval_engine_diagnostics =
          await codestoryRetrievalEngineDiagnosticsSnapshot(
            codestoryCli,
            preparation.after?.project ?? config.path,
            60_000,
            childEnv,
            opts.signal,
          );
        preparation.retrieval_status = mergeRetrievalStatusWithEngineDiagnostics(
          publicRetrievalStatus,
          preparation.retrieval_engine_diagnostics,
        );
        if (preparation.retrieval_status.retrieval_mode !== "full") {
          throw new Error(
            `mandatory retrieval index for ${repo} did not reach full mode: ${preparation.retrieval_status.retrieval_mode ?? "unknown"} ${preparation.retrieval_status.degraded_reason ?? ""}`.trim(),
          );
        }
      }
      return preparation;
    } catch (error) {
      preparation.error = error instanceof Error ? error.message : String(error);
      if (error && typeof error === "object") {
        error.preparation = preparation;
      }
      throw error;
    } finally {
      preparation.preparation_wall_ms =
        Math.round((performance.now() - preparationStarted) * 1000) / 1000;
    }
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
  return {
    local_only: true,
    locality_kind: "same_user_local_ipc",
    locality_evidence: `semantic backend ${backend} executes in the authenticated per-user CodeStory server`,
  };
}

function cachePolicyForRun(observations = {}) {
  if (observations.indexing_in_timed_run) {
    return "timed-run-indexed-cache";
  }
  return observations.cache_prepared ? "prepared-retrieval-cache-read-only" : "unprepared-cache-blocked";
}

function cachePreparationForRepo(opts, repoName, arm = null) {
  if (opts.exactCandidate && arm === "without_codestory") return null;
  const preparation = opts.cachePreparationByRepo;
  let row = null;
  if (preparation instanceof Map) {
    row = preparation.get(repoName) ?? null;
  }
  if (!row && Array.isArray(preparation)) {
    row = preparation.find((entry) => entry?.repo === repoName) ?? null;
  }
  return arm && row?.arm_preparations?.[arm] ? row.arm_preparations[arm] : row;
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

function agentPacketPreludeCacheObservations(opts, repoName, packet, analysis, arm = null) {
  const observations = packetRuntimeCacheObservations(
    opts,
    repoName,
    "agent_harness_prelude",
  );
  if (arm) {
    observations.cache_preparation = cachePreparationForRepo(opts, repoName, arm);
    observations.cache_prepared = Boolean(observations.cache_preparation);
  }
  observations.codestory_index_commands_observed =
    analysis?.codestory_index_commands_observed ?? 0;
  observations.indexing_in_timed_run =
    observations.codestory_index_commands_observed > 0;
  observations.packet_embedding_execution = packetEmbeddingExecutionProof(
    packet,
    observations.cache_preparation,
    observations.transport_mode,
  );
  return observations;
}

function packetEmbeddingExecutionProof(packet, cachePreparation, transportMode) {
  const retrievalContract = cachePreparation?.retrieval_contract ?? null;
  if (isPacketProjectionV3(packet)) {
    const reference = packet?.diagnostics?.availability === "available"
      ? packet.diagnostics.reference
      : null;
    return {
      source: "packet.v3_public_projection",
      schema_version: 3,
      transport_mode: transportMode,
      retrieval_contract: retrievalContract?.retrieval_contract ?? null,
      embedding_engine: retrievalContract?.embedding_engine ?? null,
      embedding_policy: retrievalContract?.execution_policy ?? null,
      retrieval_mode: packet?.retrieval?.state ?? null,
      packet_kind: packet?.kind ?? null,
      evidence_status: packet?.status ?? null,
      evidence_count: Array.isArray(packet?.evidence) ? packet.evidence.length : 0,
      gap_count: Array.isArray(packet?.gaps) ? packet.gaps.length : 0,
      core_generation: packet?.publication?.core?.generation_id ?? null,
      core_run_id: packet?.publication?.core?.run_id ?? null,
      retrieval_core_generation:
        packet?.publication?.retrieval?.core_generation_id ?? null,
      retrieval_core_run_id: packet?.publication?.retrieval?.core_run_id ?? null,
      retrieval_generation:
        packet?.publication?.retrieval?.retrieval_generation ?? null,
      retrieval_state_generation: packet?.retrieval?.generation_id ?? null,
      semantic_generation:
        packet?.publication?.retrieval?.semantic_generation ?? null,
      prepared_semantic_generation:
        cachePreparation?.retrieval_status?.semantic_generation ?? null,
      diagnostics_availability: packet?.diagnostics?.availability ?? null,
      diagnostics_artifact_id: reference?.artifact_id ?? null,
      diagnostics_sha256: reference?.sha256 ?? null,
      diagnostics_byte_length: reference?.byte_length ?? null,
    };
  }
  const trace = packet?.answer?.retrieval_trace ?? null;
  const diagnostics = Array.isArray(trace?.packet_sidecar_diagnostics)
    ? trace.packet_sidecar_diagnostics
    : [];
  const stageTimings = Array.isArray(trace?.retrieval_shadow?.stage_timings)
    ? trace.retrieval_shadow.stage_timings
    : [];
  const semanticStages = stageTimings.filter(
    (timing) => timing?.stage === "stage1b_semantic",
  );
  const completedSemanticStages = semanticStages.filter(
    (timing) =>
      timing?.completion_status === "completed"
      && timing?.degraded !== true
      && !timing?.stub_reason
      && !timing?.cancel_reason,
  );
  const fullDiagnosticCount = diagnostics.filter(
    (diagnostic) => diagnostic?.retrieval_mode === "full",
  ).length;
  return {
    source: "packet.answer.retrieval_trace",
    transport_mode: transportMode,
    retrieval_contract: retrievalContract?.retrieval_contract ?? null,
    embedding_engine: retrievalContract?.embedding_engine ?? null,
    embedding_policy: retrievalContract?.execution_policy ?? null,
    retrieval_mode:
      diagnostics.length > 0 && fullDiagnosticCount === diagnostics.length ? "full" : null,
    diagnostic_count: diagnostics.length,
    full_diagnostic_count: fullDiagnosticCount,
    semantic_stage_count: semanticStages.length,
    completed_semantic_stage_count: completedSemanticStages.length,
    invalid_semantic_stage_count: semanticStages.length - completedSemanticStages.length,
    shadow_degraded_reason: trace?.retrieval_shadow?.degraded_reason ?? null,
    shadow_error: trace?.retrieval_shadow?.error ?? null,
    shadow_cancel_reason: trace?.retrieval_shadow?.cancel_reason ?? null,
    semantic_fallback_count: trace?.semantic_fallback_count ?? null,
    semantic_generation: trace?.retrieval_publication?.semantic_generation ?? null,
    prepared_semantic_generation:
      cachePreparation?.retrieval_status?.semantic_generation ?? null,
  };
}

async function codestoryCacheProvenance(opts, config, observations = {}, arm = null) {
  let codestoryCli;
  try {
    codestoryCli = arm ? resolveCodeStoryCliForArm(opts, arm) : resolveCodeStoryCli(opts);
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
    selectedBenchmarkChildEnv(opts, arm),
    opts.signal,
  );
  const retrievalStatus = observations.cache_preparation?.retrieval_status ??
    await codestoryRetrievalStatusSnapshot(
      codestoryCli,
      config.path,
      Math.min(opts.timeoutMs ?? 600_000, 60_000),
      selectedBenchmarkChildEnv(opts, arm),
      opts.signal,
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
    local_only: retrievalStatus.local_only ?? doctor.local_only ?? null,
    locality_kind: retrievalStatus.locality_kind ?? doctor.locality_kind ?? null,
    locality_evidence: retrievalStatus.locality_evidence ?? doctor.locality_evidence ?? null,
    cache_policy: cachePolicyForRun(observations),
    indexing_in_timed_run: observations.indexing_in_timed_run ?? null,
    codestory_index_commands_observed: observations.codestory_index_commands_observed ?? null,
    cache_preparation: compactCachePreparation(observations.cache_preparation),
    packet_embedding_execution: observations.packet_embedding_execution ?? null,
    transport_mode: observations.transport_mode ?? null,
    retrieval_status: retrievalStatus,
    retrieval_mode: retrievalStatus.retrieval_mode ?? null,
    semantic_generation: retrievalStatus.semantic_generation ?? null,
    embedding_engine_instance_id: retrievalStatus.embedding_engine_instance_id ?? null,
    embedding_policy:
      retrievalStatus.embedding_policy
      ?? observations.packet_embedding_execution?.embedding_policy
      ?? null,
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

function sameBenchmarkRowIdentity(left, right) {
  return (
    left?.repo === right?.repo &&
    left?.task_id === right?.task_id &&
    left?.arm === right?.arm &&
    left?.repeat === right?.repeat
  );
}

async function resolveReanalysisStdoutPath(result, runDir) {
  let current = result;
  let currentRunDir = runDir;
  const visited = new Set();
  for (let depth = 0; depth < 16; depth += 1) {
    if (current.stdout_path) {
      const candidate = path.isAbsolute(current.stdout_path)
        ? current.stdout_path
        : path.resolve(currentRunDir, current.stdout_path);
      if (existsSync(candidate)) {
        return candidate;
      }
    }

    const reusedFrom = current.benchmark_contract?.reused_from;
    if (!reusedFrom) {
      return null;
    }
    const reusedRunDir = path.resolve(reusedFrom);
    if (visited.has(reusedRunDir)) {
      return null;
    }
    visited.add(reusedRunDir);
    const reusedRunsPath = path.join(reusedRunDir, "runs.jsonl");
    if (!existsSync(reusedRunsPath)) {
      return null;
    }
    const reusedRows = (await readFile(reusedRunsPath, "utf8"))
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    const reusedRow = reusedRows.find((candidate) =>
      sameBenchmarkRowIdentity(candidate, result),
    );
    if (!reusedRow) {
      return null;
    }
    current = reusedRow;
    currentRunDir = reusedRunDir;
  }
  return null;
}

async function reanalysisPacketProjection(result, runDir, task) {
  const packetPath = result.codestory_harness_prelude?.stdout_path;
  if (!packetPath) {
    return null;
  }
  const resolved = path.isAbsolute(packetPath) ? packetPath : path.resolve(runDir, packetPath);
  if (!existsSync(resolved)) {
    return null;
  }
  const packet = JSON.parse(await readFile(resolved, "utf8"));
  const manifestQuality = task ? packetManifestQualitySummary(packet, task) : null;
  const evidenceGapAccounting = packetV3EvidenceGapAccounting(packet);
  const sufficiency = packetSufficiencyTelemetry(packet, manifestQuality);
  return {
    packet_schema_version:
      packet?.schema_version ?? result.codestory_harness_prelude?.packet_schema_version ?? null,
    packet_projection_kind: isPacketProjectionV3(packet) ? packet?.kind ?? null : null,
    packet_evidence_availability: packetV3EvidenceAvailabilityTelemetry(
      packet,
      manifestQuality,
    ),
    packet_evidence_gap_accounting: evidenceGapAccounting,
    packet_evidence_count: evidenceGapAccounting?.evidence_count ?? null,
    packet_gap_count: evidenceGapAccounting?.gap_count ?? null,
    packet_sufficiency_status: sufficiency?.status ?? null,
    packet_sufficiency: sufficiency,
    packet_manifest_quality: manifestQuality,
  };
}

async function createDurableJsonlAppender(filePath, dependencies = {}) {
  await mkdir(path.dirname(filePath), { recursive: true });
  const openFile = dependencies.openFile ?? open;
  const handle = await openFile(filePath, "a");
  let pending = Promise.resolve();
  return {
    append(row) {
      pending = pending.then(async () => {
        await handle.write(`${JSON.stringify(row)}\n`, null, "utf8");
        await handle.sync();
      });
      return pending;
    },
    async close() {
      let pendingFailure = null;
      let handleFailure = null;
      try {
        await pending;
      } catch (error) {
        pendingFailure = error;
      }
      try {
        await handle.close();
      } catch (error) {
        handleFailure = error;
      }
      if (pendingFailure) {
        if (handleFailure && pendingFailure instanceof Error) {
          Object.defineProperty(pendingFailure, "benchmarkSecondaryFailures", {
            configurable: true,
            value: [benchmarkResourceFailure("ledger_handle", handleFailure)],
          });
        }
        throw pendingFailure;
      }
      if (handleFailure) throw handleFailure;
    },
  };
}

async function recomputeRunAnalysis(result, opts, runDir, taskCache) {
  const stdoutPath = await resolveReanalysisStdoutPath(result, runDir);
  if (!stdoutPath || !existsSync(stdoutPath)) {
    return {
      ...result,
      reanalysis_error: `missing stdout artifact for ${result.repo}/${result.task_id}/${result.arm}/${result.repeat}`,
    };
  }

  const { parsed, malformed } = parseJsonLines(await readFile(stdoutPath, "utf8"));
  const analysisEvents = [
    ...(await recordedHarnessPreludeEvents(result, runDir)),
    ...parsed,
  ];
  const task = await loadTaskForResult(result, opts, taskCache);
  const packetProjection = await reanalysisPacketProjection(result, runDir, task);
  const repoConfig = ALL_REPOS[result.repo] ?? null;
  const usage = extractUsage(parsed);
  const analysis = analyzeTranscript(
    analysisEvents,
    result.repo_path ?? repoConfig?.path ?? runDir,
    { task, arm: result.arm },
  );
  const packetFirstRequired = result.packet_first_required ?? isCodeStoryArm(result.arm);
  const cacheProvenance = result.codestory_cache_provenance ?? (
    repoConfig && isCodeStoryArm(result.arm)
      ? await codestoryCacheProvenance(opts, repoConfig, {
          codestory_index_commands_observed: analysis.codestory_index_commands_observed,
          indexing_in_timed_run: analysis.codestory_index_commands_observed > 0,
          cache_prepared: opts.cachePreparationByRepo?.has(result.repo) ?? false,
          cache_preparation: opts.cachePreparationByRepo?.get(result.repo) ?? null,
          transport_mode: "agent_runner",
        })
      : null
  );
  const { reanalysis_error: _staleReanalysisError, ...sourceResult } = result;
  const output = {
    ...sourceResult,
    codestory_harness_prelude: result.codestory_harness_prelude
      ? {
          ...result.codestory_harness_prelude,
          ...(packetProjection ?? {}),
        }
      : null,
    repo_provenance: result.repo_provenance ?? (repoConfig ? await repoProvenance(repoConfig) : null),
    codestory_cache_provenance: cacheProvenance,
    usage,
    estimated_cost_usd: estimateCost(usage, opts.exactCandidateCostRates),
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

function reanalysisExactCandidateAcceptance(originalSummary, rows) {
  return originalSummary.exact_candidate_acceptance == null
    ? null
    : exactCandidateAcceptance(
        rows,
        originalSummary.exact_candidate_lifecycle ?? null,
      );
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
  const reanalysisOpts = {
    ...opts,
    exactCandidateCostRates: originalSummary.cost_rates ?? null,
  };
  const rows = (await readFile(runsPath, "utf8"))
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));

  const taskCache = new Map();
  const reanalyzed = [];
  for (const row of rows) {
    reanalyzed.push(await recomputeRunAnalysis(row, reanalysisOpts, runDir, taskCache));
  }

  const summary = summarizeRuns(reanalyzed);
  const obligationAccounting = summarizePacketObligationAccounting(
    reanalyzed,
    "reanalyzed benchmark report",
  );
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
    packet_obligation_accounting: obligationAccounting,
    summary,
    cost_accounting: costAccounting,
    exact_candidate_acceptance: reanalysisExactCandidateAcceptance(
      originalSummary,
      reanalyzed,
    ),
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
  if (isPacketProjectionV3(packet)) {
    for (const row of packet.evidence ?? []) {
      chunks.push([
        row?.path,
        row?.symbol_id,
        row?.start_line == null ? null : `line ${row.start_line}`,
        row?.summary,
      ].filter(Boolean).join(" "));
    }
    for (const gap of packet.gaps ?? []) {
      chunks.push([gap?.kind, gap?.message].filter(Boolean).join(" "));
    }
    return chunks.filter(Boolean).join("\n");
  }
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
  if (isPacketProjectionV3(packet)) {
    for (const row of packet.evidence ?? []) {
      chunks.push([
        row?.path,
        row?.symbol_id,
        row?.summary,
      ].filter(Boolean).join(" "));
    }
    return chunks.join("\n");
  }
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
  const citationPaths = [
    ...(packet.answer?.citations ?? [])
    .map((citation, index) => ({
      source: "answer.citations",
      path: citation.file_path,
      rank: index + 1,
      display_name: citation.display_name ?? null,
      line: citation.line ?? null,
    }))
    .filter((entry) => entry.path),
    ...(isPacketProjectionV3(packet)
      ? (packet.evidence ?? [])
        .map((row, index) => ({
          source: "packet.evidence",
          path: row?.path,
          rank: index + 1,
          display_name: row?.symbol_id ?? null,
          line: row?.start_line ?? null,
        }))
        .filter((entry) => entry.path)
      : []),
  ];
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

function packetCoverageQueryObligations(packet) {
  const obligations = packet?.plan?.obligations?.query_obligations;
  return Array.isArray(obligations) ? obligations : null;
}

function packetCoverageUnresolvedEntryBlocks(entry, queryObligations) {
  if (packetDiagnosticIsDiagnosticOnly(entry)) {
    return false;
  }
  if (!queryObligations) {
    return true;
  }
  const query = String(
    typeof entry === "string" ? entry : entry?.query ?? "",
  ).trim();
  if (!query) {
    return true;
  }
  const matching = queryObligations.filter(
    (obligation) => String(obligation?.query ?? "").trim() === query,
  );
  if (!matching.length) {
    return true;
  }
  const material = matching.filter((obligation) => obligation?.material === true);
  if (!material.length) {
    return false;
  }
  return material.some(
    (obligation) => String(obligation?.completion?.status ?? "") !== "completed",
  );
}

function packetCoverageUnresolvedCounts(packet) {
  const unresolved =
    packet?.coverage_report?.unresolved ??
    packet?.answer?.coverage_report?.unresolved ??
    packet?.sufficiency?.coverage_report?.unresolved ??
    null;
  if (Array.isArray(unresolved)) {
    const queryObligations = packetCoverageQueryObligations(packet);
    return {
      total: unresolved.length,
      blocking: unresolved.filter((entry) =>
        packetCoverageUnresolvedEntryBlocks(entry, queryObligations)
      ).length,
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
  if (isPacketProjectionV3(packet)) {
    return {
      schema_version: 3,
      kind: packet.kind ?? null,
      status: packet.status ?? null,
      packet_bytes: jsonByteLength(packet),
      evidence_count: Array.isArray(packet.evidence) ? packet.evidence.length : 0,
      gap_count: Array.isArray(packet.gaps) ? packet.gaps.length : 0,
      continuation_present: packet.continuation != null,
      budget_limit_output_bytes: packet.kind === "budget_exceeded"
        ? packet.maximum_bytes ?? null
        : PUBLIC_PACKET_V3_MAX_OUTPUT_BYTES,
      budget_required_complete_bytes: packet.required_complete_bytes ?? null,
      budget_truncated: packet.kind === "budget_exceeded",
    };
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

function packetDispositionTelemetry(packet, quality) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  if (isPacketProjectionV3(packet)) {
    return null;
  }
  const support = Array.isArray(packet.support) ? packet.support : [];
  const supportKindCounts = {};
  for (const unit of support) {
    const kind = String(unit?.kind ?? "unknown");
    supportKindCounts[kind] = (supportKindCounts[kind] ?? 0) + 1;
  }
  const disposition = packet.disposition ?? null;
  const drill = disposition?.drill ?? null;
  const retrievalShadow = packetRetrievalShadow(packet);
  return {
    kind: disposition?.kind ?? null,
    terminal: disposition?.kind != null && disposition.kind !== "drill_once",
    reason: disposition?.reason ?? null,
    support_count: support.length,
    support_kind_counts: Object.fromEntries(
      Object.entries(supportKindCounts).sort(([left], [right]) => left.localeCompare(right)),
    ),
    omission_receipts_count: Array.isArray(disposition?.omission_receipts)
      ? disposition.omission_receipts.length
      : 0,
    drill_option_count: Array.isArray(drill?.options) ? drill.options.length : 0,
    drill_option_ids: Array.isArray(drill?.options)
      ? drill.options.map((option) => option?.id).filter(Boolean)
      : [],
    parent_packet_id: drill?.parent_packet_id ?? null,
    core_generation_id: drill?.core_generation_id ?? null,
    retrieval_generation: drill?.retrieval_generation ?? null,
    remaining_rounds: presentFiniteNumber(drill?.remaining_rounds),
    retrieval_mode: retrievalShadow?.retrieval_mode ?? null,
    degraded_reason: retrievalShadow?.degraded_reason ?? null,
    supported_quality_mismatch:
      disposition?.kind === "supported" && quality?.pass === false,
  };
}

function packetSufficiencyTelemetry(packet, quality) {
  if (!packet || typeof packet !== "object") {
    return null;
  }
  if (isPacketProjectionV3(packet)) {
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
    obligation_accounting: packetObligationAccounting(packet),
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
      candidate_resolution_ms: packetTraceNumber(step.output, "candidate_resolution_ms"),
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
  if (isPacketProjectionV3(packet)) {
    return {
      schema_version: 3,
      public_projection_only: true,
      freshness_ms: null,
      retrieval_total_ms: null,
      accounted_trace_ms: null,
      unaccounted_ms: Number.isFinite(wallMs) ? wallMs : null,
      non_trace_wall_ms: Number.isFinite(wallMs) ? wallMs : null,
      retrieval_shadow: null,
    };
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
  const provenance = await repoProvenance(repoConfig, opts.signal);
  const args = packetCommandArgs(repoConfig, task, opts);
  const started = performance.now();
  const result = await runProcess(codestoryCli, args, {
    env: selectedBenchmarkChildEnv(opts),
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
  const cacheObservations = packetRuntimeCacheObservations(
    opts,
    task.repo,
    "cold_cli_packet",
  );
  cacheObservations.packet_embedding_execution = packetEmbeddingExecutionProof(
    packet,
    cacheObservations.cache_preparation,
    cacheObservations.transport_mode,
  );
  const cacheProvenance = await codestoryCacheProvenance(
    opts,
    repoConfig,
    cacheObservations,
  );
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
    env: selectedBenchmarkChildEnv(opts),
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
  const provenance = await repoProvenance(repoConfig, opts.signal);
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
              budget: "standard",
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
  const obligationAccounting = sufficiency.obligation_accounting;
  const accountingError = packetObligationAccountingError(
    obligationAccounting,
    `${label} obligations`,
  );
  if (accountingError) {
    harnessReasons.push(accountingError);
  }
  const provenMaterial = obligationAccounting?.material_status_buckets?.proven ?? 0;
  if (
    Number.isInteger(obligationAccounting?.material) &&
    Number.isInteger(provenMaterial) &&
    provenMaterial !== obligationAccounting.material
  ) {
    productReasons.push(
      `${label} material obligations proven=${provenMaterial}/${obligationAccounting.material}; expected all`,
    );
  }
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

function groupPacketRuntimeColdJobs(tasks, repeats) {
  return [...groupTasksByRepo(tasks)].map(([repo, repoTasks]) => ({
    repo,
    jobs: repoTasks.flatMap((task) =>
      Array.from({ length: repeats }, (_, index) => ({ task, repeat: index + 1 })),
    ),
  }));
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
  opts.packetRuntimeChildEnv = benchmarkChildEnv(process.env);
  return runPacketRuntimeBenchmarkBody(opts, tasks);
}

async function runPacketRuntimeBenchmarkBody(opts, tasks) {
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
    const coldResultGroups = await parallelMap(
      groupPacketRuntimeColdJobs(tasks, opts.repeats),
      opts.jobs,
      async ({ jobs }) => {
        const repoResults = [];
        for (const { task, repeat } of jobs) {
          console.log(`packet-runtime cold-cli ${task.repo} ${task.id} repeat ${repeat}/${opts.repeats}`);
          repoResults.push(await runColdPacketRuntime(opts, task, repeat, outDir));
        }
        return repoResults;
      },
    );
    for (const coldResults of coldResultGroups) {
      for (const result of coldResults) {
        if (result) {
          results.push(result);
        }
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
  const obligationAccounting = summarizePacketObligationAccounting(
    results,
    "packet runtime report",
  );
  const blockers = packetRuntimePublishableBlockers(results, opts);
  const payload = {
    generated_at: new Date().toISOString(),
    benchmark_run_id: benchmarkId,
    codestory_cli: resolveCodeStoryCli(opts),
    modes,
    repeats: opts.repeats,
    output_dir: outDir,
    retrieval_env: benchmarkRetrievalEnv(selectedBenchmarkChildEnv(opts)),
    retrieval_contract: retrievalContractSummary(selectedBenchmarkChildEnv(opts)),
    embedding_engine: {
      ownership: "process_shared",
      lifecycle: "codestory_process",
    },
    benchmark_contract: benchmarkRunContract({
      opts,
      task: null,
      env: selectedBenchmarkChildEnv(opts),
      harnessPath: benchmarkHarnessPath,
      scorerPath: benchmarkScorerPath,
      cliIdentity: opts.codestoryCli ?? process.env.CODESTORY_CLI ?? null,
    }),
    packet_obligation_accounting: obligationAccounting,
    ...(process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT
      ? {
          release_evidence: {
            commit: process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT,
            source_tree: process.env.CODESTORY_RELEASE_EVIDENCE_TREE,
            evaluation_contract: "publishable-three-repeat-packet/v1",
            profile: process.env.CODESTORY_RELEASE_EVIDENCE_PROFILE,
            evidence_identity: {
              corpus_id: process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID,
              cache_id: process.env.CODESTORY_RELEASE_EVIDENCE_CACHE_ID,
              machine_fingerprint:
                process.env.CODESTORY_RELEASE_EVIDENCE_MACHINE_FINGERPRINT,
            },
            corpus_contract: opts.releaseEvidenceCorpusContract,
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
    interaction_turns: analysis.interaction_turns ?? null,
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
  const latencyEligible = successful.filter(
    (row) => row.comparative_wall_time_eligible !== false,
  );
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
    comparative_wall_time_eligible_runs: latencyEligible.length,
    comparative_wall_time_eligible: latencyEligible.length === successful.length,
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
    interaction_turns: {
      total: sumFinite(rows.map((row) => row.transcript_analysis?.interaction_turns?.total)),
      model_messages: sumFinite(
        rows.map((row) => row.transcript_analysis?.interaction_turns?.model_messages),
      ),
      tool_actions: sumFinite(
        rows.map((row) => row.transcript_analysis?.interaction_turns?.tool_actions),
      ),
      failed_tool_actions: sumFinite(
        rows.map((row) => row.transcript_analysis?.interaction_turns?.failed_tool_actions),
      ),
      reasoning_items_excluded: sumFinite(
        rows.map((row) => row.transcript_analysis?.interaction_turns?.reasoning_items_excluded),
      ),
      error_items_excluded: sumFinite(
        rows.map((row) => row.transcript_analysis?.interaction_turns?.error_items_excluded),
      ),
      taxonomy: "completed_agent_messages_plus_tool_actions_v1",
    },
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
          runner_wall_ms:
            withCodeStory.comparative_wall_time_eligible &&
            withoutCodeStory.comparative_wall_time_eligible
              ? accountingComparison(
                  withCodeStory.time_spent_ms.runner_wall,
                  withoutCodeStory.time_spent_ms.runner_wall,
                )
              : null,
          all_in_wall_ms:
            withCodeStory.comparative_wall_time_eligible &&
            withoutCodeStory.comparative_wall_time_eligible
              ? accountingComparison(
                  withCodeStory.time_spent_ms.all_in,
                  withoutCodeStory.time_spent_ms.all_in,
                )
              : null,
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
          interaction_turns: accountingComparison(
            withCodeStory.interaction_turns.total,
            withoutCodeStory.interaction_turns.total,
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
    const latencyEligible = successful.filter(
      (row) => row.comparative_wall_time_eligible !== false,
    );
    const qualityRows = successful.filter((row) => row.quality);
    const packetFirstRows = successful.filter((row) => row.packet_first_required);
    const packetManifestRows = successful.filter(
      (row) => row.codestory_harness_prelude?.packet_manifest_quality,
    );
    const packetDispositionCounts = {};
    for (const row of successful) {
      const kind = row.codestory_harness_prelude?.packet_disposition_kind;
      if (kind) {
        packetDispositionCounts[kind] = (packetDispositionCounts[kind] ?? 0) + 1;
      }
    }
    const binaryIdentityStatusCounts = {};
    for (const row of rows) {
      const status = row.codestory_binary_identity?.status;
      if (status) {
        binaryIdentityStatusCounts[status] = (binaryIdentityStatusCounts[status] ?? 0) + 1;
      }
    }
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
      comparative_wall_time_eligible_runs: latencyEligible.length,
      packet_first_pass_runs: packetFirstRows.filter((row) => row.packet_first_pass).length,
      packet_first_required_runs: packetFirstRows.length,
      packet_manifest_quality_pass_runs: packetManifestRows.filter(
        (row) => row.codestory_harness_prelude?.packet_manifest_quality?.pass,
      ).length,
      packet_manifest_quality_scored_runs: packetManifestRows.length,
      packet_disposition_counts: Object.fromEntries(
        Object.entries(packetDispositionCounts).sort(([left], [right]) => left.localeCompare(right)),
      ),
      packet_drill_once_runs: packetDispositionCounts.drill_once ?? 0,
      codestory_binary_identity_status_counts: Object.fromEntries(
        Object.entries(binaryIdentityStatusCounts).sort(([left], [right]) => left.localeCompare(right)),
      ),
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
      total_interaction_turns: sumFinite(
        successful.map((row) => row.transcript_analysis?.interaction_turns?.total),
      ),
      total_web_search_tool_calls: sumFinite(
        successful.map((row) => row.transcript_analysis?.tool_categories?.web_search ?? 0),
      ),
      total_direct_source_reads_total: sumFinite(
        successful.map((row) => row.transcript_analysis?.direct_source_reads_total),
      ),
      missing_token_usage_runs: successful.filter((row) => row.usage?.total_tokens == null).length,
      median_wall_ms: median(latencyEligible.map((row) => row.wall_ms)),
      observed_median_wall_ms: median(successful.map((row) => row.wall_ms)),
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
      median_interaction_turns: median(
        successful.map((row) => row.transcript_analysis?.interaction_turns?.total),
      ),
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

function managedCodeStoryRuntimeIdentityBlockers(result) {
  const reasons = [];
  const expectedVersion = exactCandidateResultIdentity(result)?.package_version ?? null;
  const analysis = result?.transcript_analysis;
  const started = analysis?.codestory_mcp_tool_calls_observed ?? 0;
  const completed = analysis?.codestory_mcp_completed_calls_observed ?? 0;
  const identities = analysis?.codestory_mcp_runtime_identities ?? [];
  if (started <= 0) {
    const prelude = result?.codestory_harness_prelude;
    if (prelude?.status !== "pass") {
      reasons.push("with_codestory arm has no passing managed CodeStory packet prelude");
      return reasons;
    }
    reasons.push(
      ...managedRuntimeIdentityBlockers(
        prelude.packet_contract_runtime,
        "CodeStory packet prelude",
        expectedVersion,
      ),
    );
    return reasons;
  }
  if (identities.length <= 0) {
    reasons.push("CodeStory arm has no managed CodeStory runtime identity");
  }
  if (identities.length < completed) {
    reasons.push(
      `CodeStory arm proved ${identities.length}/${completed} completed CodeStory MCP runtime identities`,
    );
  }
  for (const identity of identities) {
    reasons.push(...managedRuntimeIdentityBlockers(identity, "CodeStory arm", expectedVersion));
  }
  return reasons;
}

function exactPackageRuntimeIdentityBlockers(result) {
  const reasons = [];
  const packageIdentity = exactCandidateResultIdentity(result);
  const expectedContract = result?.arm === "candidate_0_18"
    ? EXACT_CANDIDATE_SOURCE_CLI_CONTRACT
    : EXACT_CANDIDATE_PACKAGE_CONTRACT;
  if (packageIdentity?.contract !== expectedContract) {
    return ["CodeStory arm has no authenticated exact runtime identity"];
  }
  const analysis = result?.transcript_analysis;
  const started = analysis?.codestory_mcp_tool_calls_observed ?? 0;
  if (started > 0) {
    return managedCodeStoryRuntimeIdentityBlockers(result);
  }
  const prelude = result?.codestory_harness_prelude;
  const runtime = prelude?.packet_contract_runtime;
  if (prelude?.status !== "pass") {
    reasons.push("CodeStory exact runtime packet prelude did not pass");
  }
  if (
    result?.codestory_prelude_cli_sha256 !== packageIdentity.cli_sha256 ||
    result?.codestory_binary_identity?.prelude_cli_sha256 !== packageIdentity.cli_sha256 ||
    !["prelude_only", "exact_match"].includes(result?.codestory_binary_identity?.status)
  ) {
    reasons.push("CodeStory CLI is not bound to the authenticated arm identity");
  }
  if (
    runtime?.cli_source !== "direct_cli_launch" ||
    runtime?.cli_version !== packageIdentity.package_version ||
    runtime?.known_override_skew_channel !== false
  ) {
    reasons.push(
      `CodeStory exact runtime is not a direct authenticated CLI ${packageIdentity.package_version}: ${JSON.stringify(runtime ?? null)}`,
    );
  }
  return reasons;
}

function packetV3EvidenceGapAccountingError(accounting, label = "packet v3 evidence/gaps") {
  if (!accounting || accounting.contract !== "codestory.packet-v3-evidence-gap-accounting/v1") {
    return `${label} accounting is missing`;
  }
  for (const [count, uniqueCount, name] of [
    [accounting.evidence_count, accounting.unique_evidence_id_count, "evidence"],
    [accounting.gap_count, accounting.unique_gap_id_count, "gap"],
    [
      accounting.continuation_gap_count,
      accounting.unique_continuation_gap_id_count,
      "continuation gap",
    ],
  ]) {
    if (!Number.isInteger(count) || count < 0 || uniqueCount !== count) {
      return `${label} ${name} identities are missing or duplicated`;
    }
  }
  if (accounting.continuation_gap_ids_bound !== true) {
    return `${label} continuation gap identities are not bound to emitted gaps`;
  }
  if (accounting.kind === "complete") {
    if (!["available", "continuation_available", "no_useful_evidence", "unavailable"].includes(accounting.status)) {
      return `${label} availability status is invalid`;
    }
  } else if (
    accounting.kind !== "budget_exceeded" ||
    accounting.status !== "unavailable" ||
    accounting.evidence_count !== 0 ||
    accounting.gap_count <= 0
  ) {
    return `${label} budget fallback accounting is invalid`;
  }
  return null;
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
        ((result.transcript_analysis?.command_categories?.codestory_cli ?? 0) > 0 ||
          (result.transcript_analysis?.codestory_mcp_tool_calls_observed ?? 0) > 0)
      ) {
        environmentReasons.push("without_codestory arm used CodeStory");
      }
      if (opts.publishable && isCodeStoryArm(result.arm)) {
        environmentReasons.push(
          ...managedCodeStoryRuntimeIdentityBlockers(result),
        );
        const obligationAccounting = resultPacketObligationAccounting(result);
        if (obligationAccounting) {
          const accountingError = packetObligationAccountingError(
            obligationAccounting,
            "codestory prelude packet obligations",
          );
          if (accountingError) {
            harnessReasons.push(accountingError);
          }
        } else if (resultRequiresPacketObligationAccounting(result)) {
          harnessReasons.push("codestory prelude packet obligation accounting is missing");
        }
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
        isCodeStoryArm(result.arm) &&
        result.packet_first_required &&
        maxSourceReadsAfterPacket == null
      ) {
        harnessReasons.push("missing explicit post-packet source-read budget");
      }
      const packetExtraProbeStrategy =
        result.codestory_harness_prelude?.packet_extra_probe_strategy ??
        result.packet_extra_probe_strategy ??
        null;
      if (opts.publishable && isCodeStoryArm(result.arm) && packetExtraProbeStrategy) {
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
        const preludeDisposition = prelude.packet_disposition;
        if (preludeDisposition) {
          if (!PACKET_DISPOSITIONS.has(preludeDisposition.kind)) {
            harnessReasons.push(
              `${label} prelude packet disposition=${preludeDisposition.kind ?? "missing"} is invalid`,
            );
          } else if (opts.publishable && preludeDisposition.kind !== "supported") {
            productReasons.push(
              `${label} prelude packet disposition=${preludeDisposition.kind}; expected supported`,
            );
          }
          if (
            preludeDisposition.kind === "supported" &&
            (presentFiniteNumber(prelude.packet_support_count) ?? 0) <= 0
          ) {
            harnessReasons.push(`${label} prelude supported packet has no support units`);
          }
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
          prelude.packet_disposition ??
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
      if (isCodeStoryArm(result.arm) && (opts.publishable || opts.enforceCacheProvenance)) {
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
      if (values == null) {
        lines.push(`| ${label} | ineligible | ineligible | ineligible | ineligible |`);
        continue;
      }
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
    commandEvent("cmd_5", "item.started", "sed -n '1,80p' /opt/codestory/SKILL.md"),
    commandEvent("cmd_5", "item.completed", "sed -n '1,80p' /opt/codestory/SKILL.md", "# Instructions"),
    commandEvent("cmd_6", "item.started", "/bin/zsh -lc \"sed -n '1,80p' /opt/codestory/references/packet.md\""),
    commandEvent("cmd_6", "item.completed", "/bin/zsh -lc \"sed -n '1,80p' /opt/codestory/references/packet.md\"", "# Packet"),
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

  const analysis = analyzeTranscript(fixtureEvents, "/workspace/repository");
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.command_categories.shell_search, 1);
  assert.equal(analysis.command_categories.direct_file_read, 4);
  assert.equal(analysis.direct_file_reads_total, 4);
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
  assert.equal(
    claimMatched(
      "Build.process processes a Jekyll site before rendering pages and documents.",
      "Build.process constructs or processes a Jekyll site.",
    ),
    true,
  );
  assert.equal(
    claimMatched(
      "Renderer handles rendering for pages and documents.",
      "Renderer renders pages and documents.",
    ),
    true,
  );
  assert.equal(
    claimMatched(
      "Renderer selects an output directory.",
      "Renderer renders pages and documents.",
    ),
    false,
  );
  assert.equal(
    forbiddenClaimMatched(
      "Jekyll does not write output before reading and rendering the site.",
      "Jekyll writes output before reading and rendering the site.",
    ),
    false,
  );
  assert.equal(
    sameBenchmarkRowIdentity(
      { repo: "repo", task_id: "task", arm: "without_codestory", repeat: 1 },
      { repo: "repo", task_id: "task", arm: "without_codestory", repeat: 1 },
    ),
    true,
  );
  assert.equal(
    sameBenchmarkRowIdentity(
      { repo: "repo", task_id: "task", arm: "without_codestory", repeat: 1 },
      { repo: "repo", task_id: "task", arm: "with_codestory", repeat: 1 },
    ),
    false,
  );
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
      plan: {
        obligations: {
          claim_obligations: [
            { material: true, proof_status: "proven" },
          ],
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
  const engineStatus = retrievalStatusSnapshotFromOutput(
    { status: "pass", exitCode: 0, timedOut: false },
    {
      retrieval_mode: "full",
      degraded_reason: null,
      embedding_device_policy: "accelerator_required",
      embedding_device_state: "accelerated",
      embedding_cpu_allowed: false,
    },
    null,
    1,
  );
  assert.equal(engineStatus.embedding_device_policy, "accelerator_required");
  assert.equal(engineStatus.embedding_device_state, "accelerated");
  assert.equal(engineStatus.embedding_cpu_allowed, false);
  assert.equal(engineStatus.locality_kind, null);
  assert.equal(engineStatus.embedding_backend, null);
  assert.equal(engineStatus.embedding_policy, null);
  assert.equal(engineStatus.embedding_engine_instance_id, null);
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
    assert.equal(cachePolicyForRun(observations), "prepared-retrieval-cache-read-only");
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
  if (opts.exactCandidate) {
    for (const [taskIndex, task] of tasks.entries()) {
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        const rotation = (taskIndex * opts.repeats + repeat - 1) % opts.arms.length;
        for (let position = 0; position < opts.arms.length; position += 1) {
          const arm = opts.arms[(rotation + position) % opts.arms.length];
          plannedRuns.push({ repo: task.repo, arm, repeat, task });
        }
      }
    }
    return plannedRuns;
  }
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

function exactCandidateAcceptance(rows, lifecycle = null) {
  const reasons = [];
  const expectedRuns = 18 * 3 * EXACT_CANDIDATE_ARMS.length;
  const expectedKeys = new Set(EXACT_CANDIDATE_TASK_IDS.flatMap((taskId) =>
    EXACT_CANDIDATE_ARMS.flatMap((arm) => [1, 2, 3].map((repeat) =>
      [taskId, EXACT_CANDIDATE_TASK_REPOS[taskId], arm, repeat].join("\t")
    ))
  ));
  const completeRows = rows.filter((row) => row?.status === "pass");
  const uniqueKeys = new Set(rows.map((row) => [row.task_id, row.repo, row.arm, row.repeat].join("\t")));
  const armCounts = Object.fromEntries(EXACT_CANDIDATE_ARMS.map((arm) => [
    arm,
    rows.filter((row) => row.arm === arm).length,
  ]));
  if (
    rows.length !== expectedRuns ||
    completeRows.length !== expectedRuns ||
    uniqueKeys.size !== expectedRuns ||
    EXACT_CANDIDATE_ARMS.some((arm) => armCounts[arm] !== 54)
  ) {
    reasons.push(
      `162 complete runs required; rows=${rows.length} complete=${completeRows.length} unique=${uniqueKeys.size}`,
    );
  }
  if (uniqueKeys.size !== expectedKeys.size || [...uniqueKeys].some((key) => !expectedKeys.has(key))) {
    reasons.push("exact task/repository/arm/repeat keys do not match the pinned 162-row contract");
  }

  const byArm = Object.fromEntries(
    EXACT_CANDIDATE_ARMS.map((arm) => [arm, rows.filter((row) => row.arm === arm)]),
  );
  const qualityPasses = Object.fromEntries(EXACT_CANDIDATE_ARMS.map((arm) => [
    arm,
    byArm[arm].filter((row) => row.quality?.pass === true).length,
  ]));
  if (qualityPasses.candidate_0_18 < qualityPasses.published_0_17_4) {
    reasons.push(
      `candidate quality ${qualityPasses.candidate_0_18} is below published 0.17.4 ${qualityPasses.published_0_17_4}`,
    );
  }
  if (qualityPasses.candidate_0_18 < qualityPasses.without_codestory) {
    reasons.push(
      `candidate quality ${qualityPasses.candidate_0_18} is below without_codestory ${qualityPasses.without_codestory}`,
    );
  }

  const comparatorErrors = new Set(
    [...byArm.without_codestory, ...byArm.published_0_17_4].flatMap((row) =>
      (row.quality?.material_factual_errors?.found_anchors ?? []).map((anchor) =>
        `${row.task_id}\t${row.repeat}\t${anchor}`
      )
    ),
  );
  const candidateOnlyErrors = byArm.candidate_0_18.flatMap((row) =>
    (row.quality?.material_factual_errors?.found_anchors ?? []).filter((anchor) =>
      !comparatorErrors.has(`${row.task_id}\t${row.repeat}\t${anchor}`)
    ).map((anchor) => ({ task_id: row.task_id, repeat: row.repeat, anchor }))
  );
  if (candidateOnlyErrors.length) {
    reasons.push(`candidate-only material factual errors=${candidateOnlyErrors.length}`);
  }
  const unsupportedProofClaims = byArm.candidate_0_18.reduce(
    (sum, row) => sum + Number(row.quality?.unsupported_proof_claims?.found ?? 0),
    0,
  );
  if (unsupportedProofClaims > 0) {
    reasons.push(`candidate unsupported proof claims=${unsupportedProofClaims}`);
  }

  const taskIds = [...new Set(rows.map((row) => row.task_id).filter(Boolean))];
  for (const taskId of taskIds) {
    const publishedPasses = byArm.published_0_17_4.filter(
      (row) => row.task_id === taskId && row.quality?.pass === true,
    ).length;
    const candidatePasses = byArm.candidate_0_18.filter(
      (row) => row.task_id === taskId && row.quality?.pass === true,
    ).length;
    if (publishedPasses - candidatePasses >= 2) {
      reasons.push(`${taskId} loses 2 repeats or more versus published 0.17.4`);
    }
  }

  const sum = (arm, selector) => byArm[arm].reduce((total, row) => {
    const value = Number(selector(row));
    return total + (Number.isFinite(value) ? value : 0);
  }, 0);
  const resourceThresholds = [
    ["tokens", (row) => row.usage?.total_tokens],
    ["tool calls", (row) => row.tool_calls_observed],
    ["cost", (row) => row.estimated_cost_usd],
  ];
  const resourceTotals = {};
  for (const [label, selector] of resourceThresholds) {
    const baseline = sum("without_codestory", selector);
    const published = sum("published_0_17_4", selector);
    const candidate = sum("candidate_0_18", selector);
    resourceTotals[label] = { without_codestory: baseline, published_0_17_4: published, candidate_0_18: candidate };
    if (candidate > published * 1.05) reasons.push(`${label} exceed 105% of published 0.17.4`);
    if (candidate > baseline * 0.8) reasons.push(`${label} exceed 80% of without_codestory`);
  }

  const uniqueRepoTiming = (arm, field) => {
    const byRepo = new Map();
    for (const row of byArm[arm]) {
      const value = row.exact_candidate_timing?.[field];
      if (byRepo.has(row.repo) && byRepo.get(row.repo) !== value) {
        reasons.push(`${arm} ${field} timing disagrees across repeats for ${row.repo}`);
      }
      byRepo.set(row.repo, value);
    }
    return [...byRepo.values()].reduce((total, value) => total + (Number.isFinite(value) ? value : 0), 0);
  };
  const lifecycleMs = (arm) => lifecycle?.package_authentication_ms?.[arm] ?? 0;
  const modelInitializationMs = (arm) => lifecycle?.model_initialization_ms?.[arm] ?? 0;
  const timingTotals = {};
  for (const arm of EXACT_CANDIDATE_ARMS) {
    const warm = sum(arm, (row) => row.exact_candidate_timing?.warm_ms);
    const measuredCold = arm === "without_codestory" ? 0 : uniqueRepoTiming(arm, "cold_ms");
    const oneTimeModel = arm === "without_codestory" ? 0 : modelInitializationMs(arm);
    const cold = Math.max(0, measuredCold - oneTimeModel);
    const incremental = arm === "without_codestory" ? 0 : uniqueRepoTiming(arm, "incremental_ms");
    timingTotals[arm] = {
      package_authentication_ms: arm === "without_codestory" ? 0 : lifecycleMs(arm),
      model_initialization_ms: oneTimeModel,
      warm_ms: warm,
      cold_ms: cold,
      incremental_ms: incremental,
      all_in_ms: warm + cold + incremental + oneTimeModel +
        (arm === "without_codestory" ? 0 : lifecycleMs(arm)),
    };
  }
  for (const [label, field, factor, display] of [
    ["warm", "warm_ms", 1.05, "105%"],
    ["cold", "cold_ms", 1.05, "5%"],
    ["incremental", "incremental_ms", 1.05, "5%"],
    ["all-in", "all_in_ms", 1.10, "110%"],
  ]) {
    const published = timingTotals.published_0_17_4[field];
    const candidate = timingTotals.candidate_0_18[field];
    if (candidate > published * factor) reasons.push(`${label} timing exceeds ${display} gate`);
  }

  for (const row of rows) {
    const finiteNonnegative = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0;
    const finiteNonnegativeInteger = (value) => finiteNonnegative(value) && Number.isInteger(value);
    if (
      !row.quality || typeof row.quality.pass !== "boolean" ||
      !row.usage || !finiteNonnegativeInteger(row.usage.total_tokens)
    ) {
      reasons.push(`missing quality or token accounting for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (!row.transcript_analysis?.tool_categories) {
      reasons.push(`missing tool categories for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (!row.transcript_analysis?.command_categories || !row.transcript_analysis?.interaction_turns) {
      reasons.push(`missing command or interaction accounting for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (!Array.isArray(row.transcript_analysis?.direct_source_reads)) {
      reasons.push(`missing direct source-read accounting for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (!finiteNonnegativeInteger(row.tool_calls_observed) || !finiteNonnegative(row.estimated_cost_usd)) {
      reasons.push(`missing tool call or cost accounting for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    for (const field of ["cold_ms", "warm_ms", "incremental_ms", "all_in_ms"]) {
      if (!finiteNonnegative(row.exact_candidate_timing?.[field])) {
        reasons.push(`missing ${field} timing for ${row.task_id}/${row.arm}/${row.repeat}`);
      }
    }
    if (!finiteNonnegative(row.wall_ms) || row.exact_candidate_timing?.warm_ms !== row.wall_ms) {
      reasons.push(`whole-task warm timing does not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (row.exact_candidate_timing?.all_in_ms !== row.exact_candidate_timing?.warm_ms) {
      reasons.push(`row all-in timing must equal whole-task warm timing for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (
      !row.quality?.material_factual_errors ||
      !row.quality?.unsupported_proof_claims
    ) {
      reasons.push(`missing factual-error or proof-claim accounting for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    const usageInput = row.usage?.input_tokens;
    const usageOutput = row.usage?.output_tokens;
    if (!finiteNonnegativeInteger(usageInput) || !finiteNonnegativeInteger(usageOutput) || usageInput + usageOutput !== row.usage?.total_tokens) {
      reasons.push(`token accounting does not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    const toolCategoryValues = Object.values(row.transcript_analysis?.tool_categories ?? {});
    const commandCategoryValues = Object.values(row.transcript_analysis?.command_categories ?? {});
    const toolCategoryTotal = toolCategoryValues.reduce((total, value) => total + value, 0);
    const commandCategoryTotal = commandCategoryValues.reduce((total, value) => total + value, 0);
    if (
      toolCategoryValues.some((value) => !finiteNonnegativeInteger(value)) ||
      commandCategoryValues.some((value) => !finiteNonnegativeInteger(value)) ||
      !finiteNonnegativeInteger(row.transcript_analysis?.command_count) ||
      toolCategoryTotal !== row.tool_calls_observed ||
      commandCategoryTotal !== row.transcript_analysis?.command_count
    ) {
      reasons.push(`tool or command categories do not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    const turns = row.transcript_analysis?.interaction_turns;
    if (
      !turns ||
      ![
        turns.total, turns.model_messages, turns.tool_actions, turns.failed_tool_actions,
        turns.reasoning_items_excluded, turns.error_items_excluded,
      ].every(finiteNonnegativeInteger) ||
      turns.total !== turns.model_messages + turns.tool_actions ||
      turns.tool_actions !== row.tool_calls_observed ||
      turns.failed_tool_actions > turns.tool_actions
    ) {
      reasons.push(`interaction accounting does not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (
      !finiteNonnegativeInteger(row.quality?.material_factual_errors?.found) ||
      !Array.isArray(row.quality?.material_factual_errors?.found_anchors) ||
      !finiteNonnegativeInteger(row.quality?.unsupported_proof_claims?.found) ||
      !Array.isArray(row.quality?.unsupported_proof_claims?.found_claims) ||
      row.quality?.material_factual_errors?.found !== row.quality?.material_factual_errors?.found_anchors?.length ||
      row.quality?.unsupported_proof_claims?.found !== row.quality?.unsupported_proof_claims?.found_claims?.length
    ) {
      reasons.push(`error or proof-claim counts do not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (
      !finiteNonnegativeInteger(row.transcript_analysis?.direct_source_reads_total) ||
      row.transcript_analysis?.direct_source_reads_total !== row.transcript_analysis?.direct_source_reads?.length
    ) {
      reasons.push(`direct source-read totals do not reconcile for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (
      !finiteNonnegativeInteger(row.transcript_analysis?.codestory_mcp_tool_calls_observed) ||
      !finiteNonnegativeInteger(row.transcript_analysis?.codestory_mcp_completed_calls_observed) ||
      !Array.isArray(row.transcript_analysis?.codestory_mcp_runtime_identities)
    ) {
      reasons.push(`CodeStory visibility accounting is incomplete for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    const eventTypeValues = Object.values(row.event_types ?? {});
    if (
      row.malformed_stdout_lines !== 0 ||
      !finiteNonnegativeInteger(row.json_events) ||
      !finiteNonnegativeInteger(row.analysis_events) ||
      row.analysis_events < row.json_events ||
      eventTypeValues.some((value) => !finiteNonnegativeInteger(value)) ||
      eventTypeValues.reduce((total, value) => total + value, 0) !== row.analysis_events
    ) {
      reasons.push(`malformed or unreconciled JSONL parser telemetry for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    const provenanceReasons = repoProvenanceBlockers(row);
    const repoMetadata = row.task_manifest_snapshot?.repo_metadata;
    if (
      row.task_manifest_snapshot?.repo !== row.repo ||
      repoMetadata?.name !== row.repo ||
      repoMetadata?.url !== row.repo_provenance?.configured?.url ||
      repoMetadata?.url !== row.repo_provenance?.manifest?.url ||
      repoMetadata?.ref !== row.repo_provenance?.configured?.ref ||
      repoMetadata?.ref !== row.repo_provenance?.manifest?.ref
    ) {
      provenanceReasons.push("task manifest repository identity does not match the observed checkout");
    }
    if (provenanceReasons.length) {
      reasons.push(`owning repo provenance is invalid for ${row.task_id}/${row.arm}/${row.repeat}: ${provenanceReasons.join("; ")}`);
    }
    if (
      !finiteNonnegativeInteger(row.transcript_analysis?.external_context_tool_calls) ||
      row.transcript_analysis.external_context_tool_calls !== 0
    ) {
      reasons.push(`external web/search context is forbidden for ${row.task_id}/${row.arm}/${row.repeat}`);
    }
    if (row.arm === "without_codestory") {
      if (
        row.package_identity != null ||
        row.source_cli_identity != null ||
        row.codestory_cache_provenance != null ||
        row.codestory_harness_prelude != null ||
        row.codestory_prelude_cli != null ||
        row.codestory_prelude_cli_sha256 != null ||
        row.codestory_binary_identity != null ||
        row.transcript_analysis?.command_categories?.codestory_cli > 0 ||
        row.transcript_analysis?.codestory_mcp_tool_calls_observed > 0 ||
        row.transcript_analysis?.codestory_mcp_completed_calls_observed > 0 ||
        row.exact_candidate_timing?.cold_ms !== 0 ||
        row.exact_candidate_timing?.incremental_ms !== 0
      ) {
        reasons.push(`baseline has CodeStory visibility or use in ${row.task_id}/${row.repeat}`);
      }
      if (
        row.packet_first_required !== false || row.packet_first_pass !== true ||
        row.transcript_analysis.command_count <= 0
      ) {
        reasons.push(`baseline local inspection telemetry is incomplete for ${row.task_id}/${row.repeat}`);
      }
    }
    if (isCodeStoryArm(row.arm)) {
      const armIdentity = exactCandidateResultIdentity(row);
      if (row.packet_first_required !== true || row.packet_first_pass !== true) {
        reasons.push(`packet-first contract failed for ${row.task_id}/${row.arm}/${row.repeat}`);
      }
      if (
        row.codestory_prelude_cli_sha256 !== armIdentity?.cli_sha256 ||
        row.codestory_binary_identity?.prelude_cli_sha256 !== armIdentity?.cli_sha256 ||
        !["prelude_only", "exact_match"].includes(row.codestory_binary_identity?.status)
      ) {
        reasons.push(`executed CLI is not bound to the authenticated ${row.arm} archive`);
      }
      for (const read of row.transcript_analysis?.direct_source_reads ?? []) {
        const authorization = read.authorization;
        const reasonIsValid = authorization?.reason === "user_named_file" ||
          (
            authorization?.reason === "explicit_evidence_gap" &&
            (authorization.evidence_command_id != null || authorization.evidence_event_index != null)
          );
        if (authorization?.status !== "authorized" || !reasonIsValid) {
          reasons.push(`unauthorized direct source read ${read.path ?? "unknown"} in ${row.task_id}/${row.arm}/${row.repeat}`);
        }
      }
      const runtimeBlockers = exactPackageRuntimeIdentityBlockers(row);
      if (runtimeBlockers.length) reasons.push(`missing per-arm exact runtime proof: ${runtimeBlockers.join("; ")}`);
      const cacheBlockers = cacheProvenanceBlockers(row);
      if (cacheBlockers.length) reasons.push(`missing per-arm cache proof: ${cacheBlockers.join("; ")}`);
      if (armIdentity?.schema_version === 3) {
        const accountingError = packetV3EvidenceGapAccountingError(
          row.codestory_harness_prelude?.packet_evidence_gap_accounting,
          "exact packet v3 evidence/gaps",
        );
        if (accountingError) {
          reasons.push(`missing per-arm v3 evidence/gap proof: ${accountingError}`);
        }
      } else {
        const obligation = resultPacketObligationAccounting(row);
        const obligationError = packetObligationAccountingError(
          obligation,
          "exact packet obligations",
        );
        if (obligationError) reasons.push(`missing per-arm obligation proof: ${obligationError}`);
      }
      if (row.codestory_harness_prelude?.packet_extra_probe_strategy != null) {
        reasons.push(`diagnostic manifest probes entered ${row.arm}`);
      }
      const mutation = row.codestory_cache_provenance?.cache_preparation?.incremental_source_mutation;
      const cachePreparation = row.codestory_cache_provenance?.cache_preparation;
      if (
        cachePreparation?.incremental_status !== "pass" ||
        !mutation ||
        typeof mutation.path !== "string" || !mutation.path ||
        !SHA256_PATTERN.test(String(mutation.original_sha256 ?? "")) ||
        !SHA256_PATTERN.test(String(mutation.mutated_sha256 ?? "")) ||
        !SHA256_PATTERN.test(String(mutation.restored_sha256 ?? "")) ||
        /^0{64}$/.test(String(mutation.original_sha256 ?? "")) ||
        /^0{64}$/.test(String(mutation.mutated_sha256 ?? "")) ||
        mutation.original_sha256 !== mutation.restored_sha256 ||
        mutation.original_sha256 === mutation.mutated_sha256
      ) {
        reasons.push(`missing verified source mutation/restore lifecycle for ${row.repo}/${row.arm}`);
      }
      if (
        cachePreparation?.preparation_wall_ms !== row.exact_candidate_timing?.cold_ms ||
        cachePreparation?.incremental_wall_ms !== row.exact_candidate_timing?.incremental_ms
      ) {
        reasons.push(`cache lifecycle timings do not reconcile for ${row.repo}/${row.arm}`);
      }
      if (row.arm === "candidate_0_18") {
        reasons.push(...retrievalWorkEvidenceShapeBlockers(
          cachePreparation?.cold_retrieval_work_evidence,
          "candidate cold",
        ));
        reasons.push(...candidateIncrementalRetrievalWorkBlockers(
          cachePreparation?.incremental_retrieval_work_evidence,
        ));
      }
      if (
        cachePreparation?.coherence_refresh_status !== "pass" ||
        !cachePreparation?.coherence_semantic_generation ||
        cachePreparation.coherence_semantic_generation !==
          row.codestory_cache_provenance?.semantic_generation
      ) {
        reasons.push(`final cross-arm cache coherence is missing for ${row.repo}/${row.arm}`);
      }
    }
  }

  for (const arm of ["published_0_17_4", "candidate_0_18"]) {
    const candidateArm = arm === "candidate_0_18";
    const identityFields = candidateArm
      ? [
          "contract", "arm", "package_version", "cli_sha256", "source_commit", "source_tree",
          "schema_version", "protocol_revision", "discovery_contract_sha256",
          "plugin_manifest_sha256", "catalog_sha256",
        ]
      : [
          "contract", "arm", "package_version", "package_sha256", "cli_sha256",
          "source_commit", "source_tree", "schema_version", "protocol_revision",
          "discovery_contract_sha256", "trust_root_kind", "trust_root_sha256",
        ];
    const identities = byArm[arm].map((row) => exactCandidateResultIdentity(row));
    const reference = identities[0];
    const expectedVersion = arm === "published_0_17_4" ? "0.17.4" : reference?.package_version;
    const expectedSchema = arm === "published_0_17_4" ? 2 : 3;
    const expectedProtocol = arm === "published_0_17_4" ? "2024-11-05" : "2025-11-25";
    const invalidDiscoveryIdentity = arm === "published_0_17_4"
      ? reference?.discovery_contract_sha256 !== null
      : !SHA256_PATTERN.test(String(reference?.discovery_contract_sha256 ?? "")) ||
        /^0{64}$/.test(String(reference?.discovery_contract_sha256 ?? ""));
    const invalidReference =
      reference?.contract !== (candidateArm
        ? EXACT_CANDIDATE_SOURCE_CLI_CONTRACT
        : EXACT_CANDIDATE_PACKAGE_CONTRACT) ||
      reference?.arm !== arm ||
      typeof reference?.package_version !== "string" || !reference.package_version.trim() ||
      reference?.package_version !== expectedVersion ||
      (!candidateArm && (
        !SHA256_PATTERN.test(String(reference?.package_sha256 ?? "")) ||
        /^0{64}$/.test(String(reference?.package_sha256 ?? ""))
      )) ||
      !SHA256_PATTERN.test(String(reference?.cli_sha256 ?? "")) ||
      /^0{64}$/.test(String(reference?.cli_sha256 ?? "")) ||
      !/^[0-9a-f]{40}$/.test(String(reference?.source_commit ?? "")) ||
      /^0{40}$/.test(String(reference?.source_commit ?? "")) ||
      !/^[0-9a-f]{40}$/.test(String(reference?.source_tree ?? "")) ||
      /^0{40}$/.test(String(reference?.source_tree ?? "")) ||
      reference?.schema_version !== expectedSchema ||
      reference?.protocol_revision !== expectedProtocol ||
      invalidDiscoveryIdentity ||
      (!candidateArm && (
        reference?.trust_root_kind !== "official_published_checksum" ||
        !SHA256_PATTERN.test(String(reference?.trust_root_sha256 ?? "")) ||
        /^0{64}$/.test(String(reference?.trust_root_sha256 ?? ""))
      )) ||
      (candidateArm && (
        !SHA256_PATTERN.test(String(reference?.plugin_manifest_sha256 ?? "")) ||
        /^0{64}$/.test(String(reference?.plugin_manifest_sha256 ?? "")) ||
        !SHA256_PATTERN.test(String(reference?.catalog_sha256 ?? "")) ||
        /^0{64}$/.test(String(reference?.catalog_sha256 ?? ""))
      ));
    const invalid = invalidReference || identities.some((identity) =>
      !identity || identityFields.some((field) => identity[field] !== reference[field])
    );
    if (invalid) {
      reasons.push(arm === "candidate_0_18"
        ? "candidate source/CLI identity mismatch"
        : "published package identity mismatch");
    }
  }

  const preparationOrder = lifecycle?.preparation_order;
  const packageAuthentication = lifecycle?.package_authentication_ms;
  const modelInitialization = lifecycle?.model_initialization_ms;
  const packageAuthenticationOrder = lifecycle?.package_authentication_order;
  const totalPackageAuthentication = lifecycle?.total_package_authentication_ms;
  const costRates = lifecycle?.cost_rates;
  if (
    lifecycle?.contract !== "codestory.agent-benchmark-exact-lifecycle/v1" ||
    !packageAuthentication || !modelInitialization ||
    !["published_0_17_4", "candidate_0_18"].every((arm) =>
      typeof packageAuthentication[arm] === "number" &&
      Number.isFinite(packageAuthentication[arm]) &&
      packageAuthentication[arm] >= 0 &&
      typeof modelInitialization[arm] === "number" &&
      Number.isFinite(modelInitialization[arm]) &&
      modelInitialization[arm] >= 0
    ) ||
    !Array.isArray(packageAuthenticationOrder) ||
    packageAuthenticationOrder.length !== 2 ||
    new Set(packageAuthenticationOrder).size !== 2 ||
    packageAuthenticationOrder.some((arm) => !["published_0_17_4", "candidate_0_18"].includes(arm)) ||
    typeof totalPackageAuthentication !== "number" ||
    !Number.isFinite(totalPackageAuthentication) ||
    totalPackageAuthentication + 0.002 <
      packageAuthentication.published_0_17_4 + packageAuthentication.candidate_0_18
  ) {
    reasons.push("exact per-arm one-time package and model lifecycle is missing or invalid");
  }
  if (
    costRates?.currency !== "USD" ||
    costRates?.model !== DEFAULT_BENCHMARK_MODEL ||
    ![costRates.input_per_mtok, costRates.output_per_mtok].every(
      (value) => typeof value === "number" && Number.isFinite(value) && value > 0,
    )
  ) {
    reasons.push("exact configured model cost rates are missing or invalid");
  }
  if (
    !Array.isArray(preparationOrder) ||
    preparationOrder.length !== 18 ||
    new Set(preparationOrder.map((entry) => entry.repo)).size !== 18 ||
    preparationOrder.some((entry) =>
      !Object.values(EXACT_CANDIDATE_TASK_REPOS).includes(entry.repo) ||
      entry.arms?.length !== 2 ||
      new Set(entry.arms).size !== 2 ||
      entry.arms.some((arm) => !["published_0_17_4", "candidate_0_18"].includes(arm))
    ) ||
    preparationOrder.filter((entry) => entry.arms[0] === "published_0_17_4").length !== 9 ||
    preparationOrder.filter((entry) => entry.arms[0] === "candidate_0_18").length !== 9
  ) {
    reasons.push("exact preparation order is not a balanced deterministic 9/9 rotation");
  }

  return {
    contract: "codestory.agent-benchmark-exact-candidate-acceptance/v2",
    pass: reasons.length === 0,
    reasons: [...new Set(reasons)],
    expected_runs: expectedRuns,
    completed_runs: completeRows.length,
    arm_counts: armCounts,
    quality_passes: qualityPasses,
    candidate_only_material_factual_errors: candidateOnlyErrors,
    unsupported_candidate_proof_claims: unsupportedProofClaims,
    resource_totals: resourceTotals,
    timing_totals: timingTotals,
  };
}

function agentRunKey(run) {
  const taskId = run.task?.id ?? run.task_id ?? "";
  return [run.repo, taskId, run.arm, String(run.repeat)].join("\t");
}

function sortAgentResultsCanonical(results, tasks, arms) {
  const taskOrder = new Map(tasks.map((task, index) => [task.id, index]));
  const armOrder = new Map(arms.map((arm, index) => [arm, index]));
  return [...results].sort((left, right) => {
    const leftTask = taskOrder.get(left.task_id) ?? Number.MAX_SAFE_INTEGER;
    const rightTask = taskOrder.get(right.task_id) ?? Number.MAX_SAFE_INTEGER;
    return (
      leftTask - rightTask ||
      (armOrder.get(left.arm) ?? Number.MAX_SAFE_INTEGER) -
        (armOrder.get(right.arm) ?? Number.MAX_SAFE_INTEGER) ||
      left.repeat - right.repeat ||
      String(left.repo).localeCompare(String(right.repo))
    );
  });
}

function withoutPooledLatency(summary) {
  return summary.map((row) => Object.fromEntries(
    Object.entries(row).map(([key, value]) => [
      key,
      key.includes("wall_ms") ? null : value,
    ]),
  ));
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
    cliIdentity: isCodeStoryArm(run.arm)
      ? opts.exactCandidatePackageByArm?.get(run.arm)?.cli_path ?? opts.codestoryCli ?? env.CODESTORY_CLI ?? null
      : null,
  });
}

function benchmarkContractEnvironment(contract) {
  return {
    contract_version: contract?.contract_version ?? null,
    scorer_hash: contract?.scorer_hash ?? null,
    harness_hash: contract?.harness_hash ?? null,
    runner: contract?.runner ?? null,
    model: contract?.model ?? null,
    sandbox: contract?.sandbox ?? null,
    retrieval_contract: contract?.retrieval_contract ?? null,
    retrieval_env: contract?.retrieval_env ?? null,
    packet_threshold_config: contract?.packet_threshold_config ?? null,
  };
}

function benchmarkContractEnvironmentSha256(contract) {
  return sha256Bytes(stableJsonForHash(benchmarkContractEnvironment(contract)));
}

function benchmarkContractProjection(contract, { pathNeutral = false } = {}) {
  return Object.fromEntries([
    "contract_version",
    "task_id",
    "task_manifest_hash",
    "scorer_hash",
    "harness_hash",
    "runner",
    "model",
    "sandbox",
    ...(!pathNeutral ? ["codestory_cli"] : []),
    "retrieval_contract",
    "retrieval_env",
    "packet_threshold_config",
  ].map((key) => [key, contract?.[key] ?? null]));
}

function benchmarkContractContentSha256(contract) {
  return sha256Bytes(stableJsonForHash(benchmarkContractProjection(contract)));
}

function benchmarkShardContractSha256(contract) {
  return sha256Bytes(stableJsonForHash(
    benchmarkContractProjection(contract, { pathNeutral: true }),
  ));
}

function benchmarkContractIntegrityError(contract, label) {
  if (!contract || typeof contract !== "object" || Array.isArray(contract)) {
    return `${label} benchmark contract is missing`;
  }
  const recomputed = benchmarkContractContentSha256(contract);
  return contract.compatibility_fingerprint === recomputed
    ? null
    : `${label} benchmark contract compatibility fingerprint does not match its contents`;
}

function benchmarkContractFingerprints(results) {
  return Object.fromEntries(
    [...results]
      .sort((left, right) => agentRunKey(left).localeCompare(agentRunKey(right)))
      .map((row) => [agentRunKey(row), benchmarkShardContractSha256(row.benchmark_contract)]),
  );
}

function benchmarkHostClassError(hostClass, label) {
  if (!hostClass || typeof hostClass !== "object" || Array.isArray(hostClass)) {
    return `${label} host class is missing`;
  }
  if (!String(hostClass.platform ?? "").trim() || !String(hostClass.arch ?? "").trim()) {
    return `${label} host class must name platform and architecture`;
  }
  if (!String(hostClass.cpu_model ?? "").trim()) {
    return `${label} host class must name the CPU model`;
  }
  if (!Number.isInteger(hostClass.logical_cpu_count) || hostClass.logical_cpu_count < 1) {
    return `${label} host class has an invalid logical CPU count`;
  }
  if (!Number.isInteger(hostClass.total_memory_bytes) || hostClass.total_memory_bytes < 1) {
    return `${label} host class has invalid total memory`;
  }
  const backend = String(hostClass.accelerator_backend ?? "").trim();
  const adapter = String(hostClass.accelerator_adapter ?? "").trim();
  if (!backend || !adapter) {
    return `${label} host class must name the accelerator backend and adapter`;
  }
  if (["llvmpipe", "lavapipe", "warp", "software rasterizer", "swiftshader", "microsoft basic render driver"]
    .some((token) => `${backend} ${adapter}`.toLowerCase().includes(token))) {
    return `${label} host class names a software accelerator`;
  }
  if (hostClass.embedding_policy !== "accelerated") {
    return `${label} host class embedding policy is not accelerated`;
  }
  if (!SHA256_PATTERN.test(String(hostClass.model_sha256 ?? ""))) {
    return `${label} host class model digest is missing or malformed`;
  }
  return null;
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

const EXACT_RESUME_CANDIDATE_IDENTITY_FIELDS = [
  "contract", "arm", "package_version", "cli_sha256", "schema_version",
  "protocol_revision", "discovery_contract_sha256", "plugin_manifest_sha256",
  "catalog_sha256",
];

function validateExactCandidateResumePrefixRows(rows, plannedRuns, opts) {
  if (!Array.isArray(rows) || rows.length === 0 || rows.length >= plannedRuns.length) {
    throw new Error("exact resume must contain a non-empty proper prefix");
  }
  const runsPerTask = opts.repeats * opts.arms.length;
  if (rows.length % runsPerTask !== 0) {
    throw new Error("exact resume prefix must end at a complete task boundary");
  }
  const currentPublished = exactCandidatePackageIdentity(
    opts.exactCandidatePackageByArm?.get("published_0_17_4"),
    "published_0_17_4",
  );
  const currentCandidate = exactCandidateSourceCliIdentity(
    opts.exactCandidatePackageByArm?.get("candidate_0_18"),
    "candidate_0_18",
  );
  for (const [index, row] of rows.entries()) {
    const planned = plannedRuns[index];
    if (agentRunKey(row) !== agentRunKey(planned) || !taskSnapshotMatches(planned.task, row)) {
      throw new Error(`exact resume row ${index + 1} is not the planned contiguous prefix`);
    }
    if (row.status !== "pass" || row.reanalysis_error) {
      throw new Error(`exact resume row ${agentRunKey(row)} is not a complete passing row`);
    }
    if (row.arm === "published_0_17_4") {
      if (stableJsonForHash(row.package_identity) !== stableJsonForHash(currentPublished)) {
        throw new Error("exact resume published package identity does not match the authenticated package");
      }
    } else if (row.arm === "candidate_0_18") {
      const previous = row.source_cli_identity;
      if (
        !previous || EXACT_RESUME_CANDIDATE_IDENTITY_FIELDS.some(
          (field) => previous[field] !== currentCandidate?.[field],
        )
      ) {
        throw new Error("exact resume candidate CLI or public contract identity changed");
      }
    } else if (row.package_identity != null || row.source_cli_identity != null) {
      throw new Error("exact resume baseline row contains a CodeStory identity");
    }
  }
  return rows.length / runsPerTask;
}

const EXACT_COMPARATOR_ARMS = new Set(["without_codestory", "published_0_17_4"]);
const EXACT_COMPARATOR_CONTRACT_KEYS = [
  "contract_version",
  "task_id",
  "task_manifest_hash",
  "scorer_hash",
  "runner",
  "model",
  "sandbox",
  "retrieval_contract",
  "retrieval_env",
  "packet_threshold_config",
];

function exactComparatorContractMismatch(current, previous) {
  const integrityError = benchmarkContractIntegrityError(previous, "comparator source row");
  if (integrityError) return integrityError;
  const mismatches = EXACT_COMPARATOR_CONTRACT_KEYS.filter(
    (key) => stableJsonForHash(current?.[key] ?? null) !== stableJsonForHash(previous?.[key] ?? null),
  );
  return mismatches.length
    ? `comparator benchmark contract differs in ${mismatches.join(", ")}`
    : null;
}

function validateExactCandidateComparatorPrefixRows(rows, plannedRuns, opts) {
  if (!Array.isArray(rows) || rows.length === 0 || rows.length > plannedRuns.length) {
    throw new Error("exact comparator source must contain a non-empty planned prefix");
  }
  const runsPerTask = opts.repeats * opts.arms.length;
  if (rows.length % runsPerTask !== 0) {
    throw new Error("exact comparator source must end at a complete task boundary with comparator triplets");
  }
  const currentPublished = exactCandidatePackageIdentity(
    opts.exactCandidatePackageByArm?.get("published_0_17_4"),
    "published_0_17_4",
  );
  for (const [index, row] of rows.entries()) {
    const planned = plannedRuns[index];
    if (agentRunKey(row) !== agentRunKey(planned) || !taskSnapshotMatches(planned.task, row)) {
      throw new Error(`exact comparator row ${index + 1} is not the planned contiguous prefix`);
    }
    if (row.status !== "pass" || row.reanalysis_error) {
      throw new Error(`exact comparator row ${agentRunKey(row)} is not a complete passing row`);
    }
    const contractMismatch = exactComparatorContractMismatch(
      benchmarkContractForRun(opts, planned),
      row.benchmark_contract,
    );
    if (contractMismatch) throw new Error(contractMismatch);
    if (row.arm === "published_0_17_4") {
      if (stableJsonForHash(row.package_identity) !== stableJsonForHash(currentPublished)) {
        throw new Error("exact comparator published package identity does not match the authenticated package");
      }
      if (row.source_cli_identity != null) {
        throw new Error("exact comparator published row contains a candidate source identity");
      }
    } else if (row.arm === "without_codestory") {
      if (row.package_identity != null || row.source_cli_identity != null) {
        throw new Error("exact comparator baseline row contains a CodeStory identity");
      }
    } else if (row.arm !== "candidate_0_18") {
      throw new Error(`exact comparator source contains unexpected arm ${row.arm}`);
    }
  }
  const completedTaskCount = rows.length / runsPerTask;
  const comparatorRows = rows.filter((row) => EXACT_COMPARATOR_ARMS.has(row.arm));
  if (comparatorRows.length !== completedTaskCount * opts.repeats * EXACT_COMPARATOR_ARMS.size) {
    throw new Error("exact comparator source does not contain complete comparator triplets");
  }
  if (comparatorRows.some((row) => row.arm === "candidate_0_18")) {
    throw new Error("exact comparator source attempted candidate-row reuse");
  }
  return { completedTaskCount, comparatorRows };
}

function validateExactComparatorLedgerSha256(bytes, expectedSha256) {
  const observed = sha256Bytes(bytes);
  if (observed !== normalizeExternalSha256(expectedSha256, "comparator ledger digest")) {
    throw new Error(`comparator ledger digest mismatch: expected ${expectedSha256}, observed ${observed}`);
  }
  return observed;
}

function exactComparatorArtifact(sourceRunDir, artifactPath) {
  if (!artifactPath) return null;
  if (!EXACT_COMPARATOR_ARTIFACT_NAME_PATTERN.test(path.basename(String(artifactPath)))) {
    throw new Error(`comparator artifact name is outside the closed artifact set: ${artifactPath}`);
  }
  const sourceRoot = realpathSync(sourceRunDir);
  const unresolved = path.isAbsolute(artifactPath)
    ? path.resolve(artifactPath)
    : path.resolve(sourceRoot, artifactPath);
  if (!isPathInside(sourceRoot, unresolved) || !existsSync(unresolved)) {
    throw new Error(`comparator artifact is missing or escapes its source run: ${artifactPath}`);
  }
  const source = realpathSync(unresolved);
  if (!isPathInside(sourceRoot, source) || !statSync(source).isFile()) {
    throw new Error(`comparator artifact is not a regular source-run file: ${artifactPath}`);
  }
  const relative = path.relative(sourceRoot, source);
  if (!relative || path.isAbsolute(relative) || relative.startsWith("..")) {
    throw new Error(`comparator artifact has an invalid source-run path: ${artifactPath}`);
  }
  return { source, relative };
}

async function exactComparatorArtifactDescriptors(sourceRunDir, artifactPaths) {
  const byRelativePath = new Map();
  for (const artifactPath of artifactPaths) {
    const artifact = exactComparatorArtifact(sourceRunDir, artifactPath);
    if (!artifact || byRelativePath.has(artifact.relative)) continue;
    const bytes = await readBoundedFile(
      artifact.source,
      MAX_REUSED_ARTIFACT_BYTES,
      `comparator artifact ${artifact.relative}`,
    );
    byRelativePath.set(artifact.relative, {
      ...artifact,
      bytes: bytes.bytes,
      sha256: bytes.sha256,
      byte_length: bytes.bytes.length,
    });
  }
  return [...byRelativePath.values()].sort((left, right) =>
    left.relative.localeCompare(right.relative)
  );
}

async function exactComparatorArtifactBundleSha256(sourceRunDir, artifactPaths) {
  const descriptors = await exactComparatorArtifactDescriptors(sourceRunDir, artifactPaths);
  return sha256Bytes(stableJsonForHash(descriptors.map((descriptor) => ({
    path: descriptor.relative,
    sha256: descriptor.sha256,
    byte_length: descriptor.byte_length,
  }))));
}

async function copyAuthenticatedComparatorArtifacts(
  sourceRunDir,
  outDir,
  artifactPaths,
  expectedBundleSha256,
) {
  const descriptors = await exactComparatorArtifactDescriptors(sourceRunDir, artifactPaths);
  const observedBundleSha256 = sha256Bytes(stableJsonForHash(descriptors.map((descriptor) => ({
    path: descriptor.relative,
    sha256: descriptor.sha256,
    byte_length: descriptor.byte_length,
  }))));
  const expected = normalizeExternalSha256(
    expectedBundleSha256,
    "comparator artifact bundle digest",
  );
  if (observedBundleSha256 !== expected) {
    throw new Error(
      `comparator artifact bundle digest mismatch: expected ${expected}, observed ${observedBundleSha256}`,
    );
  }
  const copied = new Map();
  for (const descriptor of descriptors) {
    const destination = path.resolve(outDir, descriptor.relative);
    assertPathInside(outDir, destination, "comparator artifact destination");
    await mkdir(path.dirname(destination), { recursive: true });
    await writeFile(destination, descriptor.bytes);
    const copiedBytes = await readBoundedFile(
      destination,
      MAX_REUSED_ARTIFACT_BYTES,
      `copied comparator artifact ${descriptor.relative}`,
    );
    if (copiedBytes.sha256 !== descriptor.sha256 || copiedBytes.bytes.length !== descriptor.byte_length) {
      throw new Error(`copied comparator artifact failed integrity validation: ${descriptor.relative}`);
    }
    copied.set(descriptor.relative, destination);
  }
  return copied;
}

function exactComparatorRowArtifactPaths(row) {
  return [
    row.stdout_path,
    row.stderr_path,
    row.baseline_harness_prelude?.context_path,
    row.baseline_harness_prelude?.stderr_path,
    row.codestory_harness_prelude?.stdout_path,
    row.codestory_harness_prelude?.stderr_path,
  ].filter(Boolean);
}

function copiedExactComparatorArtifactPath(sourceRunDir, copiedArtifacts, artifactPath) {
  if (!artifactPath) return artifactPath ?? null;
  const artifact = exactComparatorArtifact(sourceRunDir, artifactPath);
  const copied = copiedArtifacts.get(artifact.relative);
  if (!copied) throw new Error(`authenticated comparator artifact was not copied: ${artifact.relative}`);
  return copied;
}

function rewriteExactComparatorRowArtifacts(row, sourceRunDir, copiedArtifacts) {
  const rewritten = {
    ...row,
    stdout_path: copiedExactComparatorArtifactPath(sourceRunDir, copiedArtifacts, row.stdout_path),
    stderr_path: copiedExactComparatorArtifactPath(sourceRunDir, copiedArtifacts, row.stderr_path),
  };
  if (row.baseline_harness_prelude) {
    rewritten.baseline_harness_prelude = {
      ...row.baseline_harness_prelude,
      context_path: copiedExactComparatorArtifactPath(
        sourceRunDir,
        copiedArtifacts,
        row.baseline_harness_prelude.context_path,
      ),
      stderr_path: copiedExactComparatorArtifactPath(
        sourceRunDir,
        copiedArtifacts,
        row.baseline_harness_prelude.stderr_path,
      ),
    };
  }
  if (row.codestory_harness_prelude) {
    rewritten.codestory_harness_prelude = {
      ...row.codestory_harness_prelude,
      stdout_path: copiedExactComparatorArtifactPath(
        sourceRunDir,
        copiedArtifacts,
        row.codestory_harness_prelude.stdout_path,
      ),
      stderr_path: copiedExactComparatorArtifactPath(
        sourceRunDir,
        copiedArtifacts,
        row.codestory_harness_prelude.stderr_path,
      ),
    };
  }
  return rewritten;
}

async function loadExactCandidateComparatorReuse(opts, plannedRuns, outDir) {
  if (!opts.reuseComparatorsFrom) {
    return { rows: new Map(), completedTaskCount: 0, provenance: null };
  }
  const sourceRunDir = opts.reuseComparatorsFrom;
  const sourceLedgerPath = path.join(sourceRunDir, "runs.jsonl");
  if (!existsSync(sourceLedgerPath)) {
    throw new Error(`--reuse-comparators-from must contain runs.jsonl: ${sourceRunDir}`);
  }
  const ledger = await readBoundedFile(
    sourceLedgerPath,
    MAX_EXACT_COMPARATOR_LEDGER_BYTES,
    "comparator source ledger",
  );
  const sourceLedgerSha256 = validateExactComparatorLedgerSha256(
    ledger.bytes,
    opts.reuseComparatorsLedgerSha256,
  );
  const sourceRows = ledger.bytes.toString("utf8")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const validated = validateExactCandidateComparatorPrefixRows(sourceRows, plannedRuns, opts);
  const artifactPaths = validated.comparatorRows.flatMap(exactComparatorRowArtifactPaths);
  const copiedArtifacts = await copyAuthenticatedComparatorArtifacts(
    sourceRunDir,
    outDir,
    artifactPaths,
    opts.reuseComparatorsArtifactsSha256,
  );
  const taskCache = new Map();
  const reusable = new Map();
  for (const sourceRow of validated.comparatorRows) {
    const key = agentRunKey(sourceRow);
    const planned = plannedRuns.find((run) => agentRunKey(run) === key);
    if (!planned || !EXACT_COMPARATOR_ARMS.has(planned.arm)) {
      throw new Error(`comparator row cannot be mapped to a planned comparator: ${key}`);
    }
    const copied = rewriteExactComparatorRowArtifacts(sourceRow, sourceRunDir, copiedArtifacts);
    const reanalyzed = await recomputeRunAnalysis(copied, opts, outDir, taskCache);
    if (reanalyzed.reanalysis_error) {
      throw new Error(`comparator row reanalysis failed for ${key}: ${reanalyzed.reanalysis_error}`);
    }
    const currentContract = benchmarkContractForRun(opts, planned);
    const currentPublished = opts.exactCandidatePackageByArm.get("published_0_17_4");
    const result = {
      ...reanalyzed,
      ...(planned.arm === "published_0_17_4" ? {
        codestory_prelude_cli: currentPublished.cli_path,
      } : {}),
      benchmark_contract: currentContract,
      promotion_eligible: true,
      comparator_reuse_provenance: {
        contract: "codestory.agent-benchmark-exact-comparator-reuse/v1",
        source_run_dir: sourceRunDir,
        source_ledger_sha256: sourceLedgerSha256,
        source_artifacts_sha256: opts.reuseComparatorsArtifactsSha256,
        original_benchmark_run_id: sourceRow.benchmark_run_id ?? null,
        original_benchmark_contract: sourceRow.benchmark_contract,
        current_benchmark_contract: currentContract,
        original_identity: sourceRow.package_identity ?? null,
        authenticated_current_identity: planned.arm === "published_0_17_4"
          ? exactCandidatePackageIdentity(currentPublished, "published_0_17_4")
          : null,
        reanalyzed_with_current_scorer: true,
      },
    };
    reusable.set(key, {
      ...result,
      resource_accounting: resourceAccountingForResult(result),
    });
  }
  if ([...reusable.values()].some((row) => row.arm === "candidate_0_18")) {
    throw new Error("comparator reuse attempted to import a candidate row");
  }
  return {
    rows: reusable,
    completedTaskCount: validated.completedTaskCount,
    provenance: {
      contract: "codestory.agent-benchmark-exact-comparator-reuse/v1",
      source_run_dir: sourceRunDir,
      source_ledger_sha256: sourceLedgerSha256,
      source_artifacts_sha256: opts.reuseComparatorsArtifactsSha256,
      completed_task_count: validated.completedTaskCount,
      reused_row_count: reusable.size,
    },
  };
}

async function copyExactResumeRowArtifacts(row, sourceRunDir, outDir) {
  const runId = row.benchmark_run_id;
  const resumeArtifact = (artifactPath) => {
    if (!artifactPath || !path.isAbsolute(artifactPath)) return artifactPath;
    const absolute = path.resolve(artifactPath);
    if (!isPathInside(sourceRunDir, absolute)) {
      throw new Error(`exact resume artifact escapes its source run: ${artifactPath}`);
    }
    return path.relative(sourceRunDir, absolute);
  };
  const copied = {
    ...row,
    stdout_path: await copyResultArtifact(sourceRunDir, outDir, resumeArtifact(row.stdout_path), `${runId}.stdout.jsonl`),
    stderr_path: await copyResultArtifact(sourceRunDir, outDir, resumeArtifact(row.stderr_path), `${runId}.stderr.txt`),
  };
  if (copied.baseline_harness_prelude?.context_path) {
    copied.baseline_harness_prelude = {
      ...copied.baseline_harness_prelude,
      context_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        resumeArtifact(copied.baseline_harness_prelude.context_path),
        `${runId}.baseline-context.json`,
      ),
      stderr_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        resumeArtifact(copied.baseline_harness_prelude.stderr_path),
        `${runId}.baseline-context.stderr.txt`,
      ),
    };
  }
  if (copied.codestory_harness_prelude?.stdout_path) {
    copied.codestory_harness_prelude = {
      ...copied.codestory_harness_prelude,
      stdout_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        resumeArtifact(copied.codestory_harness_prelude.stdout_path),
        `${runId}.codestory-packet.stdout.json`,
      ),
      stderr_path: await copyResultArtifact(
        sourceRunDir,
        outDir,
        resumeArtifact(copied.codestory_harness_prelude.stderr_path),
        `${runId}.codestory-packet.stderr.txt`,
      ),
    };
  }
  return copied;
}

async function loadExactCandidateResumePrefix(opts, tasks, plannedRuns, outDir) {
  if (!opts.resumePrefixFrom) {
    return { rows: [], preparations: [], completedTaskCount: 0 };
  }
  const sourceRunDir = opts.resumePrefixFrom;
  const runsPath = path.join(sourceRunDir, "runs.jsonl");
  const preparationsPath = path.join(sourceRunDir, "preparations.jsonl");
  if (!existsSync(runsPath) || !existsSync(preparationsPath)) {
    throw new Error("--resume-prefix-from must contain runs.jsonl and preparations.jsonl");
  }
  const originalRows = await readJsonlRows(runsPath);
  const taskCache = new Map();
  const reanalyzed = [];
  for (const row of originalRows) {
    reanalyzed.push(await recomputeRunAnalysis(row, opts, sourceRunDir, taskCache));
  }
  const completedTaskCount = validateExactCandidateResumePrefixRows(
    reanalyzed,
    plannedRuns,
    opts,
  );
  const currentCandidate = exactCandidateSourceCliIdentity(
    opts.exactCandidatePackageByArm.get("candidate_0_18"),
    "candidate_0_18",
  );
  const copiedRows = [];
  for (const [index, row] of reanalyzed.entries()) {
    const planned = plannedRuns[index];
    const originalIdentity = row.source_cli_identity ?? row.package_identity ?? null;
    const copied = await copyExactResumeRowArtifacts(row, sourceRunDir, outDir);
    copiedRows.push({
      ...copied,
      ...(row.arm === "candidate_0_18" ? {
        source_cli_identity: currentCandidate,
        codestory_prelude_cli: opts.exactCandidatePackageByArm.get("candidate_0_18").cli_path,
      } : {}),
      benchmark_contract: benchmarkContractForRun(opts, planned),
      promotion_eligible: true,
      resume_provenance: {
        contract: "codestory.agent-benchmark-exact-prefix-resume/v1",
        source_run_dir: sourceRunDir,
        original_benchmark_run_id: row.benchmark_run_id,
        original_identity: originalIdentity,
        authenticated_current_identity:
          row.arm === "candidate_0_18"
            ? currentCandidate
            : row.arm === "published_0_17_4"
              ? exactCandidatePackageIdentity(
                  opts.exactCandidatePackageByArm.get("published_0_17_4"),
                  "published_0_17_4",
                )
              : null,
        artifact_cli_sha256: row.codestory_prelude_cli_sha256 ?? null,
        reanalyzed_with_current_scorer: true,
      },
    });
  }

  const completedRepos = tasks.slice(0, completedTaskCount).map((task) => task.repo);
  const preparationRows = (await readJsonlRows(preparationsPath))
    .filter((row) => row.kind === "preparation" && completedRepos.includes(row.repo));
  if (
    preparationRows.length !== completedRepos.length ||
    preparationRows.some((row, index) => row.repo !== completedRepos[index])
  ) {
    throw new Error("exact resume preparations do not match the completed task prefix");
  }
  const currentPublished = exactCandidatePackageIdentity(
    opts.exactCandidatePackageByArm.get("published_0_17_4"),
    "published_0_17_4",
  );
  const preparations = preparationRows.map((source) => {
    const { kind: _kind, recorded_at: originalRecordedAt, ...row } = source;
    const published = row.arm_preparations?.published_0_17_4;
    const candidate = row.arm_preparations?.candidate_0_18;
    if (
      stableJsonForHash(published?.package_identity) !== stableJsonForHash(currentPublished) ||
      EXACT_RESUME_CANDIDATE_IDENTITY_FIELDS.some(
        (field) => candidate?.source_cli_identity?.[field] !== currentCandidate?.[field],
      )
    ) {
      throw new Error(`exact resume preparation identity changed for ${row.repo}`);
    }
    return {
      ...row,
      source_cli_identity: currentCandidate,
      arm_preparations: {
        published_0_17_4: published,
        candidate_0_18: { ...candidate, source_cli_identity: currentCandidate },
      },
      resume_provenance: {
        contract: "codestory.agent-benchmark-exact-prefix-resume/v1",
        source_run_dir: sourceRunDir,
        original_recorded_at: originalRecordedAt,
        original_candidate_identity: candidate.source_cli_identity,
        authenticated_current_candidate_identity: currentCandidate,
      },
    };
  });
  for (const [index, row] of preparations.entries()) {
    for (const arm of ["published_0_17_4", "candidate_0_18"]) {
      const blockers = cachePreparationCanaryBlockers(
        row.arm_preparations[arm],
        selectedBenchmarkChildEnv(opts, arm),
      );
      if (blockers.length) {
        throw new Error(`exact resume ${arm} preparation is ineligible: ${blockers.join("; ")}`);
      }
    }
    opts.cachePreparationByRepo.set(row.repo, row);
    opts.exactCandidateLifecycle.preparation_order ??= [];
    opts.exactCandidateLifecycle.preparation_order.push({
      repo: row.repo,
      arms: exactCandidatePreparationArmOrder(index),
    });
  }
  return { rows: copiedRows, preparations, completedTaskCount };
}

async function runPlannedAgentRun(opts, run, outDir, reusableBaselines) {
  const reusable = reusableBaselines.get(agentRunKey(run));
  if (reusable) {
    const source = reusable.comparator_reuse_provenance?.source_run_dir ?? opts.reuseBaselineFrom;
    console.log(`reusing ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats} from ${source}`);
    return reusable;
  }
  console.log(`running ${run.repo} ${run.arm} repeat ${run.repeat}/${opts.repeats}`);
  return await runOne(opts, run, outDir);
}

function deterministicAgentRunFailure(result, opts) {
  const blockers = opts.publishable
    ? agentPublishableBlockers([result], opts)
    : result.status === "pass"
      ? []
      : [{ result, category: "product", reasons: [`status=${result.status}`] }];
  return blockers.length
    ? {
        benchmark_run_id: result.benchmark_run_id,
        repo: result.repo,
        task_id: result.task_id,
        arm: result.arm,
        repeat: result.repeat,
        blockers: blockers.map(({ category, reasons }) => ({ category, reasons })),
      }
    : null;
}

function plannedAgentRunExceptionFailure(run, error) {
  const reason = error instanceof Error ? error.message : String(error);
  const taskId = run.task?.id ?? run.task_id ?? null;
  return {
    kind: "run_exception",
    benchmark_run_id: benchmarkRunId([
      run.repo,
      ...(taskId ? [taskId] : []),
      run.arm,
      String(run.repeat).padStart(2, "0"),
    ]),
    repo: run.repo,
    task_id: taskId,
    arm: run.arm,
    repeat: run.repeat,
    error: reason,
    blockers: [{
      category: "harness-contract",
      reasons: [`agent run raised an exception: ${reason}`],
    }],
  };
}

async function runPlannedAgentRuns(
  opts,
  plannedRuns,
  reusableBaselines,
  outDir,
  options = {},
) {
  let stopScheduling = false;
  let firstFailure = null;
  const results = [];
  const executeRun = options.runOne ?? runPlannedAgentRun;
  const runOnePlanned = (run) => executeRun(
    { ...opts, signal: options.signal },
    options.decorateRun?.(run) ?? run,
    outDir,
    reusableBaselines,
  );
  const rememberFailure = async (failure, forceStop = false) => {
    if (firstFailure) {
      return;
    }
    firstFailure = failure;
    const shouldStop = forceStop || options.failFast;
    if (shouldStop) {
      stopScheduling = true;
    }
    try {
      await options.onFirstFailure?.(failure);
    } finally {
      if (shouldStop) {
        options.abortController?.abort(failure);
      }
    }
  };
  const record = async (result) => {
    await options.onResult?.(result);
    results.push(result);
    const failure = deterministicAgentRunFailure(result, opts);
    if (failure) {
      await rememberFailure(failure);
    }
  };
  const executeAndRecord = async (run) => {
    try {
      const result = await runOnePlanned(run);
      await record(result);
    } catch (error) {
      await rememberFailure(plannedAgentRunExceptionFailure(run, error), true);
    }
  };
  if (opts.jobs <= 1 || plannedRuns.length <= 1) {
    for (const run of plannedRuns) {
      if (stopScheduling || options.signal?.aborted || options.shouldSchedule?.(run) === false) {
        break;
      }
      await executeAndRecord(run);
    }
    return { results, firstFailure };
  }

  const groups = groupPlannedAgentRuns(plannedRuns);
  console.log(`running ${plannedRuns.length} planned agent rows across ${groups.length} repo groups with --jobs ${opts.jobs}`);
  await parallelMap(groups, opts.jobs, async (group) => {
    for (const run of group.runs) {
      if (stopScheduling || options.signal?.aborted || options.shouldSchedule?.(run) === false) {
        break;
      }
      await executeAndRecord(run);
    }
  });
  return { results, firstFailure };
}

function stableJsonForHash(value) {
  if (Array.isArray(value)) {
    return `[${value.map(stableJsonForHash).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJsonForHash(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

async function benchmarkShardAttestation(
  opts,
  allTasks,
  cachePreparation,
  results = [],
  dependencies = {},
) {
  const sourceCommit = dependencies.sourceCommit ??
    await gitOutput(["-C", repoRoot, "rev-parse", "HEAD"], repoRoot);
  const sourceTree = dependencies.sourceTree ??
    await gitOutput(["-C", repoRoot, "rev-parse", "HEAD^{tree}"], repoRoot);
  const trackedStatus = Object.hasOwn(dependencies, "trackedDirty")
    ? null
    : await gitOutput(
      ["-C", repoRoot, "status", "--porcelain=v1", "--untracked-files=no"],
      repoRoot,
    );
  if (!sourceCommit || !sourceTree || (!Object.hasOwn(dependencies, "trackedDirty") && trackedStatus == null)) {
    throw new Error("Unable to attest the benchmark source checkout");
  }
  const trackedDirty = dependencies.trackedDirty ?? Boolean(trackedStatus);
  if (trackedDirty) {
    throw new Error("Benchmark shard attestation requires a clean tracked source checkout");
  }
  let cliSha256 = dependencies.cliSha256 ?? null;
  if (!Object.hasOwn(dependencies, "cliSha256") && opts.arms.includes("with_codestory")) {
    const cli = resolveCodeStoryCli(opts);
    if (path.isAbsolute(cli) && existsSync(cli) && statSync(cli).isFile()) {
      cliSha256 = sha256Bytes(await readFile(cli));
    }
  }
  const manifestContract = allTasks.map((task) => {
    const { manifest_path: _manifestPath, ...snapshot } = taskSnapshotForResult(task);
    return snapshot;
  });
  const flags = {
    arms: opts.arms,
    repeats: opts.repeats,
    runner: opts.runner,
    model: opts.model,
    sandbox: opts.sandbox,
    jobs: opts.jobs,
    timeout_ms: opts.timeoutMs,
    task_suite: opts.taskSuite,
    max_source_reads_after_packet: opts.maxSourceReadsAfterPacket,
    publishable: opts.publishable,
    prepare_codestory_cache: Boolean(opts.prepareCodestoryCache),
    prepare_codestory_jobs: opts.prepareCodestoryJobs,
    prepare_codestory_timeout_ms: opts.prepareCodestoryTimeoutMs,
    packet_runtime: Boolean(opts.packetRuntime),
    packet_runtime_mode: opts.packetRuntimeMode ?? null,
    materialize_repos: Boolean(opts.materializeRepos),
    canary_task_id: opts.canaryTaskId ?? opts.manifestCanaryTaskId ?? null,
    diagnostic_extra_probes_from_manifest: Boolean(opts.diagnosticExtraProbesFromManifest),
    collect_all_failures: Boolean(opts.collectAllFailures),
    shard_count: opts.shardCount,
  };
  const rowContractEnvironmentDigests = new Set(
    results.map((row) => benchmarkContractEnvironmentSha256(row.benchmark_contract)),
  );
  if (rowContractEnvironmentDigests.size > 1) {
    throw new Error("Benchmark rows do not share one benchmark contract environment");
  }
  for (const row of results) {
    const integrityError = benchmarkContractIntegrityError(
      row.benchmark_contract,
      `row ${agentRunKey(row)}`,
    );
    if (integrityError) {
      throw new Error(integrityError);
    }
  }
  const benchmarkContractEnvironmentDigest = rowContractEnvironmentDigests.values().next().value ??
    benchmarkContractEnvironmentSha256(
      benchmarkContractForRun(opts, planAgentRuns(opts, allTasks)[0] ?? { task: null, arm: null }),
    );
  const contractFingerprints = benchmarkContractFingerprints(results);
  if (Object.values(contractFingerprints).some((fingerprint) => !fingerprint)) {
    throw new Error("Benchmark row is missing its compatibility fingerprint");
  }
  if (opts.prepareCodestoryCache || opts.publishable) {
    const ownedRepos = new Set(
      tasksForShard(allTasks, opts.shardCount, opts.shardIndex).map((task) => task.repo),
    );
    const preparedRepos = cachePreparation.map((row) => row?.repo ?? null);
    if (
      preparedRepos.length !== ownedRepos.size
      || new Set(preparedRepos).size !== preparedRepos.length
      || preparedRepos.some((repo) => !ownedRepos.has(repo))
      || [...ownedRepos].some((repo) => !preparedRepos.includes(repo))
    ) {
      throw new Error(
        "Benchmark preparation rows do not match the repositories owned by this shard",
      );
    }
  }
  const hostClass = benchmarkHostClass(cachePreparation);
  return {
    contract: "codestory.agent-benchmark-shard/v2",
    source_commit: sourceCommit,
    source_tree: sourceTree,
    tracked_dirty: false,
    cli_sha256: cliSha256,
    package_sha256: opts.candidatePackageSha256,
    manifest_sha256: sha256Bytes(stableJsonForHash(manifestContract)),
    flags_sha256: sha256Bytes(stableJsonForHash(flags)),
    benchmark_contract_environment_sha256: benchmarkContractEnvironmentDigest,
    benchmark_contract_fingerprints: contractFingerprints,
    benchmark_contract_rows_sha256: sha256Bytes(stableJsonForHash(contractFingerprints)),
    model_sha256: hostClass.model_sha256,
    host_class: hostClass,
  };
}

async function benchmarkShardAttestationForCloseout(
  opts,
  allTasks,
  cachePreparation,
  results,
  firstFailure,
  dependencies = {},
) {
  if (firstFailure) {
    return null;
  }
  return await benchmarkShardAttestation(
    opts,
    allTasks,
    cachePreparation,
    results,
    dependencies,
  );
}

async function readJsonlRows(filePath) {
  return (await readFile(filePath, "utf8"))
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

async function aggregateShardRuns(opts, allTasks) {
  if (!opts.aggregateShards?.length) {
    throw new Error("aggregateShardRuns requires at least one shard directory");
  }
  if (opts.publishable) {
    validatePublishableShape(opts, allTasks);
  }
  const shards = [];
  for (const directory of opts.aggregateShards) {
    const summaryPath = path.join(directory, "summary.json");
    const runsPath = path.join(directory, "runs.jsonl");
    if (!existsSync(summaryPath) || !existsSync(runsPath)) {
      throw new Error(`Shard is missing summary.json or runs.jsonl: ${directory}`);
    }
    shards.push({
      directory,
      summary: JSON.parse(await readFile(summaryPath, "utf8")),
      rows: await readJsonlRows(runsPath),
    });
  }
  const shardCount = shards[0].summary?.shard?.count;
  if (!Number.isInteger(shardCount) || shardCount < 1 || shards.length !== shardCount) {
    throw new Error(`Expected exactly ${shardCount ?? "unknown"} shard directories, found ${shards.length}`);
  }
  const indices = new Set();
  const attestationReference = shards[0].summary?.shard?.attestation;
  if (!attestationReference) {
    throw new Error("Shard summary is missing its attestation");
  }
  if (attestationReference.tracked_dirty !== false) {
    throw new Error("Shard attestation does not prove a clean tracked source checkout");
  }
  for (const field of [
    "contract",
    "source_commit",
    "source_tree",
    "cli_sha256",
    "package_sha256",
    "manifest_sha256",
    "model_sha256",
    "flags_sha256",
    "benchmark_contract_environment_sha256",
  ]) {
    if (!attestationReference[field]) {
      throw new Error(`Shard attestation is missing ${field}`);
    }
  }
  const commonAttestation = ({
    host_class: _hostClass,
    cli_sha256: _cliSha256,
    package_sha256: _packageSha256,
    benchmark_contract_fingerprints: _contractFingerprints,
    benchmark_contract_rows_sha256: _contractRowsSha256,
    ...common
  }) => common;
  const artifactAttestationByHostClass = new Map();
  for (const shard of shards) {
    const index = shard.summary?.shard?.index;
    if (
      shard.summary?.shard?.count !== shardCount ||
      !Number.isInteger(index) ||
      index < 0 ||
      index >= shardCount ||
      indices.has(index)
    ) {
      throw new Error(`Invalid or duplicate shard index: ${index}`);
    }
    indices.add(index);
    const attestation = shard.summary?.shard?.attestation;
    if (attestation?.contract !== "codestory.agent-benchmark-shard/v2") {
      throw new Error(`Shard ${index} attestation contract is not v2`);
    }
    if (attestation.tracked_dirty !== false) {
      throw new Error(`Shard ${index} attestation does not prove a clean tracked source checkout`);
    }
    const hostClassError = benchmarkHostClassError(attestation?.host_class, `shard ${index}`);
    if (hostClassError) {
      throw new Error(hostClassError);
    }
    if (attestation.host_class.model_sha256 !== attestation.model_sha256) {
      throw new Error(`Shard ${index} host-class model does not match its attested model`);
    }
    if (
      stableJsonForHash(commonAttestation(attestation ?? {})) !==
      stableJsonForHash(commonAttestation(attestationReference))
    ) {
      throw new Error(`Shard ${index} candidate or benchmark attestation does not match`);
    }
    const hostClassKey = stableJsonForHash(attestation.host_class);
    const artifactAttestation = {
      cli_sha256: attestation.cli_sha256 ?? null,
      package_sha256: attestation.package_sha256 ?? null,
    };
    if (
      artifactAttestationByHostClass.has(hostClassKey) &&
      stableJsonForHash(artifactAttestationByHostClass.get(hostClassKey)) !==
        stableJsonForHash(artifactAttestation)
    ) {
      throw new Error(`Shard ${index} platform artifacts do not match its host class`);
    }
    artifactAttestationByHostClass.set(hostClassKey, artifactAttestation);
    if (opts.publishable && (
      shard.summary?.publishable !== true ||
      shard.summary?.first_failure != null ||
      shard.summary?.comparative_failure != null ||
      shard.summary?.comparative_publishable !== true ||
      !Number.isInteger(shard.summary?.expected_rows) ||
      shard.summary.expected_rows !== shard.summary?.completed_rows ||
      shard.summary.completed_rows !== shard.rows.length
    )) {
      throw new Error(`Shard ${index} summary is not publishable and complete`);
    }
    if (shard.summary?.packet_obligation_accounting) {
      const error = packetObligationAccountingError(
        shard.summary.packet_obligation_accounting,
        `shard ${index} summary packet obligations`,
      );
      if (error) {
        throw new Error(error);
      }
    }
    const attestedFingerprints = attestation.benchmark_contract_fingerprints;
    if (
      !attestedFingerprints ||
      typeof attestedFingerprints !== "object" ||
      Array.isArray(attestedFingerprints) ||
      attestation.benchmark_contract_rows_sha256 !==
        sha256Bytes(stableJsonForHash(attestedFingerprints))
    ) {
      throw new Error(`Shard ${index} benchmark contract fingerprint attestation is invalid`);
    }
    const expectedFingerprintKeys = new Set(shard.rows.map(agentRunKey));
    if (
      Object.keys(attestedFingerprints).length !== expectedFingerprintKeys.size ||
      Object.keys(attestedFingerprints).some((key) => !expectedFingerprintKeys.has(key))
    ) {
      throw new Error(`Shard ${index} benchmark contract fingerprint rows do not match its ledger`);
    }
    for (const row of shard.rows) {
      if (taskShardIndex(row.task_id, shardCount) !== index) {
        throw new Error(`Row ${agentRunKey(row)} is recorded on the wrong shard`);
      }
      if (
        benchmarkContractEnvironmentSha256(row.benchmark_contract) !==
        attestation.benchmark_contract_environment_sha256
      ) {
        throw new Error(
          `Row ${agentRunKey(row)} benchmark contract environment does not match shard ${index} attestation`,
        );
      }
      const integrityError = benchmarkContractIntegrityError(
        row.benchmark_contract,
        `row ${agentRunKey(row)}`,
      );
      if (integrityError) {
        throw new Error(integrityError);
      }
      if (
        attestedFingerprints[agentRunKey(row)] !==
          benchmarkShardContractSha256(row.benchmark_contract)
      ) {
        throw new Error(
          `Row ${agentRunKey(row)} benchmark contract fingerprint does not match shard ${index} attestation`,
        );
      }
    }
  }
  const rows = shards.flatMap((shard) =>
    shard.rows.map((row) => ({
      ...row,
      host_class: shard.summary.shard.attestation.host_class,
      shard_index: shard.summary.shard.index,
    }))
  );
  const byKey = new Map();
  for (const row of rows) {
    const key = agentRunKey(row);
    if (byKey.has(key)) {
      throw new Error(`Duplicate benchmark row across shards: ${key}`);
    }
    byKey.set(key, row);
  }
  const expectedRuns = planAgentRuns(opts, allTasks);
  const expectedByKey = new Map(expectedRuns.map((run) => [agentRunKey(run), run]));
  for (const row of rows) {
    const planned = expectedByKey.get(agentRunKey(row));
    if (!planned) {
      throw new Error(`Unexpected benchmark row across shards: ${agentRunKey(row)}`);
    }
    const expectedContractFingerprint = benchmarkShardContractSha256(
      benchmarkContractForRun(opts, planned),
    );
    const observedContractFingerprint = benchmarkShardContractSha256(row.benchmark_contract);
    if (expectedContractFingerprint !== observedContractFingerprint) {
      throw new Error(
        `Shard row ${agentRunKey(row)} benchmark contract is incompatible`,
      );
    }
  }
  const missing = expectedRuns.filter((run) => !byKey.has(agentRunKey(run)));
  if (missing.length || rows.length !== expectedRuns.length) {
    throw new Error(
      `Shard aggregation is incomplete: expected ${expectedRuns.length}, found ${rows.length}, missing ${missing.length}`,
    );
  }
  const declaredCanary = opts.canaryTaskId ?? opts.manifestCanaryTaskId ?? null;
  if (
    declaredCanary &&
    opts.arms.includes("with_codestory") &&
    allTasks.some((task) => task.id === declaredCanary)
  ) {
    const canaryRows = rows.filter((row) => row.canary === true);
    const canaryRow = canaryRows[0] ?? null;
    const ownerShard = taskShardIndex(declaredCanary, shardCount);
    const effectiveCanarySummaries = shards.filter(
      (shard) => shard.summary?.effective_canary_task_id === declaredCanary,
    );
    if (
      canaryRows.length !== 1 ||
      canaryRow?.task_id !== declaredCanary ||
      canaryRow?.arm !== "with_codestory" ||
      canaryRow?.repeat !== 1 ||
      canaryRow?.shard_index !== ownerShard ||
      effectiveCanarySummaries.length !== 1 ||
      effectiveCanarySummaries[0]?.summary?.shard?.index !== ownerShard ||
      shards.some((shard) => shard.summary?.canary_task_id !== declaredCanary) ||
      shards.some((shard) =>
        shard.summary?.shard?.index !== ownerShard &&
        shard.summary?.effective_canary_task_id != null
      )
    ) {
      throw new Error(
        `Declared canary '${declaredCanary}' must appear exactly once across shard rows and summaries`,
      );
    }
  }
  const canonicalRows = sortAgentResultsCanonical(rows, allTasks, opts.arms);
  if (opts.publishable) {
    const blockers = agentPublishableBlockers(canonicalRows, opts);
    if (blockers.length) {
      throw new Error(
        `Publishable shard rows failed: ${blockers.map(formatAgentPublishableBlocker).join(" | ")}`,
      );
    }
  }
  const pooledSummary = summarizeRuns(canonicalRows);
  const obligationAccounting = summarizePacketObligationAccounting(
    canonicalRows,
    "shard aggregation",
  );
  const hostClasses = new Set(
    shards.map((shard) => stableJsonForHash(shard.summary.shard.attestation.host_class)),
  );
  const latencySummariesByHostClass = [...hostClasses]
    .sort()
    .map((hostClassKey) => ({
      host_class: JSON.parse(hostClassKey),
      platform_artifacts: artifactAttestationByHostClass.get(hostClassKey),
      summary: summarizeRuns(
        canonicalRows.filter((row) => stableJsonForHash(row.host_class) === hostClassKey),
      ),
      cost_accounting: summarizeCostAccounting(
        canonicalRows.filter((row) => stableJsonForHash(row.host_class) === hostClassKey),
      ),
    }));
  const latencyPoolingEligible = hostClasses.size === 1;
  const summary = latencyPoolingEligible
    ? pooledSummary
    : withoutPooledLatency(pooledSummary);
  const outDir = path.resolve(
    opts.outDir ?? path.join(repoRoot, "target", "agent-benchmark", `aggregate-${Date.now()}`),
  );
  await mkdir(outDir, { recursive: true });
  await writeJsonlRows(path.join(outDir, "runs.jsonl"), canonicalRows);
  await writeFile(
    path.join(outDir, "summary.json"),
    `${JSON.stringify(
      {
        generated_at: new Date().toISOString(),
        aggregate: true,
        shard_count: shardCount,
        source_attestation: commonAttestation(attestationReference),
        platform_artifacts_by_host_class: latencySummariesByHostClass.map((entry) => ({
          host_class: entry.host_class,
          platform_artifacts: entry.platform_artifacts,
        })),
        latency_pooling_eligible: latencyPoolingEligible,
        latency_host_classes: latencySummariesByHostClass.map((entry) => entry.host_class),
        latency_summaries_by_host_class: latencySummariesByHostClass,
        pooled_latency_summary: latencyPoolingEligible ? pooledSummary : null,
        expected_rows: expectedRuns.length,
        completed_rows: canonicalRows.length,
        packet_obligation_accounting: obligationAccounting,
        summary,
        cost_accounting: latencyPoolingEligible ? summarizeCostAccounting(canonicalRows) : null,
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
  console.log(`wrote ${outDir}`);
}

function pipelineStageFailure(stage, group, error) {
  const reason = error instanceof Error ? error.message : String(error);
  return {
    kind: `${stage}_failed`,
    repo: group?.repo ?? null,
    task_id: group?.tasks?.[0]?.id ?? null,
    error: reason,
    blockers: [{
      category: stage === "preparation" ? "environment" : "harness-contract",
      reasons: [reason],
    }],
  };
}

async function runExactCandidatePipeline({
  opts,
  tasks,
  plannedRuns,
  executeRun,
  outDir,
  materializeGroup,
  prepareGroup,
  prepareIsolation,
  recordResult,
  recordPreparation,
  recordPreparationState,
  recordFirstFailure,
  reusableBaselines,
}) {
  const cachePreparation = [];
  opts.cachePreparationByRepo ??= new Map();
  const groups = [...groupTasksByRepo(tasks)].map(([repo, repoTasks]) => ({ repo, tasks: repoTasks }));
  let firstFailure = null;
  let preparationIdentityReference = null;
  for (const group of groups) {
    try {
      await materializeGroup(group, null);
      await recordPreparationState({ kind: "materialized", repo: group.repo });
      const rows = await prepareGroup(group, null);
      if (!Array.isArray(rows) || rows.length !== 1 || rows[0]?.repo !== group.repo) {
        throw new Error(`preparation must return exactly one row for ${group.repo}`);
      }
      const row = rows[0];
      for (const arm of ["published_0_17_4", "candidate_0_18"]) {
        const preparation = row.arm_preparations?.[arm];
        const blockers = cachePreparationCanaryBlockers(
          preparation,
          selectedBenchmarkChildEnv(opts, arm),
        );
        if (blockers.length) {
          throw new Error(`${arm} preparation is not exact-candidate eligible: ${blockers.join("; ")}`);
        }
      }
      const identityBlockers = preparationIdentityReference
        ? cachePreparationIdentityBlockers(preparationIdentityReference, row)
        : [];
      if (identityBlockers.length) {
        throw new Error(
          `exact-candidate retrieval preparation identity changed: ${identityBlockers.join("; ")}`,
        );
      }
      await recordPreparation(row);
      cachePreparation.push(row);
      opts.cachePreparationByRepo.set(row.repo, row);
      preparationIdentityReference ??= row;
      await recordPreparationState({ kind: "prepared", repo: group.repo });
    } catch (error) {
      firstFailure = pipelineStageFailure("preparation", group, error);
      await recordFirstFailure(firstFailure);
      return {
        results: [],
        firstFailure,
        comparativeFailure: null,
        comparativePublishable: false,
        cachePreparation,
        agentCodexIsolation: null,
        aborted: true,
      };
    }
  }
  let agentCodexIsolation;
  try {
    agentCodexIsolation = await prepareIsolation();
  } catch (error) {
    firstFailure = pipelineStageFailure("agent_isolation", null, error);
    await recordFirstFailure(firstFailure);
    return {
      results: [],
      firstFailure,
      comparativeFailure: null,
      comparativePublishable: false,
      cachePreparation,
      agentCodexIsolation: null,
      aborted: true,
    };
  }
  const outcome = await runPlannedAgentRuns(
    { ...opts, jobs: 1 },
    plannedRuns,
    reusableBaselines,
    outDir,
    {
      runOne: executeRun,
      failFast: false,
      onResult: recordResult,
      onFirstFailure: async (failure) => {
        firstFailure ??= failure;
        if (firstFailure === failure) await recordFirstFailure(failure);
      },
    },
  );
  firstFailure ??= outcome.firstFailure;
  return {
    results: outcome.results,
    firstFailure,
    comparativeFailure: null,
    comparativePublishable: firstFailure == null,
    cachePreparation,
    agentCodexIsolation,
    aborted: false,
  };
}

async function runAgentBenchmarkPipeline({
  opts,
  tasks,
  plannedRuns,
  executeRun = runPlannedAgentRun,
  reusableBaselines = new Map(),
  outDir = null,
  materializeGroup = async () => {},
  prepareGroup = async () => [],
  prepareIsolation = async () => null,
  recordResult = async () => {},
  recordPreparation = async () => {},
  recordPreparationState = async () => {},
  recordFirstFailure = async () => {},
  recordComparativeFailure = async () => {},
}) {
  if (opts.exactCandidate) {
    return await runExactCandidatePipeline({
      opts,
      tasks,
      plannedRuns,
      executeRun,
      outDir,
      materializeGroup,
      prepareGroup,
      prepareIsolation,
      recordResult,
      recordPreparation,
      recordPreparationState,
      recordFirstFailure,
      reusableBaselines,
    });
  }
  const results = [];
  const cachePreparation = [];
  const abortController = new AbortController();
  const comparativeAbortController = new AbortController();
  opts.cachePreparationByRepo ??= new Map();
  let firstFailure = null;
  let comparativeFailure = null;
  let agentCodexIsolation = null;
  let preparationIdentityReference = null;
  let stopScheduling = false;

  const rememberFirstFailure = async (failure, abort = false) => {
    if (firstFailure) {
      return;
    }
    firstFailure = { recorded_at: new Date().toISOString(), ...failure };
    if (abort) {
      stopScheduling = true;
      abortController.abort(firstFailure);
    }
    await recordFirstFailure(firstFailure);
  };
  const acceptPreparation = (row) => {
    cachePreparation.push(row);
    opts.cachePreparationByRepo.set(row.repo, row);
  };
  const rememberComparativeFailure = async (failure) => {
    if (comparativeFailure) {
      return;
    }
    comparativeFailure = {
      recorded_at: new Date().toISOString(),
      kind: "comparative_baseline_failure",
      ...failure,
    };
    await recordComparativeFailure(comparativeFailure);
    comparativeAbortController.abort(comparativeFailure);
  };
  const runBatch = async (batchOpts, runs, options = {}) => {
    const batchSignal = options.comparativeOnly
      ? AbortSignal.any([abortController.signal, comparativeAbortController.signal])
      : abortController.signal;
    if (!runs.length || stopScheduling || batchSignal.aborted) {
      return { results: [], firstFailure: null };
    }
    return await runPlannedAgentRuns(
      { ...batchOpts, signal: batchSignal },
      runs,
      reusableBaselines,
      outDir,
      {
        ...options,
        runOne: executeRun,
        signal: batchSignal,
        abortController: options.comparativeOnly
          ? comparativeAbortController
          : abortController,
        onResult: async (result) => {
          await recordResult(result);
          await options.onResult?.(result);
          results.push(result);
        },
        onFirstFailure: async (failure) => {
          if (options.comparativeOnly && failure.kind !== "run_exception") {
            await rememberComparativeFailure(failure);
          } else {
            await rememberFirstFailure(
              failure,
              !options.comparativeOnly && options.failFast === true,
            );
          }
          await options.onFirstFailure?.(failure);
        },
      },
    );
  };
  const prepareTaskGroup = async (group, canary = false) => {
    if (!group || abortController.signal.aborted) {
      return;
    }
    try {
      await materializeGroup(group, abortController.signal);
      await recordPreparationState({ kind: "materialized", repo: group.repo });
      if (stopScheduling || abortController.signal.aborted) return;
    } catch (error) {
      await rememberFirstFailure(pipelineStageFailure("materialization", group, error), true);
      await recordPreparationState({ kind: "materialization_failed", repo: group.repo });
      return;
    }
    try {
      const prepared = await prepareGroup(group, abortController.signal);
      const requirePreparationRow = canary || opts.prepareCodestoryCache || opts.publishable;
      const preparationContractInvalid = !Array.isArray(prepared)
        || prepared.length > 1
        || (prepared.length === 1 && prepared[0]?.repo !== group.repo)
        || (requirePreparationRow && prepared.length !== 1);
      if (preparationContractInvalid) {
        const failure = {
          kind: "preparation_contract_failed",
          repo: group.repo,
          task_id: group.tasks[0]?.id ?? null,
          blockers: [{
            category: "harness-contract",
            reasons: [
              `preparation must return exactly one row for ${group.repo}; received `
              + `${Array.isArray(prepared) ? prepared.length : "non-array"}`,
            ],
          }],
        };
        for (const row of Array.isArray(prepared) ? prepared : []) {
          await recordPreparation(row);
        }
        await rememberFirstFailure(failure, true);
        await recordPreparationState({
          kind: "preparation_contract_failed",
          repo: group.repo,
          returned_repos: Array.isArray(prepared)
            ? prepared.map((row) => row?.repo ?? null)
            : null,
        });
        return;
      }
      for (const row of prepared) {
        await recordPreparation(row);
        if (stopScheduling || firstFailure) return;
        const eligibilityBlockers = cachePreparationCanaryBlockers(
          row,
          selectedBenchmarkChildEnv(opts),
        );
        const identityBlockers = preparationIdentityReference
          ? cachePreparationIdentityBlockers(preparationIdentityReference, row)
          : [];
        const blockers = [...eligibilityBlockers, ...identityBlockers];
        if (blockers.length) {
          await rememberFirstFailure({
            kind: identityBlockers.length
              ? "preparation_identity_mismatch"
              : (canary ? "canary_preparation" : "preparation_eligibility"),
            repo: group.repo,
            task_id: group.tasks[0]?.id ?? null,
            blockers: [{ category: "environment", reasons: blockers }],
          }, true);
          await recordPreparationState({
            kind: eligibilityBlockers.length
              ? "preparation_eligibility_failed"
              : "preparation_identity_failed",
            repo: group.repo,
            blockers,
          });
          return;
        }
        preparationIdentityReference ??= row;
        acceptPreparation(row);
      }
      await recordPreparationState({ kind: "prepared", repo: group.repo });
    } catch (error) {
      if (error?.preparation) {
        await recordPreparation(error.preparation);
      }
      await rememberFirstFailure(pipelineStageFailure("preparation", group, error), true);
      await recordPreparationState({ kind: "preparation_failed", repo: group.repo });
    }
  };

  const hasCodeStoryArm = opts.arms.includes("with_codestory");
  const declaredCanary = opts.canaryTaskId ?? opts.manifestCanaryTaskId ?? null;
  const canaryTask = hasCodeStoryArm && declaredCanary
    ? tasks.find((task) => task.id === declaredCanary) ?? null
    : null;
  const taskGroups = [...groupTasksByRepo(tasks)].map(([repo, repoTasks]) => ({
    repo,
    tasks: repoTasks,
  }));
  const canaryGroup = canaryTask
    ? taskGroups.find((group) => group.repo === canaryTask.repo) ?? null
    : null;

  if (canaryGroup) {
    await prepareTaskGroup(canaryGroup, true);
  }
  if (!firstFailure && !stopScheduling) {
    try {
      agentCodexIsolation = await prepareIsolation();
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      await rememberFirstFailure({
        kind: "agent_isolation_failed",
        repo: canaryGroup?.repo ?? null,
        task_id: canaryTask?.id ?? null,
        error: reason,
        blockers: [{ category: "environment", reasons: [reason] }],
      }, true);
      await recordPreparationState({
        kind: "agent_isolation_failed",
        repo: canaryGroup?.repo ?? null,
        error: reason,
      });
    }
  }
  if (!firstFailure && !stopScheduling && canaryTask) {
    const canaryRun = plannedRuns.find(
      (run) => run.task?.id === canaryTask.id &&
        run.arm === "with_codestory" &&
        run.repeat === 1,
    );
    if (!canaryRun) {
      throw new Error(`Canary task '${canaryTask.id}' has no with_codestory repeat-1 row`);
    }
    await runBatch(
      { ...opts, jobs: 1 },
      [{ ...canaryRun, canary: true }],
      { failFast: true },
    );
  }

  if (!firstFailure && !stopScheduling) {
    const completedKeys = new Set(results.map(agentRunKey));
    const baselineRuns = plannedRuns.filter(
      (run) => run.arm === "without_codestory" && !completedKeys.has(agentRunKey(run)),
    );
    const otherGroups = canaryGroup
      ? taskGroups.filter((group) => group.repo !== canaryGroup.repo)
      : taskGroups;
    const readyBaselines = createAsyncQueue();
    const preparationState = { drained: !otherGroups.length };
    if (canaryGroup) {
      const runs = baselineRuns.filter((run) => run.repo === canaryGroup.repo);
      if (runs.length) {
        readyBaselines.push({ repo: canaryGroup.repo, runs });
      }
    }
    const baselineWorkers = Promise.all(
      Array.from({ length: Math.min(4, Math.max(1, taskGroups.length)) }, async () => {
        for (;;) {
          const group = await readyBaselines.shift();
          if (
            !group ||
            preparationState.drained ||
            stopScheduling ||
            abortController.signal.aborted ||
            comparativeAbortController.signal.aborted
          ) {
            return;
          }
          await runBatch(
            { ...opts, jobs: 1 },
            group.runs,
            {
              failFast: true,
              comparativeOnly: true,
              shouldSchedule: () => !preparationState.drained,
              decorateRun: (run) => ({
                ...run,
                preparation_overlap: true,
                comparative_wall_time_eligible: false,
              }),
            },
          );
        }
      }),
    );

    try {
      await parallelMap(
        otherGroups,
        Math.min(2, Math.max(1, opts.prepareCodestoryJobs ?? 2)),
        async (group) => {
          if (stopScheduling || abortController.signal.aborted) {
            return;
          }
          await prepareTaskGroup(group, false);
          if (!stopScheduling && !abortController.signal.aborted && !comparativeAbortController.signal.aborted) {
            const runs = baselineRuns.filter((run) => run.repo === group.repo);
            if (runs.length) {
              readyBaselines.push({ repo: group.repo, runs });
            }
          }
        },
      );
    } finally {
      preparationState.drained = true;
      await recordPreparationState({ kind: "drained", repos: otherGroups.map((group) => group.repo) });
      readyBaselines.close();
      await baselineWorkers;
    }

    if (!stopScheduling && !abortController.signal.aborted) {
      const completedAfterOverlap = new Set(results.map(agentRunKey));
      const codeStoryRuns = plannedRuns.filter(
        (run) => run.arm === "with_codestory" &&
          !completedAfterOverlap.has(agentRunKey(run)),
      );
      const codeStoryOutcome = await runBatch(
        { ...opts, jobs: Math.min(4, opts.jobs) },
        codeStoryRuns,
        { failFast: opts.publishable && !opts.collectAllFailures },
      );
      if (
        !codeStoryOutcome.firstFailure &&
        !abortController.signal.aborted &&
        !comparativeFailure
      ) {
        const completedAfterCodeStory = new Set(results.map(agentRunKey));
        const remainingBaselines = plannedRuns.filter(
          (run) => run.arm === "without_codestory" &&
            !completedAfterCodeStory.has(agentRunKey(run)),
        );
        await runBatch(
          { ...opts, jobs: opts.jobs },
          remainingBaselines,
          {
            failFast: true,
            comparativeOnly: true,
            decorateRun: (run) => ({
              ...run,
              preparation_overlap: false,
              comparative_wall_time_eligible: true,
            }),
          },
        );
      }
    }
  }

  return {
    results,
    firstFailure,
    comparativeFailure,
    comparativePublishable: comparativeFailure == null,
    cachePreparation,
    agentCodexIsolation,
    aborted: abortController.signal.aborted,
  };
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
  const allTasks = await loadTasks(opts);
  validateExactCandidateShape(opts, allTasks);
  opts.exactCandidateCostRates = opts.exactCandidate
    ? exactCandidateCostRates(process.env)
    : null;
  opts.releaseEvidenceCorpusContract = await loadReleaseEvidenceCorpusContract(allTasks, opts);
  if (opts.aggregateShards) {
    await aggregateShardRuns(opts, allTasks);
    return;
  }
  let tasks = allTasks;
  if (tasks.length && opts.shardCount > 1) {
    tasks = tasksForShard(tasks, opts.shardCount, opts.shardIndex);
    if (!tasks.length) {
      throw new Error(`Shard ${opts.shardIndex}/${opts.shardCount} owns no selected tasks`);
    }
  }
  if (opts.publishable) {
    validatePublishableShape(opts, allTasks);
  }
  if (opts.list) {
    if (opts.materializeRepos) {
      assertManifestRepoMaterializationAllowed(tasks, opts);
      await materializeRepos(tasks, opts);
    }
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
    if (opts.materializeRepos) {
      assertManifestRepoMaterializationAllowed(tasks, opts);
      await materializeRepos(tasks, opts);
    }
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
  const runsPath = path.join(outDir, "runs.jsonl");
  if (existsSync(runsPath)) {
    throw new Error(`Refusing to append a new benchmark to an existing ledger: ${runsPath}`);
  }
  const ledger = await createDurableJsonlAppender(runsPath);
  const preparationLedger = await createDurableJsonlAppender(
    path.join(outDir, "preparations.jsonl"),
  );
  const reusableBaselines = await loadReusableBaselines(opts, plannedRuns, outDir);
  opts.cachePreparationByRepo = new Map();
  let resumedPrefix = { rows: [], preparations: [], completedTaskCount: 0 };
  let comparatorReuse = { rows: new Map(), completedTaskCount: 0, provenance: null };
  let agentCodexIsolation = null;
  let pipeline = null;
  let pipelineError = null;
  let finalizationFailures = [];
  try {
    if (opts.exactCandidate) {
      await initializeExactCandidateState(opts);
      opts.exactCandidateLifecycle.cost_rates = opts.exactCandidateCostRates;
      resumedPrefix = await loadExactCandidateResumePrefix(
        opts,
        tasks,
        plannedRuns,
        outDir,
      );
      comparatorReuse = await loadExactCandidateComparatorReuse(
        opts,
        plannedRuns,
        outDir,
      );
      for (const row of resumedPrefix.rows) {
        await ledger.append(row);
      }
      for (const row of resumedPrefix.preparations) {
        await preparationLedger.append({
          kind: "preparation",
          recorded_at: new Date().toISOString(),
          ...row,
        });
      }
    }
    pipeline = await runAgentBenchmarkPipeline({
      opts,
      tasks: tasks.slice(resumedPrefix.completedTaskCount),
      plannedRuns: plannedRuns.slice(resumedPrefix.rows.length),
      reusableBaselines: comparatorReuse.rows.size ? comparatorReuse.rows : reusableBaselines,
      outDir,
      materializeGroup: async (group, signal) => {
        if (opts.materializeRepos) {
          assertManifestRepoMaterializationAllowed(group.tasks, opts);
          await materializeRepos(group.tasks, { ...opts, signal });
        }
      },
      prepareGroup: async (group, signal) => opts.prepareCodestoryCache
        ? await prepareCodeStoryCaches(
          { ...opts, signal, prepareCodestoryJobs: 1 },
          group.tasks,
        )
        : [],
      prepareIsolation: async () => {
        agentCodexIsolation =
          opts.runner === "codex" ? await prepareAgentCodexIsolation(outDir, opts) : null;
        opts.agentCodexHomes = agentCodexIsolation?.homes ?? null;
        return agentCodexIsolation;
      },
      recordResult: (result) => ledger.append(result),
      recordPreparation: (row) => preparationLedger.append({
        kind: "preparation",
        recorded_at: new Date().toISOString(),
        ...row,
      }),
      recordPreparationState: (state) => preparationLedger.append({
        recorded_at: new Date().toISOString(),
        ...state,
      }),
      recordFirstFailure: (failure) => writeFile(
        path.join(outDir, "first-failure.json"),
        `${JSON.stringify(failure, null, 2)}\n`,
        "utf8",
      ),
      recordComparativeFailure: (failure) => writeFile(
        path.join(outDir, "comparative-failure.json"),
        `${JSON.stringify(failure, null, 2)}\n`,
        "utf8",
      ),
    });
    pipeline.results = [...resumedPrefix.rows, ...pipeline.results];
    pipeline.cachePreparation = [
      ...resumedPrefix.preparations,
      ...pipeline.cachePreparation,
    ];
  } catch (error) {
    pipelineError = error;
  } finally {
    finalizationFailures = await finalizeBenchmarkResources(
      opts,
      ledger,
      preparationLedger,
    );
  }
  if (finalizationFailures.length) {
    try {
      await writeFile(
        path.join(outDir, "cleanup-failure.json"),
        `${JSON.stringify({ failures: finalizationFailures }, null, 2)}\n`,
        "utf8",
      );
    } catch (error) {
      finalizationFailures.push({
        resource: "cleanup_failure_receipt",
        code: typeof error?.code === "string" ? error.code : null,
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }
  if (pipelineError) {
    throw pipelineError;
  }

  const {
    results,
    firstFailure: pipelineFirstFailure,
    comparativeFailure,
    comparativePublishable,
    cachePreparation,
  } = pipeline;
  const firstFailure = finalBenchmarkFailure(pipelineFirstFailure, finalizationFailures);
  if (!pipelineFirstFailure && firstFailure) {
    await writeFile(
      path.join(outDir, "first-failure.json"),
      `${JSON.stringify(firstFailure, null, 2)}\n`,
      "utf8",
    );
  }

  const canonicalResults = sortAgentResultsCanonical(results, tasks, opts.arms);
  if (cachePreparation.length) {
    await writeFile(
      path.join(outDir, "codestory-cache-preparation.json"),
      `${JSON.stringify(cachePreparation, null, 2)}\n`,
      "utf8",
    );
  }

  const summary = summarizeRuns(canonicalResults);
  const obligationAccounting = summarizePacketObligationAccounting(
    canonicalResults,
    "agent benchmark report",
  );
  const costAccounting = summarizeCostAccounting(canonicalResults);
  const shardAttestation = await benchmarkShardAttestationForCloseout(
    opts,
    allTasks,
    cachePreparation,
    canonicalResults,
    firstFailure,
  );
  const summaryPayload = {
    generated_at: new Date().toISOString(),
    runner: opts.runner,
    model: opts.model,
    repos: opts.repos ?? [...new Set(tasks.map((task) => task.repo))],
    arms: opts.arms,
    task_suite: opts.taskSuite,
    task_ids: opts.taskIds,
    task_manifest: opts.taskManifest,
    canary_task_id: opts.canaryTaskId ?? opts.manifestCanaryTaskId ?? null,
    effective_canary_task_id:
      canonicalResults.find((row) => row.canary === true)?.task_id ?? null,
    shard: {
      count: opts.shardCount,
      index: opts.shardIndex,
      attestation: shardAttestation,
    },
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
    reused_baseline_runs: canonicalResults.filter((row) => row.reused_from).length,
    resume_prefix_from: opts.resumePrefixFrom,
    resumed_prefix_runs: canonicalResults.filter((row) => row.resume_provenance).length,
    reuse_comparators_from: opts.reuseComparatorsFrom,
    reused_comparator_runs: canonicalResults.filter((row) => row.comparator_reuse_provenance).length,
    comparator_reuse: comparatorReuse.provenance,
    allow_failures: opts.allowFailures,
    timeout_ms: opts.timeoutMs,
    sandbox: opts.sandbox,
    output_dir: outDir,
    retrieval_env: retrievalEnv(),
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(process.env)),
    codex_agent_isolation: agentCodexIsolation?.receipt ?? null,
    host_class: benchmarkHostClass(cachePreparation),
    expected_rows: plannedRuns.length,
    completed_rows: canonicalResults.length,
    first_failure: firstFailure,
    comparative_failure: comparativeFailure,
    comparative_publishable: comparativePublishable,
    packet_obligation_accounting: obligationAccounting,
    summary,
    cost_accounting: costAccounting,
    cost_rates: opts.exactCandidateCostRates,
    exact_candidate_acceptance: opts.exactCandidate
      ? exactCandidateAcceptance(canonicalResults, opts.exactCandidateLifecycle)
      : null,
    exact_candidate_lifecycle: opts.exactCandidate ? opts.exactCandidateLifecycle : null,
    finalization_failures: finalizationFailures,
  };
  await writeFile(path.join(outDir, "summary.json"), `${JSON.stringify(summaryPayload, null, 2)}\n`, "utf8");
  await writeFile(path.join(outDir, "summary.md"), markdownSummary(summary, opts, costAccounting), "utf8");

  const failedRuns = canonicalResults.filter((result) => result.status !== "pass");
  let exitCode = firstFailure ? 1 : 0;
  if (opts.exactCandidate && !summaryPayload.exact_candidate_acceptance.pass) {
    console.error(
      `exact-candidate acceptance failed: ${summaryPayload.exact_candidate_acceptance.reasons.join(" | ")}`,
    );
    exitCode = 1;
  }
  if (failedRuns.length && !opts.allowFailures) {
    console.error("benchmark failed: every run must pass unless --allow-failures is set.");
    for (const failed of failedRuns) {
      console.error(`  ${failed.repo} ${failed.arm} repeat ${failed.repeat}: status=${failed.status} exit=${failed.exit_code} signal=${failed.signal ?? ""}`);
    }
    exitCode = 1;
  }

  if (opts.publishable) {
    const blockers = agentPublishableBlockers(canonicalResults, opts);
    const completedKeys = new Set(canonicalResults.map(agentRunKey));
    const missingRuns = plannedRuns.filter((run) => !completedKeys.has(agentRunKey(run)));
    if (missingRuns.length) {
      blockers.push({
        result: { repo: "benchmark", arm: "all", repeat: 0, task_id: null },
        category: "harness-contract",
        reasons: [`missing completed rows=${missingRuns.length}; expected ${plannedRuns.length}`],
      });
    }
    if (blockers.length) {
      console.error("--publishable failed: every run must pass, report total token usage, pass preludes without warnings, pass manifest quality gates when present, obey packet dispositions, prove exact runtime identity when MCP is used, and stay within the post-packet source-read budget.");
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
  aggregateShardRuns,
  agentRunnerEnv,
  analyzeTranscript,
  agentPublishableBlockers,
  assertSafeWindowsCmdArgs,
  benchmarkRunId,
  benchmarkContractEnvironmentSha256,
  benchmarkContractForRun,
  benchmarkHostClass,
  benchmarkShardAttestation,
  benchmarkShardAttestationForCloseout,
  baselineSearchPreludeStatus,
  buildPacketQualityDeltas,
  buildQualityDebugPayload,
  copyResultArtifact,
  copyAuthenticatedComparatorArtifacts,
  qualityFailureReasons,
  commandCategory,
  codeStoryBinaryIdentity,
  codestoryDoctorSnapshot,
  codestoryRetrievalEngineDiagnosticsSnapshot,
  codestoryRetrievalStatusSnapshot,
  extractCommandExecutions,
  interactionTurnTelemetry,
  isPathInside,
  isTrustedPublishableRepoUrl,
  loadTaskForResult,
  loadReleaseEvidenceCorpusContract,
  loadTasks,
  markdownCostAccounting,
  manifestRepoMaterializationBlockers,
  materializeRepos,
  parseArgs,
  parseJsonLines,
  cachePolicyForRun,
  codeStoryArmInstruction,
  cachePreparationCanaryBlockers,
  cachePreparationIdentityBlockers,
  candidateIncrementalRetrievalWorkBlockers,
  mergeRetrievalStatusWithEngineDiagnostics,
  projectResourceUri,
  retrievalEngineDiagnosticsSnapshotFromOutput,
  resourceUriMatches,
  createDurableJsonlAppender,
  createExactCandidatePrivateStateRoot,
  finalizeBenchmarkResources,
  finalBenchmarkFailure,
  initializeExactCandidateState,
  benchmarkAgentScopeArgs,
  retrievalIndexCommandArgs,
  retrievalIndexWorkEvidence,
  retrievalStatusSnapshotFromOutput,
  retrievalStatusCommandArgs,
  packetComposition,
  packetCommandArgs,
  drillPacketCommandArgs,
  packetForAgentPrompt,
  packetManifestExtraProbes,
  packetManifestQualitySummary,
  packetObligationAccounting,
  packetDispositionTelemetry,
  preludePublicFields,
  reanalysisExactCandidateAcceptance,
  reanalysisPacketProjection,
  packetPreludeContractBlockers,
  publicPacketPreludeContractPasses,
  packetPreludeManifestComplete,
  packetLatencyTelemetry,
  packetRuntimeCacheObservations,
  agentPacketPreludeCacheObservations,
  packetEmbeddingExecutionProof,
  packetSufficiencyTelemetry,
  groupPacketRuntimeColdJobs,
  gitCheckedOutput,
  gitOutput,
  packetRuntimePublishableBlockers,
  packetRuntimeQualityGateRequired,
  prepareAgentCodexIsolation,
  cacheProvenanceBlockers,
  PACKET_COMPOSITION_WEIGHTS,
  MAX_REUSED_ARTIFACT_BYTES,
  packetCompositionFileScore,
  packetFirstCommandForPrompt,
  publicCoreCorpusAudit,
  preludeAllowsAgentRun,
  planAgentRuns,
  exactCandidateAcceptance,
  exactCandidateCostRates,
  exactComparatorArtifactBundleSha256,
  authenticateExactCandidatePackages,
  exactCandidateArmEnv,
  exactCandidateBaselineEnv,
  exactCandidatePreparationArmOrder,
  refreshExactCandidatePreparation,
  packetV3EvidenceGapAccounting,
  packetV3EvidenceGapAccountingError,
  withExactSourceMutation,
  validateExactCandidateShape,
  validateExactCandidateResumePrefixRows,
  validateExactCandidateComparatorPrefixRows,
  validateExactComparatorLedgerSha256,
  repoProvenanceBlockers,
  repoProvenance,
  runnerCommand,
  resolveRunArtifactPath,
  repoConfigFromManifest,
  resolveCodeStoryCli,
  scoreQuality,
  sortAgentResultsCanonical,
  summarizeCostAccounting,
  summarizePacketObligationAccounting,
  summarizePacketRuntimeRuns,
  taskSnapshotForResult,
  taskShardIndex,
  tasksForShard,
  runCodeStoryPacketPrelude,
  runAgentBenchmarkPipeline,
  runPlannedAgentRuns,
  runProcess,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
