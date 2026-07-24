import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  buildTrustedProducerMap,
  produceReleaseCellManifest,
} from "../codestory-release-cell-manifest.mjs";
import {
  deriveReleaseCells,
  resolveReleaseCellConstraints,
} from "../codestory-release-closeout.mjs";
import { loadReleaseClaimGraph } from "../codestory-release-claims.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const graph = loadReleaseClaimGraph(root);
const gitIdentity = {
  repository: "TheGreenCedar/CodeStory",
  commit: "2".repeat(40),
  source_tree: "a".repeat(40),
};
const version = "0.16.0";
const observedAt = "2026-07-19T12:00:00.000Z";

function cell(id) {
  return deriveReleaseCells(graph, "post_publish").find(({ id: candidate }) => candidate === id);
}

function producer(selected) {
  const constraints = resolveReleaseCellConstraints(selected, "2");
  return {
    producer_workflow: constraints.producer_workflow,
    producer_job: constraints.producer_job,
    producer_run_id: "12345",
    producer_run_attempt: "2",
    producer_artifact: constraints.producer_artifact,
  };
}

function actionsMetadata(phase, overrides = []) {
  const artifacts = [];
  const jobsByAttempt = { "1": [], "2": [] };
  const seenArtifacts = new Set();
  const seenJobs = new Set();
  let nextId = 1000;
  const add = (selected, attempt, conclusion = "success") => {
    const constraints = resolveReleaseCellConstraints(selected, String(attempt));
    const hour = attempt === 1 ? "12" : "13";
    const jobKey = `${attempt}:${constraints.producer_job_name}`;
    if (!seenJobs.has(jobKey)) {
      seenJobs.add(jobKey);
      jobsByAttempt[String(attempt)].push({
        id: nextId++,
        run_id: 12345,
        run_attempt: String(attempt),
        head_sha: gitIdentity.commit,
        name: `Release / ${constraints.producer_job_name}`,
        status: "completed",
        conclusion,
        started_at: `2026-07-19T${hour}:00:00.000Z`,
        completed_at: `2026-07-19T${hour}:10:00.000Z`,
      });
    }
    const artifactKey = `${attempt}:${constraints.producer_artifact}`;
    if (conclusion === "success" && !seenArtifacts.has(artifactKey)) {
      seenArtifacts.add(artifactKey);
      artifacts.push({
        id: nextId++,
        name: constraints.producer_artifact,
        digest: `sha256:${String(nextId).padStart(64, "0")}`,
        size_in_bytes: 1024,
        expired: false,
        created_at: `2026-07-19T${hour}:05:00.000Z`,
        expires_at: "2026-08-18T12:05:00.000Z",
        workflow_run: { id: 12345, head_sha: gitIdentity.commit },
      });
    }
  };
  for (const selected of deriveReleaseCells(graph, phase)) add(selected, 1);
  for (const override of overrides) add(cell(override.cell_id), override.attempt, override.conclusion);
  return { artifacts, jobsByAttempt };
}

test("package manifest binds workflow artifact identity separately from archive bytes", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-windows-x64.zip");
  writeFileSync(archive, "release archive bytes");
  const selected = cell("package_identity:windows-x64");
  const manifest = produceReleaseCellManifest({
    graph,
    gitIdentity,
    version,
    cell: selected,
    identity: {},
    producer: producer(selected),
    observedAt,
    archivePath: archive,
  });
  assert.equal(
    manifest.evidence.identity.producer_artifact,
    "release-cell-prepublish-package-windows-x64-attempt-2",
  );
  assert.equal(manifest.archive.name, "codestory-cli-v0.16.0-windows-x64.zip");
  assert.equal(manifest.archive.sha256, manifest.evidence.identity.artifact_sha256);
});

test("post-publish byte producer refuses an archive that differs from the accepted ledger", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-windows-x64.zip");
  writeFileSync(archive, "published replacement bytes");
  const selected = cell("post_publish_bytes:windows-x64");
  const prePublishLedger = {
    phase: "pre_publish",
    decision: "accept",
    cells: [{
      id: "package_identity:windows-x64",
      manifest: { sha256: "b".repeat(64) },
      archive: {
        name: path.basename(archive),
        sha256: "c".repeat(64),
        bytes: 10,
      },
    }],
  };
  assert.throws(
    () => produceReleaseCellManifest({
      graph,
      gitIdentity,
      version,
      cell: selected,
      identity: {},
      producer: producer(selected),
      observedAt,
      archivePath: archive,
      prePublishLedger,
    }),
    /published archive bytes do not match/u,
  );
});

test("all graph-derived cells fix workflow, job name, and attempt-qualified artifact identity", () => {
  for (const selected of deriveReleaseCells(graph, "post_publish")) {
    for (const key of ["producer_workflow", "producer_job", "producer_job_name", "producer_artifact"]) {
      assert.equal(typeof selected.identity_constraints[key], "string", `${selected.id} lacks ${key}`);
      assert.notEqual(selected.identity_constraints[key], "");
    }
    assert.match(selected.identity_constraints.producer_artifact, /-attempt-\{attempt\}$/u);
  }
});

test("Actions provenance recovers a split rerun by selecting each job's latest execution", () => {
  const metadata = actionsMetadata("post_publish", [{
    cell_id: "platform_support:windows-x64",
    attempt: 2,
    conclusion: "success",
  }]);
  const map = buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "post_publish",
    runId: "12345",
    currentRunAttempt: "2",
    ...metadata,
  });
  assert.equal(map.producers.length, 13);
  assert.equal(map.artifacts.length, 9);
  for (const id of [
    "platform_support:windows-x64",
    "installed_runtime_behavior:windows-x64",
    "post_publish_bytes:windows-x64",
  ]) {
    assert.equal(map.producers.find(({ cell_id: cellId }) => cellId === id).producer_run_attempt, "2");
  }
  assert.equal(
    map.producers.find(({ cell_id: cellId }) => cellId === "source_behavior").producer_run_attempt,
    "1",
  );
});

test("Actions provenance rejects a failed latest rerun instead of reusing older proof", () => {
  const metadata = actionsMetadata("post_publish", [{
    cell_id: "platform_support:windows-x64",
    attempt: 2,
    conclusion: "failure",
  }]);
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "post_publish",
    runId: "12345",
    currentRunAttempt: "2",
    ...metadata,
  }), /latest execution of Published windows-x64 smoke.*did not succeed/u);
});

test("Actions provenance rejects duplicate or out-of-window artifact containers", () => {
  const duplicate = actionsMetadata("pre_publish");
  duplicate.artifacts.push(structuredClone(duplicate.artifacts[0]));
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...duplicate,
  }), /must retain one .* artifact/u);

  const outsideWindow = actionsMetadata("pre_publish");
  outsideWindow.artifacts[0].created_at = "2026-07-19T14:00:00.000Z";
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...outsideWindow,
  }), /was not created by the selected job window/u);
});

test("Actions provenance rejects missing, expired, malformed, stale, and future metadata", () => {
  const cases = [
    ["missing artifact", metadata => {
      metadata.artifacts.shift();
    }, /must retain one .* artifact/u],
    ["expired artifact", metadata => {
      metadata.artifacts[0].expired = true;
    }, /has stale run provenance/u],
    ["invalid digest", metadata => {
      metadata.artifacts[0].digest = "sha256:invalid";
    }, /has no SHA-256 container digest/u],
    ["wrong artifact run", metadata => {
      metadata.artifacts[0].workflow_run.id = 999;
    }, /has stale run provenance/u],
    ["wrong artifact commit", metadata => {
      metadata.artifacts[0].workflow_run.head_sha = "f".repeat(40);
    }, /has stale run provenance/u],
    ["wrong job run", metadata => {
      metadata.jobsByAttempt["1"][0].run_id = 999;
    }, /is not bound to the selected run and commit/u],
    ["wrong job commit", metadata => {
      metadata.jobsByAttempt["1"][0].head_sha = "f".repeat(40);
    }, /is not bound to the selected run and commit/u],
    ["duplicate job", metadata => {
      metadata.jobsByAttempt["1"].push(structuredClone(metadata.jobsByAttempt["1"][0]));
    }, /multiple executions/u],
  ];
  for (const [label, mutate, pattern] of cases) {
    const metadata = actionsMetadata("pre_publish");
    mutate(metadata);
    assert.throws(() => buildTrustedProducerMap({
      graph,
      gitIdentity,
      phase: "pre_publish",
      runId: "12345",
      currentRunAttempt: "1",
      ...metadata,
    }), pattern, label);
  }

  const future = actionsMetadata("pre_publish");
  future.jobsByAttempt["1"][0].run_attempt = "2";
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...future,
  }), /no execution/u);
});
