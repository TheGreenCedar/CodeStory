import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  evaluateReleaseClaims,
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
});

test("evaluation requires exact repository and source-tree identity", () => {
  const fixture = positiveFixture();
  delete fixture.expected_identity.source_tree;
  assert.throws(() => evaluate(fixture), /expected.identity.source_tree/u);
});
