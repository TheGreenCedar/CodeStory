#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  canonicalReleaseClaimValue,
  deriveTrustedGitIdentity,
  loadReleaseClaimGraph,
  releaseClaimGraphDigest,
} from "./codestory-release-claims.mjs";
import {
  deriveReleaseCells,
  validateReleaseCellManifest,
} from "./codestory-release-closeout.mjs";

const PRODUCER_MAP_SCHEMA = "codestory.release-producer-map/v1";

function fail(message) {
  throw new Error(message);
}

function text(value, label) {
  if (typeof value !== "string" || value.trim() !== value || value === "") {
    fail(`${label} must be non-empty trimmed text`);
  }
  return value;
}

function canonicalJson(value) {
  return `${JSON.stringify(canonicalReleaseClaimValue(value), null, 2)}\n`;
}

function fileAttestation(filePath) {
  const absolute = path.resolve(text(filePath, "artifact path"));
  const stat = statSync(absolute);
  if (!stat.isFile() || stat.size <= 0) fail(`artifact ${absolute} must be a non-empty regular file`);
  const bytes = readFileSync(absolute);
  return {
    path: absolute,
    name: path.basename(absolute),
    sha256: createHash("sha256").update(bytes).digest("hex"),
    bytes: bytes.length,
  };
}

function writeJson(filePath, value) {
  const absolute = path.resolve(text(filePath, "output path"));
  mkdirSync(path.dirname(absolute), { recursive: true });
  writeFileSync(absolute, canonicalJson(value));
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

function canonicalTimestamp(value, label) {
  const selected = value ?? new Date().toISOString();
  const parsed = Date.parse(selected);
  if (!Number.isFinite(parsed) || new Date(parsed).toISOString() !== selected) {
    fail(`${label} must be a canonical ISO timestamp`);
  }
  return selected;
}

function authenticatedProducer(values, gitIdentity, { requireJob = true } = {}) {
  const producer = {
    producer_workflow: text(values["producer-workflow"], "--producer-workflow"),
    producer_job: text(values["producer-job"], "--producer-job"),
    producer_run_id: text(values["producer-run-id"], "--producer-run-id"),
    producer_run_attempt: text(values["producer-run-attempt"], "--producer-run-attempt"),
    producer_artifact: text(values["producer-artifact"], "--producer-artifact"),
  };
  if (process.env.GITHUB_ACTIONS === "true") {
    const expected = {
      GITHUB_REPOSITORY: gitIdentity.repository,
      GITHUB_SHA: gitIdentity.commit,
      GITHUB_RUN_ID: producer.producer_run_id,
      GITHUB_RUN_ATTEMPT: producer.producer_run_attempt,
      ...(requireJob ? { GITHUB_JOB: producer.producer_job } : {}),
    };
    for (const [name, expectedValue] of Object.entries(expected)) {
      if (process.env[name] !== expectedValue) {
        fail(`${name} does not authenticate the requested release-cell producer`);
      }
    }
  }
  return producer;
}

function selectedCell(graph, cellId) {
  const matches = deriveReleaseCells(graph, "post_publish").filter(({ id }) => id === cellId);
  if (matches.length !== 1) fail(`release claim graph does not derive one cell ${cellId}`);
  return matches[0];
}

function evidenceType(graph, id) {
  const matches = graph.evidence_types.filter(({ id: candidate }) => candidate === id);
  if (matches.length !== 1) fail(`release claim graph does not define one evidence type ${id}`);
  return matches[0];
}

function retainedPackageRow(prePublishLedger, target) {
  const rows = Array.isArray(prePublishLedger?.cells)
    ? prePublishLedger.cells.filter(({ id }) => id === `package_identity:${target}`)
    : [];
  if (prePublishLedger?.decision !== "accept" || prePublishLedger?.phase !== "pre_publish" || rows.length !== 1) {
    fail(`accepted pre-publish ledger must retain one package_identity:${target}`);
  }
  return rows[0];
}

export function produceReleaseCellManifest({
  graph,
  gitIdentity,
  version,
  cell,
  identity: suppliedIdentity,
  producer,
  observedAt,
  artifactPath = null,
  archivePath = null,
  prePublishLedger = null,
  evidence = null,
}) {
  const graphSha256 = releaseClaimGraphDigest(graph);
  const type = evidenceType(graph, cell.evidence_type);
  const observed = canonicalTimestamp(evidence?.observed_at ?? observedAt, "observed_at");
  const expires = evidence?.expires_at ?? new Date(
    Date.parse(observed) + type.maximum_validity_hours * 60 * 60 * 1000,
  ).toISOString();
  const identity = {
    ...(evidence?.identity ?? {}),
    ...(suppliedIdentity ?? {}),
    ...gitIdentity,
    ...cell.identity_constraints,
    ...producer,
  };
  if (cell.required_identity.includes("producer_version")) identity.producer_version = version;
  if (cell.required_identity.includes("runtime_version")) identity.runtime_version = version;

  const artifact = archivePath ? fileAttestation(archivePath) : artifactPath ? fileAttestation(artifactPath) : null;
  if (artifact && cell.required_identity.includes("artifact_sha256")) {
    identity.artifact_sha256 = artifact.sha256;
  }
  const manifest = {
    schema: graph.closeout.manifest_schema,
    cell_id: cell.id,
    phase: cell.phase,
    version,
    graph_sha256: graphSha256,
    evidence: {
      ...(evidence ?? {}),
      id: evidence?.id ?? `${cell.id}:${producer.producer_run_id}:${producer.producer_run_attempt}`,
      type: cell.evidence_type,
      tier: type.tier,
      status: evidence?.status ?? "pass",
      graph_sha256: graphSha256,
      observed_at: observed,
      expires_at: expires,
      identity,
    },
  };
  if (cell.archive_role === "pre_publish") {
    if (!artifact || !archivePath) fail(`${cell.id} requires --archive`);
    manifest.archive = { name: artifact.name, sha256: artifact.sha256, bytes: artifact.bytes };
  } else if (cell.archive_role === "post_publish_compare") {
    if (!artifact || !archivePath) fail(`${cell.id} requires --archive`);
    const packageRow = retainedPackageRow(prePublishLedger, identity.target);
    identity.pre_publish_artifact_sha256 = packageRow.archive.sha256;
    manifest.comparison = {
      pre_publish_cell_id: packageRow.id,
      pre_publish_manifest_sha256: packageRow.manifest.sha256,
      pre_publish_artifact_sha256: packageRow.archive.sha256,
      published_artifact_name: artifact.name,
      published_artifact_sha256: artifact.sha256,
    };
    if (artifact.name !== packageRow.archive.name || artifact.sha256 !== packageRow.archive.sha256
        || artifact.bytes !== packageRow.archive.bytes) {
      fail(`${cell.id} published archive bytes do not match the retained pre-publish archive`);
    }
  }
  const problems = validateReleaseCellManifest({ manifest, cell, graph, version });
  if (problems.length > 0) fail(`${cell.id} manifest is invalid: ${problems.join("; ")}`);
  return manifest;
}

function common(values) {
  const repoRoot = path.resolve(values.repo ?? path.resolve(path.dirname(process.argv[1]), ".."));
  const graph = loadReleaseClaimGraph(repoRoot);
  const gitIdentity = deriveTrustedGitIdentity({
    repoRoot,
    expectedSha: text(values["expected-sha"], "--expected-sha"),
  });
  return { repoRoot, graph, gitIdentity };
}

function produceOne(values) {
  const { graph, gitIdentity } = common(values);
  const version = text(values.version, "--version").replace(/^v/u, "");
  const cell = selectedCell(graph, text(values["cell-id"], "--cell-id"));
  const producer = authenticatedProducer(values, gitIdentity);
  const identity = values.identity ? JSON.parse(readFileSync(values.identity, "utf8")) : {};
  const prePublishLedger = values["pre-publish-ledger"]
    ? JSON.parse(readFileSync(values["pre-publish-ledger"], "utf8"))
    : null;
  const manifest = produceReleaseCellManifest({
    graph,
    gitIdentity,
    version,
    cell,
    identity,
    producer,
    observedAt: values["observed-at"],
    artifactPath: values.artifact,
    archivePath: values.archive,
    prePublishLedger,
  });
  writeJson(text(values.out, "--out"), manifest);
}

function produceReleaseEvidence(values) {
  const { graph, gitIdentity } = common(values);
  const version = text(values.version, "--version").replace(/^v/u, "");
  const producer = authenticatedProducer(values, gitIdentity);
  const report = JSON.parse(readFileSync(text(values.report, "--report"), "utf8"));
  if (!new Set(["pass", "pass_with_exception"]).has(report.status)) {
    fail("release evidence report must be accepted before manifests are emitted");
  }
  const rows = Array.isArray(report.release_cell_evidence) ? report.release_cell_evidence : [];
  const outDir = path.resolve(text(values["out-dir"], "--out-dir"));
  for (const cellId of ["retrieval_readiness", "performance", "answer_quality"]) {
    const matches = rows.filter(({ type }) => type === cellId);
    if (matches.length !== 1) fail(`release evidence report must contain one ${cellId} row`);
    const cell = selectedCell(graph, cellId);
    const manifest = produceReleaseCellManifest({
      graph,
      gitIdentity,
      version,
      cell,
      identity: {},
      producer,
      evidence: matches[0],
    });
    writeJson(path.join(outDir, `${cellId}.json`), manifest);
  }
}

function produceMap(values) {
  const { graph, gitIdentity } = common(values);
  const phase = text(values.phase, "--phase");
  const runId = text(values["producer-run-id"], "--producer-run-id");
  const runAttempt = text(values["producer-run-attempt"], "--producer-run-attempt");
  if (process.env.GITHUB_ACTIONS === "true") {
    if (process.env.GITHUB_REPOSITORY !== gitIdentity.repository
        || process.env.GITHUB_SHA !== gitIdentity.commit
        || process.env.GITHUB_RUN_ID !== runId
        || process.env.GITHUB_RUN_ATTEMPT !== runAttempt) {
      fail("current workflow context does not authenticate the trusted producer map");
    }
  }
  const producers = deriveReleaseCells(graph, phase).map((cell) => ({
    cell_id: cell.id,
    producer_workflow: cell.identity_constraints.producer_workflow,
    producer_job: cell.identity_constraints.producer_job,
    producer_run_id: runId,
    producer_run_attempt: runAttempt,
    producer_artifact: cell.identity_constraints.producer_artifact,
  }));
  writeJson(text(values.out, "--out"), {
    schema: PRODUCER_MAP_SCHEMA,
    phase,
    identity: gitIdentity,
    producers,
  });
}

function main() {
  const { command, values } = parseArgs(process.argv.slice(2));
  if (command === "produce") produceOne(values);
  else if (command === "release-evidence") produceReleaseEvidence(values);
  else if (command === "producer-map") produceMap(values);
  else fail("command must be produce, release-evidence, or producer-map");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) main();
