#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs as parseNodeArgs } from "node:util";

import {
  codeStoryInvocationsFromCommand,
  directBenchmarkCliInvocation,
} from "./lib/evidence-compiler-ablation.mjs";

const CONTROL_ROW_AUDIT_CONTRACT = "codestory.control-row-audit/v1";
const CONTROL_ERRATUM_CONTRACT = "codestory.control-row-erratum/v1";
const CONTROL_ARMS = Object.freeze([
  "exact_identity_source",
  "exact_plus_relations",
]);
const RELATION_OPERATIONS = new Set([
  "references",
  "trail",
  "callers",
  "callees",
  "trace",
  "neighbors",
  "shortest_path",
  "query_subgraph",
]);
const SOURCE_BEARING_OPERATIONS = new Set([
  "search",
  "symbol",
  "symbols",
  "get_node",
  "definition",
  "snippet",
  "context",
  ...RELATION_OPERATIONS,
]);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function parseArgs(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      "run-dir": { type: "string" },
      "out-dir": { type: "string" },
    },
  });
  if (!values["run-dir"] || !values["out-dir"]) {
    throw new Error(
      "usage: codestory-control-row-audit.mjs --run-dir <immutable-receipt-dir> --out-dir <new-dir>",
    );
  }
  return {
    runDir: path.resolve(values["run-dir"]),
    outDir: path.resolve(values["out-dir"]),
  };
}

function parseJsonlWithRawLines(bytes, label) {
  return bytes.toString("utf8").split(/\r?\n/u).flatMap((line, index) => {
    if (!line) return [];
    try {
      return [{ value: JSON.parse(line), raw: line, line: index + 1 }];
    } catch (error) {
      throw new Error(`${label} line ${index + 1} is invalid JSON: ${error.message}`);
    }
  });
}

function artifactPath(runDir, storedPath) {
  if (typeof storedPath !== "string" || !storedPath) return null;
  const preserved = path.join(runDir, path.basename(storedPath));
  return existsSync(preserved) ? preserved : null;
}

async function readArtifact(runDir, storedPath, label) {
  const resolved = artifactPath(runDir, storedPath);
  if (!resolved) {
    throw new Error(`${label} artifact is missing: ${storedPath ?? "unrecorded"}`);
  }
  const bytes = await readFile(resolved);
  return {
    bytes,
    preserved_name: path.basename(resolved),
    sha256: sha256(bytes),
  };
}

function commandEvents(events) {
  return events.flatMap((event, eventIndex) => {
    const item = event?.item;
    if (event?.type !== "item.completed" || item?.type !== "command_execution") {
      return [];
    }
    return [{
      id: item.id ?? null,
      event_index: eventIndex,
      command: String(item.command ?? ""),
      output: String(item.aggregated_output ?? ""),
      exit_code: Number.isInteger(item.exit_code) ? item.exit_code : null,
      status: item.status ?? null,
    }];
  });
}

function parseStructuredError(output) {
  const text = String(output ?? "").trim();
  if (!text) return null;
  try {
    const parsed = JSON.parse(text);
    if (parsed?.error && typeof parsed.error === "object") {
      return {
        code: typeof parsed.error.code === "string" ? parsed.error.code : "unknown",
        message: typeof parsed.error.message === "string" ? parsed.error.message : null,
      };
    }
  } catch {
    // Human CLI errors are parsed below.
  }
  const match = text.match(/^Error:\s*([a-z][a-z0-9_-]*):\s*([^\n]*)/iu);
  return match ? {
    status: "unavailable",
    observed_text_error: { code: match[1], message: match[2] },
    output_sha256: sha256(Buffer.from(text, "utf8")),
  } : null;
}

function commandReceipt(command) {
  const invocations = codeStoryInvocationsFromCommand(command.command);
  if (!invocations.length) return null;
  const direct = directBenchmarkCliInvocation(command.command);
  const structuredError = command.exit_code === 0
    ? null
    : parseStructuredError(command.output);
  return {
    event_index: command.event_index,
    command: command.command,
    operation: direct?.operation ?? invocations[0]?.operation ?? null,
    arguments: direct?.args ?? null,
    checksum_bound: invocations.every((entry) => entry.checksum_bound === true),
    exit_status: command.exit_code,
    status: command.status,
    output_bytes: Buffer.byteLength(command.output, "utf8"),
    output_sha256: sha256(Buffer.from(command.output, "utf8")),
    error: command.exit_code === 0
      ? null
      : structuredError ?? {
          status: "unavailable",
          output_sha256: sha256(Buffer.from(command.output, "utf8")),
        },
  };
}

function sourceReadIndex(row, commands) {
  const reads = row.transcript_analysis?.direct_source_reads;
  if (!Array.isArray(reads) || row.transcript_analysis?.direct_source_reads_total !== reads.length) {
    throw new Error(`${row.benchmark_run_id}: source-read telemetry is missing or inconsistent`);
  }
  const commandIds = new Set(commands.map((command) => command.id));
  const byCommand = new Map();
  for (const read of reads) {
    const id = read?.command_id;
    if (typeof id !== "string" || !commandIds.has(id)) {
      throw new Error(`${row.benchmark_run_id}: source-read telemetry cannot join command ${id}`);
    }
    const current = byCommand.get(id) ?? [];
    current.push(read);
    byCommand.set(id, current);
  }
  return byCommand;
}

function sourceExposure(command, sourceReads, codeStoryReceipt) {
  const nativeReads = sourceReads.get(command.id) ?? [];
  const codeStorySource = codeStoryReceipt?.exit_status === 0 &&
    SOURCE_BEARING_OPERATIONS.has(codeStoryReceipt.operation);
  return {
    read_count: nativeReads.length,
    bytes: nativeReads.length || codeStorySource
      ? Buffer.byteLength(command.output, "utf8")
      : 0,
  };
}

function requiredOperationValidity(arm, commands) {
  const first = commands[0] ?? null;
  const firstSearchValid = first?.operation === "search" && first?.exit_status === 0;
  const relationValid = arm !== "exact_plus_relations" ||
    commands.some((entry) => entry.exit_status === 0 && RELATION_OPERATIONS.has(entry.operation));
  const reasons = [];
  if (!firstSearchValid) reasons.push("first required exact search did not succeed");
  if (!relationValid) reasons.push("no required explicit relation operation succeeded");
  return {
    valid: reasons.length === 0,
    reasons,
  };
}

function telemetryValidity(row) {
  const violations = row.builder_ablation?.operation_violations;
  const reasons = [];
  if (!Array.isArray(violations)) {
    reasons.push("operation telemetry is missing");
  } else {
    reasons.push(...violations);
  }
  if (row.builder_ablation?.first_codestory_required !== true) {
    reasons.push("first-CodeStory requirement was not recorded");
  }
  if (row.builder_ablation?.first_codestory_pass !== true) {
    reasons.push("first-CodeStory requirement did not pass");
  }
  if (row.transcript_analysis?.codestory_was_first_repository_context_action !== true) {
    reasons.push("CodeStory was not the first valid repository-context action");
  }
  return {
    valid: reasons.length === 0,
    reasons: [...new Set(reasons)],
  };
}

async function auditControlRow({
  row,
  rawRow,
  runDir,
  sourceCommit,
  sourceTree,
  sourceRunsSha256,
}) {
  const stdout = await readArtifact(runDir, row.stdout_path, `${row.benchmark_run_id} stdout`);
  const stderr = await readArtifact(runDir, row.stderr_path, `${row.benchmark_run_id} stderr`);
  const transcript = parseJsonlWithRawLines(stdout.bytes, stdout.preserved_name)
    .map((entry) => entry.value);
  const commands = commandEvents(transcript);
  const sourceReads = sourceReadIndex(row, commands);
  const codeStoryByEvent = new Map();
  for (const command of commands) {
    const receipt = commandReceipt(command);
    if (receipt) codeStoryByEvent.set(command.event_index, receipt);
  }
  const codeStoryCommands = [...codeStoryByEvent.values()];
  const firstFailure = codeStoryCommands.find((entry) => entry.exit_status !== 0) ?? null;
  const failureIndex = firstFailure?.event_index ?? Number.POSITIVE_INFINITY;
  const beforeFailure = commands.filter((entry) => entry.event_index < failureIndex);
  const sourceBeforeFailure = beforeFailure.reduce((total, command) => {
    const exposure = sourceExposure(command, sourceReads, codeStoryByEvent.get(command.event_index));
    return {
      read_count: total.read_count + exposure.read_count,
      bytes: total.bytes + exposure.bytes,
    };
  }, { read_count: 0, bytes: 0 });
  const fallbackCommands = commands.filter((entry) => entry.event_index > failureIndex);
  const fallbackActions = fallbackCommands.map((command) => {
    const codeStory = codeStoryByEvent.get(command.event_index) ?? null;
    const reads = sourceReads.get(command.id) ?? [];
    return {
      event_index: command.event_index,
      kind: codeStory ? "codestory" : "native",
      operation: codeStory?.operation ?? null,
      command: command.command,
      exit_status: command.exit_code,
      output_bytes: Buffer.byteLength(command.output, "utf8"),
      source_read_count: reads.length,
      source_paths: reads.map((read) => read.path),
    };
  });
  const nativeFallbackWithSource = fallbackActions.filter(
    (action) => action.kind === "native" && action.source_read_count > 0,
  );
  const requiredOperation = requiredOperationValidity(row.arm, codeStoryCommands);
  const telemetry = telemetryValidity(row);
  return {
    contract: CONTROL_ROW_AUDIT_CONTRACT,
    task: row.task_id,
    arm: row.arm,
    repeat: row.repeat,
    source_commit: sourceCommit,
    source_tree: sourceTree,
    row_identity: row.benchmark_run_id,
    input_binding: {
      source_runs_sha256: sourceRunsSha256,
      runs_jsonl_line: rawRow.line,
      row_sha256: sha256(Buffer.from(rawRow.raw, "utf8")),
      stdout: {
        preserved_name: stdout.preserved_name,
        sha256: stdout.sha256,
      },
      stderr: {
        preserved_name: stderr.preserved_name,
        sha256: stderr.sha256,
      },
    },
    attempted_codestory_commands: codeStoryCommands,
    first_failed_codestory_command: firstFailure,
    source_exposed_before_failure: sourceBeforeFailure,
    source_byte_measurement: "whole command output containing source; includes formatting; combined reads counted once",
    subsequent_actions: fallbackActions,
    native_fallback: {
      source_read_count: nativeFallbackWithSource.reduce(
        (sum, action) => sum + action.source_read_count,
        0,
      ),
      source_bytes: nativeFallbackWithSource.reduce(
        (sum, action) => sum + action.output_bytes,
        0,
      ),
    },
    required_operation: requiredOperation,
    telemetry,
    intervention_valid: requiredOperation.valid && telemetry.valid,
  };
}

function auditJsonl(audits) {
  return `${audits.map((audit) => JSON.stringify(audit)).join("\n")}\n`;
}

function completeTaskComparison(audits, task, candidateArm, controlArm) {
  const auditedArms = [candidateArm, controlArm].filter((arm) => CONTROL_ARMS.includes(arm));
  const expected = auditedArms.flatMap((arm) =>
    [1, 2, 3].map((repeat) => `${task}\t${arm}\t${repeat}`)
  );
  const byKey = new Map(audits.map((row) => [
    `${row.task}\t${row.arm}\t${row.repeat}`,
    row,
  ]));
  return expected.every((key) => byKey.get(key)?.intervention_valid === true);
}

function originalMetricSnapshot(builderReceipt, comparison) {
  if (
    comparison.candidate_arm === "packet_semantic_off" &&
    comparison.control_arm === "exact_plus_relations"
  ) {
    const source = builderReceipt.packet_acceptance ?? {};
    return {
      task_quality_deltas: source.task_quality_deltas ?? null,
      mean_task_pass_difference: source.mean_task_pass_difference ?? null,
      exploratory_repository_context_action_ratio:
        source.exploratory_repository_context_action_ratio ?? null,
      whole_task_wall_ratio: source.whole_task_wall_ratio ?? null,
      input_context_ratio: source.input_context_ratio ?? null,
      packet_only_critical_claims: source.packet_only_critical_claims ?? null,
    };
  }
  const source = builderReceipt.marginal_value?.graph_relations ?? {};
  return {
    mean_task_pass_difference: source.mean_task_pass_difference ?? null,
    exploratory_repository_context_action_ratio:
      source.exploratory_repository_context_action_ratio ?? null,
    whole_task_wall_ratio: source.whole_task_wall_ratio ?? null,
    input_context_ratio: source.input_context_ratio ?? null,
    candidate_only_critical_claims: source.candidate_only_critical_claims ?? null,
  };
}

function buildErratum(audits, builderReceipt, sourceRunsSha256) {
  const serializedAudit = auditJsonl(audits);
  const auditSha256 = sha256(Buffer.from(serializedAudit, "utf8"));
  const tasks = [...new Set(audits.map((row) => row.task))].sort();
  const comparisons = [
    { candidate_arm: "packet_semantic_off", control_arm: "exact_plus_relations" },
    { candidate_arm: "exact_plus_relations", control_arm: "exact_identity_source" },
  ].map((comparison) => {
    const taskValidity = tasks.map((task) => ({
      task,
      valid: completeTaskComparison(
        audits,
        task,
        comparison.candidate_arm,
        comparison.control_arm,
      ),
    }));
    return {
      ...comparison,
      task_comparisons: taskValidity,
      aggregate_valid: taskValidity.every((entry) => entry.valid),
      original_metrics: originalMetricSnapshot(builderReceipt, comparison),
    };
  });
  const invalidComparisons = comparisons.filter((comparison) => !comparison.aggregate_valid);
  const graphComparison = comparisons.find(
    (comparison) => comparison.candidate_arm === "exact_plus_relations",
  );
  return {
    contract: CONTROL_ERRATUM_CONTRACT,
    source_receipt: {
      source_commit: builderReceipt.source_commit ?? null,
      source_tree: builderReceipt.source_tree ?? null,
      runs_sha256: sourceRunsSha256,
      original_packet_decision: builderReceipt.packet_decision ?? null,
      original_packet_decision_status: "preserved",
    },
    control_row_audit: {
      contract: CONTROL_ROW_AUDIT_CONTRACT,
      sha256: auditSha256,
      rows: audits.length,
      valid_rows: audits.filter((row) => row.intervention_valid).length,
      invalid_rows: audits.filter((row) => !row.intervention_valid).map((row) => row.row_identity),
    },
    invalidated_comparisons: invalidComparisons,
    invalidated_layer_decisions: graphComparison?.aggregate_valid === false
      ? [{
          layer: "graph_relations",
          original_decision: builderReceipt.default_layer_decisions?.graph_relations ?? null,
          reason: "the exact-plus-relations intervention lacked complete valid control rows",
        }]
      : [],
    retained_authority: {
      absolute_row_outputs: "descriptive_only",
      packet_decision: "preserved_for_the_evaluated_architecture",
      dense_semantic_comparison: "not_invalidated_by_this_control_audit",
      new_architecture_revision_token: "not_created_by_this_erratum",
    },
  };
}

function erratumMarkdown(erratum) {
  const invalidRows = erratum.control_row_audit.invalid_rows.length;
  const comparisons = erratum.invalidated_comparisons.map((comparison) =>
    `- \`${comparison.candidate_arm}\` versus \`${comparison.control_arm}\`: ` +
    `${comparison.task_comparisons.filter((entry) => !entry.valid).length} task comparisons and ` +
    "the corresponding aggregate metrics are withdrawn.",
  ).join("\n");
  return `# Evidence compiler control erratum

The original \`${erratum.source_receipt.original_packet_decision}\` decision remains attached to the evaluated architecture and receipt. This erratum does not grant it another revision.

The row audit found ${invalidRows} invalid control rows. Their answer text, timings, token counts, and fallback reads remain descriptions of those sessions, but they cannot estimate the named CodeStory interventions.

## Withdrawn comparisons

${comparisons || "- None."}

The dense semantic packet comparison is not invalidated by this control defect. Any separately recorded packet telemetry or continuation violation also remains in force.

Audit SHA-256: \`${erratum.control_row_audit.sha256}\`
Source runs SHA-256: \`${erratum.source_receipt.runs_sha256}\`
`;
}

async function buildControlAudit({ runDir, requireFrozenShape = true }) {
  const runsPath = path.join(runDir, "runs.jsonl");
  const builderPath = path.join(runDir, "builder-ablation.json");
  if (!existsSync(runsPath) || !existsSync(builderPath)) {
    throw new Error("run directory must contain runs.jsonl and builder-ablation.json");
  }
  const runsBytes = await readFile(runsPath);
  const rawRows = parseJsonlWithRawLines(runsBytes, "runs.jsonl");
  const builderReceipt = JSON.parse(await readFile(builderPath, "utf8"));
  const sourceRunsSha256 = sha256(runsBytes);
  if (builderReceipt.source_runs_sha256 !== sourceRunsSha256) {
    throw new Error("runs.jsonl SHA256 does not match the preserved builder receipt");
  }
  const controlRows = rawRows.filter((entry) => CONTROL_ARMS.includes(entry.value?.arm));
  if (requireFrozenShape) {
    const tasks = new Set(controlRows.map((entry) => entry.value?.task_id));
    const keys = new Set(controlRows.map((entry) =>
      `${entry.value?.task_id}\t${entry.value?.arm}\t${entry.value?.repeat}`
    ));
    const complete = [...tasks].every((task) => CONTROL_ARMS.every((arm) =>
      [1, 2, 3].every((repeat) => keys.has(`${task}\t${arm}\t${repeat}`))
    ));
    if (controlRows.length !== 48 || tasks.size !== 8 || keys.size !== 48 || !complete) {
      throw new Error(
        `expected 48 unique control rows across 8 tasks; received ${controlRows.length} rows, ${tasks.size} tasks, and ${keys.size} keys`,
      );
    }
  }
  const audits = [];
  for (const rawRow of controlRows) {
    audits.push(await auditControlRow({
      row: rawRow.value,
      rawRow,
      runDir,
      sourceCommit: builderReceipt.source_commit ?? null,
      sourceTree: builderReceipt.source_tree ?? null,
      sourceRunsSha256,
    }));
  }
  audits.sort((left, right) =>
    left.task.localeCompare(right.task) ||
    left.arm.localeCompare(right.arm) ||
    left.repeat - right.repeat
  );
  const erratum = buildErratum(audits, builderReceipt, sourceRunsSha256);
  erratum.source_receipt.builder_receipt_sha256 = sha256(await readFile(builderPath));
  return { audits, erratum, sourceRunsSha256 };
}

async function writeControlAudit({ runDir, outDir }) {
  if (existsSync(outDir)) {
    throw new Error(`refusing to replace existing audit directory: ${outDir}`);
  }
  const result = await buildControlAudit({ runDir, requireFrozenShape: true });
  await mkdir(outDir, { recursive: false });
  const auditText = auditJsonl(result.audits);
  const erratumJson = `${JSON.stringify(result.erratum, null, 2)}\n`;
  const erratumMd = erratumMarkdown(result.erratum);
  const artifacts = {
    "control-row-audit.jsonl": auditText,
    "control-row-erratum.json": erratumJson,
    "control-row-erratum.md": erratumMd,
  };
  for (const [name, value] of Object.entries(artifacts)) {
    await writeFile(path.join(outDir, name), value, "utf8");
  }
  const checksums = Object.fromEntries(Object.entries(artifacts).map(([name, value]) => [
    name,
    sha256(Buffer.from(value, "utf8")),
  ]));
  await writeFile(
    path.join(outDir, "SHA256SUMS.json"),
    `${JSON.stringify({
      contract: "codestory.control-row-audit-checksums/v1",
      source_runs_sha256: result.sourceRunsSha256,
      artifacts: checksums,
    }, null, 2)}\n`,
    "utf8",
  );
  return { ...result, checksums };
}

export {
  CONTROL_ERRATUM_CONTRACT,
  CONTROL_ROW_AUDIT_CONTRACT,
  auditControlRow,
  auditJsonl,
  buildControlAudit,
  buildErratum,
  erratumMarkdown,
  parseArgs,
  writeControlAudit,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  writeControlAudit(parseArgs(process.argv.slice(2))).then((result) => {
    console.log(
      `audited ${result.audits.length} rows; ` +
      `${result.erratum.control_row_audit.invalid_rows.length} invalid`,
    );
  }).catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
