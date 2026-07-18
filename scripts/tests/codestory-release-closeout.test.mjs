import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  deriveReleaseCells,
  evaluateReleaseCloseout,
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

function identityFor(cell) {
  const target = cell.identity_constraints.target;
  const identity = { ...gitIdentity };
  for (const key of cell.required_identity) {
    if (identity[key] !== undefined) continue;
    if (cell.identity_constraints[key] !== undefined) {
      identity[key] = cell.identity_constraints[key];
      continue;
    }
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
      case "producer_run_attempt": identity[key] = "1"; break;
      case "producer_artifact": identity[key] = target ? archiveName(target) : `${cell.id}.json`; break;
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
        name: identity.producer_artifact,
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
        published_artifact_sha256: packageCell.archive.sha256,
      };
    }
    return manifest;
  });
}

function evaluate(phase, manifests, prePublishLedger = null) {
  return evaluateReleaseCloseout({
    graph,
    phase,
    version,
    evaluatedAt,
    gitIdentity,
    manifests,
    prePublishLedger,
  });
}

test("cell inventory is derived only from the release claim graph", () => {
  const prePublish = deriveReleaseCells(graph, "pre_publish");
  const postPublish = deriveReleaseCells(graph, "post_publish");
  assert.equal(prePublish.length, 12);
  assert.equal(postPublish.length, 30);
  assert.deepEqual(
    prePublish.filter(({ group_id }) => group_id === "package_identity").map(({ identity_constraints }) => identity_constraints.target),
    graph.workflow_policy.package_matrix.map(({ asset_target: assetTarget }) => assetTarget).sort(),
  );
  assert.deepEqual(
    postPublish.find(({ id }) => id === "platform_support:linux-arm64").identity_constraints,
    {
      producer_workflow: ".github/workflows/post-publish-release-smoke.yml",
      target: "linux-arm64",
      host_os: "Linux",
      host_arch: "ARM64",
    },
  );

  const changed = structuredClone(graph);
  changed.workflow_policy.package_matrix[0].asset_target = "linux-future";
  const changedCells = deriveReleaseCells(changed, "pre_publish");
  assert.ok(changedCells.some(({ id }) => id === "package_identity:linux-future"));
  assert.ok(!changedCells.some(({ id }) => id === "package_identity:linux-x64"));
});

test("accepted pre-publish closeout retains one manifest and evaluation per cell deterministically", () => {
  const manifests = manifestsFor("pre_publish");
  const first = evaluate("pre_publish", manifests);
  const second = evaluate("pre_publish", structuredClone(manifests));
  assert.equal(first.decision, "accept");
  assert.deepEqual(first.ledger, second.ledger);
  assert.deepEqual(first.summary, second.summary);
  assert.equal(first.summary.counts.required, 12);
  assert.equal(first.summary.counts.passed, 12);
  assert.equal(first.retainedManifests.size, 12);
  assert.equal(first.evaluations.size, 12);

  const out = mkdtempSync(path.join(os.tmpdir(), "codestory-release-closeout-"));
  writeReleaseCloseout(out, first);
  assert.equal(readdirSync(path.join(out, "manifests")).length, 12);
  assert.equal(readdirSync(path.join(out, "evaluations")).length, 12);
  assert.deepEqual(JSON.parse(readFileSync(path.join(out, "ledger.json"))), first.ledger);
  assert.deepEqual(JSON.parse(readFileSync(path.join(out, "summary.json"))), first.summary);
});

test("post-publish closeout compares every downloaded archive with the retained pre-publish bytes", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const manifests = manifestsFor("post_publish", prePublish.ledger);
  const postPublish = evaluate("post_publish", manifests, prePublish.ledger);
  assert.equal(postPublish.decision, "accept");
  assert.equal(postPublish.summary.counts.required, 30);
  assert.equal(
    postPublish.ledger.cells.filter(({ id }) => id.startsWith("post_publish_bytes:")).length,
    graph.workflow_policy.package_matrix.length,
  );

  const changed = structuredClone(manifests);
  const bytes = changed.find(({ cell_id }) => cell_id === "post_publish_bytes:linux-x64");
  bytes.comparison.published_artifact_sha256 = "d".repeat(64);
  bytes.evidence.identity.artifact_sha256 = "d".repeat(64);
  const rejected = evaluate("post_publish", changed, prePublish.ledger);
  assert.equal(rejected.decision, "reject");
  assert.ok(rejected.summary.failed_cells.includes("post_publish_bytes:linux-x64"));
});

test("hostile post-publish A/B split cannot replace the retained package used by platform proof", () => {
  const prePublish = evaluate("pre_publish", manifestsFor("pre_publish"));
  const manifests = manifestsFor("post_publish", prePublish.ledger);
  const replacementSha256 = "d".repeat(64);
  for (const cellId of [
    "package_identity:linux-x64",
    "platform_support:linux-x64",
    "installed_runtime_behavior:linux-x64",
  ]) {
    const manifest = manifests.find(({ cell_id: id }) => id === cellId);
    manifest.evidence.identity.artifact_sha256 = replacementSha256;
    if (manifest.archive) manifest.archive.sha256 = replacementSha256;
  }

  const rejected = evaluate("post_publish", manifests, prePublish.ledger);
  assert.equal(rejected.decision, "reject");
  for (const cellId of [
    "package_identity:linux-x64",
    "platform_support:linux-x64",
    "installed_runtime_behavior:linux-x64",
  ]) {
    assert.ok(rejected.summary.failed_cells.includes(cellId));
  }
  assert.ok(rejected.evaluations.get("package_identity:linux-x64").value.failures.some((message) =>
    message.includes("retained pre-publish manifest")));
  assert.ok(rejected.evaluations.get("platform_support:linux-x64").value.failures.some((message) =>
    message.includes("dependency cell package_identity:linux-x64")));
});

test("hostile producer and runtime semver claims must equal the independently supplied closeout version", () => {
  const preManifests = manifestsFor("pre_publish");
  preManifests.find(({ cell_id: id }) => id === "package_identity:linux-x64")
    .evidence.identity.producer_version = "0.15.0";
  const rejectedPrePublish = evaluate("pre_publish", preManifests);
  assert.equal(rejectedPrePublish.decision, "reject");
  assert.ok(rejectedPrePublish.summary.failed_cells.includes("package_identity:linux-x64"));

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
  platformMismatch.find(({ cell_id: id }) => id === "platform_support:linux-x64")
    .evidence.identity.host_os = "Windows";
  const rejectedPlatform = evaluate("post_publish", platformMismatch, prePublish.ledger);
  assert.equal(rejectedPlatform.decision, "reject");
  assert.ok(rejectedPlatform.summary.failed_cells.includes("platform_support:linux-x64"));

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
