import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  evaluateReleaseClaims,
  deriveTrustedGitIdentity,
  loadReleaseClaimGraph,
  releaseClaimGraphDigest,
  validateReleaseClaimGraph,
} from "../codestory-release-claims.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = path.join(root, "scripts/tests/fixtures/release-claims");
const graph = loadReleaseClaimGraph(root);

function readJson(name) {
  return JSON.parse(readFileSync(path.join(fixtureRoot, name), "utf8"));
}

function positiveFixture() {
  return readJson("positive.json");
}

function pointer(document, pointerPath) {
  const segments = pointerPath.split("/").slice(1).map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
  let value = document;
  for (const segment of segments) value = value[segment];
  return value;
}

function applyOperations(document, operations) {
  for (const operation of operations) {
    const segments = operation.path.split("/").slice(1);
    const key = segments.pop();
    let parent = document;
    for (const segment of segments) parent = parent[segment];
    if (operation.op === "remove") parent.splice(Number(key), 1);
    else if (operation.op === "replace" || operation.op === "add") parent[key] = operation.value;
    else if (operation.op === "append_clone") {
      parent[key].push({ ...structuredClone(pointer(document, operation.source)), ...structuredClone(operation.patch) });
    } else throw new Error(`unsupported fixture operation ${operation.op}`);
  }
  return document;
}

function evaluate(fixture) {
  return evaluateReleaseClaims({
    graph,
    requested_claims: fixture.requested_claims,
    evidence: fixture.evidence,
    expected: {
      commit: fixture.expected_sha,
      evaluated_at: fixture.evaluated_at,
      identity: fixture.expected_identity,
    },
  });
}

function releaseEvidenceFixture() {
  const candidateBytes = readFileSync(path.join(
    root,
    "benchmarks/release-evidence/fixtures/candidate.json",
  ));
  const candidate = JSON.parse(candidateBytes);
  const document = structuredClone(candidate.release_claims);
  for (const row of document.evidence) row.status = "pass";
  const common = document.evidence[0].identity;
  const performance = document.evidence.find(({ type }) => type === "performance").identity;
  const answerQuality = document.evidence.find(({ type }) => type === "answer_quality").identity;
  return {
    expected_sha: candidate.commit,
    evaluated_at: document.observed_at,
    expected_identity: {
      repository: common.repository,
      source_tree: common.source_tree,
      profile: common.profile,
      corpus_id: common.corpus_id,
      cache_id: common.cache_id,
      machine_fingerprint: common.machine_fingerprint,
      baseline_id: performance.baseline_id,
      baseline_sha256: performance.baseline_sha256,
      candidate_sha256: createHash("sha256").update(candidateBytes).digest("hex"),
      release_key: common.release_key,
      artifact_sha256: answerQuality.artifact_sha256,
    },
    requested_claims: document.requested_claims,
    evidence: document.evidence,
  };
}

test("versioned claim graph has one deterministic digest and all declared controls", () => {
  assert.doesNotThrow(() => validateReleaseClaimGraph(structuredClone(graph)));
  assert.match(releaseClaimGraphDigest(graph), /^[0-9a-f]{64}$/u);
  assert.equal(positiveFixture().evidence[0].graph_sha256, releaseClaimGraphDigest(graph));
  assert.equal(graph.claims.length, 8);
  assert.deepEqual(
    graph.failure_controls.map(({ id }) => id).sort(),
    [
      "benchmark_leakage",
      "observational_read_mutation",
      "project_identity_drift",
      "sidecar_runtime_mismatch",
      "stale_or_partial_publication",
    ],
  );
  assert.ok(graph.claims.every((claim) => claim.prerequisite_checks.every(({ command }) => command.length > 0)));
});

test("positive fixture evaluates deterministically", () => {
  const fixture = positiveFixture();
  const first = evaluate(fixture);
  const second = evaluate(structuredClone(fixture));
  assert.deepEqual(first, second);
  assert.equal(first.status, "pass");
  assert.deepEqual(first.failures, []);
  assert.equal(first.evidence_selection, "all_matching_rows_must_pass");
});

test("controlled negative fixtures emit stable machine failure classes", async (t) => {
  for (const fixtureCase of readJson("negative.json").cases) {
    await t.test(fixtureCase.id, () => {
      const fixture = applyOperations(positiveFixture(), fixtureCase.operations);
      const result = evaluate(fixture);
      assert.equal(result.status, "fail");
      assert.ok(
        result.failures.some((failure) => failure.class === fixtureCase.expected_class),
        JSON.stringify(result.failures, null, 2),
      );
    });
  }
});

test("graph rejects ambiguous dependencies and unstructured proof lanes", () => {
  const dependency = structuredClone(graph);
  dependency.claims.find(({ id }) => id === "source_behavior").depends_on_claims = ["source_behavior"];
  assert.throws(() => validateReleaseClaimGraph(dependency), /cannot depend on itself/u);

  const lane = structuredClone(graph);
  lane.evidence_types[0].proof_lanes = ".github/workflows/source-proof.yml";
  assert.throws(() => validateReleaseClaimGraph(lane), /proof_lanes must be a non-empty array/u);

  const missingFormat = structuredClone(graph);
  delete missingFormat.evidence_policy.identity_formats.baseline_sha256;
  assert.throws(() => validateReleaseClaimGraph(missingFormat), /must declare a format/u);

  const malformedConstraint = structuredClone(graph);
  malformedConstraint.evidence_types.find(({ id }) => id === "answer_quality")
    .identity_constraints.evaluation_contract = "unversioned";
  assert.throws(() => validateReleaseClaimGraph(malformedConstraint), /does not match versioned_contract/u);
});

test("evaluation requires exact repository and source-tree identity", () => {
  const fixture = positiveFixture();
  delete fixture.expected_identity.source_tree;
  assert.throws(() => evaluate(fixture), /expected.identity.source_tree/u);
});

test("performance and quality identities are bound to trusted candidate and graph inputs", () => {
  const fixture = releaseEvidenceFixture();
  assert.equal(evaluate(fixture).status, "pass");

  const baseline = structuredClone(fixture);
  baseline.evidence.find(({ type }) => type === "performance").identity.baseline_id = "fabricated@baseline";
  assert.ok(evaluate(baseline).failures.some(({ class: failureClass, evidence: id }) =>
    failureClass === "incompatible_tier_identity" && id.startsWith("performance-")));

  const quality = structuredClone(fixture);
  quality.evidence.find(({ type }) => type === "answer_quality").identity.evaluation_contract = "fabricated/v9";
  assert.ok(evaluate(quality).failures.some(({ class: failureClass, evidence: id }) =>
    failureClass === "incompatible_tier_identity" && id.startsWith("answer_quality-")));
});

test("risk-bearing dependencies require their own explicit request and risk acceptance", () => {
  const fixture = releaseEvidenceFixture();
  fixture.requested_claims = fixture.requested_claims.filter(({ id }) => id !== "retrieval_readiness");
  const result = evaluate(fixture);
  assert.ok(result.failures.some(({ class: failureClass, message }) =>
    failureClass === "accepted_risk" && /risk-bearing dependency retrieval_readiness must be explicitly requested/u.test(message)));

  fixture.requested_claims.unshift({
    id: "retrieval_readiness",
    accepted_risks: ["measured-corpus-is-bounded"],
  });
  assert.equal(evaluate(fixture).status, "pass");
});

test("only bounded, release-bound model microbenchmark exceptions remain visible", () => {
  const fixture = releaseEvidenceFixture();
  const performance = fixture.evidence.find(({ type }) => type === "performance");
  const answerQuality = fixture.evidence.find(({ type }) => type === "answer_quality");
  const approvedAt = fixture.evaluated_at.slice(0, 10);
  const expiresAt = new Date(`${approvedAt}T00:00:00.000Z`);
  expiresAt.setUTCDate(expiresAt.getUTCDate() + 14);
  const approval = {
    candidate_sha256: fixture.expected_identity.candidate_sha256,
    commit: fixture.expected_sha,
    profile: fixture.expected_identity.profile,
    baseline_id: fixture.expected_identity.baseline_id,
    baseline_sha256: fixture.expected_identity.baseline_sha256,
    metric: "model_bulk_docs_per_second",
    regression_class: "model_microbenchmark",
    baseline_value: 100,
    measured_value: 90,
    threshold: 95,
    regression_percent: 10,
    direction: "min",
    repeats: 3,
    release_key: fixture.expected_identity.release_key,
    owner: "release owner",
    approved_at: approvedAt,
    expires_at: expiresAt.toISOString().slice(0, 10),
    rationale: "Bound exception",
    rollback_evidence: "revert candidate and restore the accepted baseline",
    full_product_benefit: {
      evidence_id: answerQuality.id,
      artifact_sha256: answerQuality.identity.artifact_sha256,
      observed_at: answerQuality.observed_at,
      metric: "packet_quality_score",
      baseline_value: 0.5,
      measured_value: 0.6,
      direction: "increase",
      improvement_percent: 20,
    },
  };
  performance.status = "pass_with_exception";
  performance.exception = {
    schema: "codestory.release-claim-exception/v1",
    approvals: [approval],
  };
  fixture.expected_exceptions = { [performance.id]: structuredClone(performance.exception) };
  const evaluation = evaluateReleaseClaims({
    graph,
    requested_claims: fixture.requested_claims,
    evidence: fixture.evidence,
    expected: {
      commit: fixture.expected_sha,
      evaluated_at: fixture.evaluated_at,
      identity: fixture.expected_identity,
      exceptions: fixture.expected_exceptions,
    },
  });
  assert.equal(evaluation.status, "pass_with_exception");
  const performanceClaim = evaluation.claims.find(({ id }) => id === "performance");
  assert.equal(performanceClaim.status, "pass_with_exception");
  assert.equal(performanceClaim.exceptions[0].approvals[0].owner, "release owner");
  assert.equal(
    performanceClaim.exceptions[0].approvals[0].rollback_evidence,
    "revert candidate and restore the accepted baseline",
  );

  const rejection = (mutate) => {
    const changed = structuredClone(approval);
    mutate(changed);
    performance.exception.approvals = [changed];
    fixture.expected_exceptions[performance.id] = structuredClone(performance.exception);
    return evaluateReleaseClaims({
      graph,
      requested_claims: fixture.requested_claims,
      evidence: fixture.evidence,
      expected: {
        commit: fixture.expected_sha,
        evaluated_at: fixture.evaluated_at,
        identity: fixture.expected_identity,
        exceptions: fixture.expected_exceptions,
      },
    });
  };

  assert.match(
    rejection((changed) => { changed.metric = "status_seconds"; })
      .failures.map(({ message }) => message).join("\n"),
    /status_seconds is non-waivable/u,
  );
  assert.match(
    rejection((changed) => {
      changed.measured_value = 95;
      changed.threshold = 97;
      changed.regression_percent = 5;
    }).failures.map(({ message }) => message).join("\n"),
    /regression over 5 percent/u,
  );
  assert.match(
    rejection((changed) => { changed.repeats = 2; })
      .failures.map(({ message }) => message).join("\n"),
    /repeats must be at least 3/u,
  );
  assert.match(
    rejection((changed) => {
      const tooLate = new Date(`${approvedAt}T00:00:00.000Z`);
      tooLate.setUTCDate(tooLate.getUTCDate() + 15);
      changed.expires_at = tooLate.toISOString().slice(0, 10);
    }).failures.map(({ message }) => message).join("\n"),
    /expires more than 14 days/u,
  );
  assert.match(
    rejection((changed) => { changed.release_key = "next-release"; })
      .failures.map(({ message }) => message).join("\n"),
    /release_key does not match/u,
  );
  assert.match(
    rejection((changed) => { changed.candidate_sha256 = "c".repeat(64); })
      .failures.map(({ message }) => message).join("\n"),
    /candidate_sha256 does not match/u,
  );
  assert.match(
    rejection((changed) => { changed.full_product_benefit.observed_at = "2026-01-01T00:00:00.000Z"; })
      .failures.map(({ message }) => message).join("\n"),
    /not from the same run/u,
  );
  assert.match(
    rejection((changed) => { changed.full_product_benefit.artifact_sha256 = "c".repeat(64); })
      .failures.map(({ message }) => message).join("\n"),
    /artifact does not match its evidence row/u,
  );
  assert.match(
    rejection((changed) => { delete changed.rollback_evidence; })
      .failures.map(({ message }) => message).join("\n"),
    /rollback_evidence/u,
  );
});

test("CLI derives repository and tree identity from repo and rejects nonexistent commits", () => {
  const identity = deriveTrustedGitIdentity({
    repoRoot: root,
    expectedSha: spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).stdout.trim(),
  });
  const fixture = positiveFixture();
  fixture.expected_identity = { repository: "forged/document", source_tree: "0".repeat(40) };
  fixture.expected_sha = identity.commit;
  fixture.evidence[0].identity = { ...identity };
  fixture.evidence[0].graph_sha256 = releaseClaimGraphDigest(graph);
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-release-claims-"));
  const evidencePath = path.join(directory, "evidence.json");
  writeFileSync(evidencePath, JSON.stringify(fixture));
  const script = path.join(root, "scripts/codestory-release-claims.mjs");
  const valid = spawnSync(process.execPath, [
    script,
    "evaluate",
    "--repo",
    root,
    "--evidence",
    evidencePath,
    "--expected-sha",
    identity.commit,
    "--evaluated-at",
    fixture.evaluated_at,
  ], { encoding: "utf8" });
  assert.equal(valid.status, 0, valid.stderr);

  const nonexistent = spawnSync(process.execPath, [
    script,
    "evaluate",
    "--repo",
    root,
    "--evidence",
    evidencePath,
    "--expected-sha",
    "f".repeat(40),
    "--evaluated-at",
    fixture.evaluated_at,
  ], { encoding: "utf8" });
  assert.notEqual(nonexistent.status, 0);
  assert.match(nonexistent.stderr, /git cat-file -e/u);
});
