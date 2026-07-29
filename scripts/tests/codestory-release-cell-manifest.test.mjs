import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
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

/// The CLI authenticates a requested producer against the ambient `GITHUB_*` context whenever
/// `GITHUB_ACTIONS` is "true", so a spawned case must carry an environment that agrees with the
/// arguments it passes. Inheriting the runner's own context instead makes the case pass locally
/// (where the guard is inert) and fail in Actions (where `GITHUB_RUN_ID` is the real run) -- which
/// is exactly how this reached the release lane, the only lane that runs this file.
function producerEnv(overrides) {
  return {
    ...process.env,
    GITHUB_ACTIONS: "true",
    GITHUB_REPOSITORY: gitIdentity.repository,
    GITHUB_SHA: execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim(),
    GITHUB_RUN_ATTEMPT: String(nonClaimPolicy.maximum_run_attempts),
    GITHUB_JOB: nonClaimPolicy.producer_job,
    ...overrides,
  };
}

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
  assert.equal(map.producers.length, 22);
  assert.equal(map.artifacts.length, 16);
  for (const [cellId, artifact] of [
    [
      "retrieval_readiness:macos-arm64",
      "release-cell-postpublish-retrieval-macos-arm64-attempt-1",
    ],
    [
      "retrieval_readiness:windows-x64",
      "release-cell-postpublish-retrieval-windows-x64-attempt-1",
    ],
  ]) {
    assert.equal(
      map.producers.find(({ cell_id: candidate }) => candidate === cellId)
        .producer_artifact,
      artifact,
    );
  }
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

// ── Cross-run evidence reuse ────────────────────────────────────────────────────────────────

/// Metadata for a prior, binding-verified run that produced the reused cells.
function reusedRunMetadata(groupId, { runId = 777, headSha = "b".repeat(40), conclusion = "success", expired = false } = {}) {
  const artifacts = [];
  const jobs = [];
  let nextId = 5000;
  for (const selected of deriveReleaseCells(graph, "pre_publish")) {
    if (selected.group_id !== groupId) continue;
    const constraints = resolveReleaseCellConstraints(selected, "1");
    if (!jobs.some((job) => job.name.endsWith(constraints.producer_job_name))) {
      jobs.push({
        id: nextId++,
        run_id: runId,
        run_attempt: "1",
        head_sha: headSha,
        name: `Release / ${constraints.producer_job_name}`,
        status: "completed",
        conclusion,
        started_at: "2026-07-10T12:00:00.000Z",
        completed_at: "2026-07-10T12:10:00.000Z",
      });
    }
    if (conclusion === "success" && !artifacts.some(({ name }) => name === constraints.producer_artifact)) {
      artifacts.push({
        id: nextId++,
        name: constraints.producer_artifact,
        digest: `sha256:${String(nextId).padStart(64, "0")}`,
        size_in_bytes: 2048,
        expired,
        created_at: "2026-07-10T12:05:00.000Z",
        expires_at: "2026-08-09T12:05:00.000Z",
        workflow_run: { id: runId, head_sha: headSha },
      });
    }
  }
  return { runId: String(runId), headSha, bindingValue: "t".repeat(64), artifacts, jobsByAttempt: jobs };
}

test("a reuse-bound group accepts binding-verified evidence from a prior run", () => {
  const metadata = actionsMetadata("pre_publish");
  // The current run did not execute the source gate at all.
  metadata.jobsByAttempt["1"] = metadata.jobsByAttempt["1"].filter(
    (job) => !job.name.endsWith("full-source-gate"),
  );
  metadata.artifacts = metadata.artifacts.filter(
    ({ name }) => !name.startsWith("release-cell-prepublish-source"),
  );
  const map = buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...metadata,
    reuse: { source_behavior: reusedRunMetadata("source_behavior") },
  });
  const row = map.producers.find(({ cell_id: cellId }) => cellId === "source_behavior");
  assert.equal(row.producer_run_id, "777");
  assert.equal(row.reused_from.binding, "source_tree");
  assert.equal(row.reused_from.head_sha, "b".repeat(40));
  assert.equal(row.artifact.workflow_run_id, "777");
  // Non-reused cells stay bound to the current run.
  const packaged = map.producers.find(({ cell_id: cellId }) => cellId.startsWith("package_identity"));
  assert.equal(packaged.producer_run_id, "12345");
});

test("cross-run evidence is refused for groups without a reuse binding", () => {
  const metadata = actionsMetadata("pre_publish");
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...metadata,
    reuse: { package_identity: reusedRunMetadata("package_identity") },
  }), /package_identity does not admit cross-run evidence/u);
});

test("reused evidence keeps every same-run trust requirement", () => {
  const base = actionsMetadata("pre_publish");
  const withReuse = (reused) => () => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: "1",
    ...base,
    reuse: { source_behavior: reused },
  });

  assert.throws(
    withReuse(reusedRunMetadata("source_behavior", { conclusion: "failure" })),
    /did not succeed/u,
  );
  assert.throws(
    withReuse(reusedRunMetadata("source_behavior", { expired: true })),
    /stale run provenance/u,
  );
  const wrongRun = reusedRunMetadata("source_behavior");
  for (const artifact of wrongRun.artifacts) artifact.workflow_run.id = 999;
  assert.throws(withReuse(wrongRun), /stale run provenance/u);
  const missing = reusedRunMetadata("source_behavior");
  missing.artifacts = [];
  assert.throws(withReuse(missing), /must retain one/u);
});

// ── Withheld accelerator claims ─────────────────────────────────────────────────────────────

const nonClaimPolicy = graph.non_claim_policy;
const linuxHost = nonClaimPolicy.hosts.find(({ id }) => id === "linux-x64-vulkan");

const withheldAttempt = String(nonClaimPolicy.maximum_run_attempts);

/// The Actions job evidence the closeout collects for itself: the annotation, the empty step
/// conclusions, and the log-blob probe. `signature` picks which of the two shapes a failed Linux
/// proof has, and only one of them may route a cell into the withheld lane.
function lostHostEvidence({ signature = "lost", executions = nonClaimPolicy.maximum_run_attempts } = {}) {
  const rows = [];
  for (let attempt = 1; attempt <= executions; attempt += 1) {
    rows.push({
      id: 8000 + attempt,
      name: `linux-vulkan-proof / ${linuxHost.unavailable_producer_job_name}`,
      status: "completed",
      conclusion: "failure",
      run_attempt: String(attempt),
      log_uploaded: signature !== "lost",
      annotations: signature === "lost"
        ? [{ message: nonClaimPolicy.annotation }]
        : [{ message: "Process completed with exit code 1." }],
      steps: signature === "lost"
        ? [
          { name: "Checkout exact source", status: "completed", conclusion: "success" },
          { name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: null },
        ]
        : [{ name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: "failure" }],
    });
  }
  return rows;
}

/// Actions metadata for a run whose Linux proof did not succeed and whose hosted non-claim job did.
/// The run is at the recovery bound, because a cell may only be withheld once that bound is spent.
function withheldRunMetadata(phase, {
  hostConclusion = "failure",
  withNonClaimArtifact = true,
  jobEvidence = lostHostEvidence(),
} = {}) {
  const metadata = actionsMetadata(phase);
  for (const jobs of Object.values(metadata.jobsByAttempt)) {
    for (const job of jobs) {
      if (job.name.endsWith(linuxHost.unavailable_producer_job_name)) job.conclusion = hostConclusion;
    }
  }
  if (hostConclusion !== "success") {
    const lostArtifacts = new Set(linuxHost.withheld_cells.map((id) =>
      resolveReleaseCellConstraints(cell(id), "1").producer_artifact));
    metadata.artifacts = metadata.artifacts.filter(({ name }) => !lostArtifacts.has(name));
  }
  metadata.jobsByAttempt[withheldAttempt].push({
    id: 9001,
    run_id: 12345,
    run_attempt: withheldAttempt,
    head_sha: gitIdentity.commit,
    name: `Release / ${nonClaimPolicy.producer_job_name}`,
    status: "completed",
    conclusion: "success",
    started_at: "2026-07-19T13:00:00.000Z",
    completed_at: "2026-07-19T13:10:00.000Z",
  });
  if (withNonClaimArtifact) {
    metadata.artifacts.push({
      id: 9002,
      name: linuxHost.producer_artifacts.pre_publish.replaceAll("{attempt}", withheldAttempt),
      digest: `sha256:${"9".repeat(64)}`,
      size_in_bytes: 1024,
      expired: false,
      created_at: "2026-07-19T13:05:00.000Z",
      expires_at: "2026-08-18T12:05:00.000Z",
      workflow_run: { id: 12345, head_sha: gitIdentity.commit },
    });
  }
  return { ...metadata, jobEvidence };
}

test("a lost protected host routes only its own cells to the non-claim producer", () => {
  const map = buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: withheldAttempt,
    ...withheldRunMetadata("pre_publish"),
  });
  const byCell = new Map(map.producers.map((row) => [row.cell_id, row]));
  for (const cellId of ["accelerator_execution:linux-x64-vulkan", "candidate_installed_behavior:linux-x64"]) {
    assert.equal(byCell.get(cellId).non_claim, true, cellId);
    assert.equal(byCell.get(cellId).producer_job, nonClaimPolicy.producer_job);
    assert.equal(
      byCell.get(cellId).producer_artifact,
      linuxHost.producer_artifacts.pre_publish.replaceAll("{attempt}", withheldAttempt),
    );
  }
  // Every host that did report keeps its real proof producer, and nothing else is marked withheld.
  assert.deepEqual(
    map.producers.filter(({ non_claim: withheld }) => withheld === true)
      .map(({ cell_id: id }) => id).sort(),
    ["accelerator_execution:linux-x64-vulkan", "candidate_installed_behavior:linux-x64"],
  );
  assert.equal(byCell.get("accelerator_execution:windows-x64-vulkan").non_claim, undefined);
  assert.equal(byCell.get("package_identity:linux-x64").non_claim, undefined);
});

test("the non-claim producer is never substituted for a host that reported", () => {
  // Success on the protected host keeps the proof producer even when a non-claim artifact exists.
  const proven = buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: withheldAttempt,
    ...withheldRunMetadata("pre_publish", { hostConclusion: "success", jobEvidence: [] }),
  });
  assert.deepEqual(proven.producers.filter(({ non_claim: withheld }) => withheld === true), []);

  // Without a recorded non-claim there is nothing to fall back to, and the run stays broken.
  assert.throws(() => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: withheldAttempt,
    ...withheldRunMetadata("pre_publish", { withNonClaimArtifact: false }),
  }), /must retain one release-cell-nonclaim-prepublish-linux-x64-vulkan-attempt-2 artifact/u);
});

test("the closeout reads the lost-runner signature itself before it trusts a non-claim", () => {
  // The trust boundary decides whether a cell is authenticated against the real proof or against
  // the non-claim producer. Routing on "the host job is not green" made the producer's own refusal
  // to upload the single point of failure; the map now refuses the routing on its own evidence.
  const withEvidence = (jobEvidence) => () => buildTrustedProducerMap({
    graph,
    gitIdentity,
    phase: "pre_publish",
    runId: "12345",
    currentRunAttempt: withheldAttempt,
    ...withheldRunMetadata("pre_publish", { jobEvidence }),
  });

  // A proof that ran and failed its own assertions is red, and stays red.
  assert.throws(
    withEvidence(lostHostEvidence({ signature: "assertion" })),
    /failed its own assertions, so accelerator_execution:linux-x64-vulkan may not be withheld/u,
  );
  // One loss is not a spent recovery bound, however many attempts the run has had.
  assert.throws(
    withEvidence(lostHostEvidence({ executions: 1 })),
    /has been lost 1 time\(s\).*recovery bound is spent/su,
  );
  // No evidence at all is refused outright rather than defaulting to the permissive route.
  assert.throws(withEvidence(null), /cannot route .* to a non-claim without its own Actions job evidence/u);
  assert.throws(withEvidence([]), /has no failed execution of Packaged Linux Vulkan engine/u);

  // And the honest shape still routes.
  const routed = withEvidence(lostHostEvidence())();
  assert.deepEqual(
    routed.producers.filter(({ non_claim: withheld }) => withheld === true)
      .map(({ cell_id: id }) => id).sort(),
    ["accelerator_execution:linux-x64-vulkan", "candidate_installed_behavior:linux-x64"],
  );
});

test("withheld manifests carry the populated non-claim and refuse an unspent retry bound", () => {
  const bound = String(nonClaimPolicy.maximum_run_attempts);
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-nonclaim-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-linux-x64.tar.gz");
  writeFileSync(archive, "release archive bytes");
  const selected = cell("accelerator_execution:linux-x64-vulkan");
  const build = (attempt) => produceReleaseCellManifest({
    graph,
    gitIdentity,
    version,
    cell: selected,
    identity: { native_engine: "coderank_q8_embedded" },
    producer: {
      producer_workflow: nonClaimPolicy.producer_workflow,
      producer_job: nonClaimPolicy.producer_job,
      producer_job_name: nonClaimPolicy.producer_job_name,
      producer_run_id: "12345",
      producer_run_attempt: attempt,
      producer_artifact: linuxHost.producer_artifacts.pre_publish.replaceAll("{attempt}", attempt),
    },
    observedAt,
    archivePath: archive,
    nonClaim: {
      host: linuxHost.id,
      runtime_execution: nonClaimPolicy.runtime_execution,
      non_claim_reason: nonClaimPolicy.reason,
      annotation: nonClaimPolicy.annotation,
      unavailable_producer_workflow: linuxHost.unavailable_producer_workflow,
      unavailable_producer_job_name: linuxHost.unavailable_producer_job_name,
      withheld_claims: ["accelerator_execution", "package_identity", "source_behavior"],
      run_attempt: attempt,
    },
  });

  const manifest = build(bound);
  assert.equal(manifest.evidence.status, "withheld");
  assert.equal(manifest.non_claim.runtime_execution, "not_proven_by_package");
  assert.equal(manifest.non_claim.non_claim_reason, "accelerator_host_unavailable");
  // The identity still describes the proof that did not happen, so the ledger names the host.
  assert.equal(manifest.evidence.identity.backend, "Vulkan");
  assert.equal(manifest.evidence.identity.runner, "codestory-linux-vulkan");
  assert.equal(manifest.evidence.identity.producer_job, nonClaimPolicy.producer_job);

  assert.throws(() => build("1"), /recovery bound/u);
});

test("withhold writes one container per closeout phase, never a phase-mixed one", () => {
  // Each phase's trusted producer map authorizes only the manifests it selected, so a container
  // holding another phase's cell is rejected at download time and the release is lost anyway.
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-withhold-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-linux-x64.tar.gz");
  writeFileSync(archive, "release archive bytes");
  const identityPath = path.join(directory, "identity.json");
  writeFileSync(identityPath, JSON.stringify({
    installer: "candidate_managed_plugin",
    native_engine: "coderank_q8_embedded",
  }));
  const outDir = path.join(directory, "cells");
  const head = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  const result = spawnSync(process.execPath, [
    path.join(root, "scripts/codestory-release-cell-manifest.mjs"), "withhold",
    "--repo", root,
    "--expected-sha", head,
    "--version", version,
    "--host", "linux-x64-vulkan",
    "--producer-run-id", "12345",
    "--producer-run-attempt", String(nonClaimPolicy.maximum_run_attempts),
    "--identity", identityPath,
    "--archive", archive,
    "--out-dir", outDir,
  ], { encoding: "utf8", env: producerEnv({ GITHUB_RUN_ID: "12345" }) });
  assert.equal(result.status, 0, result.stderr);

  const expected = {
    pre_publish: {
      artifact: linuxHost.producer_artifacts.pre_publish,
      cells: ["accelerator_execution:linux-x64-vulkan", "candidate_installed_behavior:linux-x64"],
    },
    post_publish: {
      artifact: linuxHost.producer_artifacts.post_publish,
      cells: ["retrieval_readiness:linux-x64"],
    },
  };
  assert.deepEqual(readdirSync(outDir).sort(), ["post_publish", "pre_publish"]);
  for (const [phase, { artifact, cells }] of Object.entries(expected)) {
    const written = readdirSync(path.join(outDir, phase))
      .map((name) => JSON.parse(readFileSync(path.join(outDir, phase, name), "utf8")));
    assert.deepEqual(written.map(({ cell_id: id }) => id).sort(), [...cells].sort(), phase);
    for (const manifest of written) {
      assert.equal(manifest.phase, phase);
      assert.equal(manifest.evidence.status, "withheld");
      assert.equal(
        manifest.evidence.identity.producer_artifact,
        artifact.replaceAll("{attempt}", String(nonClaimPolicy.maximum_run_attempts)),
      );
    }
  }
});
