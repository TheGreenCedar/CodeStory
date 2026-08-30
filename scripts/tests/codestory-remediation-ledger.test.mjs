import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildRemediationFailureLedger,
} from "../codestory-remediation-ledger.mjs";

async function writeFixture(runDir, packetPath = "/private/tmp/retained/task-candidate.codestory-packet.stdout.json") {
  const task = {
    id: "dart-http-client-flow",
    repo: "dart-lang-http",
    task_class: "data_flow",
  };
  await writeFile(path.join(runDir, "summary.json"), `${JSON.stringify({
    model: "gpt-5.6-sol",
    runner: "codex",
    host_class: { platform: "darwin", arch: "arm64" },
    tasks: [task],
  })}\n`);
  await writeFile(path.join(runDir, "summary.md"), "# retained receipt\n");
  await writeFile(path.join(runDir, "task-candidate.codestory-packet.stdout.json"), `${JSON.stringify({
    schema_version: 3,
    kind: "complete",
    status: "available",
    retrieval: { state: "full" },
    evidence: [{
      identity: { evidence_id: "source-1" },
      kind: "exact_source",
      path: "pkgs/http/lib/http.dart",
      symbol_id: "Client.get",
      start_line: 42,
      end_line: 55,
    }],
    gaps: [{ identity: { gap_id: "gap-1" }, kind: "output_budget_exceeded" }],
    continuation: null,
  })}\n`);
  const common = {
    repo: task.repo,
    task_id: task.id,
    task_class: task.task_class,
    repeat: 1,
    runner: "codex",
    model: "gpt-5.6-sol",
    status: "pass",
    comparative_wall_time_eligible: true,
    wall_ms: 100,
    agent_runner_wall_ms: 80,
    benchmark_contract: {
      scorer_hash: "1".repeat(64),
      compatibility_fingerprint: "2".repeat(64),
      task_manifest_hash: "3".repeat(64),
    },
    task_manifest_snapshot: { id: task.id, repo: task.repo },
    quality: {
      pass: true,
      expected_anchors: { missed_anchors: [] },
      expected_files: { missed_anchors: [] },
      expected_symbols: { missed_anchors: [] },
      expected_claims: { missed_anchors: [] },
      citation_coverage: { missed_anchors: [] },
      material_factual_errors: { found: 0, found_anchors: [] },
      unsupported_proof_claims: { found: 0, found_claims: [] },
      forbidden_claims: { found: 0, found_anchors: [] },
    },
  };
  const rows = [
    { ...common, benchmark_run_id: "without", arm: "without_codestory" },
    {
      ...common,
      benchmark_run_id: "published",
      arm: "published_0_17_5",
      comparator_reuse_provenance: { source_ledger_sha256: "4".repeat(64) },
      exact_candidate_timing: { cold_ms: 1, warm_ms: 100, incremental_ms: 2, all_in_ms: 100 },
    },
    {
      ...common,
      benchmark_run_id: "candidate",
      arm: "candidate_0_18",
      status: "pass",
      quality: {
        ...common.quality,
        pass: false,
        expected_anchors: { missed_anchors: ["client.dart", "transport claim"] },
        expected_files: { missed_anchors: ["client.dart"] },
        expected_claims: { missed_anchors: ["transport claim"] },
        citation_coverage: { missed_anchors: ["client.dart"] },
      },
      codestory_harness_prelude: {
        wall_ms: 20,
        stdout_path: packetPath,
        packet_schema_version: 3,
        packet_projection_kind: "complete",
        packet_drill_continuation: false,
        packet_evidence_count: 1,
        packet_gap_count: 1,
        stdout_bytes: 512,
      },
      source_cli_identity: {
        source_commit: "5".repeat(40),
        source_tree: "6".repeat(40),
        cli_sha256: "7".repeat(64),
      },
      exact_candidate_timing: { cold_ms: 3, warm_ms: 100, incremental_ms: 4, all_in_ms: 100 },
    },
  ];
  await writeFile(path.join(runDir, "runs.jsonl"), `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`);
  await writeFile(path.join(runDir, "preparations.jsonl"), `${JSON.stringify({
    kind: "preparation",
    repo: "redis-redis",
    incremental_wall_ms: 21_692,
    incremental_source_mutation: { path: "src/server.c", mutation: "append_one_lf_v1" },
    incremental_retrieval_work_evidence: {
      core_phase_timings: { publish_ms: 7_683 },
      retrieval_phase_timings: [{ phase: "lexical sidecar", elapsed_ms: 1_902 }],
    },
  })}\n`);
}

test("remediation ledger preserves per-arm counts, failure detail, packet evidence, timing, and identities", async () => {
  const runDir = await mkdtemp(path.join(os.tmpdir(), "codestory-remediation-ledger-"));
  try {
    await writeFixture(runDir);
    const ledger = await buildRemediationFailureLedger(runDir, {
      generated_at: "2026-08-30T12:00:00.000Z",
    });
    assert.equal(ledger.contract, "codestory.remediation-failure-ledger/v1");
    assert.equal(ledger.source.artifacts.length, 4);
    assert.ok(ledger.source.artifacts.every((artifact) => /^[0-9a-f]{64}$/.test(artifact.sha256)));
    assert.deepEqual(ledger.tasks[0].pass_counts, {
      no_codestory: { source_arm: "without_codestory", passing_repeats: 1, repeats: 1 },
      published: { source_arm: "published_0_17_5", passing_repeats: 1, repeats: 1 },
      candidate: { source_arm: "candidate_0_18", passing_repeats: 0, repeats: 1 },
    });
    const candidate = ledger.tasks[0].rows.find((row) => row.arm === "candidate_0_18");
    assert.deepEqual(candidate.quality.missing_files, ["client.dart"]);
    assert.deepEqual(candidate.quality.missing_claims, ["transport claim"]);
    assert.deepEqual(candidate.quality.missing_citations, ["client.dart"]);
    assert.equal(candidate.packet.route, "complete");
    assert.equal(candidate.packet.evidence_rows[0].symbol_id, "Client.get");
    assert.equal(candidate.packet.gaps[0].kind, "output_budget_exceeded");
    assert.equal(candidate.packet.byte_count > 0, true);
    assert.equal(candidate.timing.legacy_warm_semantics, "whole_task_wall_ms");
    assert.equal(candidate.timing.incremental_ms, 4);
    const published = ledger.tasks[0].rows.find((row) => row.arm === "published_0_17_5");
    assert.equal(published.comparator.fresh, false);
    assert.equal(published.timing.eligible, false);
    assert.equal(ledger.identities.source_clis[0].source_commit, "5".repeat(40));
    assert.equal(ledger.identities.scorers[0], "1".repeat(64));
    assert.equal(ledger.preparations[0].incremental.wall_ms, 21_692);
  } finally {
    await rm(runDir, { recursive: true, force: true });
  }
});

test("remediation ledger rejects relative packet artifact traversal", async () => {
  const runDir = await mkdtemp(path.join(os.tmpdir(), "codestory-remediation-ledger-"));
  try {
    await writeFixture(runDir, "../outside-packet.json");
    await assert.rejects(
      () => buildRemediationFailureLedger(runDir),
      /packet artifact path escapes the retained run directory/,
    );
  } finally {
    await rm(runDir, { recursive: true, force: true });
  }
});
