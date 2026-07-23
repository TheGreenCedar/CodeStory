import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  deriveReleaseCells,
  evaluateReleaseCloseout,
  readReleaseCellArtifacts,
  resolveReleaseCellConstraints,
  writeReleaseCloseout,
} from "../codestory-release-closeout.mjs";
import {
  loadReleaseClaimGraph,
  releaseClaimGraphDigest,
} from "../codestory-release-claims.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const graph = loadReleaseClaimGraph(root);
const negativeFixtures = JSON.parse(readFileSync(path.join(
  root,
  "scripts/tests/fixtures/release-claims/closeout-negative.json",
), "utf8"));
const version = "0.16.0";
const evaluatedAt = "2026-07-18T12:00:00.000Z";
const observedAt = "2026-07-18T11:00:00.000Z";
const expiresAt = "2026-07-19T10:00:00.000Z";
const gitIdentity = {
  repository: "TheGreenCedar/CodeStory",
  commit: "2".repeat(40),
  source_tree: "a".repeat(40),
};

function sha(value) {
  return createHash("sha256").update(value).digest("hex");
}

function packageRow(target) {
  return graph.workflow_policy.package_matrix.find(({ asset_target: assetTarget }) => assetTarget === target);
}

function archiveName(target) {
  const row = packageRow(target);
  return `codestory-cli-v${version}-${target}.${row.extension}`;
}

function artifactSha(target) {
  return sha(`archive:${target}`);
}

function hostIdentity(target) {
  if (target.startsWith("linux-")) {
    return { host_os: "Linux", host_arch: target.endsWith("arm64") ? "ARM64" : "X64" };
  }
  if (target.startsWith("windows-")) {
    return { host_os: "Windows", host_arch: target.endsWith("arm64") ? "ARM64" : "X64" };
  }
  return { host_os: "macOS", host_arch: target.endsWith("arm64") ? "ARM64" : "X64" };
}

function identityFor(cell, producerRunAttempt = "1") {
  const target = cell.identity_constraints.target;
  const constraints = resolveReleaseCellConstraints(cell, producerRunAttempt);
  const identity = { ...gitIdentity, ...constraints };
  for (const key of cell.required_identity) {
    if (identity[key] !== undefined) continue;
    switch (key) {
      case "artifact_sha256": identity[key] = target ? artifactSha(target) : sha(cell.id); break;
      case "pre_publish_artifact_sha256": identity[key] = artifactSha(target); break;
      case "producer_version":
      case "runtime_version": identity[key] = version; break;
      case "target": identity[key] = target; break;
      case "host_os":
      case "host_arch": identity[key] = hostIdentity(target)[key]; break;
      case "runner": identity[key] = "hosted-runner"; break;
      case "backend": identity[key] = "CPU"; break;
      case "installer": identity[key] = "managed_plugin"; break;
      case "profile": identity[key] = "codestory-release-evidence-linux-arm64-v2"; break;
      case "corpus_id": identity[key] = "v0.16-axios-js-ts-v1"; break;
      case "cache_id": identity[key] = "cold-full-retrieval-v1"; break;
      case "machine_fingerprint": identity[key] = "fixture/machine"; break;
      case "baseline_id": identity[key] = "linux-arm64-v2@56cfed37"; break;
      case "baseline_sha256": identity[key] = "b".repeat(64); break;
      case "release_key": identity[key] = "release-0.16.0"; break;
      case "evaluation_contract": identity[key] = "publishable-three-repeat-packet/v1"; break;
      case "producer_run_id": identity[key] = "12345"; break;
      case "producer_run_attempt": identity[key] = producerRunAttempt; break;
      case "native_engine": identity[key] = "coderank_q8"; break;
      case "calibration_sha256": identity[key] = "c".repeat(64); break;
      default: throw new Error(`test fixture has no identity value for ${key}`);
    }
  }
  return identity;
}

function manifestsFor(phase, prePublishLedger = null) {
  const graphSha256 = releaseClaimGraphDigest(graph);
  return deriveReleaseCells(graph, phase).map((cell) => {
    const identity = identityFor(cell);
    const evidenceType = graph.evidence_types.find(({ id }) => id === cell.evidence_type);
    const manifest = {
      schema: graph.closeout.manifest_schema,
      cell_id: cell.id,
      phase: cell.phase,
      version,
      graph_sha256: graphSha256,
      evidence: {
        id: `${cell.id}-evidence`,
        type: cell.evidence_type,
        tier: evidenceType.tier,
        status: "pass",
        graph_sha256: graphSha256,
        observed_at: observedAt,
        expires_at: expiresAt,
        identity,
      },
    };
    if (cell.archive_role === "pre_publish") {
      manifest.archive = {
        name: archiveName(identity.target),
        sha256: identity.artifact_sha256,
        bytes: 1024,
      };
    }
    if (cell.archive_role === "post_publish_compare") {
      const packageCell = prePublishLedger.cells.find(
        ({ id }) => id === `package_identity:${identity.target}`,
      );
      manifest.comparison = {
        pre_publish_cell_id: packageCell.id,
        pre_publish_manifest_sha256: packageCell.manifest.sha256,
        pre_publish_artifact_sha256: packageCell.archive.sha256,
        published_artifact_name: packageCell.archive.name,
        published_artifact_sha256: packageCell.archive.sha256,
      };
    }
    return manifest;
  });
}

function trustedProducersFor(phase) {
  const artifactByName = new Map();
  let nextId = 1000;
  const producers = deriveReleaseCells(graph, phase).map((cell) => {
    const constraints = resolveReleaseCellConstraints(cell, "1");
    let artifact = artifactByName.get(constraints.producer_artifact);
    if (!artifact) {
      artifact = {
        id: String(nextId++),
        name: constraints.producer_artifact,
        digest: `sha256:${sha(constraints.producer_artifact)}`,
        size_in_bytes: 1024,
        expired: false,
        created_at: "2026-07-18T11:05:00.000Z",
        expires_at: "2026-08-17T11:05:00.000Z",
        workflow_run_id: "12345",
        head_sha: gitIdentity.commit,
      };
      artifactByName.set(constraints.producer_artifact, artifact);
    }
    return {
      cell_id: cell.id,
      producer_workflow: constraints.producer_workflow,
      producer_job: constraints.producer_job,
      producer_job_name: constraints.producer_job_name,
      producer_run_id: "12345",
      producer_run_attempt: "1",
      producer_artifact: constraints.producer_artifact,
      artifact,
      job: {
        id: String(nextId++),
        run_id: "12345",
        head_sha: gitIdentity.commit,
        name: `Release / ${constraints.producer_job_name}`,
        status: "completed",
        conclusion: "success",
        run_attempt: "1",
        started_at: "2026-07-18T11:00:00.000Z",
        completed_at: "2026-07-18T11:10:00.000Z",
      },
    };
  });
  return {
    schema: "codestory.release-actions-provenance/v1",
    phase,
    manifest_schema: graph.closeout.manifest_schema,
    graph_sha256: releaseClaimGraphDigest(graph),
    identity: gitIdentity,
    run_id: "12345",
    current_run_attempt: "1",
    producers,
    artifacts: [...artifactByName.values()],
  };
}

function canonicalValue(value) {
  if (Array.isArray(value)) return value.map(canonicalValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonicalValue(value[key])]));
  }
  return value;
}

function canonicalManifestBytes(manifest) {
  return `${JSON.stringify(canonicalValue(manifest), null, 2)}\n`;
}

function canonicalManifestSha(manifest) {
  return sha(canonicalManifestBytes(manifest));
}

function evaluate(
  phase,
  manifests,
  prePublishLedger = null,
  trustedProducers = trustedProducersFor(phase),
  trustedExceptionDocument = null,
  artifactBindings = null,
) {
  const bindings = artifactBindings ?? manifests.map((manifest) => {
    const producer = trustedProducers?.producers?.find(({ cell_id: cellId }) =>
      cellId === manifest.cell_id);
    return {
      cell_id: manifest.cell_id,
      producer_artifact: producer?.producer_artifact,
      artifact_id: producer?.artifact?.id,
      artifact_digest: producer?.artifact?.digest,
      manifest_sha256: canonicalManifestSha(manifest),
    };
  });
  return evaluateReleaseCloseout({
    graph,
    phase,
    version,
    evaluatedAt,
    gitIdentity,
    manifests,
    prePublishLedger,
    trustedProducers,
    trustedExceptionDocument,
    artifactBindings: bindings,
  });
}

test("cell inventory is derived only from the release claim graph", () => {
  const prePublish = deriveReleaseCells(graph, "pre_publish");
  const postPublish = deriveReleaseCells(graph, "post_publish");
  assert.equal(prePublish.length, 5);
  assert.equal(postPublish.length, 11);
  assert.deepEqual(
    prePublish.filter(({ group_id }) => group_id === "package_identity").map(({ identity_constraints }) => identity_constraints.target),
    graph.workflow_policy.package_matrix.map(({ asset_target: assetTarget }) => assetTarget).sort(),
  );
  assert.deepEqual(
    postPublish.find(({ id }) => id === "platform_support:windows-x64").identity_constraints,
    {
      producer_workflow: ".github/workflows/post-publish-release-smoke.yml",
      producer_job: "smoke",
      producer_job_name: "Published windows-x64 smoke",
      producer_artifact: "release-cell-postpublish-windows-x64-attempt-{attempt}",
      target: "windows-x64",
      host_os: "Windows",
      host_arch: "X64",
    },
  );

  const changed = structuredClone(graph);
  changed.workflow_policy.package_matrix[0].asset_target = "windows-future";
  const changedCells = deriveReleaseCells(changed, "pre_publish");
  assert.ok(changedCells.some(({ id }) => id === "package_identity:windows-future"));
  assert.ok(!changedCells.some(({ id }) => id === "package_identity:windows-x64"));

  const targets = graph.workflow_policy.package_matrix
    .map(({ asset_target: assetTarget }) => assetTarget);
  assert.equal(targets.length, 2);
  assert.equal(new Set(targets).size, 2);
  assert.equal(
    prePublish.filter(({ group_id }) => group_id === "candidate_installed_behavior").length,
    2,
  );
  assert.deepEqual(
    postPublish
      .filter(({ group_id }) => group_id === "installed_runtime_behavior")
      .map(({ identity_constraints }) => identity_constraints.target)
      .sort(),
    [...targets].sort(),
  );
});

test("accepted pre-publish closeout retains one manifest and evaluation per cell deterministically", () => {
  const manifests = manifestsFor("pre_publish");
  const first = evaluate("pre_publish", manifests);
  const second = evaluate("pre_publish", structuredClone(manifests));
  assert.equal(first.decision, "accept");
  assert.deepEqual(first.ledger, second.ledger);
  assert.deepEqual(first.summary, second.summary);
  assert.equal(first.summary.counts.required, 5);
  assert.equal(first.summary.counts.passed, 5);
  assert.equal(first.retainedManifests.size, 5);
  assert.equal(first.evaluations.size, 5);

  const out = mkdtempSync(path.join(os.tmpdir(), "codestory-release-closeout-"));
  writeReleaseCloseout(out, first);
  assert.equal(readdirSync(path.join(out, "manifests")).length, 5);
  assert.equal(readdirSync(path.join(out, "evaluations")).length, 5);
  assert.deepEqual(JSON.parse(readFileSync(path.join(out, "ledger.json"))), first.ledger);
  assert.deepEqual(JSON.parse(readFileSync(path.join(out, "summary.json"))), first.summary);
});

test("closeout rejects loose JSON and artifact bindings outside selected Actions containers", () => {
  const trustedProducers = trustedProducersFor("pre_publish");
  const selected = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-selected-"));
  const selectedManifests = manifestsFor("pre_publish");
  for (const manifest of selectedManifests) {
    const producer = trustedProducers.producers.find(({ cell_id: cellId }) =>
      cellId === manifest.cell_id);
    const artifactRoot = path.join(selected, producer.producer_artifact);
    mkdirSync(artifactRoot, { recursive: true });
    writeFileSync(
      path.join(artifactRoot, `${manifest.cell_id.replaceAll(":", "_")}.json`),
      canonicalManifestBytes(manifest),
    );
  }
  const downloaded = readReleaseCellArtifacts(selected, trustedProducers);
  assert.equal(downloaded.manifests.length, 5);
  assert.equal(downloaded.artifactBindings.length, 5);

  const loose = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-loose-"));
  writeFileSync(path.join(loose, "source_behavior.json"), "{}\n");
  assert.throws(
    () => readReleaseCellArtifacts(loose, trustedProducers),
    /must be one selected artifact directory/u,
  );

  const manifests = manifestsFor("pre_publish");
  const bindings = manifests.map((manifest) => {
    const producer = trustedProducers.producers.find(({ cell_id: cellId }) =>
      cellId === manifest.cell_id);
    return {
      cell_id: manifest.cell_id,
      producer_artifact: producer.producer_artifact,
      artifact_id: producer.artifact.id,
      artifact_digest: producer.artifact.digest,
      manifest_sha256: canonicalManifestSha(manifest),
    };
  });
  const hostileBinding = bindings.find(({ cell_id: cellId }) => cellId === "source_behavior");
  hostileBinding.artifact_id = "999999";
  hostileBinding.artifact_digest = `sha256:${"f".repeat(64)}`;
  const rejected = evaluate(
    "pre_publish",
    manifests,
    null,
    trustedProducers,
    null,
    bindings,
  );
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes("source_behavior"));
});

test("post-publish closeout compares every downloaded archive with the retained pre-publish bytes", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const manifests = manifestsFor("post_publish", prePublish.ledger);
  const postPublish = evaluate("post_publish", manifests, prePublish.ledger);
  assert.equal(postPublish.decision, "accept");
  assert.equal(postPublish.summary.counts.required, 11);
  assert.equal(
    postPublish.ledger.cells.filter(({ id }) => id.startsWith("post_publish_bytes:")).length,
    graph.workflow_policy.package_matrix.length,
  );

  const changed = structuredClone(manifests);
  const bytes = changed.find(({ cell_id }) => cell_id === "post_publish_bytes:windows-x64");
  bytes.comparison.published_artifact_sha256 = "d".repeat(64);
  bytes.evidence.identity.artifact_sha256 = "d".repeat(64);
  const rejected = evaluate("post_publish", changed, prePublish.ledger);
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes("post_publish_bytes:windows-x64"));
});

test("hostile post-publish A/B split cannot replace the retained package used by platform proof", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const manifests = manifestsFor("post_publish", prePublish.ledger);
  const replacementSha256 = "d".repeat(64);
  for (const cellId of [
    "package_identity:windows-x64",
    "platform_support:windows-x64",
    "installed_runtime_behavior:windows-x64",
  ]) {
    const manifest = manifests.find(({ cell_id: id }) => id === cellId);
    manifest.evidence.identity.artifact_sha256 = replacementSha256;
    if (manifest.archive) manifest.archive.sha256 = replacementSha256;
  }

  const rejected = evaluate("post_publish", manifests, prePublish.ledger);
  assert.equal(rejected.decision, "reject");
  for (const cellId of [
    "package_identity:windows-x64",
    "platform_support:windows-x64",
    "installed_runtime_behavior:windows-x64",
  ]) {
    assert.ok(rejected.summary.failed_cells.includes(cellId));
  }
  assert.ok(rejected.evaluations.get("package_identity:windows-x64").value.failures.some((message) =>
    message.includes("retained pre-publish manifest")));
  assert.ok(rejected.evaluations.get("platform_support:windows-x64").value.failures.some((message) =>
    message.includes("dependency cell package_identity:windows-x64")));
});

test("hostile producer and runtime semver claims must equal the independently supplied closeout version", () => {
  const preManifests = manifestsFor("pre_publish");
  preManifests.find(({ cell_id: id }) => id === "package_identity:windows-x64")
    .evidence.identity.producer_version = "0.15.0";
  const rejectedPrePublish = evaluate("pre_publish", preManifests);
  assert.equal(rejectedPrePublish.decision, "reject");
  assert.ok(rejectedPrePublish.summary.failed_cells.includes("package_identity:windows-x64"));

  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const postManifests = manifestsFor("post_publish", prePublish.ledger);
  postManifests.find(({ cell_id: id }) => id === "installed_runtime_behavior:windows-x64")
    .evidence.identity.runtime_version = "0.15.0";
  const rejectedPostPublish = evaluate("post_publish", postManifests, prePublish.ledger);
  assert.equal(rejectedPostPublish.decision, "reject");
  assert.ok(rejectedPostPublish.summary.failed_cells.includes("installed_runtime_behavior:windows-x64"));
  assert.ok(rejectedPostPublish.evaluations.get("installed_runtime_behavior:windows-x64").value.failures.some(
    (message) => message.includes("producer_version and runtime_version must match"),
  ));
});

test("hostile platform and installed manifests cannot contradict the package target host", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const platformMismatch = manifestsFor("post_publish", prePublish.ledger);
  platformMismatch.find(({ cell_id: id }) => id === "platform_support:windows-x64")
    .evidence.identity.host_os = "Linux";
  const rejectedPlatform = evaluate("post_publish", platformMismatch, prePublish.ledger);
  assert.equal(rejectedPlatform.decision, "reject");
  assert.ok(rejectedPlatform.summary.failed_cells.includes("platform_support:windows-x64"));

  const installedMismatch = manifestsFor("post_publish", prePublish.ledger);
  installedMismatch.find(({ cell_id: id }) => id === "installed_runtime_behavior:macos-arm64")
    .evidence.identity.host_arch = "X64";
  const rejectedInstalled = evaluate("post_publish", installedMismatch, prePublish.ledger);
  assert.equal(rejectedInstalled.decision, "reject");
  assert.ok(rejectedInstalled.summary.failed_cells.includes("installed_runtime_behavior:macos-arm64"));
});

test("missing, duplicate, stale, failed, aggregate, and reused evidence fail closed", async (t) => {
  assert.equal(negativeFixtures.schema, "codestory.release-closeout-negative-fixtures/v1");
  for (const fixture of negativeFixtures.cases) {
    await t.test(fixture.id, () => {
      const manifests = manifestsFor("pre_publish");
      const operation = fixture.operation;
      const manifest = manifests.find(({ cell_id: cellId }) => cellId === operation.cell);
      if (operation.kind === "remove_cell") {
        manifests.splice(manifests.indexOf(manifest), 1);
      } else if (operation.kind === "duplicate_cell") {
        manifests.push(structuredClone(manifest));
      } else if (operation.kind === "set_identity") {
        manifest.evidence.identity[operation.key] = operation.value;
      } else if (operation.kind === "set_evidence") {
        manifest.evidence[operation.key] = operation.value;
      } else if (operation.kind === "reuse_evidence") {
        manifest.evidence.id = manifests.find(
          ({ cell_id: cellId }) => cellId === operation.source_cell,
        ).evidence.id;
      } else {
        throw new Error(`unknown closeout fixture operation ${operation.kind}`);
      }
      const result = evaluate("pre_publish", manifests);
      assert.equal(result.decision, "reject");
      for (const cell of fixture.expected_missing_cells ?? []) {
        assert.ok(result.summary.missing_cells.includes(cell));
      }
      for (const cell of fixture.expected_failed_cells ?? []) {
        assert.ok(result.summary.failed_cells.includes(cell));
      }
      if (fixture.expected_input_error) {
        assert.ok(result.summary.input_errors.some((message) =>
          message.includes(fixture.expected_input_error)));
      }
    });
  }
});

test("producer identity is accepted only from the separately trusted map", () => {
  const manifests = manifestsFor("pre_publish");
  const missingMap = evaluate("pre_publish", manifests, null, null);
  assert.equal(missingMap.decision, "reject");
  assert.ok(missingMap.summary.input_errors.includes("closeout requires a separately trusted producer map"));

  for (const [key, value] of [
    ["producer_workflow", ".github/workflows/arbitrary.yml"],
    ["producer_job_name", "arbitrary job"],
    ["producer_run_id", "999"],
    ["producer_run_attempt", "2"],
  ]) {
    const wrongProducer = trustedProducersFor("pre_publish");
    wrongProducer.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")[key] = value;
    const rejected = evaluate("pre_publish", manifests, null, wrongProducer);
    assert.equal(rejected.decision, "reject", key);
    assert.ok(
      rejected.summary.failed_cells.includes("source_behavior")
        || rejected.summary.input_errors.some((message) => message.includes(key)),
      key,
    );
  }

  const wrongArtifact = trustedProducersFor("pre_publish");
  wrongArtifact.producers.find(({ cell_id: cellId }) =>
    cellId === "candidate_installed_behavior:windows-x64").producer_artifact = "wrong";
  const rejectedArtifact = evaluate("pre_publish", manifests, null, wrongArtifact);
  assert.equal(rejectedArtifact.decision, "reject");
  assert.ok(rejectedArtifact.summary.input_errors.some((message) => message.includes("producer_artifact")));

  const wrongContainer = trustedProducersFor("pre_publish");
  wrongContainer.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .artifact.workflow_run_id = "999";
  const rejectedContainer = evaluate("pre_publish", manifests, null, wrongContainer);
  assert.equal(rejectedContainer.decision, "reject");
  assert.ok(rejectedContainer.summary.input_errors.some((message) =>
    message.includes("artifact run identity")));

  const wrongJob = trustedProducersFor("pre_publish");
  wrongJob.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .job.head_sha = "f".repeat(40);
  const rejectedJob = evaluate("pre_publish", manifests, null, wrongJob);
  assert.equal(rejectedJob.decision, "reject");
  assert.ok(rejectedJob.summary.input_errors.some((message) =>
    message.includes("job run identity")));

  const wrongGraph = trustedProducersFor("pre_publish");
  wrongGraph.graph_sha256 = "f".repeat(64);
  const rejectedGraph = evaluate("pre_publish", manifests, null, wrongGraph);
  assert.equal(rejectedGraph.decision, "reject");
  assert.ok(rejectedGraph.summary.input_errors.some((message) =>
    message.includes("graph identity")));

  const hostileInventoryCases = [
    ["artifact id", trusted => {
      trusted.artifacts[0] = { ...trusted.artifacts[0], id: "999999" };
    }, "missing from the download inventory"],
    ["artifact digest", trusted => {
      trusted.artifacts[0] = {
        ...trusted.artifacts[0],
        digest: `sha256:${"f".repeat(64)}`,
      };
    }, "differs from the download inventory"],
    ["artifact name", trusted => {
      trusted.artifacts[0] = {
        ...trusted.artifacts[0],
        name: "release-cell-forged-attempt-1",
      };
    }, "differs from the download inventory"],
    ["unexpected artifact", trusted => {
      trusted.artifacts.push({
        ...structuredClone(trusted.artifacts[0]),
        id: "999999",
        name: "release-cell-unexpected-attempt-1",
      });
    }, "unused artifact"],
  ];
  for (const [label, mutate, expected] of hostileInventoryCases) {
    const trusted = trustedProducersFor("pre_publish");
    mutate(trusted);
    const rejected = evaluate("pre_publish", manifests, null, trusted);
    assert.equal(rejected.decision, "reject", label);
    assert.ok(rejected.summary.input_errors.some((message) =>
      message.includes(expected)), label);
  }

  const futureAttempt = trustedProducersFor("pre_publish");
  futureAttempt.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .producer_run_attempt = "2";
  futureAttempt.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .job.run_attempt = "2";
  const rejectedFuture = evaluate("pre_publish", manifests, null, futureAttempt);
  assert.equal(rejectedFuture.decision, "reject");
  assert.ok(rejectedFuture.summary.input_errors.some((message) =>
    message.includes("future run attempt")));

  const wrongWindow = trustedProducersFor("pre_publish");
  wrongWindow.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .job.completed_at = "2026-07-18T11:04:00.000Z";
  const rejectedWindow = evaluate("pre_publish", manifests, null, wrongWindow);
  assert.equal(rejectedWindow.decision, "reject");
  assert.ok(rejectedWindow.summary.input_errors.some((message) =>
    message.includes("outside its job window")));
});
