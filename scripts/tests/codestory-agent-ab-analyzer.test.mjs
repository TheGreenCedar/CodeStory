import test from "node:test";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync } from "node:fs";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink, truncate, writeFile } from "node:fs/promises";
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
  packetPreludeContractBlockers,
  packetPreludeManifestComplete,
  packetLatencyTelemetry,
  packetFirstCommandForPrompt,
  packetRuntimePublishableBlockers,
  packetRuntimeQualityGateRequired,
  publicCoreCorpusAudit,
  projectResourceUri,
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

test("keeps CLI overrides out of both isolated agent arms", () => {
  const opts = {
    runner: "codex",
    sandbox: "read-only",
    model: "gpt-5.6-sol",
  };
  const baseline = runnerCommand(opts, "/tmp/repo", "prompt", "without_codestory");
  const measured = runnerCommand(opts, "/tmp/repo", "prompt", "with_codestory");
  assert.ok(baseline.args.includes("--ignore-user-config"));
  assert.ok(!measured.args.includes("--ignore-user-config"));
  assert.deepEqual(
    baseline.args.filter((arg) => arg !== "--ignore-user-config"),
    measured.args,
  );
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
        { length: 21 },
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
        max_anchors: 13,
        max_files: 1,
        max_output_bytes: 98_304,
        max_snippets: 1,
        max_trail_edges: 20,
      },
      used: { anchors: 1, files: 1, output_bytes: 0, snippets: 1, trail_edges: 21 },
    },
  };
  const stdout = exactPacketStdout(packet);
  const blockers = packetPreludeContractBlockers(packet, stdout, {
    requireSupported: true,
    requireManagedRuntime: true,
  });
  assert.ok(blockers.some((blocker) => blocker.includes("trail_edges=21 exceeds 20")));
  assert.equal(blockers.some((blocker) => blocker.includes("stdout bytes")), false);

  packet.answer.graphs.pop();
  packet.budget.used.trail_edges = 20;
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
    ["max_anchors", 12, 13],
    ["max_trail_edges", 19, 20],
    ["max_output_bytes", 90_000, 98_304],
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
  packet.hostile_padding = "x".repeat(100_000);
  const raisedLimitStdout = exactPacketStdout(packet);
  assert.ok(packet.budget.used.output_bytes > 98_304);
  assert.match(
    packetPreludeContractBlockers(packet, raisedLimitStdout, {
      requireSupported: true,
      requireManagedRuntime: true,
    }).join("\n"),
    /max_output_bytes=200000 does not equal public cap=98304.*used\.output_bytes=.*exceeds public cap=98304/s,
  );
  delete packet.hostile_padding;
  packet.budget.limits.max_output_bytes = 98_304;

  for (const identity of [
    managedRuntimeIdentity({ plugin_version: "0.16.3" }),
    managedRuntimeIdentity({ cli_source: "override", known_override_skew_channel: true }),
  ]) {
    packet._meta.codestory_publication.contract_runtime = identity;
    const staleStdout = exactPacketStdout(packet);
    assert.match(
      packetPreludeContractBlockers(packet, staleStdout, {
        requireSupported: true,
        requireManagedRuntime: true,
      }).join("\n"),
      /runtime identity is not managed 0\.17\.0/,
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
    [{ embedding_server_identity: { load_generation: 2 } }, /load identities disagree/],
    [{ embedding_server_identity: { executable_version: "0.16.0" } }, /expected 0\.17\.0/],
  ]) {
    const blockers = cachePreparationCanaryBlockers(
      pipelinePreparation("canary", overrides),
      { CODESTORY_EMBED_ALLOW_CPU: "0" },
    );
    assert.match(blockers.join("\n"), expected);
  }
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
    ["embedding_engine_instance_id", "engine-2"],
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

test("nonowner shard establishes a local preparation identity reference", async () => {
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
        ? { embedding_engine_instance_id: "engine-2" }
        : {}),
    ],
    executeRun: async (_opts, run) => {
      launched.push(run.repo);
      return pipelineResult(run);
    },
  });
  assert.equal(outcome.firstFailure.kind, "preparation_identity_mismatch");
  assert.deepEqual(outcome.cachePreparation.map((row) => row.repo), ["first"]);
  assert.equal(launched.includes("second"), false);
  assert.equal(
    outcome.results.some((row) => row.arm === "with_codestory"),
    false,
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
        return [pipelinePreparation(group.repo, { embedding_engine_instance_id: "engine-2" })];
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
    ["embedding_engine_instance_id", "engine-2"],
  ]) {
    const changed = pipelinePreparation("second", { [field]: value });
    assert.match(cachePreparationIdentityBlockers(first, changed).join("\n"), new RegExp(field));
    assert.throws(
      () => benchmarkHostClass([first, changed]),
      /do not share one retrieval engine identity/,
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
  await assert.rejects(
    () => benchmarkShardAttestation(
      fixture.opts,
      fixture.tasks,
      [first, pipelinePreparation("second", { embedding_engine_instance_id: "engine-2" })],
      [],
      CLEAN_SHARD_ATTESTATION,
    ),
    /do not share one retrieval engine identity/,
  );
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

test("materialization scrubs reusable checkouts before installing the bound project manifest", async () => {
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
