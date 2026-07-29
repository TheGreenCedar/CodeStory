import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
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
  releaseCellWithheldClaims,
  resolveReleaseCellConstraints,
  resolveReleaseCellNonClaimConstraints,
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
      // The post-publish installed cells are where the closeout reads which catalog served the
      // release, so their installer must be one of the two declared delivery identities.
      case "installer":
        identity[key] = cell.group_id === "installed_runtime_behavior"
          ? "codex_marketplace_install"
          : "managed_plugin";
        break;
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
      default: throw new Error(`test fixture has no identity value for ${key}`);
    }
  }
  return identity;
}

const nonClaimPolicy = graph.non_claim_policy;
const linuxHost = nonClaimPolicy.hosts.find(({ id }) => id === "linux-x64-vulkan");

function nonClaimFor(cell, host, attempt) {
  return {
    host: host.id,
    runtime_execution: nonClaimPolicy.runtime_execution,
    non_claim_reason: nonClaimPolicy.reason,
    annotation: nonClaimPolicy.annotation,
    unavailable_producer_workflow: host.unavailable_producer_workflow,
    unavailable_producer_job_name: host.unavailable_producer_job_name,
    withheld_claims: releaseCellWithheldClaims(graph, cell),
    run_attempt: attempt,
  };
}

/// Withholding is declared per host, so every helper takes the set of hosts a scenario lost. One
/// host is the ordinary outage; the multi-host forms exist because the withhold cap is only
/// observable when more than one host is gone at once.
function withheldHostList(withheldHost) {
  if (withheldHost === null || withheldHost === undefined) return [];
  return Array.isArray(withheldHost) ? withheldHost : [withheldHost];
}

function withheldHostOf(withheldHost, cellId) {
  return withheldHostList(withheldHost).find((host) => host.withheld_cells.includes(cellId)) ?? null;
}

function manifestsFor(phase, prePublishLedger = null, { attempt = "1", withheldHost = null } = {}) {
  const graphSha256 = releaseClaimGraphDigest(graph);
  return deriveReleaseCells(graph, phase).map((cell) => {
    const host = withheldHostOf(withheldHost, cell.id);
    const withheld = host !== null;
    const identity = withheld
      ? { ...identityFor(cell, attempt), ...resolveReleaseCellNonClaimConstraints(cell, attempt) }
      : identityFor(cell, attempt);
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
        status: withheld ? "withheld" : "pass",
        graph_sha256: graphSha256,
        observed_at: observedAt,
        expires_at: expiresAt,
        identity,
      },
    };
    if (withheld) manifest.non_claim = nonClaimFor(cell, host, attempt);
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

function trustedProducersFor(phase, withheldHost = null) {
  const artifactByName = new Map();
  const attempt = withheldHostList(withheldHost).length > 0
    ? String(nonClaimPolicy.maximum_run_attempts)
    : "1";
  let nextId = 1000;
  const producers = deriveReleaseCells(graph, phase).map((cell) => {
    const withheld = withheldHostOf(withheldHost, cell.id) !== null;
    const constraints = withheld
      ? {
        ...resolveReleaseCellConstraints(cell, attempt),
        ...resolveReleaseCellNonClaimConstraints(cell, attempt),
      }
      : resolveReleaseCellConstraints(cell, attempt);
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
      ...(withheld ? { non_claim: true } : {}),
      producer_workflow: constraints.producer_workflow,
      producer_job: constraints.producer_job,
      producer_job_name: constraints.producer_job_name,
      producer_run_id: "12345",
      producer_run_attempt: attempt,
      producer_artifact: constraints.producer_artifact,
      artifact,
      job: {
        id: String(nextId++),
        run_id: "12345",
        head_sha: gitIdentity.commit,
        name: `Release / ${constraints.producer_job_name}`,
        status: "completed",
        conclusion: "success",
        run_attempt: attempt,
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
    current_run_attempt: attempt,
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
  verifyReuseBinding = null,
  resolveCommitIdentity = null,
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
    verifyReuseBinding,
    resolveCommitIdentity,
  });
}

// ── Cross-run evidence reuse ────────────────────────────────────────────────────────────────

const reusedRunId = "777";
const reusedCommit = "3".repeat(40);

/// Stands in for the git binding proof `main()` runs against the closeout's own checkout.
function reuseVerifier({
  // A commit is its own ancestor, so the release commit always satisfies the tree binding.
  ancestors = [reusedCommit, gitIdentity.commit],
  value = gitIdentity.source_tree,
} = {}) {
  return ({ binding, releaseCommit, reusedCommit: reused }) => {
    assert.equal(releaseCommit, gitIdentity.commit);
    if (binding !== "source_tree") throw new Error(`unknown reuse binding ${binding}`);
    if (!ancestors.includes(reused)) {
      throw new Error(`reused commit ${reused} is not an ancestor of the release commit`);
    }
    return value;
  };
}

/// Re-anchor the source cell's producer row onto a prior run, exactly as the producer map does
/// once release preflight selects source-proof reuse.
function reuseSourceBehavior(trustedProducers, manifests, reusedFrom = {}) {
  const row = trustedProducers.producers.find(({ cell_id: cellId }) => cellId === "source_behavior");
  row.producer_run_id = reusedRunId;
  row.reused_from = {
    run_id: reusedRunId,
    head_sha: reusedCommit,
    binding: "source_tree",
    binding_value: gitIdentity.source_tree,
    ...reusedFrom,
  };
  row.artifact.workflow_run_id = reusedRunId;
  row.artifact.head_sha = reusedCommit;
  row.job.run_id = reusedRunId;
  row.job.head_sha = reusedCommit;
  // The reused manifest was produced by that earlier run, at the binding-equal commit.
  const manifest = manifests.find(({ cell_id: cellId }) => cellId === "source_behavior");
  manifest.evidence.identity.producer_run_id = reusedRunId;
  manifest.evidence.identity.commit = reusedCommit;
  return row;
}

// ── Native-fingerprint reuse ────────────────────────────────────────────────────────────────

// The whole point of the native_fingerprint binding: the reused commit's tree is *not* this
// release's tree. Version-normalized native inputs are what is equal, so the accelerator the
// evidence exercised is the accelerator this release ships.
const reusedTree = "c".repeat(40);
const nativeFingerprint = "f".repeat(64);

function acceleratorCellIds() {
  return deriveReleaseCells(graph, "pre_publish")
    .filter(({ group_id: groupId }) => groupId === "accelerator_execution")
    .map(({ id }) => id);
}

/// Stands in for the git fingerprint proof, and for the closeout reading the reused commit's own
/// identity out of its checkout. Both are needed before a reused row may be read at this release's
/// tree: one says the trees may be equated, the other says which tree is being equated away.
function fingerprintReuse({
  ancestors = [reusedCommit],
  fingerprint = nativeFingerprint,
  tree = reusedTree,
} = {}) {
  return {
    verify: ({ binding, releaseCommit, reusedCommit: reused }) => {
      assert.equal(releaseCommit, gitIdentity.commit);
      if (binding !== "native_fingerprint") throw new Error(`unknown reuse binding ${binding}`);
      if (!ancestors.includes(reused)) {
        throw new Error(`reused commit ${reused} is not an ancestor of the release commit`);
      }
      return fingerprint;
    },
    resolve: (commit) => {
      if (commit !== reusedCommit) throw new Error(`git cat-file -e ${commit} failed`);
      return { repository: gitIdentity.repository, commit, source_tree: tree };
    },
  };
}

/// Re-anchor all three accelerator producer rows onto a prior run, exactly as the producer map
/// does once an operator selects `--reuse accelerator_execution=<run>:<sha>` in preflight.
function reuseAcceleratorExecution(trustedProducers, manifests, reusedFrom = {}) {
  const rows = [];
  for (const cellId of acceleratorCellIds()) {
    const row = trustedProducers.producers.find(({ cell_id: candidate }) => candidate === cellId);
    row.producer_run_id = reusedRunId;
    row.reused_from = {
      run_id: reusedRunId,
      head_sha: reusedCommit,
      binding: "native_fingerprint",
      binding_value: nativeFingerprint,
      ...reusedFrom,
    };
    row.artifact.workflow_run_id = reusedRunId;
    row.artifact.head_sha = reusedCommit;
    row.job.run_id = reusedRunId;
    row.job.head_sha = reusedCommit;
    const manifest = manifests.find(({ cell_id: candidate }) => candidate === cellId);
    manifest.evidence.identity.producer_run_id = reusedRunId;
    manifest.evidence.identity.commit = reusedCommit;
    // The reused run ran at its own tree, which is not this release's.
    manifest.evidence.identity.source_tree = reusedTree;
    rows.push(row);
  }
  return rows;
}

test("cell inventory is derived only from the release claim graph", () => {
  const prePublish = deriveReleaseCells(graph, "pre_publish");
  const postPublish = deriveReleaseCells(graph, "post_publish");
  assert.equal(prePublish.length, 10);
  assert.equal(postPublish.length, 22);
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
  assert.equal(targets.length, 3);
  assert.equal(new Set(targets).size, 3);
  assert.equal(
    prePublish.filter(({ group_id }) => group_id === "candidate_installed_behavior").length,
    3,
  );
  assert.equal(
    prePublish.filter(({ group_id }) => group_id === "accelerator_execution").length,
    3,
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
  assert.equal(first.summary.counts.required, 10);
  assert.equal(first.summary.counts.passed, 10);
  assert.equal(first.retainedManifests.size, 10);
  assert.equal(first.evaluations.size, 10);

  const out = mkdtempSync(path.join(os.tmpdir(), "codestory-release-closeout-"));
  writeReleaseCloseout(out, first);
  assert.equal(readdirSync(path.join(out, "manifests")).length, 10);
  assert.equal(readdirSync(path.join(out, "evaluations")).length, 10);
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
  assert.equal(downloaded.manifests.length, 10);
  assert.equal(downloaded.artifactBindings.length, 10);

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
  assert.equal(postPublish.summary.counts.required, 22);
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

test("a binding-verified reuse row is anchored to the run and commit it was produced by", () => {
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseSourceBehavior(trusted, manifests);
  const calls = [];
  const verify = reuseVerifier();
  const accepted = evaluate("pre_publish", manifests, null, trusted, null, null, (request) => {
    calls.push(request);
    return verify(request);
  });
  assert.equal(accepted.decision, "accept");
  assert.deepEqual(accepted.summary.input_errors, []);
  assert.deepEqual(accepted.summary.failed_cells, []);
  // The closeout re-proves the binding itself rather than trusting the producer map's word.
  assert.deepEqual(calls, [{
    binding: "source_tree",
    releaseCommit: gitIdentity.commit,
    reusedCommit,
  }]);
  // The ledger keeps the reused run and commit rather than restating the publishing run.
  const row = accepted.ledger.cells.find(({ id }) => id === "source_behavior");
  assert.equal(row.identity.producer_run_id, reusedRunId);
  assert.equal(row.identity.commit, reusedCommit);
  // Cells that were not reused stay bound to the publishing run.
  const packaged = accepted.ledger.cells.find(({ id }) => id === "package_identity:windows-x64");
  assert.equal(packaged.identity.producer_run_id, "12345");
  assert.equal(packaged.identity.commit, gitIdentity.commit);
});

test("a reuse row whose binding the closeout cannot reprove fails closed", () => {
  const rejections = [
    ["no binding is declared for the cell group", (trusted, manifests) => {
      const row = trusted.producers.find(({ cell_id: cellId }) =>
        cellId === "package_identity:windows-x64");
      row.producer_run_id = reusedRunId;
      row.reused_from = {
        run_id: reusedRunId,
        head_sha: reusedCommit,
        binding: "source_tree",
        binding_value: gitIdentity.source_tree,
      };
      row.artifact.workflow_run_id = reusedRunId;
      row.artifact.head_sha = reusedCommit;
      row.job.run_id = reusedRunId;
      row.job.head_sha = reusedCommit;
      manifests.find(({ cell_id: cellId }) => cellId === "package_identity:windows-x64")
        .evidence.identity.producer_run_id = reusedRunId;
    }, "package_identity:windows-x64 reuses evidence under an undeclared binding"],
    ["the row names a binding the group did not declare", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests, { binding: "native_fingerprint" });
    }, "source_behavior reuses evidence under an undeclared binding"],
    ["the reused commit is not an ancestor of the release commit", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests, { head_sha: "9".repeat(40) });
    }, "is not an ancestor of the release commit"],
    ["the reused commit does not resolve to the release tree", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests);
      // A verifier that proves the tree binding cannot prove it here.
      trusted.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
        .reused_from.binding_value = "e".repeat(40);
    }, "source_behavior recorded source_tree value does not bind this release"],
    ["the reused run identity is malformed", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests, { run_id: "0" });
    }, "source_behavior reused run identity is invalid"],
    ["the reuse record is not an object", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests);
      trusted.producers.find(({ cell_id: cellId }) => cellId === "source_behavior")
        .reused_from = reusedCommit;
    }, "source_behavior reuse record must be an object"],
    ["the reused artifact expired", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests).artifact.expired = true;
    }, "source_behavior artifact is expired"],
    ["the reused artifact still claims the publishing run's commit", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests).artifact.head_sha = gitIdentity.commit;
    }, "source_behavior artifact run identity changed"],
    ["the reused job still claims the publishing run's commit", (trusted, manifests) => {
      reuseSourceBehavior(trusted, manifests).job.head_sha = gitIdentity.commit;
    }, "source_behavior job run identity changed"],
    ["a reuse block naming the publishing run escapes its attempt cap", (trusted, manifests) => {
      const row = reuseSourceBehavior(trusted, manifests, {
        run_id: trusted.run_id,
        head_sha: gitIdentity.commit,
      });
      row.producer_run_id = trusted.run_id;
      row.producer_run_attempt = "2";
      row.artifact.workflow_run_id = trusted.run_id;
      row.artifact.head_sha = gitIdentity.commit;
      row.job.run_id = trusted.run_id;
      row.job.head_sha = gitIdentity.commit;
      row.job.run_attempt = "2";
      const manifest = manifests.find(({ cell_id: cellId }) => cellId === "source_behavior");
      manifest.evidence.identity.producer_run_id = trusted.run_id;
      manifest.evidence.identity.commit = gitIdentity.commit;
    }, "source_behavior uses a future run attempt"],
  ];
  for (const [label, mutate, expected] of rejections) {
    const manifests = manifestsFor("pre_publish");
    const trusted = trustedProducersFor("pre_publish");
    mutate(trusted, manifests);
    const rejected = evaluate("pre_publish", manifests, null, trusted, null, null, reuseVerifier());
    assert.equal(rejected.decision, "reject", label);
    assert.ok(
      rejected.summary.input_errors.some((message) => message.includes(expected)),
      `${label}: ${JSON.stringify(rejected.summary.input_errors)}`,
    );
  }
});

test("a closeout with no way to reprove a binding refuses the reuse row outright", () => {
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseSourceBehavior(trusted, manifests);
  const rejected = evaluate("pre_publish", manifests, null, trusted);
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.input_errors.some((message) =>
    message.includes("source_behavior reuses evidence this closeout cannot verify")));
});

test("a reused artifact container is still bound by its digest", () => {
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseSourceBehavior(trusted, manifests);
  const bindings = manifests.map((manifest) => {
    const producer = trusted.producers.find(({ cell_id: cellId }) => cellId === manifest.cell_id);
    return {
      cell_id: manifest.cell_id,
      producer_artifact: producer.producer_artifact,
      artifact_id: producer.artifact.id,
      artifact_digest: producer.artifact.digest,
      manifest_sha256: canonicalManifestSha(manifest),
    };
  });
  bindings.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .artifact_digest = `sha256:${"f".repeat(64)}`;
  const rejected = evaluate(
    "pre_publish",
    manifests,
    null,
    trusted,
    null,
    bindings,
    reuseVerifier(),
  );
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes("source_behavior"));
  assert.ok(rejected.evaluations.get("source_behavior").value.failures.some((message) =>
    message.includes("artifact_digest does not match Actions provenance")));
});

test("reuse never lets stale evidence through the checks that do not depend on the commit", () => {
  // The reused commit is admissible for the commit identity the binding equates, and for nothing
  // else: a reused manifest whose own tree is not this release's tree still fails.
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseSourceBehavior(trusted, manifests);
  manifests.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .evidence.identity.source_tree = "e".repeat(40);
  const rejected = evaluate("pre_publish", manifests, null, trusted, null, null, reuseVerifier());
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes("source_behavior"));
});

test("a reused manifest may declare only the commit the closeout proved bound to this release", () => {
  // Reading a reused row at the release commit is what the binding buys, and it is the only thing
  // that ever compared that row's declared commit to anything. So the declared commit has to be
  // the one the binding proof covered: not an unrelated commit, and not this release's own, which
  // the reused run could not have produced this artifact at.
  const declarations = [
    ["a commit the closeout never saw", "d".repeat(40)],
    ["the publishing run's own commit", gitIdentity.commit],
  ];
  for (const [label, declared] of declarations) {
    const manifests = manifestsFor("pre_publish");
    const trusted = trustedProducersFor("pre_publish");
    reuseSourceBehavior(trusted, manifests);
    manifests.find(({ cell_id: cellId }) => cellId === "source_behavior")
      .evidence.identity.commit = declared;
    const rejected = evaluate("pre_publish", manifests, null, trusted, null, null, reuseVerifier());
    assert.equal(rejected.decision, "reject", label);
    assert.ok(rejected.summary.failed_cells.includes("source_behavior"), label);
    assert.ok(
      rejected.evaluations.get("source_behavior").value.failures.some((message) =>
        message.includes("manifest commit is not the reused commit the closeout proved")),
      `${label}: ${JSON.stringify(rejected.evaluations.get("source_behavior").value.failures)}`,
    );
    // Nothing that reads the row as evidence inherits the unproven commit either.
    assert.ok(
      rejected.summary.failed_cells.includes("candidate_installed_behavior:linux-x64"),
      label,
    );
    // And the rejected ledger never restates the unproven commit as accepted evidence.
    assert.equal(
      rejected.ledger.cells.find(({ id }) => id === "source_behavior").status,
      "fail",
      label,
    );
  }
});

test("a reuse block naming the publishing run is not reuse", () => {
  // A commit is its own ancestor and its own tree, so a reuse block pointing at the run that is
  // publishing verifies trivially while inheriting nothing. Admitting it as a reuse anchor would
  // let an ordinary same-run row buy the reused row's standing with a proof about nothing.
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  trusted.producers.find(({ cell_id: cellId }) => cellId === "source_behavior").reused_from = {
    run_id: trusted.run_id,
    head_sha: gitIdentity.commit,
    binding: "source_tree",
    binding_value: gitIdentity.source_tree,
  };
  manifests.find(({ cell_id: cellId }) => cellId === "source_behavior")
    .evidence.identity.commit = "d".repeat(40);
  const calls = [];
  const verify = reuseVerifier();
  const rejected = evaluate("pre_publish", manifests, null, trusted, null, null, (request) => {
    calls.push(request);
    return verify(request);
  });
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.input_errors.some((message) =>
    message.includes("source_behavior reuses evidence from the publishing run")));
  // Refused as meaningless rather than proved: there is no earlier run here to inherit from, so
  // the closeout never asks the binding verifier to bless one.
  assert.deepEqual(calls, []);
  // And the row keeps the commit binding it would have had with no reuse block at all.
  assert.ok(rejected.summary.failed_cells.includes("source_behavior"));
});

/// Every message a rejected closeout produced, wherever it recorded it: input errors, cell
/// validation failures, and the claim evaluator's own failures all refuse in different places.
function refusals(result) {
  const messages = [...result.summary.input_errors];
  for (const { value } of result.evaluations.values()) {
    for (const failure of value.failures ?? []) messages.push(String(failure));
    for (const failure of value.release_claim_evaluation?.failures ?? []) {
      messages.push(String(failure.message));
    }
  }
  return messages;
}

test("native-fingerprint reuse is admitted for the tree that binding equates", () => {
  // Replaces the test that pinned this as a permanent refusal (#1567). The refusal was not a
  // trust decision, it was a missing declaration: the closeout read every reused row at the
  // release commit and at nothing else, so accelerator_execution -- whose required_identity
  // includes source_tree, and whose binding exists precisely because the trees differ -- could
  // never pass. The claim graph now says per binding which identity keys the binding may equate,
  // and native_fingerprint equates source_tree: an equal version-normalized fingerprint means
  // every input that determines the native binary is identical, so execution evidence carries
  // across a tree that differs only in code the accelerator never runs.
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  const cellIds = acceleratorCellIds();
  assert.equal(cellIds.length, 3);
  reuseAcceleratorExecution(trusted, manifests);
  const proof = fingerprintReuse();
  const proved = [];
  const resolved = [];
  const accepted = evaluate("pre_publish", manifests, null, trusted, null, null, (request) => {
    proved.push(request);
    return proof.verify(request);
  }, (commit) => {
    resolved.push(commit);
    return proof.resolve(commit);
  });
  assert.equal(accepted.decision, "accept");
  assert.deepEqual(accepted.summary.input_errors, []);
  assert.deepEqual(accepted.summary.failed_cells, []);
  assert.equal(accepted.summary.counts.passed, 10);
  // The closeout re-proves the binding for every reused cell, against its own checkout, and reads
  // the reused commit's own tree rather than taking the row's word for what it is equating away.
  assert.deepEqual(proved, cellIds.map(() => ({
    binding: "native_fingerprint",
    releaseCommit: gitIdentity.commit,
    reusedCommit,
  })));
  assert.deepEqual(resolved, cellIds.map(() => reusedCommit));
  // The ledger states what was actually inherited: the earlier run, its commit, and its tree.
  for (const cellId of cellIds) {
    const row = accepted.ledger.cells.find(({ id }) => id === cellId);
    assert.equal(row.status, "pass", cellId);
    assert.equal(row.identity.producer_run_id, reusedRunId, cellId);
    assert.equal(row.identity.commit, reusedCommit, cellId);
    assert.equal(row.identity.source_tree, reusedTree, cellId);
  }
  // Nothing else moved: cells that were not reused stay bound to the publishing run and tree.
  const packaged = accepted.ledger.cells.find(({ id }) => id === "package_identity:windows-x64");
  assert.equal(packaged.identity.commit, gitIdentity.commit);
  assert.equal(packaged.identity.source_tree, gitIdentity.source_tree);
});

test("a native-fingerprint reuse row is refused wherever the equation stops holding", () => {
  // The accept path above buys exactly one substitution, under one proof. Each row here breaks a
  // different part of that and must still reject -- the equated key included, because equating an
  // identity is not dropping it: the row still has to carry the reused commit's own value for it.
  const rejections = [
    ["the row names a binding the group did not declare", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests, { binding: "source_tree" });
    }, "reuses evidence under an undeclared binding", fingerprintReuse()],
    ["the reproved fingerprint is not the one the producer recorded", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests, { binding_value: "e".repeat(64) });
    }, "recorded native_fingerprint value does not bind this release", fingerprintReuse()],
    ["the two commits' native fingerprints differ", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests);
    }, "native_fingerprint reuse is unverified", {
      verify: () => {
        throw new Error("native fingerprint of reused commit does not match the release commit");
      },
      resolve: fingerprintReuse().resolve,
    }],
    ["the reused commit is not an ancestor of the release commit", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests);
    }, "is not an ancestor of the release commit", fingerprintReuse({ ancestors: [] })],
    ["the reused artifact expired", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests)[0].artifact.expired = true;
    }, "artifact is expired", fingerprintReuse()],
    // The equated identity, checked at its source. A reused row may be read at this release's
    // tree only because it carries the reused commit's tree; a row naming some third tree is not
    // the evidence the fingerprint proved anything about.
    ["the row declares a tree that is not the reused commit's", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests);
      manifests.find(({ cell_id: cellId }) => cellId === acceleratorCellIds()[0])
        .evidence.identity.source_tree = "e".repeat(40);
    }, "manifest source_tree is not the reused commit's source_tree the binding equates",
    fingerprintReuse()],
    ["the row declares this release's tree, which the reused run could not have produced it at",
      (trusted, manifests) => {
        reuseAcceleratorExecution(trusted, manifests);
        manifests.find(({ cell_id: cellId }) => cellId === acceleratorCellIds()[0])
          .evidence.identity.source_tree = gitIdentity.source_tree;
      }, "manifest source_tree is not the reused commit's source_tree the binding equates",
      fingerprintReuse()],
    // Keys the fingerprint does not determine are not equated, and are compared as written.
    ["the row was produced in another repository", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests);
      manifests.find(({ cell_id: cellId }) => cellId === acceleratorCellIds()[0])
        .evidence.identity.repository = "TheGreenCedar/NotCodeStory";
    }, "identity repository does not match the requested release", fingerprintReuse()],
    ["the row was produced at another version, which the fingerprint normalizes away rather than proves",
      (trusted, manifests) => {
        reuseAcceleratorExecution(trusted, manifests);
        manifests.find(({ cell_id: cellId }) => cellId === acceleratorCellIds()[0])
          .evidence.identity.producer_version = "0.15.9";
      }, "producer_version must equal closeout version", fingerprintReuse()],
    ["the row exercised an archive this release does not ship", (trusted, manifests) => {
      reuseAcceleratorExecution(trusted, manifests);
      manifests.find(({ cell_id: cellId }) => cellId === acceleratorCellIds()[0])
        .evidence.identity.artifact_sha256 = sha("some other release archive");
    }, "identity artifact_sha256 does not match the requested release", fingerprintReuse()],
  ];
  for (const [label, mutate, expected, proof] of rejections) {
    const manifests = manifestsFor("pre_publish");
    const trusted = trustedProducersFor("pre_publish");
    mutate(trusted, manifests);
    const rejected = evaluate(
      "pre_publish",
      manifests,
      null,
      trusted,
      null,
      null,
      proof.verify,
      proof.resolve,
    );
    assert.equal(rejected.decision, "reject", label);
    const messages = refusals(rejected);
    assert.ok(
      messages.some((message) => message.includes(expected)),
      `${label}: ${JSON.stringify(messages)}`,
    );
  }
});

test("a closeout that cannot read the reused commit refuses to equate anything", () => {
  // The binding proof alone does not say what is being equated away. Without this checkout's own
  // reading of the reused commit, the release's tree would be standing in for whatever the row
  // claimed -- so reuse is refused outright rather than granted on the producer map's word.
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseAcceleratorExecution(trusted, manifests);
  const rejected = evaluate(
    "pre_publish",
    manifests,
    null,
    trusted,
    null,
    null,
    fingerprintReuse().verify,
  );
  assert.equal(rejected.decision, "reject");
  for (const cellId of acceleratorCellIds()) {
    assert.ok(
      rejected.summary.input_errors.some((message) =>
        message.includes(`${cellId} equates source_tree this closeout cannot resolve`)),
      `${cellId}: ${JSON.stringify(rejected.summary.input_errors)}`,
    );
  }
});

test("a reused accelerator container is still bound by its digest", () => {
  const manifests = manifestsFor("pre_publish");
  const trusted = trustedProducersFor("pre_publish");
  reuseAcceleratorExecution(trusted, manifests);
  const [cellId] = acceleratorCellIds();
  const artifactBindings = manifests.map((manifest) => {
    const producer = trusted.producers.find(({ cell_id: candidate }) => candidate === manifest.cell_id);
    return {
      cell_id: manifest.cell_id,
      producer_artifact: producer.producer_artifact,
      artifact_id: producer.artifact.id,
      artifact_digest: producer.artifact.digest,
      manifest_sha256: canonicalManifestSha(manifest),
    };
  });
  artifactBindings.find(({ cell_id: candidate }) => candidate === cellId)
    .artifact_digest = `sha256:${"f".repeat(64)}`;
  const proof = fingerprintReuse();
  const rejected = evaluate(
    "pre_publish",
    manifests,
    null,
    trusted,
    null,
    artifactBindings,
    proof.verify,
    proof.resolve,
  );
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes(cellId));
  assert.ok(rejected.evaluations.get(cellId).value.failures.some((message) =>
    message.includes("artifact_digest does not match Actions provenance")));
});

// ── Withheld accelerator claims ─────────────────────────────────────────────────────────────

const withheldAttempt = String(nonClaimPolicy.maximum_run_attempts);

function withheldPrePublish() {
  return {
    manifests: manifestsFor("pre_publish", null, { attempt: withheldAttempt, withheldHost: linuxHost }),
    trustedProducers: trustedProducersFor("pre_publish", linuxHost),
  };
}

function cellOf(id) {
  return deriveReleaseCells(graph, "post_publish").find(({ id: candidate }) => candidate === id);
}

test("a lost host is recorded as an explicit withheld claim, never as a pass and never as a gap", () => {
  const { manifests, trustedProducers } = withheldPrePublish();
  const result = evaluate("pre_publish", manifests, null, trustedProducers);

  // The release is not lost, but the accepted ledger says out loud what it did not prove.
  assert.equal(result.decision, "accept");
  assert.deepEqual(result.summary.missing_cells, []);
  assert.deepEqual(result.summary.failed_cells, []);
  assert.deepEqual(
    result.summary.withheld_cells,
    ["accelerator_execution:linux-x64-vulkan", "candidate_installed_behavior:linux-x64"],
  );
  assert.equal(result.summary.counts.withheld, 2);
  // A withheld cell is never counted as proven.
  assert.equal(
    result.summary.counts.passed,
    result.summary.counts.required - result.summary.counts.withheld,
  );
  // Linux stopped proving the accelerator, but macOS and Windows did not, so the claim is named as
  // partially withheld. `withheld_claims` stays literal: it is what nothing in the phase proved.
  assert.ok(result.summary.partially_withheld_claims.includes("accelerator_execution"));
  assert.deepEqual(result.summary.withheld_hosts, ["linux-x64-vulkan"]);
  assert.equal(result.summary.withheld_claims.includes("accelerator_execution"), false);

  const row = result.ledger.cells.find(({ id }) => id === "accelerator_execution:linux-x64-vulkan");
  assert.equal(row.status, "withheld");
  assert.equal(row.non_claim.runtime_execution, "not_proven_by_package");
  assert.equal(row.non_claim.non_claim_reason, "accelerator_host_unavailable");
  assert.equal(row.non_claim.unavailable_producer_job_name, "Packaged Linux Vulkan engine");
  assert.deepEqual(row.withheld_claims, ["accelerator_execution", "package_identity", "source_behavior"]);
  // The withheld row still names the host, backend, and target the missing proof would have used.
  assert.equal(row.identity.backend, "Vulkan");
  assert.equal(row.identity.target, "linux-x64");
  assert.equal(row.identity.producer_job, nonClaimPolicy.producer_job);

  const evaluation = result.evaluations.get("accelerator_execution:linux-x64-vulkan").value;
  assert.equal(evaluation.status, "withheld");
  assert.equal(evaluation.release_claim_evaluation, undefined);

  // Every host that did report is untouched and still passes on its own proof.
  const windows = result.ledger.cells.find(({ id }) => id === "accelerator_execution:windows-x64-vulkan");
  assert.equal(windows.status, "pass");
  assert.equal(windows.non_claim, undefined);
});

test("nothing may pass on top of a withheld claim, and nothing else may be withheld", () => {
  // Withhold only the accelerator cell and let everything the Linux host produces keep claiming a
  // pass. Retrieval readiness rests on proven accelerator execution, so it must fail rather than
  // inherit an unexamined pass.
  const acceleratorOnly = {
    ...linuxHost,
    withheld_cells: ["accelerator_execution:linux-x64-vulkan"],
  };
  const prePublish = evaluate(
    "pre_publish",
    manifestsFor("pre_publish", null, { attempt: withheldAttempt, withheldHost: acceleratorOnly }),
    null,
    trustedProducersFor("pre_publish", acceleratorOnly),
  );
  const postManifests = manifestsFor("post_publish", prePublish.ledger, {
    attempt: withheldAttempt,
    withheldHost: acceleratorOnly,
  });
  const cascaded = evaluate(
    "post_publish",
    postManifests,
    prePublish.ledger,
    trustedProducersFor("post_publish", acceleratorOnly),
  );
  assert.equal(
    cascaded.ledger.cells.find(({ id }) => id === "retrieval_readiness:linux-x64").status,
    "fail",
  );
  assert.ok(cascaded.evaluations.get("retrieval_readiness:linux-x64").value.failures.some((message) =>
    message.includes("is withheld")));
  assert.equal(cascaded.decision, "reject");

  // A cell the graph never declared withholdable cannot be withheld into an accepted ledger.
  const ineligible = withheldPrePublish();
  const source = ineligible.manifests.find(({ cell_id: id }) => id === "source_behavior");
  source.evidence.status = "withheld";
  source.non_claim = nonClaimFor(cellOf("source_behavior"), linuxHost, withheldAttempt);
  const rejectedSource = evaluate("pre_publish", ineligible.manifests, null, ineligible.trustedProducers);
  assert.equal(rejectedSource.decision, "reject");
  assert.ok(rejectedSource.evaluations.get("source_behavior").value.failures.some((message) =>
    message.includes("does not admit a withheld non-claim")));

  // A withheld manifest paired with a real proof producer, or the reverse, is a mismatch.
  const mismatch = withheldPrePublish();
  delete mismatch.trustedProducers.producers
    .find(({ cell_id: id }) => id === "accelerator_execution:linux-x64-vulkan").non_claim;
  const rejectedMismatch = evaluate("pre_publish", mismatch.manifests, null, mismatch.trustedProducers);
  assert.equal(rejectedMismatch.decision, "reject");

  const forged = withheldPrePublish();
  const proven = forged.manifests
    .find(({ cell_id: id }) => id === "accelerator_execution:windows-x64-vulkan");
  proven.non_claim = nonClaimFor(
    cellOf("accelerator_execution:windows-x64-vulkan"),
    linuxHost,
    withheldAttempt,
  );
  const rejectedForged = evaluate("pre_publish", forged.manifests, null, forged.trustedProducers);
  assert.equal(rejectedForged.decision, "reject");
  assert.ok(rejectedForged.evaluations.get("accelerator_execution:windows-x64-vulkan").value.failures
    .some((message) => message.includes("only a withheld cell may carry a non-claim")));
});

test("a withheld cell must carry a complete, unspent-bound, honest non-claim", () => {
  const cellId = "accelerator_execution:linux-x64-vulkan";
  const cases = [
    ["absent non-claim", (manifest) => {
      delete manifest.non_claim;
    }, /non_claim must be an object/u],
    ["downgraded runtime execution", (manifest) => {
      manifest.non_claim.runtime_execution = "proven_by_package";
    }, /runtime_execution must equal not_proven_by_package/u],
    ["invented reason", (manifest) => {
      manifest.non_claim.non_claim_reason = "we_were_in_a_hurry";
    }, /non_claim_reason must equal accelerator_host_unavailable/u],
    ["mis-quoted annotation", (manifest) => {
      manifest.non_claim.annotation = "Process completed with exit code 1.";
    }, /annotation must equal/u],
    ["shrunken withheld claim list", (manifest) => {
      manifest.non_claim.withheld_claims = ["accelerator_execution"];
    }, /withheld_claims must name/u],
    ["unspent retry bound", (manifest) => {
      manifest.non_claim.run_attempt = "1";
    }, /recovery bound/u],
    ["wrong unavailable host", (manifest) => {
      manifest.non_claim.unavailable_producer_job_name = "Packaged Windows Vulkan engine";
    }, /unavailable_producer_job_name must equal/u],
  ];
  for (const [label, mutate, pattern] of cases) {
    const { manifests, trustedProducers } = withheldPrePublish();
    mutate(manifests.find(({ cell_id: id }) => id === cellId));
    const result = evaluate("pre_publish", manifests, null, trustedProducers);
    assert.equal(result.decision, "reject", label);
    assert.equal(result.ledger.cells.find(({ id }) => id === cellId).status, "fail", label);
    assert.ok(
      result.evaluations.get(cellId).value.failures.some((message) => pattern.test(message)),
      `${label}: ${JSON.stringify(result.evaluations.get(cellId).value.failures)}`,
    );
  }
});

// ── The withhold cap ────────────────────────────────────────────────────────────────────────

const withholdPolicy = nonClaimPolicy.withhold_policy;

function withheldPrePublishHosts(hosts) {
  return {
    manifests: manifestsFor("pre_publish", null, { attempt: withheldAttempt, withheldHost: hosts }),
    trustedProducers: trustedProducersFor("pre_publish", hosts),
  };
}

test("the withhold cap is graph data and leaves at least one protected host proven", () => {
  assert.equal(withholdPolicy.maximum_withheld_hosts, 1);
  assert.ok(withholdPolicy.maximum_withheld_hosts < nonClaimPolicy.hosts.length);
  assert.ok(withholdPolicy.claims_requiring_proof.includes("accelerator_execution"));
});

test("a release that withholds every accelerator host is refused, not published", () => {
  const { manifests, trustedProducers } = withheldPrePublishHosts(nonClaimPolicy.hosts);
  const result = evaluate("pre_publish", manifests, null, trustedProducers);

  // The exact shape the reviewer published: six of ten required cells withheld and accepted.
  assert.equal(result.summary.counts.withheld, 6);
  assert.equal(result.summary.counts.failed, 0);
  assert.equal(result.summary.counts.missing, 0);
  assert.equal(result.decision, "reject");
  assert.deepEqual(
    result.summary.withheld_hosts,
    ["linux-x64-vulkan", "macos-arm64-metal", "windows-x64-vulkan"],
  );
  // Refusal is a recorded state naming the cap it broke, never a silent absence.
  assert.ok(
    result.summary.input_errors.some((message) => /exceed the 1-host withhold cap/u.test(message)),
    JSON.stringify(result.summary.input_errors),
  );
  assert.ok(
    result.summary.input_errors.some((message) =>
      message === "claim accelerator_execution requires proof but no cell proved it"),
    JSON.stringify(result.summary.input_errors),
  );
  assert.ok(
    result.summary.input_errors.some((message) =>
      message === "claim installed_runtime_behavior requires proof but no cell proved it"),
    JSON.stringify(result.summary.input_errors),
  );
  assert.deepEqual(result.ledger.withhold_policy, {
    claims_requiring_proof: [...withholdPolicy.claims_requiring_proof],
    maximum_withheld_hosts: withholdPolicy.maximum_withheld_hosts,
  });
});

test("two withheld hosts already break the cap even though a third still proves the claim", () => {
  const two = nonClaimPolicy.hosts.filter(({ id }) => id !== "windows-x64-vulkan");
  const { manifests, trustedProducers } = withheldPrePublishHosts(two);
  const result = evaluate("pre_publish", manifests, null, trustedProducers);
  assert.equal(result.summary.counts.withheld, 4);
  assert.equal(result.decision, "reject");
  assert.deepEqual(result.summary.withheld_hosts, ["linux-x64-vulkan", "macos-arm64-metal"]);
  assert.ok(
    result.summary.input_errors.some((message) =>
      message === "withheld hosts linux-x64-vulkan, macos-arm64-metal exceed the 1-host withhold cap"),
    JSON.stringify(result.summary.input_errors),
  );
  // Windows still proved the accelerator, so the per-claim rule alone would not have caught this.
  assert.ok(
    !result.summary.input_errors.some((message) => /requires proof/u.test(message)),
    JSON.stringify(result.summary.input_errors),
  );
});

test("a withheld cell never satisfies a claim the policy requires proof for", () => {
  // One host is inside the cap, so only the per-claim rule can speak here: strip the two proving
  // accelerator cells to missing and the withheld third must not stand in for them.
  const { manifests, trustedProducers } = withheldPrePublish();
  const surviving = new Set([
    "accelerator_execution:macos-arm64-metal",
    "accelerator_execution:windows-x64-vulkan",
  ]);
  const thinned = manifests.filter(({ cell_id: id }) => !surviving.has(id));
  const result = evaluate("pre_publish", thinned, null, trustedProducers);
  assert.equal(result.decision, "reject");
  assert.deepEqual(result.summary.withheld_hosts, ["linux-x64-vulkan"]);
  assert.ok(
    result.summary.input_errors.some((message) =>
      message === "claim accelerator_execution requires proof but no cell proved it"),
    JSON.stringify(result.summary.input_errors),
  );
});

test("the published platform notes state a withheld accelerator instead of asserting it", () => {
  // End to end through the real programs: the real closeout writes a real ledger, and the real
  // release-notes command reads it. The graph still says Linux is a Vulkan platform; only the
  // ledger knows this release did not prove it, which is why the notes may not be rendered from
  // the graph alone.
  const { manifests, trustedProducers } = withheldPrePublish();
  const accepted = evaluate("pre_publish", manifests, null, trustedProducers);
  assert.equal(accepted.decision, "accept");
  const out = mkdtempSync(path.join(os.tmpdir(), "codestory-withheld-notes-"));
  writeReleaseCloseout(out, accepted);

  const render = (ledgerPath) => spawnSync(
    process.execPath,
    [
      path.join(root, "scripts/codestory-release-claims.mjs"),
      "release-platform-notes",
      "--ledger",
      ledgerPath,
    ],
    { encoding: "utf8" },
  );
  const withheldNotes = render(path.join(out, "ledger.json"));
  assert.equal(withheldNotes.status, 0, withheldNotes.stderr);
  assert.match(
    withheldNotes.stdout,
    /^- Linux x64: Vulkan not proven for this release \(accelerator_host_unavailable\)$/mu,
  );
  assert.equal(/^- Linux x64: supported with Vulkan$/mu.test(withheldNotes.stdout), false);
  // The hosts that did prove their accelerator still say so.
  assert.match(withheldNotes.stdout, /^- Windows x64: supported with Vulkan$/mu);
  assert.match(withheldNotes.stdout, /^- macOS 15\+ on Apple Silicon: supported with Metal$/mu);
  assert.match(withheldNotes.stdout, /release-closeout-summary\.json/u);

  // A fully proven release is unchanged, so the honest wording costs nothing when nothing was lost.
  const provenOut = mkdtempSync(path.join(os.tmpdir(), "codestory-proven-notes-"));
  writeReleaseCloseout(provenOut, evaluate("pre_publish", manifestsFor("pre_publish")));
  const provenNotes = render(path.join(provenOut, "ledger.json"));
  assert.equal(provenNotes.status, 0, provenNotes.stderr);
  assert.match(provenNotes.stdout, /^- Linux x64: supported with Vulkan$/mu);
  assert.equal(/not proven for this release/u.test(provenNotes.stdout), false);

  // Without a ledger the command refuses outright: the graph alone can never publish a claim.
  const ungrounded = spawnSync(
    process.execPath,
    [path.join(root, "scripts/codestory-release-claims.mjs"), "release-platform-notes"],
    { encoding: "utf8" },
  );
  assert.notEqual(ungrounded.status, 0);
  assert.match(ungrounded.stderr, /--ledger/u);
});

test("withheld_claims names only what nothing proved, and says so separately for the rest", () => {
  const { manifests, trustedProducers } = withheldPrePublish();
  const result = evaluate("pre_publish", manifests, null, trustedProducers);
  assert.equal(result.decision, "accept");

  // package_identity:linux-x64 and source_behavior passed in this very ledger, so reporting their
  // claims as withheld was the thing a reader could not take literally.
  assert.deepEqual(result.summary.withheld_claims, []);
  assert.deepEqual(
    result.summary.partially_withheld_claims,
    ["accelerator_execution", "installed_runtime_behavior", "package_identity", "source_behavior"],
  );
  for (const claimId of result.summary.partially_withheld_claims) {
    const proven = result.ledger.cells.filter(({ claim, status }) =>
      claim === claimId && new Set(["pass", "pass_with_exception"]).has(status));
    assert.ok(proven.length > 0, `${claimId} is reported partial but nothing proved it`);
  }
  // Together the two lists still name every claim a withheld cell rested on: nothing is dropped.
  assert.deepEqual(
    [...result.summary.withheld_claims, ...result.summary.partially_withheld_claims].sort(),
    [...new Set(result.ledger.cells
      .filter(({ status }) => status === "withheld")
      .flatMap(({ withheld_claims: rows }) => rows))].sort(),
  );
});

// Catalog publication is delivery, not a release gate, so a release may legitimately end with the
// public catalog untouched. The whole risk in allowing that is the deferred run reading as the
// published one, and the installer identity in the post-publish cells is the only thing that
// distinguishes them. Before these checks it was inert: a free-form string nothing read, so a
// deferred release's verdict was identical in shape to a published one.
test("the post-publish closeout resolves and records which catalog served the release", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const published = evaluate(
    "post_publish",
    manifestsFor("post_publish", prePublish.ledger),
    prePublish.ledger,
  );
  assert.equal(published.decision, "accept");
  assert.deepEqual(published.ledger.catalog_delivery, {
    state: "published",
    installer: "codex_marketplace_install",
    live_catalog_revision: true,
  });
  assert.deepEqual(published.summary.catalog_delivery, published.ledger.catalog_delivery);

  // A deferred release is accepted -- it published real artifacts -- but its verdict says so.
  const deferredManifests = manifestsFor("post_publish", prePublish.ledger);
  for (const manifest of deferredManifests) {
    if (manifest.cell_id.startsWith("installed_runtime_behavior:")) {
      manifest.evidence.identity.installer = "codex_marketplace_deferred_fixture";
    }
  }
  const deferred = evaluate("post_publish", deferredManifests, prePublish.ledger);
  assert.equal(deferred.decision, "accept");
  assert.deepEqual(deferred.ledger.catalog_delivery, {
    state: "deferred",
    installer: "codex_marketplace_deferred_fixture",
    live_catalog_revision: false,
  });
  // The two states must be distinguishable in the signed verdict, not only to a human reading a
  // warning in a log that expires.
  assert.notDeepEqual(deferred.ledger.catalog_delivery, published.ledger.catalog_delivery);

  // The pre-publish closeout has no post-publish installed cells, so it states nothing here
  // rather than inventing a delivery state.
  assert.equal(prePublish.ledger.catalog_delivery, undefined);
});

test("a post-publish closeout that cannot resolve one catalog delivery state is rejected", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));

  // An installer identity no delivery state declares -- including the pre-publish lane's own
  // candidate installer, which would otherwise sail through as a plausible-looking string.
  for (const installer of ["candidate_managed_plugin", "managed_plugin", ""]) {
    const manifests = manifestsFor("post_publish", prePublish.ledger);
    for (const manifest of manifests) {
      if (manifest.cell_id.startsWith("installed_runtime_behavior:")) {
        manifest.evidence.identity.installer = installer;
      }
    }
    const rejected = evaluate("post_publish", manifests, prePublish.ledger);
    assert.equal(rejected.decision, "reject", installer);
    assert.equal(rejected.ledger.catalog_delivery.state, "unresolved", installer);
    assert.ok(
      rejected.ledger.input_errors.some((message) =>
        message.includes("does not record a declared catalog delivery installer identity")),
      `${installer}: ${JSON.stringify(rejected.ledger.input_errors)}`,
    );
  }

  // Targets disagreeing about the delivery state is not a state; it is a broken release.
  const split = manifestsFor("post_publish", prePublish.ledger);
  split.find(({ cell_id: id }) => id === "installed_runtime_behavior:macos-arm64")
    .evidence.identity.installer = "codex_marketplace_deferred_fixture";
  const rejected = evaluate("post_publish", split, prePublish.ledger);
  assert.equal(rejected.decision, "reject");
  assert.equal(rejected.ledger.catalog_delivery.state, "unresolved");
  assert.ok(
    rejected.ledger.input_errors.some((message) =>
      message.includes("disagree on the catalog delivery state")),
    JSON.stringify(rejected.ledger.input_errors),
  );
});
