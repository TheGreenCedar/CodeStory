#!/usr/bin/env node
import { createHash } from "node:crypto";
import { open, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const MAX_SUMMARY_BYTES = 16 * 1024 * 1024;
const MAX_RUN_LEDGER_BYTES = 256 * 1024 * 1024;
const MAX_PREPARATION_LEDGER_BYTES = 128 * 1024 * 1024;
const MAX_PACKET_BYTES = 1024 * 1024;
const SOURCE_ARTIFACTS = Object.freeze([
  ["summary.json", MAX_SUMMARY_BYTES],
  ["summary.md", MAX_SUMMARY_BYTES],
  ["runs.jsonl", MAX_RUN_LEDGER_BYTES],
  ["preparations.jsonl", MAX_PREPARATION_LEDGER_BYTES],
]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function readRegularFileBounded(filePath, maxBytes, label) {
  const handle = await open(filePath, "r");
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || !Number.isSafeInteger(stat.size) || stat.size < 0 || stat.size > maxBytes) {
      throw new Error(`${label} exceeds its byte bound or is not a regular file`);
    }
    const bytes = await readFile(handle);
    if (bytes.length !== stat.size) {
      throw new Error(`${label} changed while it was read`);
    }
    return bytes;
  } finally {
    await handle.close();
  }
}

function parseJsonl(bytes, label) {
  return bytes.toString("utf8").split(/\r?\n/).flatMap((line, index) => {
    if (!line.trim()) return [];
    try {
      return [JSON.parse(line)];
    } catch (error) {
      throw new Error(`${label} line ${index + 1} is not valid JSON: ${error.message}`);
    }
  });
}

function pathIsInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative));
}

function resolveRetainedPacketPath(runDir, recordedPath) {
  if (typeof recordedPath !== "string" || !recordedPath.trim()) return null;
  if (!path.isAbsolute(recordedPath)) {
    const candidate = path.resolve(runDir, recordedPath);
    if (!pathIsInside(runDir, candidate)) {
      throw new Error("packet artifact path escapes the retained run directory");
    }
    return candidate;
  }
  const candidate = path.resolve(recordedPath);
  if (pathIsInside(runDir, candidate)) return candidate;
  const retainedCopy = path.join(runDir, path.basename(candidate));
  if (!pathIsInside(runDir, retainedCopy)) {
    throw new Error("packet artifact path escapes the retained run directory");
  }
  return retainedCopy;
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function uniqueValues(values) {
  const observed = new Set();
  return values.filter((value) => {
    if (value == null) return false;
    const key = canonical(value);
    if (observed.has(key)) return false;
    observed.add(key);
    return true;
  });
}

function armRole(arm) {
  if (arm === "without_codestory") return "no_codestory";
  if (arm === "candidate_0_18") return "candidate";
  if (String(arm).startsWith("published_")) return "published";
  return null;
}

function qualityProjection(quality) {
  const value = quality ?? {};
  return {
    pass: value.pass === true,
    missing_files: value.missed_anchors?.files ?? value.expected_files?.missed_anchors ?? [],
    missing_symbols: value.missed_anchors?.symbols ?? value.expected_symbols?.missed_anchors ?? [],
    missing_claims: value.missed_anchors?.claims ?? value.expected_claims?.missed_anchors ?? [],
    missing_anchors: value.expected_anchors?.missed_anchors ?? [],
    missing_citations: value.citation_coverage?.missed_anchors ?? [],
    factual_errors: {
      found: value.material_factual_errors?.found ?? null,
      anchors: value.material_factual_errors?.found_anchors ?? [],
    },
    unsupported_proof_claims: {
      found: value.unsupported_proof_claims?.found ?? null,
      claims: value.unsupported_proof_claims?.found_claims ?? [],
    },
    forbidden_claims: {
      found: value.forbidden_claims?.found ?? null,
      anchors: value.forbidden_claims?.found_anchors ?? [],
    },
  };
}

function evidenceRow(evidence) {
  return {
    evidence_id: evidence?.identity?.evidence_id ?? evidence?.id ?? null,
    kind: evidence?.kind ?? null,
    path: evidence?.path ?? null,
    symbol_id: evidence?.symbol_id ?? evidence?.symbol?.canonical_id ?? null,
    start_line: evidence?.start_line ?? evidence?.range?.start_line ?? null,
    end_line: evidence?.end_line ?? evidence?.range?.end_line ?? null,
    summary: evidence?.summary ?? null,
  };
}

async function packetProjection(runDir, row) {
  const prelude = row.codestory_harness_prelude;
  if (!prelude?.stdout_path) return null;
  const artifactPath = resolveRetainedPacketPath(runDir, prelude.stdout_path);
  const bytes = await readRegularFileBounded(artifactPath, MAX_PACKET_BYTES, "packet artifact");
  let packet;
  try {
    packet = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    throw new Error(`packet artifact is not valid JSON: ${error.message}`);
  }
  return {
    artifact: path.basename(artifactPath),
    sha256: sha256(bytes),
    byte_count: bytes.length,
    schema_version: packet?.schema_version ?? prelude.packet_schema_version ?? null,
    route: packet?.kind ?? prelude.packet_projection_kind ?? prelude.packet_disposition_kind ?? null,
    disposition: packet?.disposition ?? prelude.packet_disposition ?? null,
    retrieval_state: packet?.retrieval?.state ?? packet?.trace?.retrieval_shadow?.retrieval_mode ?? null,
    continuation: {
      used: prelude.packet_drill_continuation === true,
      status: packet?.status ?? null,
      descriptor: packet?.continuation ?? packet?.disposition?.drill ?? null,
    },
    evidence_rows: Array.isArray(packet?.evidence)
      ? packet.evidence.map(evidenceRow)
      : Array.isArray(packet?.support)
        ? packet.support.map(evidenceRow)
        : [],
    gaps: Array.isArray(packet?.gaps) ? packet.gaps : [],
  };
}

function timingProjection(row) {
  const installed = row.installed_agent_timing ?? null;
  const exact = row.exact_candidate_timing ?? null;
  const reused = row.comparator_reuse_provenance != null;
  return {
    eligible: !reused
      && row.comparative_wall_time_eligible !== false
      && row.installed_agent_timing_eligible !== false,
    ineligibility_reason: reused
      ? "reused_comparator_row"
      : row.installed_agent_timing_ineligibility_reason
        ?? (row.comparative_wall_time_eligible === false ? "source_row_marked_ineligible" : null),
    timing_cohort_id: installed?.timing_cohort_id ?? null,
    agent_runner_ms: installed?.agent_runner_ms ?? row.agent_runner_wall_ms ?? null,
    packet_prelude_ms: row.codestory_harness_prelude?.wall_ms
      ?? row.baseline_harness_prelude?.wall_ms
      ?? 0,
    time_to_first_packet_ms: installed?.time_to_first_packet_ms
      ?? row.codestory_harness_prelude?.time_to_first_packet_ms
      ?? null,
    continuation_ms: installed?.continuation_ms
      ?? row.codestory_harness_prelude?.continuation_ms
      ?? null,
    time_to_final_packet_ms: installed?.time_to_final_packet_ms
      ?? row.codestory_harness_prelude?.time_to_final_packet_ms
      ?? null,
    whole_task_wall_ms: installed?.whole_task_wall_ms ?? row.wall_ms ?? null,
    cold_ms: exact?.cold_ms ?? null,
    incremental_ms: exact?.incremental_ms ?? null,
    all_in_ms: exact?.all_in_ms ?? null,
    legacy_warm_ms: exact?.warm_ms ?? null,
    legacy_warm_semantics: exact?.warm_ms == null ? null : "whole_task_wall_ms",
  };
}

async function rowProjection(runDir, row) {
  const reused = row.comparator_reuse_provenance != null;
  return {
    benchmark_run_id: row.benchmark_run_id ?? null,
    arm: row.arm,
    repeat: row.repeat,
    status: row.status,
    quality: qualityProjection(row.quality),
    comparator: {
      fresh: !reused,
      reused,
      provenance: row.comparator_reuse_provenance ?? null,
    },
    packet: await packetProjection(runDir, row),
    timing: timingProjection(row),
    identities: {
      runner: row.runner ?? null,
      model: row.model ?? null,
      package: row.package_identity ?? null,
      source_cli: row.source_cli_identity ?? null,
      benchmark_contract_fingerprint: row.benchmark_contract?.compatibility_fingerprint ?? null,
      scorer_sha256: row.benchmark_contract?.scorer_hash ?? null,
      task_manifest_sha256: row.benchmark_contract?.task_manifest_hash ?? null,
    },
  };
}

function passCount(rows, role) {
  const selected = rows.filter((row) => armRole(row.arm) === role);
  const sourceArms = [...new Set(selected.map((row) => row.arm))];
  if (sourceArms.length !== 1) {
    throw new Error(`task does not contain exactly one ${role} source arm`);
  }
  return {
    source_arm: sourceArms[0],
    passing_repeats: selected.filter((row) => row.status === "pass" && row.quality?.pass === true).length,
    repeats: selected.length,
  };
}

function preparationProjection(row) {
  return {
    repo: row.repo ?? null,
    arm: row.arm ?? null,
    project: row.project ?? null,
    package_identity: row.package_identity ?? null,
    source_cli_identity: row.source_cli_identity ?? null,
    cold: {
      wall_ms: row.preparation_wall_ms ?? null,
      retrieval_index_ms: row.retrieval_index_wall_ms ?? null,
      retrieval_work: row.cold_retrieval_work_evidence ?? null,
    },
    incremental: {
      wall_ms: row.incremental_wall_ms ?? null,
      status: row.incremental_status ?? null,
      source_mutation: row.incremental_source_mutation ?? null,
      work: row.incremental_retrieval_work_evidence ?? null,
    },
  };
}

export async function buildRemediationFailureLedger(sourceRunDir, options = {}) {
  const runDir = path.resolve(sourceRunDir);
  const sourceBytes = new Map();
  for (const [name, maxBytes] of SOURCE_ARTIFACTS) {
    sourceBytes.set(name, await readRegularFileBounded(path.join(runDir, name), maxBytes, name));
  }
  const summary = JSON.parse(sourceBytes.get("summary.json").toString("utf8"));
  const rows = parseJsonl(sourceBytes.get("runs.jsonl"), "runs.jsonl");
  const preparationRows = parseJsonl(sourceBytes.get("preparations.jsonl"), "preparations.jsonl")
    .filter((row) => row.kind === "preparation");
  const taskIds = uniqueValues([
    ...(summary.tasks ?? []).map((task) => task.id),
    ...rows.map((row) => row.task_id),
  ]);
  const tasks = [];
  for (const taskId of taskIds) {
    const taskRows = rows.filter((row) => row.task_id === taskId);
    const summaryTask = (summary.tasks ?? []).find((task) => task.id === taskId) ?? {};
    tasks.push({
      task_id: taskId,
      repo: summaryTask.repo ?? taskRows[0]?.repo ?? null,
      task_class: summaryTask.task_class ?? taskRows[0]?.task_class ?? null,
      pass_counts: {
        no_codestory: passCount(taskRows, "no_codestory"),
        published: passCount(taskRows, "published"),
        candidate: passCount(taskRows, "candidate"),
      },
      rows: await Promise.all(taskRows.map((row) => rowProjection(runDir, row))),
    });
  }
  const flattenedPreparations = preparationRows.flatMap((row) => {
    if (!row.arm_preparations || typeof row.arm_preparations !== "object") return [row];
    return Object.entries(row.arm_preparations).map(([arm, preparation]) => ({
      ...preparation,
      repo: preparation.repo ?? row.repo,
      arm,
    }));
  });
  return {
    contract: "codestory.remediation-failure-ledger/v1",
    generated_at: options.generated_at ?? new Date().toISOString(),
    source: {
      run_dir: runDir,
      artifacts: SOURCE_ARTIFACTS.map(([name]) => ({
        name,
        sha256: sha256(sourceBytes.get(name)),
        byte_length: sourceBytes.get(name).length,
      })),
    },
    identities: {
      packages: uniqueValues([
        ...rows.map((row) => row.package_identity),
        ...flattenedPreparations.map((row) => row.package_identity),
      ]),
      source_clis: uniqueValues([
        ...rows.map((row) => row.source_cli_identity),
        ...flattenedPreparations.map((row) => row.source_cli_identity),
      ]),
      tasks: uniqueValues(rows.map((row) => row.task_manifest_snapshot)),
      models: uniqueValues(rows.map((row) => row.model ?? summary.model)),
      runners: uniqueValues(rows.map((row) => row.runner ?? summary.runner)),
      hosts: uniqueValues([summary.host_class]),
      scorers: uniqueValues(rows.map((row) => row.benchmark_contract?.scorer_hash)),
    },
    tasks,
    preparations: flattenedPreparations.map(preparationProjection),
  };
}

export async function writeRemediationFailureLedger(sourceRunDir, outputPath, options = {}) {
  const ledger = await buildRemediationFailureLedger(sourceRunDir, options);
  await writeFile(outputPath, `${JSON.stringify(ledger, null, 2)}\n`, { flag: "wx", mode: 0o600 });
  return ledger;
}

function parseCliArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (!["--run-dir", "--out"].includes(flag) || index + 1 >= argv.length) {
      throw new Error("usage: codestory-remediation-ledger.mjs --run-dir <path> --out <path>");
    }
    values[flag.slice(2).replaceAll("-", "_")] = argv[index + 1];
    index += 1;
  }
  if (!values.run_dir || !values.out) {
    throw new Error("usage: codestory-remediation-ledger.mjs --run-dir <path> --out <path>");
  }
  return values;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  try {
    const args = parseCliArgs(process.argv.slice(2));
    const ledger = await writeRemediationFailureLedger(args.run_dir, path.resolve(args.out));
    process.stdout.write(`${ledger.contract}\n`);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
