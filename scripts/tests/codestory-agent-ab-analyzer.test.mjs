import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  analyzeTranscript,
  agentPublishableBlockers,
  assertSafeWindowsCmdArgs,
  benchmarkRunId,
  commandCategory,
  isPathInside,
  loadTaskForResult,
  loadTasks,
  parseJsonLines,
  packetFirstCommandForPrompt,
  publicCoreCorpusAudit,
  repoProvenanceBlockers,
  scoreQuality,
  taskSnapshotForResult,
} from "../codestory-agent-ab-benchmark.mjs";

const RUNTIME_SERVICE_FILE = "crates/codestory-runtime/src/services.rs";
const RUN_INDEX_SYMBOL = "IndexService::run_indexing_blocking";
const RUNTIME_REFRESH_CLAIM =
  "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases.";

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
  assert.equal(commandCategory('& "C:\\tools\\codestory-cli.exe" packet --project . --question flow'), "codestory_cli");
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
  assert.equal(commandCategory("cargo test -p codestory-cli --test onboarding_contracts"), "build_test");
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

test("packet-first command renders manifest text as PowerShell literals", () => {
  const command = packetFirstCommandForPrompt(
    "Inspect $env:SECRET and $(Get-ChildItem), then read John's file.\nNext line.",
    { task_class: "bug_localization" },
  );

  assert.match(
    command,
    /--question 'Inspect \$env:SECRET and \$\(Get-ChildItem\), then read John''s file\. Next line\.'/,
  );
  assert.match(command, /--task-class 'bug-localization'/);
  assert.throws(
    () => packetFirstCommandForPrompt("Explain the task.", { task_class: "bug_localization; Remove-Item ." }),
    /task_class/,
  );
});

test("benchmark artifact run ids strip path separators from dynamic parts", () => {
  assert.equal(
    benchmarkRunId(["../repo", "task/id", "with codestory", "01"]),
    "repo-task-id-with-codestory-01",
  );
});

test("path containment rejects sibling-prefix directories", () => {
  const root = path.join(os.tmpdir(), "codestory-agent-benchmark", "repos");
  assert.equal(isPathInside(root, path.join(root, "express")), true);
  assert.equal(isPathInside(root, path.join(os.tmpdir(), "codestory-agent-benchmark", "repos2", "evil")), false);
});

test("Windows Codex runner args reject cmd metacharacters", () => {
  assert.doesNotThrow(() => assertSafeWindowsCmdArgs(["exec", "--cd", "C:\\Users\\alber\\source\\repos\\codestory"]));
  assert.throws(
    () => assertSafeWindowsCmdArgs(["exec", "--cd", "C:\\repo&whoami"]),
    /unsafe Windows cmd\.exe argument/,
  );
});

test("public-core corpus keeps publishable coverage locked", async () => {
  const tasks = await loadTasks({
    taskSuite: "public-core",
    taskManifest: null,
    taskIds: null,
    repoCacheDir: path.join("target", "agent-benchmark", "repos"),
  });
  const audit = publicCoreCorpusAudit(tasks);

  assert.equal(tasks.length, 18);
  assert.equal(audit.repo_count, 5);
  assert.deepEqual(Object.keys(audit.class_counts), [
    "architecture_explanation",
    "bug_localization",
    "change_impact",
    "edit_planning",
    "route_tracing",
    "symbol_ownership",
  ]);
  assert.deepEqual(Object.values(audit.class_counts), [3, 3, 3, 3, 3, 3]);
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
    commandEvent("cmd_6", "item.started", "cargo test -p codestory-cli --test onboarding_contracts"),
    commandEvent("cmd_6", "item.completed", "cargo test -p codestory-cli --test onboarding_contracts", "ok"),
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
  assert.equal(quality.citation_coverage.recall, 1);
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

test("publishable provenance requires pinned clean manifest checkout", () => {
  const clean = {
    repo_provenance: {
      manifest_overridden_by_builtin: false,
      configured: { ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7" },
      manifest: { ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7" },
      git_head: "abc123",
      git_dirty: false,
    },
  };
  assert.deepEqual(repoProvenanceBlockers(clean), []);
  assert.match(
    repoProvenanceBlockers({
      repo_provenance: {
        manifest_overridden_by_builtin: false,
        configured: { ref: "main" },
        manifest: { ref: "main" },
        git_head: "abc123",
        git_dirty: false,
      },
    }).join("\n"),
    /not pinned to an immutable commit or tag/,
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
          configured: { ref: "local" },
          manifest: { ref: "main" },
          git_head: "abc123",
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
          manifest_overridden_by_builtin: false,
          configured: { ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7" },
          manifest: { ref: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7" },
          git_head: "9fdfd4650427eb050a11fd9ebd7a4e13dd4b57d7",
          git_dirty: false,
        },
      },
    ],
    { publishable: true },
  );

  assert.equal(blockers.length, 1);
  assert.match(blockers[0].reasons.join("\n"), /missing CodeStory cache provenance/);
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
