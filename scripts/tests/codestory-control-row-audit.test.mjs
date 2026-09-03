import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  auditControlRow,
  buildControlAudit,
  buildErratum,
} from "../codestory-control-row-audit.mjs";

const hash = (value) => createHash("sha256").update(value).digest("hex");
const receipt = {
  source_commit: "a".repeat(40),
  source_tree: "b".repeat(40),
  packet_decision: "stop",
  packet_acceptance: { mean_task_pass_difference: -0.2 },
  marginal_value: { graph_relations: { mean_task_pass_difference: 0.1 } },
  default_layer_decisions: { graph_relations: "disable_default" },
};

async function fixture() {
  const runDir = await mkdtemp(path.join(os.tmpdir(), "codestory-control-audit-"));
  const events = [
    {
      type: "item.completed",
      item: {
        id: "search",
        type: "command_execution",
        command: '$CODESTORY_CLI search --project /repo --query Alpha --repo-text off',
        aggregated_output: "Error: internal: Operation not permitted\n",
        exit_code: 1,
        status: "failed",
      },
    },
    {
      type: "item.completed",
      item: {
        id: "read",
        type: "command_execution",
        command: "sed -n '1,2p' src/alpha.rs src/beta.rs",
        aggregated_output: "fn alpha() { beta(); }\nfn beta() {}\n",
        exit_code: 0,
        status: "completed",
      },
    },
  ];
  const stdout = events.map((event) => JSON.stringify(event)).join("\n") + "\n";
  await writeFile(path.join(runDir, "row.stdout.jsonl"), stdout);
  await writeFile(path.join(runDir, "row.stderr.txt"), "");
  const row = {
    benchmark_run_id: "case-exact-1",
    task_id: "case",
    arm: "exact_identity_source",
    repeat: 1,
    stdout_path: "/old/location/row.stdout.jsonl",
    stderr_path: "/old/location/row.stderr.txt",
    builder_ablation: {
      first_codestory_required: true,
      first_codestory_pass: false,
      operation_violations: ["first required operation failed"],
    },
    transcript_analysis: {
      codestory_was_first_repository_context_action: false,
      direct_source_reads_total: 2,
      direct_source_reads: [
        { command_id: "read", path: "src/alpha.rs", event_index: 1 },
        { command_id: "read", path: "src/beta.rs", event_index: 1 },
      ],
    },
  };
  const raw = JSON.stringify(row);
  const params = {
    row,
    rawRow: { raw, line: 1 },
    runDir,
    sourceCommit: receipt.source_commit,
    sourceTree: receipt.source_tree,
    sourceRunsSha256: hash(raw + "\n"),
  };
  return { runDir, events, stdout, row, params };
}

test("control audit binds commands and artifacts and counts composed source output once", async () => {
  const setup = await fixture();
  const audit = await auditControlRow(setup.params);
  assert.equal(audit.intervention_valid, false);
  assert.equal(audit.first_failed_codestory_command.exit_status, 1);
  assert.deepEqual(audit.first_failed_codestory_command.arguments, [
    "--project", "/repo", "--query", "Alpha", "--repo-text", "off",
  ]);
  assert.equal(audit.input_binding.stdout.sha256, hash(setup.stdout));
  assert.equal(audit.input_binding.stderr.sha256, hash(""));
  assert.equal(audit.input_binding.row_sha256, hash(setup.params.rawRow.raw));
  assert.deepEqual(audit.source_exposed_before_failure, { read_count: 0, bytes: 0 });
  assert.equal(audit.native_fallback.source_read_count, 2);
  assert.equal(audit.native_fallback.source_bytes,
    Buffer.byteLength(setup.events[1].item.aggregated_output));
});

test("audit uses the preserved artifact even when the former mutable path still exists", async () => {
  const setup = await fixture();
  const mutableDir = await mkdtemp(path.join(os.tmpdir(), "codestory-audit-mutable-"));
  const formerPath = path.join(mutableDir, "row.stdout.jsonl");
  await writeFile(formerPath, '{"type":"unrelated"}\n');
  setup.params.row.stdout_path = formerPath;
  const audit = await auditControlRow(setup.params);
  assert.equal(audit.input_binding.stdout.sha256, hash(setup.stdout));
  assert.equal(audit.first_failed_codestory_command.exit_status, 1);
});

test("missing or unjoinable source-read telemetry is never reported as zero measured reads", async () => {
  for (const mutate of [
    (row) => { delete row.transcript_analysis.direct_source_reads; },
    (row) => { row.transcript_analysis.direct_source_reads[0].command_id = "absent"; },
    (row) => { row.transcript_analysis.direct_source_reads[0].event_index = 0; },
    (row) => { delete row.transcript_analysis.direct_source_reads[0].path; },
  ]) {
    const setup = await fixture();
    mutate(setup.row);
    await assert.rejects(() => auditControlRow(setup.params), /source.read telemetry/i);
  }
});

test("duplicate completed command identities cannot multiply measured exposure", async () => {
  const setup = await fixture();
  setup.events.push(setup.events[1]);
  await writeFile(path.join(setup.runDir, "row.stdout.jsonl"), setup.events.map(JSON.stringify).join("\n") + "\n");
  await assert.rejects(() => auditControlRow(setup.params), /source.read telemetry/);
});

test("source telemetry may bind the unique start event paired with its completion", async () => {
  const setup = await fixture();
  setup.events.splice(1, 0, { type: "item.started", item: { ...setup.events[1].item, exit_code: null } });
  await writeFile(path.join(setup.runDir, "row.stdout.jsonl"), setup.events.map(JSON.stringify).join("\n") + "\n");
  const audit = await auditControlRow(setup.params);
  assert.equal(audit.native_fallback.source_read_count, 2);
});

test("a failed CodeStory operation remains invalid after successful native fallback", async () => {
  const setup = await fixture();
  setup.events[0].item.exit_code = 0;
  setup.events[0].item.aggregated_output = "source";
  setup.events.push({ type: "item.completed", item: {
    id: "snippet", type: "command_execution", command: "$CODESTORY_CLI snippet --project /repo --id node",
    exit_code: 1, aggregated_output: "failed",
  } });
  setup.row.builder_ablation.first_codestory_pass = true;
  setup.row.builder_ablation.operation_violations = [];
  setup.row.transcript_analysis.codestory_was_first_repository_context_action = true;
  await writeFile(path.join(setup.runDir, "row.stdout.jsonl"), setup.events.map(JSON.stringify).join("\n") + "\n");
  const audit = await auditControlRow(setup.params);
  assert.equal(audit.intervention_valid, false);
  assert.match(audit.required_operation.reasons.join(" "), /snippet did not succeed/);
});

test("erratum withdraws the whole aggregate for one invalid repeat and preserves the old stop", () => {
  const audits = ["one", "two"].flatMap((task) =>
    ["exact_identity_source", "exact_plus_relations"].flatMap((arm) =>
      [1, 2, 3].map((repeat) => ({
        task, arm, repeat,
        row_identity: `${task}/${arm}/${repeat}`,
        intervention_valid: true,
      }))
    )
  );
  assert.equal(buildErratum(audits, receipt, "c".repeat(64)).invalidated_comparisons.length, 0);
  audits.find((row) => row.task === "two" && row.arm === "exact_plus_relations").intervention_valid = false;
  const erratum = buildErratum(audits, receipt, "c".repeat(64));
  assert.equal(erratum.invalidated_comparisons.length, 2);
  for (const comparison of erratum.invalidated_comparisons) {
    assert.equal(comparison.aggregate_valid, false);
    assert.deepEqual(comparison.task_comparisons, [
      { task: "one", valid: true }, { task: "two", valid: false },
    ]);
  }
  assert.equal(erratum.source_receipt.original_packet_decision, "stop");
  assert.equal(erratum.source_receipt.original_packet_decision_status, "preserved");
  const changed = structuredClone(audits);
  changed[0].observed_bytes = 1;
  const next = buildErratum(changed, receipt, "c".repeat(64));
  assert.notEqual(next.control_row_audit.sha256, erratum.control_row_audit.sha256);
  assert.notEqual(hash(JSON.stringify(next)), hash(JSON.stringify(erratum)));
});

test("audit rejects a receipt whose runs digest no longer matches", async () => {
  const setup = await fixture();
  await writeFile(path.join(setup.runDir, "runs.jsonl"), setup.params.rawRow.raw + "\n");
  await writeFile(path.join(setup.runDir, "builder-ablation.json"), JSON.stringify({
    ...receipt, source_runs_sha256: "0".repeat(64),
  }));
  await assert.rejects(
    () => buildControlAudit({ runDir: setup.runDir, requireFrozenShape: false }),
    /runs.*digest|runs.*sha256/i,
  );
  assert.equal(await readFile(path.join(setup.runDir, "runs.jsonl"), "utf8"),
    setup.params.rawRow.raw + "\n");
});
