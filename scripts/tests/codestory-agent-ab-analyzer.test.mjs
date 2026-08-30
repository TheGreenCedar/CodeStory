import test from "node:test";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync } from "node:fs";
import assert from "node:assert/strict";
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, truncate, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { PassThrough } from "node:stream";

import {
  aggregateShardRuns,
  agentRunnerEnv,
  analyzeTranscript,
  agentPublishableBlockers,
  assertSafeWindowsCmdArgs,
  baselineSearchPreludeStatus,
  benchmarkAgentScopeArgs,
  benchmarkContractEnvironmentSha256,
  benchmarkContractForRun,
  benchmarkHostClass,
  benchmarkRunId,
  benchmarkShardAttestation,
  benchmarkShardAttestationForCloseout,
  cachePreparationCanaryBlockers,
  cachePreparationIdentityBlockers,
  codeStoryBinaryIdentity,
  commandCategory,
  codestoryDoctorSnapshot,
  codestoryRetrievalEngineDiagnosticsSnapshot,
  codestoryRetrievalStatusSnapshot,
  copyResultArtifact,
  createDurableJsonlAppender,
  groupPacketRuntimeColdJobs,
  gitCheckedOutput,
  isTrustedPublishableRepoUrl,
  isPathInside,
  interactionTurnTelemetry,
  loadTaskForResult,
  loadReleaseEvidenceCorpusContract,
  loadTasks,
  markdownCostAccounting,
  manifestRepoMaterializationBlockers,
  materializeRepos,
  mergeRetrievalStatusWithEngineDiagnostics,
  MAX_REUSED_ARTIFACT_BYTES,
  parseArgs as parseBenchmarkArgs,
  parseJsonLines,
  packetComposition,
  packetCommandArgs,
  drillPacketCommandArgs,
  packetRuntimeCacheObservations,
  agentPacketPreludeCacheObservations,
  packetEmbeddingExecutionProof,
  packetSufficiencyTelemetry,
  packetForAgentPrompt,
  packetManifestExtraProbes,
  packetManifestQualitySummary,
  packetObligationAccounting,
  packetDispositionTelemetry,
  preludePublicFields,
  packetPreludeContractBlockers,
  publicPacketPreludeContractPasses,
  packetPreludeManifestComplete,
  packetLatencyTelemetry,
  packetFirstCommandForPrompt,
  packetRuntimePublishableBlockers,
  packetRuntimeQualityGateRequired,
  preludeAllowsAgentRun,
  publicCoreCorpusAudit,
  projectResourceUri,
  reanalysisExactCandidateAcceptance,
  reanalysisPacketProjection,
  planAgentRuns,
  repoProvenance,
  repoProvenanceBlockers,
  resolveRunArtifactPath,
  resolveCodeStoryCli,
  runnerCommand,
  retrievalIndexCommandArgs,
  retrievalStatusCommandArgs,
  retrievalEngineDiagnosticsSnapshotFromOutput,
  retrievalStatusSnapshotFromOutput,
  resourceUriMatches,
  scoreQuality,
  sortAgentResultsCanonical,
  summarizeCostAccounting,
  summarizePacketObligationAccounting,
  summarizePacketRuntimeRuns,
  runCodeStoryPacketPrelude,
  runAgentBenchmarkPipeline,
  runPlannedAgentRuns,
  runProcess,
  buildQualityDebugPayload,
  qualityFailureReasons,
  taskSnapshotForResult,
  taskShardIndex,
  tasksForShard,
  cachePolicyForRun,
  cacheProvenanceBlockers,
} from "../codestory-agent-ab-benchmark.mjs";
import * as benchmarkHarness from "../codestory-agent-ab-benchmark.mjs";
import {
  packetGateSelectionOrThrow,
  packetGateStderrPath,
  parseArgs as parseScoreArgs,
  retryablePacketGateTaskIds,
} from "../codestory-agent-ab-score.mjs";

const RUNTIME_SERVICE_FILE = "crates/codestory-runtime/src/services.rs";
const RUN_INDEX_SYMBOL = "IndexService::run_indexing_blocking";
const RUNTIME_REFRESH_CLAIM =
  "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases.";

const EXACT_CANDIDATE_ARMS = [
  "without_codestory",
  "published_0_17_4",
  "candidate_0_18",
];
const EXACT_TASKS = [
  ["python-requests-session-flow", "psf-requests"],
  ["java-commons-lang-string-utils", "apache-commons-lang"],
  ["rust-ripgrep-search-pipeline", "BurntSushi-ripgrep"],
  ["javascript-express-routing-flow", "expressjs-express"],
  ["typescript-swr-hook-flow", "vercel-swr"],
  ["cpp-fmt-formatting-flow", "fmtlib-fmt"],
  ["c-redis-command-loop", "redis-redis"],
  ["go-gin-route-dispatch", "gin-gonic-gin"],
  ["ruby-jekyll-site-build", "jekyll-jekyll"],
  ["php-monolog-record-flow", "Seldaek-monolog"],
  ["csharp-automapper-map-flow", "AutoMapper-AutoMapper"],
  ["kotlin-okio-buffer-flow", "square-okio"],
  ["swift-alamofire-request-flow", "Alamofire-Alamofire"],
  ["dart-http-client-flow", "dart-lang-http"],
  ["bash-nvm-install-dispatch", "nvm-sh-nvm"],
  ["html-mdn-form-validation", "mdn-learning-area"],
  ["css-animate-base-and-keyframes", "animate-css-animate-css"],
  ["sql-chinook-schema-relations", "lerocha-chinook-database"],
];

function exactLifecycle() {
  return {
    contract: "codestory.agent-benchmark-exact-lifecycle/v1",
    package_authentication_order: ["published_0_17_4", "candidate_0_18"],
    package_authentication_ms: { published_0_17_4: 10, candidate_0_18: 10 },
    total_package_authentication_ms: 20,
    model_initialization_ms: { published_0_17_4: 5, candidate_0_18: 5 },
    cost_rates: {
      currency: "USD",
      model: "gpt-5.6-sol",
      input_per_mtok: 4,
      output_per_mtok: 20,
      source: "configured_environment",
    },
    preparation_order: EXACT_TASKS.map(([_, repo], index) => ({
      repo,
      arms: index % 2 === 0
        ? ["published_0_17_4", "candidate_0_18"]
        : ["candidate_0_18", "published_0_17_4"],
    })),
  };
}

function passingCacheProvenance(schemaVersion = 2) {
  const provenance = {
    doctor_status: "pass",
    storage_path: "/isolated/cache.db",
    cache_policy: "prepared-retrieval-cache-read-only",
    retrieval_mode: "full",
    semantic_generation: "semantic-1",
    manifest_embedding_backend: `per-user-server:coderank-embed:q8_0:sha256-${"a".repeat(64)}:fixture`,
    embedding_engine_instance_id: "engine-1",
    embedding_policy: "accelerated",
    semantic_backend: "embedded",
    local_only: true,
    indexed: true,
    freshness_status: "fresh",
    semantic_ready: true,
    indexing_in_timed_run: false,
    transport_mode: "agent_harness_prelude",
    packet_embedding_execution: {
      source: "packet.answer.retrieval_trace",
      transport_mode: "agent_harness_prelude",
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      embedding_policy: "accelerated",
      retrieval_mode: "full",
      diagnostic_count: 1,
      full_diagnostic_count: 1,
      semantic_stage_count: 1,
      completed_semantic_stage_count: 1,
      invalid_semantic_stage_count: 0,
      shadow_degraded_reason: null,
      shadow_error: null,
      shadow_cancel_reason: null,
      semantic_fallback_count: 0,
      semantic_generation: "semantic-1",
      prepared_semantic_generation: "semantic-1",
    },
    cache_preparation: {
      preparation_wall_ms: 100,
      incremental_status: "pass",
      incremental_wall_ms: 20,
      coherence_refresh_status: "pass",
      coherence_refresh_exit_code: 0,
      coherence_semantic_generation: "semantic-1",
      incremental_source_mutation: {
        path: "src/named.rs",
        original_sha256: "b".repeat(64),
        mutated_sha256: "c".repeat(64),
        restored_sha256: "b".repeat(64),
      },
    },
  };
  if (schemaVersion === 3) {
    provenance.cache_preparation.cold_retrieval_work_evidence = {
      core_phase_timings: { total_ms: 80 },
      retrieval_phase_timings: [
        { phase: "input fingerprint", elapsed_ms: 1 },
        { phase: "lexical sidecar", elapsed_ms: 20 },
        { phase: "embedded vectors", elapsed_ms: 40 },
        { phase: "graph artifact", elapsed_ms: 19 },
      ],
      retrieval_component_work: [
        { component: "lexical", mode: "complete", retained: 0, inserted: 100, removed: 0 },
        { component: "vectors", mode: "complete", retained: 0, inserted: 80, removed: 0 },
        { component: "graph", mode: "complete", retained: 0, inserted: 120, removed: 0 },
      ],
    };
    provenance.cache_preparation.incremental_retrieval_work_evidence = {
      core_phase_timings: { total_ms: 12 },
      retrieval_phase_timings: [
        { phase: "input fingerprint", elapsed_ms: 1 },
        { phase: "lexical sidecar", elapsed_ms: 4 },
        { phase: "embedded vectors", elapsed_ms: 4 },
        { phase: "graph artifact", elapsed_ms: 3 },
      ],
      retrieval_component_work: [
        { component: "lexical", mode: "copy_on_write", retained: 99, inserted: 1, removed: 1 },
        { component: "vectors", mode: "reused", retained: 80, inserted: 0, removed: 0 },
        { component: "graph", mode: "copy_on_write", retained: 119, inserted: 1, removed: 1 },
      ],
    };
    provenance.packet_embedding_execution = {
      source: "packet.v3_public_projection",
      schema_version: 3,
      transport_mode: "agent_harness_prelude",
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      embedding_policy: "accelerated",
      retrieval_mode: "full",
      packet_kind: "complete",
      evidence_status: "available",
      evidence_count: 1,
      gap_count: 0,
      core_generation: "core-1",
      core_run_id: "run-1",
      retrieval_core_generation: "core-1",
      retrieval_core_run_id: "run-1",
      retrieval_generation: "retrieval-1",
      retrieval_state_generation: "retrieval-1",
      semantic_generation: "semantic-1",
      prepared_semantic_generation: "semantic-1",
      diagnostics_availability: "available",
      diagnostics_artifact_id: "diagnostic-1",
      diagnostics_sha256: "7".repeat(64),
      diagnostics_byte_length: 512,
    };
  }
  return provenance;
}

function exactCandidateRows() {
  const rows = [];
  for (const [taskId, repo] of EXACT_TASKS) {
    for (let repeat = 1; repeat <= 3; repeat += 1) {
      for (const arm of EXACT_CANDIDATE_ARMS) {
        const codestory = arm !== "without_codestory";
        const packageVersion = "0.17.4";
        const packageByte = arm === "published_0_17_4" ? "a" : "b";
        rows.push({
          repo,
          task_id: taskId,
          arm,
          repeat,
          status: "pass",
          quality: {
            pass: true,
            material_factual_errors: { found: 0, found_anchors: [] },
            unsupported_proof_claims: { found: 0, found_claims: [] },
          },
          usage: codestory
            ? { input_tokens: 50, output_tokens: 20, total_tokens: 70 }
            : { input_tokens: 70, output_tokens: 30, total_tokens: 100 },
          estimated_cost_usd: codestory ? 0.7 : 1,
          tool_calls_observed: codestory ? 7 : 10,
          transcript_analysis: {
            command_count: codestory ? 7 : 10,
            tool_categories: { command_execution: codestory ? 7 : 10 },
            command_categories: codestory
              ? { codestory_cli: 1, direct_file_read: 6 }
              : { codestory_cli: 0, direct_file_read: 10 },
            interaction_turns: codestory
              ? {
                  total: 9, model_messages: 2, tool_actions: 7, failed_tool_actions: 0,
                  reasoning_items_excluded: 0, error_items_excluded: 0,
                }
              : {
                  total: 12, model_messages: 2, tool_actions: 10, failed_tool_actions: 0,
                  reasoning_items_excluded: 0, error_items_excluded: 0,
                },
            codestory_mcp_tool_calls_observed: 0,
            codestory_mcp_completed_calls_observed: 0,
            codestory_mcp_runtime_identities: [],
            external_context_tool_calls: 0,
            direct_source_reads: codestory
              ? [{ path: "src/named.rs", authorization: { status: "authorized", reason: "user_named_file" } }]
              : [{ path: "src/read.rs", authorization: { status: "baseline_local_exploration", reason: "without_codestory" } }],
            direct_source_reads_total: 1,
          },
          exact_candidate_timing: codestory
            ? {
                cold_ms: arm === "candidate_0_18" ? 100 : 100,
                warm_ms: arm === "candidate_0_18" ? 50 : 50,
                incremental_ms: arm === "candidate_0_18" ? 20 : 20,
                all_in_ms: arm === "candidate_0_18" ? 50 : 50,
              }
            : { cold_ms: 0, warm_ms: 100, incremental_ms: 0, all_in_ms: 100 },
          wall_ms: codestory ? 50 : 100,
          malformed_stdout_lines: 0,
          json_events: 1,
          analysis_events: 1,
          event_types: { fixture: 1 },
          packet_first_required: codestory,
          packet_first_pass: true,
          repo_provenance: pinnedRepoProvenance(),
          task_manifest_snapshot: {
            repo,
            repo_metadata: {
              name: repo,
              url: "https://github.com/example/fixture.git",
              ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
            },
          },
          package_identity: arm === "published_0_17_4"
            ? {
                contract: "codestory.agent-benchmark-package/v2",
                arm,
                package_version: packageVersion,
                package_sha256: packageByte.repeat(64),
                cli_sha256: "c".repeat(64),
                source_commit: "e".repeat(40),
                source_tree: "1".repeat(40),
                schema_version: 2,
                protocol_revision: "2024-11-05",
                discovery_contract_sha256: null,
                trust_root_kind: "official_published_checksum",
                trust_root_sha256: "5".repeat(64),
              }
            : null,
          source_cli_identity: arm === "candidate_0_18"
            ? {
                contract: "codestory.agent-benchmark-source-cli/v1",
                arm,
                package_version: packageVersion,
                cli_sha256: "d".repeat(64),
                source_commit: "f".repeat(40),
                source_tree: "2".repeat(40),
                schema_version: 3,
                protocol_revision: "2025-11-25",
                discovery_contract_sha256: "4".repeat(64),
                plugin_manifest_sha256: "6".repeat(64),
                catalog_sha256: "7".repeat(64),
              }
            : null,
          codestory_prelude_cli: codestory ? "/authenticated/codestory-cli" : null,
          codestory_prelude_cli_sha256: codestory
            ? (arm === "published_0_17_4" ? "c" : "d").repeat(64)
            : null,
          codestory_binary_identity: codestory
            ? {
                status: "prelude_only",
                prelude_cli_sha256: (arm === "published_0_17_4" ? "c" : "d").repeat(64),
              }
            : null,
          codestory_cache_provenance: codestory
            ? passingCacheProvenance(arm === "candidate_0_18" ? 3 : 2)
            : null,
          codestory_harness_prelude: codestory
            ? {
                status: "pass",
                packet_schema_version: arm === "candidate_0_18" ? 3 : 2,
                packet_extra_probe_strategy: null,
                packet_contract_runtime: {
                  cli_version: packageVersion,
                  cli_source: "direct_cli_launch",
                  known_override_skew_channel: false,
                },
                ...(arm === "candidate_0_18"
                  ? {
                      packet_evidence_gap_accounting: {
                        contract: "codestory.packet-v3-evidence-gap-accounting/v1",
                        kind: "complete",
                        status: "available",
                        evidence_count: 1,
                        unique_evidence_id_count: 1,
                        evidence_kind_counts: { exact_source: 1 },
                        gap_count: 0,
                        unique_gap_id_count: 0,
                        gap_kind_counts: {},
                        continuation_gap_count: 0,
                        unique_continuation_gap_id_count: 0,
                        continuation_gap_ids_bound: true,
                      },
                    }
                  : { packet_sufficiency: {
                  obligation_accounting: {
                    total: 0,
                    material: 0,
                    nonmaterial: 0,
                    material_status_buckets: {},
                  },
                } }),
              }
            : null,
        });
      }
    }
  }
  return rows;
}

function exactArgs(extra = []) {
  return [
    "--exact-candidate",
    "--task-suite", "language-expansion-holdout",
    "--published-archive", "/tmp/published.tar.gz",
    "--published-checksum-manifest", "/tmp/SHA256SUMS.txt",
    "--published-checksum-sha256", "a".repeat(64),
    "--candidate-source-root", "/tmp/candidate-source",
    ...extra,
  ];
}

test("exact-candidate mode freezes the 18x3x3 shape and rejects baseline reuse", () => {
  assert.throws(
    () => benchmarkHarness.parseArgs(exactArgs(["--reuse-baseline-from", "/tmp/old"])),
    /forbids option.*reuse-baseline/i,
  );
  const opts = benchmarkHarness.parseArgs(exactArgs());
  assert.deepEqual(opts.arms, EXACT_CANDIDATE_ARMS);
  assert.equal(opts.repeats, 3);
  assert.throws(
    () => benchmarkHarness.validateExactCandidateShape(opts, Array.from({ length: 17 }, (_, index) => ({ id: `t-${index}` }))),
    /exactly 18 pinned tasks/i,
  );
  assert.doesNotThrow(() =>
    benchmarkHarness.validateExactCandidateShape(
      opts,
      Array.from({ length: 18 }, (_, index) => ({ id: `t-${index}` })),
    )
  );
});

test("exact-candidate task contract includes the file-local routing field", async () => {
  const opts = benchmarkHarness.parseArgs(exactArgs());
  const tasks = await loadTasks(opts);
  assert.equal(tasks.length, 18);
  assert.ok(tasks.every((task) => task.file_local === false));
  assert.doesNotThrow(() => benchmarkHarness.validateExactCandidateShape(opts, tasks));

  const changedTasks = tasks.map((task, index) =>
    index === 0 ? { ...task, prompt: `${task.prompt} changed` } : task
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateShape(opts, changedTasks),
    /qualification inputs differ from the pinned task window/i,
  );
});

test("exact-candidate cost rates fail before package or repository work", () => {
  assert.throws(
    () => benchmarkHarness.exactCandidateCostRates({}),
    /requires positive CODESTORY_BENCH_INPUT_COST_PER_MTOK.*before package authentication or repository materialization/i,
  );
  assert.deepEqual(
    benchmarkHarness.exactCandidateCostRates({
      CODESTORY_BENCH_INPUT_COST_PER_MTOK: "4",
      CODESTORY_BENCH_OUTPUT_COST_PER_MTOK: "20",
    }),
    {
      currency: "USD",
      model: "gpt-5.6-sol",
      input_per_mtok: 4,
      output_per_mtok: 20,
      source: "configured_environment",
    },
  );
});

test("exact-candidate mode closes every option that can change freshness oracle or run shape", () => {
  const forbidden = [
    ["--list"],
    ["--self-test"],
    ["--reanalyze-dir", "/tmp/old"],
    ["--quick"],
    ["--publishable"],
    ["--allow-failures"],
    ["--diagnostic-extra-probes-from-manifest"],
    ["--include-local-repos"],
    ["--packet-runtime"],
    ["--packet-runtime-mode", "cold-cli"],
    ["--codestory-cli", "/tmp/local-codestory-cli"],
    ["--repos", "psf-requests"],
    ["--arms", EXACT_CANDIDATE_ARMS.join(",")],
    ["--task-ids", EXACT_TASKS[0][0]],
    ["--task-manifest", "/tmp/tasks"],
    ["--repeats", "3"],
    ["--runner", "codex"],
    ["--model", "gpt-5.6-sol"],
    ["--sandbox", "workspace-write"],
    ["--benchmark-run-id", "reused"],
    ["--timeout-ms", "600000"],
    ["--jobs", "1"],
    ["--reuse-baseline-from", "/tmp/old"],
    ["--prepare-codestory-cache"],
    ["--no-prepare-codestory-cache"],
    ["--prepare-codestory-timeout-ms", "1800000"],
    ["--prepare-codestory-jobs", "1"],
    ["--canary-task-id", EXACT_TASKS[0][0]],
    ["--shard-count", "1"],
    ["--shard-index", "0"],
    ["--aggregate-shards", "/tmp/shard"],
    ["--candidate-package-sha256", "c".repeat(64)],
    ["--candidate-cli", "/tmp/arbitrary-candidate-cli"],
    ["--candidate-cli-sha256", "c".repeat(64)],
    ["--collect-all-failures"],
    ["--max-source-reads-after-packet", "0"],
  ];
  for (const args of forbidden) {
    assert.throws(
      () => benchmarkHarness.parseArgs(exactArgs(args)),
      /exact-candidate mode forbids|unsupported|mutually exclusive|unknown option/i,
      args[0],
    );
  }

  for (const required of [
    "--published-archive",
    "--published-checksum-manifest",
    "--published-checksum-sha256",
    "--candidate-source-root",
  ]) {
    const args = exactArgs();
    args.splice(args.indexOf(required), 2);
    assert.throws(
      () => benchmarkHarness.parseArgs(args),
      /requires authenticated published archive and candidate source input/i,
      required,
    );
  }
  for (const digestOption of ["--published-checksum-sha256"]) {
    const args = exactArgs();
    args[args.indexOf(digestOption) + 1] = "0".repeat(64);
    assert.throws(() => benchmarkHarness.parseArgs(args), /all-zero digest/i, digestOption);
  }

  const permitted = benchmarkHarness.parseArgs(exactArgs([
    "--materialize-repos",
    "--repo-cache-dir", "/tmp/exact-repos",
    "--out-dir", "/tmp/exact-output",
    "--resume-prefix-from", "/tmp/exact-prefix",
  ]));
  assert.equal(permitted.materializeRepos, true);
  assert.equal(permitted.diagnosticExtraProbesFromManifest, false);
  assert.equal(permitted.resumePrefixFrom, "/tmp/exact-prefix");

  const comparatorReuse = benchmarkHarness.parseArgs(exactArgs([
    "--reuse-comparators-from", "/tmp/exact-comparators",
    "--reuse-comparators-ledger-sha256", "b".repeat(64),
    "--reuse-comparators-artifacts-sha256", "c".repeat(64),
  ]));
  assert.equal(comparatorReuse.reuseComparatorsFrom, "/tmp/exact-comparators");
  assert.equal(comparatorReuse.reuseComparatorsLedgerSha256, "b".repeat(64));
  assert.equal(comparatorReuse.reuseComparatorsArtifactsSha256, "c".repeat(64));
  assert.throws(
    () => benchmarkHarness.parseArgs(exactArgs([
      "--reuse-comparators-from", "/tmp/exact-comparators",
    ])),
    /requires --reuse-comparators-from.*ledger.*artifacts/i,
  );
  assert.throws(
    () => benchmarkHarness.parseArgs(exactArgs([
      "--resume-prefix-from", "/tmp/exact-prefix",
      "--reuse-comparators-from", "/tmp/exact-comparators",
      "--reuse-comparators-ledger-sha256", "b".repeat(64),
      "--reuse-comparators-artifacts-sha256", "c".repeat(64),
    ])),
    /mutually exclusive/i,
  );
});

test("exact-candidate planning balances deterministic arm position across 162 fresh rows", () => {
  const tasks = Array.from({ length: 18 }, (_, index) => ({
    id: `task-${index + 1}`,
    repo: `repo-${index + 1}`,
  }));
  const opts = { exactCandidate: true, arms: EXACT_CANDIDATE_ARMS, repeats: 3, repos: null };
  const first = benchmarkHarness.planAgentRuns(opts, tasks);
  const second = benchmarkHarness.planAgentRuns(opts, tasks);
  assert.deepEqual(first, second);
  assert.equal(first.length, 162);
  const positions = Object.fromEntries(EXACT_CANDIDATE_ARMS.map((arm) => [arm, [0, 0, 0]]));
  for (let index = 0; index < first.length; index += 3) {
    const triplet = first.slice(index, index + 3);
    assert.equal(new Set(triplet.map((run) => run.task.id)).size, 1);
    assert.equal(new Set(triplet.map((run) => run.repeat)).size, 1);
    assert.deepEqual(new Set(triplet.map((run) => run.arm)), new Set(EXACT_CANDIDATE_ARMS));
    triplet.forEach((run, position) => {
      positions[run.arm][position] += 1;
    });
  }
  for (const arm of EXACT_CANDIDATE_ARMS) {
    assert.deepEqual(positions[arm], [18, 18, 18]);
  }
});

test("exact-candidate resume accepts only an authenticated whole-task contiguous prefix", () => {
  const tasks = [
    { id: "task-1", repo: "repo-1" },
    { id: "task-2", repo: "repo-2" },
  ];
  const candidate = {
    contract: "codestory.agent-benchmark-source-cli/v1",
    arm: "candidate_0_18",
    package_version: "0.17.5",
    cli_sha256: "1".repeat(64),
    source_commit: "2".repeat(40),
    source_tree: "3".repeat(40),
    schema_version: 3,
    protocol_revision: "2025-11-25",
    discovery_contract_sha256: "4".repeat(64),
    plugin_manifest_sha256: "5".repeat(64),
    catalog_sha256: "6".repeat(64),
  };
  const published = {
    contract: "codestory.agent-benchmark-exact-package/v1",
    arm: "published_0_17_4",
    package_version: "0.17.4",
    package_sha256: "7".repeat(64),
    cli_sha256: "8".repeat(64),
    source_commit: "9".repeat(40),
    source_tree: "a".repeat(40),
    schema_version: 2,
    protocol_revision: "2024-11-05",
    discovery_contract_sha256: null,
    trust_root: { kind: "official_published_checksum", sha256: "b".repeat(64) },
  };
  const opts = {
    exactCandidate: true,
    arms: EXACT_CANDIDATE_ARMS,
    repeats: 3,
    repos: null,
    exactCandidatePackageByArm: new Map([
      ["candidate_0_18", candidate],
      ["published_0_17_4", published],
    ]),
  };
  const planned = benchmarkHarness.planAgentRuns(opts, tasks);
  const rows = planned.slice(0, 9).map((run) => ({
    repo: run.repo,
    task_id: run.task.id,
    arm: run.arm,
    repeat: run.repeat,
    status: "pass",
    task_manifest_snapshot: benchmarkHarness.taskSnapshotForResult(run.task),
    package_identity: run.arm === "published_0_17_4"
      ? {
          contract: published.contract,
          arm: published.arm,
          package_version: published.package_version,
          package_sha256: published.package_sha256,
          cli_sha256: published.cli_sha256,
          source_commit: published.source_commit,
          source_tree: published.source_tree,
          schema_version: published.schema_version,
          protocol_revision: published.protocol_revision,
          discovery_contract_sha256: null,
          trust_root_kind: published.trust_root.kind,
          trust_root_sha256: published.trust_root.sha256,
        }
      : null,
    source_cli_identity: run.arm === "candidate_0_18"
      ? { ...candidate, source_commit: "c".repeat(40), source_tree: "d".repeat(40) }
      : null,
  }));

  assert.equal(
    benchmarkHarness.validateExactCandidateResumePrefixRows(rows, planned, opts),
    1,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateResumePrefixRows(rows.slice(0, 8), planned, opts),
    /complete task boundary/i,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateResumePrefixRows(
      [rows[1], rows[0], ...rows.slice(2)],
      planned,
      opts,
    ),
    /planned contiguous prefix/i,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateResumePrefixRows(
      rows.map((row) => row.arm === "candidate_0_18"
        ? { ...row, source_cli_identity: { ...row.source_cli_identity, cli_sha256: "e".repeat(64) } }
        : row),
      planned,
      opts,
    ),
    /candidate CLI or public contract identity changed/i,
  );
});

test("exact comparator reuse accepts only complete ordered comparator triplets and never candidates", () => {
  const tasks = [
    { id: "task-1", repo: "repo-1" },
    { id: "task-2", repo: "repo-2" },
  ];
  const candidate = {
    contract: "codestory.agent-benchmark-source-cli/v1",
    arm: "candidate_0_18",
    package_version: "0.17.5",
    cli_sha256: "1".repeat(64),
    source_commit: "2".repeat(40),
    source_tree: "3".repeat(40),
    schema_version: 3,
    protocol_revision: "2025-11-25",
    discovery_contract_sha256: "4".repeat(64),
    plugin_manifest_sha256: "5".repeat(64),
    catalog_sha256: "6".repeat(64),
    cli_path: "/new/candidate",
  };
  const published = {
    contract: "codestory.agent-benchmark-package/v2",
    arm: "published_0_17_4",
    package_version: "0.17.4",
    package_sha256: "7".repeat(64),
    cli_sha256: "8".repeat(64),
    source_commit: "9".repeat(40),
    source_tree: "a".repeat(40),
    schema_version: 2,
    protocol_revision: "2024-11-05",
    discovery_contract_sha256: null,
    trust_root: { kind: "official_published_checksum", sha256: "b".repeat(64) },
    cli_path: "/new/published",
  };
  const opts = {
    exactCandidate: true,
    arms: EXACT_CANDIDATE_ARMS,
    repeats: 3,
    repos: null,
    runner: "codex",
    model: "gpt-5.6-sol",
    sandbox: "workspace-write",
    taskSuite: "language-expansion-holdout",
    exactCandidatePackageByArm: new Map([
      ["candidate_0_18", candidate],
      ["published_0_17_4", published],
    ]),
  };
  const planned = benchmarkHarness.planAgentRuns(opts, tasks);
  const publishedIdentity = {
    contract: published.contract,
    arm: published.arm,
    package_version: published.package_version,
    package_sha256: published.package_sha256,
    cli_sha256: published.cli_sha256,
    source_commit: published.source_commit,
    source_tree: published.source_tree,
    schema_version: published.schema_version,
    protocol_revision: published.protocol_revision,
    discovery_contract_sha256: null,
    trust_root_kind: published.trust_root.kind,
    trust_root_sha256: published.trust_root.sha256,
  };
  const rows = planned.slice(0, 9).map((run) => ({
    repo: run.repo,
    task_id: run.task.id,
    arm: run.arm,
    repeat: run.repeat,
    status: "pass",
    task_manifest_snapshot: benchmarkHarness.taskSnapshotForResult(run.task),
    benchmark_contract: benchmarkHarness.benchmarkContractForRun(opts, run),
    package_identity: run.arm === "published_0_17_4" ? publishedIdentity : null,
    source_cli_identity: run.arm === "candidate_0_18"
      ? { ...candidate, cli_sha256: "c".repeat(64), source_commit: "d".repeat(40) }
      : null,
  }));

  const accepted = benchmarkHarness.validateExactCandidateComparatorPrefixRows(
    rows,
    planned,
    opts,
  );
  assert.equal(accepted.completedTaskCount, 1);
  assert.equal(accepted.comparatorRows.length, 6);
  assert.equal(accepted.comparatorRows.some((row) => row.arm === "candidate_0_18"), false);
  assert.throws(
    () => benchmarkHarness.validateExactCandidateComparatorPrefixRows(
      rows.filter((row) => !(row.arm === "published_0_17_4" && row.repeat === 3)),
      planned,
      opts,
    ),
    /complete task boundary|comparator triplets/i,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateComparatorPrefixRows(
      [rows[1], rows[0], ...rows.slice(2)],
      planned,
      opts,
    ),
    /planned contiguous prefix/i,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateComparatorPrefixRows(
      rows.map((row) => row.arm === "published_0_17_4"
        ? { ...row, package_identity: { ...row.package_identity, cli_sha256: "e".repeat(64) } }
        : row),
      planned,
      opts,
    ),
    /published package identity/i,
  );
  assert.throws(
    () => benchmarkHarness.validateExactCandidateComparatorPrefixRows(
      rows.map((row) => row.arm === "without_codestory"
        ? { ...row, benchmark_contract: { ...row.benchmark_contract, model: "drifted" } }
        : row),
      planned,
      opts,
    ),
    /benchmark contract/i,
  );
});

test("exact comparator reuse binds both ledger and referenced artifact bytes", async () => {
  const runDir = await mkdtemp(path.join(os.tmpdir(), "codestory-comparator-source-"));
  const outDir = await mkdtemp(path.join(os.tmpdir(), "codestory-comparator-copy-"));
  try {
    const ledgerBytes = Buffer.from('{"row":1}\n', "utf8");
    await writeFile(path.join(runDir, "runs.jsonl"), ledgerBytes);
    await writeFile(path.join(runDir, "row.stdout.jsonl"), '{"type":"done"}\n');
    await writeFile(path.join(runDir, "row.stderr.txt"), "");
    const artifactPaths = ["row.stdout.jsonl", "row.stderr.txt"];
    const artifactSha = await benchmarkHarness.exactComparatorArtifactBundleSha256(
      runDir,
      artifactPaths,
    );
    const ledgerSha = createHash("sha256").update(ledgerBytes).digest("hex");
    await benchmarkHarness.copyAuthenticatedComparatorArtifacts(
      runDir,
      outDir,
      artifactPaths,
      artifactSha,
    );
    assert.equal(
      await readFile(path.join(outDir, "row.stdout.jsonl"), "utf8"),
      '{"type":"done"}\n',
    );
    assert.doesNotThrow(() =>
      benchmarkHarness.validateExactComparatorLedgerSha256(ledgerBytes, ledgerSha)
    );

    await writeFile(path.join(runDir, "row.stdout.jsonl"), '{"type":"tampered"}\n');
    await assert.rejects(
      benchmarkHarness.copyAuthenticatedComparatorArtifacts(
        runDir,
        outDir,
        artifactPaths,
        artifactSha,
      ),
      /artifact bundle digest/i,
    );
    assert.throws(
      () => benchmarkHarness.validateExactComparatorLedgerSha256(
        Buffer.from('{"row":2}\n', "utf8"),
        ledgerSha,
      ),
      /ledger digest/i,
    );
  } finally {
    await rm(runDir, { recursive: true, force: true });
    await rm(outDir, { recursive: true, force: true });
  }
});

async function makeExactArchive(root, name, {
  version,
  runtimeVersion = version,
  schema,
  source,
  tree,
  discovery,
  executionMarker = null,
}) {
  const packageRoot = path.join(root, `${name}-root`);
  await mkdir(packageRoot, { recursive: true });
  const cliPath = path.join(packageRoot, "codestory-cli");
  await writeFile(
    cliPath,
    `#!/usr/bin/env node
const readline = require("node:readline");
const fs = require("node:fs");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const request = JSON.parse(line);
  if (request.method !== "initialize") return;
  ${executionMarker ? `fs.writeFileSync(${JSON.stringify(executionMarker)}, "executed");` : ""}
  const response = {
    jsonrpc: "2.0",
    id: request.id,
    result: {
      protocolVersion: request.params.protocolVersion,
      serverInfo: { name: "codestory", version: ${JSON.stringify(runtimeVersion)} },
      _meta: {
        codestory_publication: { schema_version: ${schema} },
        codestory_protocol: { discovery_contract_sha256: ${JSON.stringify(discovery)} },
      },
    },
  };
  process.stdout.write(JSON.stringify(response) + "\\n");
});
`,
  );
  await chmod(cliPath, 0o755);
  const cliSha = createHash("sha256").update(await readFile(cliPath)).digest("hex");
  await writeFile(path.join(packageRoot, "codestory-native-manifest.json"), JSON.stringify({
    schema_version: 3,
    release_version: version,
    source: { commit: source, tree, tracked_dirty: false },
    binary: { name: "codestory-cli", sha256: cliSha },
  }));
  const archivePath = path.join(root, `${name}.tar.gz`);
  const packed = spawnSync("tar", ["-czf", archivePath, "-C", root, path.basename(packageRoot)], {
    encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
  return {
    archivePath,
    sha256: createHash("sha256").update(await readFile(archivePath)).digest("hex"),
  };
}

async function makeCandidateSourceCli(root, name, {
  version = "0.17.5",
  pluginVersion = version,
  runtimeVersion = version,
  schema = 3,
  catalogSchema = 3,
  protocol = "2025-11-25",
  discovery = "f".repeat(64),
  executionMarker = null,
}) {
  const sourceRoot = path.join(root, `${name}-source`);
  await mkdir(path.join(sourceRoot, "crates", "codestory-cli"), { recursive: true });
  await mkdir(path.join(sourceRoot, "plugins", "codestory"), { recursive: true });
  await writeFile(path.join(sourceRoot, ".gitignore"), "target/\n");
  await writeFile(path.join(sourceRoot, "crates", "codestory-cli", "Cargo.toml"), `[package]\nname = "codestory-cli"\nversion = "${version}"\n`);
  const pluginBytes = Buffer.from(`${JSON.stringify({ name: "codestory", version: pluginVersion }, null, 2)}\n`);
  const catalogBytes = Buffer.from(`${JSON.stringify({
    wireContract: {
      publicationStampSchemaVersion: catalogSchema,
      minimumCompatiblePublicationStampSchemaVersion: catalogSchema,
      supportedMcpProtocolVersions: ["2024-11-05", "2025-03-26", "2025-06-18", protocol],
      preferredMcpProtocolVersion: protocol,
      discoveryContracts: { [protocol]: discovery },
    },
  }, null, 2)}\n`);
  await writeFile(path.join(sourceRoot, "plugins", "codestory", "plugin.json"), pluginBytes);
  await writeFile(path.join(sourceRoot, "plugins", "codestory", "generated-mcp-catalog.json"), catalogBytes);
  for (const args of [
    ["init", "-q", sourceRoot],
    ["-C", sourceRoot, "config", "user.email", "fixture@example.com"],
    ["-C", sourceRoot, "config", "user.name", "Fixture"],
    ["-C", sourceRoot, "add", "."],
    ["-C", sourceRoot, "commit", "-q", "-m", "fixture"],
  ]) {
    const result = spawnSync("git", args, { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
  }
  const cliPath = path.join(root, `${name}-codestory-cli`);
  await writeFile(cliPath, `#!/usr/bin/env node
const readline = require("node:readline");
const fs = require("node:fs");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const request = JSON.parse(line);
  if (request.method !== "initialize") return;
  ${executionMarker ? `fs.writeFileSync(${JSON.stringify(executionMarker)}, "executed");` : ""}
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {
    protocolVersion: request.params.protocolVersion,
    serverInfo: { name: "codestory", version: ${JSON.stringify(runtimeVersion)} },
    _meta: {
      codestory_publication: { schema_version: ${schema} },
      codestory_protocol: { discovery_contract_sha256: ${JSON.stringify(discovery)} },
    },
  } }) + "\\n");
});
`);
  await chmod(cliPath, 0o755);
  const cliSha256 = createHash("sha256").update(await readFile(cliPath)).digest("hex");
  const sourceCommit = spawnSync("git", ["-C", sourceRoot, "rev-parse", "HEAD"], { encoding: "utf8" }).stdout.trim();
  const sourceTree = spawnSync("git", ["-C", sourceRoot, "rev-parse", "HEAD^{tree}"], { encoding: "utf8" }).stdout.trim();
  return {
    sourceRoot,
    cliPath,
    cliSha256,
    sourceCommit,
    sourceTree,
    pluginSha256: createHash("sha256").update(pluginBytes).digest("hex"),
    catalogSha256: createHash("sha256").update(catalogBytes).digest("hex"),
  };
}

test("exact candidate binds clean source, checked-in identities, immutable CLI bytes, and live initialize", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-three-arm-source-cli-"));
  try {
    const published = await makeExactArchive(root, "published", {
      version: "0.17.4", schema: 2, source: "a".repeat(40), tree: "b".repeat(40), discovery: null,
    });
    const candidate = await makeCandidateSourceCli(root, "candidate", {});
    const checksumPath = path.join(root, "SHA256SUMS.txt");
    await writeFile(checksumPath, `${published.sha256}  ${path.basename(published.archivePath)}\n`);
    const checksumSha = createHash("sha256").update(await readFile(checksumPath)).digest("hex");
    const run = async (candidateInput = candidate, overrides = {}) => {
      const state = await mkdtemp(path.join(root, "state-"));
      return await benchmarkHarness.authenticateExactCandidatePackages({
        exactCandidate: true,
        exactCandidateStateRoot: state,
        publishedArchive: published.archivePath,
        publishedChecksumManifest: checksumPath,
        publishedChecksumSha256: checksumSha,
        candidateSourceRoot: candidateInput.sourceRoot,
        exactCandidateBuildCli: async ({ sourceRoot, targetDir, cliPath }) => {
          assert.equal(sourceRoot, await realpath(candidateInput.sourceRoot));
          assert.equal(targetDir, path.join(sourceRoot, "target", "codestory-mission-candidate"));
          assert.equal(cliPath, path.join(targetDir, "release", process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli"));
          await mkdir(path.dirname(cliPath), { recursive: true });
          await copyFile(candidateInput.cliPath, cliPath);
          await chmod(cliPath, 0o755);
        },
        ...overrides,
      });
    };
    const accepted = await run();
    assert.equal(accepted.packages.get("published_0_17_4").package_sha256, published.sha256);
    assert.equal(accepted.packages.get("published_0_17_4").protocol_revision, "2024-11-05");
    assert.equal(accepted.packages.get("published_0_17_4").discovery_contract_sha256, null);
    const candidateIdentity = accepted.packages.get("candidate_0_18");
    assert.equal(candidateIdentity.contract, "codestory.agent-benchmark-source-cli/v1");
    assert.equal(candidateIdentity.source_commit, candidate.sourceCommit);
    assert.equal(candidateIdentity.source_tree, candidate.sourceTree);
    assert.equal(candidateIdentity.cli_sha256, candidate.cliSha256);
    assert.equal(candidateIdentity.plugin_manifest_sha256, candidate.pluginSha256);
    assert.equal(candidateIdentity.catalog_sha256, candidate.catalogSha256);
    assert.equal(Object.hasOwn(candidateIdentity, "package_sha256"), false);
    assert.equal(Object.hasOwn(candidateIdentity, "trust_root"), false);
    assert.match(accepted.packages.get("candidate_0_18").cli_path, /state-/);
    await assert.rejects(run(candidate, { publishedChecksumSha256: "2".repeat(64) }), /external digest/i);
    const runtimeDrift = await makeCandidateSourceCli(root, "candidate-runtime-drift", { runtimeVersion: "0.18.0" });
    await assert.rejects(run(runtimeDrift), /runtime package_version=0\.18\.0; expected 0\.17\.5/i);
    const schemaDrift = await makeCandidateSourceCli(root, "candidate-schema-drift", { schema: 4 });
    await assert.rejects(run(schemaDrift), /runtime schema_version=4; expected 3/i);
    const pluginDrift = await makeCandidateSourceCli(root, "candidate-plugin-drift", { pluginVersion: "9.9.9" });
    await assert.rejects(run(pluginDrift), /checked-in plugin\/catalog identity/i);
    await writeFile(path.join(candidate.sourceRoot, "untracked"), "dirty");
    await assert.rejects(run(candidate), /clean tracked and untracked worktree/i);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("exact input ingestion makes every caller path irrelevant before parsing extraction or execution", async () => {
  for (const kind of [
    "published_checksum_manifest",
    "published_0_17_4_archive",
    "candidate_cli",
  ]) {
    const root = await mkdtemp(path.join(os.tmpdir(), `codestory-exact-race-${kind}-`));
    try {
      const marker = path.join(root, "substituted-cli-executed");
      const published = await makeExactArchive(root, "published-original", {
        version: "0.17.4", schema: 2, source: "a".repeat(40), tree: "b".repeat(40), discovery: null,
      });
      const candidate = await makeCandidateSourceCli(root, "candidate-original", {});
      const publishedSubstitute = await makeExactArchive(root, "published-substitute", {
        version: "0.17.4", schema: 2, source: "a".repeat(40), tree: "b".repeat(40), discovery: null,
        executionMarker: marker,
      });
      const candidateSubstitute = await makeCandidateSourceCli(root, "candidate-substitute", {
        executionMarker: marker,
      });
      const publishedInput = path.join(root, "published-input.tar.gz");
      const candidateInput = path.join(root, "candidate-input-cli");
      await copyFile(published.archivePath, publishedInput);
      await copyFile(candidate.cliPath, candidateInput);
      await chmod(candidateInput, 0o755);
      const checksumPath = path.join(root, "SHA256SUMS.txt");
      await writeFile(checksumPath, `${published.sha256}  ${path.basename(publishedInput)}\n`);
      const state = await mkdtemp(path.join(root, "state-"));
      const result = await benchmarkHarness.authenticateExactCandidatePackages({
        exactCandidate: true,
        exactCandidateStateRoot: state,
        publishedArchive: publishedInput,
        publishedChecksumManifest: checksumPath,
        publishedChecksumSha256: createHash("sha256").update(await readFile(checksumPath)).digest("hex"),
        candidateSourceRoot: candidate.sourceRoot,
        exactCandidateBuildCli: async ({ cliPath }) => {
          await mkdir(path.dirname(cliPath), { recursive: true });
          await copyFile(candidateInput, cliPath);
          await chmod(cliPath, 0o755);
        },
        exactCandidateAfterInputIngest: async (event) => {
          if (event.kind !== kind) return;
          if (kind === "published_checksum_manifest") {
            await writeFile(event.source_path, "substituted after ingest");
          } else if (kind === "published_0_17_4_archive") {
            await copyFile(publishedSubstitute.archivePath, event.source_path);
          } else {
            await copyFile(candidateSubstitute.cliPath, event.source_path);
          }
        },
      });
      assert.equal(result.packages.get("published_0_17_4").package_sha256, published.sha256, kind);
      assert.equal(result.packages.get("candidate_0_18").cli_sha256, candidate.cliSha256, kind);
      assert.equal(existsSync(marker), false, `${kind} executed the substituted CLI`);
      assert.ok(isPathInside(path.join(state, "authenticated-inputs"), result.packages.get("published_0_17_4").package_path));
      assert.ok(isPathInside(path.join(state, "authenticated-inputs"), result.packages.get("candidate_0_18").cli_path));
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test("exact-candidate acceptance closes the complete causal threshold matrix", () => {
  const passing = exactCandidateRows();
  const accepted = benchmarkHarness.exactCandidateAcceptance(passing, exactLifecycle());
  assert.equal(accepted.pass, true, JSON.stringify(accepted));
  assert.equal(accepted.expected_runs, 162);
  assert.equal(accepted.completed_runs, 162);

  const failedCommandTelemetry = exactCandidateRows();
  failedCommandTelemetry[0].transcript_analysis.interaction_turns.failed_tool_actions = 1;
  assert.equal(
    benchmarkHarness.exactCandidateAcceptance(failedCommandTelemetry, exactLifecycle()).pass,
    true,
    "a reconciled failed command is telemetry, not an incomplete run",
  );

  const mutations = [
    ["complete rows", (rows) => rows.pop(), /162 complete runs/i],
    ["candidate quality vs published", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").quality.pass = false;
    }, /candidate quality.*published/i],
    ["candidate quality vs baseline", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) row.quality.pass = false;
    }, /candidate quality.*without_codestory/i],
    ["candidate-only factual error", (rows) => {
      const row = rows.find((entry) => entry.arm === "candidate_0_18");
      row.quality.material_factual_errors = { found: 1, found_anchors: ["false fact"] };
    }, /candidate-only material factual error/i],
    ["unsupported proof", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").quality.unsupported_proof_claims = { found: 1, found_claims: ["ContractProven"] };
    }, /unsupported proof claim/i],
    ["task repeat loss", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18" && entry.task_id === EXACT_TASKS[0][0] && entry.repeat <= 2)) row.quality.pass = false;
      rows.find((entry) => entry.arm === "without_codestory" && entry.task_id === EXACT_TASKS[0][0]).quality.pass = false;
    }, /loses 2 repeats/i],
    ["tokens vs published", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) {
        row.usage.input_tokens = 54;
        row.usage.total_tokens = 74;
      }
    }, /tokens.*105%/i],
    ["tokens vs baseline", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) {
        row.usage.input_tokens = 61;
        row.usage.total_tokens = 81;
      }
      for (const row of rows.filter((entry) => entry.arm === "published_0_17_4")) {
        row.usage.input_tokens = 60;
        row.usage.total_tokens = 80;
      }
    }, /tokens.*80%/i],
    ["tools", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) {
        row.tool_calls_observed = 8;
        row.transcript_analysis.tool_categories.command_execution = 8;
      }
    }, /tool calls/i],
    ["cost", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) row.estimated_cost_usd = 0.81;
    }, /cost/i],
    ["warm", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) {
        row.exact_candidate_timing.warm_ms = 53;
        row.exact_candidate_timing.all_in_ms = 53;
      }
    }, /warm.*105%/i],
    ["cold", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) row.exact_candidate_timing.cold_ms = 106;
    }, /cold.*5%/i],
    ["incremental", (rows) => {
      for (const row of rows.filter((entry) => entry.arm === "candidate_0_18")) row.exact_candidate_timing.incremental_ms = 22;
    }, /incremental.*5%/i],
    ["row all-in mismatch", (rows) => {
      rows.find((entry) => entry.arm === "candidate_0_18").exact_candidate_timing.all_in_ms = 89;
    }, /row all-in timing/i],
    ["source authorization", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").transcript_analysis.direct_source_reads[0].authorization = { status: "unauthorized", reason: null };
    }, /unauthorized direct source read/i],
    ["forged source authorization", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").transcript_analysis.direct_source_reads[0].authorization = { status: "authorized", reason: "reviewer_said_ok" };
    }, /unauthorized direct source read/i],
    ["identity", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").source_cli_identity.cli_sha256 = "0".repeat(64);
    }, /candidate source\/CLI identity mismatch/i],
    ["fabricated legacy discovery identity", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").package_identity.discovery_contract_sha256 = "9".repeat(64);
    }, /published package identity mismatch/i],
    ["missing candidate discovery identity", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").source_cli_identity.discovery_contract_sha256 = null;
    }, /candidate source\/CLI identity mismatch/i],
    ["accounting", (rows) => {
      delete rows.find((row) => row.arm === "candidate_0_18").transcript_analysis.tool_categories;
    }, /missing tool categories/i],
    ["arbitrary task id", (rows) => {
      rows[0].task_id = "invented-task";
    }, /exact task\/repository\/arm\/repeat keys/i],
    ["arbitrary repository id", (rows) => {
      rows[0].repo = "invented-repository";
    }, /exact task\/repository\/arm\/repeat keys/i],
    ["null token telemetry", (rows) => {
      rows[0].usage.total_tokens = null;
    }, /token accounting/i],
    ["negative category telemetry", (rows) => {
      rows[0].transcript_analysis.tool_categories.command_execution = -1;
      rows[0].tool_calls_observed = -1;
    }, /tool or command categories|tool call or cost/i],
    ["empty category telemetry", (rows) => {
      rows[0].transcript_analysis.tool_categories = {};
    }, /tool or command categories/i],
    ["failed action overcount", (rows) => {
      rows[0].transcript_analysis.interaction_turns.failed_tool_actions =
        rows[0].transcript_analysis.interaction_turns.tool_actions + 1;
    }, /interaction accounting/i],
    ["factual count mismatch", (rows) => {
      rows[0].quality.material_factual_errors.found = 1;
    }, /error or proof-claim counts/i],
    ["timing repeat mismatch", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").exact_candidate_timing.cold_ms = 99;
    }, /timing disagrees across repeats/i],
    ["baseline CodeStory visibility", (rows) => {
      rows.find((row) => row.arm === "without_codestory").transcript_analysis.command_categories.codestory_cli = 1;
    }, /baseline has CodeStory visibility/i],
    ["published runtime proof", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").codestory_harness_prelude.packet_contract_runtime = null;
    }, /missing per-arm exact runtime proof/i],
    ["candidate cache proof", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance = null;
    }, /missing per-arm cache proof/i],
    ["published obligation proof", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").codestory_harness_prelude.packet_sufficiency = null;
    }, /missing per-arm obligation proof/i],
    ["candidate v3 evidence gap proof", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_harness_prelude.packet_evidence_gap_accounting = null;
    }, /missing per-arm v3 evidence\/gap proof/i],
    ["candidate source mutation proof", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_source_mutation = null;
    }, /missing verified source mutation/i],
    ["candidate source mutation digest", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_source_mutation.original_sha256 = null;
    }, /missing verified source mutation/i],
    ["candidate cold work evidence", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.cold_retrieval_work_evidence = null;
    }, /missing candidate cold retrieval work evidence/i],
    ["candidate incremental work evidence", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_retrieval_work_evidence = null;
    }, /missing candidate incremental retrieval work evidence/i],
    ["candidate incremental complete rebuild", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_retrieval_work_evidence.retrieval_component_work[1].mode = "complete";
    }, /candidate incremental retrieval rebuilt vectors completely/i],
    ["candidate incremental duplicate component", (rows) => {
      const evidence = rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_retrieval_work_evidence;
      evidence.retrieval_component_work[2].component = "vectors";
    }, /candidate incremental retrieval component roster/i],
    ["candidate incremental invalid component work", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_retrieval_work_evidence.retrieval_component_work[0].retained = -1;
    }, /candidate incremental retrieval component work/i],
    ["candidate incremental missing phase timing", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.incremental_retrieval_work_evidence.retrieval_phase_timings = [];
    }, /candidate incremental retrieval phase timings/i],
    ["cache timing mismatch", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").codestory_cache_provenance.cache_preparation.incremental_wall_ms = 19;
    }, /cache lifecycle timings do not reconcile/i],
    ["cross-arm coherence", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_cache_provenance.cache_preparation.coherence_semantic_generation = "stale";
    }, /cross-arm cache coherence/i],
    ["executed CLI substitution", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").codestory_prelude_cli_sha256 = "9".repeat(64);
    }, /executed CLI is not bound/i],
    ["zero trust root", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").package_identity.trust_root_sha256 = "0".repeat(64);
    }, /published package identity mismatch/i],
    ["malformed JSONL", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").malformed_stdout_lines = 1;
    }, /malformed or unreconciled JSONL parser telemetry/i],
    ["external web context", (rows) => {
      rows.find((row) => row.arm === "published_0_17_4").transcript_analysis.external_context_tool_calls = 1;
    }, /external web\/search context is forbidden/i],
    ["zero baseline local commands", (rows) => {
      const row = rows.find((entry) => entry.arm === "without_codestory");
      row.transcript_analysis.command_count = 0;
      row.transcript_analysis.command_categories = {};
    }, /baseline local inspection telemetry is incomplete/i],
    ["packet-first failure", (rows) => {
      rows.find((row) => row.arm === "candidate_0_18").packet_first_pass = false;
    }, /packet-first contract failed/i],
    ["dirty repo provenance", (rows) => {
      rows[0].repo_provenance.git_dirty = true;
    }, /owning repo provenance.*dirty/i],
    ["moving repo provenance", (rows) => {
      rows[0].repo_provenance.configured.ref = "main";
    }, /owning repo provenance.*not pinned/i],
  ];
  for (const [label, mutate, expected] of mutations) {
    const rows = exactCandidateRows();
    mutate(rows);
    const result = benchmarkHarness.exactCandidateAcceptance(rows, exactLifecycle());
    assert.equal(result.pass, false, `${label}: ${JSON.stringify(result)}`);
    assert.match(result.reasons.join("\n"), expected, label);
  }

  for (const [label, mutate, expected] of [
    ["missing lifecycle", () => null, /one-time package and model lifecycle/i],
    ["missing model lifecycle", (lifecycle) => {
      delete lifecycle.model_initialization_ms;
      return lifecycle;
    }, /one-time package and model lifecycle/i],
    ["unbalanced lifecycle", (lifecycle) => {
      for (const entry of lifecycle.preparation_order) {
        entry.arms = ["published_0_17_4", "candidate_0_18"];
      }
      return lifecycle;
    }, /balanced deterministic 9\/9 rotation/i],
    ["all-in lifecycle", (lifecycle) => {
      lifecycle.package_authentication_ms.candidate_0_18 = 1000;
      return lifecycle;
    }, /all-in timing exceeds 110%/i],
    ["missing cost rates", (lifecycle) => {
      delete lifecycle.cost_rates;
      return lifecycle;
    }, /cost rates are missing/i],
  ]) {
    const lifecycle = mutate(exactLifecycle());
    const result = benchmarkHarness.exactCandidateAcceptance(exactCandidateRows(), lifecycle);
    assert.equal(result.pass, false, `${label}: ${JSON.stringify(result)}`);
    assert.match(result.reasons.join("\n"), expected, label);
  }
});

test("retrieval index work evidence preserves the measured trust-boundary fields", () => {
  const evidence = benchmarkHarness.retrievalIndexWorkEvidence(JSON.stringify({
    unrelated: "discarded",
    core_phase_timings: { publish_ms: 7 },
    retrieval_phase_timings: [{ phase: "graph artifact", elapsed_ms: 3 }],
    retrieval_component_work: [
      { component: "graph", mode: "copy_on_write", retained: 9, inserted: 1, removed: 1 },
    ],
  }, null, 2));
  assert.deepEqual(evidence, {
    core_phase_timings: { publish_ms: 7 },
    retrieval_phase_timings: [{ phase: "graph artifact", elapsed_ms: 3 }],
    retrieval_component_work: [
      { component: "graph", mode: "copy_on_write", retained: 9, inserted: 1, removed: 1 },
    ],
  });
  assert.equal(benchmarkHarness.retrievalIndexWorkEvidence("not json"), null);
});

test("exact lifecycle alternates preparation and restores the selected source bytes", async () => {
  assert.deepEqual(benchmarkHarness.exactCandidatePreparationArmOrder(0), [
    "published_0_17_4", "candidate_0_18",
  ]);
  assert.deepEqual(benchmarkHarness.exactCandidatePreparationArmOrder(1), [
    "candidate_0_18", "published_0_17_4",
  ]);
  const firstArms = Array.from({ length: 18 }, (_, index) =>
    benchmarkHarness.exactCandidatePreparationArmOrder(index)[0]
  );
  assert.equal(firstArms.filter((arm) => arm === "published_0_17_4").length, 9);
  assert.equal(firstArms.filter((arm) => arm === "candidate_0_18").length, 9);

  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-exact-mutation-"));
  const sourcePath = path.join(root, "source.ts");
  const original = Buffer.from("export function run() {}\n", "utf8");
  await writeFile(sourcePath, original);
  const spy = [];
  try {
    const receipt = await benchmarkHarness.withExactSourceMutation(
      sourcePath,
      async ({ original_sha256, mutated_sha256 }) => {
        spy.push("incremental");
        assert.notEqual(mutated_sha256, original_sha256);
        assert.notDeepEqual(await readFile(sourcePath), original);
        return { status: "pass" };
      },
      async ({ original_sha256, restored_sha256 }) => {
        spy.push("restore");
        assert.equal(restored_sha256, original_sha256);
        assert.deepEqual(await readFile(sourcePath), original);
      },
    );
    assert.deepEqual(spy, ["incremental", "restore"]);
    assert.equal(receipt.result.status, "pass");
    assert.deepEqual(await readFile(sourcePath), original);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("final exact-candidate coherence refresh overwrites stale cross-arm publication state", async () => {
  const task = { repo: "psf-requests", project: "/repo" };
  const opts = {
    exactCandidate: true,
    exactCandidateStateRoot: "/tmp/exact-state",
    exactCandidatePackageByArm: new Map([
      ["candidate_0_18", { cli_path: "/authenticated/candidate-cli" }],
    ]),
    prepareCodestoryTimeoutMs: 60_000,
    signal: null,
    packetRuntimeChildEnv: {},
  };
  const row = {
    project: "/repo",
    retrieval_status: { semantic_generation: "stale-generation" },
  };
  const calls = [];
  await benchmarkHarness.refreshExactCandidatePreparation(
    opts,
    task,
    "candidate_0_18",
    row,
    {
      run: async (command, args) => {
        calls.push([command, ...args]);
        return { status: "pass", exitCode: 0, stdout: "", stderr: "" };
      },
      statusSnapshot: async () => ({
        status: "pass",
        retrieval_mode: "full",
        degraded_reason: null,
        semantic_generation: "fresh-generation",
      }),
      engineSnapshot: async () => ({
        status: "pass",
        retrieval_mode: "full",
        degraded_reason: null,
        engine: {},
        server: null,
      }),
      doctorSnapshot: async () => ({ freshness_status: "fresh" }),
    },
  );
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], "/authenticated/candidate-cli");
  assert.equal(row.retrieval_status.semantic_generation, "fresh-generation");
  assert.equal(row.coherence_refresh_status, "pass");
  assert.equal(row.coherence_semantic_generation, "fresh-generation");
});

test("exact CodeStory arms use disjoint embedding-server qualification namespaces", () => {
  const stateRoot = path.join(os.tmpdir(), "codestory-exact-state");
  const opts = {
    exactCandidate: true,
    exactCandidateStateRoot: stateRoot,
  };
  const published = benchmarkHarness.exactCandidateArmEnv(opts, "published_0_17_4");
  const candidate = benchmarkHarness.exactCandidateArmEnv(opts, "candidate_0_18");

  for (const [arm, env] of [
    ["published_0_17_4", published],
    ["candidate_0_18", candidate],
  ]) {
    assert.equal(
      env.CODESTORY_EMBED_QUALIFICATION_DIR,
      path.join(stateRoot, arm, "embedding-qualification"),
    );
    assert.match(env.CODESTORY_EMBED_QUALIFICATION_NONCE, /^[A-Za-z0-9_-]+$/);
  }
  assert.notEqual(
    published.CODESTORY_EMBED_QUALIFICATION_DIR,
    candidate.CODESTORY_EMBED_QUALIFICATION_DIR,
  );
  assert.notEqual(
    published.CODESTORY_EMBED_QUALIFICATION_NONCE,
    candidate.CODESTORY_EMBED_QUALIFICATION_NONCE,
  );
});

test("exact candidate private state roots use their canonical native path", async () => {
  const root = await benchmarkHarness.createExactCandidatePrivateStateRoot(
    "codestory-exact-canonical-state-",
  );
  try {
    assert.equal(root, await realpath(root));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("exact candidate setup removes every allocated root after a later setup failure", async () => {
  const parent = await realpath(await mkdtemp(path.join(os.tmpdir(), "codestory-exact-setup-")));
  const allocated = [];
  const opts = { exactCandidate: true };
  try {
    await assert.rejects(
      benchmarkHarness.initializeExactCandidateState(opts, {
        createPrivateStateRoot: async (prefix) => {
          const root = await mkdtemp(path.join(parent, prefix));
          allocated.push(root);
          return root;
        },
        makeDirectory: async () => {
          throw new Error("synthetic later setup failure");
        },
        authenticatePackages: async () => {
          throw new Error("authentication must not run");
        },
      }),
      /synthetic later setup failure/,
    );
    assert.equal(allocated.length, 2);
    for (const root of allocated) assert.equal(existsSync(root), false);
    assert.equal(opts.exactCandidateStateRoot, undefined);
    assert.equal(opts.exactCandidateBaselineContainerRoot, undefined);
    assert.equal(opts.exactCandidateBaselineStateRoot, undefined);
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test("exact candidate setup preserves its primary failure across every cleanup failure", async () => {
  for (const failedRoots of [
    new Set(["/fixture/baseline-state"]),
    new Set(["/fixture/exact-state"]),
    new Set(["/fixture/baseline-state", "/fixture/exact-state"]),
  ]) {
    const opts = { exactCandidate: true };
    const primary = new Error("primary authentication failure");
    const initializationRemovals = [];
    let allocation = 0;
    let observed;
    try {
      await benchmarkHarness.initializeExactCandidateState(opts, {
        createPrivateStateRoot: async () => [
          "/fixture/exact-state",
          "/fixture/baseline-state",
        ][allocation++],
        makeDirectory: async () => {},
        authenticatePackages: async () => {
          throw primary;
        },
        remove: async (root, options) => {
          initializationRemovals.push({ root, options });
          if (failedRoots.has(root)) {
            const error = new Error(`cleanup failed for ${root}`);
            error.code = "ENOTEMPTY";
            throw error;
          }
        },
      });
    } catch (error) {
      observed = error;
    }
    assert.equal(observed, primary);
    assert.deepEqual(initializationRemovals.map(({ root }) => root), [
      "/fixture/baseline-state",
      "/fixture/exact-state",
    ]);
    assert.ok(initializationRemovals.every(({ options }) => options.maxRetries >= 10));
    assert.equal(
      opts.exactCandidateBaselineContainerRoot,
      failedRoots.has("/fixture/baseline-state") ? "/fixture/baseline-state" : undefined,
    );
    assert.equal(
      opts.exactCandidateStateRoot,
      failedRoots.has("/fixture/exact-state") ? "/fixture/exact-state" : undefined,
    );

    const finalizationRemovals = [];
    const failures = await benchmarkHarness.finalizeBenchmarkResources(
      opts,
      { close: async () => {} },
      { close: async () => {} },
      {
        remove: async (root) => {
          finalizationRemovals.push(root);
        },
      },
    );
    assert.deepEqual(finalizationRemovals, [...failedRoots].reverse());
    assert.deepEqual(
      new Set(failures.map(({ path }) => path)),
      failedRoots,
    );
    assert.equal(benchmarkHarness.finalBenchmarkFailure(primary, failures), primary);
  }
});

test("durable JSONL appender closes its handle after every pending failure", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-ledger-close-"));
  try {
    for (const stage of ["serialization", "write", "sync"]) {
      let closeCalls = 0;
      const stageFailure = new Error(`${stage} failed`);
      const closeFailure = new Error(`${stage} close failed`);
      const appender = await createDurableJsonlAppender(
        path.join(root, `${stage}.jsonl`),
        {
          openFile: async () => ({
            write: async () => {
              if (stage === "write") throw stageFailure;
            },
            sync: async () => {
              if (stage === "sync") throw stageFailure;
            },
            close: async () => {
              closeCalls += 1;
              throw closeFailure;
            },
          }),
        },
      );
      const row = {};
      if (stage === "serialization") row.self = row;
      let appendFailure;
      try {
        await appender.append(row);
      } catch (error) {
        appendFailure = error;
      }
      let observedCloseFailure;
      try {
        await appender.close();
      } catch (error) {
        observedCloseFailure = error;
      }
      assert.equal(closeCalls, 1);
      assert.equal(observedCloseFailure, appendFailure);
      assert.deepEqual(observedCloseFailure.benchmarkSecondaryFailures, [{
        resource: "ledger_handle",
        code: null,
        message: closeFailure.message,
      }]);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("benchmark finalization retries owned roots and preserves the pipeline failure", async () => {
  const removed = [];
  const opts = {
    exactCandidateStateRoot: "/fixture/exact-state",
    exactCandidateBaselineContainerRoot: "/fixture/baseline-state",
    exactCandidateBaselineStateRoot: "/fixture/baseline-state/private-state",
    exactCandidatePackageByArm: new Map(),
    exactCandidateLifecycle: {},
  };
  const pipelineFailure = { kind: "preparation_failed", repo: "fmtlib-fmt" };
  const failures = await benchmarkHarness.finalizeBenchmarkResources(
    opts,
    { close: async () => {} },
    { close: async () => {} },
    {
      remove: async (root, options) => {
        removed.push({ root, options });
        if (root.endsWith("exact-state")) {
          const error = new Error("directory not empty");
          error.code = "ENOTEMPTY";
          throw error;
        }
      },
    },
  );

  assert.deepEqual(removed.map(({ root }) => root), [
    "/fixture/exact-state",
    "/fixture/baseline-state",
  ]);
  for (const { options } of removed) {
    assert.equal(options.recursive, true);
    assert.equal(options.force, true);
    assert.ok(options.maxRetries >= 10);
    assert.ok(options.retryDelay >= 100);
  }
  assert.deepEqual(failures, [{
    resource: "exact_candidate_state",
    path: "/fixture/exact-state",
    code: "ENOTEMPTY",
    message: "directory not empty",
  }]);
  assert.equal(
    benchmarkHarness.finalBenchmarkFailure(pipelineFailure, failures),
    pipelineFailure,
  );
  assert.equal(opts.exactCandidateStateRoot, undefined);
  assert.equal(opts.exactCandidateBaselineContainerRoot, undefined);
  assert.equal(opts.exactCandidateBaselineStateRoot, undefined);
  assert.ok(opts.exactCandidatePackageByArm instanceof Map);
  assert.deepEqual(opts.exactCandidateLifecycle, {});
});

test("benchmark finalization closes and cleans every resource after independent failures", async () => {
  const events = [];
  const opts = {
    exactCandidateStateRoot: "/fixture/exact-state",
    exactCandidateBaselineContainerRoot: "/fixture/baseline-state",
  };
  const failures = await benchmarkHarness.finalizeBenchmarkResources(
    opts,
    {
      close: async () => {
        events.push("runs");
        const error = new Error("runs close failed");
        error.benchmarkSecondaryFailures = [{
          resource: "ledger_handle",
          code: "EIO",
          message: "runs descriptor close failed",
        }];
        throw error;
      },
    },
    {
      close: async () => {
        events.push("preparations");
        throw new Error("preparations close failed");
      },
    },
    {
      remove: async (root) => {
        events.push(root);
        if (root.endsWith("exact-state")) throw new Error("exact cleanup failed");
      },
    },
  );

  assert.deepEqual(events, [
    "runs",
    "preparations",
    "/fixture/exact-state",
    "/fixture/baseline-state",
  ]);
  assert.deepEqual(failures.map(({ resource }) => resource), [
    "runs_ledger",
    "runs_ledger.ledger_handle",
    "preparations_ledger",
    "exact_candidate_state",
  ]);
  assert.equal(benchmarkHarness.finalBenchmarkFailure(null, failures).kind, "cleanup_failed");
});

test("exact Codex isolation keeps scalar namespace credentials out of cache roots and cwd", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-exact-isolation-"));
  const sourceCodexHome = path.join(root, "source-codex-home");
  const stateRoot = path.join(root, "state");
  const baselineRoot = path.join(root, "baseline", "private-state");
  const outDir = path.join(root, "out");
  await mkdir(sourceCodexHome, { recursive: true });
  await mkdir(baselineRoot, { recursive: true });
  await mkdir(outDir, { recursive: true });
  await writeFile(path.join(sourceCodexHome, "auth.json"), "{}\n");
  const harnessUrl = new URL("../codestory-agent-ab-benchmark.mjs", import.meta.url).href;
  const script = `
    const benchmark = await import(${JSON.stringify(harnessUrl)});
    const opts = {
      exactCandidate: true,
      exactCandidateStateRoot: ${JSON.stringify(stateRoot)},
      exactCandidateBaselineStateRoot: ${JSON.stringify(baselineRoot)},
      exactCandidatePackageByArm: new Map([
        ["published_0_17_4", { cli_path: process.execPath }],
        ["candidate_0_18", { cli_path: process.execPath }],
      ]),
      model: "gpt-5.6-sol",
    };
    const result = await benchmark.prepareAgentCodexIsolation(${JSON.stringify(outDir)}, opts);
    process.stdout.write(JSON.stringify(result.receipt));
  `;
  try {
    const child = await runProcess(process.execPath, ["--input-type=module", "-e", script], {
      cwd: root,
      env: { ...process.env, CODEX_HOME: sourceCodexHome },
      timeoutMs: 10_000,
    });
    assert.equal(child.status, "pass", child.stderr);
    const receipt = JSON.parse(child.stdout);
    const rootEntries = await readdir(root);
    assert.equal(rootEntries.includes("agent-benchmark-published_0_17_4"), false);
    assert.equal(rootEntries.includes("agent-benchmark-candidate_0_18"), false);
    for (const arm of ["published_0_17_4", "candidate_0_18"]) {
      assert.equal(
        Object.hasOwn(receipt.cache_roots[arm], "CODESTORY_EMBED_QUALIFICATION_NONCE"),
        false,
      );
      assert.equal(
        receipt.embedding_server_namespaces[arm].nonce,
        `agent-benchmark-${arm}`,
      );
      assert.match(
        receipt.embedding_server_namespaces[arm].qualification_directory,
        /embedding-qualification$/,
      );
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("exact baseline child gets an allowlisted disjoint environment with no CodeStory surface", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "agent-exact-baseline-env-"));
  const codeStoryRoot = await mkdtemp(path.join(os.tmpdir(), "codestory-exact-arm-env-"));
  try {
    const baselineContainer = path.join(root, "baseline-container");
    const baselineRoot = path.join(baselineContainer, "private-state");
    const exposedBin = path.join(root, "exposed-bin");
    await mkdir(baselineRoot, { recursive: true });
    await mkdir(codeStoryRoot, { recursive: true });
    await mkdir(exposedBin, { recursive: true });
    await writeFile(path.join(exposedBin, "codestory-cli"), "not executable");
    const baselineEnv = benchmarkHarness.exactCandidateBaselineEnv({
      exactCandidate: true,
      exactCandidateBaselineStateRoot: baselineRoot,
      exactCandidateStateRoot: codeStoryRoot,
    }, {
      ...process.env,
      PATH: `${exposedBin}${path.delimiter}${process.env.PATH ?? ""}`,
      CODESTORY_CLI: path.join(exposedBin, "codestory-cli"),
      CODESTORY_CACHE_ROOT: path.join(codeStoryRoot, "cache"),
      CODESTORY_PLUGIN_DATA: path.join(codeStoryRoot, "plugin"),
    });
    for (const directory of [
      baselineEnv.HOME, baselineEnv.TMPDIR, baselineEnv.XDG_CACHE_HOME,
      baselineEnv.XDG_CONFIG_HOME, baselineEnv.XDG_DATA_HOME,
    ]) {
      await mkdir(directory, { recursive: true });
    }
    const childEnv = agentRunnerEnv(baselineEnv, path.join(baselineRoot, "host"), false);
    await mkdir(childEnv.CODEX_HOME, { recursive: true });
    const child = await runProcess(process.execPath, ["-e", `
      const fs = require("node:fs");
      const path = require("node:path");
      const entries = String(process.env.PATH || "").split(path.delimiter).filter(Boolean);
      const executableHits = entries.filter((entry) =>
        ["codestory", "codestory-cli", "codestory.exe", "codestory-cli.exe", "codestory.cmd"]
          .some((name) => fs.existsSync(path.join(entry, name)))
      );
      const readableParent = path.dirname(path.dirname(process.env.HOME));
      process.stdout.write(JSON.stringify({
        env: process.env,
        executableHits,
        readableParent,
        parentEntries: fs.readdirSync(readableParent),
      }));
    `], { env: childEnv, timeoutMs: 10_000 });
    assert.equal(child.status, "pass", child.stderr);
    const observed = JSON.parse(child.stdout);
    assert.deepEqual(observed.executableHits, []);
    assert.equal(Object.keys(observed.env).some((key) => key.startsWith("CODESTORY_")), false);
    assert.equal(
      Object.values(observed.env).some((value) => String(value).includes(codeStoryRoot)),
      false,
    );
    assert.equal(
      Object.values(observed.env).some((value) => /codestory/i.test(String(value))),
      false,
    );
    assert.deepEqual(observed.parentEntries, ["private-state"]);
    assert.equal(observed.env.PATH.includes(exposedBin), false);
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(codeStoryRoot, { recursive: true, force: true });
  }
});

test("keeps CLI overrides out of both isolated agent arms", () => {
  const opts = {
    runner: "codex",
    sandbox: "read-only",
    model: "gpt-5.6-sol",
  };
  const baseline = runnerCommand(opts, "/tmp/repo", "prompt", "without_codestory");
  const measured = runnerCommand(opts, "/tmp/repo", "prompt", "with_codestory");
  assert.ok(baseline.args.includes("--ignore-user-config"));
  assert.ok(measured.args.includes("--ignore-user-config"));
  assert.deepEqual(baseline.args, measured.args);
  for (const setting of [
    'approval_policy="never"',
    'model_reasoning_effort="xhigh"',
    'service_tier="default"',
    'personality="pragmatic"',
    'model_verbosity="low"',
  ]) {
    assert.ok(measured.args.includes(setting), `missing pinned runner setting ${setting}`);
  }

  const env = agentRunnerEnv(
    {
      CODESTORY_CLI: "/tmp/stale-codestory-cli",
      CODESTORY_EMBED_ALLOW_CPU: "0",
    },
    "/tmp/isolated-codex-home",
  );
  assert.equal(env.CODESTORY_CLI, undefined);
  assert.equal(env.CODESTORY_RETRIEVAL, "1");
  assert.equal(env.CODEX_HOME, "/tmp/isolated-codex-home");
});

test("groups cold packet-runtime jobs by repo", () => {
  const expressRouting = { repo: "express", id: "express-routing" };
  const muxRouting = { repo: "mux", id: "mux-routing" };
  const expressResponse = { repo: "express", id: "express-response" };

  const groups = groupPacketRuntimeColdJobs(
    [expressRouting, muxRouting, expressResponse],
    3,
  );

  assert.deepEqual(
    groups.map(({ repo, jobs }) => ({
      repo,
      jobs: jobs.map(({ task, repeat }) => `${task.id}:${repeat}`),
    })),
    [
      {
        repo: "express",
        jobs: [
          "express-routing:1",
          "express-routing:2",
          "express-routing:3",
          "express-response:1",
          "express-response:2",
          "express-response:3",
        ],
      },
      {
        repo: "mux",
        jobs: ["mux-routing:1", "mux-routing:2", "mux-routing:3"],
      },
    ],
  );
});

test("parses packet-runtime benchmark run id", () => {
  const opts = parseBenchmarkArgs([
    "--packet-runtime",
    "--task-suite",
    "local-real",
    "--benchmark-run-id",
    "segment 43/v2",
    "--prepare-codestory-jobs",
    "2",
  ]);

  assert.equal(opts.packetRuntime, true);
  assert.equal(opts.benchmarkRunId, "segment-43-v2");
  assert.equal(opts.prepareCodestoryCache, true);
  assert.equal(opts.prepareCodestoryJobs, 2);
  assert.throws(
    () =>
      parseBenchmarkArgs([
        "--packet-runtime",
        "--task-suite",
        "local-real",
        "--no-prepare-codestory-cache",
      ]),
    /retrieval preparation is mandatory/,
  );
  assert.throws(
    () =>
      parseBenchmarkArgs([
        "--packet-runtime",
        "--task-suite",
        "local-real",
        "--prepare-codestory-jobs",
        "0",
      ]),
    /--prepare-codestory-jobs must be a positive integer/,
  );
});

test("defaults preparation concurrency to two and validates deterministic shard options", () => {
  const opts = parseBenchmarkArgs([
    "--task-suite",
    "language-expansion-holdout",
    "--shard-count",
    "3",
    "--shard-index",
    "1",
  ]);
  assert.equal(opts.prepareCodestoryJobs, 2);
  assert.equal(opts.shardCount, 3);
  assert.equal(opts.shardIndex, 1);
  assert.throws(
    () => parseBenchmarkArgs(["--shard-count", "2", "--shard-index", "2"]),
    /--shard-index must be zero-based/,
  );
});

test("manifest declares the real Requests canary and sharding keeps whole tasks", async () => {
  const opts = {
    taskSuite: "language-expansion-holdout",
    taskManifest: null,
    taskIds: null,
    repoCacheDir: path.join(os.tmpdir(), "codestory-shard-fixture"),
    canaryTaskId: null,
  };
  const tasks = await loadTasks(opts);
  assert.equal(opts.manifestCanaryTaskId, "python-requests-session-flow");
  assert.equal(opts.canaryTaskId, "python-requests-session-flow");
  const shard = taskShardIndex(opts.canaryTaskId, 4);
  assert.ok(tasksForShard(tasks, 4, shard).some((task) => task.id === opts.canaryTaskId));
  assert.ok(!tasksForShard(tasks, 4, (shard + 1) % 4).some((task) => task.id === opts.canaryTaskId));
});

function exactPacketStdout(packet) {
  for (;;) {
    const stdout = `${JSON.stringify(packet, null, 2)}\n`;
    const bytes = Buffer.byteLength(stdout, "utf8");
    if (packet.budget.used.output_bytes === bytes) {
      return stdout;
    }
    packet.budget.used.output_bytes = bytes;
  }
}

function packetV3Fixture() {
  return {
    schema_version: 3,
    kind: "complete",
    status: "available",
    identity: {
      packet_id: "packet-v3",
      request_id: "request-v3",
      question_sha256: "1".repeat(64),
    },
    publication: {
      core: {
        project_id: "project-v3",
        generation_id: "core-v3",
        run_id: "run-v3",
      },
      retrieval: {
        core_generation_id: "core-v3",
        core_run_id: "run-v3",
        retrieval_generation: "retrieval-v3",
        retrieval_input_sha256: "2".repeat(64),
        semantic_generation: "semantic-v3",
      },
    },
    retrieval: { state: "full", generation_id: "retrieval-v3" },
    evidence: [{
      identity: { evidence_id: "evidence-v3" },
      kind: "exact_source",
      path: "src/lib.rs",
      symbol_id: "crate::run",
      start_line: 4,
      end_line: 7,
      summary: "run calls dispatch",
    }],
    gaps: [],
    continuation: null,
    diagnostics: {
      availability: "available",
      reference: {
        artifact_id: "diagnostic-v3",
        sha256: "3".repeat(64),
        byte_length: 512,
      },
    },
    _meta: {
      codestory_publication: {
        contract_runtime: {
          cli_source: "direct_cli_launch",
          cli_version: "0.17.4",
          known_override_skew_channel: false,
        },
      },
    },
  };
}

test("v3 packet validation keeps evidence authority version-native and closed", () => {
  const packet = packetV3Fixture();
  assert.deepEqual(packetPreludeContractBlockers(packet, JSON.stringify(packet), {
    requireSupported: true,
    requireManagedRuntime: false,
  }), []);
  assert.match(
    packetPreludeContractBlockers(packet, JSON.stringify(packet), {
      expectedQuestion: "different question",
    }).join("\n"),
    /question digest does not match/,
  );
  assert.equal(publicPacketPreludeContractPasses(packet, JSON.stringify(packet)), true);
  const prompt = packetForAgentPrompt(packet);
  assert.equal(prompt.schema_version, 3);
  assert.equal(prompt.status, "available");
  assert.equal(prompt.evidence.length, 1);
  assert.equal(Object.hasOwn(prompt, "support"), false);
  assert.equal(Object.hasOwn(prompt, "disposition"), false);
  assert.deepEqual(benchmarkHarness.packetV3EvidenceGapAccounting(packet), {
    contract: "codestory.packet-v3-evidence-gap-accounting/v1",
    kind: "complete",
    status: "available",
    evidence_count: 1,
    unique_evidence_id_count: 1,
    evidence_kind_counts: { exact_source: 1 },
    gap_count: 0,
    unique_gap_id_count: 0,
    gap_kind_counts: {},
    continuation_gap_count: 0,
    unique_continuation_gap_id_count: 0,
    continuation_gap_ids_bound: true,
  });

  const mutations = [
    ["missing identity", (value) => { value.evidence[0].identity.evidence_id = ""; }, /evidence identity is missing/],
    ["duplicate evidence", (value) => { value.evidence.push(structuredClone(value.evidence[0])); }, /evidence identity=.*duplicated/],
    ["duplicate gap", (value) => {
      value.status = "continuation_available";
      value.gaps = [
        { identity: { gap_id: "gap-v3" }, kind: "continuation_required", message: null },
        { identity: { gap_id: "gap-v3" }, kind: "evidence_missing", message: null },
      ];
      value.continuation = {
        continuation_id: "continuation-v3",
        remaining_rounds: 1,
        gap_ids: [{ gap_id: "gap-v3" }],
      };
    }, /gap identity=.*duplicated/],
    ["unbound continuation", (value) => {
      value.status = "continuation_available";
      value.gaps = [{ identity: { gap_id: "gap-v3" }, kind: "continuation_required", message: null }];
      value.continuation = {
        continuation_id: "continuation-v3",
        remaining_rounds: 1,
        gap_ids: [{ gap_id: "other-gap" }],
      };
    }, /continuation gap identities.*unbound/],
    ["wrong retrieval binding", (value) => {
      value.publication.retrieval.core_generation_id = "other-core";
    }, /retrieval publication is not bound/],
    ["over cap", (value) => { value.padding = "x".repeat(17_000); }, /compact bytes=.*exceeds public cap/],
  ];
  for (const [label, mutate, expected] of mutations) {
    const hostile = structuredClone(packet);
    mutate(hostile);
    assert.match(
      packetPreludeContractBlockers(hostile, JSON.stringify(hostile)).join("\n"),
      expected,
      label,
    );
  }
});

test("v3 packet continuation and budget fallback remain bounded and non-partial", () => {
  const continuation = packetV3Fixture();
  continuation.status = "continuation_available";
  continuation.gaps = [{
    identity: { gap_id: "gap-v3" },
    kind: "continuation_required",
    message: "one more bounded lookup",
  }];
  continuation.continuation = {
    continuation_id: "continuation-v3",
    remaining_rounds: 1,
    gap_ids: [{ gap_id: "gap-v3" }],
  };
  assert.deepEqual(
    drillPacketCommandArgs(
      { path: "/repo", prompt: "question" },
      { prompt: "question", task_class: "architecture_explanation" },
      {},
      continuation,
    ).slice(-8),
    [
      "--parent-packet-id", "continuation-v3",
      "--option-id", "gap-v3",
      "--core-generation-id", "core-v3",
      "--retrieval-generation", "retrieval-v3",
    ],
  );

  const fallback = packetV3Fixture();
  fallback.kind = "budget_exceeded";
  fallback.status = "unavailable";
  delete fallback.evidence;
  delete fallback.continuation;
  fallback.gaps = [{
    identity: { gap_id: "packet-output-budget-exceeded" },
    kind: "output_budget_exceeded",
    message: null,
  }];
  fallback.maximum_bytes = 16 * 1024;
  fallback.required_complete_bytes = 16 * 1024 + 1;
  assert.deepEqual(
    packetPreludeContractBlockers(fallback, JSON.stringify(fallback)),
    [],
  );
  fallback.evidence = [];
  assert.match(
    packetPreludeContractBlockers(fallback, JSON.stringify(fallback)).join("\n"),
    /contains partial evidence or continuation/,
  );
  delete fallback.evidence;
  fallback.required_complete_bytes = 16 * 1024;
  assert.match(
    packetPreludeContractBlockers(fallback, JSON.stringify(fallback)).join("\n"),
    /required_complete_bytes is invalid/,
  );
});

function managedRuntimeIdentity(overrides = {}) {
  return {
    plugin_version: "0.17.0",
    plugin_cli_version: "0.17.0",
    cli_version: "0.17.0",
    cli_source: "managed",
    pinned_pair_matches: true,
    known_override_skew_channel: false,
    ...overrides,
  };
}

test("packet canary rejects exact byte and graph-limit escapes before the agent", () => {
  const packet = {
    packet_id: "packet-1",
    _meta: {
      codestory_publication: { contract_runtime: managedRuntimeIdentity() },
    },
    plan: { obligations: { claim_obligations: [] } },
    answer: {
      citations: [{ node_id: "carrier", file_path: "src/lib.rs" }],
      graphs: Array.from(
        { length: 61 },
        () => ({ graph: { edges: [{ id: "protected" }] } }),
      ),
      retrieval_trace: {
        steps: [{ kind: "source_read", status: "ok" }],
        retrieval_shadow: { retrieval_mode: "full" },
      },
    },
    support: [{ id: "support-1", kind: "symbol_location", summary: "carrier", path: "src/lib.rs" }],
    disposition: { kind: "supported", omission_receipts: [] },
    budget: {
      limits: {
        max_anchors: 16,
        max_files: 1,
        max_output_bytes: 128 * 1024,
        max_snippets: 1,
        max_trail_edges: 60,
      },
      used: { anchors: 1, files: 1, output_bytes: 0, snippets: 1, trail_edges: 61 },
    },
  };
  const stdout = exactPacketStdout(packet);
  const blockers = packetPreludeContractBlockers(packet, stdout, {
    requireSupported: true,
    requireManagedRuntime: true,
  });
  assert.ok(blockers.some((blocker) => blocker.includes("trail_edges=61 exceeds 60")));
  assert.equal(blockers.some((blocker) => blocker.includes("stdout bytes")), false);

  packet.answer.graphs.pop();
  packet.budget.used.trail_edges = 60;
  const validStdout = exactPacketStdout(packet);
  assert.deepEqual(packetPreludeContractBlockers(packet, validStdout, {
    requireSupported: true,
    requireManagedRuntime: true,
  }), []);
  assert.deepEqual(packetDispositionTelemetry(packet, { pass: true }), {
    kind: "supported",
    terminal: true,
    reason: null,
    support_count: 1,
    support_kind_counts: { symbol_location: 1 },
    omission_receipts_count: 0,
    drill_option_count: 0,
    drill_option_ids: [],
    parent_packet_id: null,
    core_generation_id: null,
    retrieval_generation: null,
    remaining_rounds: null,
    retrieval_mode: "full",
    degraded_reason: null,
    supported_quality_mismatch: false,
  });

  for (const [field, lowered, publicCap] of [
    ["max_anchors", 15, 16],
    ["max_trail_edges", 59, 60],
    ["max_output_bytes", 120_000, 128 * 1024],
  ]) {
    const original = packet.budget.limits[field];
    packet.budget.limits[field] = lowered;
    const loweredLimitStdout = exactPacketStdout(packet);
    assert.ok(
      packetPreludeContractBlockers(packet, loweredLimitStdout, {
        requireSupported: true,
        requireManagedRuntime: true,
      }).some((blocker) =>
        blocker.includes(
          `budget.limits.${field}=${lowered} does not equal public cap=${publicCap}`,
        )
      ),
    );
    packet.budget.limits[field] = original;
  }

  packet.budget.used.files = 0;
  packet.budget.used.snippets = 0;
  const invalidStructuralCounts = exactPacketStdout(packet);
  const countBlockers = packetPreludeContractBlockers(packet, invalidStructuralCounts, {
    requireSupported: true,
    requireManagedRuntime: true,
  });
  assert.ok(countBlockers.some((blocker) => blocker.includes("unique citation files=1")));
  assert.ok(countBlockers.some((blocker) => blocker.includes("successful source reads=1")));
  packet.budget.used.files = 1;
  packet.budget.used.snippets = 1;

  packet.disposition = {
    kind: "drill_once",
    reason: "one bounded gap",
    drill: {
      parent_packet_id: "wrong-parent",
      core_generation_id: "core-1",
      options: [],
      remaining_rounds: 2,
    },
  };
  const invalidDrill = exactPacketStdout(packet);
  assert.match(
    packetPreludeContractBlockers(packet, invalidDrill, {
      requireSupported: true,
      requireManagedRuntime: true,
    }).join("\n"),
    /parent_packet_id.*option count=0.*remaining_rounds=2.*expected supported/s,
  );
  packet.disposition = { kind: "supported", omission_receipts: [] };

  packet.answer.retrieval_trace.retrieval_shadow = {
    retrieval_mode: "degraded",
    degraded_reason: "semantic unavailable",
  };
  const degradedRetrieval = exactPacketStdout(packet);
  assert.match(
    packetPreludeContractBlockers(packet, degradedRetrieval, {
      requireSupported: true,
      requireManagedRuntime: true,
    }).join("\n"),
    /retrieval shadow mode=degraded.*degraded_reason=semantic unavailable/s,
  );
  packet.answer.retrieval_trace.retrieval_shadow = { retrieval_mode: "full" };

  packet.budget.limits.max_output_bytes = 200_000;
  packet.hostile_padding = "x".repeat(140_000);
  const raisedLimitStdout = exactPacketStdout(packet);
  assert.ok(packet.budget.used.output_bytes > 128 * 1024);
  assert.match(
    packetPreludeContractBlockers(packet, raisedLimitStdout, {
      requireSupported: true,
      requireManagedRuntime: true,
    }).join("\n"),
    /max_output_bytes=200000 does not equal public cap=131072.*used\.output_bytes=.*exceeds public cap=131072/s,
  );
  delete packet.hostile_padding;
  packet.budget.limits.max_output_bytes = 128 * 1024;

  for (const [identity, selectedVersion] of [
    [managedRuntimeIdentity({ plugin_version: "0.16.3" }), "0.16.3"],
    [managedRuntimeIdentity({ cli_source: "override", known_override_skew_channel: true }), "0.17.0"],
  ]) {
    packet._meta.codestory_publication.contract_runtime = identity;
    const staleStdout = exactPacketStdout(packet);
    assert.match(
      packetPreludeContractBlockers(packet, staleStdout, {
        requireSupported: true,
        requireManagedRuntime: true,
      }).join("\n"),
      new RegExp(`runtime identity is not managed ${selectedVersion.replaceAll(".", "\\.")}`, "u"),
    );
  }
  packet._meta.codestory_publication.contract_runtime = managedRuntimeIdentity();

  packet.plan.obligations.claim_obligations.push({ material: null, proof_status: "proven" });
  const invalidObligationsStdout = exactPacketStdout(packet);
  assert.deepEqual(packetPreludeContractBlockers(packet, invalidObligationsStdout, {
    requireSupported: true,
    requireManagedRuntime: true,
  }), []);
});

test("drill continuation packets are public only when they keep the advertised budget caps", () => {
  const packet = {
    packet_id: "packet-1",
    plan: { obligations: { claim_obligations: [] } },
    answer: {
      citations: [{ node_id: "carrier", file_path: "src/lib.rs" }],
      graphs: [],
      retrieval_trace: {
        steps: [{ kind: "source_read", status: "ok" }],
        retrieval_shadow: { retrieval_mode: "full" },
      },
    },
    support: [{ id: "support-1", kind: "symbol_location", summary: "carrier", path: "src/lib.rs" }],
    disposition: { kind: "not_established", omission_receipts: [] },
    budget: {
      limits: {
        max_anchors: 16,
        max_files: 16,
        max_output_bytes: 128 * 1024,
        max_snippets: 24,
        max_trail_edges: 60,
      },
      used: { anchors: 1, files: 1, output_bytes: 0, snippets: 1, trail_edges: 0 },
    },
  };
  const stdout = exactPacketStdout(packet);
  assert.equal(publicPacketPreludeContractPasses(packet, stdout), true);

  packet.budget.limits.max_anchors = 8;
  packet.budget.limits.max_files = 8;
  packet.budget.limits.max_snippets = 8;
  packet.budget.limits.max_trail_edges = 16;
  packet.budget.limits.max_output_bytes = 32 * 1024;
  const shrunk = exactPacketStdout(packet);
  assert.equal(publicPacketPreludeContractPasses(packet, shrunk), false);
  assert.match(
    packetPreludeContractBlockers(packet, shrunk).join("\n"),
    /max_anchors=8 does not equal public cap=16/,
  );
});

test("nonzero packet prelude preserves structured retrieval failure and keeps the agent gate closed", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-packet-structured-failure-"));
  try {
    const cliPath = path.join(root, "codestory-cli");
    const failure = {
      schema_version: 1,
      error: {
        code: "retrieval_unavailable",
        message: "retrieval rejected query: sidecar retrieval trace `stage_deadline` is not eligible for primary results; stages=[stage1_lexical added=0 cancel=stage_deadline; stage1b_semantic added=0 cancel=stage_deadline]",
        details: {
          failed_layer: "retrieval_engine",
          project: root,
          next_commands: ["codestory-cli retrieval index --project fixture"],
          minimum_next: ["codestory-cli retrieval index --project fixture"],
          full_repair: ["codestory-cli retrieval index --project fixture"],
        },
      },
    };
    await writeFile(
      cliPath,
      `#!/usr/bin/env node\nprocess.stdout.write(${JSON.stringify(`${JSON.stringify(failure)}\n`)});\nprocess.exitCode = 1;\n`,
      "utf8",
    );
    await chmod(cliPath, 0o755);

    const prelude = await runCodeStoryPacketPrelude(
      {
        timeoutMs: 5_000,
        publishable: false,
        diagnosticExtraProbesFromManifest: false,
      },
      {
        task: {
          prompt: "Trace the Redis command loop.",
          task_class: "route_tracing",
        },
      },
      { path: root, prompt: "Trace the Redis command loop." },
      root,
      "redis-structured-failure",
      cliPath,
      process.env,
    );

    assert.equal(prelude.public.process_status, "fail");
    assert.equal(prelude.public.exit_code, 1);
    assert.equal(prelude.public.status, "fail");
    assert.equal(prelude.public.packet_parse_error, null);
    assert.deepEqual(prelude.public.packet_command_failure, failure);
    assert.match(prelude.public.error, /^retrieval_unavailable: retrieval rejected query:/);
    assert.match(prelude.public.error, /stages=\[stage1_lexical/);
    assert.match(prelude.public.error, /failed_layer=retrieval_engine/);
    assert.deepEqual(prelude.public.packet_contract_blockers, [prelude.public.error]);
    assert.equal(preludeAllowsAgentRun(prelude.public), false);
    assert.equal(prelude.packet, null);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("packet obligation accounting preserves the historical material split", () => {
  const materialGroups = [
    [33, { proof_status: "proven" }],
    [105, { proof_status: "reported", reason: "packet_budget_truncated" }],
    [33, { proof_status: "reported", reason: "carrier_not_sufficiency_eligible" }],
    [18, { proof_status: "reported", reason: "carrier_does_not_satisfy_role_contract" }],
    [6, { proof_status: "reported", reason: "required_evidence_edge_missing" }],
    [3, { proof_status: "unsupported", reason: "requested_claim_binding_limit_exceeded:7" }],
  ];
  const packet = {
    plan: {
      obligations: {
        claim_obligations: [
          ...materialGroups.flatMap(([count, obligation]) =>
            Array.from({ length: count }, () => ({ material: true, ...obligation }))
          ),
          ...Array.from({ length: 18 }, () => ({
            material: false,
            proof_status: "planned",
          })),
        ],
      },
    },
  };

  assert.deepEqual(packetObligationAccounting(packet), {
    total: 216,
    material: 198,
    nonmaterial: 18,
    material_status_buckets: {
      carrier_does_not_satisfy_role_contract: 18,
      carrier_not_sufficiency_eligible: 33,
      packet_budget_truncated: 105,
      proven: 33,
      requested_claim_binding_limit_exceeded: 3,
      required_evidence_edge_missing: 6,
    },
  });

  assert.deepEqual(packetObligationAccounting({
    plan: {
      obligations: {
        claim_obligations: [
          { material: true, proof_status: "reported" },
          { material: true, proof_status: "unsupported", reason: "new reason!" },
        ],
      },
    },
  }).material_status_buckets, {
    missing_reason: 1,
    "unclassified_reason:new_reason": 1,
  });
});

test("packet prelude projection preserves revision-native accounting", () => {
  const legacyPacket = {
    plan: {
      obligations: {
        claim_obligations: [
          { material: true, proof_status: "proven" },
          {
            material: true,
            proof_status: "reported",
            reason: "required_evidence_edge_missing",
          },
          { material: false, proof_status: "planned" },
        ],
      },
    },
    sufficiency: {
      status: "partial",
      covered_claims: [],
      open_next: [],
      gaps: ["missing edge"],
      follow_up_commands: [],
    },
  };
  const legacySufficiency = packetSufficiencyTelemetry(legacyPacket, { pass: false });
  const legacyPrelude = preludePublicFields({
    command: "codestory-cli packet --project . --question flow",
    packet_schema_version: 2,
    packet_sufficiency: legacySufficiency,
  });
  assert.deepEqual(legacyPrelude.packet_sufficiency?.obligation_accounting, {
    total: 3,
    material: 2,
    nonmaterial: 1,
    material_status_buckets: {
      proven: 1,
      required_evidence_edge_missing: 1,
    },
  });

  const v3Accounting = {
    contract: "codestory.packet-v3-evidence-gap-accounting/v1",
    kind: "complete",
    status: "available",
    evidence_count: 1,
    unique_evidence_id_count: 1,
    evidence_kind_counts: { exact_source: 1 },
    gap_count: 0,
    unique_gap_id_count: 0,
    gap_kind_counts: {},
    continuation_gap_count: 0,
    unique_continuation_gap_id_count: 0,
    continuation_gap_ids_bound: true,
  };
  const v3Prelude = preludePublicFields({
    command: "candidate_cli.bin packet --project . --question flow",
    packet_schema_version: 3,
    packet_sufficiency: null,
    packet_evidence_gap_accounting: v3Accounting,
  });
  assert.equal(v3Prelude.packet_sufficiency, null);
  assert.deepEqual(v3Prelude.packet_evidence_gap_accounting, v3Accounting);
});

test("saved packet reanalysis rebuilds revision-native accounting and derived acceptance", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-packet-reanalysis-"));
  const task = {
    expected_files: [],
    expected_symbols: [],
    expected_claims: [],
    forbidden_claims: [],
  };
  try {
    const legacyPath = path.join(root, "legacy.json");
    await writeFile(legacyPath, JSON.stringify({
      plan: {
        obligations: {
          claim_obligations: [
            { material: true, proof_status: "proven" },
            { material: false, proof_status: "planned" },
          ],
        },
      },
      sufficiency: {
        status: "partial",
        covered_claims: [],
        open_next: [],
        gaps: ["missing edge"],
        follow_up_commands: [],
      },
    }));
    const legacy = await reanalysisPacketProjection({
      codestory_harness_prelude: {
        stdout_path: legacyPath,
        packet_schema_version: 2,
      },
    }, root, task);
    assert.deepEqual(legacy.packet_sufficiency?.obligation_accounting, {
      total: 2,
      material: 1,
      nonmaterial: 1,
      material_status_buckets: { proven: 1 },
    });
    assert.equal(legacy.packet_evidence_gap_accounting, null);
    const legacyWithoutTask = await reanalysisPacketProjection({
      codestory_harness_prelude: {
        stdout_path: legacyPath,
        packet_schema_version: 2,
      },
    }, root, null);
    assert.deepEqual(
      legacyWithoutTask.packet_sufficiency?.obligation_accounting,
      legacy.packet_sufficiency.obligation_accounting,
    );
    assert.equal(legacyWithoutTask.packet_manifest_quality, null);

    const v3Path = path.join(root, "v3.json");
    await writeFile(v3Path, JSON.stringify({
      schema_version: 3,
      kind: "complete",
      status: "available",
      evidence: [
        { identity: { evidence_id: "evidence-1" }, kind: "exact_source" },
      ],
      gaps: [
        { identity: { gap_id: "gap-1" }, kind: "evidence_missing" },
      ],
      continuation: null,
    }));
    const v3 = await reanalysisPacketProjection({
      codestory_harness_prelude: {
        stdout_path: v3Path,
        packet_schema_version: 3,
      },
    }, root, task);
    assert.equal(v3.packet_sufficiency, null);
    assert.equal(v3.packet_evidence_gap_accounting.evidence_count, 1);
    assert.equal(v3.packet_evidence_gap_accounting.gap_count, 1);
    const v3WithoutTask = await reanalysisPacketProjection({
      codestory_harness_prelude: {
        stdout_path: v3Path,
        packet_schema_version: 3,
      },
    }, root, null);
    assert.equal(v3WithoutTask.packet_evidence_gap_accounting.evidence_count, 1);
    assert.equal(v3WithoutTask.packet_evidence_gap_accounting.gap_count, 1);
    assert.equal(v3WithoutTask.packet_manifest_quality, null);

    const refreshed = reanalysisExactCandidateAcceptance({
      exact_candidate_acceptance: { stale: true },
      exact_candidate_lifecycle: exactLifecycle(),
    }, exactCandidateRows());
    assert.equal(refreshed.stale, undefined);
    assert.equal(refreshed.pass, true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("packet obligation accounting rejects unreconciled summaries", () => {
  const row = (accounting) => ({
    repo: "fixture",
    task_id: "generic-flow",
    mode: "cold_cli_packet",
    repeat: 1,
    status: "pass",
    sufficiency: {
      status: "partial",
      obligation_accounting: accounting,
    },
  });
  assert.throws(
    () => summarizePacketObligationAccounting([
      row({
        total: 215,
        material: 198,
        nonmaterial: 18,
        material_status_buckets: { proven: 198 },
      }),
    ], "benchmark summary"),
    /total=215 does not reconcile with material=198 \+ nonmaterial=18/,
  );
  assert.throws(
    () => summarizePacketObligationAccounting([
      row({
        total: 216,
        material: 198,
        nonmaterial: 18,
        material_status_buckets: { proven: 197 },
      }),
    ], "benchmark summary"),
    /material=198 does not reconcile with material status buckets=197/,
  );
  assert.equal(
    summarizePacketObligationAccounting([
      { repo: "fixture", task_id: "baseline", arm: "without_codestory", repeat: 1 },
    ], "benchmark summary"),
    null,
  );
  for (const missing of [
    { repo: "fixture", task_id: "measured", arm: "with_codestory", repeat: 1, status: "pass" },
    { repo: "fixture", task_id: "runtime", mode: "cold_cli_packet", repeat: 1, status: "pass" },
    { repo: "fixture", task_id: "legacy", arm: "with_codestory", repeat: 1 },
  ]) {
    assert.throws(
      () => summarizePacketObligationAccounting([missing], "benchmark summary"),
      /packet obligation accounting is missing/,
    );
  }
  for (const status of ["cancelled", "fail", "failed", "partial"]) {
    assert.equal(
      summarizePacketObligationAccounting([{
        repo: "fixture",
        task_id: status,
        arm: "with_codestory",
        repeat: 1,
        status,
      }], "benchmark summary"),
      null,
      `${status} rows that emitted no packet have no packet accounting to report`,
    );
  }
  for (const emitted of [
    { sufficiency: { status: "partial" } },
    { packet_shape: {} },
    { codestory_harness_prelude: { packet_sufficiency: { status: "partial" } } },
  ]) {
    assert.throws(
      () => summarizePacketObligationAccounting([{
        repo: "fixture",
        task_id: "failed-packet",
        mode: "cold_cli_packet",
        repeat: 1,
        status: "fail",
        ...emitted,
      }], "benchmark summary"),
      /packet obligation accounting is missing/,
      "a failed row with evidence that its packet path began cannot omit accounting",
    );
  }
  assert.equal(
    summarizePacketObligationAccounting([{
      repo: "fixture",
      task_id: "aborted-before-packet",
      arm: "with_codestory",
      repeat: 1,
      status: "cancelled",
      response_bytes: 0,
      codestory_harness_prelude: {
        status: "fail",
        process_status: "aborted",
        stdout_bytes: 0,
        packet_parse_error: null,
        packet_sufficiency_status: null,
        packet_sufficiency: null,
      },
    }], "benchmark summary"),
    null,
    "an aborted prelude with no parsed packet has no packet accounting to report",
  );
});

test("packet obligation accounting aggregates valid failed packets and skips cancelled omissions", () => {
  const accounting = (proven, reported = 0) => ({
    total: proven + reported,
    material: proven + reported,
    nonmaterial: 0,
    material_status_buckets: {
      ...(proven ? { proven } : {}),
      ...(reported ? { required_evidence_edge_missing: reported } : {}),
    },
  });
  const rows = [
    {
      repo: "z-pass",
      task_id: "pass",
      arm: "with_codestory",
      repeat: 1,
      status: "pass",
      sufficiency: { obligation_accounting: accounting(2) },
    },
    {
      repo: "a-cancelled",
      task_id: "cancelled",
      arm: "with_codestory",
      repeat: 1,
      status: "cancelled",
    },
    {
      repo: "m-partial",
      task_id: "partial",
      arm: "with_codestory",
      repeat: 1,
      status: "fail",
      sufficiency: { obligation_accounting: accounting(1, 1) },
    },
    {
      repo: "n-failed-before-packet",
      task_id: "failed-before-packet",
      arm: "with_codestory",
      repeat: 1,
      status: "fail",
    },
  ];
  assert.deepEqual(
    summarizePacketObligationAccounting(rows, "fail-fast summary"),
    {
      packets: 2,
      total: 4,
      material: 4,
      nonmaterial: 0,
      material_status_buckets: {
        proven: 3,
        required_evidence_edge_missing: 1,
      },
    },
  );
});

const FIXTURE_MODEL_SHA256 = "a".repeat(64);
const FIXTURE_SERVER_SHA256 = "b".repeat(64);

function eligibleRetrievalStatus(overrides = {}) {
  const serverOverrides = overrides.embedding_server_identity ?? {};
  return {
    status: "pass",
    retrieval_mode: "full",
    degraded_reason: null,
    semantic_generation: "semantic-generation-1",
    engine_diagnostics_status: "pass",
    engine_diagnostics_error: null,
    embedding_device_policy: "accelerator_required",
    embedding_device_state: "accelerated",
    embedding_device_observation_source: "runtime_probe",
    embedding_cpu_allowed: false,
    embedding_model_sha256: FIXTURE_MODEL_SHA256,
    manifest_embedding_backend:
      `per-user-server:coderank-embed:q8_0:sha256-${FIXTURE_MODEL_SHA256}:fixture`,
    embedding_ggml_build_identity: "ggml-fixture",
    embedding_backend: "Metal",
    embedding_adapter: "Apple M5 GPU",
    embedding_adapter_description: "Apple family 9 GPU",
    embedding_policy: "accelerated",
    embedding_engine_instance_id: "engine-1",
    embedding_engine_residency: "resident",
    embedding_engine_load_generation: 1,
    embedding_engine_load_error: null,
    embedding_model_load_count: 1,
    embedding_smoke_ms: 1,
    embedding_initialization_ms: 2,
    embedding_materialized_reused: false,
    embedding_accelerator_execution_verified: true,
    embedding_execution_devices: ["Apple M5 GPU"],
    embedding_execution_backends: ["Metal"],
    embedding_execution_observation_source: "ggml_eval_callback",
    embedding_encode_count: 2,
    embedding_execution_node_count: 3,
    embedding_resident_accelerator_tensor_count: 4,
    embedding_resident_accelerator_tensor_bytes: 4096,
    embedding_model_layer_count: 13,
    embedding_offloaded_layer_count: 13,
    local_only: true,
    embedding_server_identity: {
      lifecycle: "resident",
      peer_verified: true,
      server_instance_id: "engine-1",
      executable_sha256: FIXTURE_SERVER_SHA256,
      executable_version: "0.17.0",
      load_generation: 1,
      model_load_count: 1,
      successful_encode_count: 2,
      ...serverOverrides,
    },
    ...overrides,
    embedding_server_identity: {
      lifecycle: "resident",
      peer_verified: true,
      server_instance_id: "engine-1",
      executable_sha256: FIXTURE_SERVER_SHA256,
      executable_version: "0.17.0",
      load_generation: 1,
      model_load_count: 1,
      successful_encode_count: 2,
      ...serverOverrides,
    },
  };
}

function pipelinePreparation(repo, retrievalOverrides = {}) {
  return {
    repo,
    retrieval_index_status: "pass",
    retrieval_status: eligibleRetrievalStatus(retrievalOverrides),
  };
}

function exactPipelinePreparation(repo, overridesByArm = {}) {
  const published = pipelinePreparation(repo, overridesByArm.published_0_17_4);
  const candidate = pipelinePreparation(repo, overridesByArm.candidate_0_18);
  return {
    ...candidate,
    arm: "candidate_0_18",
    arm_preparations: {
      published_0_17_4: published,
      candidate_0_18: candidate,
    },
  };
}

test("canary preparation requires complete live accelerator and server identity", () => {
  const preparation = pipelinePreparation("canary");
  assert.deepEqual(
    cachePreparationCanaryBlockers(preparation, { CODESTORY_EMBED_ALLOW_CPU: "0" }),
    [],
  );
  for (const [overrides, expected] of [
    [{ degraded_reason: "semantic stale" }, /retrieval is degraded/],
    [{ embedding_accelerator_execution_verified: false }, /accelerator execution was not verified/],
    [{ embedding_model_sha256: "bad" }, /model digest is missing or malformed/],
    [{ embedding_adapter: "llvmpipe" }, /software accelerator adapter/],
    [{ embedding_execution_observation_source: "inferred_from_request" }, /not backend-measured/],
    [{ embedding_execution_devices: [] }, /execution devices are missing/],
    [{ embedding_offloaded_layer_count: 12 }, /not every embedding model layer was offloaded/],
    [{ embedding_server_identity: { peer_verified: false } }, /peer identity is not verified/],
    [{ embedding_engine_instance_id: "engine-2" }, /instance identities disagree/],
    [{ embedding_server_identity: { load_generation: 2 } }, /load identities disagree/],
  ]) {
    const blockers = cachePreparationCanaryBlockers(
      pipelinePreparation("canary", overrides),
      { CODESTORY_EMBED_ALLOW_CPU: "0" },
    );
    assert.match(blockers.join("\n"), expected);
  }
  const versionDrift = pipelinePreparation("canary", {
    embedding_server_identity: { executable_version: "0.17.4" },
  });
  versionDrift.package_identity = { package_version: "0.18.0" };
  assert.match(
    cachePreparationCanaryBlockers(versionDrift, { CODESTORY_EMBED_ALLOW_CPU: "0" }).join("\n"),
    /expected 0\.18\.0/,
  );
});

function retrievalEngineDiagnosticsPayload(overrides = {}) {
  const engineOverrides = overrides.engine ?? {};
  const serverOverrides = overrides.embedding_server ?? {};
  return {
    retrieval_mode: "full",
    degraded_reason: null,
    engine: {
      ...eligibleRetrievalStatus(),
      embedding_adapter: "Métal GPU",
      embedding_materialized_path: "/private/host/model.gguf",
      ...engineOverrides,
    },
    embedding_server: {
      lifecycle: "resident",
      authority: { peer_verified: true },
      process: {
        server_instance_id: "engine-1",
        executable_sha256: FIXTURE_SERVER_SHA256,
        executable_version: "0.17.0",
        pid: 4242,
        endpoint: "/private/host/server.sock",
      },
      engine: {
        load_generation: 1,
        model_load_count: 1,
        successful_encode_count: 2,
      },
      ...serverOverrides,
    },
    ...overrides,
    engine: {
      ...eligibleRetrievalStatus(),
      embedding_adapter: "Métal GPU",
      embedding_materialized_path: "/private/host/model.gguf",
      ...engineOverrides,
    },
    embedding_server: {
      lifecycle: "resident",
      authority: { peer_verified: true },
      process: {
        server_instance_id: "engine-1",
        executable_sha256: FIXTURE_SERVER_SHA256,
        executable_version: "0.17.0",
        pid: 4242,
        endpoint: "/private/host/server.sock",
        ...(serverOverrides.process ?? {}),
      },
      engine: {
        load_generation: 1,
        model_load_count: 1,
        successful_encode_count: 2,
        ...(serverOverrides.engine ?? {}),
      },
      ...serverOverrides,
    },
  };
}

function retrievalEngineResourceResponse(uri, diagnostics = retrievalEngineDiagnosticsPayload()) {
  return {
    jsonrpc: "2.0",
    id: "benchmark-retrieval-engine",
    result: {
      contents: [{ uri, mimeType: "application/json", text: JSON.stringify(diagnostics) }],
    },
  };
}

function scriptedStdioChild(onFrame, options = {}) {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = new PassThrough();
  child.frames = [];
  child.signals = [];
  let input = "";
  let closed = false;
  const close = (exitCode, signal = null) => {
    if (closed) return;
    closed = true;
    child.stdout.end();
    child.stderr.end();
    queueMicrotask(() => child.emit("close", exitCode, signal));
  };
  const respond = (value, splitUnicode = false) => {
    const bytes = Buffer.from(`${JSON.stringify(value)}\n`, "utf8");
    if (!splitUnicode) {
      child.stdout.write(bytes);
      return;
    }
    const marker = Buffer.from("é", "utf8");
    const index = bytes.indexOf(marker);
    assert.ok(index >= 0, "scripted response lacks the split Unicode marker");
    child.stdout.write(bytes.subarray(0, index + 1));
    child.stdout.write(bytes.subarray(index + 1));
  };
  child.stdin.on("data", (chunk) => {
    input += chunk.toString();
    for (;;) {
      const newline = input.indexOf("\n");
      if (newline < 0) break;
      const line = input.slice(0, newline).trim();
      input = input.slice(newline + 1);
      if (!line) continue;
      const frame = JSON.parse(line);
      child.frames.push(frame);
      onFrame?.(frame, { respond, close, child });
    }
  });
  child.stdin.on("finish", () => {
    if (!options.hangOnEnd) close(0);
  });
  child.kill = (signal) => {
    child.signals.push(signal);
    if (signal === "SIGKILL" || options.closeOnTerm) close(null, signal);
    return true;
  };
  return child;
}

test("retrieval engine diagnostics follows MCP lifecycle and redacts host state", async () => {
  const project = "/tmp/CodeStory space !'()*é";
  const expectedUri = projectResourceUri("codestory://diagnostics/retrieval-engine", project);
  let spawnCall = null;
  const initializeSeen = deferred();
  let releaseInitialize = null;
  const child = scriptedStdioChild((frame, { respond }) => {
    if (frame.method === "initialize") {
      releaseInitialize = () => respond({
          jsonrpc: "2.0",
          id: frame.id,
          result: {
            protocolVersion: "2024-11-05",
            _meta: { codestory_protocol: { status: "agreed", compatible: true } },
          },
        });
      initializeSeen.resolve();
    } else if (frame.method === "resources/read") {
      respond(retrievalEngineResourceResponse(
        expectedUri,
        retrievalEngineDiagnosticsPayload({
          engine: {
            embedding_engine_load_error: "/private/host/load-error.gguf could not be read",
          },
        }),
      ), true);
    }
  });
  const snapshotPromise = codestoryRetrievalEngineDiagnosticsSnapshot(
    "/fixture/codestory-cli",
    project,
    1_000,
    { CODESTORY_RETRIEVAL_RUN_ID: "hostile-run" },
    null,
    {
      spawnProcess: (command, args, options) => {
        spawnCall = { command, args, options };
        return child;
      },
    },
  );
  await initializeSeen.promise;
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(child.frames.map((frame) => frame.method), ["initialize"]);
  releaseInitialize();
  const snapshot = await snapshotPromise;

  assert.equal(snapshot.status, "pass");
  assert.equal(snapshot.engine.embedding_adapter, "Métal GPU");
  assert.deepEqual(child.frames.map((frame) => frame.method), [
    "initialize",
    "notifications/initialized",
    "resources/read",
  ]);
  assert.deepEqual(child.frames[0].params.clientInfo, {
    name: "codestory-benchmark",
    version: "1",
  });
  assert.equal(child.frames[2].params.uri, expectedUri);
  assert.equal(spawnCall.command, "/fixture/codestory-cli");
  assert.deepEqual(spawnCall.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
  assert.equal(spawnCall.options.env.CODESTORY_RETRIEVAL_PROFILE, "agent");
  assert.equal(spawnCall.options.env.CODESTORY_RETRIEVAL_RUN_ID, "shared-agent");
  assert.deepEqual(child.signals, []);
  const retained = JSON.stringify(snapshot);
  assert.doesNotMatch(retained, /model\.gguf|server\.sock|4242/);
  assert.doesNotMatch(retained, /load-error\.gguf/);
  assert.equal(snapshot.engine.embedding_engine_load_error, "present");
});

test("retrieval engine diagnostics refuses invalid negotiation before resource reads", async () => {
  for (const response of [
    { jsonrpc: "2.0", id: "benchmark-initialize", error: { code: -32000, message: "no" } },
    {
      jsonrpc: "2.0",
      id: "benchmark-initialize",
      result: {
        protocolVersion: "2025-01-01",
        _meta: { codestory_protocol: { status: "agreed", compatible: true } },
      },
    },
  ]) {
    const child = scriptedStdioChild((frame, { respond }) => {
      if (frame.method === "initialize") respond(response);
    }, { closeOnTerm: true });
    const snapshot = await codestoryRetrievalEngineDiagnosticsSnapshot(
      "fixture-cli",
      "/fixture/project",
      1_000,
      {},
      null,
      { spawnProcess: () => child },
    );
    assert.equal(snapshot.status, "fail");
    assert.deepEqual(child.frames.map((frame) => frame.method), ["initialize"]);
  }
});

test("retrieval engine diagnostics rejects malformed resource envelopes", () => {
  const uri = projectResourceUri("codestory://diagnostics/retrieval-engine", "/fixture/project");
  const valid = retrievalEngineResourceResponse(uri);
  const cases = [
    { ...valid, error: { code: -32000, message: "failed" }, result: undefined },
    { ...valid, result: { contents: [{ ...valid.result.contents[0], uri: `${uri}-wrong` }] } },
    { ...valid, result: { contents: [valid.result.contents[0], valid.result.contents[0]] } },
    { ...valid, result: { contents: [{ ...valid.result.contents[0], mimeType: "text/plain" }] } },
    { ...valid, result: { contents: [{ ...valid.result.contents[0], text: "{" }] } },
    { ...valid, result: { contents: [{ ...valid.result.contents[0], text: "[]" }] } },
    {
      ...valid,
      result: { contents: [{ ...valid.result.contents[0], text: JSON.stringify({ retrieval_mode: "full" }) }] },
    },
  ];
  for (const response of cases) {
    const snapshot = retrievalEngineDiagnosticsSnapshotFromOutput(response, uri, 1);
    assert.equal(snapshot.status, "fail");
    assert.ok(snapshot.error);
  }
});

test("retrieval engine resource URIs are strict and filesystem-identity aware", async () => {
  const base = "codestory://diagnostics/retrieval-engine";
  assert.equal(
    projectResourceUri(base, "/tmp/space !'()*é"),
    `${base}?project=%2Ftmp%2Fspace%20%21%27%28%29%2A%C3%A9`,
  );
  assert.equal(
    projectResourceUri(base, String.raw`\\?\C:\Repo Space\!`, "win32"),
    `${base}?project=C%3A%2FRepo%20Space%2F%21`,
  );
  assert.equal(
    projectResourceUri(base, String.raw`\\?\UNC\server\share\repo`, "win32"),
    `${base}?project=%2F%2Fserver%2Fshare%2Frepo`,
  );
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-resource-uri-"));
  const real = path.join(root, "real");
  const alias = path.join(root, "alias");
  await mkdir(real);
  await symlink(real, alias, process.platform === "win32" ? "junction" : "dir");
  assert.equal(
    resourceUriMatches(projectResourceUri(base, alias), projectResourceUri(base, real), process.platform),
    true,
  );
  const other = path.join(root, "other");
  await mkdir(other);
  assert.equal(
    resourceUriMatches(projectResourceUri(base, real), projectResourceUri(base, other), process.platform),
    false,
  );
  assert.equal(
    resourceUriMatches(
      projectResourceUri(base, "C:/repo", "win32"),
      projectResourceUri(base, "c:/REPO", "win32"),
      "win32",
      () => true,
    ),
    true,
  );
  await rm(root, { recursive: true, force: true });
});

test("retrieval status and diagnostics merge preserve public ownership", () => {
  const uri = projectResourceUri("codestory://diagnostics/retrieval-engine", "/fixture/project");
  const diagnostics = retrievalEngineDiagnosticsSnapshotFromOutput(
    retrievalEngineResourceResponse(uri),
    uri,
    3,
  );
  const publicStatus = {
    status: "pass",
    retrieval_mode: "full",
    degraded_reason: null,
    semantic_generation: "semantic-generation-1",
    manifest_embedding_backend:
      `per-user-server:coderank-embed:q8_0:sha256-${FIXTURE_MODEL_SHA256}:fixture`,
    embedding_device_policy: "accelerator_required",
    embedding_device_state: "accelerated",
    embedding_cpu_allowed: false,
  };
  const merged = mergeRetrievalStatusWithEngineDiagnostics(publicStatus, diagnostics);
  assert.equal(merged.engine_diagnostics_status, "pass");
  assert.equal(merged.embedding_device_policy, "accelerator_required");
  assert.equal(merged.embedding_device_state, "accelerated");
  assert.equal(merged.embedding_model_sha256, FIXTURE_MODEL_SHA256);
  assert.equal(merged.local_only, true);
  assert.deepEqual(
    cachePreparationCanaryBlockers(
      { retrieval_index_status: "pass", retrieval_status: merged },
      { CODESTORY_EMBED_ALLOW_CPU: "0" },
    ),
    [],
  );
  const disagreement = mergeRetrievalStatusWithEngineDiagnostics(
    publicStatus,
    { ...diagnostics, retrieval_mode: "degraded" },
  );
  assert.equal(disagreement.engine_diagnostics_status, "fail");
  assert.match(disagreement.engine_diagnostics_error, /disagree/);
});

test("public retrieval status cannot inject maintainer engine identity", () => {
  const snapshot = retrievalStatusSnapshotFromOutput(
    { status: "pass", exitCode: 0, timedOut: false },
    {
      retrieval_mode: "full",
      degraded_reason: null,
      embedding_device_policy: "accelerator_required",
      embedding_device_state: "accelerated",
      embedding_cpu_allowed: false,
      embedding_backend: "hostile-backend",
      embedding_policy: "hostile-policy",
      embedding_engine_instance_id: "hostile-engine",
      embedding_accelerator_execution_verified: true,
      local_only: true,
    },
    null,
    1,
  );
  assert.equal(snapshot.embedding_device_policy, "accelerator_required");
  assert.equal(snapshot.embedding_device_state, "accelerated");
  assert.equal(snapshot.embedding_cpu_allowed, false);
  assert.equal(snapshot.embedding_backend, null);
  assert.equal(snapshot.embedding_policy, null);
  assert.equal(snapshot.embedding_engine_instance_id, null);
  assert.equal(snapshot.embedding_accelerator_execution_verified, null);
  assert.equal(snapshot.local_only, null);
});

test("retrieval engine diagnostics abort and EOF hang terminate task-owned children", async () => {
  const abortSignals = [];
  const abortChild = scriptedStdioChild(null, { hangOnEnd: true });
  abortChild.kill = (signal) => {
    abortSignals.push(signal);
    if (signal === "SIGKILL") queueMicrotask(() => abortChild.emit("close", null, signal));
    return true;
  };
  const controller = new AbortController();
  const aborted = codestoryRetrievalEngineDiagnosticsSnapshot(
    "fixture-cli",
    "/fixture/project",
    60_000,
    {},
    controller.signal,
    { forceKillAfterMs: 5, spawnProcess: () => abortChild },
  );
  controller.abort();
  assert.equal((await aborted).status, "aborted");
  assert.deepEqual(abortSignals, ["SIGTERM", "SIGKILL"]);

  const eofSignals = [];
  const uri = projectResourceUri("codestory://diagnostics/retrieval-engine", "/fixture/project");
  const eofChild = scriptedStdioChild((frame, { respond }) => {
    if (frame.method === "initialize") {
      respond({
        jsonrpc: "2.0",
        id: frame.id,
        result: {
          protocolVersion: "2024-11-05",
          _meta: { codestory_protocol: { status: "agreed", compatible: true } },
        },
      });
    } else if (frame.method === "resources/read") {
      respond(retrievalEngineResourceResponse(uri));
    }
  }, { hangOnEnd: true });
  eofChild.kill = (signal) => {
    eofSignals.push(signal);
    if (signal === "SIGKILL") queueMicrotask(() => eofChild.emit("close", null, signal));
    return true;
  };
  const timedOut = await codestoryRetrievalEngineDiagnosticsSnapshot(
    "fixture-cli",
    "/fixture/project",
    20,
    {},
    null,
    { forceKillAfterMs: 5, spawnProcess: () => eofChild },
  );
  assert.equal(timedOut.status, "timeout");
  assert.deepEqual(eofSignals, ["SIGTERM", "SIGKILL"]);
});

test("retrieval engine diagnostics bounds cumulative output and stream errors", async () => {
  for (const emitFailure of [
    (child) => {
      for (let index = 0; index < 1100; index += 1) child.stdout.write(`${" ".repeat(1024)}\n`);
    },
    (child) => child.stdout.emit("error", new Error("fixture stdout broke")),
    (child) => child.stderr.emit("error", new Error("fixture stderr broke")),
  ]) {
    const child = scriptedStdioChild((frame, { child: runningChild }) => {
      if (frame.method === "initialize") emitFailure(runningChild);
    }, { hangOnEnd: true });
    const snapshot = await codestoryRetrievalEngineDiagnosticsSnapshot(
      "fixture-cli",
      "/fixture/project",
      1_000,
      {},
      null,
      { forceKillAfterMs: 5, spawnProcess: () => child },
    );
    assert.notEqual(snapshot.status, "pass");
    assert.match(snapshot.error, /exceeded 1 MiB|stdout error|stderr error/);
    assert.deepEqual(child.signals, ["SIGTERM", "SIGKILL"]);
  }
});

const CLEAN_SHARD_ATTESTATION = {
  sourceCommit: "source",
  sourceTree: "tree",
  trackedDirty: false,
  cliSha256: "cli",
};

function pipelineResult(run, status = "pass") {
  return {
    benchmark_run_id: `${run.task.id}-${run.arm}-${run.repeat}`,
    repo: run.repo,
    task_id: run.task.id,
    arm: run.arm,
    repeat: run.repeat,
    canary: run.canary === true,
    preparation_overlap: run.preparation_overlap === true,
    comparative_wall_time_eligible: run.comparative_wall_time_eligible !== false,
    status,
  };
}

function pipelineFixture(overrides = {}) {
  const tasks = (overrides.repos ?? ["canary", "second"]).map((repo) => ({
    id: `${repo}-task`,
    repo,
    prompt: `trace ${repo}`,
  }));
  const opts = {
    arms: ["with_codestory", "without_codestory"],
    repeats: overrides.repeats ?? 1,
    jobs: overrides.jobs ?? 4,
    prepareCodestoryJobs: 2,
    publishable: false,
    collectAllFailures: false,
    canaryTaskId: overrides.canaryTaskId ?? tasks[0].id,
    manifestCanaryTaskId: overrides.canaryTaskId ?? tasks[0].id,
  };
  return { tasks, opts, plannedRuns: planAgentRuns(opts, tasks) };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

test("benchmark pipeline fences a failed canary and counts a passing canary once", async () => {
  const fixture = pipelineFixture({ repeats: 2 });
  const materialized = [];
  const prepared = [];
  const launched = [];
  const failed = await runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async (group) => materialized.push(group.repo),
    prepareGroup: async (group) => {
      prepared.push(group.repo);
      return [pipelinePreparation(group.repo)];
    },
    executeRun: async (_opts, run) => {
      launched.push(`${run.repo}/${run.arm}/${run.repeat}`);
      return pipelineResult(run, run.canary ? "fail" : "pass");
    },
  });
  assert.deepEqual(materialized, ["canary"]);
  assert.deepEqual(prepared, ["canary"]);
  assert.deepEqual(launched, ["canary/with_codestory/1"]);
  assert.equal(failed.firstFailure.task_id, "canary-task");

  const passingLaunches = [];
  const passing = await runAgentBenchmarkPipeline({
    ...pipelineFixture({ repeats: 2 }),
    materializeGroup: async () => {},
    prepareGroup: async (group) => [pipelinePreparation(group.repo)],
    executeRun: async (_opts, run) => {
      passingLaunches.push(`${run.repo}/${run.arm}/${run.repeat}`);
      return pipelineResult(run);
    },
  });
  assert.equal(passing.firstFailure, null);
  assert.equal(passing.results.length, passingLaunches.length);
  assert.equal(passing.results.length, 8);
  assert.equal(
    passing.results.filter((row) =>
      row.task_id === "canary-task" &&
      row.arm === "with_codestory" &&
      row.repeat === 1
    ).length,
    1,
  );
  assert.equal(new Set(passingLaunches).size, passingLaunches.length);

  const nonOwner = pipelineFixture({ repos: ["second"], canaryTaskId: "canary-task" });
  const nonOwnerResult = await runAgentBenchmarkPipeline({
    ...nonOwner,
    materializeGroup: async () => {},
    prepareGroup: async (group) => [pipelinePreparation(group.repo)],
    executeRun: async (_opts, run) => pipelineResult(run),
  });
  assert.equal(nonOwnerResult.results.some((row) => row.canary), false);
});

test("benchmark pipeline rejects missing or wrong-repository canary preparation before agents", async () => {
  for (const prepared of [[], [pipelinePreparation("wrong-repo")]]) {
    const launched = [];
    const outcome = await runAgentBenchmarkPipeline({
      ...pipelineFixture({ repos: ["canary", "second"] }),
      materializeGroup: async () => {},
      prepareGroup: async () => prepared,
      executeRun: async (_opts, run) => {
        launched.push(run.repo);
        return pipelineResult(run);
      },
    });
    assert.equal(outcome.firstFailure.kind, "preparation_contract_failed");
    assert.deepEqual(outcome.cachePreparation, []);
    assert.deepEqual(launched, []);
  }
});

test("preparation identity drift aborts before later rows and remains evidence only", async () => {
  for (const [field, value] of [
    ["embedding_model_sha256", "c".repeat(64)],
    ["embedding_backend", "Vulkan"],
    ["embedding_adapter", "Different GPU"],
    ["embedding_policy", "different-policy"],
  ]) {
    const evidence = [];
    const launched = [];
    const fixture = pipelineFixture({ repos: ["canary", "second"] });
    const outcome = await runAgentBenchmarkPipeline({
      ...fixture,
      materializeGroup: async () => {},
      prepareGroup: async (group) => [
        pipelinePreparation(group.repo, group.repo === "second" ? { [field]: value } : {}),
      ],
      recordPreparation: async (row) => evidence.push(row),
      executeRun: async (_opts, run) => {
        launched.push(`${run.repo}/${run.arm}`);
        return pipelineResult(run);
      },
    });
    assert.equal(outcome.firstFailure.kind, "preparation_identity_mismatch", field);
    assert.deepEqual(outcome.cachePreparation.map((row) => row.repo), ["canary"]);
    assert.deepEqual(evidence.map((row) => row.repo), ["canary", "second"]);
    assert.equal(fixture.opts.cachePreparationByRepo.has("canary"), true);
    assert.equal(fixture.opts.cachePreparationByRepo.has("second"), false);
    assert.equal(launched.some((row) => row.startsWith("second/")), false);
  }
});

test("preparation identity permits authenticated engine restarts on one host class", async () => {
  const fixture = pipelineFixture({
    repos: ["first", "second"],
    canaryTaskId: "global-canary-not-on-this-shard",
  });
  const launched = [];
  const outcome = await runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async () => {},
    prepareGroup: async (group) => [
      pipelinePreparation(group.repo, group.repo === "second"
        ? {
            embedding_engine_instance_id: "engine-2",
            embedding_server_identity: { server_instance_id: "engine-2" },
          }
        : {}),
    ],
    executeRun: async (_opts, run) => {
      launched.push(run.repo);
      return pipelineResult(run);
    },
  });
  assert.equal(outcome.firstFailure, null);
  assert.deepEqual(outcome.cachePreparation.map((row) => row.repo), ["first", "second"]);
  assert.equal(launched.includes("second"), true);
  assert.deepEqual(
    outcome.cachePreparation.map(
      (row) => row.retrieval_status.embedding_engine_instance_id,
    ),
    ["engine-1", "engine-2"],
  );
});

test("identity failure aborts sibling preparation before durable failure recording completes", async () => {
  const fixture = pipelineFixture({ repos: ["canary", "failing", "sibling"] });
  const bothStarted = deferred();
  const siblingObservedAbort = deferred();
  const releaseFailureWrite = deferred();
  const launched = [];
  let activePreparations = 0;
  const pipelinePromise = runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async () => {},
    prepareGroup: async (group, signal) => {
      if (group.repo === "canary") return [pipelinePreparation(group.repo)];
      activePreparations += 1;
      if (activePreparations === 2) bothStarted.resolve();
      await bothStarted.promise;
      if (group.repo === "failing") {
        return [pipelinePreparation(group.repo, { embedding_model_sha256: "c".repeat(64) })];
      }
      await new Promise((resolve) => {
        if (signal.aborted) return resolve();
        signal.addEventListener("abort", resolve, { once: true });
      });
      siblingObservedAbort.resolve();
      return [pipelinePreparation(group.repo)];
    },
    recordFirstFailure: async (failure) => {
      if (failure.kind === "preparation_identity_mismatch") {
        await releaseFailureWrite.promise;
      }
    },
    executeRun: async (_opts, run) => {
      launched.push(run.repo);
      return pipelineResult(run);
    },
  });
  await siblingObservedAbort.promise;
  assert.equal(fixture.opts.cachePreparationByRepo.has("failing"), false);
  assert.equal(fixture.opts.cachePreparationByRepo.has("sibling"), false);
  assert.equal(launched.includes("failing"), false);
  assert.equal(launched.includes("sibling"), false);
  releaseFailureWrite.resolve();
  const outcome = await pipelinePromise;
  assert.equal(outcome.firstFailure.kind, "preparation_identity_mismatch");
});

test("host class and shard attestation reject inconsistent preparation identity", async () => {
  const first = pipelinePreparation("first");
  const second = pipelinePreparation("second");
  const hostClass = benchmarkHostClass([first, second]);
  assert.equal(hostClass.model_sha256, FIXTURE_MODEL_SHA256);
  assert.equal(Object.hasOwn(hostClass, "embedding_engine_instance_id"), false);
  for (const [field, value] of [
    ["embedding_model_sha256", "c".repeat(64)],
    ["embedding_backend", "Vulkan"],
    ["embedding_adapter", "Different GPU"],
    ["embedding_policy", "different-policy"],
  ]) {
    const changed = pipelinePreparation("second", { [field]: value });
    assert.match(cachePreparationIdentityBlockers(first, changed).join("\n"), new RegExp(field));
    assert.throws(
      () => benchmarkHostClass([first, changed]),
      /do not share one retrieval host class/,
    );
  }
  const restarted = pipelinePreparation("second", {
    embedding_engine_instance_id: "engine-2",
    embedding_server_identity: { server_instance_id: "engine-2" },
  });
  assert.deepEqual(cachePreparationIdentityBlockers(first, restarted), []);
  assert.deepEqual(benchmarkHostClass([first, restarted]), hostClass);
  const exactFirst = exactPipelinePreparation("first");
  const exactRestarted = exactPipelinePreparation("second", {
    published_0_17_4: {
      embedding_engine_instance_id: "published-engine-2",
      embedding_server_identity: { server_instance_id: "published-engine-2" },
    },
    candidate_0_18: {
      embedding_engine_instance_id: "candidate-engine-2",
      embedding_server_identity: { server_instance_id: "candidate-engine-2" },
    },
  });
  assert.deepEqual(cachePreparationIdentityBlockers(exactFirst, exactRestarted), []);
  for (const arm of ["published_0_17_4", "candidate_0_18"]) {
    const changed = exactPipelinePreparation("second", {
      [arm]: { embedding_model_sha256: "c".repeat(64) },
    });
    assert.match(
      cachePreparationIdentityBlockers(exactFirst, changed).join("\n"),
      new RegExp(`${arm}.*embedding_model_sha256`),
    );
    assert.throws(
      () => benchmarkHostClass([exactFirst, changed]),
      /do not share one retrieval host class/,
    );
  }
  const fixture = pipelineFixture({ repos: ["first", "second"] });
  fixture.opts.prepareCodestoryCache = true;
  fixture.opts.shardCount = 1;
  fixture.opts.shardIndex = 0;
  const attestation = await benchmarkShardAttestation(
    fixture.opts,
    fixture.tasks,
    [first, second],
    [],
    CLEAN_SHARD_ATTESTATION,
  );
  assert.equal(attestation.model_sha256, attestation.host_class.model_sha256);
  await assert.rejects(
    () => benchmarkShardAttestation(
      fixture.opts,
      fixture.tasks,
      [first, first],
      [],
      CLEAN_SHARD_ATTESTATION,
    ),
    /preparation rows do not match/,
  );
  await assert.rejects(
    () => benchmarkShardAttestation(
      fixture.opts,
      fixture.tasks,
      [first, second, pipelinePreparation("extra")],
      [],
      CLEAN_SHARD_ATTESTATION,
    ),
    /preparation rows do not match/,
  );
  const restartedAttestation = await benchmarkShardAttestation(
    fixture.opts,
    fixture.tasks,
    [first, restarted],
    [],
    CLEAN_SHARD_ATTESTATION,
  );
  assert.deepEqual(restartedAttestation.host_class, hostClass);
});

test("exact-candidate preparation fences stable identity drift in either CodeStory arm", async () => {
  for (const arm of ["published_0_17_4", "candidate_0_18"]) {
    const tasks = ["first", "second"].map((repo) => ({
      id: `${repo}-task`,
      repo,
      prompt: `trace ${repo}`,
    }));
    const opts = {
      exactCandidate: true,
      exactCandidateStateRoot: "/fixture/exact-candidate-state",
      arms: EXACT_CANDIDATE_ARMS,
      jobs: 1,
    };
    const launched = [];
    const outcome = await runAgentBenchmarkPipeline({
      opts,
      tasks,
      plannedRuns: tasks.map((task) => ({
        task,
        repo: task.repo,
        arm: "candidate_0_18",
        repeat: 1,
      })),
      prepareGroup: async (group) => [exactPipelinePreparation(
        group.repo,
        group.repo === "second"
          ? { [arm]: { embedding_adapter: "Different GPU" } }
          : {},
      )],
      prepareIsolation: async () => ({ receipt: {} }),
      executeRun: async (_runOpts, run) => {
        launched.push(run.repo);
        return pipelineResult(run);
      },
    });
    assert.equal(outcome.firstFailure.kind, "preparation_failed");
    assert.match(
      outcome.firstFailure.error,
      new RegExp(`${arm}.*embedding_adapter`),
    );
    assert.deepEqual(launched, []);
    assert.equal(opts.cachePreparationByRepo.has("first"), true);
    assert.equal(opts.cachePreparationByRepo.has("second"), false);
  }
});

test("fail-fast closeout keeps the first failure instead of requiring unfinished shard preparation", async () => {
  const fixture = pipelineFixture({ repos: ["canary", "second"] });
  fixture.opts.prepareCodestoryCache = true;
  fixture.opts.publishable = true;
  fixture.opts.shardCount = 1;
  fixture.opts.shardIndex = 0;
  const retainedPreparation = [pipelinePreparation("canary")];
  const firstFailure = {
    kind: "canary_preparation",
    repo: "canary",
    task_id: "canary-task",
    blockers: [{ category: "environment", reasons: ["fixture canary failed"] }],
  };

  assert.equal(
    await benchmarkShardAttestationForCloseout(
      fixture.opts,
      fixture.tasks,
      retainedPreparation,
      [],
      firstFailure,
      CLEAN_SHARD_ATTESTATION,
    ),
    null,
  );
  await assert.rejects(
    () => benchmarkShardAttestationForCloseout(
      fixture.opts,
      fixture.tasks,
      retainedPreparation,
      [],
      null,
      CLEAN_SHARD_ATTESTATION,
    ),
    /preparation rows do not match/,
    "a successful publishable closeout must still reconcile every owned repository",
  );
});

test("benchmark pipeline retains product rows after an overlap baseline failure", async () => {
  const fixture = pipelineFixture({
    repos: ["canary", "alpha", "beta", "gamma", "delta", "epsilon"],
  });
  const baselineStarted = [];
  const comparativeFailures = [];
  const twoBaselinesStarted = deferred();
  const releaseRemainingPreparation = deferred();
  let abortedBaselineSiblings = 0;
  const outcome = await runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async () => {},
    prepareGroup: async (group) => {
      if (!["canary", "alpha"].includes(group.repo)) {
        await releaseRemainingPreparation.promise;
      }
      return [pipelinePreparation(group.repo)];
    },
    recordComparativeFailure: async (failure) => comparativeFailures.push(failure),
    executeRun: async (runOpts, run) => {
      if (run.arm === "with_codestory") return pipelineResult(run);
      baselineStarted.push(run.repo);
      if (baselineStarted.length === 2) {
        twoBaselinesStarted.resolve();
        releaseRemainingPreparation.resolve();
      }
      if (run.repo === "canary") {
        await twoBaselinesStarted.promise;
        return pipelineResult(run, "fail");
      }
      await new Promise((resolve) => {
        if (runOpts.signal.aborted) return resolve();
        runOpts.signal.addEventListener("abort", resolve, { once: true });
      });
      abortedBaselineSiblings += 1;
      return pipelineResult(run, "cancelled");
    },
  });
  assert.equal(outcome.firstFailure, null);
  assert.equal(outcome.comparativeFailure.kind, "comparative_baseline_failure");
  assert.equal(outcome.comparativePublishable, false);
  assert.equal(comparativeFailures.length, 1);
  assert.ok(abortedBaselineSiblings >= 1);
  assert.ok(baselineStarted.length >= 2);
  assert.ok(baselineStarted.length < fixture.tasks.length);
  assert.equal(outcome.cachePreparation.length, fixture.tasks.length);
  assert.equal(
    outcome.results.filter((row) => row.arm === "with_codestory" && row.status === "pass").length,
    fixture.tasks.length,
  );
  assert.equal(
    outcome.results.some((row) => row.arm === "without_codestory" && row.status === "pass"),
    false,
  );
});

test("benchmark pipeline prepares two repos while baselines overlap and waits before CodeStory", async () => {
  const fixture = pipelineFixture({ repos: ["canary", "alpha", "beta", "gamma"] });
  const barriers = new Map();
  const started = [];
  const startedTwo = deferred();
  const startedThree = deferred();
  let active = 0;
  let maxActive = 0;
  let completed = 0;
  let codeStoryStartedBeforeDrain = false;
  const pipelinePromise = runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async () => {},
    prepareGroup: async (group) => {
      if (group.repo === "canary") {
        return [pipelinePreparation(group.repo)];
      }
      active += 1;
      maxActive = Math.max(maxActive, active);
      started.push(group.repo);
      const barrier = deferred();
      barriers.set(group.repo, barrier);
      if (started.length === 2) startedTwo.resolve();
      if (started.length === 3) startedThree.resolve();
      await barrier.promise;
      active -= 1;
      completed += 1;
      return [pipelinePreparation(group.repo)];
    },
    executeRun: async (_opts, run) => {
      if (run.arm === "with_codestory" && !run.canary && completed < 3) {
        codeStoryStartedBeforeDrain = true;
      }
      return pipelineResult(run);
    },
  });

  await startedTwo.promise;
  assert.equal(started.length, 2);
  assert.equal(maxActive, 2);
  assert.equal(codeStoryStartedBeforeDrain, false);
  barriers.get(started[0]).resolve();
  await startedThree.promise;
  assert.equal(started.length, 3);
  assert.equal(maxActive, 2);
  for (const barrier of barriers.values()) barrier.resolve();
  const outcome = await pipelinePromise;

  assert.equal(completed, 3);
  assert.equal(codeStoryStartedBeforeDrain, false);
  const overlapBaselines = outcome.results.filter((row) =>
    row.arm === "without_codestory" && row.preparation_overlap
  );
  assert.ok(overlapBaselines.length > 0);
  assert.ok(overlapBaselines.every((row) => row.comparative_wall_time_eligible === false));
});

test("agent fail-fast aborts active siblings and stops queued repo groups", async () => {
  const fixture = pipelineFixture({ repos: ["first", "second", "third"], jobs: 2 });
  const runs = fixture.plannedRuns.filter((run) => run.arm === "with_codestory");
  const bothStarted = deferred();
  const launched = [];
  const recorded = [];
  let siblingAborted = false;
  const controller = new AbortController();
  const outcome = await runPlannedAgentRuns(
    fixture.opts,
    runs,
    new Map(),
    null,
    {
      signal: controller.signal,
      abortController: controller,
      failFast: true,
      onResult: async (row) => recorded.push(row),
      runOne: async (runOpts, run) => {
        launched.push(run.repo);
        if (launched.length === 2) bothStarted.resolve();
        await bothStarted.promise;
        if (run.repo === "first") return pipelineResult(run, "fail");
        await new Promise((resolve) => {
          if (runOpts.signal.aborted) return resolve();
          runOpts.signal.addEventListener("abort", resolve, { once: true });
        });
        siblingAborted = true;
        return pipelineResult(run, "cancelled");
      },
    },
  );
  assert.deepEqual(new Set(launched), new Set(["first", "second"]));
  assert.equal(siblingAborted, true);
  assert.equal(controller.signal.aborted, true);
  assert.equal(outcome.results.length, 2);
  assert.equal(recorded.length, 2);
});

test("fail-fast pipeline summary retains the causal partial packet and cancelled sibling", async () => {
  const fixture = pipelineFixture({ repos: ["apache", "express", "ripgrep"], jobs: 3 });
  const runs = fixture.plannedRuns.filter((run) => run.arm === "with_codestory");
  const allStarted = deferred();
  const launched = [];
  const controller = new AbortController();
  const partialAccounting = {
    total: 2,
    material: 2,
    nonmaterial: 0,
    material_status_buckets: {
      proven: 1,
      required_evidence_edge_missing: 1,
    },
  };
  const outcome = await runPlannedAgentRuns(
    fixture.opts,
    runs,
    new Map(),
    null,
    {
      signal: controller.signal,
      abortController: controller,
      failFast: true,
      runOne: async (runOpts, run) => {
        launched.push(run.repo);
        if (launched.length === runs.length) allStarted.resolve();
        await allStarted.promise;
        if (run.repo === "express") {
          return {
            ...pipelineResult(run, "fail"),
            sufficiency: {
              status: "partial",
              obligation_accounting: partialAccounting,
            },
          };
        }
        await new Promise((resolve) => {
          if (runOpts.signal.aborted) return resolve();
          runOpts.signal.addEventListener("abort", resolve, { once: true });
        });
        return pipelineResult(run, "cancelled");
      },
    },
  );

  assert.equal(outcome.firstFailure.repo, "express");
  const canonical = sortAgentResultsCanonical(
    outcome.results,
    [...fixture.tasks].sort((left, right) => left.repo.localeCompare(right.repo)),
    fixture.opts.arms,
  );
  assert.equal(canonical[0].repo, "apache");
  assert.equal(canonical[0].status, "cancelled");
  assert.deepEqual(
    summarizePacketObligationAccounting(canonical, "agent benchmark report"),
    {
      packets: 1,
      total: 2,
      material: 2,
      nonmaterial: 0,
      material_status_buckets: {
        proven: 1,
        required_evidence_edge_missing: 1,
      },
    },
    "summary closeout must not let a cancelled alphabetically earlier sibling mask Express",
  );
});

test("agent scheduler durably retains sibling rows after an active run exception", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-run-exception-"));
  const runsLedger = await createDurableJsonlAppender(path.join(root, "runs.jsonl"));
  const firstFailurePath = path.join(root, "first-failure.json");
  const fixture = pipelineFixture({ repos: ["first", "second", "third"], jobs: 2 });
  const runs = fixture.plannedRuns.filter((run) => run.arm === "with_codestory");
  const controller = new AbortController();
  const bothStarted = deferred();
  const launched = [];
  let outcome;
  try {
    outcome = await runPlannedAgentRuns(
      fixture.opts,
      runs,
      new Map(),
      root,
      {
        signal: controller.signal,
        abortController: controller,
        failFast: true,
        onResult: (row) => runsLedger.append(row),
        onFirstFailure: (failure) => writeFile(
          firstFailurePath,
          `${JSON.stringify(failure)}\n`,
        ),
        runOne: async (runOpts, run) => {
          launched.push(run.repo);
          if (launched.length === 2) bothStarted.resolve();
          await bothStarted.promise;
          if (run.repo === "first") {
            throw new Error("fixture provenance write failed");
          }
          await new Promise((resolve) => {
            if (runOpts.signal.aborted) return resolve();
            runOpts.signal.addEventListener("abort", resolve, { once: true });
          });
          return pipelineResult(run, "cancelled");
        },
      },
    );
  } finally {
    await runsLedger.close();
  }

  const firstFailure = JSON.parse(await readFile(firstFailurePath, "utf8"));
  const rows = (await readFile(path.join(root, "runs.jsonl"), "utf8"))
    .split(/\r?\n/)
    .filter(Boolean)
    .map(JSON.parse);
  assert.deepEqual(new Set(launched), new Set(["first", "second"]));
  assert.equal(controller.signal.aborted, true);
  assert.equal(firstFailure.kind, "run_exception");
  assert.equal(firstFailure.repo, "first");
  assert.equal(firstFailure.task_id, "first-task");
  assert.equal(firstFailure.arm, "with_codestory");
  assert.equal(firstFailure.repeat, 1);
  assert.deepEqual(firstFailure.blockers, [{
    category: "harness-contract",
    reasons: ["agent run raised an exception: fixture provenance write failed"],
  }]);
  assert.deepEqual(rows.map((row) => row.repo), ["second"]);
  assert.deepEqual(outcome.results.map((row) => row.repo), ["second"]);
  assert.equal(outcome.firstFailure.kind, "run_exception");
  await rm(root, { recursive: true, force: true });
});

test("task-owned process abort escalates and timeout keeps precedence", async () => {
  const ignoringChild = (signals) => {
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = { end() {} };
    child.kill = (signal) => {
      signals.push(signal);
      if (signal === "SIGKILL") {
        queueMicrotask(() => child.emit("close", null, signal));
      }
      return true;
    };
    return child;
  };

  const abortSignals = [];
  const abortController = new AbortController();
  const aborted = runProcess("fixture", [], {
    signal: abortController.signal,
    timeoutMs: 1,
    forceKillAfterMs: 5,
    spawnProcess: () => ignoringChild(abortSignals),
  });
  abortController.abort();
  const abortResult = await aborted;
  assert.equal(abortResult.status, "aborted");
  assert.deepEqual(abortSignals, ["SIGTERM", "SIGKILL"]);

  const timeoutSignals = [];
  const timeoutController = new AbortController();
  const timedOut = runProcess("fixture", [], {
    signal: timeoutController.signal,
    timeoutMs: 1,
    forceKillAfterMs: 5,
    spawnProcess: () => ignoringChild(timeoutSignals),
  });
  setTimeout(() => timeoutController.abort(), 2);
  const timeoutResult = await timedOut;
  assert.equal(timeoutResult.status, "timeout");
  assert.equal(timeoutResult.aborted, false);
  assert.deepEqual(timeoutSignals, ["SIGTERM", "SIGKILL"]);
});

test("snapshot child abort reaches doctor and retrieval-status probes", async () => {
  for (const snapshot of [codestoryDoctorSnapshot, codestoryRetrievalStatusSnapshot]) {
    const signals = [];
    const controller = new AbortController();
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = { end() {} };
    child.kill = (signal) => {
      signals.push(signal);
      if (signal === "SIGKILL") {
        queueMicrotask(() => child.emit("close", null, signal));
      }
      return true;
    };
    const resultPromise = snapshot(
      "fixture-codestory-cli",
      "/fixture/project",
      60_000,
      {},
      controller.signal,
      { forceKillAfterMs: 5, spawnProcess: () => child },
    );
    controller.abort();
    const result = await resultPromise;
    assert.equal(result.status, "aborted");
    assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
  }
});

test("benchmark pipeline aborts active materialization and provenance git children", async () => {
  const base = pipelineFixture({ repos: ["canary", "failing", "sibling"] });
  const opts = { ...base.opts, arms: ["with_codestory"] };
  const fixture = { ...base, opts, plannedRuns: planAgentRuns(opts, base.tasks) };
  const materializationChildStarted = deferred();
  const materializationSignals = [];
  const ignoringChild = (signals, onStart = () => {}) => {
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = { end() {} };
    child.kill = (signal) => {
      signals.push(signal);
      if (signal === "SIGKILL") {
        queueMicrotask(() => child.emit("close", null, signal));
      }
      return true;
    };
    onStart();
    return child;
  };

  const outcome = await runAgentBenchmarkPipeline({
    ...fixture,
    materializeGroup: async (group, signal) => {
      if (group.repo === "canary") return;
      if (group.repo === "failing") {
        await materializationChildStarted.promise;
        throw new Error("fixture materialization failed");
      }
      await gitCheckedOutput(["status"], "/fixture", {
        timeoutMs: 60_000,
        signal,
        forceKillAfterMs: 5,
        spawnProcess: () => ignoringChild(
          materializationSignals,
          () => materializationChildStarted.resolve(),
        ),
      });
    },
    prepareGroup: async (group) => [pipelinePreparation(group.repo)],
    executeRun: async (_runOpts, run) => pipelineResult(run),
  });
  assert.equal(outcome.firstFailure.kind, "materialization_failed");
  assert.equal(outcome.aborted, true);
  assert.deepEqual(materializationSignals, ["SIGTERM", "SIGKILL"]);
  assert.equal(outcome.results.length, 1);
  assert.equal(outcome.results[0].canary, true);

  const provenanceSignals = [];
  const provenanceController = new AbortController();
  const provenanceRemoteStarted = deferred();
  const provenanceCommands = [];
  const completedChild = (stdout) => {
    const child = new EventEmitter();
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = { end() {} };
    child.kill = () => true;
    queueMicrotask(() => {
      if (stdout) child.stdout.write(stdout);
      child.stdout.end();
      child.stderr.end();
      child.emit("close", 0, null);
    });
    return child;
  };
  const provenance = repoProvenance(
    {
      path: "/fixture/project",
      checkout_path: "/fixture/project",
      url: "https://example.test/project.git",
      ref: "fixture",
    },
    provenanceController.signal,
    {
      forceKillAfterMs: 5,
      spawnProcess: (_command, args) => {
        provenanceCommands.push(args);
        if (provenanceCommands.length === 1) return completedChild("");
        if (provenanceCommands.length === 2) return completedChild(`${"a".repeat(40)}\n`);
        return ignoringChild(provenanceSignals, () => provenanceRemoteStarted.resolve());
      },
    },
  );
  await provenanceRemoteStarted.promise;
  provenanceController.abort();
  const provenanceResult = await provenance;
  assert.equal(provenanceResult.git_head, "a".repeat(40));
  assert.equal(provenanceResult.git_origin, null);
  assert.deepEqual(provenanceCommands, [
    ["-C", "/fixture/project", "status", "--short"],
    ["-C", "/fixture/project", "rev-parse", "HEAD"],
    ["-C", "/fixture/project", "remote", "get-url", "origin"],
  ]);
  assert.deepEqual(provenanceSignals, ["SIGTERM", "SIGKILL"]);
});

test("benchmark pipeline durably retains preparation and first failure state", async () => {
  for (const stage of ["materialization", "preparation", "agent_isolation"]) {
    const root = await mkdtemp(path.join(os.tmpdir(), `codestory-pipeline-${stage}-`));
    const runsLedger = await createDurableJsonlAppender(path.join(root, "runs.jsonl"));
    const preparationLedger = await createDurableJsonlAppender(path.join(root, "preparations.jsonl"));
    const firstFailurePath = path.join(root, "first-failure.json");
    const fixture = pipelineFixture();
    const launched = [];
    try {
      await runAgentBenchmarkPipeline({
        ...fixture,
        materializeGroup: async (group) => {
          if (group.repo === "second" && stage === "materialization") {
            throw new Error("fixture materialization failed");
          }
        },
        prepareGroup: async (group) => {
          if (group.repo === "second" && stage === "preparation") {
            const error = new Error("fixture preparation failed");
            error.preparation = { repo: group.repo, error: error.message };
            throw error;
          }
          return [pipelinePreparation(group.repo)];
        },
        prepareIsolation: async () => {
          if (stage === "agent_isolation") {
            throw new Error("fixture agent isolation failed");
          }
          return null;
        },
        executeRun: async (_opts, run) => {
          launched.push(run.repo);
          return pipelineResult(run);
        },
        recordResult: (row) => runsLedger.append(row),
        recordPreparation: (row) => preparationLedger.append({ kind: "preparation", ...row }),
        recordPreparationState: (row) => preparationLedger.append(row),
        recordFirstFailure: (failure) => writeFile(
          firstFailurePath,
          `${JSON.stringify(failure)}\n`,
        ),
      });
    } finally {
      await runsLedger.close();
      await preparationLedger.close();
    }
    const parseLedger = (contents) => contents.split(/\r?\n/).filter(Boolean).map(JSON.parse);
    const runRows = parseLedger(await readFile(path.join(root, "runs.jsonl"), "utf8"));
    const preparationRows = parseLedger(
      await readFile(path.join(root, "preparations.jsonl"), "utf8"),
    );
    const firstFailure = JSON.parse(await readFile(firstFailurePath, "utf8"));
    assert.equal(preparationRows.some((row) => row.repo === "canary"), true);
    if (stage === "preparation") {
      assert.equal(preparationRows.some((row) => row.repo === "second" && row.error), true);
    }
    if (stage === "agent_isolation") {
      assert.equal(runRows.length, 0);
      assert.equal(firstFailure.kind, "agent_isolation_failed");
      assert.equal(firstFailure.repo, "canary");
      assert.deepEqual(launched, []);
    } else {
      assert.equal(runRows.some((row) => row.canary), true);
      assert.equal(firstFailure.kind, `${stage}_failed`);
      assert.equal(firstFailure.repo, "second");
      assert.equal(launched.every((repo) => repo === "canary"), true);
    }
    await rm(root, { recursive: true, force: true });
  }
});

test("shard aggregation binds canary, contract, accounting, and host-class latency", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-shards-"));
  try {
    const taskIds = [null, null];
    for (let index = 0; taskIds.some((value) => value == null); index += 1) {
      const id = `task-${index}`;
      taskIds[taskShardIndex(id, 2)] ??= id;
    }
    const tasks = taskIds.map((id, index) => ({
      id,
      name: id,
      repo: `repo-${index}`,
      task_class: "route_tracing",
      prompt: `trace ${id}`,
      quality_thresholds: {},
    }));
    const canaryTask = tasks[0];
    const opts = {
      aggregateShards: null,
      outDir: path.join(root, "aggregate"),
      arms: ["with_codestory"],
      repeats: 1,
      jobs: 4,
      timeoutMs: 600_000,
      prepareCodestoryCache: true,
      prepareCodestoryJobs: 2,
      prepareCodestoryTimeoutMs: 1_800_000,
      packetRuntime: false,
      packetRuntimeMode: "both",
      materializeRepos: true,
      collectAllFailures: false,
      shardCount: 2,
      runner: "codex",
      model: "gpt-5.6-sol",
      sandbox: "read-only",
      taskSuite: null,
      maxSourceReadsAfterPacket: 0,
      diagnosticExtraProbesFromManifest: false,
      packetGateImprovedFrom: null,
      codestoryCli: process.execPath,
      candidatePackageSha256: "package",
      canaryTaskId: canaryTask.id,
      manifestCanaryTaskId: canaryTask.id,
      publishable: false,
    };
    const planned = planAgentRuns(opts, tasks);
    const emptyAccounting = {
      total: 0,
      material: 0,
      nonmaterial: 0,
      material_status_buckets: {},
    };
    const rows = planned.map((run) => ({
      repo: run.repo,
      task_id: run.task.id,
      arm: run.arm,
      repeat: run.repeat,
      canary: run.task.id === canaryTask.id,
      status: "pass",
      wall_ms: 10,
      benchmark_contract: benchmarkContractForRun(opts, run),
      codestory_harness_prelude: {
        packet_sufficiency: { obligation_accounting: emptyAccounting },
      },
    }));
    const writeShards = async (rowValues = rows, summaryMutator = (summary) => summary) => {
      const directories = [];
      for (const index of [0, 1]) {
        const directory = path.join(root, `shard-${index}`);
        await mkdir(directory, { recursive: true });
        const shardRows = rowValues.filter((row) => taskShardIndex(row.task_id, 2) === index);
        const preparation = tasksForShard(tasks, 2, index).map((task) =>
          pipelinePreparation(task.repo)
        );
        const attestation = await benchmarkShardAttestation(
          { ...opts, shardIndex: index },
          tasks,
          preparation,
          shardRows,
          CLEAN_SHARD_ATTESTATION,
        );
        const summary = summaryMutator({
          publishable: false,
          expected_rows: shardRows.length,
          completed_rows: shardRows.length,
          first_failure: null,
          canary_task_id: canaryTask.id,
          effective_canary_task_id:
            shardRows.some((row) => row.canary === true) ? canaryTask.id : null,
          packet_obligation_accounting: emptyAccounting,
          shard: { count: 2, index, attestation },
        }, index);
        await writeFile(path.join(directory, "summary.json"), `${JSON.stringify(summary)}\n`);
        await writeFile(
          path.join(directory, "runs.jsonl"),
          shardRows.map((row) => JSON.stringify(row)).join("\n") + (shardRows.length ? "\n" : ""),
        );
        directories.push(directory);
      }
      return directories;
    };

    let shardDirs = await writeShards();
    await aggregateShardRuns({ ...opts, aggregateShards: shardDirs }, tasks);
    let aggregate = JSON.parse(await readFile(path.join(opts.outDir, "summary.json"), "utf8"));
    assert.equal(aggregate.latency_pooling_eligible, true);
    assert.equal(aggregate.pooled_latency_summary.length, 2);
    const shardOneRows = rows.filter((row) => taskShardIndex(row.task_id, 2) === 1);
    const shardOnePreparation = tasksForShard(tasks, 2, 1).map((task) =>
      pipelinePreparation(task.repo)
    );
    const shardOneSummaryPath = path.join(shardDirs[1], "summary.json");
    const shardOneSummary = JSON.parse(await readFile(shardOneSummaryPath, "utf8"));
    shardOneSummary.shard.attestation = await benchmarkShardAttestation(
      { ...opts, jobs: 8, shardIndex: 1 },
      tasks,
      shardOnePreparation,
      shardOneRows,
      CLEAN_SHARD_ATTESTATION,
    );
    await writeFile(shardOneSummaryPath, `${JSON.stringify(shardOneSummary)}\n`);
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "different-jobs") }, tasks),
      /attestation does not match/,
    );
    await assert.rejects(
      () => benchmarkShardAttestation(
        { ...opts, shardIndex: 1 },
        tasks,
        shardOnePreparation,
        shardOneRows,
        {
        ...CLEAN_SHARD_ATTESTATION,
        trackedDirty: true,
        },
      ),
      /clean tracked source checkout/,
    );
    shardDirs = await writeShards(rows, (summary, index) => index === 1
      ? {
          ...summary,
          shard: {
            ...summary.shard,
            attestation: { ...summary.shard.attestation, tracked_dirty: true },
          },
        }
      : summary);
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "dirty") }, tasks),
      /clean tracked source checkout/,
    );

    const noCanaryRows = rows.map((row) => ({ ...row, canary: false }));
    shardDirs = await writeShards(noCanaryRows);
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "no-canary") }, tasks),
      /must appear exactly once/,
    );

    shardDirs = await writeShards(rows);
    const wrongContractRow = {
      ...rows[0],
      benchmark_contract: {
        ...rows[0].benchmark_contract,
        task_manifest_hash: "different-task-contract",
      },
    };
    const wrongContractShard = taskShardIndex(wrongContractRow.task_id, 2);
    await writeFile(
      path.join(shardDirs[wrongContractShard], "runs.jsonl"),
      `${JSON.stringify(wrongContractRow)}\n`,
    );
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "contract") }, tasks),
      /compatibility fingerprint does not match its contents/,
    );

    const invalidAccounting = {
      total: 216,
      material: 198,
      nonmaterial: 18,
      material_status_buckets: { proven: 197 },
    };
    const invalidRows = rows.map((row, index) => index === 0
      ? {
          ...row,
          codestory_harness_prelude: {
            packet_sufficiency: { obligation_accounting: invalidAccounting },
          },
        }
      : row);
    shardDirs = await writeShards(invalidRows);
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "obligations") }, tasks),
      /material=198 does not reconcile with material status buckets=197/,
    );

    shardDirs = await writeShards(rows, (summary, index) => index === 1
      ? {
          ...summary,
          shard: {
            ...summary.shard,
            attestation: {
              ...summary.shard.attestation,
              cli_sha256: "different-same-host-cli",
            },
          },
        }
      : summary);
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: path.join(root, "same-host-binary") }, tasks),
      /platform artifacts do not match its host class/,
    );

    shardDirs = await writeShards(rows, (summary, index) => ({
      ...summary,
      shard: {
        ...summary.shard,
        attestation: {
          ...summary.shard.attestation,
          host_class: {
            platform: "darwin",
            arch: "arm64",
            cpu_model: index === 0 ? "Apple M1 Pro" : "Apple M5 Pro",
            logical_cpu_count: 10,
            total_memory_bytes: 34_359_738_368,
            accelerator_backend: "Metal",
            accelerator_adapter: "MTL0",
            embedding_policy: "accelerated",
            model_sha256: FIXTURE_MODEL_SHA256,
          },
        },
      },
    }));
    const cpuClassOut = path.join(root, "different-cpu-class");
    await aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: cpuClassOut }, tasks);
    const cpuClassAggregate = JSON.parse(
      await readFile(path.join(cpuClassOut, "summary.json"), "utf8"),
    );
    assert.equal(cpuClassAggregate.latency_pooling_eligible, false);
    assert.equal(cpuClassAggregate.pooled_latency_summary, null);
    assert.equal(cpuClassAggregate.latency_summaries_by_host_class.length, 2);

    for (const [field, value, expected] of [
      ["accelerator_backend", null, /accelerator backend and adapter/],
      ["accelerator_adapter", "llvmpipe", /software accelerator/],
      ["embedding_policy", "cpu_explicit", /policy is not accelerated/],
      ["model_sha256", "bad", /model digest is missing or malformed/],
    ]) {
      shardDirs = await writeShards(rows, (summary, index) => index === 1
        ? {
            ...summary,
            shard: {
              ...summary.shard,
              attestation: {
                ...summary.shard.attestation,
                host_class: {
                  ...summary.shard.attestation.host_class,
                  [field]: value,
                },
              },
            },
          }
        : summary);
      await assert.rejects(
        () => aggregateShardRuns({
          ...opts,
          aggregateShards: shardDirs,
          outDir: path.join(root, `bad-host-${field}`),
        }, tasks),
        expected,
      );
    }
    shardDirs = await writeShards(rows, (summary, index) => index === 1
      ? {
          ...summary,
          shard: {
            ...summary.shard,
            attestation: {
              ...summary.shard.attestation,
              host_class: {
                ...summary.shard.attestation.host_class,
                model_sha256: "c".repeat(64),
              },
            },
          },
        }
      : summary);
    await assert.rejects(
      () => aggregateShardRuns({
        ...opts,
        aggregateShards: shardDirs,
        outDir: path.join(root, "host-model-disagreement"),
      }, tasks),
      /host-class model does not match/,
    );

    const mixedRows = rows.map((row) => {
      if (taskShardIndex(row.task_id, 2) !== 1) return row;
      const run = planned.find((candidate) => candidate.task.id === row.task_id);
      return {
        ...row,
        benchmark_contract: benchmarkContractForRun(
          { ...opts, codestoryCli: "C:\\managed\\codestory-cli.exe" },
          run,
        ),
      };
    });
    shardDirs = await writeShards(mixedRows, (summary, index) => index === 1
      ? {
          ...summary,
          shard: {
            ...summary.shard,
            attestation: {
              ...summary.shard.attestation,
              cli_sha256: "other-platform-cli",
              package_sha256: "other-platform-package",
              host_class: {
                ...summary.shard.attestation.host_class,
                platform: "linux",
                arch: "x64",
                accelerator_backend: "Vulkan",
              },
            },
          },
        }
      : summary);
    const mixedOut = path.join(root, "mixed-hosts");
    await aggregateShardRuns({ ...opts, aggregateShards: shardDirs, outDir: mixedOut }, tasks);
    aggregate = JSON.parse(await readFile(path.join(mixedOut, "summary.json"), "utf8"));
    assert.equal(aggregate.latency_pooling_eligible, false);
    assert.equal(aggregate.pooled_latency_summary, null);
    assert.equal(aggregate.latency_summaries_by_host_class.length, 2);
    assert.ok(aggregate.latency_summaries_by_host_class.every((entry) => entry.cost_accounting));
    assert.ok(aggregate.summary.every((entry) => entry.median_wall_ms === null));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("packet-runtime cache observations preserve prepared cache provenance", () => {
  const cachePreparation = [
    {
      repo: "codestory",
      retrieval_status: { retrieval_mode: "full" },
    },
  ];
  const opts = {
    cachePreparationByRepo: new Map(cachePreparation.map((row) => [row.repo, row])),
  };

  for (const transportMode of ["cold_cli_packet", "warm_stdio_packet"]) {
    const observations = packetRuntimeCacheObservations(opts, "codestory", transportMode);

    assert.equal(observations.cache_prepared, true);
    assert.equal(observations.cache_preparation, cachePreparation[0]);
    assert.equal(cachePolicyForRun(observations), "prepared-retrieval-cache-read-only");
  }
});

test("agent packet prelude carries exact semantic execution after the server idles", () => {
  const preparation = {
    repo: "codestory",
    retrieval_contract: {
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      execution_policy: "accelerated",
    },
    retrieval_status: { semantic_generation: "semantic-1" },
  };
  const packet = {
    answer: {
      retrieval_trace: {
        retrieval_publication: { semantic_generation: "semantic-1" },
        semantic_fallback_count: 0,
        packet_sidecar_diagnostics: [{ retrieval_mode: "full" }],
        retrieval_shadow: {
          degraded_reason: null,
          error: null,
          cancel_reason: null,
          stage_timings: [{
            stage: "stage1b_semantic",
            completion_status: "completed",
            degraded: false,
            stub_reason: null,
            cancel_reason: null,
          }],
        },
      },
    },
  };
  const observations = agentPacketPreludeCacheObservations(
    { cachePreparationByRepo: new Map([[preparation.repo, preparation]]) },
    preparation.repo,
    packet,
    { codestory_index_commands_observed: 0 },
  );
  const provenance = localCacheProvenance({
    semantic_ready: false,
    embedding_engine_instance_id: null,
    semantic_generation: "semantic-1",
    transport_mode: observations.transport_mode,
    packet_embedding_execution: observations.packet_embedding_execution,
  });

  assert.equal(observations.transport_mode, "agent_harness_prelude");
  assert.equal(observations.cache_preparation, preparation);
  assert.deepEqual(cacheProvenanceBlockers({ codestory_cache_provenance: provenance }), []);

  observations.packet_embedding_execution.semantic_generation = "other-generation";
  provenance.packet_embedding_execution = observations.packet_embedding_execution;
  assert.match(
    cacheProvenanceBlockers({ codestory_cache_provenance: provenance }).join("\n"),
    /does not match the prepared generation/,
  );
});

test("cold packet embedding execution binds full retrieval to the prepared semantic generation", () => {
  const preparation = {
    retrieval_contract: {
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      execution_policy: "accelerated",
    },
    retrieval_status: { semantic_generation: "semantic-1" },
  };
  const packet = {
    answer: {
      retrieval_trace: {
        retrieval_publication: { semantic_generation: "semantic-1" },
        semantic_fallback_count: 0,
        packet_sidecar_diagnostics: [
          { retrieval_mode: "full" },
          { retrieval_mode: "full" },
        ],
        retrieval_shadow: {
          stage_timings: [
            { stage: "stage1_lexical" },
            {
              stage: "stage1b_semantic",
              completion_status: "completed",
              degraded: false,
              stub_reason: null,
              cancel_reason: null,
            },
          ],
        },
      },
    },
  };

  assert.deepEqual(
    packetEmbeddingExecutionProof(packet, preparation, "cold_cli_packet"),
    {
      source: "packet.answer.retrieval_trace",
      transport_mode: "cold_cli_packet",
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      embedding_policy: "accelerated",
      retrieval_mode: "full",
      diagnostic_count: 2,
      full_diagnostic_count: 2,
      semantic_stage_count: 1,
      completed_semantic_stage_count: 1,
      invalid_semantic_stage_count: 0,
      shadow_degraded_reason: null,
      shadow_error: null,
      shadow_cancel_reason: null,
      semantic_fallback_count: 0,
      semantic_generation: "semantic-1",
      prepared_semantic_generation: "semantic-1",
    },
  );
});

test("v3 public packet projection binds full retrieval to the prepared publication", () => {
  const preparation = {
    retrieval_contract: {
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      execution_policy: "accelerated",
    },
    retrieval_status: { semantic_generation: "semantic-v3" },
  };
  const packet = packetV3Fixture();
  const proof = packetEmbeddingExecutionProof(
    packet,
    preparation,
    "agent_harness_prelude",
  );
  const provenance = localCacheProvenance({
    semantic_generation: "semantic-v3",
    transport_mode: "agent_harness_prelude",
    packet_embedding_execution: proof,
  });
  assert.equal(proof.source, "packet.v3_public_projection");
  assert.equal(proof.semantic_generation, "semantic-v3");
  assert.deepEqual(
    cacheProvenanceBlockers({ codestory_cache_provenance: provenance }),
    [],
  );

  for (const [field, value, expected] of [
    ["retrieval_mode", "degraded", /retrieval mode=degraded/],
    ["retrieval_state_generation", "other", /state generation does not match/],
    ["semantic_generation", "other", /does not match the prepared generation/],
    ["diagnostics_sha256", null, /no valid diagnostics reference/],
    ["evidence_count", -1, /invalid evidence accounting/],
  ]) {
    const hostileProof = structuredClone(proof);
    hostileProof[field] = value;
    const blockers = cacheProvenanceBlockers({
      codestory_cache_provenance: {
        ...provenance,
        packet_embedding_execution: hostileProof,
      },
    });
    assert.match(blockers.join("\n"), expected, field);
  }
});

test("packet latency telemetry preserves retrieval shadow cache diagnostics", () => {
  const packet = {
    answer: {
      freshness: { duration_ms: 12 },
      retrieval_trace: {
        total_latency_ms: 40,
        sla_target_ms: 500,
        sla_missed: false,
        steps: [{ kind: "search", status: "success", duration_ms: 25, message: "ok" }],
      },
    },
    benchmark_trace: {
      retrieval_trace: {
        retrieval_shadow: {
          retrieval_mode: "full",
          retrieval_total_ms: 7,
          cache_hit: true,
          stage_timings: [
            { stage: "stage1_lexical", elapsed_ms: 2, cache_hit: false },
            { stage: "stage2_semantic_vector", elapsed_ms: 1, cache_hit: true },
          ],
          candidate_count: 4,
          resolved_hit_count: 3,
          unresolved_candidate_count: 1,
        },
      },
    },
  };

  const telemetry = packetLatencyTelemetry(packet, 80);
  assert.equal(telemetry.retrieval_shadow.cache_hit, true);
  assert.equal(telemetry.retrieval_shadow.cache_hit_stage_count, 1);
  assert.deepEqual(telemetry.retrieval_shadow.cache_hit_stages, ["stage2_semantic_vector"]);

  const summary = summarizePacketRuntimeRuns([
    {
      repo: "fixture",
      task_id: "cache",
      mode: "warm_stdio_packet",
      status: "pass",
      wall_ms: 80,
      warm_stdio_packet_cache_hit: true,
      packet_latency: telemetry,
    },
  ]);
  assert.equal(summary[0].warm_stdio_packet_cache_hit_runs, 1);
  assert.equal(summary[0].retrieval_shadow_cache_hit_runs, 1);
  assert.equal(summary[0].retrieval_shadow_stage_cache_hit_runs, 1);

  const debug = buildQualityDebugPayload([
    {
      repo: "fixture",
      task_id: "cache",
      mode: "warm_stdio_packet",
      status: "pass",
      warm_stdio_packet_cache_hit: true,
      packet_latency: telemetry,
    },
  ]);
  assert.equal(debug.rows[0].warm_stdio_packet_cache_hit, true);
  assert.equal(debug.rows[0].retrieval.cache_hit, true);
  assert.equal(debug.rows[0].retrieval.cache_hit_stage_count, 1);
});

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

function agentMessageEvent(text) {
  return {
    type: "item.completed",
    item: {
      id: "msg_1",
      type: "agent_message",
      text,
    },
  };
}

function runtimeQualityTask(id, qualityThresholds) {
  return {
    id,
    task_class: "architecture_explanation",
    expected_files: [RUNTIME_SERVICE_FILE],
    expected_symbols: [RUN_INDEX_SYMBOL],
    expected_claims: [RUNTIME_REFRESH_CLAIM],
    forbidden_claims: [],
    quality_thresholds: qualityThresholds,
  };
}

function manifestFixture(overrides = {}) {
  return {
    id: "fixture-task",
    suite: "fixture",
    task_class: "architecture_explanation",
    repo: {
      name: "fixture-repo",
      url: "https://example.com/fixture.git",
      ref: "main",
      workspace_root: ".",
    },
    prompt: "Explain the fixture flow.",
    expected_files: ["src/main.rs"],
    expected_symbols: ["run"],
    expected_claims: ["The fixture runs."],
    quality_thresholds: {
      min_expected_anchor_recall: 0.5,
      min_expected_file_recall: 0.5,
      min_expected_symbol_recall: 0.5,
      min_expected_claim_recall: 0.5,
      min_citation_coverage: 0.5,
      max_forbidden_claims: 0,
    },
    ...overrides,
  };
}

async function withManifestFile(manifest, callback) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "codestory-benchmark-manifest-"));
  try {
    const manifestPath = path.join(dir, "fixture.task.json");
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
    return await callback(manifestPath, dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function gitFixture(args, cwd) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

test("categorizes commands without treating source paths as cli invocations", () => {
  assert.equal(commandCategory("& $env:CODESTORY_CLI packet --project . --question flow"), "codestory_cli");
  assert.equal(commandCategory('"$CODESTORY_CLI" index --project . --refresh full'), "codestory_cli");
  assert.equal(commandCategory('& "C:\\tools\\codestory-cli.exe" packet --project . --question flow'), "codestory_cli");
  assert.equal(
    commandCategory(
      String.raw`"C:\Program Files\PowerShell\pwsh.exe" -Command '& $env:CODESTORY_CLI packet --project . --question 'Trace flow' --task-class 'route-tracing' --budget compact --format json"`,
    ),
    "codestory_cli",
  );
  assert.equal(
    commandCategory(
      '"C:\\Program Files\\PowerShell\\pwsh.exe" -Command "& \\"C:\\tools\\codestory-cli.exe\\" packet --project . --question flow"',
    ),
    "codestory_cli",
  );
  assert.equal(commandCategory("rg -n \"run_index\" crates/codestory-cli/src/main.rs"), "shell_search");
  assert.equal(commandCategory('rg -n "codestory-cli" scripts'), "shell_search");
  assert.equal(
    commandCategory(
      '"C:\\Program Files\\PowerShell\\pwsh.exe" -Command \'rg --files crates/codestory-cli crates/codestory-runtime\'',
    ),
    "shell_search",
  );
  assert.equal(
    commandCategory(
      '"C:\\Program Files\\PowerShell\\pwsh.exe" -Command "rg -n \\"codestory-cli index|packet\\" C:\\Users\\alber\\.codex\\memories\\MEMORY.md"',
    ),
    "shell_search",
  );
  assert.equal(commandCategory("Get-Content crates/codestory-cli/src/main.rs"), "direct_file_read");
  assert.equal(commandCategory("Get-Content C:\\tools\\codestory-cli.exe"), "direct_file_read");
  assert.equal(commandCategory("cargo test -p codestory-cli --test runtime_backed_flows"), "build_test");
});

test("packet gate retries only transient retrieval packet failures", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "codestory-packet-gate-retry-"));
  try {
    const retryable = {
      repo: "dart-lang-http",
      task_id: "dart-http-client-flow",
      mode: "cold_cli_packet",
      repeat: 1,
      status: "fail",
      quality_pass: null,
      failure_reasons: ["missing_quality_score"],
    };
    const qualityFailure = {
      repo: "fixture",
      task_id: "quality-failure",
      mode: "cold_cli_packet",
      repeat: 1,
      status: "fail",
      quality_pass: false,
      failure_reasons: ["expected_claim_recall_low"],
    };
    await writeFile(
      packetGateStderrPath(dir, retryable),
      "Error: retrieval_unavailable: project is not in full mode (mode=no_semantic, reason=embedded_vector_index_unavailable)\n",
      "utf8",
    );
    await writeFile(packetGateStderrPath(dir, qualityFailure), "manifest quality failed\n", "utf8");

    assert.deepEqual(retryablePacketGateTaskIds([retryable, qualityFailure], dir), [
      "dart-http-client-flow",
    ]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("packet gate empty selection is explicit exploratory behavior", () => {
  assert.throws(
    () =>
      packetGateSelectionOrThrow(
        [],
        [
          {
            taskId: "python-requests-session-flow",
            reason: "not_improved",
          },
        ],
        {},
      ),
    /allow-empty-packet-gate/,
  );

  assert.equal(packetGateSelectionOrThrow([], [], { allowEmptyPacketGate: true }), null);
  assert.deepEqual(packetGateSelectionOrThrow(["python-requests-session-flow"], [], {}), [
    "python-requests-session-flow",
  ]);
  assert.equal(parseScoreArgs(["--packet-gate", "--allow-empty-packet-gate"]).allowEmptyPacketGate, true);
});

test("rejects manifest repo and workspace paths outside the cache", async () => {
  await withManifestFile(
    manifestFixture({
      repo: {
        name: "../evil",
        url: "https://example.com/evil.git",
        ref: "main",
      },
    }),
    async (manifestPath, dir) => {
      await assert.rejects(
        () => loadTasks({ taskManifest: manifestPath, taskSuite: null, taskIds: null, repoCacheDir: path.join(dir, "repos") }),
        /repo\.name/,
      );
    },
  );

  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "https://example.com/fixture.git",
        ref: "main",
        workspace_root: "../outside",
      },
    }),
    async (manifestPath, dir) => {
      await assert.rejects(
        () => loadTasks({ taskManifest: manifestPath, taskSuite: null, taskIds: null, repoCacheDir: path.join(dir, "repos") }),
        /workspace_root/,
      );
    },
  );

  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "https://example.com/fixture.git",
        ref: "main",
        workspace_root: ".",
        codestory_project_manifest: {
          path: "../../outside.json",
          sha256: "0".repeat(64),
        },
      },
    }),
    async (manifestPath, dir) => {
      await assert.rejects(
        () => loadTasks({ taskManifest: manifestPath, taskSuite: null, taskIds: null, repoCacheDir: path.join(dir, "repos") }),
        /codestory_project_manifest\.path must stay inside/,
      );
    },
  );
});

test("Axios v2 release task preserves v1 evidence while binding its exact project and corpus", async () => {
  const v1Path = path.resolve("benchmarks/tasks/holdout-retrieval/axios-request-dispatch.task.json");
  const v2Path = path.resolve("benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json");
  const projectPath = path.resolve("benchmarks/tasks/release-evidence/axios-js-ts-codestory-project-v2.json");
  const v1 = JSON.parse(await readFile(v1Path, "utf8"));
  const v2 = JSON.parse(await readFile(v2Path, "utf8"));
  for (const field of [
    "prompt",
    "expected_files",
    "expected_symbols",
    "expected_claims",
    "forbidden_claims",
    "quality_thresholds",
  ]) {
    assert.deepEqual(v2[field], v1[field], `${field} must remain identical to the retained v1 task`);
  }
  assert.equal(v2.id, "axios-request-dispatch-v2");
  assert.equal(v2.suite, "release-evidence");
  assert.equal(v2.repo.ref, "ab3f0f9a94853c821cb00f1112788ecdd3ae7ed1");
  assert.equal(
    createHash("sha256").update(await readFile(projectPath)).digest("hex"),
    v2.repo.codestory_project_manifest.sha256,
  );
  const project = JSON.parse(await readFile(projectPath, "utf8"));
  assert.deepEqual(
    project.source_groups.map(({ language, source_paths: sourcePaths }) => ({ language, sourcePaths })),
    [
      { language: "JavaScript", sourcePaths: ["index.js", "lib"] },
      { language: "TypeScript", sourcePaths: ["index.d.ts", "index.d.cts"] },
    ],
  );

  const opts = {
    taskManifest: v2Path,
    taskSuite: null,
    taskIds: null,
    repoCacheDir: path.resolve("target/agent-benchmark/test-axios-v2"),
    materializeRepos: true,
    publishable: true,
    packetRuntimeMode: "cold-cli",
    repeats: 3,
  };
  const tasks = await loadTasks(opts);
  const previous = {
    commit: process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT,
    corpusId: process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID,
    corpusContract: process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT,
  };
  try {
    process.env.CODESTORY_RELEASE_EVIDENCE_COMMIT = "1".repeat(40);
    process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_ID
      = "codestory-release-corpus-v0.16-axios-js-ts-v2";
    process.env.CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT
      = "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v2.json";
    const corpus = await loadReleaseEvidenceCorpusContract(tasks, opts);
    assert.deepEqual(corpus.task_ids, ["axios-request-dispatch-v2"]);
    assert.deepEqual(corpus.project_manifests, {
      "axios-request-dispatch-v2": {
        path: "benchmarks/tasks/release-evidence/axios-js-ts-codestory-project-v2.json",
        sha256: v2.repo.codestory_project_manifest.sha256,
      },
    });
  } finally {
    for (const [key, value] of Object.entries({
      CODESTORY_RELEASE_EVIDENCE_COMMIT: previous.commit,
      CODESTORY_RELEASE_EVIDENCE_CORPUS_ID: previous.corpusId,
      CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT: previous.corpusContract,
    })) {
      if (value == null) delete process.env[key];
      else process.env[key] = value;
    }
  }
});

test("materialization scrubs reusable checkouts before installing the bound project manifest", async (t) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "codestory-materialized-project-"));
  try {
    const source = path.join(dir, "source");
    const origin = path.join(dir, "origin.git");
    const repoCacheDir = path.join(dir, "cache");
    const templatePath = path.join(dir, "project.json");
    const template = '{"name":"fixture","version":1,"source_groups":[]}\n';
    await mkdir(source, { recursive: true });
    gitFixture(["init", "-q"], source);
    gitFixture(["config", "user.email", "fixture@example.invalid"], source);
    gitFixture(["config", "user.name", "Fixture"], source);
    await writeFile(path.join(source, "lib.rs"), "fn main() {}\n");
    gitFixture(["add", "lib.rs"], source);
    gitFixture(["commit", "-qm", "fixture"], source);
    const ref = gitFixture(["rev-parse", "HEAD"], source);
    gitFixture(["clone", "--bare", source, origin], dir);
    await writeFile(templatePath, template, "utf8");
    const manifestPath = path.join(dir, "fixture.task.json");
    await writeFile(
      manifestPath,
      `${JSON.stringify(manifestFixture({
        repo: {
          name: "scrub-fixture",
          url: origin,
          ref,
          workspace_root: ".",
          codestory_project_manifest: {
            path: "project.json",
            sha256: createHash("sha256").update(template).digest("hex"),
          },
        },
      }), null, 2)}\n`,
      "utf8",
    );
    const opts = { taskManifest: manifestPath, taskSuite: null, taskIds: null, repoCacheDir, timeoutMs: 10_000 };
    const tasks = await loadTasks(opts);
    await materializeRepos(tasks, opts);
    const checkout = path.join(repoCacheDir, "scrub-fixture");
    await writeFile(path.join(checkout, "untracked-source.rs"), "fn stale() {}\n");
    await writeFile(path.join(checkout, ".git", "info", "exclude"), "/ignored-source.rs\n", "utf8");
    await writeFile(path.join(checkout, "ignored-source.rs"), "fn stale_ignored() {}\n");

    await materializeRepos(tasks, opts);

    assert.equal(existsSync(path.join(checkout, "untracked-source.rs")), false);
    assert.equal(existsSync(path.join(checkout, "ignored-source.rs")), false);
    assert.equal(await readFile(path.join(checkout, "codestory_project.json"), "utf8"), template);
    assert.equal(gitFixture(["status", "--porcelain"], checkout), "");
    assert.equal(gitFixture(["rev-parse", "HEAD"], checkout), ref);

    const manifestSha256 = createHash("sha256").update(template).digest("hex");
    const provenanceConfig = {
      name: "scrub-fixture",
      path: checkout,
      checkout_path: checkout,
      url: "https://github.com/example/fixture.git",
      ref,
      manifest_url: "https://github.com/example/fixture.git",
      manifest_ref: ref,
      manifest_codestory_project_manifest: {
        source_path: templatePath,
        declared_path: "project.json",
        sha256: manifestSha256,
      },
      installed_codestory_project_manifest: null,
    };
    await t.test("observes the installed manifest after the in-memory receipt is lost", async () => {
      const observed = await repoProvenance(provenanceConfig);
      assert.deepEqual(observed.installed_codestory_project_manifest, {
        source_path: path.relative(path.resolve("."), templatePath).replaceAll(path.sep, "/"),
        declared_sha256: manifestSha256,
        installed_path: "codestory_project.json",
        installed_sha256: manifestSha256,
        ignored: true,
      });
      assert.doesNotMatch(
        repoProvenanceBlockers({ repo_provenance: observed }).join("\n"),
        /CodeStory project manifest/,
      );
    });

    await t.test("keeps tampered and missing installed manifests fail-closed", async () => {
      await writeFile(path.join(checkout, "codestory_project.json"), '{"name":"tampered"}\n', "utf8");
      const tampered = await repoProvenance(provenanceConfig);
      assert.match(
        repoProvenanceBlockers({ repo_provenance: tampered }).join("\n"),
        /installed CodeStory project manifest bytes do not match declared hash/,
      );

      await rm(path.join(checkout, "codestory_project.json"));
      const missing = await repoProvenance(provenanceConfig);
      assert.equal(missing.installed_codestory_project_manifest, null);
      assert.match(
        repoProvenanceBlockers({ repo_provenance: missing }).join("\n"),
        /missing installed CodeStory project manifest provenance/,
      );
    });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("packet-first command renders manifest text for host shells", () => {
  const windowsCommand = packetFirstCommandForPrompt(
    "Inspect $env:SECRET and $(Get-ChildItem), then read John's file.\nNext line.",
    { task_class: "bug_localization" },
    "win32",
  );

  assert.match(
    windowsCommand,
    /--question 'Inspect \$env:SECRET and \$\(Get-ChildItem\), then read John''s file\. Next line\.'/,
  );
  assert.match(windowsCommand, /--task-class 'bug-localization'/);

  const unixCommand = packetFirstCommandForPrompt(
    "Inspect $env:SECRET and $(Get-ChildItem), then read John's file.\nNext line.",
    { task_class: "bug_localization" },
    "linux",
  );

  assert.ok(unixCommand.startsWith('"$CODESTORY_CLI" packet '));
  assert.ok(
    unixCommand.includes(
      "--question 'Inspect $env:SECRET and $(Get-ChildItem), then read John'\\''s file. Next line.'",
    ),
  );
  assert.match(unixCommand, /--task-class 'bug-localization'/);
  assert.throws(
    () => packetFirstCommandForPrompt("Explain the task.", { task_class: "bug_localization; Remove-Item ." }, "linux"),
    /task_class/,
  );
});

test("packet and cache preparation share one explicit agent retrieval namespace", () => {
  const task = {
    prompt: "Explain how Requests dispatch works.",
    task_class: "architecture_explanation",
    expected_files: ["src/requests/api.py", "src/requests/sessions.py"],
    expected_symbols: ["request", "Session.request"],
    expected_symbol_probes: [
      "src/requests/api.py request",
      "src/requests/sessions.py Session.request",
      "src/requests/sessions.py Session.send",
    ],
  };

  assert.deepEqual(packetManifestExtraProbes(task), [
    "src/requests/api.py",
    "src/requests/sessions.py",
    "src/requests/api.py request",
    "src/requests/sessions.py Session.request",
    "src/requests/sessions.py Session.send",
  ]);

  const args = packetCommandArgs({ path: "C:\\repo" }, task);
  assert.equal(args.filter((arg) => arg === "--extra-probe").length, 0);
  for (const anchor of [...task.expected_files, ...task.expected_symbol_probes]) {
    assert.equal(args.includes(anchor), false, `expected anchor leaked into packet arguments: ${anchor}`);
  }
  assert.deepEqual(benchmarkAgentScopeArgs(), ["--profile", "agent", "--run-id", "shared-agent"]);
  assert.deepEqual(args.slice(3, 7), benchmarkAgentScopeArgs());

  const drillArgs = drillPacketCommandArgs({ path: "/tmp/repo" }, task, {}, {
    disposition: {
      kind: "drill_once",
      drill: {
        parent_packet_id: "packet-1",
        core_generation_id: "core-1",
        retrieval_generation: "retrieval-1",
        options: [{ id: "omitted_mandatory_support:symbol%3A42" }],
      },
    },
  });
  assert.ok(drillArgs);
  assert.deepEqual(drillArgs.slice(0, args.length), packetCommandArgs({ path: "/tmp/repo" }, task));
  assert.equal(drillArgs.at(-8), "--parent-packet-id");
  assert.equal(drillArgs.at(-7), "packet-1");
  assert.equal(drillArgs.at(-6), "--option-id");
  assert.equal(drillArgs.at(-5), "omitted_mandatory_support:symbol%3A42");
  assert.equal(drillArgs.at(-4), "--core-generation-id");
  assert.equal(drillArgs.at(-3), "core-1");
  assert.equal(drillArgs.at(-2), "--retrieval-generation");
  assert.equal(drillArgs.at(-1), "retrieval-1");
  assert.equal(drillPacketCommandArgs({ path: "/tmp/repo" }, task, {}, {
    disposition: { kind: "supported" },
  }), null);
  assert.deepEqual(retrievalIndexCommandArgs("C:\\repo"), [
    "retrieval",
    "index",
    "--project",
    "C:\\repo",
    "--profile",
    "agent",
    "--run-id",
    "shared-agent",
    "--refresh",
    "auto",
    "--format",
    "json",
  ]);
  assert.deepEqual(retrievalStatusCommandArgs("C:\\repo"), [
    "retrieval",
    "status",
    "--project",
    "C:\\repo",
    "--profile",
    "agent",
    "--run-id",
    "shared-agent",
    "--format",
    "json",
  ]);

  const diagnosticArgs = packetCommandArgs(
    { path: "C:\\repo" },
    task,
    { diagnosticExtraProbesFromManifest: true },
  );
  const extraProbeIndexes = diagnosticArgs
    .map((arg, index) => (arg === "--extra-probe" ? index : -1))
    .filter((index) => index >= 0);

  assert.equal(extraProbeIndexes.length, 5);
  assert.equal(diagnosticArgs[extraProbeIndexes[0] + 1], "src/requests/api.py");
  assert.equal(diagnosticArgs[extraProbeIndexes[3] + 1], "src/requests/sessions.py Session.request");
});

test("benchmark artifact run ids strip path separators from dynamic parts", () => {
  assert.equal(
    benchmarkRunId(["../repo", "task/id", "with codestory", "01"]),
    "repo-task-id-with-codestory-01",
  );
});

test("v3 benchmark routing treats projection bounds as descriptive rather than read authority", () => {
  const instruction = benchmarkHarness.codeStoryArmInstruction({ schema_version: 3 });

  assert.match(instruction, /output_budget_exceeded gap is descriptive/);
  assert.match(instruction, /does not authorize a source read or another repository tool/);
  assert.match(instruction, /at most one declared continuation/);
  assert.match(instruction, /reassess its returned gaps without another retrieval call/);
  assert.match(instruction, /exact focused source read/);
  assert.match(instruction, /file-local task/);
  assert.match(instruction, /broad flow question.*does not authorize/i);
  assert.match(instruction, /at most one bounded read per authorized path/i);
  assert.match(instruction, /Do not use shell search, Git, or free-form repository recovery/);
  assert.match(instruction, /requested material stage.*direct subject-verb claim/iu);
  assert.match(instruction, /before describing.*gaps/iu);
  assert.match(instruction, /heading.*symbol inventory.*partial observation/iu);
  assert.match(instruction, /claim.*no broader than.*evidence row/iu);
  assert.match(instruction, /higher-level action.*mechanism.*same evidence rows/iu);
  assert.match(instruction, /participates.*calls/iu);
  assert.doesNotMatch(instruction, /direct source reads as packet recovery/);
});

test("publishable benchmark args reject diagnostic packet probes", () => {
  assert.throws(
    () =>
      parseBenchmarkArgs([
        "--publishable",
        "--diagnostic-extra-probes-from-manifest",
      ]),
    /diagnostic-only/,
  );
});

test("publishable repo URL trust only accepts plain GitHub HTTPS repo URLs", () => {
  assert.equal(isTrustedPublishableRepoUrl("https://github.com/expressjs/express.git"), true);
  assert.equal(isTrustedPublishableRepoUrl("https://github.com/expressjs/express"), true);
  assert.equal(isTrustedPublishableRepoUrl("file:///tmp/repo.git"), false);
  assert.equal(isTrustedPublishableRepoUrl("https://example.com/expressjs/express.git"), false);
  assert.equal(isTrustedPublishableRepoUrl("https://github.com/expressjs/express.git?ref=main"), false);
  assert.equal(isTrustedPublishableRepoUrl("https://token@github.com/expressjs/express.git"), false);
});

test("publishable materialization preflight rejects arbitrary URLs and moving refs", async () => {
  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "file:///tmp/fixture.git",
        ref: "main",
        workspace_root: ".",
      },
    }),
    async (manifestPath) => {
      const opts = parseBenchmarkArgs([
        "--task-manifest",
        manifestPath,
        "--publishable",
        "--materialize-repos",
        "--max-source-reads-after-packet",
        "0",
      ]);
      const tasks = await loadTasks(opts);
      const blockers = manifestRepoMaterializationBlockers(tasks, opts);
      const blockerText = blockers.join("\n");

      assert.match(blockerText, /https:\/\/github\.com\/<owner>\/<repo>/);
      assert.match(blockerText, /full immutable commit SHA/);
    },
  );
});

test("publishable materialization preflight stays fail-closed for direct options", async () => {
  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "file:///tmp/fixture.git",
        ref: "main",
        workspace_root: ".",
      },
    }),
    async (manifestPath) => {
      const opts = parseBenchmarkArgs([
        "--task-manifest",
        manifestPath,
        "--materialize-repos",
        "--max-source-reads-after-packet",
        "0",
      ]);
      const tasks = await loadTasks(opts);
      const blockers = manifestRepoMaterializationBlockers(tasks, {
        ...opts,
        publishable: true,
      });

      assert.match(blockers.join("\n"), /full immutable commit SHA/);
    },
  );
});

test("publishable materialization preflight rejects mutable tags before fetch", async () => {
  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "https://github.com/example/fixture.git",
        ref: "v1.2.3",
        workspace_root: ".",
      },
    }),
    async (manifestPath) => {
      const opts = parseBenchmarkArgs([
        "--task-manifest",
        manifestPath,
        "--publishable",
        "--materialize-repos",
        "--max-source-reads-after-packet",
        "0",
      ]);
      const tasks = await loadTasks(opts);
      const blockers = manifestRepoMaterializationBlockers(tasks, opts);

      assert.match(blockers.join("\n"), /full immutable commit SHA/);
    },
  );
});

test("publishable materialization preflight accepts trusted pinned GitHub manifests", async () => {
  await withManifestFile(
    manifestFixture({
      repo: {
        name: "fixture-repo",
        url: "https://github.com/example/fixture.git",
        ref: "1234567890abcdef1234567890abcdef12345678",
        workspace_root: ".",
      },
    }),
    async (manifestPath) => {
      const opts = parseBenchmarkArgs([
        "--task-manifest",
        manifestPath,
        "--publishable",
        "--materialize-repos",
        "--max-source-reads-after-packet",
        "0",
      ]);
      const tasks = await loadTasks(opts);

      assert.deepEqual(manifestRepoMaterializationBlockers(tasks, opts), []);
    },
  );
});

test("path containment rejects sibling-prefix directories", () => {
  const root = path.join(os.tmpdir(), "codestory-agent-benchmark", "repos");
  assert.equal(isPathInside(root, path.join(root, "express")), true);
  assert.equal(isPathInside(root, path.join(os.tmpdir(), "codestory-agent-benchmark", "repos2", "evil")), false);
});

test("reused baseline artifact paths stay inside the previous run directory", () => {
  const runDir = path.join(os.tmpdir(), "codestory-agent-benchmark", "previous-run");
  assert.equal(
    resolveRunArtifactPath(runDir, "codestory.without.01.stdout.jsonl"),
    path.resolve(runDir, "codestory.without.01.stdout.jsonl"),
  );
  assert.equal(resolveRunArtifactPath(runDir, path.join(runDir, "codestory.without.01.stdout.jsonl")), null);
  assert.equal(resolveRunArtifactPath(runDir, "..\\outside.stdout.jsonl"), null);
  assert.equal(resolveRunArtifactPath(runDir, "codestory.without.01.env"), null);
});

test("copying reused baseline artifacts rejects oversized files", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-reused-artifacts-"));
  try {
    const runDir = path.join(root, "previous");
    const outDir = path.join(root, "next");
    await mkdir(runDir, { recursive: true });
    await mkdir(outDir, { recursive: true });
    const sourceName = "codestory.without.01.stdout.jsonl";
    const sourcePath = path.join(runDir, sourceName);
    await writeFile(sourcePath, "");
    await truncate(sourcePath, MAX_REUSED_ARTIFACT_BYTES + 1);

    await assert.rejects(
      () => copyResultArtifact(runDir, outDir, sourceName, "copied.stdout.jsonl"),
      /Refusing to reuse oversized baseline artifact/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("copying reused baseline artifacts rejects absolute source paths", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-reused-artifacts-"));
  try {
    const runDir = path.join(root, "previous");
    const outDir = path.join(root, "next");
    await mkdir(runDir, { recursive: true });
    await mkdir(outDir, { recursive: true });
    const sourcePath = path.join(runDir, "codestory.without.01.stdout.jsonl");
    await writeFile(sourcePath, "{}\n");

    assert.equal(await copyResultArtifact(runDir, outDir, sourcePath, "copied.stdout.jsonl"), null);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("Windows Codex runner args reject cmd metacharacters", () => {
  assert.doesNotThrow(() => assertSafeWindowsCmdArgs(["exec", "--cd", "C:\\Users\\alber\\source\\repos\\codestory"]));
  assert.throws(
    () => assertSafeWindowsCmdArgs(["exec", "--cd", "C:\\repo&whoami"]),
    /unsafe Windows cmd\.exe argument/,
  );
});

test("holdout-retrieval suite loads three OSS manifests", async () => {
  const tasks = await loadTasks({
    taskSuite: "holdout-retrieval",
    taskManifest: null,
    taskIds: null,
    materializeRepos: true,
    repoCacheDir: path.join("target", "agent-benchmark", "repos"),
  });

  assert.equal(tasks.length, 3);
  assert.deepEqual(
    tasks.map((task) => task.id).sort(),
    ["axios-request-dispatch", "redis-server-event-loop", "ripgrep-search-pipeline"],
  );
  for (const task of tasks) {
    assert.equal(task.suite, "holdout-retrieval");
    assert.equal(task.task_class, "architecture_explanation");
    assert.ok(task.repo_metadata?.url);
    assert.ok(task.repo_metadata?.ref);
    assert.notEqual(task.repo_metadata.ref, "local");
  }
});

test("public-core corpus keeps publishable coverage locked", async () => {
  const tasks = await loadTasks({
    taskSuite: "public-core",
    taskManifest: null,
    taskIds: null,
    repoCacheDir: path.join("target", "agent-benchmark", "repos"),
  });
  const audit = publicCoreCorpusAudit(tasks);

  assert.equal(tasks.length, 19);
  assert.equal(audit.repo_count, 5);
  assert.deepEqual(Object.keys(audit.class_counts), [
    "architecture_explanation",
    "bug_localization",
    "change_impact",
    "edit_planning",
    "route_tracing",
    "symbol_ownership",
  ]);
  assert.deepEqual(Object.values(audit.class_counts), [4, 3, 3, 3, 3, 3]);
  assert.deepEqual(audit.missing_classes, []);
  assert.deepEqual(audit.underfilled_classes, []);
});

test("analyzes transcript command friction and scores manifest anchors", () => {
  const events = [
    { type: "thread.started" },
    { type: "turn.started" },
    commandEvent("cmd_1", "item.started", "& $env:CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "cmd_1",
      "item.completed",
      "& $env:CODESTORY_CLI packet --project . --question flow",
      "Evidence: crates/codestory-cli/src/main.rs RuntimeContext::ensure_open",
    ),
    commandEvent("cmd_2", "item.started", "rg -n \"run_index\" crates"),
    commandEvent("cmd_2", "item.completed", "rg -n \"run_index\" crates", "crates/codestory-cli/src/main.rs:1:run_index"),
    commandEvent("cmd_3", "item.started", "Get-Content crates/codestory-cli/src/main.rs"),
    commandEvent("cmd_3", "item.completed", "Get-Content crates/codestory-cli/src/main.rs", "fn run_index() {}"),
    commandEvent("cmd_4", "item.started", "Get-Content crates/codestory-cli/src/main.rs"),
    commandEvent("cmd_4", "item.completed", "Get-Content crates/codestory-cli/src/main.rs", "fn run_index() {}"),
    commandEvent("cmd_7", "item.started", `$p='"'crates/codestory-runtime/src/lib.rs'; Get-Content $p`),
    commandEvent("cmd_7", "item.completed", `$p='"'crates/codestory-runtime/src/lib.rs'; Get-Content $p`, "pub struct RuntimeContext;"),
    commandEvent("cmd_5", "item.started", "git status --short"),
    commandEvent("cmd_5", "item.completed", "git status --short", ""),
    commandEvent("cmd_6", "item.started", "cargo test -p codestory-cli --test runtime_backed_flows"),
    commandEvent("cmd_6", "item.completed", "cargo test -p codestory-cli --test runtime_backed_flows", "ok"),
    {
      type: "item.completed",
      item: {
        id: "msg_1",
        type: "agent_message",
        text: "Full indexing starts in crates/codestory-cli/src/main.rs and calls RuntimeContext::ensure_open.",
      },
    },
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.command_categories.shell_search, 1);
  assert.equal(analysis.command_categories.direct_file_read, 3);
  assert.equal(analysis.command_categories.git, 1);
  assert.equal(analysis.command_categories.build_test, 1);
  assert.equal(analysis.ordinary_source_reads_after_first_packet, 3);
  assert.deepEqual(analysis.direct_file_reads_duplicated, {
    "crates/codestory-cli/src/main.rs": 2,
  });

  const quality = scoreQuality(events, {
    id: "fixture",
    task_class: "architecture_explanation",
    expected_files: ["crates/codestory-cli/src/main.rs"],
    expected_verification_files: ["crates/codestory-cli/tests/runtime_backed_flows.rs"],
    expected_symbols: ["RuntimeContext::ensure_open", "MissingSymbol"],
    expected_claims: ["Full indexing starts"],
    forbidden_claims: ["remote service is required"],
    quality_thresholds: {
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 0.5,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      min_expected_anchor_recall: 0.75,
      max_forbidden_claims: 0,
    },
  });

  assert.equal(quality.pass, true);
  assert.equal(quality.expected_files.recall, 1);
  assert.equal(quality.expected_symbols.recall, 0.5);
  assert.deepEqual(quality.missed_anchors.symbols, ["MissingSymbol"]);
  assert.equal(quality.expected_verification_files.recall, 0);
  assert.deepEqual(quality.missed_anchors.verification_files, [
    "crates/codestory-cli/tests/runtime_backed_flows.rs",
  ]);
  assert.equal(quality.citation_coverage.recall, 1);
});

test("counts direct source reads for every supported language extension family", () => {
  const paths = [
    "src/main.rs",
    "src/app.py",
    "src/App.java",
    "src/index.js",
    "src/index.tsx",
    "include/fmt/base.hpp",
    "src/server.c",
    "router.go",
    "lib/site.rb",
    "src/Logger.php",
    "src/Mapper.cs",
    "src/Main.kt",
    "Package.swift",
    "lib/client.dart",
    "nvm.sh",
    "index.html",
    "styles/site.css",
    "schema/chinook.sql",
  ];
  const events = paths.flatMap((sourcePath, index) => [
    commandEvent(`cmd_${index}`, "item.started", `Get-Content ${sourcePath}`),
    commandEvent(`cmd_${index}`, "item.completed", `Get-Content ${sourcePath}`, "source"),
  ]);

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.direct_file_read, paths.length);
  assert.equal(analysis.direct_source_reads_total, paths.length);
});

test("transcript analysis authorizes source reads only from user-named files or prior explicit evidence gaps", () => {
  const project = "/tmp/exact-transcript-project";
  const namedEvents = [
    commandEvent("named", "item.started", "Get-Content src/named.ts"),
    commandEvent("named", "item.completed", "Get-Content src/named.ts", "source"),
  ];
  const named = analyzeTranscript(namedEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Inspect src/named.ts and explain the call.", file_local: true },
  });
  assert.equal(named.direct_source_reads[0].authorization.reason, "user_named_file");

  const broadNamed = analyzeTranscript(namedEvents, project, {
    arm: "candidate_0_18",
    task: {
      task_class: "route_tracing",
      prompt: "Trace the broad flow through src/named.ts and the rest of the subsystem.",
    },
  });
  assert.equal(broadNamed.direct_source_reads[0].authorization.status, "unauthorized");

  const gapEvents = [
    commandEvent("packet", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent("packet", "item.completed", "$CODESTORY_CLI packet --project . --question flow", "Unknown: explicit evidence gap for src/gap.ts"),
    commandEvent("gap-read", "item.started", "Get-Content src/gap.ts"),
    commandEvent("gap-read", "item.completed", "Get-Content src/gap.ts", "source"),
  ];
  const gap = analyzeTranscript(gapEvents, project, {
    arm: "published_0_17_4",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(gap.direct_source_reads[0].authorization.reason, "explicit_evidence_gap");
  assert.equal(gap.direct_source_reads[0].authorization.evidence_command_id, "packet");

  const repeatedGapRead = analyzeTranscript([
    ...gapEvents,
    commandEvent("gap-read-again", "item.started", "Get-Content src/gap.ts"),
    commandEvent("gap-read-again", "item.completed", "Get-Content src/gap.ts", "source again"),
  ], project, {
    arm: "published_0_17_4",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(repeatedGapRead.direct_source_reads[0].authorization.status, "authorized");
  assert.equal(repeatedGapRead.direct_source_reads[1].authorization.status, "unauthorized");

  const distinctSuffixReads = analyzeTranscript([
    commandEvent("vendor-read", "item.started", "Get-Content vendor/src/foo.rs"),
    commandEvent("vendor-read", "item.completed", "Get-Content vendor/src/foo.rs", "vendor"),
    commandEvent("source-read", "item.started", "Get-Content src/foo.rs"),
    commandEvent("source-read", "item.completed", "Get-Content src/foo.rs", "source"),
  ], project, {
    arm: "candidate_0_18",
    task: {
      file_local: true,
      prompt: "Compare `vendor/src/foo.rs` with `src/foo.rs`.",
    },
  });
  assert.deepEqual(
    distinctSuffixReads.direct_source_reads.map((read) => read.authorization.status),
    ["authorized", "authorized"],
  );

  if (process.platform !== "win32") {
    const caseDistinctReads = analyzeTranscript([
      commandEvent("upper-read", "item.started", "Get-Content src/Foo.rs"),
      commandEvent("upper-read", "item.completed", "Get-Content src/Foo.rs", "upper"),
      commandEvent("lower-read", "item.started", "Get-Content src/foo.rs"),
      commandEvent("lower-read", "item.completed", "Get-Content src/foo.rs", "lower"),
    ], project, {
      arm: "candidate_0_18",
      task: {
        file_local: true,
        prompt: "Compare `src/Foo.rs` with `src/foo.rs`.",
      },
    });
    assert.deepEqual(
      caseDistinctReads.direct_source_reads.map((read) => read.authorization.status),
      ["authorized", "authorized"],
    );
  }

  const v3GapEvents = [
    commandEvent("packet-v3", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "packet-v3",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      JSON.stringify({
        schema_version: 3,
        status: "no_useful_evidence",
        gaps: [{ kind: "evidence_missing", message: "Missing evidence for src/v3-gap.ts" }],
      }),
    ),
    commandEvent("v3-gap-read", "item.started", "Get-Content src/v3-gap.ts"),
    commandEvent("v3-gap-read", "item.completed", "Get-Content src/v3-gap.ts", "source"),
  ];
  const v3Gap = analyzeTranscript(v3GapEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(v3Gap.direct_source_reads[0].authorization.reason, "explicit_evidence_gap");
  assert.equal(v3Gap.direct_source_reads[0].authorization.evidence_command_id, "packet-v3");

  const unrelatedGapEvents = [
    ...v3GapEvents.slice(0, 2),
    commandEvent("unrelated-read", "item.started", "Get-Content src/unrelated.ts"),
    commandEvent("unrelated-read", "item.completed", "Get-Content src/unrelated.ts", "source"),
  ];
  const unrelatedGap = analyzeTranscript(unrelatedGapEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(unrelatedGap.direct_source_reads[0].authorization.status, "unauthorized");

  const outputBudgetEvents = [
    commandEvent("packet-budget", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "packet-budget",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      JSON.stringify({
        status: "unavailable",
        reason: "output_budget_exceeded for src/budget.ts",
      }),
    ),
    commandEvent("budget-read", "item.started", "Get-Content src/budget.ts"),
    commandEvent("budget-read", "item.completed", "Get-Content src/budget.ts", "source"),
  ];
  const outputBudget = analyzeTranscript(outputBudgetEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(outputBudget.direct_source_reads[0].authorization.status, "unauthorized");

  const failedGapEvents = [
    commandEvent("packet-failed", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "packet-failed",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      "Unknown: missing evidence for src/failed.ts",
      1,
    ),
    commandEvent("failed-read", "item.started", "Get-Content src/failed.ts"),
    commandEvent("failed-read", "item.completed", "Get-Content src/failed.ts", "source"),
  ];
  const failedGap = analyzeTranscript(failedGapEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(failedGap.direct_source_reads[0].authorization.status, "unauthorized");

  const retroactiveGapEvents = [
    commandEvent("packet-late", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent("late-read", "item.started", "Get-Content src/late.ts"),
    commandEvent(
      "packet-late",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      "Unknown: missing evidence for src/late.ts",
    ),
    commandEvent("late-read", "item.completed", "Get-Content src/late.ts", "source"),
  ];
  const retroactiveGap = analyzeTranscript(retroactiveGapEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(retroactiveGap.direct_source_reads[0].authorization.status, "unauthorized");

  const citedPathEvents = [
    commandEvent("packet-cited", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "packet-cited",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      "Unknown: missing evidence for src/other.ts, cited src/cited.ts",
    ),
    commandEvent("cited-read", "item.started", "Get-Content src/cited.ts"),
    commandEvent("cited-read", "item.completed", "Get-Content src/cited.ts", "source"),
  ];
  const citedPath = analyzeTranscript(citedPathEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(citedPath.direct_source_reads[0].authorization.status, "unauthorized");

  const mcpGapBody = {
    status: "no_useful_evidence",
    gaps: [{ kind: "evidence_missing", message: "Missing evidence for src/mcp-gap.ts" }],
  };
  const mcpGapEvents = [
    {
      type: "item.started",
      item: { id: "mcp-gap", type: "mcp_tool_call", server: "codestory", tool: "packet" },
    },
    {
      type: "item.completed",
      item: {
        id: "mcp-gap",
        type: "mcp_tool_call",
        server: "codestory",
        tool: "packet",
        result: {
          structuredContent: mcpGapBody,
          content: [{ type: "text", text: JSON.stringify(mcpGapBody) }],
        },
      },
    },
    commandEvent("mcp-gap-read", "item.started", "Get-Content src/mcp-gap.ts"),
    commandEvent("mcp-gap-read", "item.completed", "Get-Content src/mcp-gap.ts", "source"),
  ];
  const mcpGap = analyzeTranscript(mcpGapEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(mcpGap.direct_source_reads[0].authorization.reason, "explicit_evidence_gap");
  assert.equal(mcpGap.direct_source_reads[0].authorization.evidence_event_index, 1);

  const mismatchedMirrorEvents = structuredClone(mcpGapEvents);
  mismatchedMirrorEvents[1].item.result.structuredContent.gaps[0].message = "Missing evidence for src/other.ts";
  const mismatchedMirror = analyzeTranscript(mismatchedMirrorEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(mismatchedMirror.direct_source_reads[0].authorization.status, "unauthorized");

  const unauthorized = analyzeTranscript(namedEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(unauthorized.direct_source_reads[0].authorization.status, "unauthorized");

  const prefixCollision = analyzeTranscript(namedEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Inspect src/named.tsx and explain the call." },
  });
  assert.equal(prefixCollision.direct_source_reads[0].authorization.status, "unauthorized");

  const escapedEvents = [
    commandEvent("escaped", "item.started", "Get-Content ../secret.ts"),
    commandEvent("escaped", "item.completed", "Get-Content ../secret.ts", "source"),
  ];
  const escaped = analyzeTranscript(escapedEvents, project, {
    arm: "candidate_0_18",
    task: { prompt: "Inspect ../secret.ts and explain the call." },
  });
  assert.equal(escaped.direct_source_reads[0].authorization.status, "unauthorized");

  const baseline = analyzeTranscript(gapEvents.slice(2), project, {
    arm: "without_codestory",
    task: { prompt: "Explain the flow." },
  });
  assert.equal(baseline.direct_source_reads[0].authorization.status, "baseline_local_exploration");
});

test("file-local read authorization survives manifest normalization and result snapshots", async () => {
  await withManifestFile(
    manifestFixture({
      file_local: true,
      prompt: "Inspect src/main.rs and explain its local behavior.",
    }),
    async (manifestPath, dir) => {
      const [task] = await loadTasks({
        taskManifest: manifestPath,
        taskSuite: null,
        taskIds: null,
        repoCacheDir: path.join(dir, "repos"),
      });
      const snapshot = taskSnapshotForResult(task);
      assert.equal(task.file_local, true);
      assert.equal(snapshot.file_local, true);

      const project = path.join(dir, "repos", "fixture-repo");
      const analysis = analyzeTranscript([
        commandEvent("named-read", "item.started", "Get-Content src/main.rs"),
        commandEvent("named-read", "item.completed", "Get-Content src/main.rs", "source"),
      ], project, { arm: "candidate_0_18", task: snapshot });
      assert.equal(analysis.direct_source_reads[0].authorization.reason, "user_named_file");
    },
  );

  await withManifestFile(
    manifestFixture({ file_local: "true" }),
    async (manifestPath, dir) => {
      await assert.rejects(
        () => loadTasks({
          taskManifest: manifestPath,
          taskSuite: null,
          taskIds: null,
          repoCacheDir: path.join(dir, "repos"),
        }),
        /file_local must be a boolean/,
      );
    },
  );
});

test("transcript analysis authorizes an exact source read from a continuation's returned material gap", () => {
  const project = "/tmp/exact-transcript-project";
  const events = [
    commandEvent("packet", "item.started", "$CODESTORY_CLI packet --project . --question flow"),
    commandEvent(
      "packet",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow",
      JSON.stringify({
        schema_version: 3,
        status: "continuation_available",
        gaps: [{ kind: "continuation_required", message: "Continue the bounded lookup" }],
      }),
    ),
    commandEvent(
      "continuation",
      "item.started",
      "$CODESTORY_CLI packet --project . --question flow --parent-packet-id continuation-v3",
    ),
    commandEvent(
      "continuation",
      "item.completed",
      "$CODESTORY_CLI packet --project . --question flow --parent-packet-id continuation-v3",
      JSON.stringify({
        schema_version: 3,
        status: "no_useful_evidence",
        gaps: [{ kind: "evidence_missing", message: "Missing evidence for src/continued-gap.ts" }],
      }),
    ),
    commandEvent("continued-gap-read", "item.started", "Get-Content src/continued-gap.ts"),
    commandEvent("continued-gap-read", "item.completed", "Get-Content src/continued-gap.ts", "source"),
  ];

  const analysis = analyzeTranscript(events, project, {
    arm: "candidate_0_18",
    task: { prompt: "Explain the flow." },
  });

  assert.equal(analysis.direct_source_reads[0].authorization.status, "authorized");
  assert.equal(analysis.direct_source_reads[0].authorization.reason, "explicit_evidence_gap");
  assert.equal(analysis.direct_source_reads[0].authorization.evidence_command_id, "continuation");
});

test("exact telemetry sees malformed JSONL web context and an empty local transcript through the real parser", () => {
  const jsonl = [
    JSON.stringify({ type: "item.started", item: { id: "web", type: "web_search", query: "upstream source" } }),
    "{malformed-jsonl",
  ].join("\n");
  const { parsed, malformed } = parseJsonLines(jsonl);
  assert.equal(parsed.length, 1);
  assert.equal(malformed.length, 1);
  const web = analyzeTranscript(parsed, "/tmp/exact-parser", {
    arm: "candidate_0_18",
    task: { prompt: "Explain the local repository." },
  });
  assert.equal(web.external_context_tool_calls, 1);
  assert.equal(web.tool_categories.web_search, 1);
  const empty = analyzeTranscript([], "/tmp/exact-parser", {
    arm: "without_codestory",
    task: { prompt: "Explain the local repository." },
  });
  assert.equal(empty.command_count, 0);
  assert.equal(empty.direct_source_reads_total, 0);
});

test("counts PowerShell LiteralPath source reads after a CodeStory packet", () => {
  const command = String.raw`"C:\\Program Files\\PowerShell\\pwsh.exe" -Command '$lines = Get-Content -LiteralPath '"'src/index/use-swr.ts'
for ($i = 1; $i -le 2; $i++) { "{0}: {1}" -f $i, $lines[$i - 1] }'`;
  const events = [
    commandEvent("packet", "item.started", "& $env:CODESTORY_CLI packet --project . --question flow"),
    commandEvent("packet", "item.completed", "& $env:CODESTORY_CLI packet --project . --question flow", "{}"),
    commandEvent("read", "item.started", command),
    commandEvent("read", "item.completed", command, "export default useSWR"),
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.direct_file_read, 1);
  assert.equal(analysis.direct_source_reads_total, 1);
  assert.equal(analysis.ordinary_source_reads_after_first_packet, 1);
  assert.deepEqual(analysis.direct_file_reads_duplicated, {});
});

test("multiline numbered reads do not treat the next nl command as a sed path", () => {
  const command = `/bin/zsh -lc "nl -ba src/first.java | sed -n '1,20p'
nl -ba src/second.java | sed -n '30,40p'"`;
  const events = [
    commandEvent("read", "item.started", command),
    commandEvent("read", "item.completed", command, "source"),
  ];
  const analysis = analyzeTranscript(events);
  assert.deepEqual(
    analysis.direct_source_reads.map((read) => read.path),
    ["src/first.java", "src/second.java"],
  );
});

test("counts modern Codex JSONL tool categories including web search", () => {
  const events = [
    {
      type: "item.started",
      item: {
        id: "item_web",
        type: "web_search",
        query: "github psf requests api.py",
      },
    },
    {
      type: "item.completed",
      item: {
        id: "item_web",
        type: "web_search",
        query: "github psf requests api.py",
      },
    },
    {
      type: "item.started",
      item: {
        id: "item_mcp",
        type: "mcp_tool_call",
        server: "codex",
        tool: "list_mcp_resources",
      },
    },
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_count, 0);
  assert.equal(analysis.tool_categories.web_search, 1);
  assert.equal(analysis.tool_categories.mcp_tool_call, 1);
  assert.equal(analysis.external_context_tool_calls, 1);

  const blockers = agentPublishableBlockers([
    {
      status: "pass",
      arm: "without_codestory",
      wall_ms: 1,
      usage: { total_tokens: 1 },
      tool_calls_observed: 1,
      quality: { pass: true },
      transcript_analysis: analysis,
    },
  ]);
  assert.match(blockers[0].reasons.join("\n"), /external web\/search tool calls=1 > 0/);
});

test("interaction turns count agent messages and tool actions but exclude reasoning and errors", () => {
  const telemetry = interactionTurnTelemetry([
    { type: "item.completed", item: { type: "reasoning" } },
    { type: "item.completed", item: { type: "error" } },
    { type: "item.completed", item: { type: "agent_message" } },
    { type: "item.completed", item: { type: "command_execution", status: "completed" } },
    { type: "item.completed", item: { type: "mcp_tool_call", status: "failed" } },
  ]);
  assert.deepEqual(telemetry, {
    total: 3,
    model_messages: 1,
    tool_actions: 2,
    failed_tool_actions: 1,
    reasoning_items_excluded: 1,
    error_items_excluded: 1,
    taxonomy: "completed_agent_messages_plus_tool_actions_v1",
  });
});

test("same-binary identity requires every completed MCP call to declare the prelude SHA", () => {
  const sha = "a".repeat(64);
  assert.equal(codeStoryBinaryIdentity(sha, {
    codestory_mcp_completed_calls_observed: 0,
    codestory_mcp_runtime_identities: [],
  }).status, "prelude_only");
  assert.equal(codeStoryBinaryIdentity(sha, {
    codestory_mcp_completed_calls_observed: 1,
    codestory_mcp_runtime_identities: [{ cli_sha256: sha }],
  }).status, "exact_match");
  assert.equal(codeStoryBinaryIdentity(sha, {
    codestory_mcp_completed_calls_observed: 1,
    codestory_mcp_runtime_identities: [{ cli_sha256: "b".repeat(64) }],
  }).status, "mismatch");
  assert.equal(codeStoryBinaryIdentity(sha, {
    codestory_mcp_completed_calls_observed: 1,
    codestory_mcp_runtime_identities: [{ cli_sha256: null }],
  }).status, "mcp_sha_missing_or_invalid");
});

test("counts only started CodeStory MCP calls", () => {
  const events = [
    {
      type: "item.started",
      item: { id: "codestory", type: "mcp_tool_call", server: "codestory", tool: "packet" },
    },
    {
      type: "item.completed",
      item: {
        id: "codestory",
        type: "mcp_tool_call",
        server: "codestory",
        tool: "packet",
        result: null,
      },
    },
    {
      type: "item.started",
      item: { id: "other", type: "mcp_tool_call", server: "other", tool: "packet" },
    },
  ];
  const analysis = analyzeTranscript(events);
  assert.equal(analysis.codestory_mcp_tool_calls_observed, 1);
  assert.equal(analysis.codestory_mcp_completed_calls_observed, 0);
  assert.deepEqual(analysis.codestory_mcp_runtime_identities, []);
});

test("extracts managed runtime identity from completed CodeStory MCP results", () => {
  const identity = {
    plugin_version: "0.17.0",
    plugin_cli_version: "0.17.0",
    cli_version: "0.17.0",
    cli_sha256: "a".repeat(64),
    cli_source: "managed",
    pinned_pair_matches: true,
    known_override_skew_channel: false,
  };
  const analysis = analyzeTranscript([
    {
      type: "item.started",
      item: { id: "codestory", type: "mcp_tool_call", server: "codestory", tool: "packet" },
    },
    {
      type: "item.completed",
      item: {
        id: "codestory",
        type: "mcp_tool_call",
        server: "codestory",
        tool: "packet",
        result: { _meta: { codestory_publication: { contract_runtime: identity } } },
      },
    },
  ]);
  assert.equal(analysis.codestory_mcp_tool_calls_observed, 1);
  assert.equal(analysis.codestory_mcp_completed_calls_observed, 1);
  assert.deepEqual(analysis.codestory_mcp_runtime_identities, [identity]);
});

test("publishable measured rows fail closed without managed runtime identity", () => {
  const blockers = agentPublishableBlockers(
    [
      {
        arm: "with_codestory",
        status: "pass",
        wall_ms: 1,
        usage: { total_tokens: 1 },
        tool_calls_observed: 1,
        packet_first_required: false,
        packet_first_pass: true,
        transcript_analysis: {
          command_count: 1,
          command_categories: {},
          codestory_mcp_tool_calls_observed: 1,
          codestory_mcp_completed_calls_observed: 1,
          codestory_mcp_runtime_identities: [],
          external_context_tool_calls: 0,
        },
      },
    ],
    { publishable: true },
  );
  assert.match(
    blockers.flatMap((blocker) => blocker.reasons).join("\n"),
    /no managed CodeStory runtime identity/,
  );
});

test("publishable measured rows accept a validated managed packet prelude without a redundant MCP call", () => {
  const transcriptAnalysis = {
    codestory_mcp_tool_calls_observed: 0,
    codestory_mcp_completed_calls_observed: 0,
    codestory_mcp_runtime_identities: [],
  };
  const managedRuntime = {
    plugin_version: "0.17.0",
    plugin_cli_version: "0.17.0",
    cli_version: "0.17.0",
    cli_source: "managed",
    pinned_pair_matches: true,
    known_override_skew_channel: false,
  };
  const valid = publishableWithCodeStoryResult({
    transcript_analysis: transcriptAnalysis,
    codestory_harness_prelude: { packet_contract_runtime: managedRuntime },
  });

  assert.deepEqual(
    agentPublishableBlockers([valid], {
      publishable: true,
      maxSourceReadsAfterPacket: 0,
    }),
    [],
  );

  for (const codestoryPrelude of [
    { status: "fail", packet_contract_runtime: managedRuntime },
    {
      packet_contract_runtime: {
        ...managedRuntime,
        cli_source: "override",
      },
    },
  ]) {
    const invalid = publishableWithCodeStoryResult({
      transcript_analysis: transcriptAnalysis,
      codestory_harness_prelude: codestoryPrelude,
    });
    assert.ok(
      agentPublishableBlockers([invalid], {
        publishable: true,
        maxSourceReadsAfterPacket: 0,
      }).length > 0,
    );
  }
});

test("summarizes A/B cost accounting totals and ratios", () => {
  const costAccounting = summarizeCostAccounting([
    {
      arm: "without_codestory",
      status: "pass",
      wall_ms: 200,
      agent_runner_wall_ms: 190,
      baseline_harness_prelude: {
        wall_ms: 10,
      },
      usage: { input_tokens: 80, output_tokens: 20, total_tokens: 100 },
      estimated_cost_usd: 0.02,
      tool_calls_observed: 4,
      transcript_analysis: {
        command_count: 4,
        tool_categories: { command_execution: 4 },
        command_categories: { shell_search: 2, direct_file_read: 2 },
        direct_source_reads_total: 2,
        external_context_tool_calls: 0,
      },
    },
    {
      arm: "with_codestory",
      status: "pass",
      wall_ms: 50,
      agent_runner_wall_ms: 40,
      usage: { input_tokens: 30, output_tokens: 10, total_tokens: 40 },
      estimated_cost_usd: 0.01,
      tool_calls_observed: 1,
      codex_tool_calls_observed: 0,
      codestory_harness_prelude: {
        wall_ms: 10,
      },
      codestory_cache_provenance: {
        cache_preparation: { preparation_wall_ms: 10 },
      },
      transcript_analysis: {
        command_count: 1,
        tool_categories: { command_execution: 1 },
        command_categories: { codestory_cli: 1 },
        direct_source_reads_total: 0,
        external_context_tool_calls: 0,
      },
    },
    {
      arm: "with_codestory",
      status: "fail",
      wall_ms: 5,
      usage: null,
      estimated_cost_usd: null,
      tool_calls_observed: 1,
      transcript_analysis: {
        command_count: 1,
        tool_categories: { command_execution: 1 },
        command_categories: { codestory_cli: 1 },
        direct_source_reads_total: 0,
        external_context_tool_calls: 0,
      },
    },
  ]);

  assert.equal(costAccounting.arms.with_codestory.runs, 2);
  assert.equal(costAccounting.arms.with_codestory.failed_runs, 1);
  assert.equal(costAccounting.arms.with_codestory.missing_token_usage_runs, 1);
  assert.equal(costAccounting.arms.without_codestory.time_spent_ms.agent_runner, 190);
  assert.equal(costAccounting.arms.without_codestory.time_spent_ms.baseline_harness_prelude, 10);
  assert.equal(costAccounting.arms.with_codestory.time_spent_ms.runner_wall, 55);
  assert.equal(costAccounting.arms.with_codestory.time_spent_ms.agent_runner, 45);
  assert.equal(costAccounting.arms.with_codestory.time_spent_ms.codestory_harness_prelude, 10);
  assert.equal(costAccounting.arms.with_codestory.time_spent_ms.all_in, 65);
  assert.equal(costAccounting.arms.with_codestory.tokens_spent.total_tokens, 40);
  assert.equal(costAccounting.arms.with_codestory.tool_calls.codex_observed, 0);
  assert.equal(costAccounting.arms.without_codestory.tool_calls.observed, 4);
  assert.equal(costAccounting.arms.without_codestory.commands.categories.shell_search, 2);
  assert.equal(costAccounting.with_vs_without.total_tokens.ratio, 0.4);
  assert.equal(costAccounting.with_vs_without.all_in_wall_ms.ratio, 0.325);
  assert.equal(costAccounting.with_vs_without.tool_calls.with_minus_without, -2);
});

test("renders ineligible comparative wall time without losing other accounting", () => {
  const costAccounting = summarizeCostAccounting([
    {
      arm: "without_codestory",
      status: "pass",
      wall_ms: 200,
      comparative_wall_time_eligible: false,
      usage: { input_tokens: 80, output_tokens: 20, total_tokens: 100 },
      tool_calls_observed: 4,
      transcript_analysis: { command_count: 4, command_categories: {} },
    },
    {
      arm: "with_codestory",
      status: "pass",
      wall_ms: 50,
      comparative_wall_time_eligible: true,
      usage: { input_tokens: 30, output_tokens: 10, total_tokens: 40 },
      tool_calls_observed: 1,
      transcript_analysis: { command_count: 1, command_categories: {} },
    },
    {
      arm: "with_codestory",
      status: "fail",
      wall_ms: 5,
      usage: null,
      tool_calls_observed: 1,
      transcript_analysis: { command_count: 1, command_categories: {} },
    },
    {
      arm: "with_codestory",
      status: "cancelled",
      wall_ms: 1,
      usage: null,
      tool_calls_observed: 0,
      transcript_analysis: { command_count: 0, command_categories: {} },
    },
  ]);

  assert.equal(costAccounting.with_vs_without.runner_wall_ms, null);
  assert.equal(costAccounting.with_vs_without.all_in_wall_ms, null);
  const markdown = markdownCostAccounting(costAccounting).join("\n");
  assert.match(
    markdown,
    /\| runner_wall_ms \| ineligible \| ineligible \| ineligible \| ineligible \|/,
  );
  assert.match(
    markdown,
    /\| all_in_wall_ms \| ineligible \| ineligible \| ineligible \| ineligible \|/,
  );
  assert.match(markdown, /\| total_tokens \| 40 \| 100 \| -60 \| 0\.4 \|/);
  assert.match(markdown, /\| tool_calls \| 2 \| 4 \| -2 \| 0\.5 \|/);
});

test("parses JSONL transcript text before analysis", () => {
  const jsonl = [
    JSON.stringify(commandEvent("cmd_1", "item.started", "codestory-cli packet --project . --question flow")),
    JSON.stringify(
      commandEvent(
        "cmd_1",
        "item.completed",
        "codestory-cli packet --project . --question flow",
        "crates/codestory-cli/src/main.rs",
      ),
    ),
    "not json",
    "",
  ].join("\n");

  const { parsed, malformed } = parseJsonLines(jsonl);
  assert.equal(parsed.length, 2);
  assert.equal(malformed.length, 1);
  assert.equal(analyzeTranscript(parsed).command_categories.codestory_cli, 1);
});

test("requires packet as the CodeStory subcommand for packet-first telemetry", () => {
  const events = [
    commandEvent("cmd_1", "item.started", 'codestory-cli search --project . --query "packet"'),
    commandEvent("cmd_1", "item.completed", 'codestory-cli search --project . --query "packet"', "ok"),
    commandEvent("cmd_help", "item.started", 'codestory-cli packet --help'),
    commandEvent("cmd_help", "item.completed", 'codestory-cli packet --help', "Usage: codestory-cli packet", 0),
    commandEvent(
      "cmd_2",
      "item.started",
      '"C:\\Program Files\\PowerShell\\pwsh.exe" -Command "rg -n \\"codestory-cli index|packet\\" C:\\Users\\alber\\.codex\\memories\\MEMORY.md"',
    ),
    commandEvent(
      "cmd_2",
      "item.completed",
      '"C:\\Program Files\\PowerShell\\pwsh.exe" -Command "rg -n \\"codestory-cli index|packet\\" C:\\Users\\alber\\.codex\\memories\\MEMORY.md"',
      "memory hit",
    ),
    commandEvent("cmd_3", "item.started", '& "C:\\tools\\codestory-cli.exe" packet --project . --question flow'),
    commandEvent("cmd_3", "item.completed", '& "C:\\tools\\codestory-cli.exe" packet --project . --question flow', "ok"),
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 3);
  assert.equal(analysis.command_categories.shell_search, 1);
  assert.equal(analysis.first_successful_packet_command.id, "cmd_3");
  assert.equal(analysis.first_successful_context_command.id, "cmd_1");
  assert.equal(analysis.packet_was_first_context_command, false);
});

test("recognizes quoted PowerShell variable CodeStory packet commands", () => {
  const command =
    "\"C:\\\\Program Files\\\\PowerShell\\\\pwsh.exe\" -Command '$cli = $env:CODESTORY_CLI\n& \"'$cli packet --project . --question '\"'Explain flow' --task-class 'architecture-explanation' --budget compact --format json\"";
  const events = [
    commandEvent("cmd_1", "item.started", command),
    commandEvent("cmd_1", "item.completed", command, "{\"packet_id\":\"ask-1\"}", 0),
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.first_successful_packet_command.id, "cmd_1");
  assert.equal(analysis.packet_was_first_context_command, true);
});

test("recognizes inline PowerShell env CodeStory packet commands", () => {
  const command = String.raw`"C:\Program Files\PowerShell\pwsh.exe" -Command '& $env:CODESTORY_CLI packet --project . --question 'Trace flow' --task-class 'route-tracing' --budget compact --format json"`;
  const events = [
    commandEvent("cmd_1", "item.started", command),
    commandEvent("cmd_1", "item.completed", command, "{\"packet_id\":\"ask-1\"}", 0),
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.first_successful_packet_command.id, "cmd_1");
  assert.equal(analysis.packet_was_first_context_command, true);
});

test("packet-first telemetry treats git and help probes before packet as context", () => {
  const gitFirst = analyzeTranscript([
    commandEvent("cmd_git", "item.completed", "git status --short", " M file"),
    commandEvent("cmd_packet", "item.completed", '& $env:CODESTORY_CLI packet --project . --question flow', "ok"),
  ]);
  assert.equal(gitFirst.first_successful_context_command.id, "cmd_git");
  assert.equal(gitFirst.packet_was_first_context_command, false);

  const helpFirst = analyzeTranscript([
    commandEvent("cmd_help", "item.completed", "codestory-cli packet --help", "Usage: codestory-cli packet"),
    commandEvent("cmd_packet", "item.completed", "codestory-cli packet --project . --question flow", "ok"),
  ]);
  assert.equal(helpFirst.first_successful_context_command.id, "cmd_help");
  assert.equal(helpFirst.first_successful_packet_command.id, "cmd_packet");
  assert.equal(helpFirst.packet_was_first_context_command, false);
});

test("harness packet prelude counts as the first context command", () => {
  const events = [
    {
      type: "harness.command.started",
      item: {
        id: "harness_codestory_packet",
        type: "command_execution",
        command: '"C:\\tools\\codestory-cli.exe" packet --project . --question flow --format json',
      },
    },
    {
      type: "harness.command.completed",
      item: {
        id: "harness_codestory_packet",
        type: "command_execution",
        command: '"C:\\tools\\codestory-cli.exe" packet --project . --question flow --format json',
        aggregated_output: '{"answer":{"citations":[{"file_path":"src/requests/sessions.py"}]}}',
        exit_code: 0,
      },
    },
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_count, 1);
  assert.equal(analysis.tool_categories.command_execution, 1);
  assert.equal(analysis.first_successful_packet_command.id, "harness_codestory_packet");
  assert.equal(analysis.packet_was_first_context_command, true);
});

test("typed harness packet preludes preserve semantics independent of executable spelling and prompt text", () => {
  const command =
    '/authenticated-inputs/candidate_cli.bin packet --project . --question "Explain the type-erased request flow" --format json';
  const harnessSemantics = {
    source: "codestory_packet_prelude_v1",
    category: "codestory_cli",
    operation: "packet",
  };
  const events = [
    {
      type: "harness.command.started",
      item: {
        id: "harness_codestory_packet",
        type: "command_execution",
        command,
        harness_semantics: harnessSemantics,
      },
    },
    {
      type: "harness.command.completed",
      item: {
        id: "harness_codestory_packet",
        type: "command_execution",
        command,
        harness_semantics: harnessSemantics,
        aggregated_output: '{"kind":"complete","schema_version":3}',
        exit_code: 0,
        status: "completed",
      },
    },
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.command_categories.direct_file_read ?? 0, 0);
  assert.equal(analysis.direct_source_reads_total, 0);
  assert.equal(analysis.first_successful_packet_command.id, "harness_codestory_packet");
  assert.equal(analysis.packet_was_first_context_command, true);

  const unmarked = analyzeTranscript([
    commandEvent("agent_command", "item.completed", command, "ok", 0),
  ]);
  assert.equal(unmarked.command_categories.codestory_cli ?? 0, 0);
  assert.equal(unmarked.first_successful_packet_command, null);
  assert.equal(unmarked.packet_was_first_context_command, false);
});

test("codestory cli resolver prefers explicit path, release binary, then fails closed", () => {
  const explicit = resolveCodeStoryCli({ codestoryCli: "C:/custom/codestory-cli.exe" }, () => {
    throw new Error("explicit path should not probe local candidates");
  });
  assert.equal(explicit, "C:/custom/codestory-cli.exe");

  const release = resolveCodeStoryCli({ codestoryCli: null }, (candidate) =>
    candidate.includes(`${path.sep}target${path.sep}release${path.sep}`),
  );
  assert.match(release, /target[\\/]release[\\/]codestory-cli(?:\.exe)?$/);

  assert.throws(
    () => resolveCodeStoryCli({ codestoryCli: null }, () => false),
    /Pass --codestory-cli, set CODESTORY_CLI, or build the release binary/,
  );
});

test("scores expected claims without requiring exact wording", () => {
  const events = [
    agentMessageEvent(
      "Runtime orchestration opens the workspace and store, chooses full or incremental indexing, and coordinates refresh phases.",
    ),
  ];

  const quality = scoreQuality(
    events,
    runtimeQualityTask("claim-fixture", {
      min_expected_file_recall: 0,
      min_expected_symbol_recall: 0,
      min_expected_claim_recall: 1,
      min_citation_coverage: 0,
      min_expected_anchor_recall: 0,
      max_forbidden_claims: 0,
    }),
  );

  assert.equal(quality.expected_claims.recall, 1);
});

test("positive claim scoring expands snake-case roles without crediting a partial flow", () => {
  const task = {
    id: "identifier-role-flow",
    task_class: "architecture_explanation",
    expected_files: [],
    expected_symbols: [],
    expected_claims: [
      "Parallel search uses the walker parallel builder to distribute file work.",
    ],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 0,
      min_expected_file_recall: 0,
      min_expected_symbol_recall: 0,
      min_expected_claim_recall: 1,
      min_citation_coverage: 0,
      max_forbidden_claims: 0,
    },
  };

  const complete = scoreQuality([
    agentMessageEvent(
      "`search_parallel` starts `walk_builder().build_parallel().run(...)`. Traversal supplies parallelism by feeding files to a search worker.",
    ),
  ], task);
  assert.equal(complete.expected_claims.found, 1);

  const partial = scoreQuality([
    agentMessageEvent("`search_parallel` starts a local worker."),
  ], task);
  assert.equal(partial.expected_claims.found, 0);
});

test("aggregate anchor recall uses fuzzy claim matching", () => {
  const events = [
    agentMessageEvent(
      "In crates/codestory-runtime/src/services.rs, IndexService::run_indexing_blocking opens the workspace and store, chooses full or incremental indexing, and coordinates refresh phases.",
    ),
  ];

  const quality = scoreQuality(
    events,
    runtimeQualityTask("aggregate-claim-fixture", {
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 1,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      min_expected_anchor_recall: 1,
      max_forbidden_claims: 0,
    }),
  );

  assert.equal(quality.expected_claims.recall, 1);
  assert.equal(quality.expected_anchors.recall, 1);
  assert.equal(quality.pass, true);
});

test("quality scoring treats class member separator variants as symbol matches", () => {
  const task = {
    id: "php-symbol-separator",
    task_class: "data_flow",
    expected_files: ["src/Logger.php"],
    expected_symbols: ["Logger::addRecord", "AbstractProcessingHandler::handle"],
    expected_claims: ["addRecord creates a LogRecord before passing it to handlers."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 1,
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 1,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      max_forbidden_claims: 0,
    },
  };
  const events = [
    agentMessageEvent(
      "`Logger.addRecord` in `src/Logger.php` creates a LogRecord before passing it to handlers. `AbstractProcessingHandler.handle` writes the processed record.",
    ),
  ];

  const quality = scoreQuality(events, task);

  assert.equal(quality.expected_symbols.recall, 1);
  assert.equal(quality.pass, true);
});

test("quality scoring never turns negative or gap sentences into positive symbol and claim credit", () => {
  const task = {
    id: "swift-polarity",
    task_class: "route_tracing",
    expected_files: [],
    expected_symbols: ["DataRequest.validate", "RequestTaskMap"],
    expected_claims: ["DataRequest.validate runs before the response is serialized."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 0,
      min_expected_file_recall: 0,
      min_expected_symbol_recall: 0,
      min_expected_claim_recall: 0,
      min_citation_coverage: 0,
      max_forbidden_claims: 0,
    },
  };

  const negative = scoreQuality([
    agentMessageEvent(
      "The packet does not establish `DataRequest.validate`. `RequestTaskMap` remains an evidence gap, so its role is unknown.",
    ),
  ], task);
  assert.equal(negative.expected_symbols.found, 0);
  assert.equal(negative.expected_claims.found, 0);

  const affirmative = scoreQuality([
    agentMessageEvent(
      "`DataRequest.validate` runs before the response is serialized. `RequestTaskMap` owns the concrete task mapping.",
    ),
  ], task);
  assert.equal(affirmative.expected_symbols.found, 2);
  assert.equal(affirmative.expected_claims.found, 1);

  const mixed = scoreQuality([
    agentMessageEvent(
      "The packet does not establish `DataRequest.validate`. `RequestTaskMap` owns the concrete task mapping.",
    ),
  ], task);
  assert.deepEqual(mixed.expected_symbols.found_anchors, ["RequestTaskMap"]);
  assert.equal(mixed.expected_claims.found, 0);
});

test("quality scoring keeps affirmative support units intact without combining unrelated bullets", () => {
  const compoundTask = {
    id: "compound-support-unit",
    task_class: "route_tracing",
    expected_files: [],
    expected_symbols: ["createApplication"],
    expected_claims: [
      "createApplication creates the callable app and mixes in the EventEmitter prototype.",
    ],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 0,
      min_expected_file_recall: 0,
      min_expected_symbol_recall: 0,
      min_expected_claim_recall: 0,
      min_citation_coverage: 0,
      max_forbidden_claims: 0,
    },
  };

  const oneUnit = scoreQuality([
    agentMessageEvent(
      "1. `createApplication` creates the callable app. It then mixes in the EventEmitter prototype.",
    ),
  ], compoundTask);
  assert.equal(oneUnit.expected_claims.found, 1);

  const separateUnits = scoreQuality([
    agentMessageEvent(
      "- `createApplication` creates the callable app.\n\n- The EventEmitter prototype is mixed into another object.",
    ),
  ], compoundTask);
  assert.equal(separateUnits.expected_claims.found, 0);

  const subjectTask = {
    ...compoundTask,
    id: "subject-bound-support-unit",
    expected_symbols: ["Engine.addRoute", "RouterGroup.Handle"],
    expected_claims: ["RouterGroup.Handle registers route handlers."],
  };
  const adversative = scoreQuality([
    agentMessageEvent(
      "`Engine.addRoute` is established, but `RouterGroup.Handle` is missing. The engine registers route handlers.",
    ),
  ], subjectTask);
  assert.deepEqual(adversative.expected_symbols.found_anchors, ["Engine.addRoute"]);
  assert.equal(adversative.expected_claims.found, 0);

  const lacksEvidence = scoreQuality([
    agentMessageEvent(
      "`RouterGroup.Handle` lacks evidence; `Engine.addRoute` registers route handlers.",
    ),
  ], subjectTask);
  assert.deepEqual(lacksEvidence.expected_symbols.found_anchors, ["Engine.addRoute"]);
  assert.equal(lacksEvidence.expected_claims.found, 0);

  const noEvidence = scoreQuality([
    agentMessageEvent(
      "No evidence establishes `RouterGroup.Handle`; `Engine.addRoute` registers route handlers.",
    ),
  ], subjectTask);
  assert.deepEqual(noEvidence.expected_symbols.found_anchors, ["Engine.addRoute"]);
  assert.equal(noEvidence.expected_claims.found, 0);

  const differentAffirmativeSubject = scoreQuality([
    agentMessageEvent(
      "`RouterGroup.Handle` reports registration errors; `Engine.addRoute` registers route handlers.",
    ),
  ], subjectTask);
  assert.equal(differentAffirmativeSubject.expected_claims.found, 0);

  const linkedWithinOneUnit = scoreQuality([
    agentMessageEvent(
      "`Session.send` is the dispatch point. Packet relations show it calls `Session.get_adapter`; the next cited line shows `adapter.send(request)`.",
    ),
  ], {
    ...compoundTask,
    id: "linked-one-unit",
    expected_symbols: ["Session.send", "Session.get_adapter", "HTTPAdapter.send"],
    expected_claims: ["Session.send chooses an adapter and calls the adapter send method."],
  });
  assert.equal(linkedWithinOneUnit.expected_claims.found, 1);

  const genericCapitalizedSubject = scoreQuality([
    agentMessageEvent(
      "`source/_vars.css` defines shared `:root` variables for `--animate-duration`, `--animate-delay`, and `--animate-repeat`.",
    ),
  ], {
    ...compoundTask,
    id: "generic-capitalized-subject",
    expected_symbols: [":root", "--animate-duration", "--animate-delay", "--animate-repeat"],
    expected_claims: [
      "Shared CSS custom properties define animation duration, delay, and repeat defaults.",
    ],
  });
  assert.equal(genericCapitalizedSubject.expected_claims.found, 1);

  const nestedListRelation = scoreQuality([
    agentMessageEvent(
      "- Named animation classes connect to same-named keyframes through `animation-name`:\n  - `@keyframes bounce`; `.bounce { animation-name: bounce; }`",
    ),
  ], {
    ...compoundTask,
    id: "nested-list-relation",
    expected_symbols: ["@keyframes bounce", ".bounce"],
    expected_claims: [
      "Named classes such as .bounce set animation-name to matching keyframes.",
    ],
  });
  assert.equal(nestedListRelation.expected_claims.found, 1);

  const unrelatedSibling = scoreQuality([
    agentMessageEvent(
      "- Named animation classes connect to same-named keyframes through `animation-name`:\n- `.bounce` is listed in an unrelated selector inventory.",
    ),
  ], {
    ...compoundTask,
    id: "unrelated-list-sibling",
    expected_symbols: ["@keyframes bounce", ".bounce"],
    expected_claims: [
      "Named classes such as .bounce set animation-name to matching keyframes.",
    ],
  });
  assert.equal(unrelatedSibling.expected_claims.found, 0);

  const gapSection = scoreQuality([
    agentMessageEvent(
      "The request reaches `Session.send`.\n\n## Material gaps\n`RouterGroup.Handle` registers route handlers.",
    ),
  ], subjectTask);
  assert.equal(gapSection.expected_symbols.found, 0);
  assert.equal(gapSection.expected_claims.found, 0);
});

test("quality scoring accepts bounded routing and check paraphrases for the same subject", () => {
  const task = {
    id: "routing-check-paraphrases",
    task_class: "route_tracing",
    expected_files: [],
    expected_symbols: ["processCommand"],
    expected_claims: ["processCommand performs command routing and execution checks."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 0,
      min_expected_file_recall: 0,
      min_expected_symbol_recall: 0,
      min_expected_claim_recall: 0,
      min_citation_coverage: 0,
      max_forbidden_claims: 0,
    },
  };

  for (const answer of [
    "`processCommand` determines a cluster target; an absent or non-local result rejects the command.",
    "`processCommand` selects a destination and rejects commands that fail its admission checks.",
  ]) {
    const quality = scoreQuality([agentMessageEvent(answer)], task);
    assert.equal(quality.expected_claims.found, 1, answer);
  }

  for (const answer of [
    "`processCommand` does not select a destination or reject invalid commands.",
    "`prepareCommand` selects a destination and rejects invalid commands; `processCommand` remains unknown.",
    "`processCommand` selects only metrics; it does not route or check commands.",
  ]) {
    const quality = scoreQuality([agentMessageEvent(answer)], task);
    assert.equal(quality.expected_claims.found, 0, answer);
  }
});

test("positive claim normalization is closed over qualified tokens and call relations", () => {
  const zeroThresholds = {
    min_expected_anchor_recall: 0,
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    max_forbidden_claims: 0,
  };
  const dartTask = {
    id: "dart-qualified-call-relations",
    task_class: "route_tracing",
    expected_files: [],
    expected_symbols: ["Client", "BaseClient", "BaseRequest.finalize", "IOClient.send"],
    expected_claims: [
      "Top-level package:http helpers delegate to a Client.",
      "BaseClient implements convenience methods in terms of send.",
      "BaseRequest.finalize prepares the request body for sending.",
      "IOClient.send is the dart:io transport implementation.",
    ],
    forbidden_claims: ["Top-level package:http helpers bypass Client."],
    quality_thresholds: zeroThresholds,
  };

  const dartQuality = scoreQuality([
    agentMessageEvent(
      "- The top-level API in `http.dart` exposes `get`, which calls `_withClient`; its callback invokes `client.get`.\n" +
      "- `BaseClient` convenience methods forward through `_sendUnstreamed`, which calls `send`.\n" +
      "- `BaseRequest.finalize` marks the request finalized, but the evidence does not establish body preparation.\n" +
      "- `IOClient.send` owns the I/O transport in `io_client`; it opens the platform request with `openUrl`.",
    ),
  ], dartTask);
  assert.deepEqual(dartQuality.expected_claims.found_anchors, [
    "Top-level package:http helpers delegate to a Client.",
    "BaseClient implements convenience methods in terms of send.",
    "IOClient.send is the dart:io transport implementation.",
  ]);
  assert.equal(dartQuality.forbidden_claims.found, 0);

  const relationTask = (relation) => ({
    id: `closed-call-relation-${relation}`,
    task_class: "route_tracing",
    expected_files: [],
    expected_symbols: ["Session.send", "Adapter.send"],
    expected_claims: [`Session.send ${relation} payload via Adapter.send.`],
    forbidden_claims: ["Session.send bypasses Adapter.send."],
    quality_thresholds: zeroThresholds,
  });
  for (const [expected, observed] of [
    ["delegates", "calls"],
    ["invokes", "forwards"],
    ["calls", "delegates"],
    ["forwards", "invokes"],
  ]) {
    const quality = scoreQuality([
      agentMessageEvent(`\`Session.send\` ${observed} bytes through \`Adapter.send\`.`),
    ], relationTask(expected));
    assert.equal(quality.expected_claims.found, 1, `${expected} should match ${observed}`);
    assert.equal(quality.forbidden_claims.found, 0);
  }

  const wrongSubject = scoreQuality([
    agentMessageEvent(
      "`BrowserClient.send` calls bytes through `Adapter.send`. `Session.send` is listed separately.",
    ),
  ], relationTask("delegates"));
  assert.equal(wrongSubject.expected_claims.found, 0);

  const negative = scoreQuality([
    agentMessageEvent("`Session.send` does not call bytes through `Adapter.send`."),
  ], relationTask("delegates"));
  assert.equal(negative.expected_claims.found, 0);

  const splitFlow = scoreQuality([
    agentMessageEvent(
      "- `Session.send` is present.\n- `Adapter.send` receives the payload.\n- `Router.forward` invokes telemetry.",
    ),
  ], relationTask("delegates"));
  assert.equal(splitFlow.expected_claims.found, 0);
});

test("quality scoring treats Ruby instance separator variants as symbol matches", () => {
  const task = {
    id: "ruby-symbol-separator",
    task_class: "route_tracing",
    expected_files: ["lib/jekyll/site.rb"],
    expected_symbols: ["Site#process", "Site#read", "Site#render", "Site#write"],
    expected_claims: ["Site.process runs reset, read, generate, render, cleanup, and write phases."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 1,
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 1,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      max_forbidden_claims: 0,
    },
  };
  const events = [
    agentMessageEvent(
      "`Site.process` in `lib/jekyll/site.rb` runs reset, read, generate, render, cleanup, and write phases. `Site.read`, `Site.render`, and `Site.write` are the lifecycle phase methods.",
    ),
  ];

  const quality = scoreQuality(events, task);

  assert.equal(quality.expected_symbols.recall, 1);
  assert.equal(quality.pass, true);
});

test("quality scoring treats Go receiver notation as a canonical member match", () => {
  const task = {
    id: "go-receiver-symbol",
    task_class: "route_tracing",
    expected_files: ["gin.go", "context.go"],
    expected_symbols: ["Engine.handleHTTPRequest", "Context.Next"],
    expected_claims: ["Engine dispatches a matched request through the context handler chain."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 1,
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 1,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      max_forbidden_claims: 0,
    },
  };
  const events = [
    agentMessageEvent(
      "`(*Engine).handleHTTPRequest` in `gin.go` dispatches a matched request through the context handler chain by calling `(*Context).Next` in `context.go`.",
    ),
  ];

  const quality = scoreQuality(events, task);

  assert.equal(quality.expected_symbols.recall, 1);
  assert.equal(quality.pass, true);
});

test("quality scoring treats namespace-qualified symbol tails as matches", () => {
  const task = {
    id: "ruby-namespace-symbol-tail",
    task_class: "route_tracing",
    expected_files: ["lib/jekyll/commands/build.rb", "lib/jekyll/site.rb"],
    expected_symbols: ["Jekyll::Commands::Build.process", "Jekyll::Site"],
    expected_claims: ["Build.process constructs or processes a Jekyll site."],
    forbidden_claims: [],
    quality_thresholds: {
      min_expected_anchor_recall: 1,
      min_expected_file_recall: 1,
      min_expected_symbol_recall: 1,
      min_expected_claim_recall: 1,
      min_citation_coverage: 1,
      max_forbidden_claims: 0,
    },
  };
  const events = [
    agentMessageEvent(
      "`Build.process` in `lib/jekyll/commands/build.rb` constructs or processes a Jekyll site, and `Site` in `lib/jekyll/site.rb` owns the lifecycle state.",
    ),
  ];

  const quality = scoreQuality(events, task);

  assert.equal(quality.expected_symbols.recall, 1);
  assert.equal(quality.pass, true);
});

test("quality scoring does not promote transcript-only expected anchors", () => {
  const task = runtimeQualityTask("runtime-flow", {
    min_expected_file_recall: 1,
    min_expected_symbol_recall: 1,
    min_expected_claim_recall: 1,
    min_citation_coverage: 1,
    min_expected_anchor_recall: 1,
    max_forbidden_claims: 0,
  });
  const events = [
    commandEvent(
      "cmd_1",
      "item.completed",
      "rg -n run_index crates/codestory-runtime/src/services.rs",
      `${RUNTIME_SERVICE_FILE}\n${RUN_INDEX_SYMBOL}`,
    ),
    agentMessageEvent(RUNTIME_REFRESH_CLAIM),
  ];

  const quality = scoreQuality(events, task);

  assert.equal(quality.pass, false);
  assert.equal(quality.observed_files.recall, 1);
  assert.equal(quality.observed_symbols.recall, 1);
  assert.equal(quality.expected_files.recall, 0);
  assert.equal(quality.expected_symbols.recall, 0);
});

test("packet composition separates citations, answer surfaces, and structured-only paths", () => {
  const composition = packetComposition(
    {
      answer: {
        summary: "The storage flow also mentions src/lib/data/storage/StorageAccessProxy.cpp.",
        sections: [
          {
            title: "Indexing",
            blocks: [
              {
                markdown: "Project::buildIndex creates indexing work.",
              },
            ],
          },
        ],
        citations: [
          {
            display_name: "Project::buildIndex",
            file_path: "src/lib/project/Project.cpp",
            line: 42,
          },
        ],
      },
      sufficiency: {
        avoid_opening: ["src/lib/data/storage/LegacyOnly.cpp because this is legacy prose"],
        avoid_opening_paths: ["src/lib/data/storage/PersistentStorage.cpp"],
        covered_claims: [
          {
            claim: "Hidden trace source mentions src/lib_cxx/project/SourceGroupCxxCdb.cpp.",
          },
        ],
      },
    },
    {
      expected_files: [
        "src/lib/project/Project.cpp",
        "src/lib/data/storage/PersistentStorage.cpp",
        "src/lib/data/storage/StorageAccessProxy.cpp",
        "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        "src/lib_java/data/indexer/IndexerJava.cpp",
      ],
      expected_verification_files: ["test/lib/project/ProjectTest.cpp"],
    },
  );

  assert.equal(composition.expected_file_count, 5);
  assert.equal(composition.expected_verification_file_count, 1);
  assert.equal(composition.cited_file_count, 1);
  assert.equal(composition.citation_backed_file_count, 2);
  assert.equal(composition.answer_surface_file_count, 3);
  assert.equal(composition.structured_file_count, 4);
  assert.equal(composition.citation_recall, 1 / 5);
  assert.equal(composition.citation_backed_recall, 2 / 5);
  assert.equal(composition.answer_surface_recall, 3 / 5);
  assert.equal(composition.structured_file_recall, 4 / 5);
  assert.ok(Math.abs(composition.composition_score - (1 + 0.9 + 0.25) / 5) < 1e-9);
  assert.deepEqual(
    composition.files.map((file) => [file.expected_file, file.packet_boundary]),
    [
      ["src/lib/project/Project.cpp", "cited_in_answer"],
      ["src/lib/data/storage/PersistentStorage.cpp", "listed_in_avoid_opening"],
      ["src/lib/data/storage/StorageAccessProxy.cpp", "mentioned_in_answer_text"],
      ["src/lib_cxx/project/SourceGroupCxxCdb.cpp", "present_only_in_structured_json"],
      ["src/lib_java/data/indexer/IndexerJava.cpp", "absent_from_packet"],
    ],
  );
  assert.deepEqual(
    composition.verification_files.map((file) => [file.expected_file, file.packet_boundary]),
    [["test/lib/project/ProjectTest.cpp", "absent_from_packet"]],
  );
  assert.equal(composition.verification_summary.structured_file_recall, 0);
});

test("packet prompt excerpt keeps answer support while dropping bulky packet fields", () => {
  const longText = `${"flow ".repeat(1400)}tail`;
  const promptPacket = packetForAgentPrompt({
    answer: {
      summary: "Requests flow",
      sections: [{ title: "Verbose", blocks: [{ markdown: longText }] }],
      citations: [
        {
          display_name: "Session.request",
          kind: "function",
          file_path:
            "C:/repo/target/agent-benchmark/repos/psf-requests/src/requests/sessions.py",
          line: 557,
          snippet: "large snippet should not be embedded",
        },
      ],
    },
    packet_id: "packet-requests",
    support: [
      {
        id: "support-1",
        kind: "source_range",
        summary: "Session.request prepares requests.",
        path: "src/requests/sessions.py",
        start_line: 557,
        end_line: 557,
        snippet: "def request(...)",
      },
    ],
    disposition: { kind: "supported", omission_receipts: [] },
  });

  assert.equal(promptPacket.answer.summary, "Requests flow");
  assert.match(promptPacket.answer.text, /\[truncated \d+ chars\]$/);
  assert.ok(promptPacket.answer.text.length < longText.length);
  assert.deepEqual(promptPacket.answer.citations, [
    {
      display_name: "Session.request",
      kind: "function",
      file_path: "src/requests/sessions.py",
      line: 557,
    },
  ]);
  assert.equal(promptPacket.packet_id, "packet-requests");
  assert.deepEqual(promptPacket.disposition, { kind: "supported", omission_receipts: [] });
  assert.equal(promptPacket.support.length, 1);
  assert.equal(promptPacket.support[0].summary, "Session.request prepares requests.");
  assert.equal(Object.hasOwn(promptPacket.answer, "sections"), false);
  assert.equal(Object.hasOwn(promptPacket, "sufficiency"), false);
});

test("packet manifest completion is gated by packet quality evidence", () => {
  const task = manifestFixture({
    expected_files: ["src/requests/sessions.py"],
    expected_symbols: ["Session.request"],
    expected_claims: ["Session.request prepares requests."],
  });
  const packet = {
    answer: {
      summary: "Session.request prepares requests in src/requests/sessions.py.",
      sections: [],
      citations: [
        {
          display_name: "Session.request",
          file_path: "src/requests/sessions.py",
          line: 557,
        },
      ],
    },
    support: [{ summary: "Session.request prepares requests." }],
    disposition: { kind: "supported" },
  };

  const quality = packetManifestQualitySummary(packet, task);
  assert.equal(quality.pass, true);
  assert.equal(
    packetPreludeManifestComplete({
      packet_manifest_quality: quality,
      packet_composition: packetComposition(packet, task),
      packet_disposition_kind: "supported",
      packet_support_count: 1,
    }),
    true,
  );
  assert.equal(
    packetPreludeManifestComplete({
      packet_manifest_quality: quality,
      packet_composition: packetComposition(packet, task),
      packet_disposition_kind: "supported",
      packet_support_count: 0,
    }),
    false,
  );
  assert.equal(
    packetPreludeManifestComplete({
      packet_manifest_quality: quality,
      packet_composition: packetComposition(packet, task),
      packet_disposition_kind: "drill_once",
      packet_support_count: 1,
    }),
    false,
  );

  const incompleteQuality = packetManifestQualitySummary(
    {
      answer: {
        summary: "Session.request is present in src/requests/sessions.py.",
        citations: [
          {
            display_name: "Session.request",
            file_path: "src/requests/sessions.py",
            line: 557,
          },
        ],
      },
      support: [],
    },
    task,
  );
  assert.equal(incompleteQuality.pass, false);
  assert.equal(
    packetPreludeManifestComplete({
      packet_manifest_quality: incompleteQuality,
      packet_composition: packetComposition(packet, task),
    }),
    false,
  );
});

test("packet manifest quality counts exact edge-derived server flow receipts", () => {
  const task = manifestFixture({
    id: "server-flow-receipts",
    task_class: "route_tracing",
    expected_files: ["lib/express.js", "lib/application.js", "lib/request.js", "lib/response.js"],
    expected_symbols: ["createApplication", "app.init", "app.handle", "app.use", "app.route", "res.send"],
    expected_claims: [
      "createApplication builds a callable app object and mixes in request and response prototypes.",
      "app.use registers middleware on the router.",
      "app.handle delegates request handling to the router.",
      "res.send prepares and sends the response body.",
    ],
    quality_thresholds: {
      min_expected_anchor_recall: 0.62,
      min_expected_file_recall: 0.6,
      min_expected_symbol_recall: 0.55,
      min_expected_claim_recall: 0.65,
      min_citation_coverage: 0.6,
      max_forbidden_claims: 0,
    },
  });
  const citations = [
    ["createApplication", "lib/express.js"],
    ["app.init", "lib/application.js"],
    ["app.handle", "lib/application.js"],
    ["app.use", "lib/application.js"],
    ["app.route", "lib/application.js"],
    ["res.send", "lib/response.js"],
    ["req.header", "lib/request.js"],
  ].map(([display_name, file_path], index) => ({ display_name, file_path, line: index + 1 }));
  const packet = {
    answer: { summary: "Server request flow", sections: [], citations },
    support: [
      {
        summary:
          "`app.use` registers middleware through the retained `app.router.use` call on the router.",
      },
      { summary: "`app.handle` delegates request handling through the retained `handle` call boundary." },
      {
        summary:
          "`res.send` sends output through the retained `end` call, completing the response body.",
      },
    ],
  };

  const quality = packetManifestQualitySummary(packet, task);
  assert.equal(quality.expected_claim_recall, 0.75);
  assert.equal(quality.expected_symbol_recall, 1);
  assert.equal(quality.pass, true);
});

test("baseline prelude tolerates benign ripgrep missing-path warnings when matches exist", () => {
  const status = baselineSearchPreludeStatus(
    {
      exitCode: 2,
      stderr:
        "rg: .\\test\\source\\symlink-test\\missing-target: The system cannot find the path specified. (os error 3)\n" +
        "rg: .\\test\\source\\_includes\\tmp: The system cannot find the file specified. (os error 2)\n",
    },
    [{ path: "src/site.rb", line: 1, column: 1, text: "build_site" }],
  );

  assert.equal(status.allowed, true);
  assert.equal(status.status, "pass_with_warnings");
  assert.equal(status.warning_lines.length, 2);
});

test("baseline prelude keeps non-benign ripgrep errors fail-closed", () => {
  const status = baselineSearchPreludeStatus(
    {
      exitCode: 2,
      stderr: "rg: ./secret: Permission denied (os error 13)\n",
    },
    [{ path: "src/site.rb", line: 1, column: 1, text: "build_site" }],
  );

  assert.equal(status.allowed, false);
  assert.equal(status.status, "fail");
});

const LOCAL_REAL_COMPACT_BUDGET_TASKS = [
  {
    repo: "vscode",
    task_id: "vscode-workbench-extension-host",
    expected_files: [
      "src/vs/workbench/browser/workbench.ts",
      "src/vs/workbench/services/extensions/browser/extensionService.ts",
      "src/vs/workbench/services/extensions/common/extensionHostManager.ts",
      "src/vs/workbench/api/common/extHostExtensionService.ts",
      "src/vs/workbench/api/common/extHostCommands.ts",
    ],
  },
  {
    repo: "codex",
    task_id: "codex-exec-json-flow",
    expected_files: [
      "codex-rs/cli/src/main.rs",
      "codex-rs/exec/src/lib.rs",
      "codex-rs/exec/src/event_processor.rs",
      "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
      "codex-rs/exec/src/exec_events.rs",
    ],
  },
  {
    repo: "sourcetrail",
    task_id: "sourcetrail-indexing-to-storage",
    expected_files: [
      "src/lib/project/Project.cpp",
      "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
      "src/lib_cxx/project/SourceGroupCxxCdb.h",
      "src/lib/data/storage/StorageAccess.h",
      "src/lib/data/storage/PersistentStorage.cpp",
    ],
  },
];

for (const task of LOCAL_REAL_COMPACT_BUDGET_TASKS) {
  test(`compact-budget packet composition rewards citation-backed recall for ${task.repo}/${task.task_id}`, () => {
    const citedPath = task.expected_files[0];
    const composition = packetComposition(
      {
        answer: {
          summary: `Cited ${citedPath} and mentioned another path only in prose.`,
          citations: [{ display_name: "Anchor", file_path: citedPath, line: 1 }],
        },
        sufficiency: { avoid_opening: [], covered_claims: [] },
      },
      { expected_files: task.expected_files },
    );

    assert.equal(composition.cited_file_count, 1);
    assert.equal(composition.citation_backed_file_count, 1);
    assert.equal(composition.answer_text_file_count, 0);
    assert.equal(composition.citation_backed_recall, composition.citation_recall);
    assert.ok(composition.composition_score >= composition.citation_recall);
  });
}

test("scores forbidden claims with the same fuzzy matcher as expected claims", () => {
  const task = runtimeQualityTask("forbidden-claim-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = ["remote service integration"];

  const quality = scoreQuality(
    [agentMessageEvent("This integration depends on a remote service.")],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 1);
  assert.equal(quality.pass, false);
});

test("forbidden claim scoring requires negative polarity terms", () => {
  const task = runtimeQualityTask("forbidden-negation-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = [
    "ThreadStartParams and TurnStartParams are only used by the interactive TUI, not by codex exec.",
  ];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "codex exec sends ThreadStartParams and TurnStartParams through thread/start and turn/start, while the TUI has a separate helper.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 0);
  assert.equal(quality.pass, true);
});

test("forbidden claim scoring does not flag contradicted positive claims", () => {
  const task = runtimeQualityTask("forbidden-positive-contradicted-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = ["StringUtils.isEmpty treats whitespace-only strings as empty."];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "StringUtils.isEmpty does not trim whitespace before deciding emptiness.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 0);
  assert.equal(quality.pass, true);
});

test("forbidden claim scoring does not combine unrelated storage sentences", () => {
  const task = runtimeQualityTask("forbidden-storage-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = ["StorageAccessProxy is the persistent SQLite storage implementation."];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "StorageAccessProxy forwards storage calls to the active storage subject. PersistentStorage is the concrete persistent implementation behind the storage access contract.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 0);
  assert.equal(quality.pass, true);
});

test("forbidden claim scoring keeps polarity inside one candidate sentence", () => {
  const task = runtimeQualityTask("forbidden-shell-polarity-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = [
    "nvm is a compiled binary and does not dispatch through shell functions.",
  ];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "`nvm` is the shell function dispatcher. `nvm_use_if_needed` switches versions only when the requested version is not already active.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 0);
  assert.equal(quality.pass, true);
});

test("forbidden claim scoring does not invert an explicit evidence-gap sentence", () => {
  const task = runtimeQualityTask("forbidden-shell-gap-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = [
    "nvm is a compiled binary and does not dispatch through shell functions.",
  ];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "Material gaps: the packet does not establish how downloaded nvm.sh is added to or sourced by a shell profile, the main nvm() command-dispatch cases for install, download, or use, the binary-install route, or any nvm use execution path.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 0);
  assert.equal(quality.pass, true);
});

test("forbidden claim scoring still catches a polarity-preserving paraphrase", () => {
  const task = runtimeQualityTask("forbidden-shell-paraphrase-fixture", {
    min_expected_file_recall: 0,
    min_expected_symbol_recall: 0,
    min_expected_claim_recall: 0,
    min_citation_coverage: 0,
    min_expected_anchor_recall: 0,
    max_forbidden_claims: 0,
  });
  task.forbidden_claims = [
    "nvm is a compiled binary and does not dispatch through shell functions.",
  ];

  const quality = scoreQuality(
    [
      agentMessageEvent(
        "nvm is a compiled binary; it does not dispatch through shell functions.",
      ),
    ],
    task,
  );

  assert.equal(quality.forbidden_claims.found, 1);
  assert.equal(quality.pass, false);
});

function pinnedRepoProvenance() {
  return {
    manifest_overridden_by_builtin: false,
    configured: {
      url: "https://github.com/example/fixture.git",
      ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
    },
    manifest: {
      url: "https://github.com/example/fixture.git",
      ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
    },
    git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
    git_origin: "https://github.com/example/fixture.git",
    git_dirty: false,
  };
}

function localCacheProvenance(overrides = {}) {
  return {
    doctor_status: "pass",
    storage_path: "C:/Users/alber/AppData/Local/codestory/cache/codestory.db",
    cache_policy: "prepared-retrieval-cache-read-only",
    retrieval_mode: "full",
    semantic_generation: "proj-current",
    manifest_embedding_backend: "per-user-server:coderank-embed:q8_0:sha256-deadbeef",
    semantic_backend: "per_user_server",
    embedding_engine_instance_id: "engine-1",
    embedding_policy: "accelerated",
    local_only: true,
    locality_kind: "same_user_local_ipc",
    indexed: true,
    freshness_status: "fresh",
    semantic_ready: true,
    indexing_in_timed_run: false,
    ...overrides,
  };
}

function localColdPacketCacheProvenance(overrides = {}) {
  return localCacheProvenance({
    embedding_engine_instance_id: null,
    embedding_policy: "accelerated",
    semantic_ready: false,
    packet_embedding_execution: {
      source: "packet.answer.retrieval_trace",
      transport_mode: "cold_cli_packet",
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      embedding_policy: "accelerated",
      retrieval_mode: "full",
      diagnostic_count: 2,
      full_diagnostic_count: 2,
      semantic_stage_count: 1,
      completed_semantic_stage_count: 1,
      invalid_semantic_stage_count: 0,
      shadow_degraded_reason: null,
      shadow_error: null,
      shadow_cancel_reason: null,
      semantic_fallback_count: 0,
      semantic_generation: "proj-current",
      prepared_semantic_generation: "proj-current",
    },
    ...overrides,
  });
}

test("cold packet execution proof replaces only unavailable process-local identity", () => {
  assert.deepEqual(
    cacheProvenanceBlockers({
      codestory_cache_provenance: localColdPacketCacheProvenance(),
    }),
    [],
  );

  for (const [field, value, message] of [
    ["source", "status", /source=status/],
    ["transport_mode", "warm_stdio_packet", /transport=warm_stdio_packet/],
    ["embedding_engine", "other", /embedding engine=other/],
    ["embedding_policy", "cpu_explicit", /expected accelerated/],
    ["retrieval_mode", null, /retrieval mode=unknown/],
    ["diagnostic_count", 0, /no sidecar diagnostics/],
    ["full_diagnostic_count", 1, /non-full sidecar diagnostic/],
    ["semantic_stage_count", 0, /no semantic stage/],
    ["completed_semantic_stage_count", 0, /incomplete semantic stage/],
    ["invalid_semantic_stage_count", 1, /degraded, stubbed, or cancelled semantic stage/],
    ["shadow_degraded_reason", "degraded", /retrieval shadow is degraded/],
    ["shadow_error", "failed", /retrieval shadow contains an error/],
    ["shadow_cancel_reason", "deadline", /retrieval shadow was cancelled/],
    ["semantic_fallback_count", 1, /semantic fallback count=1/],
    ["semantic_generation", "semantic-2", /does not match the prepared generation/],
  ]) {
    const provenance = localColdPacketCacheProvenance();
    provenance.packet_embedding_execution[field] = value;
    const blockers = cacheProvenanceBlockers({ codestory_cache_provenance: provenance });
    assert.match(blockers.join("\n"), message);
  }
});

test("skipped or degraded semantic stages cannot replace live engine identity", () => {
  const preparation = {
    retrieval_contract: {
      retrieval_contract: "in_process_v1",
      embedding_engine: "process_shared",
      execution_policy: "accelerated",
    },
    retrieval_status: { semantic_generation: "proj-current" },
  };
  const packet = {
    answer: {
      retrieval_trace: {
        retrieval_publication: { semantic_generation: "proj-current" },
        semantic_fallback_count: 0,
        packet_sidecar_diagnostics: [{ retrieval_mode: "full" }],
        retrieval_shadow: {
          degraded_reason: "semantic_unavailable",
          error: "semantic failed",
          cancel_reason: "deadline",
          stage_timings: [{
            stage: "stage1b_semantic",
            completion_status: "skipped",
            degraded: true,
            stub_reason: "stubbed",
            cancel_reason: "deadline",
          }],
        },
      },
    },
  };
  const proof = packetEmbeddingExecutionProof(packet, preparation, "cold_cli_packet");
  const provenance = localColdPacketCacheProvenance({ packet_embedding_execution: proof });
  const blockers = cacheProvenanceBlockers({ codestory_cache_provenance: provenance });

  assert.equal(proof.completed_semantic_stage_count, 0);
  assert.equal(proof.invalid_semantic_stage_count, 1);
  assert.match(blockers.join("\n"), /incomplete semantic stage/);
  assert.match(blockers.join("\n"), /retrieval shadow is degraded/);
  assert.match(blockers.join("\n"), /retrieval shadow contains an error/);
  assert.match(blockers.join("\n"), /retrieval shadow was cancelled/);
});

test("warm process provenance still requires live engine identity and semantic readiness", () => {
  const blockers = cacheProvenanceBlockers({
    codestory_cache_provenance: localCacheProvenance({
      embedding_engine_instance_id: null,
      semantic_ready: false,
    }),
  });
  assert.match(blockers.join("\n"), /missing CodeStory embedding engine identity/);
  assert.match(blockers.join("\n"), /CodeStory semantic docs are not ready/);
});

function emptyPacketObligationAccounting() {
  return {
    total: 0,
    material: 0,
    nonmaterial: 0,
    material_status_buckets: {},
  };
}

function publishableWithCodeStoryResult(overrides = {}) {
  const transcriptAnalysis = {
    command_count: 1,
    ordinary_source_reads_after_first_packet: 0,
    codestory_mcp_tool_calls_observed: 1,
    codestory_mcp_completed_calls_observed: 1,
    codestory_mcp_runtime_identities: [
      {
        plugin_version: "0.17.0",
        plugin_cli_version: "0.17.0",
        cli_version: "0.17.0",
        cli_source: "managed",
        pinned_pair_matches: true,
        known_override_skew_channel: false,
      },
    ],
    ...(overrides.transcript_analysis ?? {}),
  };
  const defaultPrelude = {
    status: "pass",
    packet_sufficiency_status: "sufficient",
    packet_sufficiency: {
      status: "sufficient",
      obligation_accounting: emptyPacketObligationAccounting(),
    },
  };
  const preludeOverride = overrides.codestory_harness_prelude;
  const codestoryPrelude = {
    ...defaultPrelude,
    ...(preludeOverride ?? {}),
    packet_sufficiency: {
      ...defaultPrelude.packet_sufficiency,
      ...(preludeOverride?.packet_sufficiency ?? {}),
    },
  };
  return {
    repo: "codestory",
    task_id: "codestory-indexing-flow",
    arm: "with_codestory",
    repeat: 1,
    status: "pass",
    wall_ms: 10,
    usage: { total_tokens: 100 },
    tool_calls_observed: 1,
    packet_first_required: true,
    packet_first_pass: true,
    quality: { pass: true },
    repo_provenance: pinnedRepoProvenance(),
    codestory_cache_provenance: localCacheProvenance(),
    ...overrides,
    transcript_analysis: transcriptAnalysis,
    codestory_harness_prelude: codestoryPrelude,
  };
}

test("publishable shard aggregation rejects incomplete, failed, and low-quality shards", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-publishable-shard-"));
  try {
    const task = {
      id: "codestory-indexing-flow",
      name: "CodeStory indexing flow",
      repo: "codestory",
      task_class: "route_tracing",
      prompt: "Trace indexing",
      quality_thresholds: {},
    };
    const directory = path.join(root, "shard-0");
    await mkdir(directory);
    const opts = {
      aggregateShards: [directory],
      outDir: path.join(root, "aggregate"),
      arms: ["with_codestory"],
      repeats: 3,
      jobs: 4,
      timeoutMs: 600_000,
      prepareCodestoryCache: true,
      prepareCodestoryJobs: 2,
      prepareCodestoryTimeoutMs: 1_800_000,
      packetRuntime: false,
      packetRuntimeMode: "both",
      materializeRepos: true,
      collectAllFailures: false,
      shardCount: 1,
      shardIndex: 0,
      runner: "codex",
      model: "gpt-5.6-sol",
      sandbox: "read-only",
      taskSuite: null,
      maxSourceReadsAfterPacket: 0,
      diagnosticExtraProbesFromManifest: false,
      packetGateImprovedFrom: null,
      codestoryCli: process.execPath,
      candidatePackageSha256: "package",
      canaryTaskId: task.id,
      manifestCanaryTaskId: task.id,
      publishable: true,
    };
    const planned = planAgentRuns(opts, [task]);
    const validRows = planned.map((run) => publishableWithCodeStoryResult({
      repo: task.repo,
      task_id: task.id,
      repeat: run.repeat,
      canary: run.repeat === 1,
      benchmark_contract: benchmarkContractForRun(opts, run),
    }));
    const preparation = [pipelinePreparation(task.repo)];
    const writeFixture = async (rows, summaryOverrides = {}) => {
      const attestation = await benchmarkShardAttestation(
        opts,
        [task],
        preparation,
        rows,
        CLEAN_SHARD_ATTESTATION,
      );
      await writeFile(
        path.join(directory, "runs.jsonl"),
        `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`,
      );
      await writeFile(
        path.join(directory, "summary.json"),
        `${JSON.stringify({
          publishable: true,
          comparative_publishable: true,
          comparative_failure: null,
          expected_rows: 3,
          completed_rows: rows.length,
          first_failure: null,
          canary_task_id: task.id,
          effective_canary_task_id: task.id,
          packet_obligation_accounting: emptyPacketObligationAccounting(),
          shard: { count: 1, index: 0, attestation },
          ...summaryOverrides,
        })}\n`,
      );
    };

    await writeFixture(validRows);
    await aggregateShardRuns(opts, [task]);

    await writeFixture(validRows, { publishable: false });
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, outDir: path.join(root, "not-publishable") }, [task]),
      /summary is not publishable and complete/,
    );

    await writeFixture(validRows.map((row, index) => index === 1
      ? { ...row, status: "fail" }
      : row));
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, outDir: path.join(root, "failed") }, [task]),
      /Publishable shard rows failed/,
    );

    await writeFixture(validRows.map((row, index) => index === 1
      ? { ...row, quality: { pass: false } }
      : row));
    await assert.rejects(
      () => aggregateShardRuns({ ...opts, outDir: path.join(root, "low-quality") }, [task]),
      /Publishable shard rows failed/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

function publishablePacketRuntimeResult(overrides = {}) {
  const defaultSufficiency = {
    status: "sufficient",
    sufficient_quality_mismatch: false,
    obligation_accounting: emptyPacketObligationAccounting(),
  };
  return {
    repo: "codestory",
    task_id: "codestory-indexing-flow",
    mode: "cold",
    repeat: 1,
    status: "pass",
    quality: { pass: true },
    sufficiency: defaultSufficiency,
    packet_latency: {
      sla_missed: false,
      retrieval_shadow: {
        retrieval_mode: "full",
      },
    },
    repo_provenance: pinnedRepoProvenance(),
    codestory_cache_provenance: localCacheProvenance(),
    ...overrides,
    sufficiency: overrides.sufficiency === null
      ? null
      : { ...defaultSufficiency, ...(overrides.sufficiency ?? {}) },
  };
}

test("publishable packet obligation accounting rejects mismatched rows", () => {
  assert.deepEqual(
    agentPublishableBlockers(
      [publishableWithCodeStoryResult()],
      { publishable: true, maxSourceReadsAfterPacket: 0 },
    ),
    [],
  );
  assert.deepEqual(
    packetRuntimePublishableBlockers(
      [publishablePacketRuntimeResult()],
      { publishable: true },
    ),
    [],
  );
  const mismatched = {
    total: 3,
    material: 2,
    nonmaterial: 1,
    material_status_buckets: { proven: 1 },
  };
  const agentBlockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        codestory_harness_prelude: {
          packet_sufficiency: { obligation_accounting: mismatched },
        },
      }),
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );
  assert.match(
    agentBlockers.flatMap((blocker) => blocker.reasons).join("\n"),
    /material=2 does not reconcile with material status buckets=1/,
  );

  const runtimeBlockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        sufficiency: { obligation_accounting: mismatched },
      }),
    ],
    { publishable: true },
  );
  assert.match(
    runtimeBlockers.flatMap((blocker) => blocker.reasons).join("\n"),
    /material=2 does not reconcile with material status buckets=1/,
  );

  const failedWithoutPacket = publishableWithCodeStoryResult({
    status: "cancelled",
    codestory_harness_prelude: null,
    transcript_analysis: {
      command_count: 0,
      codestory_mcp_tool_calls_observed: 0,
      codestory_mcp_completed_calls_observed: 0,
      codestory_mcp_runtime_identities: [],
    },
  });
  const failedReasons = agentPublishableBlockers(
    [failedWithoutPacket],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  ).flatMap((blocker) => blocker.reasons);
  assert.equal(
    failedReasons.includes("codestory prelude packet obligation accounting is missing"),
    false,
    "a cancelled row with no packet must not claim that packet accounting was omitted",
  );
});

test("publishable gate blocks avoidable source reads after packet", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        transcript_analysis: {
          command_count: 1,
          ordinary_source_reads_after_first_packet: 1,
        },
      }),
    ],
    { maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /ordinary source reads after packet=1 > 0/);
});

test("publishable gate records but does not block post-packet reads by default", () => {
  const blockers = agentPublishableBlockers([
    publishableWithCodeStoryResult({
      transcript_analysis: {
        command_count: 3,
        ordinary_source_reads_after_first_packet: 2,
      },
    }),
  ]);

  assert.deepEqual(blockers, []);
});

test("publishable gate requires explicit post-packet source-read budget", () => {
  const blockers = agentPublishableBlockers(
    [publishableWithCodeStoryResult()],
    { publishable: true },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /missing explicit post-packet source-read budget/);
});

test("publishable gate treats baseline prelude warnings as harness-contract blockers", () => {
  const blockers = agentPublishableBlockers(
    [
      {
        repo: "codestory",
        task_id: "codestory-indexing-flow",
        arm: "without_codestory",
        repeat: 1,
        status: "pass",
        wall_ms: 10,
        usage: { total_tokens: 100 },
        tool_calls_observed: 1,
        packet_first_required: false,
        packet_first_pass: true,
        quality: { pass: true },
        transcript_analysis: {
          command_count: 1,
          external_context_tool_calls: 0,
        },
        baseline_harness_prelude: {
          status: "pass_with_warnings",
        },
        repo_provenance: pinnedRepoProvenance(),
      },
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.equal(blockers[0].category, "harness-contract");
  assert.match(blockers[0].reasons.join("\n"), /baseline prelude status=pass_with_warnings; expected pass/);
});

test("publishable gate rejects diagnostic packet probes", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        codestory_harness_prelude: {
          packet_extra_probe_count: 2,
          packet_extra_probe_strategy: "diagnostic_manifest_expected_anchors",
        },
      }),
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /diagnostic packet extra probes used/);
});

test("publishable gate requires packet before ordinary context exploration", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        repo: "vite",
        task_id: "vite-dev-server-architecture",
        packet_first_pass: false,
        transcript_analysis: {
          command_count: 1,
          ordinary_source_reads_after_first_packet: 0,
        },
      }),
    ],
    { maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /missing answer packet as first successful context command/);
});

test("publishable gate rejects CodeStory use in the without arm", () => {
  const blockers = agentPublishableBlockers([
    {
      repo: "codestory",
      task_id: "codestory-indexing-flow",
      arm: "without_codestory",
      repeat: 1,
      status: "pass",
      wall_ms: 10,
      usage: { total_tokens: 100 },
      tool_calls_observed: 1,
      packet_first_required: false,
      packet_first_pass: true,
      quality: { pass: true },
      transcript_analysis: {
        command_count: 1,
        command_categories: {
          codestory_cli: 1,
        },
        external_context_tool_calls: 0,
      },
    },
  ]);

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /without_codestory arm used CodeStory/);
});

test("publishable gate rejects CodeStory MCP use in the without arm", () => {
  const blockers = agentPublishableBlockers([
    {
      repo: "codestory",
      task_id: "codestory-indexing-flow",
      arm: "without_codestory",
      repeat: 1,
      status: "pass",
      wall_ms: 10,
      usage: { total_tokens: 100 },
      tool_calls_observed: 2,
      packet_first_required: false,
      packet_first_pass: true,
      quality: { pass: true },
      transcript_analysis: {
        command_count: 1,
        command_categories: { shell_search: 1 },
        codestory_mcp_tool_calls_observed: 1,
        external_context_tool_calls: 0,
      },
    },
  ]);

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /without_codestory arm used CodeStory/);
});

test("publishable gate requires local repo inspection in the without arm", () => {
  const blockers = agentPublishableBlockers([
    {
      repo: "codestory",
      task_id: "codestory-indexing-flow",
      arm: "without_codestory",
      repeat: 1,
      status: "pass",
      wall_ms: 10,
      usage: { total_tokens: 100 },
      tool_calls_observed: 1,
      packet_first_required: false,
      packet_first_pass: true,
      quality: { pass: true },
      transcript_analysis: {
        command_count: 0,
        command_categories: {},
        external_context_tool_calls: 0,
      },
    },
  ]);

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /without_codestory arm did not inspect local repository/);
});

test("publishable gate accepts ordinary local inspection in the without arm", () => {
  const blockers = agentPublishableBlockers([
    {
      repo: "codestory",
      task_id: "codestory-indexing-flow",
      arm: "without_codestory",
      repeat: 1,
      status: "pass",
      wall_ms: 10,
      usage: { total_tokens: 100 },
      tool_calls_observed: 1,
      packet_first_required: false,
      packet_first_pass: true,
      quality: { pass: true },
      transcript_analysis: {
        command_count: 2,
        command_categories: {
          shell_search: 1,
          direct_file_read: 1,
        },
        external_context_tool_calls: 0,
      },
    },
  ]);

  assert.deepEqual(blockers, []);
});

test("publishable provenance requires full-SHA clean manifest checkout", () => {
  const clean = {
    repo_provenance: {
      manifest_overridden_by_builtin: false,
      configured: {
        url: "https://github.com/example/fixture.git",
        ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
      },
      manifest: {
        url: "https://github.com/example/fixture.git",
        ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
      },
      git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
      git_origin: "https://github.com/example/fixture.git",
      git_dirty: false,
    },
  };
  assert.deepEqual(repoProvenanceBlockers(clean), []);
  const projectBound = structuredClone(clean);
  projectBound.repo_provenance.manifest.codestory_project_manifest = {
    path: "benchmarks/tasks/holdout-retrieval/ripgrep-rust-codestory-project.json",
    sha256: "85b8ade56e2907ba78366a231cb11970f2b18830725771d9f435d3109bb1972a",
  };
  projectBound.repo_provenance.installed_codestory_project_manifest = {
    source_path: "benchmarks/tasks/holdout-retrieval/ripgrep-rust-codestory-project.json",
    declared_sha256: "85b8ade56e2907ba78366a231cb11970f2b18830725771d9f435d3109bb1972a",
    installed_path: "codestory_project.json",
    installed_sha256: "85b8ade56e2907ba78366a231cb11970f2b18830725771d9f435d3109bb1972a",
    ignored: true,
  };
  assert.deepEqual(repoProvenanceBlockers(projectBound), []);
  projectBound.repo_provenance.installed_codestory_project_manifest.installed_sha256 = "0".repeat(64);
  assert.match(
    repoProvenanceBlockers(projectBound).join("\n"),
    /installed CodeStory project manifest bytes do not match declared hash/,
  );
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: {
          url: "https://github.com/example/fixture.git",
          ref: "main",
        },
        manifest: {
          url: "https://github.com/example/fixture.git",
          ref: "main",
        },
        git_head: "abc123",
        git_origin: "https://github.com/example/fixture.git",
        git_dirty: false,
      },
    }).join("\n"),
    /not pinned to a full immutable commit SHA/,
  );
  for (const ref of ["abcdef0", "v1.2.3", "refs/tags/v1.2.3"]) {
    assert.match(
      repoProvenanceBlockers({
        repo_provenance: {
          manifest_overridden_by_builtin: false,
          configured: {
            url: "https://github.com/example/fixture.git",
            ref,
          },
          manifest: {
            url: "https://github.com/example/fixture.git",
            ref,
          },
          git_head: "abc123",
          git_origin: "https://github.com/example/fixture.git",
          git_dirty: false,
        },
      }).join("\n"),
      /not pinned to a full immutable commit SHA/,
      `publishable provenance should reject ${ref}`,
    );
  }
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: {
          url: "https://github.com/example/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        manifest: {
          url: "https://github.com/example/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        git_head: "1234567890abcdef1234567890abcdef12345678",
        git_origin: "https://github.com/example/fixture.git",
        git_dirty: false,
      },
    }).join("\n"),
    /does not match configured ref/,
  );
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: {
          url: "file:///tmp/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        manifest: {
          url: "file:///tmp/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        git_origin: "file:///tmp/fixture.git",
        git_dirty: false,
      },
    }).join("\n"),
    /configured repo URL is not a trusted GitHub HTTPS repo URL/,
  );
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: {
          url: "https://github.com/example/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        manifest: {
          url: "https://github.com/other/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        git_origin: "https://github.com/example/fixture.git",
        git_dirty: false,
      },
    }).join("\n"),
    /manifest repo URL .* does not match configured URL/,
  );
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: {
          url: "https://github.com/example/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        manifest: {
          url: "https://github.com/example/fixture.git",
          ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        },
        git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
        git_origin: "https://github.com/other/fixture.git",
        git_dirty: false,
      },
    }).join("\n"),
    /git origin .* does not match configured URL/,
  );

  const blockers = agentPublishableBlockers(
    [
      {
        repo: "codestory",
        task_id: "codestory-indexing-flow",
        arm: "with_codestory",
        repeat: 1,
        status: "pass",
        wall_ms: 10,
        usage: { total_tokens: 100 },
        tool_calls_observed: 1,
        packet_first_required: true,
        packet_first_pass: true,
        quality: { pass: true },
        transcript_analysis: {
          command_count: 1,
          ordinary_source_reads_after_first_packet: 0,
        },
        repo_provenance: {
          manifest_overridden_by_builtin: true,
          configured: { url: "local", ref: "local" },
          manifest: { url: "https://github.com/example/fixture.git", ref: "main" },
          git_head: "abc123",
          git_origin: "local",
          git_dirty: true,
        },
      },
    ],
    { maxSourceReadsAfterPacket: 0, enforceRepoProvenance: true },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /overridden by a built-in checkout/);
  assert.match(blockers[0].reasons.join("\n"), /repo ref is not pinned/);
  assert.match(blockers[0].reasons.join("\n"), /repo checkout is dirty/);
});

test("publishable gate requires CodeStory cache provenance for CodeStory arm", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        codestory_cache_provenance: null,
      }),
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /missing CodeStory cache provenance/);
});

test("publishable gate accepts local-only CodeStory cache provenance", () => {
  const blockers = agentPublishableBlockers(
    [publishableWithCodeStoryResult()],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.deepEqual(blockers, []);
});

test("publishable gate requires resource accounting fields", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        wall_ms: null,
        usage: { total_tokens: null },
        tool_calls_observed: null,
        transcript_analysis: {
          command_count: null,
          ordinary_source_reads_after_first_packet: 0,
        },
      }),
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  const reasons = blockers[0].reasons.join("\n");
  assert.match(reasons, /missing wall time/);
  assert.match(reasons, /missing total token usage/);
  assert.match(reasons, /missing tool call count/);
  assert.match(reasons, /missing command count/);
});

test("publishable gate requires CodeStory local-only provenance", () => {
  const blockers = agentPublishableBlockers(
    [
      publishableWithCodeStoryResult({
        codestory_cache_provenance: localCacheProvenance({
          local_only: false,
          locality_kind: "remote_endpoint",
        }),
      }),
    ],
    { publishable: true, maxSourceReadsAfterPacket: 0 },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /local-only guarantee is not proven/);
});

test("packet runtime publishable gate requires sufficient packets and telemetry", () => {
  assert.deepEqual(
    packetRuntimePublishableBlockers([publishablePacketRuntimeResult()], { publishable: true }),
    [],
  );

  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({ sufficiency: null }),
      publishablePacketRuntimeResult({
        sufficiency: { status: "partial", sufficient_quality_mismatch: false },
      }),
      publishablePacketRuntimeResult({ packet_latency: null }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 3);
  assert.match(blockers[0].reasons.join("\n"), /missing packet sufficiency telemetry/);
  assert.match(blockers[1].reasons.join("\n"), /packet sufficiency status=partial; expected sufficient/);
  assert.match(blockers[2].reasons.join("\n"), /missing packet latency telemetry/);
});

test("packet runtime publishable gate requires SLA pass and full retrieval shadow", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        packet_latency: {
          sla_missed: true,
          retrieval_shadow: { retrieval_mode: "full" },
        },
      }),
      publishablePacketRuntimeResult({
        packet_latency: {
          sla_missed: false,
          retrieval_shadow: null,
        },
      }),
      publishablePacketRuntimeResult({
        packet_latency: {
          sla_missed: false,
          retrieval_shadow: { retrieval_mode: "degraded" },
        },
      }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 3);
  assert.match(blockers[0].reasons.join("\n"), /packet retrieval SLA missed=true; expected false/);
  assert.match(blockers[1].reasons.join("\n"), /missing retrieval shadow telemetry/);
  assert.match(blockers[2].reasons.join("\n"), /packet retrieval shadow mode=degraded; expected full/);
});

test("packet runtime publishable gate rejects diagnostic packet probes", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        packet_extra_probe_count: 1,
        packet_extra_probe_strategy: "diagnostic_manifest_expected_anchors",
      }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /diagnostic packet extra probes used/);
});

test("packet coverage unresolved accounting follows material query completion", () => {
  const packet = {
    plan: {
      obligations: {
        query_obligations: [
          {
            query: "public facade",
            material: true,
            completion: { status: "completed" },
          },
          {
            query: "supplemental wording",
            material: false,
            completion: { status: "cancelled", reason: "not_dispatched" },
          },
        ],
      },
    },
    sufficiency: {
      status: "sufficient",
      coverage_report: {
        unresolved: ["public facade", "supplemental wording"],
      },
    },
  };

  let telemetry = packetSufficiencyTelemetry(packet, { pass: true });
  assert.equal(telemetry.coverage_unresolved_count, 2);
  assert.equal(telemetry.coverage_unresolved_blocking_count, 0);

  packet.plan.obligations.query_obligations[0].completion = {
    status: "cancelled",
    reason: "deadline",
  };
  telemetry = packetSufficiencyTelemetry(packet, { pass: true });
  assert.equal(telemetry.coverage_unresolved_blocking_count, 1);

  packet.sufficiency.coverage_report.unresolved = ["unknown query"];
  telemetry = packetSufficiencyTelemetry(packet, { pass: true });
  assert.equal(telemetry.coverage_unresolved_blocking_count, 1);

  delete packet.plan.obligations;
  packet.sufficiency.coverage_report.unresolved = ["public facade"];
  telemetry = packetSufficiencyTelemetry(packet, { pass: true });
  assert.equal(telemetry.coverage_unresolved_blocking_count, 1);
});

test("packet runtime publishable gate blocks unresolved packet diagnostics as product blockers", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        sufficiency: {
          status: "sufficient",
          follow_up_commands_count: 0,
          unresolved_candidate_count: 2,
          coverage_unresolved_count: 1,
          coverage_unresolved_blocking_count: 1,
        },
      }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 1);
  assert.equal(blockers[0].category, "product");
  assert.match(blockers[0].reasons.join("\n"), /packet unresolved retrieval candidates=2; expected 0/);
  assert.match(blockers[0].reasons.join("\n"), /packet unresolved coverage diagnostics=1; expected 0/);
});

test("packet runtime publishable gate allows explicitly diagnostic-only unresolved diagnostics", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        sufficiency: {
          status: "sufficient",
          follow_up_commands_count: 0,
          unresolved_candidate_count: 2,
          unresolved_candidate_diagnostic_only: true,
          coverage_unresolved_count: 2,
          coverage_unresolved_blocking_count: 0,
        },
      }),
    ],
    { publishable: true },
  );

  assert.deepEqual(blockers, []);
});

test("packet runtime publishable gate separates product, harness, and environment blockers", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        quality: { pass: false },
        packet_extra_probe_strategy: "diagnostic_manifest_expected_anchors",
        packet_latency: {
          sla_missed: false,
          retrieval_shadow: { retrieval_mode: "degraded" },
        },
      }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 3);
  assert.deepEqual(blockers.map((blocker) => blocker.category), ["product", "harness-contract", "environment"]);
  assert.match(blockers[0].reasons.join("\n"), /manifest quality failed/);
  assert.match(blockers[1].reasons.join("\n"), /diagnostic packet extra probes used/);
  assert.match(blockers[2].reasons.join("\n"), /packet retrieval shadow mode=degraded; expected full/);
});

test("holdout packet runtime requires quality gate unless failures are allowed", () => {
  assert.equal(
    packetRuntimeQualityGateRequired({ taskSuite: "holdout-retrieval" }),
    true,
  );
  assert.equal(
    packetRuntimeQualityGateRequired({ taskSuite: "language-expansion-holdout" }),
    true,
  );
  assert.equal(
    packetRuntimeQualityGateRequired({
      taskSuite: "language-expansion-holdout",
      allowFailures: true,
    }),
    false,
  );
  assert.equal(packetRuntimeQualityGateRequired({ taskSuite: "local-real" }), false);
});

test("reanalysis uses the run-time task snapshot before current manifest contents", async () => {
  await withManifestFile(
    manifestFixture({
      expected_claims: ["The current manifest changed."],
    }),
    async (manifestPath) => {
      const snapshot = taskSnapshotForResult({
        ...runtimeQualityTask("snapshot-task", {
          min_expected_file_recall: 0,
          min_expected_symbol_recall: 0,
          min_expected_claim_recall: 1,
          min_citation_coverage: 0,
          min_expected_anchor_recall: 0,
          max_forbidden_claims: 0,
        }),
        name: "Snapshot task",
        suite: "fixture",
        repo: "fixture-repo",
        prompt: "Explain the old task.",
        expected_claims: ["The snapshot claim is immutable."],
        manifest_path: manifestPath,
      });

      const loaded = await loadTaskForResult(
        {
          task_manifest_path: manifestPath,
          task_manifest_snapshot: snapshot,
        },
        {},
        new Map(),
      );

      assert.deepEqual(loaded.expected_claims, ["The snapshot claim is immutable."]);
    },
  );
});

test("qualityFailureReasons lists recall misses", () => {
  const reasons = qualityFailureReasons({
    pass: false,
    thresholds: { expected_file_recall: 0.8 },
    expected_anchors: { recall: 1 },
    expected_files: { recall: 0.2 },
    expected_symbols: { recall: 1 },
    expected_claims: { recall: 1 },
    citation_coverage: { recall: 1 },
    forbidden_claims: { found: 0 },
  });
  assert.ok(reasons.includes("expected_file_recall_low"));
});

test("buildQualityDebugPayload aggregates failure counts", () => {
  const payload = buildQualityDebugPayload([
    {
      repo: "ripgrep",
      task_id: "ripgrep-search-pipeline",
      mode: "cold-cli",
      status: "pass",
      quality: {
        pass: false,
        thresholds: {},
        expected_anchors: { recall: 0.5 },
        expected_files: { recall: 0.5 },
        expected_symbols: { recall: 0.5 },
        expected_claims: { recall: 0.5 },
        citation_coverage: { recall: 0.5 },
        forbidden_claims: { found: 0 },
      },
    },
  ]);
  assert.equal(payload.summary.quality_fail_runs, 1);
  assert.ok(Object.keys(payload.summary.failure_reason_counts).length > 0);
});

test("buildQualityDebugPayload preserves packet sufficiency diagnostics", () => {
  const payload = buildQualityDebugPayload([
    {
      repo: "requests",
      task_id: "requests-session-flow",
      mode: "cold_cli_packet",
      status: "pass",
      quality: {
        pass: true,
        thresholds: {},
        expected_anchors: { recall: 1 },
        expected_files: { recall: 1 },
        expected_symbols: { recall: 1 },
        expected_claims: { recall: 1 },
        citation_coverage: { recall: 1 },
        forbidden_claims: { found: 0 },
      },
      sufficiency: {
        status: "partial",
        gaps_count: 2,
        gaps: [
          "Packet was truncated by Compact budget: citations, trail_edges.",
          "Packet omitted answer-critical evidence under Compact budget; use a deeper packet before treating this as complete.",
        ],
        open_next_count: 2,
        open_next: ["codestory-cli packet --budget standard", "codestory-cli search --why"],
        follow_up_commands_count: 2,
        follow_up_commands: [
          "codestory-cli packet --budget standard",
          "codestory-cli search --why",
        ],
        covered_claims_count: 8,
        avoid_opening_count: 4,
        sufficient_quality_mismatch: false,
      },
    },
  ]);

  assert.equal(payload.rows[0].sufficiency_status, "partial");
  assert.deepEqual(payload.rows[0].sufficiency.gaps, [
    "Packet was truncated by Compact budget: citations, trail_edges.",
    "Packet omitted answer-critical evidence under Compact budget; use a deeper packet before treating this as complete.",
  ]);
  assert.equal(payload.rows[0].sufficiency.follow_up_commands_count, 2);
  assert.equal(payload.summary.packet_partial_runs, 1);
  assert.equal(
    payload.summary.partial_gap_counts[
      "Packet was truncated by Compact budget: citations, trail_edges."
    ],
    1,
  );
});
