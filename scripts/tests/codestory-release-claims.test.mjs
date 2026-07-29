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
  releaseAssetNames,
  releaseClaimGraphDigest,
  renderPublicSupport,
  renderReleasePlatformNotes,
  validatePublicSupportDocuments,
  validateReleaseClaimGraph,
  verifyReuseBinding,
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
  assert.equal(graph.graph_version, 8);
  assert.deepEqual(
    [...graph.standard_release_claims].sort(),
    [
      "accelerator_execution",
      "installed_runtime_behavior",
      "package_identity",
      "platform_support",
      "retrieval_readiness",
      "source_behavior",
    ],
  );
  assert.deepEqual(
    [...graph.optional_evaluations].sort(),
    ["answer_quality", "performance"],
  );
  assert.ok(!graph.workflow_policy.release_chain.exact_sha_jobs.includes("release-evidence"));
  assert.ok(Object.values(graph.workflow_policy.release_chain.dependencies)
    .every((needs) => !needs.includes("release-evidence")));
  assert.deepEqual(graph.closeout.phases, ["pre_publish", "post_publish"]);
  assert.equal(graph.closeout.cell_groups.length, 8);
  assert.deepEqual(
    graph.workflow_policy.package_matrix.map(({ asset_target: target }) => target).sort(),
    ["linux-x64", "macos-arm64", "windows-x64"],
  );
  assert.equal(graph.evidence_policy.identity_formats.calibration_sha256, undefined);
  for (const cellId of [
    "accelerator_execution",
    "candidate_installed_behavior",
    "installed_runtime_behavior",
  ]) {
    assert.ok(
      !graph.closeout.cell_groups
        .find(({ id }) => id === cellId)
        .required_identity.includes("calibration_sha256"),
      `${cellId} must not claim external calibration evidence`,
    );
  }
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
  assert.deepEqual(graph.workflow_policy.promotion.required_events, ["labeled"]);
  assert.equal(graph.workflow_policy.promotion.proof_run_sha_expression, "${{ github.sha }}");
  assert.equal(graph.workflow_policy.promotion.manual_pr_ref_hint, "--ref <same-repository PR head branch>");
  assert.equal(graph.workflow_policy.promotion.source_cache_namespace, "source-proof-v2");
  assert.equal(graph.workflow_policy.promotion.packaged_cache_namespace, "codestory-cli-native-v4");
});

test("benchmark leakage names only the one-process Node contract", () => {
  const benchmarkLeakage = graph.failure_controls
    .find(({ id }) => id === "benchmark_leakage");
  assert.equal(
    benchmarkLeakage.command,
    "node --test scripts/tests/lint-retrieval-generalization.test.mjs",
  );

  for (const command of [
    "cargo test --locked -p codestory-runtime --test retrieval_generalization_guard",
    "node scripts/lint-retrieval-generalization.mjs",
  ]) {
    const mutated = structuredClone(graph);
    mutated.failure_controls
      .find(({ id }) => id === "benchmark_leakage")
      .command = command;
    assert.throws(
      () => validateReleaseClaimGraph(mutated),
      /failure control benchmark_leakage must be exactly node --test scripts\/tests\/lint-retrieval-generalization\.test\.mjs/u,
    );
  }
});

test("public support, assets, and release notes derive from the package and closeout graph", () => {
  assert.doesNotThrow(() => validatePublicSupportDocuments(graph, root));
  assert.deepEqual(
    graph.public_support.packages.map(({ target, accelerator_claim: accelerator }) =>
      ({ target, accelerator })),
    [
      { target: "macos-arm64", accelerator: "metal" },
      { target: "windows-x64", accelerator: "vulkan" },
      { target: "linux-x64", accelerator: "vulkan" },
    ],
  );
  assert.deepEqual(
    releaseAssetNames(graph, "0.16.0"),
    [
      "codestory-cli-v0.16.0-windows-x64.zip",
      "codestory-cli-v0.16.0-macos-arm64.tar.gz",
      "codestory-cli-v0.16.0-linux-x64.tar.gz",
      "SHA256SUMS.txt",
      // The README tells a reader to consult the ledger rather than the platform table, so the
      // machine-readable closeout summary has to ship with the release itself.
      "release-closeout-summary.json",
    ],
  );
  assert.match(renderPublicSupport(graph), /Apple Silicon \| Supported with Metal/u);
  assert.match(renderPublicSupport(graph), /Windows x64 \| Supported with Vulkan/u);
  assert.match(renderPublicSupport(graph), /Linux x64 \| Supported with Vulkan/u);
  assert.match(renderPublicSupport(graph), /CPU-only Windows and Linux \| Unsupported/u);

  // The release notes are a claim about one release, so they are rendered from that release's
  // ledger. The graph alone can no longer produce them.
  const proven = renderReleasePlatformNotes(graph, { withheld_cells: [] });
  assert.match(proven, /macOS 15\+ on Apple Silicon: supported with Metal/u);
  assert.match(proven, /Windows x64: supported with Vulkan/u);
  assert.match(proven, /Linux x64: supported with Vulkan/u);
  assert.throws(() => renderReleasePlatformNotes(graph), /closeout ledger/u);
  assert.throws(() => renderReleasePlatformNotes(graph, {}), /withheld_cells/u);

  const withheld = renderReleasePlatformNotes(graph, {
    withheld_cells: [
      "accelerator_execution:linux-x64-vulkan",
      "candidate_installed_behavior:linux-x64",
    ],
  });
  assert.match(
    withheld,
    /Linux x64: Vulkan not proven for this release \(accelerator_host_unavailable\)/u,
  );
  assert.equal(/Linux x64: supported with Vulkan/u.test(withheld), false);
  assert.match(withheld, /Windows x64: supported with Vulkan/u);
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

  const missingCloseoutCell = structuredClone(graph);
  missingCloseoutCell.closeout.cell_groups = missingCloseoutCell.closeout.cell_groups
    .filter(({ id }) => id !== "post_publish_bytes");
  assert.throws(
    () => validateReleaseClaimGraph(missingCloseoutCell),
    /closeout must define exactly/u,
  );

  const aggregateCell = structuredClone(graph);
  aggregateCell.closeout.cell_groups.find(({ id }) => id === "package_identity")
    .required_identity.push("undeclared_identity");
  assert.throws(
    () => validateReleaseClaimGraph(aggregateCell),
    /identity undeclared_identity must declare a format/u,
  );

  // A non-claim that withholds less than the lost host actually produced would leave a live claim
  // resting on a proof that never ran, so the withheld set is checked against the graph itself.
  const partialNonClaim = structuredClone(graph);
  partialNonClaim.non_claim_policy.hosts.find(({ id }) => id === "linux-x64-vulkan")
    .withheld_cells = ["accelerator_execution:linux-x64-vulkan"];
  assert.throws(
    () => validateReleaseClaimGraph(partialNonClaim),
    /must withhold exactly the cells Packaged Linux Vulkan engine produces/u,
  );

  const unboundedRecovery = structuredClone(graph);
  unboundedRecovery.non_claim_policy.maximum_run_attempts = 12;
  assert.throws(
    () => validateReleaseClaimGraph(unboundedRecovery),
    /maximum_run_attempts must be 2/u,
  );

  const softenedNonClaim = structuredClone(graph);
  softenedNonClaim.non_claim_policy.runtime_execution = "assumed_from_prior_release";
  assert.throws(
    () => validateReleaseClaimGraph(softenedNonClaim),
    /runtime_execution must be not_proven_by_package/u,
  );

  // The withhold cap is data, and the graph refuses a cap that could leave nothing proven. A cap
  // equal to the number of protected hosts makes "no accelerator was proven anywhere" a legal
  // release, which is the whole thing the cap exists to make unrepresentable.
  const uncappedWithholding = structuredClone(graph);
  uncappedWithholding.non_claim_policy.withhold_policy.maximum_withheld_hosts =
    uncappedWithholding.non_claim_policy.hosts.length;
  assert.throws(
    () => validateReleaseClaimGraph(uncappedWithholding),
    /must leave at least one protected host proven/u,
  );

  const noCap = structuredClone(graph);
  delete noCap.non_claim_policy.withhold_policy;
  assert.throws(() => validateReleaseClaimGraph(noCap), /withhold_policy must be an object/u);

  const zeroCap = structuredClone(graph);
  zeroCap.non_claim_policy.withhold_policy.maximum_withheld_hosts = 0;
  assert.throws(
    () => validateReleaseClaimGraph(zeroCap),
    /maximum_withheld_hosts must be a positive integer/u,
  );

  // Dropping a claim a lost host can erase would make the cap silent about exactly that claim.
  const unguardedClaim = structuredClone(graph);
  unguardedClaim.non_claim_policy.withhold_policy.claims_requiring_proof =
    unguardedClaim.non_claim_policy.withhold_policy.claims_requiring_proof
      .filter((claimId) => claimId !== "accelerator_execution");
  assert.throws(
    () => validateReleaseClaimGraph(unguardedClaim),
    /claims_requiring_proof must include accelerator_execution/u,
  );

  const unmatchedHosts = structuredClone(graph);
  unmatchedHosts.non_claim_policy.hosts = unmatchedHosts.non_claim_policy.hosts
    .filter(({ id }) => id !== "linux-x64-vulkan");
  assert.throws(
    () => validateReleaseClaimGraph(unmatchedHosts),
    /must name exactly the protected accelerator instances/u,
  );

  const mismatchedSupport = structuredClone(graph);
  mismatchedSupport.public_support.packages[0].target = "macos-x64";
  assert.throws(
    () => validateReleaseClaimGraph(mismatchedSupport),
    /package targets must exactly match/u,
  );

  const incompleteUnsupportedMatrix = structuredClone(graph);
  incompleteUnsupportedMatrix.public_support.unsupported.pop();
  assert.throws(
    () => validateReleaseClaimGraph(incompleteUnsupportedMatrix),
    /canonical unsupported release matrix/u,
  );

  const cpuOnlyLinuxSupported = structuredClone(graph);
  cpuOnlyLinuxSupported.public_support.unsupported[0].targets = ["windows-x64"];
  assert.throws(
    () => validateReleaseClaimGraph(cpuOnlyLinuxSupported),
    /canonical unsupported release matrix/u,
  );

  const missingAcceleratorCell = structuredClone(graph);
  missingAcceleratorCell.closeout.cell_groups
    .find(({ id }) => id === "accelerator_execution")
    .instances = [];
  assert.throws(
    () => validateReleaseClaimGraph(missingAcceleratorCell),
    /must be a non-empty array|no required closeout cell/u,
  );

  for (const field of [
    "proof_run_sha_expression",
    "manual_pr_ref_hint",
    "source_cache_namespace",
    "packaged_cache_namespace",
  ]) {
    const incompletePromotion = structuredClone(graph);
    delete incompletePromotion.workflow_policy.promotion[field];
    assert.throws(
      () => validateReleaseClaimGraph(incompletePromotion),
      new RegExp(`workflow_policy\\.promotion\\.${field}`, "u"),
    );
  }
});

// Catalog publication is delivery rather than a release gate, which is only honest if the run
// records which of the two states it ended in. The graph is where that vocabulary lives, so
// deleting it, reinstating the gate, or collapsing the two installer identities onto one -- which
// is exactly how a deferred run would come to read as a published one -- must be refusals here,
// not merely in the workflow policy that consumes them.
test("catalog delivery declares two distinguishable states and no release gate", () => {
  const delivery = graph.workflow_policy.catalog_delivery;
  assert.equal(delivery.release_gate, false);
  assert.deepEqual(delivery.states.map(({ id }) => id).sort(), ["deferred", "published"]);
  assert.equal(new Set(delivery.states.map(({ installer }) => installer)).size, 2);

  const missing = structuredClone(graph);
  delete missing.workflow_policy.catalog_delivery;
  assert.throws(
    () => validateReleaseClaimGraph(missing),
    /workflow_policy\.catalog_delivery must be an object/u,
  );

  const gated = structuredClone(graph);
  gated.workflow_policy.catalog_delivery.release_gate = true;
  assert.throws(
    () => validateReleaseClaimGraph(gated),
    /release_gate must be false: catalog publication is delivery, not a release gate/u,
  );

  const collapsed = structuredClone(graph);
  const [first, second] = collapsed.workflow_policy.catalog_delivery.states;
  second.installer = first.installer;
  assert.throws(
    () => validateReleaseClaimGraph(collapsed),
    /must record distinct installer identities/u,
  );

  const renamed = structuredClone(graph);
  renamed.workflow_policy.catalog_delivery.states
    .find(({ id }) => id === "deferred").id = "unknown";
  assert.throws(
    () => validateReleaseClaimGraph(renamed),
    /must declare the deferred state/u,
  );

  const inverted = structuredClone(graph);
  inverted.workflow_policy.catalog_delivery.states
    .find(({ id }) => id === "deferred").live_catalog_revision = true;
  assert.throws(
    () => validateReleaseClaimGraph(inverted),
    /deferred state must not consume a live catalog revision/u,
  );

  const unpublished = structuredClone(graph);
  unpublished.workflow_policy.catalog_delivery.states
    .find(({ id }) => id === "published").live_catalog_revision = false;
  assert.throws(
    () => validateReleaseClaimGraph(unpublished),
    /published state must consume the live catalog revision/u,
  );

  const detached = structuredClone(graph);
  detached.workflow_policy.catalog_delivery.publish_job = "no-such-job";
  assert.throws(
    () => validateReleaseClaimGraph(detached),
    /publish_job no-such-job must be a release chain job/u,
  );

  const unrecoverable = structuredClone(graph);
  delete unrecoverable.workflow_policy.catalog_delivery.recovery_workflow;
  assert.throws(
    () => validateReleaseClaimGraph(unrecoverable),
    /workflow_policy\.catalog_delivery\.recovery_workflow/u,
  );
});

// Naming the two delivery states buys nothing on its own: the first version of this graph
// declared both and nothing anywhere read the mark, so a deferred release's closeout verdict was
// identical in shape to a published one. The graph must therefore also name the cell family whose
// signed installer identity the closeout resolves the state from -- and that family has to be a
// post-publish one that actually carries `installer`, or the reader would have nothing to read.
test("catalog delivery names the cells whose installer identity resolves the state", () => {
  const delivery = graph.workflow_policy.catalog_delivery;
  const group = graph.closeout.cell_groups
    .find(({ id }) => id === delivery.installed_cell_group);
  assert.equal(group.phase, "post_publish");
  assert.ok(group.required_identity.includes("installer"));
  assert.ok(group.singleton_identity.includes("installer"));

  const unread = structuredClone(graph);
  delete unread.workflow_policy.catalog_delivery.installed_cell_group;
  assert.throws(
    () => validateReleaseClaimGraph(unread),
    /workflow_policy\.catalog_delivery\.installed_cell_group/u,
  );

  const unknown = structuredClone(graph);
  unknown.workflow_policy.catalog_delivery.installed_cell_group = "no-such-group";
  assert.throws(
    () => validateReleaseClaimGraph(unknown),
    /must be a closeout cell group/u,
  );

  // A pre-publish family cannot carry the delivery state: it is produced before the catalog is
  // ever touched, so reading it would report a state nothing had decided yet.
  const early = structuredClone(graph);
  early.workflow_policy.catalog_delivery.installed_cell_group = "candidate_installed_behavior";
  assert.throws(
    () => validateReleaseClaimGraph(early),
    /must be a post-publish cell group/u,
  );

  // The installer must be a singleton identity, or the three targets could each report a
  // different catalog and no single state would exist to record.
  const nonSingleton = structuredClone(graph);
  nonSingleton.closeout.cell_groups
    .find(({ id }) => id === delivery.installed_cell_group)
    .singleton_identity = ["host_os", "host_arch", "native_engine"];
  assert.throws(
    () => validateReleaseClaimGraph(nonSingleton),
    /must carry installer in singleton_identity/u,
  );
});

// check-workflow-policy.mjs asserts only that plugin-release.yml's `needs:` match this data, so a
// chain that parses but orders nothing would let both gates pass while `gh release create` ran
// detached from the release-authority checks and the plugin-proof matrix. Every mutation below
// leaves the workflow and the graph agreeing with each other; only the schema can refuse them.
test("the plugin chain must order the lane, not merely name it", async (t) => {
  const chain = (graphValue) => graphValue.workflow_policy.plugin_chain.dependencies;
  const mutations = [
    ["the ordering contract is dropped wholesale", (mutated) => {
      delete mutated.workflow_policy.plugin_chain;
    }, /workflow_policy\.plugin_chain must be an object/u],
    ["the dependencies key is not a mapping", (mutated) => {
      mutated.workflow_policy.plugin_chain.dependencies = [];
    }, /workflow_policy\.plugin_chain\.dependencies must be an object/u],
    ["the lane declares no jobs at all", (mutated) => {
      mutated.workflow_policy.plugin_chain.dependencies = {};
    }, /plugin_chain\.dependencies must declare at least one job/u],
    ["tagging is cut loose from every gate", (mutated) => {
      chain(mutated).publish = [];
    }, /plugin_chain\.dependencies\.publish must be a non-empty array/u],
    ["the install proof is cut loose from every gate", (mutated) => {
      chain(mutated)["post-publish-smoke"] = [];
    }, /plugin_chain\.dependencies\.post-publish-smoke must be a non-empty array/u],
    ["tagging stops waiting on the plugin proof", (mutated) => {
      chain(mutated).publish = ["preflight"];
    }, /plugin_chain\.dependencies\.publish must run behind plugin-proof/u],
    ["the plugin proof is deleted from the lane", (mutated) => {
      delete chain(mutated)["plugin-proof"];
      chain(mutated).publish = ["preflight"];
    }, /plugin_chain\.dependencies must declare publish and plugin-proof/u],
    ["catalog publication races the release it advertises", (mutated) => {
      chain(mutated)["marketplace-publish"] = ["preflight"];
    }, /plugin_chain\.dependencies\.marketplace-publish must run behind publish/u],
    ["the install proof stops waiting on catalog publication", (mutated) => {
      chain(mutated)["post-publish-smoke"] = ["preflight", "publish"];
    }, /plugin_chain\.dependencies\.post-publish-smoke must run behind marketplace-publish/u],
    ["a dependency names a job the lane never declares", (mutated) => {
      chain(mutated).publish = ["preflight", "plugin-proof", "imaginary-gate"];
    }, /plugin_chain\.dependencies\.publish names undeclared job imaginary-gate/u],
    ["the lane closes into a cycle no job can enter", (mutated) => {
      chain(mutated).preflight = ["plugin-proof"];
    }, /plugin_chain\.dependencies\.(?:preflight|plugin-proof) cannot depend on itself/u],
  ];
  for (const [name, mutate, expected] of mutations) {
    await t.test(name, () => {
      const mutated = structuredClone(graph);
      mutate(mutated);
      assert.throws(() => validateReleaseClaimGraph(mutated), expected);
    });
  }
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

test("optional evaluations do not inherit standard release dependencies", () => {
  const fixture = releaseEvidenceFixture();
  assert.equal(evaluate(fixture).status, "pass");
  for (const id of graph.optional_evaluations) {
    assert.deepEqual(graph.claims.find((claim) => claim.id === id).depends_on_claims, []);
  }
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

test("reuse bindings verify tree identity and fingerprint equality against real history", () => {
  // Both sides of the ledger prove reuse with this one function -- the producer before it admits
  // cross-run evidence, the closeout before it anchors a row onto the earlier run -- so it is
  // proved here, against real history, in the suite pull requests actually run.
  //
  // v0.16.0 -> v0.16.1 is a pure version bump in this repository's real history: different
  // trees (so source_tree reuse must refuse) but identical native fingerprints (so
  // accelerator inheritance is exactly what version_only_delta authorizes).
  const releaseTag = "00121349"; // v0.16.1 release commit
  const priorTag = "29bd4795"; // v0.16.0 release commit
  assert.throws(
    () => verifyReuseBinding({
      binding: "source_tree",
      repository: root,
      releaseCommit: releaseTag,
      reusedCommit: priorTag,
    }),
    /does not match release tree/u,
  );
  const fingerprint = verifyReuseBinding({
    binding: "native_fingerprint",
    repository: root,
    releaseCommit: releaseTag,
    reusedCommit: priorTag,
  });
  assert.match(fingerprint, /^[0-9a-f]{64}$/u);
  // Identical commits always satisfy the tree binding.
  const tree = verifyReuseBinding({
    binding: "source_tree",
    repository: root,
    releaseCommit: releaseTag,
    reusedCommit: releaseTag,
  });
  assert.match(tree, /^[0-9a-f]{40}$/u);
  // A binding name the claim graph never declared proves nothing.
  assert.throws(
    () => verifyReuseBinding({
      binding: "source_history",
      repository: root,
      releaseCommit: releaseTag,
      reusedCommit: priorTag,
    }),
    /unknown reuse binding source_history/u,
  );
  // Ancestry belongs to reuse itself, not to any one binding. Read the other way round, v0.16.1
  // is not on v0.16.0's history, and the fingerprints are equal -- content equality alone would
  // admit a run from a fork or an abandoned branch as this release's own proof.
  assert.throws(
    () => verifyReuseBinding({
      binding: "native_fingerprint",
      repository: root,
      releaseCommit: priorTag,
      reusedCommit: releaseTag,
    }),
    /is not an ancestor of the release commit/u,
  );
});

test("a reuse binding may equate only identities its own construction determines", () => {
  // What a reused row is allowed to differ from this release in is declared per binding, in the
  // graph, with a reason -- never per cell group, and never by narrowing a group's
  // required_identity, which would drop the check for fresh evidence too (#1567).
  const declared = graph.evidence_policy.reuse.bindings;
  assert.deepEqual(Object.keys(declared).sort(), ["native_fingerprint", "source_tree"]);
  assert.deepEqual(declared.source_tree.equates, []);
  assert.deepEqual(declared.native_fingerprint.equates.map(({ identity }) => identity), ["source_tree"]);
  assert.ok(declared.native_fingerprint.equates[0].justification.length > 0);

  // The fingerprint determines the built native binary. It says nothing about which repository
  // produced the row, so it may not equate that -- graph text alone cannot grant an equation.
  const foreignRepository = structuredClone(graph);
  foreignRepository.evidence_policy.reuse.bindings.native_fingerprint.equates = [
    { identity: "repository", justification: "same organisation, surely" },
  ];
  assert.throws(
    () => validateReleaseClaimGraph(foreignRepository),
    /native_fingerprint may not equate identity repository, which its construction does not determine/u,
  );

  // The tree binding proves the reused commit resolves to this release's own tree, so there is
  // nothing to substitute: equating the tree there would replace a live check with nothing.
  const vacuousEquation = structuredClone(graph);
  vacuousEquation.evidence_policy.reuse.bindings.source_tree.equates = [
    { identity: "source_tree", justification: "the trees are equal anyway" },
  ];
  assert.throws(
    () => validateReleaseClaimGraph(vacuousEquation),
    /source_tree may not equate identity source_tree, which its construction does not determine/u,
  );

  // An identity outside the release identity binding has no authoritative release-side value the
  // closeout could put in its place, so it can never be equated whatever a binding proves.
  const unboundIdentity = structuredClone(graph);
  unboundIdentity.evidence_policy.reuse.bindings.native_fingerprint.equates = [
    { identity: "artifact_sha256", justification: "the inputs were identical" },
  ];
  assert.throws(
    () => validateReleaseClaimGraph(unboundIdentity),
    /may not equate identity artifact_sha256 outside the release identity binding/u,
  );

  // An equation nobody can justify in a sentence is one nobody should be granting.
  const unjustified = structuredClone(graph);
  delete unjustified.evidence_policy.reuse.bindings.native_fingerprint.equates[0].justification;
  assert.throws(
    () => validateReleaseClaimGraph(unjustified),
    /equated identity source_tree.justification must be a non-empty string/u,
  );

  // Every binding the verifier implements has to say what it equates, so a new binding cannot
  // arrive with its equations left unstated.
  const undeclaredBinding = structuredClone(graph);
  delete undeclaredBinding.evidence_policy.reuse.bindings.native_fingerprint;
  assert.throws(
    () => validateReleaseClaimGraph(undeclaredBinding),
    /reuse bindings must declare exactly native_fingerprint, source_tree/u,
  );

  // And a cell group admits cross-run evidence only under a binding the policy declares.
  const inventedBinding = structuredClone(graph);
  inventedBinding.closeout.cell_groups.find(({ id }) => id === "accelerator_execution")
    .reuse_binding = "source_history";
  assert.throws(
    () => validateReleaseClaimGraph(inventedBinding),
    /accelerator_execution names undeclared reuse binding source_history/u,
  );
});
