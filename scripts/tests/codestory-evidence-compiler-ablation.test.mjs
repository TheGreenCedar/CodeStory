import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  agentRunnerEnv,
  analyzeTranscript,
  builderContinuationContract,
  composeBuilderAblationPrompt,
  opaqueBuilderAgentHome,
  opaqueBuilderContinuationProofPath,
  packetCommandArgs,
  parseArgs,
  planAgentRuns,
} from "../codestory-agent-ab-benchmark.mjs";
import {
  BUILDER_ABLATION_ARMS,
  BUILDER_ABLATION_TASK_IDS,
  builderOperationViolations,
  codeStoryInvocationsFromCommand,
  codeStoryOperationFromCommand,
  evidenceCompilerBuilderAcceptance,
  marginalValue,
} from "../lib/evidence-compiler-ablation.mjs";
import {
  evaluate,
  packetNextAction,
} from "../codestory-evidence-compiler-ablation-evaluate.mjs";
import {
  JUDGMENTS_CONTRACT,
  finalizeAdjudication,
  prepareBlindedCases,
} from "../codestory-evidence-compiler-ablation-adjudication.mjs";

const cliPath = process.execPath;
const exactOptions = { expectedProject: "/tmp/repo" };
const exactSearchCommand = "$CODESTORY_CLI search --project /tmp/repo --profile agent --run-id shared-agent --query Alpha --repo-text off";

function commandEvent(id, phase, command, output = "", exitCode = 0) {
  return {
    type: `item.${phase}`,
    item: {
      id,
      type: "command_execution",
      command,
      aggregated_output: output,
      exit_code: exitCode,
      status: exitCode === 0 ? "completed" : "failed",
    },
  };
}

test("builder ablation CLI freezes fresh five-arm shape", () => {
  const opts = parseArgs(["--builder-ablation", "--codestory-cli", cliPath]);
  assert.deepEqual(opts.arms, BUILDER_ABLATION_ARMS);
  assert.deepEqual(opts.taskIds, BUILDER_ABLATION_TASK_IDS);
  assert.equal(opts.repeats, 3);
  assert.equal(opts.prepareCodestoryCache, true);
  assert.equal(opts.publishable, false);
  assert.throws(
    () => parseArgs([
      "--builder-ablation",
      "--codestory-cli",
      cliPath,
      "--reuse-baseline-from",
      "/tmp/old",
    ]),
    /builder-ablation mode forbids option/i,
  );
  assert.throws(
    () => parseArgs([
      "--builder-ablation",
      "--codestory-cli",
      cliPath,
      "--diagnostic-extra-probes-from-manifest",
    ]),
    /builder-ablation mode forbids option/i,
  );
});

test("builder run plan uses a balanced per-repeat rotation", () => {
  const tasks = BUILDER_ABLATION_TASK_IDS.map((id, index) => ({
    id,
    repo: `repo-${index}`,
  }));
  const planned = planAgentRuns({ builderAblation: true, repeats: 3 }, tasks);
  assert.equal(planned.length, tasks.length * 3 * BUILDER_ABLATION_ARMS.length);
  for (let index = 0; index < tasks.length * 3; index += 1) {
    const window = planned.slice(index * 5, index * 5 + 5);
    assert.equal(new Set(window.map((row) => row.arm)).size, 5);
    assert.equal(new Set(window.map((row) => row.task.id)).size, 1);
    assert.equal(new Set(window.map((row) => row.repeat)).size, 1);
  }
  const firstPositions = Object.fromEntries(BUILDER_ABLATION_ARMS.map((arm) => [arm, 0]));
  for (let index = 0; index < tasks.length * 3; index += 1) {
    firstPositions[planned[index * 5].arm] += 1;
  }
  assert.ok(Math.max(...Object.values(firstPositions)) - Math.min(...Object.values(firstPositions)) <= 1);
});

test("hidden scorer fields cannot change builder prompt, packet args, environment, or run order", () => {
  const repo = { path: "/tmp/repo", prompt: "fallback" };
  const base = {
    id: "visible-id-a",
    repo: "repo-a",
    prompt: "Explain Alpha and cite its source.",
    task_class: "architecture_explanation",
    expected_files: ["src/answer.rs"],
    expected_symbols: ["Alpha"],
    expected_claims: ["Alpha calls Beta"],
  };
  const hostile = {
    ...base,
    id: "renamed-hidden-id",
    task_class: "bug_localization",
    expected_files: ["totally/different.py"],
    expected_symbols: ["RenamedSecret"],
    expected_claims: ["A different answer shape"],
  };
  const prelude = {
    public: {
      command: "$CODESTORY_CLI packet --task-id visible-id-a --arm packet_semantic_on --repeat 03",
    },
    packet: {
      schema_version: 3,
      kind: "complete",
      status: "continuation_available",
      identity: { packet_id: "packet-1" },
      publication: {
        core: { generation_id: "core-1" },
        retrieval: { retrieval_generation: "retrieval-1" },
      },
      evidence: [],
      gaps: [{ identity: { gap_id: "gap-1" } }],
      continuation: {
        continuation_id: "packet-1",
        remaining_rounds: 1,
        gap_ids: [{ gap_id: "gap-1" }],
      },
    },
  };
  const continuation = builderContinuationContract(
    repo,
    base,
    { builderAblation: true, diagnosticExtraProbesFromManifest: false },
    prelude.packet,
    "packet_semantic_on",
    "/tmp/opaque-4fa40dce.json",
  );
  assert.ok(continuation);
  const otherPacketTreatmentContinuation = builderContinuationContract(
    repo,
    base,
    { builderAblation: true, diagnosticExtraProbesFromManifest: false },
    prelude.packet,
    "packet_semantic_off",
    "/tmp/opaque-4fa40dce.json",
  );
  assert.equal(continuation.command, otherPacketTreatmentContinuation.command);
  assert.equal(continuation.command.includes("--benchmark-disable-dense-semantic"), true);
  assert.equal(
    composeBuilderAblationPrompt("repo-a", repo, "packet_semantic_on", base, {
      codestoryPrelude: prelude,
      builderContinuationContract: continuation,
    }),
    composeBuilderAblationPrompt("repo-a", repo, "packet_semantic_on", hostile, {
      codestoryPrelude: prelude,
      builderContinuationContract: continuation,
    }),
  );
  const prompt = composeBuilderAblationPrompt(
    "repo-a",
    repo,
    "packet_semantic_on",
    base,
    { codestoryPrelude: prelude, builderContinuationContract: continuation },
  );
  assert.equal(prompt.includes(base.id), false);
  assert.equal(prompt.includes(base.task_class), false);
  assert.equal(prompt.includes("packet_semantic_on"), false);
  assert.equal(prompt.includes("--repeat 03"), false);
  assert.equal(prompt.includes("visible-id-a"), false);
  assert.equal(prompt.includes("opaque-4fa40dce.json"), true);
  assert.equal(prompt.includes("Project path: /tmp/repo"), true);
  assert.deepEqual(
    packetCommandArgs(repo, base, { builderAblation: true }, "packet_semantic_off"),
    packetCommandArgs(repo, hostile, { builderAblation: true }, "packet_semantic_off"),
  );
  assert.deepEqual(
    agentRunnerEnv({ PATH: "/bin" }, "/tmp/host", true),
    agentRunnerEnv({ PATH: "/bin" }, "/tmp/host", true),
  );
  const project = (task) => planAgentRuns(
    { builderAblation: true, repeats: 3 },
    [task],
  ).map((row) => [row.repo, row.arm, row.repeat]);
  assert.deepEqual(project(base), project(hostile));
});

test("semantic-off packet command uses the hidden stage switch without manifest probes", () => {
  const args = packetCommandArgs(
    { path: "/tmp/repo", prompt: "fallback" },
    { prompt: "Explain Alpha", expected_files: ["src/answer.rs"] },
    { builderAblation: true, diagnosticExtraProbesFromManifest: false },
    "packet_semantic_off",
  );
  assert.ok(args.includes("--benchmark-disable-dense-semantic"));
  assert.equal(args.includes("--extra-probe"), false);
});

test("continuation proof paths are opaque and cannot encode experimental identities", () => {
  const proofPath = opaqueBuilderContinuationProofPath(
    "/tmp/codestory-agent-builder-ablation-private",
    () => Buffer.alloc(24, 0xab),
  );
  assert.equal(
    proofPath,
    "/tmp/codestory-agent-builder-ablation-private/continuation-proofs/abababababababababababababababababababababababab.json",
  );
  for (const hidden of [
    "python-requests-session-flow",
    "packet_semantic_off",
    "repeat-03",
  ]) {
    assert.equal(proofPath.includes(hidden), false);
  }
});

test("builder agent homes have one opaque treatment-blind path shape", () => {
  const home = opaqueBuilderAgentHome(
    "/tmp/codestory-agent-builder-ablation-private",
    () => Buffer.alloc(24, 0xcd),
  );
  assert.equal(
    home,
    "/tmp/codestory-agent-builder-ablation-private/agent-sessions/cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd/host",
  );
  for (const arm of BUILDER_ABLATION_ARMS) assert.equal(home.includes(arm), false);
});

test("operation parser and policy fail closed on arm leakage", () => {
  const continuationContract = {
    parent_packet_id: "parent-1",
    allowed_option_ids: ["gap-1", "gap-2"],
    core_generation_id: "core-1",
    retrieval_generation: "retrieval-1",
    project: "/tmp/repo",
    question: "Explain Alpha",
    question_sha256: createHash("sha256").update("Explain Alpha").digest("hex"),
    proof_path: "/tmp/opaque-proof.json",
  };
  assert.equal(
    codeStoryOperationFromCommand('"$CODESTORY_CLI" callers --project . --query Alpha'),
    "callers",
  );
  assert.deepEqual(
    builderOperationViolations("exact_identity_source", [{
      operation: "search",
      transport: "cli",
      successful: true,
      raw: { command: exactSearchCommand },
    }], exactOptions),
    [],
  );
  assert.deepEqual(
    builderOperationViolations("exact_plus_relations", [{
      operation: "callers",
      transport: "cli",
      source: "agent_cli",
      successful: true,
      raw: { command: "$CODESTORY_CLI callers --project /tmp/repo --query Alpha --depth 1" },
    }], exactOptions),
    [],
  );
  assert.match(
    builderOperationViolations("exact_identity_source", [{
      operation: "callers",
      transport: "cli",
      successful: true,
      raw: { command: "$CODESTORY_CLI callers --query Alpha" },
    }], exactOptions)[0],
    /forbidden/,
  );
  assert.match(
    builderOperationViolations("exact_identity_source", [{
      operation: "search",
      transport: "cli",
      successful: true,
      raw: { command: "codestory-cli search --query Alpha --repo-text off" },
    }], exactOptions)[0],
    /checksum-bound/,
  );
  assert.match(
    builderOperationViolations("exact_identity_source", [{
      operation: "search",
      transport: "cli",
      successful: true,
      raw: {
        command: "CODESTORY_CACHE_ROOT=/tmp/other $CODESTORY_CLI search --query Alpha --repo-text off",
      },
    }], exactOptions).join("\n"),
    /benchmark state/,
  );
  assert.match(
    builderOperationViolations("exact_identity_source", [{
      operation: "search",
      transport: "cli",
      successful: true,
      raw: {
        command: "$CODESTORY_CLI search --query Alpha --repo-text off; cat src/lib.rs",
      },
    }], exactOptions).join("\n"),
    /uncomposed/,
  );
  assert.match(
    builderOperationViolations("packet_semantic_off", [
      { operation: "packet", transport: "cli", source: "harness_packet_prelude", successful: true, raw: {} },
      {
        operation: "packet",
        transport: "cli",
        source: "agent_cli",
        successful: true,
        raw: { command: "$CODESTORY_CLI packet --parent-packet-id p --option-id o --core-generation-id c --retrieval-generation r" },
      },
    ], { continuationContract })[0],
    /dense policy/,
  );
  const validContinuation = `$CODESTORY_CLI packet --project /tmp/repo --profile agent --run-id shared-agent --question 'Explain Alpha' --budget standard --format json --benchmark-disable-dense-semantic --parent-packet-id parent-1 --option-id gap-2 --core-generation-id core-1 --retrieval-generation retrieval-1 --benchmark-retrieval-proof-out /tmp/opaque-proof.json`;
  assert.deepEqual(
    builderOperationViolations("packet_semantic_off", [
      { operation: "packet", transport: "cli", source: "harness_packet_prelude", successful: true, raw: {} },
      {
        operation: "packet",
        transport: "cli",
        source: "agent_cli",
        successful: true,
        raw: { command: validContinuation },
      },
    ], { continuationContract }),
    [],
  );
  const inventedContinuation = validContinuation
    .replace("parent-1", "fake-parent")
    .replace("gap-2", "self-authored-option");
  assert.match(
    builderOperationViolations("packet_semantic_off", [
      { operation: "packet", transport: "cli", source: "harness_packet_prelude", successful: true, raw: {} },
      {
        operation: "packet",
        transport: "cli",
        source: "agent_cli",
        successful: false,
        raw: { command: inventedContinuation },
      },
    ], { continuationContract }).join("\n"),
    /continuation/i,
  );
  assert.deepEqual(
    codeStoryInvocationsFromCommand(
      '"${CODESTORY_CLI}" search --query Alpha --repo-text off; codestory-cli packet --question Alpha',
    ),
    [
      { operation: "search", checksum_bound: true },
      { operation: "packet", checksum_bound: false },
    ],
  );
  const compound = analyzeTranscript([
    commandEvent(
      "compound",
      "completed",
      '"${CODESTORY_CLI}" search --query Alpha --repo-text off; "$CODESTORY_CLI" packet --question Alpha',
      "{}",
    ),
  ]);
  assert.deepEqual(
    compound.codestory_operations.map((entry) => entry.operation),
    ["search", "packet"],
  );
  assert.match(
    builderOperationViolations("exact_identity_source", compound.codestory_operations, exactOptions).join("\n"),
    /packet/,
  );
  assert.equal(
    codeStoryInvocationsFromCommand("echo $CODESTORY_CLI")[0].operation,
    null,
  );
  for (const wrapped of [
    `sh -c '\"$CODESTORY_CLI\" search --query Alpha --repo-text off; cat src/lib.rs'`,
    `eval '$CODESTORY_CLI search --query Alpha --repo-text off'`,
    `env FOO=bar $CODESTORY_CLI search --query Alpha --repo-text off`,
    `$CODESTORY_CLI search --query \"$(cat src/lib.rs)\" --repo-text off`,
    `$CODESTORY_CLI search --query \"\u0060cat src/lib.rs\u0060\" --repo-text off`,
    `sh -c \"$(printenv CODESTORY_CLI) search --query Alpha --repo-text off\"`,
  ]) {
    const wrapperAnalysis = analyzeTranscript([
      commandEvent("wrapped", "completed", wrapped, "source text"),
    ]);
    assert.notEqual(
      builderOperationViolations(
        "exact_identity_source",
        wrapperAnalysis.codestory_operations,
        exactOptions,
      ).length,
      0,
      wrapped,
    );
  }
  assert.match(
    builderOperationViolations("exact_identity_source", [{
      operation: "search",
      transport: "mcp",
      source: "agent_mcp",
      successful: true,
      raw: {},
    }], exactOptions).join("\n"),
    /checksum-bound CodeStory CLI/,
  );

  const escaped = builderOperationViolations("exact_identity_source", [{
    operation: "search",
    transport: "cli",
    source: "agent_cli",
    successful: true,
    raw: {
      command: "$CODESTORY_CLI search --project /tmp/foreign --cache-dir /tmp/foreign-cache --query Alpha --repo-text off --refresh full --profile local --run-id other --output-file /tmp/leak.json",
    },
  }], exactOptions).join("\n");
  assert.match(escaped, /--cache-dir/);
  assert.match(escaped, /--refresh/);
  assert.match(escaped, /--output-file/);
  assert.match(escaped, /pinned repository/);
  assert.match(escaped, /prepared agent profile/);
});

test("transcript records observed repository context after CodeStory", () => {
  const events = [
    commandEvent("cs", "started", "$CODESTORY_CLI search --query Alpha --repo-text off"),
    commandEvent("cs", "completed", "$CODESTORY_CLI search --query Alpha --repo-text off", "result"),
    commandEvent("rg", "started", "rg -n Alpha src"),
    commandEvent("rg", "completed", "rg -n Alpha src", "src/lib.rs:1: Alpha\n"),
    commandEvent("read", "started", "head -20 src/lib.rs"),
    commandEvent("read", "completed", "head -20 src/lib.rs", "fn Alpha() {}\n"),
    commandEvent("node-read", "started", "node -e \"require('fs').readFileSync('src/lib.rs','utf8')\""),
    commandEvent("node-read", "completed", "node -e \"require('fs').readFileSync('src/lib.rs','utf8')\"", "fn Beta() {}\n"),
    commandEvent("git-read", "started", "/usr/bin/git -C . show HEAD:src/lib.rs"),
    commandEvent("git-read", "completed", "/usr/bin/git -C . show HEAD:src/lib.rs", "fn Gamma() {}\n"),
    commandEvent("opaque-read", "started", "custom-reader src/lib.rs"),
    commandEvent("opaque-read", "completed", "custom-reader src/lib.rs", "fn Delta() {}\n"),
  ];
  const analysis = analyzeTranscript(events, "/tmp/repo", {
    arm: "exact_identity_source",
    task: { prompt: "Explain Alpha" },
  });
  assert.deepEqual(analysis.codestory_operations.map((row) => row.operation), ["search"]);
  assert.equal(analysis.codestory_was_first_context_command, true);
  assert.equal(analysis.ordinary_source_actions_after_first_codestory, 5);
  assert.equal(analysis.exploratory_source_reads_after_first_codestory, 5);
  assert.equal(analysis.exploratory_repository_context_actions_after_first_codestory, 5);
  assert.equal(
    analysis.ordinary_source_output_bytes_after_first_codestory,
    Buffer.byteLength("src/lib.rs:1: Alpha\nfn Alpha() {}\nfn Beta() {}\nfn Gamma() {}\nfn Delta() {}\n", "utf8"),
  );
  const sourceBeforeCodeStory = analyzeTranscript([
    commandEvent("head", "started", "head -20 src/lib.rs"),
    commandEvent("head", "completed", "head -20 src/lib.rs", "fn Alpha() {}\n"),
    commandEvent("cs", "started", exactSearchCommand),
    commandEvent("cs", "completed", exactSearchCommand, "result"),
  ], "/tmp/repo");
  assert.equal(sourceBeforeCodeStory.codestory_was_first_context_command, false);
  const remote = analyzeTranscript([
    commandEvent("local", "completed", "rg -n 'git fetch' src", "src/lib.rs:1: git fetch"),
    commandEvent("remote", "completed", "git fetch origin main", ""),
  ]);
  assert.deepEqual(remote.remote_context_commands, ["git fetch origin main"]);
});

test("observed output closes failed, extensionless, and globbed reader accounting gaps", () => {
  const failedBeforeCodeStory = analyzeTranscript([
    commandEvent("failed-head", "started", "head -20 src/lib.rs; false"),
    commandEvent(
      "failed-head",
      "completed",
      "head -20 src/lib.rs; false",
      "fn Alpha() {}\n",
      1,
    ),
    commandEvent("cs", "started", exactSearchCommand),
    commandEvent("cs", "completed", exactSearchCommand, "result"),
  ], "/tmp/repo", { arm: "exact_identity_source" });
  assert.equal(failedBeforeCodeStory.codestory_was_first_context_command, false);
  assert.equal(
    failedBeforeCodeStory.first_observed_repository_context_action.id,
    "failed-head",
  );

  const extensionlessBeforeCodeStory = analyzeTranscript([
    commandEvent("makefile", "started", "custom-reader Makefile"),
    commandEvent("makefile", "completed", "custom-reader Makefile", "all: build\n"),
    commandEvent("cs", "started", exactSearchCommand),
    commandEvent("cs", "completed", exactSearchCommand, "result"),
  ], "/tmp/repo", { arm: "exact_identity_source" });
  assert.equal(extensionlessBeforeCodeStory.codestory_was_first_context_command, false);

  const afterCodeStory = analyzeTranscript([
    commandEvent("cs", "started", exactSearchCommand),
    commandEvent("cs", "completed", exactSearchCommand, "result"),
    commandEvent("glob", "started", "custom-reader src/lib.*"),
    commandEvent("glob", "completed", "custom-reader src/lib.*", "fn Beta() {}\n"),
    commandEvent("failed", "started", "custom-reader Makefile"),
    commandEvent("failed", "completed", "custom-reader Makefile", "all: build\n", 1),
    commandEvent("empty-search", "started", "rg DoesNotExist ."),
    commandEvent("empty-search", "completed", "rg DoesNotExist .", "", 1),
    commandEvent("empty-unknown", "started", "custom-reader missing"),
    commandEvent("empty-unknown", "completed", "custom-reader missing", "", 1),
  ], "/tmp/repo", { arm: "exact_identity_source" });
  assert.equal(afterCodeStory.exploratory_repository_context_actions_after_first_codestory, 3);
  assert.equal(
    afterCodeStory.observed_repository_context_output_bytes_after_first_codestory,
    Buffer.byteLength("fn Beta() {}\nall: build\n", "utf8"),
  );
});

test("parallel exploration is sliced after the matched CodeStory action", () => {
  const parallelCommand = analyzeTranscript([
    commandEvent("cs", "started", exactSearchCommand),
    commandEvent("reader", "started", "custom-reader Makefile"),
    commandEvent("reader", "completed", "custom-reader Makefile", "all: build\n"),
    commandEvent("cs", "completed", exactSearchCommand, "result"),
  ], "/tmp/repo", { arm: "exact_identity_source" });
  assert.equal(parallelCommand.codestory_was_first_repository_context_action, true);
  assert.equal(
    parallelCommand.exploratory_repository_context_actions_after_first_codestory,
    1,
  );

  const parallelTool = analyzeTranscript([
    commandEvent("cs", "started", exactSearchCommand),
    {
      type: "item.started",
      item: {
        id: "reader-tool",
        type: "function_call",
        name: "read_repository_file",
      },
    },
    commandEvent("cs", "completed", exactSearchCommand, "result"),
  ], "/tmp/repo", { arm: "exact_identity_source" });
  assert.equal(parallelTool.codestory_was_first_repository_context_action, true);
  assert.equal(
    parallelTool.exploratory_repository_context_actions_after_first_codestory,
    1,
  );
});

function passingRows() {
  return BUILDER_ABLATION_TASK_IDS.flatMap((taskId) =>
    BUILDER_ABLATION_ARMS.flatMap((arm) => [1, 2, 3].map((repeat) => ({
      task_id: taskId,
      arm,
      repeat,
      status: "pass",
      quality: { pass: true },
      usage: { input_tokens: arm === "packet_semantic_off" ? 100 : 100 },
      installed_agent_timing: {
        timing_cohort_id: createHash("sha256").update(`${taskId}-${repeat}`).digest("hex"),
        whole_task_wall_ms: 100,
      },
      installed_agent_timing_eligible: true,
      transcript_analysis: {
        codestory_was_first_repository_context_action: arm !== "native_tools",
        exploratory_repository_context_actions_after_first_codestory:
          arm === "packet_semantic_off" ? 4 : 5,
        ordinary_source_output_bytes_after_first_codestory:
          arm === "packet_semantic_off" ? 8 : 10,
      },
      builder_ablation: {
        operation_violations: [],
        continuation_offer: null,
        first_codestory_required: arm !== "native_tools",
        first_codestory_pass: true,
      },
      task_manifest_snapshot: { prompt: `Question for ${taskId}` },
      repo_provenance: { git_dirty: false, git_head: "d".repeat(40) },
      codestory_prelude_cli_sha256: arm === "native_tools" ? null : "a".repeat(64),
      codestory_cache_provenance: arm === "native_tools"
        ? null
        : {
            storage_path: `/cache/${taskId}/codestory.db`,
            retrieval_status: {
              sidecar_generation: `retrieval-${taskId}`,
              semantic_generation: `semantic-${taskId}`,
            },
          },
      codestory_harness_prelude: arm.startsWith("packet_")
        ? {
            packet_retrieval_proof: {
              contract: "codestory.packet-builder-ablation-receipt/v1",
              requested_dense_semantic: arm === "packet_semantic_on",
              request: {
                question_sha256: createHash("sha256")
                  .update(`Question for ${taskId}`)
                  .digest("hex"),
                parent_packet_id: null,
                option_ids: [],
                core_generation_id: null,
                retrieval_generation: null,
              },
              retrieval_proof: {
                contract: "codestory.packet-dense-candidate-ablation-proof/v1",
                requested_policy: arm === "packet_semantic_on"
                  ? "repository_graph_lexical_dense_candidate_stage_enabled_v1"
                  : "repository_graph_lexical_dense_candidate_stage_disabled_v1",
                descriptor_query_count: 1,
                descriptor_cache_hit_count: 0,
                descriptor_stage_invocations: arm === "packet_semantic_on"
                  ? { stage1b_semantic: 1 }
                  : { stage1_lexical: 1 },
                descriptor_stage_candidates: {},
                dense_semantic_stage_invocations: arm === "packet_semantic_on" ? 1 : 0,
              },
              core_generation_id: `core-${taskId}`,
              core_run_id: `run-${taskId}`,
              retrieval_generation: `retrieval-${taskId}`,
              semantic_generation: `semantic-${taskId}`,
            },
          }
        : null,
    })))
  );
}

function zeroAdjudication(rows) {
  return {
    contract: "codestory.evidence-compiler-builder-adjudication/v1",
    blinded: true,
    independent_reviewer: "independent-test-reviewer",
    source_cases_sha256: "b".repeat(64),
    source_judgments_sha256: "c".repeat(64),
    rows: rows
      .filter((row) => row.arm !== "native_tools")
      .map((row) => ({
        task_id: row.task_id,
        arm: row.arm,
        repeat: row.repeat,
        critical_factual_errors: 0,
        critical_factual_finding_ids: [],
        unsupported_relation_claims: 0,
        unsupported_relation_finding_ids: [],
      })),
  };
}

test("frozen packet acceptance passes only with complete independent adjudication", () => {
  const rows = passingRows();
  const accepted = evidenceCompilerBuilderAcceptance(rows, zeroAdjudication(rows));
  assert.equal(accepted.pass, true, accepted.reasons.join("; "));
  assert.equal(accepted.exploratory_repository_context_action_ratio, 0.8);
  const incomplete = evidenceCompilerBuilderAcceptance(rows, null);
  assert.equal(incomplete.pass, false);
  assert.match(incomplete.reasons.join("\n"), /adjudication/i);

  const missingObservedBoundary = passingRows();
  missingObservedBoundary.find((row) => row.arm === "exact_identity_source")
    .transcript_analysis.codestory_was_first_repository_context_action = false;
  const missingObservedReceipt = evidenceCompilerBuilderAcceptance(
    missingObservedBoundary,
    zeroAdjudication(missingObservedBoundary),
  );
  assert.equal(missingObservedReceipt.pass, false);
  assert.match(missingObservedReceipt.reasons.join("\n"), /observed repository-context accounting/);

  for (const row of rows) {
    row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 0;
  }
  const noDemonstratedReduction = evidenceCompilerBuilderAcceptance(rows, zeroAdjudication(rows));
  assert.equal(noDemonstratedReduction.pass, false);
  assert.equal(noDemonstratedReduction.exploratory_repository_context_action_ratio, 1);

  const byteOnlyRows = passingRows();
  for (const row of byteOnlyRows) {
    row.transcript_analysis.ordinary_source_output_bytes_after_first_codestory =
      row.arm === "packet_semantic_off" ? 100_000 : 1;
  }
  assert.equal(
    evidenceCompilerBuilderAcceptance(byteOnlyRows, zeroAdjudication(byteOnlyRows)).pass,
    true,
    "source output size is diagnostic and cannot replace the frozen read-count gate",
  );
  const moreShortReads = passingRows();
  for (const row of moreShortReads) {
    if (row.arm === "packet_semantic_off") {
      row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 5;
      row.transcript_analysis.ordinary_source_output_bytes_after_first_codestory = 1;
    } else if (row.arm === "exact_plus_relations") {
      row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 4;
      row.transcript_analysis.ordinary_source_output_bytes_after_first_codestory = 100_000;
    }
  }
  const readCountWins = evidenceCompilerBuilderAcceptance(
    moreShortReads,
    zeroAdjudication(moreShortReads),
  );
  assert.equal(readCountWins.pass, false);
  assert.equal(readCountWins.exploratory_repository_context_action_ratio, 1.25);
});

test("packet continuations require a same-publication dense-stage execution proof", () => {
  const rows = passingRows();
  const row = rows.find((entry) => entry.arm === "packet_semantic_off");
  const questionSha256 = createHash("sha256")
    .update(row.task_manifest_snapshot.prompt)
    .digest("hex");
  row.builder_ablation.continuation_offer = {
    parent_packet_id: "packet-1",
    allowed_option_ids: ["gap-1"],
    core_generation_id: `core-${row.task_id}`,
    retrieval_generation: `retrieval-${row.task_id}`,
    question_sha256: questionSha256,
  };
  row.transcript_analysis.codestory_operations = [
    { operation: "packet", source: "harness_packet_prelude", successful: true },
    { operation: "packet", source: "agent_cli", successful: true },
  ];
  row.builder_ablation.continuation_retrieval_proof = structuredClone(
    row.codestory_harness_prelude.packet_retrieval_proof,
  );
  row.builder_ablation.continuation_retrieval_proof.request = {
    question_sha256: questionSha256,
    parent_packet_id: "packet-1",
    option_ids: ["gap-1"],
    core_generation_id: `core-${row.task_id}`,
    retrieval_generation: `retrieval-${row.task_id}`,
  };
  assert.equal(
    evidenceCompilerBuilderAcceptance(rows, zeroAdjudication(rows)).pass,
    true,
  );
  row.builder_ablation.continuation_retrieval_proof.request.parent_packet_id = "invented-parent";
  const invented = evidenceCompilerBuilderAcceptance(rows, zeroAdjudication(rows));
  assert.equal(invented.pass, false);
  assert.match(invented.reasons.join("\n"), /continuation execution proof/);
  row.builder_ablation.continuation_retrieval_proof.request.parent_packet_id = "packet-1";
  row.builder_ablation.continuation_retrieval_proof.retrieval_generation = "different";
  const changed = evidenceCompilerBuilderAcceptance(rows, zeroAdjudication(rows));
  assert.equal(changed.pass, false);
  assert.match(changed.reasons.join("\n"), /continuation execution proof/);
});

test("packet timing fails closed on cohort drift and serializes zero-denominator ratios", () => {
  const cohortRows = passingRows();
  const packet = cohortRows.find((entry) => entry.arm === "packet_semantic_off");
  packet.installed_agent_timing.timing_cohort_id = "f".repeat(64);
  const drifted = evidenceCompilerBuilderAcceptance(cohortRows, zeroAdjudication(cohortRows));
  assert.equal(drifted.pass, false);
  assert.match(drifted.reasons.join("\n"), /cohort identities/);

  const infiniteRows = passingRows();
  for (const row of infiniteRows) {
    if (row.arm === "exact_plus_relations") {
      row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 0;
    }
    if (row.arm === "packet_semantic_off") {
      row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 1;
    }
  }
  const infinite = evidenceCompilerBuilderAcceptance(
    infiniteRows,
    zeroAdjudication(infiniteRows),
  );
  assert.equal(infinite.pass, false);
  assert.equal(infinite.exploratory_repository_context_action_ratio, "infinite");
});

test("builder Codex sessions use isolated ephemeral homes without MCP config", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-builder-isolation-"));
  const sourceHome = path.join(root, "source-home");
  const stateRoot = path.join(root, "state");
  const outDir = path.join(root, "out");
  await mkdir(sourceHome, { recursive: true });
  await mkdir(stateRoot, { recursive: true });
  await mkdir(outDir, { recursive: true });
  await writeFile(path.join(sourceHome, "auth.json"), "{}\n");
  const harnessUrl = new URL("../codestory-agent-ab-benchmark.mjs", import.meta.url).href;
  const script = `
    const benchmark = await import(${JSON.stringify(harnessUrl)});
    const result = await benchmark.prepareAgentCodexIsolation(${JSON.stringify(outDir)}, {
      builderAblation: true,
      builderAblationStateRoot: ${JSON.stringify(stateRoot)},
      model: "gpt-5.6-sol",
    });
    process.stdout.write(JSON.stringify(result));
  `;
  try {
    const { spawnSync } = await import("node:child_process");
    const child = spawnSync(process.execPath, ["--input-type=module", "-e", script], {
      cwd: root,
      env: { ...process.env, CODEX_HOME: sourceHome },
      encoding: "utf8",
    });
    assert.equal(child.status, 0, child.stderr);
    const isolation = JSON.parse(child.stdout);
    assert.equal(isolation.receipt.contract, "codestory.agent-benchmark-codex-isolation/v4");
    assert.equal(isolation.receipt.host_config_files, "none");
    assert.deepEqual(Object.keys(isolation.homes), BUILDER_ABLATION_ARMS);
    for (const [arm, home] of Object.entries(isolation.homes)) {
      assert.equal(existsSync(path.join(home, "auth.json")), true);
      assert.equal(existsSync(path.join(home, "config.toml")), false);
      assert.equal(home.includes(arm), false);
      assert.match(
        path.relative(stateRoot, home).replaceAll(path.sep, "/"),
        /^agent-sessions\/[0-9a-f]{48}\/host$/u,
      );
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("packet-only critical findings are compared by identity rather than count", () => {
  const rows = passingRows();
  const adjudication = zeroAdjudication(rows);
  const packet = adjudication.rows.find((row) => row.arm === "packet_semantic_off");
  const control = adjudication.rows.find((row) =>
    row.arm === "exact_plus_relations" &&
    row.task_id === packet.task_id &&
    row.repeat === packet.repeat
  );
  packet.critical_factual_errors = 1;
  packet.critical_factual_finding_ids = ["packet-only-wrong-edge"];
  control.critical_factual_errors = 1;
  control.critical_factual_finding_ids = ["different-control-error"];
  const receipt = evidenceCompilerBuilderAcceptance(rows, adjudication);
  assert.equal(receipt.pass, false);
  assert.deepEqual(
    receipt.packet_only_critical_claims[0].packet_only_finding_ids,
    ["factual:packet-only-wrong-edge"],
  );
});

test("evaluator binds independent adjudication to the exact run ledger", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-builder-ablation-"));
  try {
    const rows = passingRows();
    const runBytes = Buffer.from(`${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
    const adjudication = {
      ...zeroAdjudication(rows),
      source_runs_sha256: createHash("sha256").update(runBytes).digest("hex"),
    };
    await writeFile(path.join(root, "runs.jsonl"), runBytes);
    const adjudicationPath = path.join(root, "adjudication.json");
    await writeFile(adjudicationPath, `${JSON.stringify(adjudication)}\n`);

    const receipt = await evaluate({
      runDir: root,
      adjudication: adjudicationPath,
      attempt: "initial",
    });
    assert.equal(receipt.packet_acceptance.pass, true);
    assert.equal(receipt.packet_decision, "advance");
    assert.deepEqual(receipt.default_layer_decisions, {
      graph_relations: "disable_default",
      dense_semantic_candidates: "disable_default",
    });
    assert.equal(
      JSON.parse(await readFile(path.join(root, "builder-ablation.json"), "utf8"))
        .source_runs_sha256,
      adjudication.source_runs_sha256,
    );

    await writeFile(adjudicationPath, `${JSON.stringify({
      ...adjudication,
      source_runs_sha256: "0".repeat(64),
    })}\n`);
    await assert.rejects(
      () => evaluate({ runDir: root, adjudication: adjudicationPath, attempt: "initial" }),
      /does not match runs\.jsonl/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("blinded adjudication withholds arm identity and rejoins exact rows", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-builder-blind-"));
  try {
    const runDir = path.join(root, "run");
    const blindDir = path.join(root, "blind");
    await mkdir(runDir, { recursive: true });
    const rows = passingRows();
    for (const row of rows) {
      const stdoutPath = path.join(
        runDir,
        `${row.task_id}-${row.arm}-${row.repeat}.stdout.jsonl`,
      );
      await writeFile(stdoutPath, `${JSON.stringify({
        type: "item.completed",
        item: { type: "agent_message", text: `Answer for ${row.task_id} repeat ${row.repeat}` },
      })}\n`);
      row.stdout_path = stdoutPath;
      row.repo = `repo-${row.task_id}`;
      row.repo_path = root;
      row.repo_provenance = { git_head: "a".repeat(40) };
      row.task_manifest_snapshot = { prompt: `Question for ${row.task_id}` };
    }
    await writeFile(
      path.join(runDir, "runs.jsonl"),
      `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`,
    );
    const prepared = await prepareBlindedCases({ runDir, outputDir: blindDir });
    const cases = JSON.parse(await readFile(prepared.casesPath, "utf8"));
    const serializedCases = JSON.stringify(cases);
    assert.equal(cases.cases.length, 96);
    assert.equal(serializedCases.includes("packet_semantic_off"), false);
    assert.equal(serializedCases.includes("exact_plus_relations"), false);
    const judgmentsPath = path.join(blindDir, "judgments.json");
    await writeFile(judgmentsPath, `${JSON.stringify({
      contract: JUDGMENTS_CONTRACT,
      source_cases_sha256: prepared.casesSha256,
      independent_reviewer: "independent-test-reviewer",
      rows: cases.cases.map((entry) => ({
        case_id: entry.case_id,
        critical_factual_errors: 0,
        critical_factual_finding_ids: [],
        unsupported_relation_claims: 0,
        unsupported_relation_finding_ids: [],
        notes: "fixture",
      })),
    })}\n`);
    const outputPath = path.join(blindDir, "adjudication.json");
    const adjudication = await finalizeAdjudication({
      runDir,
      casesPath: prepared.casesPath,
      mapPath: prepared.mapPath,
      judgmentsPath,
      outputPath,
    });
    assert.equal(adjudication.blinded, true);
    assert.equal(adjudication.rows.length, 96);
    assert.deepEqual(new Set(adjudication.rows.map((row) => row.arm)), new Set([
      "exact_identity_source",
      "exact_plus_relations",
      "packet_semantic_off",
      "packet_semantic_on",
    ]));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("marginal value rejects neutral and cost-regressing layers", () => {
  const rows = passingRows();
  const adjudication = zeroAdjudication(rows);
  const semantic = marginalValue(
    rows,
    "packet_semantic_on",
    "packet_semantic_off",
    adjudication,
  );
  assert.equal(semantic.positive, false);
  const graph = marginalValue(
    rows,
    "exact_plus_relations",
    "exact_identity_source",
    adjudication,
  );
  assert.equal(graph.positive, false);
});

test("marginal decisions fail closed on candidate-only critical claims", () => {
  const rows = passingRows();
  for (const row of rows.filter((entry) => entry.arm === "exact_plus_relations")) {
    row.quality.pass = true;
    row.transcript_analysis.exploratory_repository_context_actions_after_first_codestory = 4;
  }
  const adjudication = zeroAdjudication(rows);
  const candidate = adjudication.rows.find((row) => row.arm === "exact_plus_relations");
  candidate.unsupported_relation_claims = 1;
  candidate.unsupported_relation_finding_ids = ["candidate-only-edge"];
  const graph = marginalValue(
    rows,
    "exact_plus_relations",
    "exact_identity_source",
    adjudication,
  );
  assert.equal(graph.positive, false);
  assert.equal(graph.candidate_only_critical_claims.length, 1);
});

test("machine stop rule distinguishes first revision from terminal failure", () => {
  assert.equal(packetNextAction(true, "initial", null), "advance");
  assert.equal(packetNextAction(false, "initial", null), "failed_needs_causal_classification");
  assert.equal(packetNextAction(false, "initial", "new"), "revise_once");
  assert.equal(packetNextAction(false, "initial", "equivalent"), "stop");
  assert.equal(packetNextAction(false, "general_revision", "new"), "stop");
  assert.throws(
    () => packetNextAction(true, "initial", "new"),
    /passing packet gate/,
  );
});

test("evaluator writes the bounded revision or stop decision for a failed packet gate", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-builder-stop-rule-"));
  try {
    const rows = passingRows();
    for (const row of rows.filter((entry) => entry.arm === "packet_semantic_off")) {
      row.quality.pass = false;
    }
    const runBytes = Buffer.from(`${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
    const adjudication = {
      ...zeroAdjudication(rows),
      source_runs_sha256: createHash("sha256").update(runBytes).digest("hex"),
    };
    await writeFile(path.join(root, "runs.jsonl"), runBytes);
    const adjudicationPath = path.join(root, "adjudication.json");
    await writeFile(adjudicationPath, `${JSON.stringify(adjudication)}\n`);

    const unclassified = await evaluate({
      runDir: root,
      adjudication: adjudicationPath,
      attempt: "initial",
    });
    assert.equal(unclassified.packet_decision, "failed_needs_causal_classification");
    const revision = await evaluate({
      runDir: root,
      adjudication: adjudicationPath,
      attempt: "initial",
      causalClassification: "new",
    });
    assert.equal(revision.packet_decision, "revise_once");
    const equivalent = await evaluate({
      runDir: root,
      adjudication: adjudicationPath,
      attempt: "initial",
      causalClassification: "equivalent",
    });
    assert.equal(equivalent.packet_decision, "stop");
    const exhausted = await evaluate({
      runDir: root,
      adjudication: adjudicationPath,
      attempt: "general_revision",
    });
    assert.equal(exhausted.packet_decision, "stop");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
