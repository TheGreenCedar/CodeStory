import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { produceReleaseCellManifest } from "../codestory-release-cell-manifest.mjs";
import { deriveReleaseCells } from "../codestory-release-closeout.mjs";
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
  return {
    producer_workflow: selected.identity_constraints.producer_workflow,
    producer_job: selected.identity_constraints.producer_job,
    producer_run_id: "12345",
    producer_run_attempt: "2",
    producer_artifact: selected.identity_constraints.producer_artifact,
  };
}

test("package manifest binds workflow artifact identity separately from archive bytes", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-linux-x64.tar.gz");
  writeFileSync(archive, "release archive bytes");
  const selected = cell("package_identity:linux-x64");
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
  assert.equal(manifest.evidence.identity.producer_artifact, "release-cell-prepublish-package-linux-x64");
  assert.equal(manifest.archive.name, "codestory-cli-v0.16.0-linux-x64.tar.gz");
  assert.equal(manifest.archive.sha256, manifest.evidence.identity.artifact_sha256);
});

test("post-publish byte producer refuses an archive that differs from the accepted ledger", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-cell-"));
  const archive = path.join(directory, "codestory-cli-v0.16.0-linux-x64.tar.gz");
  writeFileSync(archive, "published replacement bytes");
  const selected = cell("post_publish_bytes:linux-x64");
  const prePublishLedger = {
    phase: "pre_publish",
    decision: "accept",
    cells: [{
      id: "package_identity:linux-x64",
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

test("release-evidence wrappers retain upstream evidence ids and statuses", () => {
  const selected = cell("performance");
  const upstream = {
    id: "performance-measurement-1",
    type: "performance",
    tier: "live_behavior",
    status: "pass_with_exception",
    graph_sha256: "0".repeat(64),
    observed_at: observedAt,
    expires_at: "2026-07-20T12:00:00.000Z",
    identity: {
      profile: "codestory-release-evidence-linux-arm64-v2",
      corpus_id: "v0.16-corpus",
      cache_id: "cold-full-retrieval-v1",
      machine_fingerprint: "fixture/linux-arm64",
      baseline_id: "release-baseline@1111111",
      baseline_sha256: "b".repeat(64),
      release_key: "release-0.16.0",
    },
  };
  const manifest = produceReleaseCellManifest({
    graph,
    gitIdentity,
    version,
    cell: selected,
    identity: {},
    producer: producer(selected),
    evidence: upstream,
  });
  assert.equal(manifest.evidence.id, upstream.id);
  assert.equal(manifest.evidence.status, upstream.status);
  assert.equal(manifest.evidence.graph_sha256, manifest.graph_sha256);
});

test("all graph-derived cells fix workflow, job, and artifact producer identity", () => {
  for (const selected of deriveReleaseCells(graph, "post_publish")) {
    for (const key of ["producer_workflow", "producer_job", "producer_artifact"]) {
      assert.equal(typeof selected.identity_constraints[key], "string", `${selected.id} lacks ${key}`);
      assert.notEqual(selected.identity_constraints[key], "");
    }
  }
});
