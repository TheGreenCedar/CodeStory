#!/usr/bin/env node

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import {
  cacheProvenanceBlockers,
  repoProvenanceBlockers,
} from "./codestory-evidence-provenance.mjs";

const METRICS = [
  "status_seconds", "local_grounding_seconds", "convergence_seconds",
  "packet_seconds", "search_seconds", "indexing_seconds", "storage_growth_ratio",
];
const SHA = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;
const MACHINE_FINGERPRINT = /^[A-Za-z0-9][A-Za-z0-9._:/+-]{2,199}$/;

function fail(message) { throw new Error(message); }
function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(`${label} must be an object`);
  return value;
}
function text(value, label) {
  if (typeof value !== "string" || value.trim() === "") fail(`${label} must be a non-empty string`);
  return value;
}
function number(value, label) {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) fail(`${label} must be a finite positive number`);
  return value;
}
function fullSha(value, label) {
  const normalized = text(value, label).toLowerCase();
  if (!SHA.test(normalized)) fail(`${label} must be a full 40-character Git SHA`);
  return normalized;
}
function sha256(bytes) { return createHash("sha256").update(bytes).digest("hex"); }
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  }
  return value;
}
function readJson(filePath, label) {
  try { return JSON.parse(readFileSync(filePath, "utf8")); }
  catch (error) { fail(`failed to read ${label} ${filePath}: ${error.message}`); }
}
function fileAttestation(filePath, baseDir) {
  const absolute = path.resolve(baseDir, filePath);
  if (!existsSync(absolute) || !statSync(absolute).isFile()) fail(`artifact does not exist: ${filePath}`);
  const bytes = readFileSync(absolute);
  if (bytes.length === 0) fail(`artifact is empty: ${filePath}`);
  return { path: path.relative(baseDir, absolute).replaceAll(path.sep, "/"), sha256: sha256(bytes), bytes: bytes.length };
}
function git(args, cwd) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) fail(`git ${args.join(" ")} failed: ${result.stderr.trim()}`);
  return result.stdout.trim();
}
function machineFingerprint() {
  const provisioningPath = process.env.CODESTORY_RELEASE_EVIDENCE_PROVISIONING?.trim();
  if (provisioningPath) {
    const provisioning = readJson(provisioningPath, "release-evidence provisioning");
    if (provisioning.schema_version !== 2) fail("provisioning schema_version must be 2");
    const profileId = text(provisioning.profile_id, "provisioning.profile_id");
    const contractSha = text(provisioning.contract_sha256, "provisioning.contract_sha256");
    if (!SHA256.test(contractSha)) fail("provisioning.contract_sha256 must be a SHA-256 digest");
    const fingerprint = text(provisioning.fingerprint, "provisioning.fingerprint");
    if (!MACHINE_FINGERPRINT.test(fingerprint) || fingerprint !== `${profileId}/${contractSha}`) {
      fail("provisioning fingerprint does not match its profile and machine contract");
    }
    const observed = object(provisioning.observed_identity, "provisioning.observed_identity");
    const observedSha = sha256(`${JSON.stringify(canonical(observed))}\n`);
    if (observedSha !== provisioning.observed_identity_sha256) {
      fail("provisioning observed identity attestation changed");
    }
    return fingerprint;
  }
  const cpu = os.cpus()[0]?.model?.trim() ?? "unknown";
  const memoryGiB = Math.round(os.totalmem() / 2 ** 30);
  return `${process.platform}/${process.arch}/${os.cpus().length}/${cpu}/${memoryGiB}GiB`;
}
function profileFrom(document, name, mode, baselineDir) {
  if (document.schema_version !== 2) fail("baseline schema_version must be 2");
  const profile = object(object(document.profiles, "baseline.profiles")[name], `profile ${name}`);
  text(profile.baseline_id, "baseline_id");
  fullSha(profile.commit, "baseline.commit");
  if (profile.git_state !== "clean") fail("baseline git_state must be clean");
  if (profile.status !== "approved") fail("baseline status must be approved");
  if (mode === "release" && profile.release_eligible !== true) fail(`profile ${name} is not release eligible`);
  const identity = object(profile.identity, "baseline.identity");
  for (const key of ["corpus_id", "cache_id", "machine_fingerprint"]) text(identity[key], `baseline.identity.${key}`);
  const approval = object(profile.approval, "baseline.approval");
  for (const key of ["owner", "approved_at", "rationale"]) text(approval[key], `baseline.approval.${key}`);
  if (!DATE.test(approval.approved_at)) fail("baseline approval date must be ISO YYYY-MM-DD");
  if (!Array.isArray(profile.artifacts) || profile.artifacts.length === 0) fail("baseline artifacts must be non-empty");
  for (const artifact of profile.artifacts) {
    const actual = fileAttestation(artifact.path, baselineDir);
    if (actual.sha256 !== artifact.sha256) fail(`baseline artifact hash changed: ${artifact.path}`);
  }
  for (const metric of METRICS) {
    const budget = object(object(profile.metrics, "baseline.metrics")[metric], `baseline metric ${metric}`);
    number(budget.reference, `${metric}.reference`);
    number(budget.threshold, `${metric}.threshold`);
    for (const key of ["unit", "aggregation", "source"]) text(budget[key], `${metric}.${key}`);
    if (budget.direction !== "max") fail(`${metric}.direction must be max`);
    if (metric === "storage_growth_ratio") number(budget.reference_bytes, `${metric}.reference_bytes`);
  }
  const raw = readJson(path.resolve(baselineDir, profile.artifacts[0].path), "baseline raw artifact");
  if (fullSha(raw.commit, "baseline raw commit") !== profile.commit) fail("baseline raw commit does not match profile");
  if (JSON.stringify(raw.evidence_identity) !== JSON.stringify(identity)) fail("baseline raw identity does not match profile");
  for (const metric of METRICS.filter((name) => name !== "storage_growth_ratio")) {
    if (raw.metrics?.[metric] !== profile.metrics[metric].reference) fail(`baseline raw metric ${metric} does not match profile`);
  }
  if (raw.metrics?.storage_bytes !== profile.metrics.storage_growth_ratio.reference_bytes) fail("baseline raw storage bytes do not match profile");
  return profile;
}
function candidateCommit(expectedSha, mode, repoRoot) {
  const expected = fullSha(expectedSha, "expected SHA");
  if (mode === "release") {
    const actual = fullSha(git(["rev-parse", "HEAD"], repoRoot), "Git HEAD");
    if (actual !== expected) fail(`Git HEAD ${actual} does not match expected SHA ${expected}`);
    if (git(["status", "--porcelain"], repoRoot) !== "") fail("release evidence requires a clean Git worktree");
  }
  return expected;
}
function packetMaximum(packet) {
  const rows = packet.summary;
  if (!Array.isArray(rows) || rows.length === 0) fail("packet artifact summary must be a non-empty array");
  return Math.max(...rows.map((row, index) => number(row.median_e2e_wall_ms, `packet summary[${index}].median_e2e_wall_ms`))) / 1000;
}
function metricsFrom(stats, packet, profile) {
  const values = {
    status_seconds: stats.retrieval_status_seconds,
    local_grounding_seconds: stats.ground_seconds,
    convergence_seconds: stats.repeat_full_refresh_seconds,
    packet_seconds: packetMaximum(packet),
    search_seconds: stats.search_seconds,
    indexing_seconds: stats.index_seconds,
    storage_growth_ratio: stats.storage_bytes / profile.metrics.storage_growth_ratio.reference_bytes,
  };
  for (const metric of METRICS) number(values[metric], `candidate metric ${metric}`);
  return values;
}

function statsContractFrom(repoRoot) {
  const contract = readJson(path.join(repoRoot, "benchmarks/release-evidence/repo-stats-contract.json"), "stats contract");
  if (contract.schema_version !== 1) fail("stats contract schema_version must be 1");
  for (const field of ["repeat_graph_phase_seconds_max", "repeat_semantic_phase_seconds_max", "repeat_full_refresh_regression_factor"]) {
    number(contract[field], `stats contract ${field}`);
  }
  return contract;
}

function validateRawProvenance(stats, packet, commit, profileName, identity, statsContract) {
  if (fullSha(stats.commit, "stats.commit") !== commit) fail("raw stats commit does not match candidate");
  if (JSON.stringify(stats.evidence_identity) !== JSON.stringify(identity)) fail("raw stats identity does not match profile");
  if (stats.proof_tier !== "full_sidecar"
      || stats.index?.error_count !== 0
      || stats.index?.sidecar_status_after_retrieval_index !== "full"
      || stats.ground?.sidecar_status_after_retrieval_index !== "full"
      || stats.search?.sidecar_shadow_retrieval_mode !== "full"
      || !(stats.sidecar_manifest?.symbol_doc_count > 0)
      || !(stats.sidecar_manifest?.dense_projection_count > 0)
      || stats.sidecar_manifest?.dense_projection_count !== stats.sidecar_manifest?.projection_count
      || stats.sidecar_manifest?.semantic_policy_version !== "graph_first_v1"
      || stats.sidecar_manifest?.graph_artifact_hash_present !== true
      || stats.sidecar_manifest?.dense_reason_count_total !== stats.sidecar_manifest?.dense_projection_count
      || stats.repeat_semantic_docs_embedded !== 0) {
    fail("raw stats artifact does not prove the full-sidecar readiness contract");
  }
  const repeatBaseline = stats.stats_baseline?.repeat_full_refresh_seconds;
  if (!(stats.repeat_graph_phase_seconds < statsContract.repeat_graph_phase_seconds_max)
      || !(stats.repeat_semantic_phase_seconds < statsContract.repeat_semantic_phase_seconds_max)
      || !(repeatBaseline > 0)
      || !(stats.repeat_full_refresh_seconds < repeatBaseline * statsContract.repeat_full_refresh_regression_factor)) {
    fail("raw stats artifact does not satisfy the shared repeat-refresh release contract");
  }
  const packetProvenance = object(packet.release_evidence, "packet.release_evidence");
  if (fullSha(packetProvenance.commit, "packet commit") !== commit) fail("raw packet commit does not match candidate");
  if (packetProvenance.profile !== profileName) fail("raw packet profile does not match candidate");
  if (JSON.stringify(packetProvenance.evidence_identity) !== JSON.stringify(identity)) fail("raw packet identity does not match profile");
  if (packetProvenance.publishable !== true || packetProvenance.repeats < 3 || packetProvenance.quality_gate_status !== "pass") fail("raw packet artifact is not publishable three-repeat quality-gated evidence");
  if (packet.repeats !== packetProvenance.repeats || !Array.isArray(packet.modes) || packet.modes.length === 0) fail("raw packet top-level modes/repeats do not match provenance");
  if (!Array.isArray(packetProvenance.publishable_blockers) || packetProvenance.publishable_blockers.length !== 0) fail("raw packet artifact contains publishable blockers");
  const rows = packetProvenance.rows;
  if (!Array.isArray(rows) || rows.length === 0) fail("raw packet artifact has no quality rows");
  const repeats = new Map();
  for (const [index, row] of rows.entries()) {
    const sufficiency = row.sufficiency;
    const latency = row.packet_latency;
    const invalid = row.status !== "pass"
      || row.quality?.pass !== true
      || sufficiency?.status !== "sufficient"
      || sufficiency?.sufficient_quality_mismatch === true
      || ["follow_up_commands_count", "open_next_count", "gaps_count", "coverage_unresolved_blocking_count"]
        .some((field) => Number(sufficiency?.[field] ?? 0) > 0)
      || latency?.sla_missed !== false
      || latency?.retrieval_shadow?.retrieval_mode !== "full";
    if (invalid) fail(`raw packet quality row ${index} does not satisfy the publishable contract`);
    const provenanceFailures = [
      ...repoProvenanceBlockers(row),
      ...cacheProvenanceBlockers(row),
    ];
    if (provenanceFailures.length > 0) {
      fail(`raw packet provenance row ${index} failed: ${provenanceFailures.join("; ")}`);
    }
    if (!Number.isInteger(row.repeat) || row.repeat < 1 || row.repeat > packetProvenance.repeats) fail(`raw packet row ${index} has invalid repeat`);
    const key = `${row.repo}/${row.task_id}/${row.mode}`;
    const values = repeats.get(key) ?? new Set();
    if (values.has(row.repeat)) fail(`raw packet rows contain duplicate repeat ${row.repeat} for ${key}`);
    values.add(row.repeat);
    repeats.set(key, values);
  }
  if ([...repeats.values()].some((values) => values.size !== packetProvenance.repeats)) fail("raw packet rows do not exactly cover the declared repeat count");
  const rowModes = [...new Set(rows.map((row) => row.mode))].sort();
  if (JSON.stringify(rowModes) !== JSON.stringify([...packet.modes].sort())) fail("raw packet row modes do not match top-level modes");
}

export function produceCandidate({ baselineDocument, baselineDir, profileName, statsPath, packetPath, outPath, expectedSha, mode, repoRoot }) {
  const profile = profileFrom(baselineDocument, profileName, mode, baselineDir);
  const commit = candidateCommit(expectedSha, mode, repoRoot);
  if (commit === profile.commit) fail("candidate and baseline commits are identical");
  const statsAbsolute = path.resolve(repoRoot, statsPath);
  const packetAbsolute = path.resolve(repoRoot, packetPath);
  const stats = readJson(statsAbsolute, "stats artifact");
  const packet = readJson(packetAbsolute, "packet artifact");
  const identity = object(stats.evidence_identity, "stats.evidence_identity");
  for (const key of ["corpus_id", "cache_id", "machine_fingerprint"]) {
    if (identity[key] !== profile.identity[key]) fail(`candidate ${key} does not match approved profile`);
  }
  const statsContract = statsContractFrom(repoRoot);
  validateRawProvenance(stats, packet, commit, profileName, identity, statsContract);
  const baseDir = path.dirname(path.resolve(outPath));
  const measured = metricsFrom(stats, packet, profile);
  const candidate = {
    schema_version: 2,
    baseline_id: profile.baseline_id,
    baseline_sha256: sha256(Buffer.from(JSON.stringify(profile))),
    commit,
    git_state: "clean",
    profile: profileName,
    identity,
    artifacts: {
      stats: fileAttestation(statsAbsolute, baseDir),
      packet: fileAttestation(packetAbsolute, baseDir),
    },
    metrics: Object.fromEntries(METRICS.map((metric) => [metric, {
      value: measured[metric],
      unit: profile.metrics[metric].unit,
      aggregation: profile.metrics[metric].aggregation,
    }])),
  };
  mkdirSync(baseDir, { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(candidate, null, 2)}\n`);
  return candidate;
}

function strictDate(value, label) {
  const parsed = new Date(`${value}T00:00:00Z`);
  if (!DATE.test(text(value, label)) || Number.isNaN(parsed.valueOf()) || parsed.toISOString().slice(0, 10) !== value) fail(`${label} must be a valid ISO date`);
  return value;
}
function exceptionFor(approval, context) {
  if (!approval) return null;
  for (const [key, expected] of Object.entries(context.bindings)) {
    if (approval[key] !== expected) fail(`approval ${key} does not match measured evidence`);
  }
  strictDate(approval.approved_at, "approval.approved_at");
  strictDate(approval.expires_at, "approval.expires_at");
  if (approval.expires_at < approval.approved_at || approval.expires_at < new Date().toISOString().slice(0, 10)) fail("approval is expired or expires before approval date");
  text(approval.owner, "approval.owner");
  text(approval.rationale, "approval.rationale");
  return approval;
}

export function evaluateCandidate({ baselineDocument, baselineDir, candidatePath, approvalDocument = null, outPath, expectedSha, mode, repoRoot }) {
  const bytes = readFileSync(candidatePath);
  const candidateHash = sha256(bytes);
  const candidate = JSON.parse(bytes);
  if (candidate.schema_version !== 2) fail("candidate schema_version must be 2");
  const profile = profileFrom(baselineDocument, candidate.profile, mode, baselineDir);
  const commit = candidateCommit(expectedSha, mode, repoRoot);
  if (candidate.commit !== commit || candidate.git_state !== "clean") fail("candidate is not bound to the clean expected commit");
  if (candidate.commit === profile.commit) fail("candidate and baseline commits are identical");
  const baselineHash = sha256(Buffer.from(JSON.stringify(profile)));
  if (candidate.baseline_id !== profile.baseline_id || candidate.baseline_sha256 !== baselineHash) fail("candidate baseline identity/hash does not match");
  if (JSON.stringify(candidate.identity) !== JSON.stringify(profile.identity)) fail("candidate identity does not match profile");
  const baseDir = path.dirname(path.resolve(candidatePath));
  for (const artifact of Object.values(object(candidate.artifacts, "candidate.artifacts"))) {
    const actual = fileAttestation(artifact.path, baseDir);
    if (actual.sha256 !== artifact.sha256 || actual.bytes !== artifact.bytes) fail(`artifact attestation changed: ${artifact.path}`);
  }
  const statsPath = path.resolve(baseDir, candidate.artifacts.stats.path);
  const packetPath = path.resolve(baseDir, candidate.artifacts.packet.path);
  const stats = readJson(statsPath, "stats artifact");
  const packet = readJson(packetPath, "packet artifact");
  const statsContract = statsContractFrom(repoRoot);
  validateRawProvenance(stats, packet, commit, candidate.profile, profile.identity, statsContract);
  const measured = metricsFrom(stats, packet, profile);
  if (approvalDocument && approvalDocument.schema_version !== 2) fail("approval schema_version must be 2");
  const approvals = approvalDocument?.metrics ?? {};
  const rows = METRICS.map((metric) => {
    const recorded = object(candidate.metrics[metric], `candidate metric ${metric}`);
    const budget = profile.metrics[metric];
    if (recorded.value !== measured[metric] || recorded.unit !== budget.unit || recorded.aggregation !== budget.aggregation) fail(`candidate metric ${metric} is not derived with the approved unit/aggregation`);
    const passed = measured[metric] <= budget.threshold;
    const bindings = {
      candidate_sha256: candidateHash, commit, profile: candidate.profile,
      baseline_id: profile.baseline_id, baseline_sha256: baselineHash, metric,
      measured_value: measured[metric], threshold: budget.threshold,
    };
    const exception = passed ? null : exceptionFor(approvals[metric], { bindings });
    return { status: passed ? "pass" : exception ? "approved_exception" : "fail", metric,
      decision: passed ? "accept" : exception ? "accept_with_rationale" : "reject",
      measured_value: measured[metric], reference: budget.reference, threshold: budget.threshold,
      unit: budget.unit, aggregation: budget.aggregation, source: budget.source,
      ...(exception ? { approval: exception } : {}) };
  });
  const rejected = rows.some((row) => row.decision === "reject");
  const report = { schema_version: 2, status: rejected ? "fail" : "pass", decision: rejected ? "reject_release" : mode === "release" ? "accept_release" : "accept_contract",
    commit, profile: candidate.profile, baseline_id: profile.baseline_id, baseline_sha256: baselineHash,
    candidate_path: path.relative(repoRoot, path.resolve(candidatePath)).replaceAll(path.sep, "/"), candidate_sha256: candidateHash,
    artifact_paths: Object.values(candidate.artifacts), metrics: rows };
  mkdirSync(path.dirname(path.resolve(outPath)), { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
  return report;
}

function args(argv) {
  const command = argv.shift();
  const values = {};
  while (argv.length) { const key = argv.shift(); const value = argv.shift(); if (!key?.startsWith("--") || value == null) fail("arguments must be --key value pairs"); values[key.slice(2)] = value; }
  return { command, values };
}
function main() {
  const { command, values } = args(process.argv.slice(2));
  if (command === "fingerprint") {
    console.log(machineFingerprint());
    return;
  }
  const repoRoot = path.resolve(values.repo ?? path.resolve(path.dirname(new URL(import.meta.url).pathname), ".."));
  const baselineDocument = readJson(values.baseline, "baseline");
  const baselineDir = path.dirname(path.resolve(values.baseline));
  const mode = values.mode ?? "contract";
  if (command === "produce") produceCandidate({ baselineDocument, baselineDir, profileName: values.profile, statsPath: values.stats, packetPath: values.packet, outPath: values.out, expectedSha: values["expected-sha"], mode, repoRoot });
  else if (command === "evaluate") {
    const report = evaluateCandidate({ baselineDocument, baselineDir, candidatePath: values.candidate, approvalDocument: values.approval ? readJson(values.approval, "approval") : null, outPath: values.out, expectedSha: values["expected-sha"], mode, repoRoot });
    if (!report.decision.startsWith("accept_")) process.exitCode = 1;
  } else fail("command must be produce or evaluate");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();
