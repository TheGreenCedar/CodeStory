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
import {
  evaluateReleaseClaims,
  loadReleaseClaimGraph,
  releaseClaimGraphDigest,
} from "./codestory-release-claims.mjs";

const METRICS = [
  "status_seconds", "local_grounding_seconds", "convergence_seconds",
  "packet_seconds", "search_seconds", "indexing_seconds", "storage_growth_ratio",
];
const RELEASE_EVIDENCE_CLAIM_IDS = [
  "retrieval_readiness",
  "performance",
  "answer_quality",
];
const SHA = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;
const MACHINE_FINGERPRINT = /^[A-Za-z0-9][A-Za-z0-9._:/+-]{2,199}$/;
const PACKET_RUNTIME_MODES = new Map([
  ["cold-cli", "cold_cli_packet"],
  ["warm-stdio", "warm_stdio_packet"],
]);
const SOURCE_RUN_PRODUCERS = new Map([
  [".github/workflows/packaged-platform-pr.yml", new Set(["workflow_dispatch"])],
  [".github/workflows/release.yml", new Set(["workflow_dispatch"])],
  [".github/workflows/auto-release.yml", new Set(["push"])],
]);
const SELECTED_REF_PRODUCERS = new Set([
  ".github/workflows/packaged-platform-pr.yml",
]);
const RELEASE_CORPUS_CONTRACTS = new Map([
  [
    "codestory-release-corpus-v0.16-axios-js-ts-v1",
    "benchmarks/release-evidence/corpus-contracts/v0.16-axios-js-ts-v1.json",
  ],
]);

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
function repositoryIdentity(repoRoot) {
  const remote = git(["config", "--get", "remote.origin.url"], repoRoot);
  const match = remote.match(/github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?$/u);
  if (!match) fail(`cannot derive GitHub repository identity from origin ${remote}`);
  return match[1];
}
function sourceTreeIdentity(commit, mode, repoRoot) {
  return mode === "release"
    ? git(["rev-parse", `${commit}^{tree}`], repoRoot)
    : createHash("sha1").update(`contract-fixture:${commit}`).digest("hex");
}
function releaseClaimObservedAt() {
  const observedAt = process.env.CODESTORY_RELEASE_EVIDENCE_OBSERVED_AT?.trim() ?? new Date().toISOString();
  const parsed = Date.parse(observedAt);
  if (!Number.isFinite(parsed) || new Date(parsed).toISOString() !== observedAt) {
    fail("CODESTORY_RELEASE_EVIDENCE_OBSERVED_AT must be a canonical ISO timestamp");
  }
  return observedAt;
}
function releaseEvidenceClaimProfile(graph) {
  const nonWaivableMetrics = [...graph.exception_policy.non_waivable_metrics].sort();
  if (JSON.stringify(nonWaivableMetrics) !== JSON.stringify([...METRICS].sort())) {
    fail("release claim graph must mark every release-gate metric as non-waivable");
  }
  const claims = new Map(graph.claims.map((claim) => [claim.id, claim]));
  const evidenceTypes = new Map(graph.evidence_types.map((evidenceType) => [evidenceType.id, evidenceType]));
  return RELEASE_EVIDENCE_CLAIM_IDS.map((id) => {
    const claim = claims.get(id);
    const evidenceType = evidenceTypes.get(id);
    if (!claim || !evidenceType || JSON.stringify(claim.required_evidence) !== JSON.stringify([id])) {
      fail(`release evidence claim profile requires one ${id} evidence type`);
    }
    return {
      id,
      accepted_risks: [...claim.accepted_risks].sort(),
      tier: evidenceType.tier,
    };
  });
}
function releaseClaimDocument({
  repoRoot,
  mode,
  commit,
  profileName,
  identity,
  baselineId,
  baselineSha256,
  releaseKey,
  packetSha256,
  observedAt = releaseClaimObservedAt(),
}) {
  const graph = loadReleaseClaimGraph(repoRoot);
  const graphSha256 = releaseClaimGraphDigest(graph);
  const parsedObservedAt = Date.parse(observedAt);
  if (!Number.isFinite(parsedObservedAt) || new Date(parsedObservedAt).toISOString() !== observedAt) {
    fail("release claim observed_at must be a canonical ISO timestamp");
  }
  const expiresAt = new Date(Date.parse(observedAt) + 24 * 60 * 60 * 1000).toISOString();
  const claimProfile = releaseEvidenceClaimProfile(graph);
  const common = {
    repository: repositoryIdentity(repoRoot),
    commit,
    source_tree: sourceTreeIdentity(commit, mode, repoRoot),
    profile: profileName,
    corpus_id: identity.corpus_id,
    cache_id: identity.cache_id,
    machine_fingerprint: identity.machine_fingerprint,
    release_key: releaseKey,
  };
  const row = (type, tier, extra = {}, status = "pass") => ({
    id: `${type}-${commit.slice(0, 12)}`,
    type,
    tier,
    status,
    graph_sha256: graphSha256,
    observed_at: observedAt,
    expires_at: expiresAt,
    identity: { ...common, ...extra },
  });
  return {
    graph_schema: graph.schema,
    graph_sha256: graphSha256,
    observed_at: observedAt,
    expires_at: expiresAt,
    requested_claims: claimProfile.map(({ id, accepted_risks: acceptedRisks }) => ({
      id,
      accepted_risks: acceptedRisks,
    })),
    evidence: claimProfile.map(({ id, tier }) => {
      if (id === "performance") {
        return row(id, tier, { baseline_id: baselineId, baseline_sha256: baselineSha256 }, "measured");
      }
      if (id === "answer_quality") {
        const evaluationContract = graph.evidence_types.find(({ id: type }) => type === id)
          .identity_constraints.evaluation_contract;
        return row(id, tier, {
          artifact_sha256: packetSha256,
          evaluation_contract: evaluationContract,
        });
      }
      return row(id, tier);
    }),
  };
}
function requireExactReleaseClaimDocument({
  document,
  repoRoot,
  mode,
  commit,
  profileName,
  identity,
  baselineId,
  baselineSha256,
  releaseKey,
  packetSha256,
}) {
  object(document, "candidate.release_claims");
  const expected = releaseClaimDocument({
    repoRoot,
    mode,
    commit,
    profileName,
    identity,
    baselineId,
    baselineSha256,
    releaseKey,
    packetSha256,
    observedAt: text(document.observed_at, "candidate.release_claims.observed_at"),
  });
  const requestedClaimsMatch = JSON.stringify(canonical(document.requested_claims))
    === JSON.stringify(canonical(expected.requested_claims));
  const actualEvidenceProfile = Array.isArray(document.evidence)
    ? document.evidence.map(({ id, type }) => ({ id, type }))
    : null;
  const expectedEvidenceProfile = expected.evidence.map(({ id, type }) => ({ id, type }));
  const evidenceProfileMatches = JSON.stringify(canonical(actualEvidenceProfile))
    === JSON.stringify(canonical(expectedEvidenceProfile));
  if (!requestedClaimsMatch || !evidenceProfileMatches) {
    fail(
      "candidate.release_claims must exactly match the trusted release-evidence claim profile, including retrieval_readiness, performance, and answer_quality with one evidence row each",
    );
  }
  return document;
}
function evaluateClaimDocument({
  document,
  repoRoot,
  mode,
  commit,
  profileName,
  identity,
  baselineId,
  baselineSha256,
  releaseKey,
  packetSha256,
  candidateSha256,
  expectedExceptions,
}) {
  const graph = loadReleaseClaimGraph(repoRoot);
  if (document.graph_schema !== graph.schema || document.graph_sha256 !== releaseClaimGraphDigest(graph)) {
    fail("release claim evaluation failed: stale_evidence graph schema or digest mismatch");
  }
  const evaluation = evaluateReleaseClaims({
    graph,
    requested_claims: document.requested_claims,
    evidence: document.evidence,
    expected: {
      commit,
      evaluated_at: mode === "release" ? new Date().toISOString() : document.observed_at,
      identity: {
        repository: repositoryIdentity(repoRoot),
        source_tree: sourceTreeIdentity(commit, mode, repoRoot),
        profile: profileName,
        corpus_id: identity.corpus_id,
        cache_id: identity.cache_id,
        machine_fingerprint: identity.machine_fingerprint,
        baseline_id: baselineId,
        baseline_sha256: baselineSha256,
        candidate_sha256: candidateSha256,
        release_key: releaseKey,
        artifact_sha256: packetSha256,
        evaluation_contract: graph.evidence_types.find(({ id }) => id === "answer_quality")
          .identity_constraints.evaluation_contract,
      },
      exceptions: expectedExceptions,
    },
  });
  return evaluation;
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

export function validateSourceRunMetadata({ run, artifacts, expectedRepo, expectedSha }) {
  object(run, "source run");
  const repo = text(expectedRepo, "expected repository");
  const sha = fullSha(expectedSha, "expected SHA");
  if (run.repository?.full_name !== repo || run.head_repository?.full_name !== repo) {
    fail("source run must come from the current repository");
  }
  if (run.conclusion !== "failure") {
    fail("source run must be a rejected failed evidence run");
  }
  const events = SOURCE_RUN_PRODUCERS.get(run.path);
  if (!events?.has(run.event)) {
    fail("source run workflow and event are not trusted evidence producers");
  }
  const runId = number(run.id, "source run id");
  const artifactList = object(artifacts, "source run artifacts");
  if (!Array.isArray(artifactList.artifacts)) {
    fail("source run artifacts.artifacts must be an array");
  }
  const artifactName = `release-evidence-${sha}`;
  const matches = artifactList.artifacts.filter((artifact) => artifact?.name === artifactName);
  if (matches.length !== 1) {
    fail(`source run must contain exactly one ${artifactName} artifact`);
  }
  const artifact = object(matches[0], "source run evidence artifact");
  if (artifact.expired !== false) fail("source run evidence artifact must not be expired");
  number(artifact.size_in_bytes, "source run evidence artifact size");
  const artifactRun = object(artifact.workflow_run, "source run evidence artifact workflow_run");
  if (number(artifactRun.id, "source run evidence artifact workflow_run id") !== runId) {
    fail("source run evidence artifact belongs to a different workflow run");
  }
  const headSha = fullSha(run.head_sha, "source run head SHA");
  if (fullSha(artifactRun.head_sha, "source run evidence artifact head SHA") !== headSha) {
    fail("source run evidence artifact head SHA does not match its workflow run");
  }
  if (headSha !== sha && !SELECTED_REF_PRODUCERS.has(run.path)) {
    fail("source run head SHA does not match the evidence SHA");
  }
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

function packetRuntimeModes(modes) {
  return [...new Set(modes.map((mode, index) => {
    const selected = text(mode, `packet.modes[${index}]`);
    const runtime = PACKET_RUNTIME_MODES.get(selected);
    if (!runtime) fail(`packet.modes[${index}] is not a supported packet runtime mode`);
    return runtime;
  }))].sort();
}

function statsContractFrom(repoRoot) {
  const contract = readJson(path.join(repoRoot, "benchmarks/release-evidence/repo-stats-contract.json"), "stats contract");
  if (contract.schema_version !== 1) fail("stats contract schema_version must be 1");
  for (const field of ["repeat_graph_phase_seconds_max", "repeat_semantic_phase_seconds_max", "repeat_full_refresh_regression_factor"]) {
    number(contract[field], `stats contract ${field}`);
  }
  return contract;
}

function releaseCorpusContract(repoRoot, identity) {
  const relativePath = RELEASE_CORPUS_CONTRACTS.get(identity.corpus_id);
  if (!relativePath) return null;
  const absolutePath = path.resolve(repoRoot, relativePath);
  const bytes = readFileSync(absolutePath);
  const contract = JSON.parse(bytes.toString("utf8"));
  if (contract.schema_version !== 1 || contract.corpus_id !== identity.corpus_id) {
    fail("release corpus contract is malformed or has the wrong corpus_id");
  }
  if (!Array.isArray(contract.task_ids) || contract.task_ids.length === 0) {
    fail("release corpus contract must name selected task IDs");
  }
  const taskIds = [...new Set(contract.task_ids)].sort();
  if (taskIds.length !== contract.task_ids.length || JSON.stringify(taskIds) !== JSON.stringify(contract.task_ids)) {
    fail("release corpus contract task IDs must be sorted and unique");
  }
  const runtimeModes = Array.isArray(contract.runtime_modes)
    ? [...new Set(contract.runtime_modes)].sort()
    : [];
  if (
    runtimeModes.length === 0
    || runtimeModes.length !== contract.runtime_modes.length
    || JSON.stringify(runtimeModes) !== JSON.stringify(contract.runtime_modes)
    || runtimeModes.some((mode) => ![...PACKET_RUNTIME_MODES.values()].includes(mode))
  ) {
    fail("release corpus contract runtime modes must be supported, sorted, and unique");
  }
  if (!Number.isInteger(contract.repeats) || contract.repeats < 1) {
    fail("release corpus contract repeats must be a positive integer");
  }
  const taskManifestIds = Object.keys(object(contract.task_manifests, "release corpus task_manifests")).sort();
  if (JSON.stringify(taskManifestIds) !== JSON.stringify(taskIds)) {
    fail("release corpus contract task manifest keys must exactly match task IDs");
  }
  const taskRepositories = {};
  for (const taskId of taskIds) {
    const declaration = object(contract.task_manifests[taskId], `release corpus task manifest ${taskId}`);
    const manifestPath = path.resolve(repoRoot, text(declaration.path, `release corpus task manifest path ${taskId}`));
    const relativeManifestPath = path.relative(repoRoot, manifestPath);
    if (relativeManifestPath.startsWith(`..${path.sep}`) || path.isAbsolute(relativeManifestPath)) {
      fail(`release corpus task manifest path escapes the repository for ${taskId}`);
    }
    const manifestBytes = readFileSync(manifestPath);
    if (sha256(manifestBytes) !== text(declaration.sha256, `release corpus task manifest hash ${taskId}`)) {
      fail(`release corpus task manifest hash does not match for ${taskId}`);
    }
    const manifest = JSON.parse(manifestBytes.toString("utf8"));
    if (manifest.id !== taskId) fail(`release corpus task manifest ID does not match ${taskId}`);
    taskRepositories[taskId] = text(manifest.repo?.name, `release corpus task repository ${taskId}`);
  }
  return {
    path: relativePath,
    sha256: sha256(bytes),
    corpus_id: contract.corpus_id,
    task_ids: taskIds,
    runtime_modes: runtimeModes,
    repeats: contract.repeats,
    task_manifests: contract.task_manifests,
    task_repositories: taskRepositories,
    project_manifests: contract.project_manifests ?? {},
  };
}

export function validatePacketCorpusContract(packetProvenance, rows, repoRoot, identity, profile) {
  const expected = releaseCorpusContract(repoRoot, identity);
  if (!expected) return;
  const approved = object(profile.corpus_contract, "baseline profile corpus_contract");
  if (approved.path !== expected.path || approved.sha256 !== expected.sha256) {
    fail("checked-in release corpus contract does not match the approved baseline scope");
  }
  if (JSON.stringify(packetProvenance.corpus_contract) !== JSON.stringify(expected)) {
    fail("raw packet corpus contract does not match the checked-in release scope");
  }
  const expectedRows = [];
  for (const taskId of expected.task_ids) {
    for (const mode of expected.runtime_modes) {
      for (let repeat = 1; repeat <= expected.repeats; repeat += 1) {
        expectedRows.push(`${expected.task_repositories[taskId]}/${taskId}/${mode}/${repeat}`);
      }
    }
  }
  const observedRows = rows.map((row, index) => {
    const repo = text(row.repo, `packet row ${index} repo`);
    const taskId = text(row.task_id, `packet row ${index} task_id`);
    const mode = text(row.mode, `packet row ${index} mode`);
    if (!Number.isInteger(row.repeat) || row.repeat < 1) {
      fail(`packet row ${index} repeat must be a positive integer`);
    }
    return `${repo}/${taskId}/${mode}/${row.repeat}`;
  }).sort();
  if (
    new Set(observedRows).size !== observedRows.length
    || JSON.stringify(observedRows) !== JSON.stringify(expectedRows.sort())
  ) {
    fail("raw packet rows do not exactly match the checked-in release task scope");
  }
}

function validateRawProvenance(
  stats,
  packet,
  commit,
  sourceTree,
  profileName,
  identity,
  statsContract,
  repoRoot,
  profile,
) {
  if (fullSha(stats.commit, "stats.commit") !== commit) fail("raw stats commit does not match candidate");
  if (JSON.stringify(stats.evidence_identity) !== JSON.stringify(identity)) fail("raw stats identity does not match profile");
  if (stats.proof_tier !== "full_retrieval"
      || stats.index?.error_count !== 0
      || stats.index?.retrieval_status_after_index !== "full"
      || stats.ground?.retrieval_status_after_index !== "full"
      || stats.search?.retrieval_shadow_mode !== "full"
      || !(stats.retrieval_manifest?.symbol_doc_count > 0)
      || !(stats.retrieval_manifest?.dense_projection_count > 0)
      || stats.retrieval_manifest?.dense_projection_count !== stats.retrieval_manifest?.projection_count
      || stats.retrieval_manifest?.semantic_policy_version !== "graph_first_v1"
      || stats.retrieval_manifest?.graph_artifact_hash_present !== true
      || stats.retrieval_manifest?.dense_reason_count_total !== stats.retrieval_manifest?.dense_projection_count
      || stats.repeat_semantic_docs_embedded !== 0) {
    fail("raw stats artifact does not prove the full-retrieval readiness contract");
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
  if (packetProvenance.source_tree !== sourceTree) fail("raw packet source tree does not match candidate");
  if (packetProvenance.evaluation_contract !== "publishable-three-repeat-packet/v1") {
    fail("raw packet evaluation contract is unsupported");
  }
  if (packetProvenance.profile !== profileName) fail("raw packet profile does not match candidate");
  if (JSON.stringify(packetProvenance.evidence_identity) !== JSON.stringify(identity)) fail("raw packet identity does not match profile");
  if (packetProvenance.publishable !== true || packetProvenance.repeats < 3 || packetProvenance.quality_gate_status !== "pass") fail("raw packet artifact is not publishable three-repeat quality-gated evidence");
  if (packet.repeats !== packetProvenance.repeats || !Array.isArray(packet.modes) || packet.modes.length === 0) fail("raw packet top-level modes/repeats do not match provenance");
  if (!Array.isArray(packetProvenance.publishable_blockers) || packetProvenance.publishable_blockers.length !== 0) fail("raw packet artifact contains publishable blockers");
  const rows = packetProvenance.rows;
  if (!Array.isArray(rows) || rows.length === 0) fail("raw packet artifact has no quality rows");
  validatePacketCorpusContract(packetProvenance, rows, repoRoot, identity, profile);
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
  const rowModes = [...new Set(rows.map((row, index) => text(row.mode, `packet row ${index} mode`)))].sort();
  if (JSON.stringify(rowModes) !== JSON.stringify(packetRuntimeModes(packet.modes))) fail("raw packet row modes do not match top-level modes");
}

export function produceCandidate({ baselineDocument, baselineDir, profileName, statsPath, packetPath, outPath, expectedSha, mode, repoRoot, releaseKey }) {
  const profile = profileFrom(baselineDocument, profileName, mode, baselineDir);
  const commit = candidateCommit(expectedSha, mode, repoRoot);
  const selectedReleaseKey = text(releaseKey, "release key");
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
  validateRawProvenance(
    stats,
    packet,
    commit,
    sourceTreeIdentity(commit, mode, repoRoot),
    profileName,
    identity,
    statsContract,
    repoRoot,
    profile,
  );
  const baseDir = path.dirname(path.resolve(outPath));
  const measured = metricsFrom(stats, packet, profile);
  const baselineSha256 = sha256(Buffer.from(JSON.stringify(profile)));
  const artifacts = {
    stats: fileAttestation(statsAbsolute, baseDir),
    packet: fileAttestation(packetAbsolute, baseDir),
  };
  const releaseClaims = releaseClaimDocument({
    repoRoot,
    mode,
    commit,
    profileName,
    identity,
    baselineId: profile.baseline_id,
    baselineSha256,
    releaseKey: selectedReleaseKey,
    packetSha256: artifacts.packet.sha256,
  });
  const candidate = {
    schema_version: 3,
    baseline_id: profile.baseline_id,
    baseline_sha256: baselineSha256,
    commit,
    git_state: "clean",
    profile: profileName,
    release_key: selectedReleaseKey,
    identity,
    artifacts,
    metrics: Object.fromEntries(METRICS.map((metric) => [metric, {
      value: measured[metric],
      unit: profile.metrics[metric].unit,
      aggregation: profile.metrics[metric].aggregation,
    }])),
    release_claims: releaseClaims,
  };
  mkdirSync(baseDir, { recursive: true });
  writeFileSync(outPath, `${JSON.stringify(candidate, null, 2)}\n`);
  return candidate;
}

export function evaluateCandidate({ baselineDocument, baselineDir, candidatePath, approvalDocument = null, outPath, expectedSha, mode, repoRoot, releaseKey }) {
  const bytes = readFileSync(candidatePath);
  const candidateHash = sha256(bytes);
  const candidate = JSON.parse(bytes);
  if (candidate.schema_version !== 3) fail("candidate schema_version must be 3");
  const profile = profileFrom(baselineDocument, candidate.profile, mode, baselineDir);
  const commit = candidateCommit(expectedSha, mode, repoRoot);
  const selectedReleaseKey = text(releaseKey, "release key");
  if (candidate.commit !== commit || candidate.git_state !== "clean") fail("candidate is not bound to the clean expected commit");
  if (candidate.release_key !== selectedReleaseKey) fail("candidate release_key does not match the selected release");
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
  validateRawProvenance(
    stats,
    packet,
    commit,
    sourceTreeIdentity(commit, mode, repoRoot),
    candidate.profile,
    profile.identity,
    statsContract,
    repoRoot,
    profile,
  );
  const measured = metricsFrom(stats, packet, profile);
  if (approvalDocument && approvalDocument.schema_version !== 3) fail("approval schema_version must be 3");
  const approvals = object(approvalDocument?.metrics ?? {}, "approval.metrics");
  for (const metric of Object.keys(approvals)) {
    if (METRICS.includes(metric)) {
      fail(`release metric ${metric} is a non-waivable full-product gate`);
    }
    fail(`approval metric ${metric} is not a supported model microbenchmark`);
  }
  const rows = METRICS.map((metric) => {
    const recorded = object(candidate.metrics[metric], `candidate metric ${metric}`);
    const budget = profile.metrics[metric];
    if (recorded.value !== measured[metric] || recorded.unit !== budget.unit || recorded.aggregation !== budget.aggregation) fail(`candidate metric ${metric} is not derived with the approved unit/aggregation`);
    const passed = measured[metric] <= budget.threshold;
    return { status: passed ? "pass" : "fail", metric,
      decision: passed ? "accept" : "reject",
      measured_value: measured[metric], reference: budget.reference, threshold: budget.threshold,
      unit: budget.unit, aggregation: budget.aggregation, source: budget.source };
  });
  const claimDocument = structuredClone(requireExactReleaseClaimDocument({
    document: candidate.release_claims,
    repoRoot,
    mode,
    commit,
    profileName: candidate.profile,
    identity: profile.identity,
    baselineId: profile.baseline_id,
    baselineSha256: baselineHash,
    releaseKey: selectedReleaseKey,
    packetSha256: candidate.artifacts.packet.sha256,
  }));
  const expectedExceptions = {};
  for (const evidence of claimDocument.evidence ?? []) {
    if (evidence.type === "retrieval_readiness" || evidence.type === "answer_quality") {
      evidence.status = "pass";
      delete evidence.exception;
    }
    if (evidence.type === "performance") {
      if (rows.some((row) => row.decision === "reject")) {
        evidence.status = "fail";
        delete evidence.exception;
      } else {
        evidence.status = "pass";
        delete evidence.exception;
      }
    }
  }
  const claimEvaluation = evaluateClaimDocument({
    document: claimDocument,
    repoRoot,
    mode,
    commit,
    profileName: candidate.profile,
    identity: profile.identity,
    baselineId: profile.baseline_id,
    baselineSha256: baselineHash,
    releaseKey: selectedReleaseKey,
    packetSha256: candidate.artifacts.packet.sha256,
    candidateSha256: candidateHash,
    expectedExceptions,
  });
  const rejected = rows.some((row) => row.decision === "reject") || claimEvaluation.status === "fail";
  const acceptedWithException = !rejected && claimEvaluation.status === "pass_with_exception";
  const reportStatus = rejected ? "fail" : acceptedWithException ? "pass_with_exception" : "pass";
  const acceptedDecision = mode === "release" ? "accept_release" : "accept_contract";
  const decision = rejected
    ? "reject_release"
    : acceptedWithException ? `${acceptedDecision}_with_exception` : acceptedDecision;
  const report = { schema_version: 3, status: reportStatus, decision,
    commit, profile: candidate.profile, baseline_id: profile.baseline_id, baseline_sha256: baselineHash,
    candidate_path: path.relative(repoRoot, path.resolve(candidatePath)).replaceAll(path.sep, "/"), candidate_sha256: candidateHash,
    artifact_paths: Object.values(candidate.artifacts), release_claim_evaluation: claimEvaluation, metrics: rows };
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
  if (command === "validate-source-run") {
    validateSourceRunMetadata({
      run: readJson(values.run, "source run"),
      artifacts: readJson(values.artifacts, "source run artifacts"),
      expectedRepo: values.repo,
      expectedSha: values["expected-sha"],
    });
    return;
  }
  const repoRoot = path.resolve(values.repo ?? path.resolve(path.dirname(new URL(import.meta.url).pathname), ".."));
  const baselineDocument = readJson(values.baseline, "baseline");
  const baselineDir = path.dirname(path.resolve(values.baseline));
  const mode = values.mode ?? "contract";
  if (command === "produce") produceCandidate({ baselineDocument, baselineDir, profileName: values.profile, statsPath: values.stats, packetPath: values.packet, outPath: values.out, expectedSha: values["expected-sha"], mode, repoRoot, releaseKey: values["release-key"] });
  else if (command === "evaluate") {
    const report = evaluateCandidate({ baselineDocument, baselineDir, candidatePath: values.candidate, approvalDocument: values.approval ? readJson(values.approval, "approval") : null, outPath: values.out, expectedSha: values["expected-sha"], mode, repoRoot, releaseKey: values["release-key"] });
    if (report.release_claim_evaluation.failures.length > 0) {
      console.error(`Release claim failures: ${JSON.stringify(report.release_claim_evaluation.failures)}`);
    }
    if (!report.decision.startsWith("accept_")) process.exitCode = 1;
  } else fail("command must be fingerprint, validate-source-run, produce, or evaluate");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();
