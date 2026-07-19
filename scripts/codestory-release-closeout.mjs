#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  canonicalReleaseClaimValue,
  deriveTrustedGitIdentity,
  evaluateReleaseClaims,
  loadReleaseClaimGraph,
  releaseClaimGraphDigest,
  releaseClaimIdentityMatchesFormat,
} from "./codestory-release-claims.mjs";

const MANIFEST_EVALUATION_SCHEMA = "codestory.release-cell-evaluation/v1";
const PRODUCER_MAP_SCHEMA = "codestory.release-actions-provenance/v1";
const TRUSTED_EXCEPTIONS_SCHEMA = "codestory.release-closeout-exceptions/v1";
const SHA256 = /^[0-9a-f]{64}$/u;
const ACTIONS_DIGEST = /^sha256:[0-9a-f]{64}$/u;
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u;
const AGGREGATE_IDENTITY = /^(?:aggregate|all|matrix|mixed|multiple|various)$/iu;

function fail(message) {
  throw new Error(message);
}

function object(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  return value;
}

function text(value, label) {
  if (typeof value !== "string" || value.trim() !== value || value === "") {
    fail(`${label} must be a non-empty trimmed string`);
  }
  return value;
}

function canonicalJson(value) {
  return `${JSON.stringify(canonicalReleaseClaimValue(value), null, 2)}\n`;
}

function digest(value) {
  return createHash("sha256").update(value).digest("hex");
}

function safeFileName(cellId) {
  return `${cellId.replaceAll(/[^A-Za-z0-9._-]/gu, "_")}.json`;
}

function phaseIndex(closeout, phase) {
  const index = closeout.phases.indexOf(phase);
  if (index < 0) fail(`phase must be one of ${closeout.phases.join(", ")}`);
  return index;
}

function packageHostIdentity(row) {
  const rustTarget = text(row.rust_target, "workflow_policy.package_matrix rust_target");
  const hostArch = rustTarget.startsWith("x86_64-")
    ? "X64"
    : rustTarget.startsWith("aarch64-")
      ? "ARM64"
      : null;
  const hostOs = rustTarget.includes("-unknown-linux-")
    ? "Linux"
    : rustTarget.includes("-pc-windows-")
      ? "Windows"
      : rustTarget.endsWith("-apple-darwin")
        ? "macOS"
        : null;
  if (hostOs === null || hostArch === null) {
    fail(`package target ${row.asset_target} uses unsupported Rust target ${rustTarget}`);
  }
  return { host_os: hostOs, host_arch: hostArch };
}

export function deriveReleaseCells(graph, phase) {
  const closeout = object(graph.closeout, "release claim graph.closeout");
  const selectedPhase = text(phase, "phase");
  const selectedIndex = phaseIndex(closeout, selectedPhase);
  const cells = [];
  for (const group of closeout.cell_groups) {
    if (phaseIndex(closeout, group.phase) > selectedIndex) continue;
    const add = (suffix, constraints = {}) => {
      const id = suffix === null ? group.id : `${group.id}:${suffix}`;
      const expandedConstraints = Object.fromEntries(
        Object.entries({
          ...(group.identity_constraints ?? {}),
          ...constraints,
        }).map(([key, value]) => [
          key,
          typeof value === "string" && suffix !== null
            ? value.replaceAll("{target}", suffix)
            : value,
        ]),
      );
      cells.push({
        id,
        group_id: group.id,
        phase: group.phase,
        claim: group.claim,
        evidence_type: group.evidence_type,
        required_identity: [...group.required_identity],
        singleton_identity: [...(group.singleton_identity ?? [])],
        identity_constraints: expandedConstraints,
        archive_role: group.archive_role,
      });
    };
    if (group.expansion === "singleton") {
      add(null);
    } else if (group.expansion === "package_matrix") {
      for (const row of graph.workflow_policy.package_matrix) {
        const constraints = { target: row.asset_target };
        const hostIdentity = packageHostIdentity(row);
        for (const key of ["host_os", "host_arch"]) {
          if (group.required_identity.includes(key)) constraints[key] = hostIdentity[key];
        }
        add(row.asset_target, constraints);
      }
    } else if (group.expansion === "instances") {
      for (const instance of group.instances) add(instance.id, instance.identity_constraints);
    } else {
      fail(`unsupported closeout expansion ${String(group.expansion)}`);
    }
  }
  cells.sort((left, right) => left.id.localeCompare(right.id));
  const ids = cells.map(({ id }) => id);
  if (new Set(ids).size !== ids.length) fail("release claim graph derives duplicate closeout cell ids");
  return cells;
}

export function resolveReleaseCellConstraints(cell, producerRunAttempt) {
  const attempt = text(producerRunAttempt, "producer run attempt");
  if (!/^[1-9]\d*$/u.test(attempt)) fail("producer run attempt must be a positive integer");
  return Object.fromEntries(Object.entries(cell.identity_constraints).map(([key, value]) => [
    key,
    typeof value === "string" ? value.replaceAll("{attempt}", attempt) : value,
  ]));
}

function manifestProblems({ manifest, cell, graph, graphSha256, version }) {
  const problems = [];
  if (manifest.schema !== graph.closeout.manifest_schema) {
    problems.push(`manifest schema must be ${graph.closeout.manifest_schema}`);
  }
  if (manifest.cell_id !== cell.id) problems.push(`manifest cell_id must be ${cell.id}`);
  if (manifest.phase !== cell.phase) problems.push(`manifest phase must be ${cell.phase}`);
  if (manifest.version !== version) problems.push(`manifest version must be ${version}`);
  if (manifest.graph_sha256 !== graphSha256) problems.push("manifest graph_sha256 is stale");
  let evidence = null;
  try {
    evidence = object(manifest.evidence, `${cell.id}.evidence`);
  } catch (error) {
    problems.push(error.message);
    return problems;
  }
  if (evidence.type !== cell.evidence_type) {
    problems.push(`manifest evidence type must be ${cell.evidence_type}`);
  }
  if (evidence.graph_sha256 !== graphSha256) problems.push("manifest evidence graph_sha256 is stale");
  const identity = evidence.identity;
  if (identity === null || typeof identity !== "object" || Array.isArray(identity)) {
    problems.push("manifest evidence identity must be an object");
    return problems;
  }
  const formats = graph.evidence_policy.identity_formats;
  for (const key of cell.required_identity) {
    if (!releaseClaimIdentityMatchesFormat(identity[key], formats[key])) {
      problems.push(`manifest identity ${key} does not match ${formats[key]}`);
    }
  }
  let resolvedConstraints = cell.identity_constraints;
  try {
    resolvedConstraints = resolveReleaseCellConstraints(cell, identity.producer_run_attempt);
  } catch (error) {
    problems.push(error.message);
  }
  for (const [key, expected] of Object.entries(resolvedConstraints)) {
    if (identity[key] !== expected) {
      problems.push(`manifest identity ${key} must equal ${expected}`);
    }
  }
  if (cell.required_identity.includes("producer_version") && identity.producer_version !== version) {
    problems.push(`manifest identity producer_version must equal closeout version ${version}`);
  }
  if (cell.required_identity.includes("runtime_version") && identity.runtime_version !== version) {
    problems.push(`manifest identity runtime_version must equal closeout version ${version}`);
  }
  if (cell.required_identity.includes("producer_version")
      && cell.required_identity.includes("runtime_version")
      && identity.producer_version !== identity.runtime_version) {
    problems.push("manifest producer_version and runtime_version must match");
  }
  for (const key of cell.singleton_identity) {
    if (AGGREGATE_IDENTITY.test(String(identity[key] ?? ""))) {
      problems.push(`manifest identity ${key} must name one concrete value, not ${identity[key]}`);
    }
  }
  if (cell.archive_role === "pre_publish") {
    const archive = manifest.archive;
    if (archive === null || typeof archive !== "object" || Array.isArray(archive)) {
      problems.push("pre-publish package manifest must retain one archive attestation");
    } else {
      if (typeof archive.name !== "string" || archive.name === "" || path.basename(archive.name) !== archive.name) {
        problems.push("archive name must be one plain file name");
      }
      if (!SHA256.test(String(archive.sha256 ?? ""))) problems.push("archive sha256 must be SHA-256");
      if (!Number.isSafeInteger(archive.bytes) || archive.bytes <= 0) {
        problems.push("archive bytes must be a positive safe integer");
      }
      if (archive.sha256 !== identity.artifact_sha256) {
        problems.push("archive sha256 must match evidence artifact_sha256");
      }
    }
    if (manifest.comparison !== undefined) problems.push("pre-publish package manifest must not contain a byte comparison");
  } else if (cell.archive_role === "post_publish_compare") {
    if (manifest.archive !== undefined) problems.push("post-publish byte manifest must use comparison, not archive");
    const comparison = manifest.comparison;
    if (comparison === null || typeof comparison !== "object" || Array.isArray(comparison)) {
      problems.push("post-publish byte manifest must retain one comparison");
    } else {
      if (!SHA256.test(String(comparison.pre_publish_manifest_sha256 ?? ""))) {
        problems.push("comparison pre_publish_manifest_sha256 must be SHA-256");
      }
      if (!SHA256.test(String(comparison.pre_publish_artifact_sha256 ?? ""))) {
        problems.push("comparison pre_publish_artifact_sha256 must be SHA-256");
      }
      if (!SHA256.test(String(comparison.published_artifact_sha256 ?? ""))) {
        problems.push("comparison published_artifact_sha256 must be SHA-256");
      }
      if (comparison.pre_publish_artifact_sha256 !== identity.pre_publish_artifact_sha256) {
        problems.push("comparison pre-publish digest must match evidence identity");
      }
      if (comparison.published_artifact_sha256 !== identity.artifact_sha256) {
        problems.push("comparison published digest must match evidence artifact_sha256");
      }
      if (typeof comparison.published_artifact_name !== "string"
          || comparison.published_artifact_name === ""
          || path.basename(comparison.published_artifact_name) !== comparison.published_artifact_name) {
        problems.push("comparison published_artifact_name must be one plain file name");
      }
    }
  } else if (manifest.archive !== undefined || manifest.comparison !== undefined) {
    problems.push("non-archive closeout cells must not contain archive attestations");
  }
  return problems;
}

export function validateReleaseCellManifest({ manifest, cell, graph, version }) {
  return manifestProblems({
    manifest,
    cell,
    graph,
    graphSha256: releaseClaimGraphDigest(graph),
    version,
  });
}

function trustedProducerIndex({ trustedProducers, cells, gitIdentity, graph, phase }) {
  const errors = [];
  if (trustedProducers === null || typeof trustedProducers !== "object" || Array.isArray(trustedProducers)) {
    return { byCell: new Map(), errors: ["closeout requires a separately trusted producer map"] };
  }
  if (trustedProducers.schema !== PRODUCER_MAP_SCHEMA) {
    errors.push(`trusted producer map schema must be ${PRODUCER_MAP_SCHEMA}`);
  }
  if (trustedProducers.phase !== phase) errors.push(`trusted producer map phase must be ${phase}`);
  if (trustedProducers.manifest_schema !== graph.closeout.manifest_schema) {
    errors.push("trusted producer map manifest schema changed");
  }
  if (trustedProducers.graph_sha256 !== releaseClaimGraphDigest(graph)) {
    errors.push("trusted producer map graph identity changed");
  }
  if (!/^[1-9]\d*$/u.test(String(trustedProducers.run_id ?? ""))) {
    errors.push("trusted producer map run_id must be a positive integer");
  }
  if (!/^[1-9]\d*$/u.test(String(trustedProducers.current_run_attempt ?? ""))) {
    errors.push("trusted producer map current_run_attempt must be a positive integer");
  }
  for (const key of ["repository", "commit", "source_tree"]) {
    if (trustedProducers.identity?.[key] !== gitIdentity[key]) {
      errors.push(`trusted producer map ${key} identity changed`);
    }
  }
  const artifactsById = new Map();
  const artifactNames = new Set();
  if (!Array.isArray(trustedProducers.artifacts)) {
    errors.push("trusted producer map artifacts must be an array");
  } else {
    for (const [index, artifact] of trustedProducers.artifacts.entries()) {
      if (artifact === null || typeof artifact !== "object" || Array.isArray(artifact)) {
        errors.push(`trusted producer map artifact[${index}] must be an object`);
        continue;
      }
      const artifactId = String(artifact.id ?? "");
      if (!/^[1-9]\d*$/u.test(artifactId)) {
        errors.push(`trusted producer map artifact[${index}] id is invalid`);
      } else if (artifactsById.has(artifactId)) {
        errors.push(`trusted producer map duplicates artifact id ${artifactId}`);
      } else {
        artifactsById.set(artifactId, artifact);
      }
      if (typeof artifact.name !== "string" || artifact.name === "") {
        errors.push(`trusted producer map artifact[${index}] name is invalid`);
      } else if (artifactNames.has(artifact.name)) {
        errors.push(`trusted producer map duplicates artifact name ${artifact.name}`);
      } else {
        artifactNames.add(artifact.name);
      }
    }
  }
  const usedArtifactIds = new Set();
  const rows = Array.isArray(trustedProducers.producers) ? trustedProducers.producers : [];
  if (!Array.isArray(trustedProducers.producers)) errors.push("trusted producer map producers must be an array");
  const byCell = new Map();
  for (const [index, row] of rows.entries()) {
    if (row === null || typeof row !== "object" || Array.isArray(row)) {
      errors.push(`trusted producer map producer[${index}] must be an object`);
      continue;
    }
    if (typeof row.cell_id !== "string" || row.cell_id === "") {
      errors.push(`trusted producer map producer[${index}].cell_id must be non-empty`);
      continue;
    }
    if (byCell.has(row.cell_id)) {
      errors.push(`trusted producer map duplicates ${row.cell_id}`);
    } else {
      byCell.set(row.cell_id, row);
    }
  }
  const required = new Set(cells.map(({ id }) => id));
  for (const cellId of byCell.keys()) {
    if (!required.has(cellId)) errors.push(`trusted producer map contains undeclared cell ${cellId}`);
  }
  for (const cell of cells) {
    const row = byCell.get(cell.id);
    if (!row) {
      errors.push(`trusted producer map is missing ${cell.id}`);
      continue;
    }
    for (const key of [
      "producer_workflow",
      "producer_job",
      "producer_job_name",
      "producer_run_id",
      "producer_run_attempt",
      "producer_artifact",
    ]) {
      if (!releaseClaimIdentityMatchesFormat(row[key], graph.evidence_policy.identity_formats[key])) {
        errors.push(`trusted producer map ${cell.id} ${key} is invalid`);
      }
      let constrained;
      try {
        constrained = resolveReleaseCellConstraints(cell, row.producer_run_attempt)[key];
      } catch (error) {
        errors.push(`trusted producer map ${cell.id} ${error.message}`);
      }
      if (constrained !== undefined && row[key] !== constrained) {
        errors.push(`trusted producer map ${cell.id} ${key} must equal ${constrained}`);
      }
    }
    if (row.producer_run_id !== trustedProducers.run_id) {
      errors.push(`trusted producer map ${cell.id} run identity differs from the Actions run`);
    }
    if (/^[1-9]\d*$/u.test(String(row.producer_run_attempt ?? ""))
        && /^[1-9]\d*$/u.test(String(trustedProducers.current_run_attempt ?? ""))
        && Number(row.producer_run_attempt) > Number(trustedProducers.current_run_attempt)) {
      errors.push(`trusted producer map ${cell.id} uses a future run attempt`);
    }
    const artifact = row.artifact;
    if (artifact === null || typeof artifact !== "object" || Array.isArray(artifact)) {
      errors.push(`trusted producer map ${cell.id} artifact provenance is missing`);
    } else {
      if (!/^[1-9]\d*$/u.test(String(artifact.id ?? ""))) {
        errors.push(`trusted producer map ${cell.id} artifact id is invalid`);
      } else {
        usedArtifactIds.add(String(artifact.id));
        const inventoryArtifact = artifactsById.get(String(artifact.id));
        if (!inventoryArtifact) {
          errors.push(`trusted producer map ${cell.id} artifact is missing from the download inventory`);
        } else if (canonicalJson(inventoryArtifact) !== canonicalJson(artifact)) {
          errors.push(`trusted producer map ${cell.id} artifact differs from the download inventory`);
        }
      }
      if (artifact.name !== row.producer_artifact) {
        errors.push(`trusted producer map ${cell.id} artifact name changed`);
      }
      if (!ACTIONS_DIGEST.test(String(artifact.digest ?? ""))) {
        errors.push(`trusted producer map ${cell.id} artifact digest is invalid`);
      }
      if (!Number.isSafeInteger(artifact.size_in_bytes) || artifact.size_in_bytes <= 0) {
        errors.push(`trusted producer map ${cell.id} artifact size is invalid`);
      }
      if (artifact.expired !== false) {
        errors.push(`trusted producer map ${cell.id} artifact is expired`);
      }
      if (artifact.workflow_run_id !== row.producer_run_id
          || artifact.head_sha !== gitIdentity.commit) {
        errors.push(`trusted producer map ${cell.id} artifact run identity changed`);
      }
    }
    const job = row.job;
    if (job === null || typeof job !== "object" || Array.isArray(job)) {
      errors.push(`trusted producer map ${cell.id} job provenance is missing`);
    } else {
      if (!/^[1-9]\d*$/u.test(String(job.id ?? ""))) {
        errors.push(`trusted producer map ${cell.id} job id is invalid`);
      }
      if (job.run_id !== row.producer_run_id || job.head_sha !== gitIdentity.commit) {
        errors.push(`trusted producer map ${cell.id} job run identity changed`);
      }
      if (job.run_attempt !== row.producer_run_attempt
          || job.conclusion !== "success"
          || job.status !== "completed") {
        errors.push(`trusted producer map ${cell.id} job is not a successful matching attempt`);
      }
      if (typeof job.name !== "string"
          || job.name.split(" / ").at(-1) !== row.producer_job_name) {
        errors.push(`trusted producer map ${cell.id} job name changed`);
      }
      const createdAt = Date.parse(String(artifact?.created_at ?? ""));
      const startedAt = Date.parse(String(job.started_at ?? ""));
      const completedAt = Date.parse(String(job.completed_at ?? ""));
      if (![createdAt, startedAt, completedAt].every(Number.isFinite)
          || createdAt < startedAt
          || createdAt > completedAt) {
        errors.push(`trusted producer map ${cell.id} artifact is outside its job window`);
      }
    }
  }
  for (const artifactId of artifactsById.keys()) {
    if (!usedArtifactIds.has(artifactId)) {
      errors.push(`trusted producer map download inventory contains unused artifact ${artifactId}`);
    }
  }
  return { byCell, errors };
}

function producerAuthenticationProblems(manifest, trustedProducer) {
  if (!trustedProducer) return ["manifest producer is absent from the trusted producer map"];
  const identity = manifest.evidence?.identity ?? {};
  const problems = [];
  for (const key of [
    "producer_workflow",
    "producer_job",
    "producer_job_name",
    "producer_run_id",
    "producer_run_attempt",
    "producer_artifact",
  ]) {
    if (identity[key] !== trustedProducer[key]) {
      problems.push(`manifest ${key} does not match the trusted producer map`);
    }
  }
  return problems;
}

function trustedExceptionInput({ document, graph, graphSha256, gitIdentity, version, trustedProducer }) {
  const errors = [];
  if (document === null || typeof document !== "object" || Array.isArray(document)) {
    return {
      exceptions: {},
      identity: {},
      errors: ["closeout requires separately trusted exception inputs"],
    };
  }
  if (document.schema !== TRUSTED_EXCEPTIONS_SCHEMA) {
    errors.push(`trusted exception schema must be ${TRUSTED_EXCEPTIONS_SCHEMA}`);
  }
  if (document.graph_sha256 !== graphSha256) errors.push("trusted exception graph identity changed");
  if (document.version !== version) errors.push("trusted exception version changed");
  for (const key of ["repository", "commit", "source_tree"]) {
    if (document.identity?.[key] !== gitIdentity[key]) {
      errors.push(`trusted exception ${key} identity changed`);
    }
  }
  for (const key of [
    "producer_workflow",
    "producer_job",
    "producer_job_name",
    "producer_run_id",
    "producer_run_attempt",
    "producer_artifact",
  ]) {
    if (document.producer?.[key] !== trustedProducer?.[key]) {
      errors.push(`trusted exception ${key} does not match authenticated release evidence`);
    }
  }
  const exceptions = document.exceptions;
  if (exceptions === null || typeof exceptions !== "object" || Array.isArray(exceptions)) {
    errors.push("trusted exception inputs must be an object keyed by evidence id");
    return { exceptions: {}, identity: {}, errors };
  }
  const trustedIdentity = document.trusted_identity;
  if (trustedIdentity === null || typeof trustedIdentity !== "object" || Array.isArray(trustedIdentity)) {
    errors.push("trusted exception identity must be an object");
  } else {
    for (const key of [graph.exception_policy.artifact_binding, "artifact_sha256"]) {
      if (!SHA256.test(String(trustedIdentity[key] ?? ""))) {
        errors.push(`trusted exception identity ${key} must be SHA-256`);
      }
    }
  }
  return {
    exceptions: canonicalReleaseClaimValue(exceptions),
    identity: canonicalReleaseClaimValue(trustedIdentity ?? {}),
    errors,
  };
}

function transitiveClaims(graph, claimId) {
  const claims = new Map(graph.claims.map((claim) => [claim.id, claim]));
  const ordered = [];
  const found = new Set();
  const visit = (id) => {
    if (found.has(id)) return;
    found.add(id);
    const claim = claims.get(id);
    if (!claim) fail(`closeout cell references unknown claim ${id}`);
    for (const dependency of claim.depends_on_claims) visit(dependency);
    ordered.push(claim);
  };
  visit(claimId);
  return ordered;
}

function evaluationClaims(graph, cell, manifest) {
  const claims = transitiveClaims(graph, cell.claim);
  if (manifest.evidence?.status !== "pass_with_exception") return claims;
  const policy = graph.exception_policy;
  if (manifest.evidence.type !== policy.eligible_evidence_type) return claims;
  const seen = new Set(claims.map(({ id }) => id));
  for (const claim of transitiveClaims(graph, policy.full_product_benefit_evidence_type)) {
    if (!seen.has(claim.id)) {
      seen.add(claim.id);
      claims.push(claim);
    }
  }
  return claims;
}

function dependencyCell(cells, claimId, target) {
  let candidates = cells.filter((candidate) => candidate.group_id === claimId);
  if (target !== undefined) {
    const sameTarget = candidates.filter((candidate) => candidate.identity_constraints.target === target);
    if (sameTarget.length > 0) candidates = sameTarget;
  }
  if (candidates.length !== 1) {
    fail(`claim ${claimId} did not resolve to one dependency cell${target ? ` for ${target}` : ""}`);
  }
  return candidates[0];
}

function evaluateCell({
  cell,
  cells,
  manifests,
  graph,
  gitIdentity,
  evaluatedAt,
  trustedExceptions,
  trustedExceptionIdentity,
}) {
  const focal = manifests.get(cell.id);
  const claims = evaluationClaims(graph, cell, focal);
  const evidenceCells = [];
  for (const claim of claims) {
    evidenceCells.push(
      claim.id === cell.claim
        ? cell
        : dependencyCell(cells, claim.id, focal.evidence.identity.target),
    );
  }
  const evidence = evidenceCells.map((dependency) => manifests.get(dependency.id).evidence);
  const requestedClaims = claims.map((claim) => ({
    id: claim.id,
    accepted_risks: [...claim.accepted_risks],
  }));
  const expectedIdentity = {
    ...Object.assign({}, ...evidence.map((row) => row.identity ?? {})),
    ...focal.evidence.identity,
    ...(focal.evidence.status === "pass_with_exception" ? trustedExceptionIdentity : {}),
    ...gitIdentity,
  };
  const evaluation = evaluateReleaseClaims({
    graph,
    requested_claims: requestedClaims,
    evidence,
    expected: {
      commit: gitIdentity.commit,
      evaluated_at: evaluatedAt,
      identity: expectedIdentity,
      exceptions: trustedExceptions,
    },
  });
  return {
    schema: MANIFEST_EVALUATION_SCHEMA,
    cell_id: cell.id,
    evidence_cells: evidenceCells.map(({ id }) => id),
    status: evaluation.status,
    release_claim_evaluation: evaluation,
  };
}

function dependencyValidationProblems({ cell, cells, manifests, graph, problemsByCell }) {
  const focal = manifests.get(cell.id);
  const problems = [];
  for (const claim of evaluationClaims(graph, cell, focal)) {
    if (claim.id === cell.claim) continue;
    const dependency = dependencyCell(cells, claim.id, focal.evidence.identity.target);
    if ((problemsByCell.get(dependency.id) ?? []).length > 0) {
      problems.push(`dependency cell ${dependency.id} failed closeout validation`);
    }
  }
  return problems;
}

function prePublishLedgerProblems({ prePublishLedger, graphSha256, gitIdentity, version }) {
  const problems = [];
  if (prePublishLedger === null) {
    return ["post-publish closeout requires the accepted pre-publish ledger"];
  }
  if (prePublishLedger.schema !== "codestory.release-closeout-ledger/v1"
      || prePublishLedger.phase !== "pre_publish"
      || prePublishLedger.decision !== "accept") {
    problems.push("pre-publish ledger must be an accepted pre_publish closeout ledger");
  }
  if (prePublishLedger.graph_sha256 !== graphSha256) problems.push("pre-publish ledger graph identity changed");
  if (prePublishLedger.version !== version) problems.push("pre-publish ledger version changed");
  if (!SHA256.test(String(prePublishLedger.producer_provenance_sha256 ?? ""))) {
    problems.push("pre-publish ledger omitted authenticated producer provenance");
  }
  if (!SHA256.test(String(prePublishLedger.trusted_exceptions_sha256 ?? ""))) {
    problems.push("pre-publish ledger omitted trusted exception provenance");
  }
  for (const key of ["repository", "commit", "source_tree"]) {
    if (prePublishLedger.identity?.[key] !== gitIdentity[key]) {
      problems.push(`pre-publish ledger ${key} identity changed`);
    }
  }
  return problems;
}

function retainedPackageRow({ prePublishLedger, target, problems }) {
  if (prePublishLedger === null) return null;
  const packageCellId = `package_identity:${target}`;
  const packageRows = Array.isArray(prePublishLedger.cells)
    ? prePublishLedger.cells.filter(({ id }) => id === packageCellId)
    : [];
  if (packageRows.length !== 1) {
    problems.push(`pre-publish ledger must contain one ${packageCellId}`);
    return null;
  }
  return packageRows[0];
}

function compareRetainedPackageManifest({
  manifest,
  cell,
  prePublishLedger,
  graphSha256,
  gitIdentity,
  version,
}) {
  const problems = prePublishLedgerProblems({
    prePublishLedger,
    graphSha256,
    gitIdentity,
    version,
  });
  const packageRow = retainedPackageRow({
    prePublishLedger,
    target: cell.identity_constraints.target,
    problems,
  });
  if (packageRow === null) return problems;
  const manifestSha256 = digest(canonicalJson(manifest));
  if (manifestSha256 !== packageRow.manifest?.sha256) {
    problems.push("post-publish package manifest does not match the retained pre-publish manifest");
  }
  for (const key of ["name", "sha256", "bytes"]) {
    if (manifest.archive?.[key] !== packageRow.archive?.[key]) {
      problems.push(`post-publish package archive ${key} does not match the retained pre-publish archive`);
    }
  }
  return problems;
}

function comparePublishedArchive({ manifest, cell, prePublishLedger, graphSha256, gitIdentity, version }) {
  const problems = prePublishLedgerProblems({
    prePublishLedger,
    graphSha256,
    gitIdentity,
    version,
  });
  const target = cell.identity_constraints.target;
  const packageCellId = `package_identity:${target}`;
  const packageRow = retainedPackageRow({ prePublishLedger, target, problems });
  if (packageRow === null) return problems;
  const comparison = manifest.comparison ?? {};
  if (comparison.pre_publish_cell_id !== packageCellId) {
    problems.push(`comparison pre_publish_cell_id must be ${packageCellId}`);
  }
  if (comparison.pre_publish_manifest_sha256 !== packageRow.manifest?.sha256) {
    problems.push("comparison pre-publish manifest digest does not match the retained ledger manifest");
  }
  if (comparison.pre_publish_artifact_sha256 !== packageRow.archive?.sha256
      || comparison.published_artifact_sha256 !== packageRow.archive?.sha256) {
    problems.push("published archive bytes are not identical to the retained pre-publish archive");
  }
  if (comparison.published_artifact_name !== packageRow.archive?.name) {
    problems.push("published archive name does not match the retained pre-publish archive");
  }
  return problems;
}

function indexManifests(inputManifests) {
  const byCell = new Map();
  const errors = [];
  for (const [index, value] of inputManifests.entries()) {
    let manifest;
    try {
      manifest = object(value, `manifest[${index}]`);
    } catch (error) {
      errors.push(error.message);
      continue;
    }
    const cellId = typeof manifest.cell_id === "string" ? manifest.cell_id : `<manifest-${index}>`;
    const rows = byCell.get(cellId) ?? [];
    rows.push(manifest);
    byCell.set(cellId, rows);
  }
  for (const [cellId, rows] of byCell) {
    if (rows.length > 1) errors.push(`closeout contains duplicate manifest for ${cellId}`);
  }
  return { byCell, errors };
}

function indexArtifactBindings(inputBindings) {
  const byCell = new Map();
  const errors = [];
  if (!Array.isArray(inputBindings)) {
    return { byCell, errors: ["closeout requires downloaded artifact bindings"] };
  }
  for (const [index, binding] of inputBindings.entries()) {
    if (binding === null || typeof binding !== "object" || Array.isArray(binding)) {
      errors.push(`artifact binding[${index}] must be an object`);
      continue;
    }
    if (typeof binding.cell_id !== "string" || binding.cell_id === "") {
      errors.push(`artifact binding[${index}].cell_id must be non-empty`);
      continue;
    }
    if (byCell.has(binding.cell_id)) {
      errors.push(`artifact bindings duplicate ${binding.cell_id}`);
    } else {
      byCell.set(binding.cell_id, binding);
    }
  }
  return { byCell, errors };
}

function artifactBindingProblems(manifest, binding, trustedProducer) {
  if (!binding) return ["manifest is not bound to one selected Actions artifact container"];
  if (!trustedProducer) return ["manifest artifact has no authenticated producer"];
  const problems = [];
  const expected = {
    producer_artifact: trustedProducer.producer_artifact,
    artifact_id: trustedProducer.artifact?.id,
    artifact_digest: trustedProducer.artifact?.digest,
  };
  for (const [key, value] of Object.entries(expected)) {
    if (binding[key] !== value) problems.push(`manifest ${key} does not match Actions provenance`);
  }
  if (binding.manifest_sha256 !== digest(canonicalJson(manifest))) {
    problems.push("downloaded manifest bytes changed after artifact extraction");
  }
  return problems;
}

export function evaluateReleaseCloseout({
  graph,
  phase,
  version,
  evaluatedAt,
  gitIdentity,
  manifests: inputManifests,
  prePublishLedger = null,
  trustedProducers = null,
  trustedExceptionDocument = null,
  artifactBindings = null,
}) {
  if (!SEMVER.test(version)) fail("version must be semantic version text without a leading v");
  const evaluatedEpoch = Date.parse(evaluatedAt);
  if (!Number.isFinite(evaluatedEpoch) || new Date(evaluatedEpoch).toISOString() !== evaluatedAt) {
    fail("evaluatedAt must be a canonical ISO timestamp");
  }
  const graphSha256 = releaseClaimGraphDigest(graph);
  const cells = deriveReleaseCells(graph, phase);
  const trusted = trustedProducerIndex({ trustedProducers, cells, gitIdentity, graph, phase });
  const performanceCell = cells.find(({ id }) => id === graph.exception_policy.eligible_evidence_type);
  const trustedException = trustedExceptionInput({
    document: trustedExceptionDocument,
    graph,
    graphSha256,
    gitIdentity,
    version,
    trustedProducer: performanceCell ? trusted.byCell.get(performanceCell.id) : null,
  });
  const cellIds = new Set(cells.map(({ id }) => id));
  const indexed = indexManifests(inputManifests);
  const bindings = indexArtifactBindings(artifactBindings);
  const inputErrors = [
    ...indexed.errors,
    ...trusted.errors,
    ...trustedException.errors,
    ...bindings.errors,
  ];
  for (const cellId of indexed.byCell.keys()) {
    if (!cellIds.has(cellId)) inputErrors.push(`closeout contains undeclared cell ${cellId}`);
  }
  for (const cellId of bindings.byCell.keys()) {
    if (!cellIds.has(cellId)) inputErrors.push(`closeout contains undeclared artifact binding ${cellId}`);
  }
  const manifests = new Map();
  const problemsByCell = new Map();
  const evidenceIds = new Map();
  for (const cell of cells) {
    const rows = indexed.byCell.get(cell.id) ?? [];
    if (rows.length !== 1) continue;
    const manifest = rows[0];
    const problems = manifestProblems({ manifest, cell, graph, graphSha256, version });
    problems.push(...producerAuthenticationProblems(manifest, trusted.byCell.get(cell.id)));
    problems.push(...artifactBindingProblems(
      manifest,
      bindings.byCell.get(cell.id),
      trusted.byCell.get(cell.id),
    ));
    const evidenceId = manifest.evidence?.id;
    if (typeof evidenceId !== "string" || evidenceId === "") {
      problems.push("manifest evidence id must be a non-empty string");
    } else {
      const owner = evidenceIds.get(evidenceId);
      if (owner !== undefined) {
        problems.push(`manifest evidence id ${evidenceId} is reused from ${owner}`);
        const ownerProblems = problemsByCell.get(owner) ?? [];
        ownerProblems.push(`manifest evidence id ${evidenceId} is reused by ${cell.id}`);
        problemsByCell.set(owner, ownerProblems);
      } else {
        evidenceIds.set(evidenceId, cell.id);
      }
    }
    if (cell.archive_role === "post_publish_compare") {
      problems.push(...comparePublishedArchive({
        manifest,
        cell,
        prePublishLedger,
        graphSha256,
        gitIdentity,
        version,
      }));
    }
    if (phase === "post_publish" && cell.archive_role === "pre_publish") {
      problems.push(...compareRetainedPackageManifest({
        manifest,
        cell,
        prePublishLedger,
        graphSha256,
        gitIdentity,
        version,
      }));
    }
    manifests.set(cell.id, manifest);
    problemsByCell.set(cell.id, [...(problemsByCell.get(cell.id) ?? []), ...problems]);
  }
  const exceptionEvidenceIds = new Set([...manifests.values()]
    .filter(({ evidence }) => evidence?.status === "pass_with_exception")
    .map(({ evidence }) => evidence.id));
  for (const evidenceId of Object.keys(trustedException.exceptions)) {
    if (!exceptionEvidenceIds.has(evidenceId)) {
      inputErrors.push(`trusted exception input contains unused evidence ${evidenceId}`);
    }
  }

  const retainedManifests = new Map();
  const evaluations = new Map();
  const ledgerCells = [];
  for (const cell of cells) {
    const manifest = manifests.get(cell.id);
    if (!manifest) {
      ledgerCells.push({
        id: cell.id,
        phase: cell.phase,
        claim: cell.claim,
        evidence_type: cell.evidence_type,
        status: "missing",
      });
      continue;
    }
    const manifestBytes = canonicalJson(manifest);
    const manifestRecord = {
      path: `manifests/${safeFileName(cell.id)}`,
      sha256: digest(manifestBytes),
      bytes: Buffer.byteLength(manifestBytes),
    };
    retainedManifests.set(cell.id, { value: manifest, bytes: manifestBytes, record: manifestRecord });
    let evaluation;
    const problems = [...(problemsByCell.get(cell.id) ?? [])];
    if (problems.length === 0) {
      try {
        problems.push(...dependencyValidationProblems({ cell, cells, manifests, graph, problemsByCell }));
      } catch (error) {
        problems.push(error.message);
      }
    }
    if (problems.length > 0) {
      evaluation = {
        schema: MANIFEST_EVALUATION_SCHEMA,
        cell_id: cell.id,
        evidence_cells: [cell.id],
        status: "fail",
        failures: [...new Set(problems)].sort(),
      };
    } else {
      try {
        evaluation = evaluateCell({
          cell,
          cells,
          manifests,
          graph,
          gitIdentity,
          evaluatedAt,
          trustedExceptions: trustedException.exceptions,
          trustedExceptionIdentity: trustedException.identity,
        });
      } catch (error) {
        evaluation = {
          schema: MANIFEST_EVALUATION_SCHEMA,
          cell_id: cell.id,
          evidence_cells: [cell.id],
          status: "fail",
          failures: [error.message],
        };
      }
    }
    const evaluationBytes = canonicalJson(evaluation);
    const evaluationRecord = {
      path: `evaluations/${safeFileName(cell.id)}`,
      sha256: digest(evaluationBytes),
      bytes: Buffer.byteLength(evaluationBytes),
    };
    evaluations.set(cell.id, { value: evaluation, bytes: evaluationBytes, record: evaluationRecord });
    ledgerCells.push({
      id: cell.id,
      phase: cell.phase,
      claim: cell.claim,
      evidence_type: cell.evidence_type,
      status: evaluation.status,
      evidence_id: manifest.evidence.id,
      identity: canonicalReleaseClaimValue(manifest.evidence.identity),
      manifest: manifestRecord,
      evaluation: evaluationRecord,
      ...(manifest.archive ? { archive: canonicalReleaseClaimValue(manifest.archive) } : {}),
      ...(manifest.comparison ? { comparison: canonicalReleaseClaimValue(manifest.comparison) } : {}),
    });
  }
  const missingCells = ledgerCells.filter(({ status }) => status === "missing").map(({ id }) => id);
  const failedCells = ledgerCells.filter(({ status }) => status === "fail").map(({ id }) => id);
  inputErrors.sort();
  const decision = inputErrors.length === 0 && missingCells.length === 0 && failedCells.length === 0
    ? "accept"
    : "reject";
  const ledger = {
    schema: graph.closeout.ledger_schema,
    phase,
    decision,
    graph_schema: graph.schema,
    graph_sha256: graphSha256,
    version,
    evaluated_at: evaluatedAt,
    identity: canonicalReleaseClaimValue(gitIdentity),
    producer_provenance_sha256: digest(canonicalJson(trustedProducers)),
    trusted_exceptions_sha256: digest(canonicalJson(trustedExceptionDocument)),
    cells: ledgerCells,
    input_errors: inputErrors,
  };
  const summary = {
    schema: graph.closeout.summary_schema,
    phase,
    decision,
    graph_sha256: graphSha256,
    version,
    identity: canonicalReleaseClaimValue(gitIdentity),
    producer_provenance_sha256: ledger.producer_provenance_sha256,
    trusted_exceptions_sha256: ledger.trusted_exceptions_sha256,
    counts: {
      required: ledgerCells.length,
      passed: ledgerCells.filter(({ status }) => new Set(["pass", "pass_with_exception"]).has(status)).length,
      failed: failedCells.length,
      missing: missingCells.length,
    },
    failed_cells: failedCells,
    missing_cells: missingCells,
    input_errors: inputErrors,
  };
  return {
    decision,
    ledger,
    summary,
    retainedManifests,
    evaluations,
    trustedProducers: canonicalReleaseClaimValue(trustedProducers),
    trustedExceptionDocument: canonicalReleaseClaimValue(trustedExceptionDocument),
  };
}

export function writeReleaseCloseout(outDir, result) {
  const output = path.resolve(outDir);
  if (existsSync(output)) {
    if (!lstatSync(output).isDirectory()) fail(`closeout output ${output} is not a directory`);
    if (readdirSync(output).length > 0) fail(`closeout output ${output} must be absent or empty`);
  }
  mkdirSync(path.join(output, "manifests"), { recursive: true });
  mkdirSync(path.join(output, "evaluations"), { recursive: true });
  for (const { bytes, record } of result.retainedManifests.values()) {
    writeFileSync(path.join(output, record.path), bytes);
  }
  for (const { bytes, record } of result.evaluations.values()) {
    writeFileSync(path.join(output, record.path), bytes);
  }
  writeFileSync(path.join(output, "ledger.json"), canonicalJson(result.ledger));
  writeFileSync(path.join(output, "summary.json"), canonicalJson(result.summary));
  writeFileSync(path.join(output, "producer-provenance.json"), canonicalJson(result.trustedProducers));
  writeFileSync(path.join(output, "trusted-exceptions.json"), canonicalJson(result.trustedExceptionDocument));
}

export function readReleaseCellArtifacts(directory, trustedProducers) {
  const root = path.resolve(directory);
  if (!lstatSync(root).isDirectory()) fail(`manifest directory ${root} is not a directory`);
  const producerRows = Array.isArray(trustedProducers?.producers) ? trustedProducers.producers : [];
  const selectedArtifacts = new Map();
  for (const row of producerRows) {
    const existing = selectedArtifacts.get(row.producer_artifact);
    if (existing && existing.artifact?.id !== row.artifact?.id) {
      fail(`trusted producer map gives ${row.producer_artifact} multiple artifact ids`);
    }
    selectedArtifacts.set(row.producer_artifact, row);
  }
  const entries = readdirSync(root, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    if (!entry.isDirectory() || entry.isSymbolicLink()) {
      fail(`release-cell input ${entry.name} must be one selected artifact directory`);
    }
    if (!selectedArtifacts.has(entry.name)) {
      fail(`release-cell input contains unexpected artifact container ${entry.name}`);
    }
  }
  for (const artifactName of selectedArtifacts.keys()) {
    if (!entries.some(({ name }) => name === artifactName)) {
      fail(`release-cell input is missing selected artifact container ${artifactName}`);
    }
  }
  const manifests = [];
  const artifactBindings = [];
  let trustedExceptionCount = 0;
  for (const [artifactName, producerRow] of selectedArtifacts) {
    const artifactRoot = path.join(root, artifactName);
    const files = readdirSync(artifactRoot, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of files) {
      if (!entry.isFile() || entry.isSymbolicLink() || !entry.name.endsWith(".json")) {
        fail(`artifact container ${artifactName}/${entry.name} must be a regular JSON file`);
      }
      const manifestBytes = readFileSync(path.join(artifactRoot, entry.name), "utf8");
      const manifest = JSON.parse(manifestBytes);
      if (manifestBytes !== canonicalJson(manifest)) {
        fail(`artifact container ${artifactName}/${entry.name} is not canonical JSON bytes`);
      }
      if (manifest.schema === TRUSTED_EXCEPTIONS_SCHEMA) {
        if (entry.name !== "trusted-exceptions.json") {
          fail(`artifact container ${artifactName}/${entry.name} is an unexpected exception document`);
        }
        trustedExceptionCount += 1;
        continue;
      }
      if (manifest.schema !== trustedProducers.manifest_schema) {
        fail(`artifact container ${artifactName}/${entry.name} is not a release-cell manifest`);
      }
      if (!producerRows.some((row) =>
        row.cell_id === manifest.cell_id && row.producer_artifact === artifactName)) {
        fail(`artifact container ${artifactName} does not own ${String(manifest.cell_id)}`);
      }
      manifests.push(manifest);
      artifactBindings.push({
        cell_id: manifest.cell_id,
        producer_artifact: artifactName,
        artifact_id: producerRow.artifact.id,
        artifact_digest: producerRow.artifact.digest,
        manifest_sha256: digest(manifestBytes),
      });
    }
  }
  if (trustedExceptionCount !== 1) {
    fail("selected release-cell containers must contain exactly one trusted-exceptions.json");
  }
  return { manifests, artifactBindings };
}

function parseArgs(argv) {
  const command = argv.shift();
  const values = {};
  while (argv.length > 0) {
    const key = argv.shift();
    const value = argv.shift();
    if (!key?.startsWith("--") || value === undefined) fail("arguments must be --key value pairs");
    const name = key.slice(2);
    if (values[name] !== undefined) fail(`argument --${name} was supplied more than once`);
    values[name] = value;
  }
  return { command, values };
}

function main() {
  const { command, values } = parseArgs(process.argv.slice(2));
  if (command !== "evaluate") fail("command must be evaluate");
  const repoRoot = path.resolve(values.repo ?? path.resolve(path.dirname(process.argv[1]), ".."));
  const graph = loadReleaseClaimGraph(repoRoot);
  const gitIdentity = deriveTrustedGitIdentity({
    repoRoot,
    expectedSha: text(values["expected-sha"], "--expected-sha"),
  });
  const phase = text(values.phase, "--phase");
  const prePublishLedger = values["pre-publish-ledger"]
    ? JSON.parse(readFileSync(values["pre-publish-ledger"], "utf8"))
    : null;
  const trustedProducers = JSON.parse(readFileSync(
    text(values["trusted-producers"], "--trusted-producers"),
    "utf8",
  ));
  const trustedExceptionPath = path.resolve(text(
    values["trusted-exceptions"],
    "--trusted-exceptions",
  ));
  const trustedExceptionDocument = JSON.parse(readFileSync(trustedExceptionPath, "utf8"));
  const performanceProducer = trustedProducers.producers?.find(({ cell_id: cellId }) =>
    cellId === graph.exception_policy.eligible_evidence_type);
  const expectedExceptionParent = path.join(
    path.resolve(text(values["manifest-dir"], "--manifest-dir")),
    String(performanceProducer?.producer_artifact ?? ""),
  );
  if (path.dirname(trustedExceptionPath) !== expectedExceptionParent) {
    fail("trusted exception input must come from the selected release-evidence artifact");
  }
  const downloaded = readReleaseCellArtifacts(
    text(values["manifest-dir"], "--manifest-dir"),
    trustedProducers,
  );
  const result = evaluateReleaseCloseout({
    graph,
    phase,
    version: text(values.version, "--version"),
    evaluatedAt: text(values["evaluated-at"], "--evaluated-at"),
    gitIdentity,
    manifests: downloaded.manifests,
    prePublishLedger,
    trustedProducers,
    trustedExceptionDocument,
    artifactBindings: downloaded.artifactBindings,
  });
  writeReleaseCloseout(text(values["out-dir"], "--out-dir"), result);
  console.log(JSON.stringify(result.summary, null, 2));
  if (result.decision !== "accept") process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();
