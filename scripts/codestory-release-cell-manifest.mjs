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
  resolveReleaseCellConstraints,
  validateReleaseCellManifest,
} from "./codestory-release-closeout.mjs";

const PRODUCER_MAP_SCHEMA = "codestory.release-actions-provenance/v1";
const ACTIONS_DIGEST = /^sha256:[0-9a-f]{64}$/u;

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
    ...resolveReleaseCellConstraints(cell, producer.producer_run_attempt),
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

function positiveInteger(value, label) {
  const selected = text(String(value ?? ""), label);
  if (!/^[1-9]\d*$/u.test(selected)) fail(`${label} must be a positive integer`);
  return selected;
}

function actionsTimestamp(value, label) {
  const selected = text(value, label);
  if (!Number.isFinite(Date.parse(selected))) fail(`${label} must be an Actions timestamp`);
  return selected;
}

function leafJobName(value) {
  return text(value, "Actions job name").split(" / ").at(-1);
}

function flattenJobs(jobsByAttempt) {
  if (Array.isArray(jobsByAttempt)) return jobsByAttempt;
  if (jobsByAttempt === null || typeof jobsByAttempt !== "object") {
    fail("Actions jobs must be grouped by run attempt");
  }
  return Object.entries(jobsByAttempt).flatMap(([attempt, jobs]) => {
    if (!Array.isArray(jobs)) fail(`Actions jobs attempt ${attempt} must be an array`);
    return jobs.map((job) => ({ ...job, run_attempt: String(job.run_attempt ?? attempt) }));
  });
}

export function buildTrustedProducerMap({
  graph,
  gitIdentity,
  phase,
  runId,
  currentRunAttempt,
  artifacts,
  jobsByAttempt,
}) {
  const selectedRunId = positiveInteger(runId, "Actions run id");
  const selectedCurrentAttempt = positiveInteger(currentRunAttempt, "Actions current run attempt");
  if (!Array.isArray(artifacts)) fail("Actions artifacts must be an array");
  const jobs = flattenJobs(jobsByAttempt);
  const selectionCache = new Map();
  const producers = deriveReleaseCells(graph, phase).map((cell) => {
    const jobName = text(cell.identity_constraints.producer_job_name, `${cell.id} producer job name`);
    const cacheKey = `${jobName}\0${cell.identity_constraints.producer_artifact}`;
    let selected = selectionCache.get(cacheKey);
    if (!selected) {
      const occurrences = jobs.filter((job) =>
        leafJobName(job.name) === jobName
        && Number(positiveInteger(job.run_attempt, `${jobName} run attempt`))
          <= Number(selectedCurrentAttempt));
      if (occurrences.length === 0) fail(`Actions run has no execution of ${jobName}`);
      const latestAttempt = Math.max(...occurrences.map(({ run_attempt: attempt }) => Number(attempt)));
      const latestJobs = occurrences.filter(({ run_attempt: attempt }) => Number(attempt) === latestAttempt);
      if (latestJobs.length !== 1) {
        fail(`Actions run attempt ${latestAttempt} has multiple executions of ${jobName}`);
      }
      const job = latestJobs[0];
      if (job.status !== "completed" || job.conclusion !== "success") {
        fail(`latest execution of ${jobName} in attempt ${latestAttempt} did not succeed`);
      }
      if (String(job.run_id) !== selectedRunId || job.head_sha !== gitIdentity.commit) {
        fail(`Actions job ${jobName} is not bound to the selected run and commit`);
      }
      const constraints = resolveReleaseCellConstraints(cell, String(latestAttempt));
      const matchingArtifacts = artifacts.filter(({ name }) => name === constraints.producer_artifact);
      if (matchingArtifacts.length !== 1) {
        fail(`Actions run must retain one ${constraints.producer_artifact} artifact`);
      }
      const artifact = matchingArtifacts[0];
      if (artifact.expired !== false
          || String(artifact.workflow_run?.id) !== selectedRunId
          || artifact.workflow_run?.head_sha !== gitIdentity.commit) {
        fail(`Actions artifact ${constraints.producer_artifact} has stale run provenance`);
      }
      if (!ACTIONS_DIGEST.test(String(artifact.digest ?? ""))) {
        fail(`Actions artifact ${constraints.producer_artifact} has no SHA-256 container digest`);
      }
      if (!Number.isSafeInteger(artifact.size_in_bytes) || artifact.size_in_bytes <= 0) {
        fail(`Actions artifact ${constraints.producer_artifact} has invalid size`);
      }
      const createdAt = actionsTimestamp(artifact.created_at, "Actions artifact created_at");
      const startedAt = actionsTimestamp(job.started_at, "Actions job started_at");
      const completedAt = actionsTimestamp(job.completed_at, "Actions job completed_at");
      if (Date.parse(createdAt) < Date.parse(startedAt)
          || Date.parse(createdAt) > Date.parse(completedAt)) {
        fail(`Actions artifact ${constraints.producer_artifact} was not created by the selected job window`);
      }
      selected = {
        constraints,
        artifact: {
          id: positiveInteger(artifact.id, "Actions artifact id"),
          name: artifact.name,
          digest: artifact.digest,
          size_in_bytes: artifact.size_in_bytes,
          expired: false,
          created_at: createdAt,
          expires_at: actionsTimestamp(artifact.expires_at, "Actions artifact expires_at"),
          workflow_run_id: selectedRunId,
          head_sha: gitIdentity.commit,
        },
        job: {
          id: positiveInteger(job.id, "Actions job id"),
          run_id: selectedRunId,
          head_sha: gitIdentity.commit,
          name: job.name,
          status: job.status,
          conclusion: job.conclusion,
          run_attempt: String(latestAttempt),
          started_at: startedAt,
          completed_at: completedAt,
        },
      };
      selectionCache.set(cacheKey, selected);
    }
    return {
      cell_id: cell.id,
      producer_workflow: selected.constraints.producer_workflow,
      producer_job: selected.constraints.producer_job,
      producer_job_name: selected.constraints.producer_job_name,
      producer_run_id: selectedRunId,
      producer_run_attempt: selected.job.run_attempt,
      producer_artifact: selected.constraints.producer_artifact,
      artifact: selected.artifact,
      job: selected.job,
    };
  });
  return {
    schema: PRODUCER_MAP_SCHEMA,
    phase,
    manifest_schema: graph.closeout.manifest_schema,
    graph_sha256: releaseClaimGraphDigest(graph),
    identity: gitIdentity,
    run_id: selectedRunId,
    current_run_attempt: selectedCurrentAttempt,
    producers,
    artifacts: [...new Map(producers.map((row) => [row.artifact.id, row.artifact])).values()],
  };
}

async function githubPages(url, token, field) {
  const values = [];
  for (let page = 1; ; page += 1) {
    const separator = url.includes("?") ? "&" : "?";
    const response = await fetch(`${url}${separator}per_page=100&page=${page}`, {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
    });
    if (!response.ok) fail(`GitHub Actions API ${response.status} for ${url}`);
    const document = await response.json();
    const rows = document[field];
    if (!Array.isArray(rows)) fail(`GitHub Actions API omitted ${field}`);
    values.push(...rows);
    if (rows.length < 100) return values;
  }
}

async function produceMap(values) {
  const { graph, gitIdentity } = common(values);
  const phase = text(values.phase, "--phase");
  const runId = positiveInteger(values["producer-run-id"], "--producer-run-id");
  const runAttempt = positiveInteger(values["producer-run-attempt"], "--producer-run-attempt");
  if (process.env.GITHUB_ACTIONS !== "true"
      || process.env.GITHUB_REPOSITORY !== gitIdentity.repository
      || process.env.GITHUB_SHA !== gitIdentity.commit
      || process.env.GITHUB_RUN_ID !== runId
      || process.env.GITHUB_RUN_ATTEMPT !== runAttempt) {
    fail("current workflow context does not authenticate the Actions provenance query");
  }
  const token = text(process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN, "GitHub Actions token");
  const apiRoot = text(process.env.GITHUB_API_URL ?? "https://api.github.com", "GitHub API URL");
  const repositoryPath = gitIdentity.repository.split("/").map(encodeURIComponent).join("/");
  const runUrl = `${apiRoot}/repos/${repositoryPath}/actions/runs/${runId}`;
  const artifacts = await githubPages(`${runUrl}/artifacts`, token, "artifacts");
  const jobsByAttempt = {};
  for (let attempt = 1; attempt <= Number(runAttempt); attempt += 1) {
    jobsByAttempt[String(attempt)] = (await githubPages(
      `${runUrl}/attempts/${attempt}/jobs`,
      token,
      "jobs",
    )).map((job) => ({ ...job, run_attempt: String(attempt) }));
  }
  const map = buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase,
    runId,
    currentRunAttempt: runAttempt,
    artifacts,
    jobsByAttempt,
  });
  writeJson(text(values.out, "--out"), map);
}

async function main() {
  const { command, values } = parseArgs(process.argv.slice(2));
  if (command === "produce") produceOne(values);
  else if (command === "producer-map") await produceMap(values);
  else fail("command must be produce or producer-map");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main();
}
