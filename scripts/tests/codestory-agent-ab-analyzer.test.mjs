import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, truncate, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  analyzeTranscript,
  agentPublishableBlockers,
  assertSafeWindowsCmdArgs,
  baselineSearchPreludeStatus,
  benchmarkRunId,
  commandCategory,
  copyResultArtifact,
  isTrustedPublishableRepoUrl,
  isPathInside,
  loadTaskForResult,
  loadTasks,
  manifestRepoMaterializationBlockers,
  MAX_REUSED_ARTIFACT_BYTES,
  parseArgs as parseBenchmarkArgs,
  parseJsonLines,
  packetComposition,
  packetCommandArgs,
  packetRuntimeCacheObservations,
  packetForAgentPrompt,
  packetManifestExtraProbes,
  packetManifestQualitySummary,
  packetPreludeManifestComplete,
  packetLatencyTelemetry,
  packetFirstCommandForPrompt,
  packetRuntimePublishableBlockers,
  packetRuntimeQualityGateRequired,
  publicCoreCorpusAudit,
  repoProvenanceBlockers,
  resolveRunArtifactPath,
  resolveCodeStoryCli,
  scoreQuality,
  summarizeCostAccounting,
  summarizePacketRuntimeRuns,
  buildQualityDebugPayload,
  qualityFailureReasons,
  taskSnapshotForResult,
  cachePolicyForRun,
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
    /sidecar preparation is mandatory/,
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
    assert.equal(cachePolicyForRun(observations), "prepared-sidecar-cache-read-only");
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
            { stage: "stage1_zoekt_lexical", elapsed_ms: 2, cache_hit: false },
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

test("categorizes commands without treating source paths as cli invocations", () => {
  assert.equal(commandCategory("& $env:CODESTORY_CLI packet --project . --question flow"), "codestory_cli");
  assert.equal(commandCategory('"${CODESTORY_CLI:-codestory-cli}" packet --project . --question flow'), "codestory_cli");
  assert.equal(commandCategory('"$CODESTORY_CLI" index --project . --refresh full'), "codestory_cli");
  assert.equal(commandCategory('& "C:\\tools\\codestory-cli.exe" packet --project . --question flow'), "codestory_cli");
  assert.equal(
    commandCategory(
      String.raw`"C:\Program Files\PowerShell\pwsh.exe" -Command '& $(if ($env:CODESTORY_CLI) { $env:CODESTORY_CLI } else { 'codestory-cli' }) packet --project . --question 'Trace flow' --task-class 'route-tracing' --budget compact --format json"`,
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

test("packet gate retries only transient sidecar packet failures", async () => {
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
      "Error: retrieval_unavailable: project is not in full mode (mode=no_semantic, reason=qdrant_unreachable)\n",
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

  assert.ok(unixCommand.startsWith('"${CODESTORY_CLI:-codestory-cli}" packet '));
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

test("packet command keeps manifest-derived extra probes diagnostic-only", () => {
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
      usage: { total_tokens: 1 },
      transcript_analysis: analysis,
    },
  ]);
  assert.match(blockers[0].reasons.join("\n"), /external web\/search tool calls=1 > 0/);
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
    "\"C:\\\\Program Files\\\\PowerShell\\\\pwsh.exe\" -Command '$cli = if ($env:CODESTORY_CLI) { $env:CODESTORY_CLI } else { '\"'codestory-cli' }\n& \"'$cli packet --project . --question '\"'Explain flow' --task-class 'architecture-explanation' --budget compact --format json\"";
  const events = [
    commandEvent("cmd_1", "item.started", command),
    commandEvent("cmd_1", "item.completed", command, "{\"packet_id\":\"ask-1\"}", 0),
  ];

  const analysis = analyzeTranscript(events);
  assert.equal(analysis.command_categories.codestory_cli, 1);
  assert.equal(analysis.first_successful_packet_command.id, "cmd_1");
  assert.equal(analysis.packet_was_first_context_command, true);
});

test("recognizes inline PowerShell env fallback CodeStory packet commands", () => {
  const command = String.raw`"C:\Program Files\PowerShell\pwsh.exe" -Command '& $(if ($env:CODESTORY_CLI) { $env:CODESTORY_CLI } else { 'codestory-cli' }) packet --project . --question 'Trace flow' --task-class 'route-tracing' --budget compact --format json"`;
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

test("codestory cli resolver prefers explicit path, release binary, then PATH fallback", () => {
  const explicit = resolveCodeStoryCli({ codestoryCli: "C:/custom/codestory-cli.exe" }, () => {
    throw new Error("explicit path should not probe local candidates");
  });
  assert.equal(explicit, "C:/custom/codestory-cli.exe");

  const release = resolveCodeStoryCli({ codestoryCli: null }, (candidate) =>
    candidate.includes(`${path.sep}target${path.sep}release${path.sep}`),
  );
  assert.match(release, /target[\\/]release[\\/]codestory-cli(?:\.exe)?$/);

  const fallback = resolveCodeStoryCli({ codestoryCli: null }, () => false);
  assert.equal(fallback, "codestory-cli");
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
    sufficiency: {
      status: "partial",
      gaps: ["drop me"],
      open_next: ["drop me too"],
      avoid_opening: [
        "C:/repo/target/agent-benchmark/repos/psf-requests/src/requests/legacy.py because legacy prose",
      ],
      avoid_opening_paths: [
        "C:/repo/target/agent-benchmark/repos/psf-requests/src/requests/api.py",
      ],
      follow_up_commands: ["a", "b", "c", "d", "e"],
      covered_claims: [
        {
          claim: "Session.request prepares requests.",
          citations: [
            {
              display_name: "Session.request",
              file_path:
                "C:/repo/target/agent-benchmark/repos/psf-requests/src/requests/sessions.py",
              line: 557,
            },
          ],
        },
      ],
    },
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
  assert.deepEqual(promptPacket.sufficiency.avoid_opening, ["src/requests/api.py"]);
  assert.deepEqual(promptPacket.sufficiency.follow_up_commands, ["a", "b", "c", "d"]);
  assert.deepEqual(promptPacket.sufficiency.covered_claims, [
    "Session.request prepares requests.",
  ]);
  assert.equal(Object.hasOwn(promptPacket.answer, "sections"), false);
  assert.equal(Object.hasOwn(promptPacket.sufficiency, "gaps"), false);
  assert.equal(Object.hasOwn(promptPacket.sufficiency, "open_next"), false);
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
    sufficiency: {
      covered_claims: [{ claim: "Session.request prepares requests." }],
    },
  };

  const quality = packetManifestQualitySummary(packet, task);
  assert.equal(quality.pass, true);
  assert.equal(
    packetPreludeManifestComplete({
      packet_manifest_quality: quality,
      packet_composition: packetComposition(packet, task),
    }),
    true,
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
      sufficiency: { covered_claims: [] },
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
    cache_policy: "prepared-sidecar-cache-read-only",
    retrieval_mode: "full",
    sidecar_generation: "proj-current",
    manifest_embedding_backend: "llamacpp:bge-base-en-v1.5",
    semantic_backend: "onnx",
    local_only: true,
    locality_kind: "local_model_files",
    indexed: true,
    freshness_status: "fresh",
    semantic_ready: true,
    indexing_in_timed_run: false,
    ...overrides,
  };
}

function publishableWithCodeStoryResult(overrides = {}) {
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
    transcript_analysis: {
      command_count: 1,
      ordinary_source_reads_after_first_packet: 0,
    },
    repo_provenance: pinnedRepoProvenance(),
    codestory_cache_provenance: localCacheProvenance(),
    ...overrides,
  };
}

function publishablePacketRuntimeResult(overrides = {}) {
  return {
    repo: "codestory",
    task_id: "codestory-indexing-flow",
    mode: "cold",
    repeat: 1,
    status: "pass",
    quality: { pass: true },
    sufficiency: {
      status: "sufficient",
      sufficient_quality_mismatch: false,
    },
    packet_latency: {
      sla_missed: false,
      retrieval_shadow: {
        retrieval_mode: "full",
      },
    },
    repo_provenance: pinnedRepoProvenance(),
    codestory_cache_provenance: localCacheProvenance(),
    ...overrides,
  };
}

test("publishable gate blocks avoidable source reads after packet", () => {
  const blockers = agentPublishableBlockers(
    [
      {
        repo: "codestory",
        task_id: "codestory-indexing-flow",
        arm: "with_codestory",
        repeat: 1,
        status: "pass",
        usage: { total_tokens: 100 },
        packet_first_required: true,
        packet_first_pass: true,
        quality: { pass: true },
        transcript_analysis: {
          ordinary_source_reads_after_first_packet: 1,
        },
      },
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
      {
        repo: "vite",
        task_id: "vite-dev-server-architecture",
        arm: "with_codestory",
        repeat: 1,
        status: "pass",
        usage: { total_tokens: 100 },
        packet_first_required: true,
        packet_first_pass: false,
        quality: { pass: true },
        transcript_analysis: {
          ordinary_source_reads_after_first_packet: 0,
        },
      },
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
        usage: { total_tokens: 100 },
        packet_first_required: true,
        packet_first_pass: true,
        quality: { pass: true },
        transcript_analysis: {
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

test("packet runtime publishable gate separates product and harness blockers", () => {
  const blockers = packetRuntimePublishableBlockers(
    [
      publishablePacketRuntimeResult({
        quality: { pass: false },
        codestory_cache_provenance: localCacheProvenance({
          cache_policy: "unprepared-cache-blocked",
          retrieval_mode: "full",
        }),
      }),
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 2);
  assert.deepEqual(blockers.map((blocker) => blocker.category), ["product", "harness-contract"]);
  assert.match(blockers[0].reasons.join("\n"), /manifest quality failed/);
  assert.match(blockers[1].reasons.join("\n"), /CodeStory sidecar cache was not prepared/);
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
