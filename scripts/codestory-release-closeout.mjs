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
const SHA256 = /^[0-9a-f]{64}$/u;
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

export function deriveReleaseCells(graph, phase) {
  const closeout = object(graph.closeout, "release claim graph.closeout");
  const selectedPhase = text(phase, "phase");
  const selectedIndex = phaseIndex(closeout, selectedPhase);
  const cells = [];
  for (const group of closeout.cell_groups) {
    if (phaseIndex(closeout, group.phase) > selectedIndex) continue;
    const add = (suffix, constraints = {}) => {
      const id = suffix === null ? group.id : `${group.id}:${suffix}`;
      cells.push({
        id,
        group_id: group.id,
        phase: group.phase,
        claim: group.claim,
        evidence_type: group.evidence_type,
        required_identity: [...group.required_identity],
        singleton_identity: [...(group.singleton_identity ?? [])],
        identity_constraints: {
          ...(group.identity_constraints ?? {}),
          ...constraints,
        },
        archive_role: group.archive_role,
      });
    };
    if (group.expansion === "singleton") {
      add(null);
    } else if (group.expansion === "package_matrix") {
      for (const row of graph.workflow_policy.package_matrix) {
        add(row.asset_target, { target: row.asset_target });
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
  for (const [key, expected] of Object.entries(cell.identity_constraints)) {
    if (identity[key] !== expected) {
      problems.push(`manifest identity ${key} must equal ${expected}`);
    }
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
      if (archive.name !== identity.producer_artifact) {
        problems.push("archive name must match evidence producer_artifact");
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
    }
  } else if (manifest.archive !== undefined || manifest.comparison !== undefined) {
    problems.push("non-archive closeout cells must not contain archive attestations");
  }
  return problems;
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

function evaluateCell({ cell, cells, manifests, graph, gitIdentity, evaluatedAt }) {
  const focal = manifests.get(cell.id);
  const claims = transitiveClaims(graph, cell.claim);
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
  const expectedIdentity = { ...focal.evidence.identity, ...gitIdentity };
  const evaluation = evaluateReleaseClaims({
    graph,
    requested_claims: requestedClaims,
    evidence,
    expected: {
      commit: gitIdentity.commit,
      evaluated_at: evaluatedAt,
      identity: expectedIdentity,
      exceptions: {},
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

function comparePublishedArchive({ manifest, cell, prePublishLedger, graphSha256, gitIdentity, version }) {
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
  for (const key of ["repository", "commit", "source_tree"]) {
    if (prePublishLedger.identity?.[key] !== gitIdentity[key]) {
      problems.push(`pre-publish ledger ${key} identity changed`);
    }
  }
  const target = cell.identity_constraints.target;
  const packageCellId = `package_identity:${target}`;
  const packageRows = Array.isArray(prePublishLedger.cells)
    ? prePublishLedger.cells.filter(({ id }) => id === packageCellId)
    : [];
  if (packageRows.length !== 1) {
    problems.push(`pre-publish ledger must contain one ${packageCellId}`);
    return problems;
  }
  const packageRow = packageRows[0];
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
  if (manifest.evidence.identity.producer_artifact !== packageRow.archive?.name) {
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

export function evaluateReleaseCloseout({
  graph,
  phase,
  version,
  evaluatedAt,
  gitIdentity,
  manifests: inputManifests,
  prePublishLedger = null,
}) {
  if (!SEMVER.test(version)) fail("version must be semantic version text without a leading v");
  const evaluatedEpoch = Date.parse(evaluatedAt);
  if (!Number.isFinite(evaluatedEpoch) || new Date(evaluatedEpoch).toISOString() !== evaluatedAt) {
    fail("evaluatedAt must be a canonical ISO timestamp");
  }
  const graphSha256 = releaseClaimGraphDigest(graph);
  const cells = deriveReleaseCells(graph, phase);
  const cellIds = new Set(cells.map(({ id }) => id));
  const indexed = indexManifests(inputManifests);
  const inputErrors = [...indexed.errors];
  for (const cellId of indexed.byCell.keys()) {
    if (!cellIds.has(cellId)) inputErrors.push(`closeout contains undeclared cell ${cellId}`);
  }
  const manifests = new Map();
  const problemsByCell = new Map();
  const evidenceIds = new Map();
  for (const cell of cells) {
    const rows = indexed.byCell.get(cell.id) ?? [];
    if (rows.length !== 1) continue;
    const manifest = rows[0];
    const problems = manifestProblems({ manifest, cell, graph, graphSha256, version });
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
    manifests.set(cell.id, manifest);
    problemsByCell.set(cell.id, [...(problemsByCell.get(cell.id) ?? []), ...problems]);
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
    const problems = problemsByCell.get(cell.id) ?? [];
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
        evaluation = evaluateCell({ cell, cells, manifests, graph, gitIdentity, evaluatedAt });
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
  return { decision, ledger, summary, retainedManifests, evaluations };
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
}

function readManifestDirectory(directory) {
  const root = path.resolve(directory);
  if (!lstatSync(root).isDirectory()) fail(`manifest directory ${root} is not a directory`);
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.name.endsWith(".json"))
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((entry) => {
      if (!entry.isFile() || entry.isSymbolicLink()) fail(`manifest input ${entry.name} must be a regular file`);
      return JSON.parse(readFileSync(path.join(root, entry.name), "utf8"));
    });
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
  const result = evaluateReleaseCloseout({
    graph,
    phase,
    version: text(values.version, "--version"),
    evaluatedAt: text(values["evaluated-at"], "--evaluated-at"),
    gitIdentity,
    manifests: readManifestDirectory(text(values["manifest-dir"], "--manifest-dir")),
    prePublishLedger,
  });
  writeReleaseCloseout(text(values["out-dir"], "--out-dir"), result);
  console.log(JSON.stringify(result.summary, null, 2));
  if (result.decision !== "accept") process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();
