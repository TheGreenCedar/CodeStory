#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const GRAPH_SCHEMA = "codestory.release-claims/v1";
const GRAPH_VERSION = 11;
const KNOWN_PACKAGE_TARGETS = new Set([
  "linux-arm64",
  "linux-x64",
  "macos-arm64",
  "macos-x64",
  "windows-arm64",
  "windows-x64",
]);
const UNSUPPORTED_RELEASE_CONFIGURATIONS = [
  {
    label: "CPU-only Windows and Linux",
    targets: ["windows-x64", "linux-x64"],
    condition: "cpu_only",
  },
  {
    label: "Intel Mac",
    targets: ["macos-x64"],
    condition: "all",
  },
  {
    label: "Windows ARM",
    targets: ["windows-arm64"],
    condition: "all",
  },
];
const FULL_SHA = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/u;
const IDENTITY_FORMATS = new Set([
  "baseline_id",
  "git_sha",
  "github_repository",
  "identifier",
  "non_empty_text",
  "release_target",
  "semver",
  "sha256",
  "versioned_contract",
  "workflow_path",
  "positive_integer",
]);
const REQUIRED_CLAIMS = [
  "accelerator_execution",
  "answer_quality",
  "installed_runtime_behavior",
  "package_identity",
  "performance",
  "platform_support",
  "retrieval_readiness",
  "source_behavior",
];
const STANDARD_RELEASE_CLAIMS = [
  "accelerator_execution",
  "installed_runtime_behavior",
  "package_identity",
  "platform_support",
  "retrieval_readiness",
  "source_behavior",
];
const OPTIONAL_EVALUATIONS = [
  "answer_quality",
  "performance",
];
const REQUIRED_FAILURE_CONTROLS = [
  "benchmark_leakage",
  "observational_read_mutation",
  "project_identity_drift",
  "sidecar_runtime_mismatch",
  "stale_or_partial_publication",
];
const BENCHMARK_LEAKAGE_COMMAND =
  "node --test scripts/tests/lint-retrieval-generalization.test.mjs";
const FAILURE_ORDER = new Map([
  ["unsupported_claim", 0],
  ["missing", 1],
  ["stale_sha", 2],
  ["stale_evidence", 3],
  ["incompatible_tier_identity", 4],
  ["failed_evidence", 5],
  ["accepted_risk", 6],
]);

function fail(message) {
  throw new Error(message);
}

function object(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  return value;
}

function nonEmptyText(value, label) {
  if (typeof value !== "string" || value.trim() === "") {
    fail(`${label} must be a non-empty string`);
  }
  return value;
}

function stringArray(value, label, { nonEmpty = false } = {}) {
  if (!Array.isArray(value) || (nonEmpty && value.length === 0)) {
    fail(`${label} must be ${nonEmpty ? "a non-empty" : "an"} array`);
  }
  const values = value.map((item, index) => nonEmptyText(item, `${label}[${index}]`));
  if (new Set(values).size !== values.length) fail(`${label} must not contain duplicates`);
  return values;
}

function validIsoDate(value) {
  if (typeof value !== "string" || !ISO_DATE.test(value)) return false;
  const parsed = new Date(`${value}T00:00:00.000Z`);
  return Number.isFinite(parsed.valueOf()) && parsed.toISOString().slice(0, 10) === value;
}

function identityMatchesFormat(value, format) {
  if (typeof value !== "string" || value.trim() !== value || value === "") return false;
  switch (format) {
    case "git_sha": return FULL_SHA.test(value);
    case "sha256": return SHA256.test(value);
    case "github_repository": return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(value);
    case "semver": return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u.test(value);
    case "identifier":
    case "release_target": return /^[A-Za-z0-9][A-Za-z0-9._:/+-]*$/u.test(value);
    case "baseline_id": return /^[A-Za-z0-9][A-Za-z0-9._:/+@-]*$/u.test(value);
    case "versioned_contract": return /^[A-Za-z0-9][A-Za-z0-9._+-]*(?:\/[A-Za-z0-9._+-]+)*\/v[1-9]\d*$/u.test(value);
    case "workflow_path": return /^\.github\/workflows\/[A-Za-z0-9][A-Za-z0-9._-]*\.ya?ml$/u.test(value);
    case "positive_integer": return /^[1-9]\d*$/u.test(value);
    case "non_empty_text": return value.length > 0;
    default: return false;
  }
}

export { identityMatchesFormat as releaseClaimIdentityMatchesFormat };

function git(args, repoRoot) {
  const result = spawnSync("git", args, { cwd: repoRoot, encoding: "utf8" });
  if (result.status !== 0) {
    const detail = result.stderr.trim() || result.stdout.trim() || `exit ${String(result.status)}`;
    fail(`git ${args.join(" ")} failed: ${detail}`);
  }
  return result.stdout.trim();
}

function githubRepositoryFromRemote(remote) {
  const match = remote.match(/github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?$/u);
  if (!match) fail(`cannot derive GitHub repository identity from origin ${remote}`);
  return match[1];
}

export function deriveTrustedGitIdentity({ repoRoot, expectedSha }) {
  const commit = nonEmptyText(expectedSha, "expectedSha").toLowerCase();
  if (!FULL_SHA.test(commit)) fail("expectedSha must be a full lowercase Git SHA");
  git(["cat-file", "-e", `${commit}^{commit}`], repoRoot);
  const resolvedCommit = git(["rev-parse", `${commit}^{commit}`], repoRoot).toLowerCase();
  if (resolvedCommit !== commit) fail("expectedSha must identify a commit object directly");
  const sourceTree = git(["rev-parse", `${commit}^{tree}`], repoRoot).toLowerCase();
  if (!FULL_SHA.test(sourceTree)) fail(`git returned invalid tree identity for ${commit}`);
  const remote = git(["config", "--get", "remote.origin.url"], repoRoot);
  return {
    repository: githubRepositoryFromRemote(remote),
    commit,
    source_tree: sourceTree,
  };
}

/// What each reuse binding's own construction determines, and may therefore equate.
///
/// Reuse always reads a reused row's *commit* at the release commit -- that is what anchoring to
/// the earlier run means. Equating goes further: it lets a reused row keep an identity whose value
/// differs from this release's, so it may only ever name a key the binding itself determines. The
/// graph declares which of these keys each binding actually uses (`evidence_policy.reuse.bindings`)
/// and why; this map is the ceiling, stated next to the proofs that establish it, so a graph edit
/// alone can never grant an equation no binding proves.
///
///   * `source_tree` proves the reused commit resolves to this release's own tree. Nothing needs
///     substituting: every tree-derived identity a reused row declares is still checkable directly
///     against this release, and equating one would replace a live check with nothing. Hence [].
///   * `native_fingerprint` proves the two commits' native build inputs -- crates/**, Cargo.lock,
///     vendor/**, the packaging scripts, the toolchain pins, version-normalized -- hash equal. That
///     determines the built accelerator, so accelerator execution evidence transfers across the
///     source_tree difference the binding exists to tolerate. It determines nothing about the
///     repository, the packaged bytes, the host, or the version, so none of those may be equated.
const REUSE_BINDING_EQUATABLE_IDENTITY = Object.freeze({
  source_tree: Object.freeze([]),
  native_fingerprint: Object.freeze(["source_tree"]),
});

/// Verify a reuse binding against the local repository and return its recorded value.
///
/// Both sides of the release ledger need this: the producer proves the binding before it admits
/// cross-run evidence, and the closeout re-proves it against its own checkout before it anchors a
/// reused row to the earlier run.
export function verifyReuseBinding({ binding, repository, releaseCommit, reusedCommit }) {
  if (!Object.hasOwn(REUSE_BINDING_EQUATABLE_IDENTITY, binding)) {
    fail(`unknown reuse binding ${binding}`);
  }
  // Ancestry is a property of reuse itself, not of any one binding: evidence may only be inherited
  // forward along this release's own history. Without it, a binding that compares content alone --
  // a fingerprint, say -- would admit a run from a fork or an abandoned branch that happens to
  // share the content, which is a different repository's proof wearing this release's clothes.
  if (spawnSync("git", ["merge-base", "--is-ancestor", reusedCommit, releaseCommit], {
    cwd: repository,
    encoding: "utf8",
  }).status !== 0) {
    fail(`reused commit ${reusedCommit} is not an ancestor of the release commit`);
  }
  if (binding === "source_tree") {
    const releaseTree = git(["rev-parse", `${releaseCommit}^{tree}`], repository);
    const reusedTree = git(["rev-parse", `${reusedCommit}^{tree}`], repository);
    if (releaseTree !== reusedTree) {
      fail(`reused commit ${reusedCommit} tree ${reusedTree} does not match release tree ${releaseTree}`);
    }
    return releaseTree;
  }
  if (binding === "native_fingerprint") {
    const script = fileURLToPath(new URL("./native-fingerprint.mjs", import.meta.url));
    const fingerprint = (ref) => {
      const result = spawnSync(process.execPath, [script, "--ref", ref], {
        cwd: repository,
        encoding: "utf8",
      });
      if (result.status !== 0) fail(`native fingerprint of ${ref} failed: ${result.stderr.trim()}`);
      return result.stdout.trim();
    };
    const releasePrint = fingerprint(releaseCommit);
    const reusedPrint = fingerprint(reusedCommit);
    if (releasePrint !== reusedPrint) {
      fail(
        `native fingerprint of reused commit ${reusedCommit} (${reusedPrint}) does not match `
          + `the release commit (${releasePrint}); accelerator evidence cannot be inherited`,
      );
    }
    return releasePrint;
  }
  // Reachable only if a binding is added to the equation ceiling above without a proof here.
  fail(`reuse binding ${binding} has no verification`);
}

function uniqueById(values, label) {
  if (!Array.isArray(values) || values.length === 0) fail(`${label} must be a non-empty array`);
  const found = new Map();
  for (const [index, value] of values.entries()) {
    const row = object(value, `${label}[${index}]`);
    const id = nonEmptyText(row.id, `${label}[${index}].id`);
    if (found.has(id)) fail(`${label} contains duplicate id ${id}`);
    found.set(id, row);
  }
  return found;
}

/// Every closeout cell one protected job produces, keyed by the leaf Actions job name that produces
/// it. A host that goes missing takes its whole column with it, so the withheld set is derived from
/// the graph instead of listed by hand: adding a cell to a protected job cannot leave a stale
/// non-claim behind that quietly keeps claiming something.
function cellsByProducerJobName(cellGroups) {
  const byJobName = new Map();
  const record = (jobName, cellId) => {
    if (typeof jobName !== "string" || jobName === "") return;
    if (!byJobName.has(jobName)) byJobName.set(jobName, []);
    byJobName.get(jobName).push(cellId);
  };
  for (const [groupId, group] of cellGroups) {
    if (group.expansion === "instances") {
      for (const instance of group.instances) {
        record(
          instance.identity_constraints?.producer_job_name
            ?? group.identity_constraints?.producer_job_name,
          `${groupId}:${instance.id}`,
        );
      }
    }
  }
  return byJobName;
}

/// What each reuse binding is permitted to equate, declared per binding by the graph.
///
/// A reused row is read at the release commit; every other identity it carries is checked as
/// written unless its binding declares that key here. That declaration is the whole authorisation:
/// no cell group gets an exception, and no group narrows its own `required_identity` to make room
/// for one, because narrowing would drop the check for fresh evidence too. So the equation has to
/// survive three separate refusals before the closeout will honour it:
///
///   * The key must be one the binding's construction determines -- `REUSE_BINDING_EQUATABLE_IDENTITY`
///     above is the ceiling, stated beside the proofs, so a graph edit alone cannot invent one.
///   * The key must be part of the release identity binding (minus `commit`, which reuse anchors
///     rather than equates), so the closeout always holds an authoritative release-side value and
///     an authoritative reused-side value to put in its place.
///   * The graph must say, in prose, why that particular key follows from that particular proof.
///     An equation nobody can justify in a sentence is one nobody should be granting.
function validateReuseBindings(evidencePolicy, identityBinding) {
  const reuse = object(evidencePolicy.reuse, "release claim graph.evidence_policy.reuse");
  nonEmptyText(reuse.equation, "release claim graph.evidence_policy.reuse.equation");
  nonEmptyText(reuse.verification, "release claim graph.evidence_policy.reuse.verification");
  const bindings = object(reuse.bindings, "release claim graph.evidence_policy.reuse.bindings");
  const implemented = Object.keys(REUSE_BINDING_EQUATABLE_IDENTITY).sort();
  if (JSON.stringify(Object.keys(bindings).sort()) !== JSON.stringify(implemented)) {
    fail(`release claim graph reuse bindings must declare exactly ${implemented.join(", ")}`);
  }
  const equatable = new Set(identityBinding.filter((key) => key !== "commit"));
  for (const [id, value] of Object.entries(bindings)) {
    const binding = object(value, `release claim graph reuse binding ${id}`);
    nonEmptyText(binding.admits, `release claim graph reuse binding ${id}.admits`);
    if (!Array.isArray(binding.equates)) {
      fail(`release claim graph reuse binding ${id}.equates must be an array`);
    }
    const determines = new Set(REUSE_BINDING_EQUATABLE_IDENTITY[id]);
    const declared = new Set();
    for (const [index, entryValue] of binding.equates.entries()) {
      const entry = object(entryValue, `release claim graph reuse binding ${id}.equates[${index}]`);
      const key = nonEmptyText(entry.identity, `release claim graph reuse binding ${id}.equates[${index}].identity`);
      nonEmptyText(entry.justification, `release claim graph reuse binding ${id} equated identity ${key}.justification`);
      if (declared.has(key)) fail(`release claim graph reuse binding ${id} equates ${key} twice`);
      declared.add(key);
      if (!equatable.has(key)) {
        fail(`release claim graph reuse binding ${id} may not equate identity ${key} outside the release identity binding`);
      }
      if (!determines.has(key)) {
        fail(`release claim graph reuse binding ${id} may not equate identity ${key}, which its construction does not determine`);
      }
    }
  }
}

/// How much of a release may go unproven and still publish. Withholding exists so one dead host
/// cannot cost a release its other nine cells -- it is not a way to publish a release nothing
/// vouched for. The two numbers below are the whole policy and they live in the graph rather than
/// in the closeout, because a reader deciding whether to trust a release reads the graph:
///
///   * `maximum_withheld_hosts` bounds how many of the protected hosts may be silent at once. It
///     must stay strictly below the number of hosts, so "every accelerator was withheld" is
///     unrepresentable rather than merely discouraged.
///   * `claims_requiring_proof` names the claims that must retain at least one *passing* cell in
///     any phase that requires them. A withheld cell records a non-claim, so it can never be the
///     thing that satisfies one of these.
function validateWithholdPolicy(policy, hosts, cellGroups) {
  const withhold = object(
    policy.withhold_policy,
    "release claim graph.non_claim_policy.withhold_policy",
  );
  const maximum = withhold.maximum_withheld_hosts;
  if (!Number.isInteger(maximum) || maximum < 1) {
    fail("non_claim_policy.withhold_policy.maximum_withheld_hosts must be a positive integer");
  }
  if (maximum >= hosts.size) {
    fail(
      "non_claim_policy.withhold_policy.maximum_withheld_hosts must leave at least one protected "
      + `host proven (${hosts.size} hosts are declared)`,
    );
  }
  if (withhold.archive_identity_source !== "candidate_archive_record") {
    fail(
      "non_claim_policy.withhold_policy.archive_identity_source must be candidate_archive_record",
    );
  }
  const required = stringArray(
    withhold.claims_requiring_proof,
    "non_claim_policy.withhold_policy.claims_requiring_proof",
    { nonEmpty: true },
  );
  if (JSON.stringify(required) !== JSON.stringify([...required].sort())) {
    fail("non_claim_policy.withhold_policy.claims_requiring_proof must be sorted");
  }
  const claimOfCellGroup = new Map(
    [...cellGroups].map(([groupId, group]) => [groupId, group.claim]),
  );
  const closeoutClaims = new Set(claimOfCellGroup.values());
  for (const claimId of required) {
    if (!closeoutClaims.has(claimId)) {
      fail(`non_claim_policy.withhold_policy.claims_requiring_proof names unclosed claim ${claimId}`);
    }
  }
  const withheldClaims = new Set();
  for (const host of hosts.values()) {
    for (const cellId of host.withheld_cells ?? []) {
      const claimId = claimOfCellGroup.get(String(cellId).split(":")[0]);
      if (claimId !== undefined) withheldClaims.add(claimId);
    }
  }
  // Every claim a host can withhold has to be one the policy insists stays proven somewhere,
  // otherwise the cap would be silent about exactly the claims withholding can erase.
  for (const claimId of [...withheldClaims].sort()) {
    if (!required.includes(claimId)) {
      fail(
        `non_claim_policy.withhold_policy.claims_requiring_proof must include ${claimId}, `
        + "which a withheld host can erase",
      );
    }
  }
}

function validateNonClaimPolicy(graph, cellGroups) {
  const policy = object(graph.non_claim_policy, "release claim graph.non_claim_policy");
  if (policy.schema !== "codestory.release-non-claim/v1") {
    fail("release claim graph.non_claim_policy.schema must be codestory.release-non-claim/v1");
  }
  // The recorded state mirrors the package manifest's own accelerator non-claim, so a reader who
  // already understands `not_proven_by_package` reads a withheld release cell the same way.
  if (policy.runtime_execution !== "not_proven_by_package") {
    fail("release claim graph.non_claim_policy.runtime_execution must be not_proven_by_package");
  }
  nonEmptyText(policy.reason, "release claim graph.non_claim_policy.reason");
  nonEmptyText(policy.annotation, "release claim graph.non_claim_policy.annotation");
  nonEmptyText(policy.recovery_contract, "release claim graph.non_claim_policy.recovery_contract");
  if (policy.maximum_run_attempts !== 2) {
    fail("release claim graph.non_claim_policy.maximum_run_attempts must be 2");
  }
  for (const key of ["producer_workflow", "producer_job", "producer_job_name"]) {
    nonEmptyText(policy[key], `release claim graph.non_claim_policy.${key}`);
  }
  const producedCells = cellsByProducerJobName(cellGroups);
  const hosts = uniqueById(policy.hosts, "release claim graph.non_claim_policy.hosts");
  validateWithholdPolicy(policy, hosts, cellGroups);
  const artifacts = new Set();
  const accelerator = cellGroups.get("accelerator_execution");
  const acceleratorInstances = (accelerator?.instances ?? []).map(({ id }) => id).sort();
  if (JSON.stringify([...hosts.keys()].sort()) !== JSON.stringify(acceleratorInstances)) {
    fail("non_claim_policy.hosts must name exactly the protected accelerator instances");
  }
  for (const [hostId, host] of hosts) {
    nonEmptyText(host.unavailable_producer_workflow, `non_claim_policy.hosts ${hostId}.unavailable_producer_workflow`);
    const jobName = nonEmptyText(
      host.unavailable_producer_job_name,
      `non_claim_policy.hosts ${hostId}.unavailable_producer_job_name`,
    );
    // One artifact container may only hold cells of a single closeout phase, because the phase's
    // trusted producer map is what authorizes every manifest inside the container it downloads.
    const hostArtifacts = object(
      host.producer_artifacts,
      `non_claim_policy.hosts ${hostId}.producer_artifacts`,
    );
    if (JSON.stringify(Object.keys(hostArtifacts).sort()) !== JSON.stringify(["post_publish", "pre_publish"])) {
      fail(`non_claim_policy.hosts ${hostId}.producer_artifacts must name one artifact per closeout phase`);
    }
    for (const [phase, artifact] of Object.entries(hostArtifacts)) {
      nonEmptyText(artifact, `non_claim_policy.hosts ${hostId}.producer_artifacts.${phase}`);
      if (!artifact.includes("{attempt}")) {
        fail(`non_claim_policy.hosts ${hostId}.producer_artifacts.${phase} must be attempt-qualified`);
      }
      if (artifacts.has(artifact)) fail(`non_claim_policy.hosts duplicates artifact ${artifact}`);
      artifacts.add(artifact);
    }
    const declared = stringArray(
      host.withheld_cells,
      `non_claim_policy.hosts ${hostId}.withheld_cells`,
      { nonEmpty: true },
    );
    const derived = producedCells.get(jobName) ?? [];
    if (JSON.stringify([...declared].sort()) !== JSON.stringify([...derived].sort())) {
      fail(`non_claim_policy host ${hostId} must withhold exactly the cells ${jobName} produces`);
    }
  }
}

// Marketplace catalog publication is delivery, not a release gate: it happens after the tag and
// the GitHub release already exist, so failing the release on it would only convert a recoverable
// delivery gap into an unrecoverable one. The price of that is that the release must say which of
// the two states it is in, so the graph names both and pins a distinct installer identity to each.
// A run that could not publish records the deferred identity in its post-publish cells; nothing in
// the pipeline is allowed to record the published identity without the catalog push succeeding.
function validateCatalogDelivery(policy, dependencies, cellGroups) {
  const delivery = object(policy.catalog_delivery, "workflow_policy.catalog_delivery");
  const publishJob = nonEmptyText(delivery.publish_job, "workflow_policy.catalog_delivery.publish_job");
  if (dependencies[publishJob] === undefined) {
    fail(`workflow_policy.catalog_delivery.publish_job ${publishJob} must be a release chain job`);
  }
  nonEmptyText(delivery.recovery_workflow, "workflow_policy.catalog_delivery.recovery_workflow");
  if (delivery.release_gate !== false) {
    fail("workflow_policy.catalog_delivery.release_gate must be false: catalog publication is delivery, not a release gate");
  }
  if (!Array.isArray(delivery.states) || delivery.states.length !== 2) {
    fail("workflow_policy.catalog_delivery.states must name exactly the published and deferred states");
  }
  const installers = new Set();
  const byId = new Map();
  for (const [index, stateValue] of delivery.states.entries()) {
    const state = object(stateValue, `workflow_policy.catalog_delivery.states[${index}]`);
    const id = nonEmptyText(state.id, `workflow_policy.catalog_delivery.states[${index}].id`);
    const installer = nonEmptyText(
      state.installer,
      `workflow_policy.catalog_delivery.states[${index}].installer`,
    );
    if (!identityMatchesFormat(installer, "identifier")) {
      fail(`workflow_policy.catalog_delivery.states[${index}].installer does not match identifier`);
    }
    if (typeof state.live_catalog_revision !== "boolean") {
      fail(`workflow_policy.catalog_delivery.states[${index}].live_catalog_revision must be a boolean`);
    }
    if (installers.has(installer)) {
      fail("workflow_policy.catalog_delivery states must record distinct installer identities");
    }
    installers.add(installer);
    byId.set(id, state);
  }
  for (const id of ["published", "deferred"]) {
    if (!byId.has(id)) fail(`workflow_policy.catalog_delivery.states must declare the ${id} state`);
  }
  if (byId.get("published").live_catalog_revision !== true) {
    fail("workflow_policy.catalog_delivery published state must consume the live catalog revision");
  }
  if (byId.get("deferred").live_catalog_revision !== false) {
    fail("workflow_policy.catalog_delivery deferred state must not consume a live catalog revision");
  }
  // Naming the two states is not enough on its own: something has to read the mark, or a
  // deferred release's closeout verdict stays indistinguishable from a published one. This
  // names the post-publish cell family whose signed `installer` identity the closeout resolves
  // the delivery state from, so the graph cannot declare the states without a reader.
  const installedCellGroup = nonEmptyText(
    delivery.installed_cell_group,
    "workflow_policy.catalog_delivery.installed_cell_group",
  );
  const group = cellGroups.get(installedCellGroup);
  if (group === undefined) {
    fail(`workflow_policy.catalog_delivery.installed_cell_group ${installedCellGroup} must be a closeout cell group`);
  }
  if (group.phase !== "post_publish") {
    fail(`workflow_policy.catalog_delivery.installed_cell_group ${installedCellGroup} must be a post-publish cell group`);
  }
  for (const key of ["required_identity", "singleton_identity"]) {
    if (!(group[key] ?? []).includes("installer")) {
      fail(`workflow_policy.catalog_delivery.installed_cell_group ${installedCellGroup} must carry installer in ${key}`);
    }
  }
  return delivery;
}

function validateCalibrationPolicy(value) {
  const calibration = object(value, "workflow_policy.calibration");
  if (
    calibration.coordinator_workflow !== "packaged-platform-pr.yml"
    || calibration.mode !== "calibration"
    || calibration.assembly_job !== "calibration-assemble"
    || calibration.pre_collection_source_proof_required !== false
    || calibration.source_proof_stage !== "frozen_candidate_before_qualification"
  ) {
    fail("workflow_policy.calibration must collect before the sole frozen-candidate source proof");
  }
  if (calibration.runs_per_required_cell !== 3) {
    fail("workflow_policy.calibration must require exactly three clean runs per required cell");
  }
  if (calibration.samples_per_metric_per_run !== 1) {
    fail("workflow_policy.calibration must require exactly one sample per metric per run");
  }
  const requiredCells = calibration.required_cells;
  if (!Array.isArray(requiredCells) || requiredCells.length !== 1) {
    fail("workflow_policy.calibration must declare exactly one required cell");
  }
  const required = object(requiredCells[0], "workflow_policy.calibration.required_cells[0]");
  if (
    required.id !== "protected_macos_arm64_metal"
    || required.workflow !== "macos-metal-proof.yml"
    || required.job !== "packaged-metal"
    || required.policy !== "accelerated"
    || required.backend !== "metal"
    || required.feeds_constant_selection !== true
  ) {
    fail("workflow_policy.calibration required cell must be protected macOS Metal");
  }
  const optionalCells = calibration.optional_cells;
  if (!Array.isArray(optionalCells) || optionalCells.length !== 1) {
    fail("workflow_policy.calibration must declare exactly one optional evidence cell");
  }
  const optional = object(optionalCells[0], "workflow_policy.calibration.optional_cells[0]");
  if (
    optional.id !== "protected_linux_x64_vulkan"
    || optional.workflow !== "linux-vulkan-proof.yml"
    || optional.job !== "optional-constant-calibration"
    || optional.trigger !== "workflow_dispatch"
    || optional.policy !== "accelerated"
    || optional.backend !== "vulkan"
    || optional.assembly_dependency !== false
    || optional.feeds_constant_selection !== false
  ) {
    fail("workflow_policy.calibration optional cell must be standalone non-selecting Linux Vulkan evidence");
  }
  const expectedMetrics = [
    "existing_owner_connect",
    "spawn_convergence",
    "cold_first_vector",
    "first_product_ready",
    "warm_query_ipc",
    "warm_bulk_ipc",
    "bulk_documents_per_second",
    "bulk_tokens_per_second",
    "busy_retry_usefulness",
  ];
  const metrics = stringArray(
    calibration.constant_metrics,
    "workflow_policy.calibration.constant_metrics",
    { nonEmpty: true },
  );
  if (JSON.stringify(metrics) !== JSON.stringify(expectedMetrics)) {
    fail("workflow_policy.calibration constant metrics must match the runtime-constant source set");
  }
  const expectedForbiddenEvidence = [
    "qualification_scenarios",
    "true_idle_exit",
    "total_codestory_process_memory",
    "retrieval_quality",
    "backend_observed_accelerator_residency",
  ];
  const forbiddenEvidence = stringArray(
    calibration.forbidden_evidence,
    "workflow_policy.calibration.forbidden_evidence",
    { nonEmpty: true },
  );
  if (JSON.stringify(forbiddenEvidence) !== JSON.stringify(expectedForbiddenEvidence)) {
    fail("workflow_policy.calibration must exclude qualification-only evidence");
  }
  const forbiddenPolicies = stringArray(
    calibration.forbidden_policies,
    "workflow_policy.calibration.forbidden_policies",
    { nonEmpty: true },
  );
  const forbiddenBackends = stringArray(
    calibration.forbidden_backends,
    "workflow_policy.calibration.forbidden_backends",
    { nonEmpty: true },
  );
  const forbiddenEnvironment = stringArray(
    calibration.forbidden_environment,
    "workflow_policy.calibration.forbidden_environment",
    { nonEmpty: true },
  );
  if (
    JSON.stringify(forbiddenPolicies) !== JSON.stringify(["cpu_explicit"])
    || JSON.stringify(forbiddenBackends) !== JSON.stringify(["cpu"])
    || JSON.stringify(forbiddenEnvironment)
      !== JSON.stringify(["CODESTORY_EMBED_ALLOW_CPU=1"])
  ) {
    fail("workflow_policy.calibration must forbid CPU environment, policy, and backend selection");
  }
}

function validateQualificationPolicy(value) {
  const qualification = object(value, "workflow_policy.qualification");
  if (
    qualification.coordinator_workflow !== "packaged-platform-pr.yml"
    || qualification.mode !== "qualification"
    || qualification.runs_per_available_cell !== 1
  ) {
    fail("workflow_policy.qualification must name the canonical one-run frozen-candidate coordinator");
  }
  const driver = object(
    qualification.driver_contract,
    "workflow_policy.qualification.driver_contract",
  );
  const expectedDriverKeys = [
    "artifact_directory_template",
    "artifact_name_template",
    "build_invocations_per_platform",
    "identity_fields",
    "identity_file",
    "identity_schema_version",
    "producer_job",
    "producer_workflow",
    "public_release_asset",
    "reuse_required",
  ].sort();
  const expectedIdentityFields = [
    "schema_version",
    "source.commit",
    "source.tree",
    "release_version",
    "asset_target",
    "archive.file",
    "archive.bytes",
    "archive.sha256",
    "driver.file",
    "driver.bytes",
    "driver.sha256",
  ];
  if (
    JSON.stringify(Object.keys(driver).sort()) !== JSON.stringify(expectedDriverKeys)
    || driver.producer_workflow !== "packaged-platform-proof.yml"
    || driver.producer_job !== "build"
    || driver.artifact_name_template
      !== "codestory-qualification-driver-{asset_target}"
    || driver.artifact_directory_template !== "."
    || driver.identity_file !== "qualification-driver-identity.json"
    || driver.identity_schema_version !== 1
    || JSON.stringify(driver.identity_fields) !== JSON.stringify(expectedIdentityFields)
    || driver.build_invocations_per_platform !== 1
    || driver.reuse_required !== true
    || driver.public_release_asset !== false
  ) {
    fail("workflow_policy.qualification must bind one archive-matched package-built qualification driver");
  }
  const requiredCells = qualification.required_cells;
  if (!Array.isArray(requiredCells) || requiredCells.length !== 2) {
    fail("workflow_policy.qualification must require protected macOS Metal and Windows Vulkan");
  }
  const expectedRequiredCells = [
    {
      id: "protected_macos_arm64_metal",
      workflow: "macos-metal-proof.yml",
      job: "packaged-metal",
      policy: "accelerated",
      backend: "metal",
    },
    {
      id: "protected_windows_x64_vulkan",
      workflow: "windows-vulkan-proof.yml",
      job: "packaged-vulkan",
      policy: "accelerated",
      backend: "vulkan",
    },
  ];
  if (JSON.stringify(requiredCells) !== JSON.stringify(expectedRequiredCells)) {
    fail("workflow_policy.qualification required cells must be the protected Metal and Vulkan producers");
  }
  const optionalCells = qualification.optional_cells;
  const expectedOptionalCells = [
    {
      id: "protected_linux_x64_vulkan",
      workflow: "linux-vulkan-proof.yml",
      job: "packaged-vulkan",
      trigger: "workflow_dispatch",
      policy: "accelerated",
      backend: "vulkan",
      closeout_dependency: false,
      blocking: false,
    },
  ];
  if (JSON.stringify(optionalCells) !== JSON.stringify(expectedOptionalCells)) {
    fail("workflow_policy.qualification Linux Vulkan cell must be standalone and nonblocking");
  }
  const quality = object(
    qualification.quality_contract,
    "workflow_policy.qualification.quality_contract",
  );
  const expectedQuality = {
    producer_workflow: "packaged-platform-pr.yml",
    producer_job: "frozen-candidate-quality",
    producer_cell: "protected_macos_arm64_metal",
    scheduled_once_per_frozen_candidate: true,
    blocking: false,
    closeout_dependency: false,
    claimed: false,
    archive_cache_key_fields: [
      "source.commit",
      "target",
      "archive.sha256",
    ],
    archive_cache_contract: "candidate_archive_cache",
    archive_transfer: "authenticated_miss_only",
    evaluation_owner: "isolated_reusable_workflow",
    evaluation_owner_sha256:
      "92d0a7ab0e0df63dacd5cc3ef0b58500a6578036494c329aa35279048734f173",
    evaluation_contract: "publishable-three-repeat-packet/v1",
    task_count: 1,
    repeats_per_task: 3,
    row_count: 3,
  };
  if (
    JSON.stringify(quality) !== JSON.stringify(expectedQuality)
  ) {
    fail("workflow_policy.qualification quality contract must bind the optional isolated exact-package adjunct");
  }
  const expectedEvidence = [
    "qualification_scenarios",
    "true_idle_exit",
    "total_codestory_process_memory",
    "backend_observed_accelerator_residency",
  ];
  if (
    JSON.stringify(stringArray(
      qualification.required_evidence,
      "workflow_policy.qualification.required_evidence",
      { nonEmpty: true },
    )) !== JSON.stringify(expectedEvidence)
  ) {
    fail("workflow_policy.qualification must retain lifecycle evidence without optional retrieval quality");
  }
  const expectedScenarios = [
    "client_death",
    "cold_race",
    "frozen_owner",
    "incompatible_owner",
    "mixed_queue",
    "server_crash",
    "true_idle_respawn",
    "worker_stall",
  ];
  if (
    JSON.stringify(stringArray(
      qualification.required_scenarios,
      "workflow_policy.qualification.required_scenarios",
      { nonEmpty: true },
    )) !== JSON.stringify(expectedScenarios)
  ) {
    fail("workflow_policy.qualification must run each lifecycle and fault scenario once");
  }
  if (
    qualification.true_idle_timeout_ms !== 60_000
    || qualification.true_idle_observation_grace_ms !== 2_500
  ) {
    fail("workflow_policy.qualification true-idle bound must be the product timeout plus explicit grace");
  }
  if (
    JSON.stringify(qualification.forbidden_policies) !== JSON.stringify(["cpu_explicit"])
    || JSON.stringify(qualification.forbidden_backends) !== JSON.stringify(["cpu"])
    || JSON.stringify(qualification.forbidden_environment)
      !== JSON.stringify(["CODESTORY_EMBED_ALLOW_CPU=1"])
  ) {
    fail("workflow_policy.qualification must forbid CPU environment, policy, and backend selection");
  }
}

function exactStringList(value, expected, label) {
  const actual = stringArray(value, label, { nonEmpty: true });
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${label} must be exactly ${expected.join(", ")}`);
  }
  return actual;
}

function validateProofFloor(value) {
  const floor = object(value, "workflow_policy.proof_floor");
  if (
    floor.schema !== 1
    || JSON.stringify(Object.keys(floor).sort())
      !== JSON.stringify(["architecture_contract", "crate_durability", "schema"])
  ) {
    fail("workflow_policy.proof_floor must use the exact schema 1 contract");
  }

  const architecture = object(
    floor.architecture_contract,
    "workflow_policy.proof_floor.architecture_contract",
  );
  nonEmptyText(
    architecture.workflow,
    "workflow_policy.proof_floor.architecture_contract.workflow",
  );
  if (
    JSON.stringify(Object.keys(architecture).sort())
      !== JSON.stringify(["command", "job", "workflow"])
    || architecture.job !== "linux-contracts"
    || architecture.command
      !== "cargo test --locked -p codestory-cli --test architecture_contracts"
  ) {
    fail("workflow_policy.proof_floor architecture contract must stay in the universal linux-contracts lane");
  }

  const durability = object(
    floor.crate_durability,
    "workflow_policy.proof_floor.crate_durability",
  );
  if (
    JSON.stringify(Object.keys(durability).sort())
      !== JSON.stringify([
        "artifact_free",
        "branches",
        "cache_namespace",
        "commands",
        "job",
        "paths",
        "timeout_minutes",
        "workflow",
      ])
    || durability.workflow !== "crate-durability.yml"
    || durability.job !== "linux-durability"
    || durability.artifact_free !== true
    || durability.timeout_minutes !== 60
    || durability.cache_namespace !== "crate-durability-v1"
  ) {
    fail("workflow_policy.proof_floor crate durability lane must keep its source-only identity and bound");
  }
  exactStringList(
    durability.branches,
    ["main", "dev/codestory-next"],
    "workflow_policy.proof_floor.crate_durability.branches",
  );
  exactStringList(
    durability.paths,
    [
      "crates/codestory-store/**",
      "crates/codestory-indexer/**",
      "crates/codestory-workspace/**",
      "crates/codestory-contracts/**",
      ".github/workflows/crate-durability.yml",
      ".github/scripts/check-workflow-policy.mjs",
      ".github/scripts/check-workflow-policy.test.mjs",
      "release-claims.json",
      "scripts/codestory-release-claims.mjs",
      "scripts/tests/codestory-release-claims.test.mjs",
      "docs/contributors/testing-matrix.md",
    ],
    "workflow_policy.proof_floor.crate_durability.paths",
  );
  exactStringList(
    durability.commands,
    [
      "cargo test --locked -p codestory-store",
      "cargo test --locked -p codestory-indexer --test fidelity_regression",
      "cargo test --locked -p codestory-indexer --test tictactoe_language_coverage",
    ],
    "workflow_policy.proof_floor.crate_durability.commands",
  );
}

function validateWindowsPackageGraph(value) {
  const graph = object(value, "workflow_policy.windows_package_graph");
  if (
    graph.asset_target !== "windows-x64"
    || graph.cargo_profile !== "release"
    || graph.cargo_build_invocations !== 1
    || graph.cargo_test_invocations_after_build !== 0
    || graph.package_artifact !== "codestory-cli"
  ) {
    fail("workflow_policy.windows_package_graph must build and package one exact Windows release graph");
  }
  exactStringList(
    graph.artifacts,
    [
      "codestory-cli",
      "codestory-cli-runtime",
      "codestory_embedding_qualification",
    ],
    "workflow_policy.windows_package_graph.artifacts",
  );
  if (
    JSON.stringify(stringArray(
      graph.direct_test_harnesses,
      "workflow_policy.windows_package_graph.direct_test_harnesses",
    )) !== JSON.stringify([])
  ) {
    fail("workflow_policy.windows_package_graph.direct_test_harnesses must be empty");
  }
  exactStringList(
    graph.source_test_harnesses,
    ["native_staging", "windows_path_identity"],
    "workflow_policy.windows_package_graph.source_test_harnesses",
  );
  exactStringList(
    graph.production_feature_probes,
    ["cargo_message_feature_contract", "runtime_observation_source"],
    "workflow_policy.windows_package_graph.production_feature_probes",
  );
  exactStringList(
    graph.timing_phases,
    [
      "cache_restore",
      "native_setup",
      "cargo_graph",
      "msvc_link",
      "feature_probe",
      "packaging",
      "artifact_transfer",
    ],
    "workflow_policy.windows_package_graph.timing_phases",
  );
}

function validateCandidateArchiveCache(value) {
  const cache = object(value, "workflow_policy.candidate_archive_cache");
  if (
    cache.record_schema !== "codestory-candidate-archive-store/v1"
    || cache.protected_host_root !== "runner_tool_cache"
    || cache.package_producer_workflow !== "packaged-platform-proof.yml"
    || cache.package_artifact_name !== "codestory-cli-{asset_target}"
    || cache.record_artifact_name
      !== "codestory-candidate-archive-record-{asset_target}"
    || cache.qualification_driver_artifact_name
      !== "codestory-qualification-driver-{asset_target}"
    || cache.miss_admission !== "same_filesystem_atomic_rename"
    || cache.owned_corruption !== "quarantine_then_authenticated_miss"
    || cache.unowned_corruption !== "fail_closed"
    || cache.cross_source_reuse !== false
    || cache.restore_prefixes !== false
    || cache.public_asset_reauthentication !== true
  ) {
    fail("workflow_policy.candidate_archive_cache must retain exact-source atomic protected-host reuse");
  }
  exactStringList(
    cache.key_fields,
    ["source.commit", "target", "archive.sha256"],
    "workflow_policy.candidate_archive_cache.key_fields",
  );
  exactStringList(
    cache.hit_verification,
    [
      "repository",
      "source.commit",
      "source.tree",
      "target",
      "archive.file",
      "archive.bytes",
      "archive.sha256",
    ],
    "workflow_policy.candidate_archive_cache.hit_verification",
  );
}

function validateModelMaterialCache(value) {
  const cache = object(value, "workflow_policy.model_material_cache");
  if (
    cache.key_field !== "model.sha256"
    || cache.source_sha_in_key !== false
    || cache.toolchain_in_key !== false
    || cache.miss_admission !== "same_filesystem_atomic_no_replace"
  ) {
    fail("workflow_policy.model_material_cache must key immutable model material only by model SHA");
  }
  exactStringList(
    cache.hit_verification,
    [
      "model.real_ancestry",
      "model.single_link",
      "model.size_bytes",
      "model.sha256",
    ],
    "workflow_policy.model_material_cache.hit_verification",
  );
}

function validatePublicSupport(graph, packageTargets, cellGroups) {
  const publicSupport = object(
    graph.public_support,
    "release claim graph.public_support",
  );
  if (!/^\d+\.\d+$/u.test(publicSupport.release_line)) {
    fail("public_support.release_line must be a major.minor release line");
  }
  if (!Array.isArray(publicSupport.packages) || publicSupport.packages.length === 0) {
    fail("public_support.packages must be a non-empty array");
  }

  const supportRows = new Map();
  for (const [index, value] of publicSupport.packages.entries()) {
    const row = object(value, `public_support.packages[${index}]`);
    const target = nonEmptyText(row.target, `public_support.packages[${index}].target`);
    if (supportRows.has(target)) fail(`public_support.packages duplicates ${target}`);
    nonEmptyText(row.label, `public_support.packages[${index}].label`);
    if (row.local_map !== "supported") {
      fail(`public_support package ${target} must support the local map`);
    }
    if (row.broad_retrieval !== "accelerated") {
      fail(`public_support package ${target} must require accelerated broad retrieval`);
    }
    if (!new Set(["none", "metal", "vulkan"]).has(row.accelerator_claim)) {
      fail(`public_support package ${target} has an invalid accelerator claim`);
    }
    supportRows.set(target, row);
  }
  if (
    JSON.stringify([...supportRows.keys()].sort())
    !== JSON.stringify([...packageTargets].sort())
  ) {
    fail("public_support package targets must exactly match workflow_policy.package_matrix");
  }

  const acceleratorCloseoutTargets = new Set(
    [...cellGroups.values()]
      .filter(({ claim }) => claim === "accelerator_execution")
      .flatMap(({ instances = [] }) => instances)
      .map(({ identity_constraints: constraints }) => constraints?.target)
      .filter(Boolean),
  );
  for (const [target, row] of supportRows) {
    if (
      row.accelerator_claim !== "none"
      && !acceleratorCloseoutTargets.has(target)
    ) {
      fail(`public accelerator claim ${target}/${row.accelerator_claim} has no required closeout cell`);
    }
    if (row.accelerator_claim === "none") {
      fail(`public_support package ${target} must name its accelerator claim`);
    }
  }

  const unshippedTargets = stringArray(
    publicSupport.unshipped_targets,
    "public_support.unshipped_targets",
    { nonEmpty: true },
  );
  const expectedUnshippedTargets = [...KNOWN_PACKAGE_TARGETS]
    .filter((target) => !packageTargets.has(target))
    .sort();
  if (JSON.stringify([...unshippedTargets].sort()) !== JSON.stringify(expectedUnshippedTargets)) {
    fail("public_support.unshipped_targets must name every non-release package target");
  }
  if (unshippedTargets.some((target) => supportRows.has(target))) {
    fail("public_support cannot mark a shipped package target as unshipped");
  }

  if (!Array.isArray(publicSupport.unsupported) || publicSupport.unsupported.length === 0) {
    fail("public_support.unsupported must be a non-empty array");
  }
  const unsupportedLabels = new Set();
  for (const [index, value] of publicSupport.unsupported.entries()) {
    const row = object(value, `public_support.unsupported[${index}]`);
    const label = nonEmptyText(row.label, `public_support.unsupported[${index}].label`);
    if (unsupportedLabels.has(label)) fail(`public_support.unsupported duplicates ${label}`);
    unsupportedLabels.add(label);
    const targets = stringArray(
      row.targets,
      `public_support.unsupported[${index}].targets`,
      { nonEmpty: true },
    );
    if (targets.some((target) => !KNOWN_PACKAGE_TARGETS.has(target))) {
      fail(`public_support.unsupported ${label} names an unknown package target`);
    }
    if (!new Set(["all", "cpu_only"]).has(row.condition)) {
      fail(`public_support.unsupported ${label} has an invalid condition`);
    }
    if (row.condition === "all" && targets.some((target) => supportRows.has(target))) {
      fail(`public_support.unsupported ${label} conflicts with a shipped package target`);
    }
    if (
      row.condition === "cpu_only"
      && targets.some((target) =>
        !supportRows.has(target) || supportRows.get(target).accelerator_claim === "none")
    ) {
      fail(`public_support.unsupported ${label} conflicts with a CPU-supported package target`);
    }
  }
  if (
    JSON.stringify(publicSupport.unsupported)
    !== JSON.stringify(UNSUPPORTED_RELEASE_CONFIGURATIONS)
  ) {
    fail("public_support.unsupported must match the canonical unsupported release matrix");
  }

  const unclaimedCapabilities = stringArray(
    publicSupport.unclaimed_capabilities,
    "public_support.unclaimed_capabilities",
    { nonEmpty: true },
  );
  if (
    JSON.stringify([...unclaimedCapabilities].sort())
    !== JSON.stringify(["answer_quality", "performance"])
  ) {
    fail("public_support.unclaimed_capabilities must name the release non-claims");
  }
}

// The plugin lane's job DAG lives in the claim graph rather than in check-workflow-policy.mjs, and
// the checker only asserts that the workflow's `needs:` match whatever this data says. That makes
// this the only place left that can tell a real ordering contract from an empty one: with the
// dependency lists blanked out, both gates would pass while `gh release create` ran with the
// release-authority checks and the whole plugin-proof matrix detached from it.
const PLUGIN_CHAIN_ROOT = "workflow-policy";
const PLUGIN_CHAIN_ORDER = [
  // Tagging is irreversible, so everything that can still refuse the release runs before it.
  ["publish", "preflight"],
  ["publish", "plugin-proof"],
  // The catalog and the install proof that reads it only mean anything once the release exists.
  ["marketplace-publish", "publish"],
  ["post-publish-smoke", "publish"],
  ["post-publish-smoke", "marketplace-publish"],
];

function pluginChainAncestors(dependencies, job, seen = new Set()) {
  for (const dependency of dependencies[job] ?? []) {
    if (seen.has(dependency)) continue;
    seen.add(dependency);
    pluginChainAncestors(dependencies, dependency, seen);
  }
  return seen;
}

function validatePluginChain(value) {
  const chain = object(value, "workflow_policy.plugin_chain");
  const dependencies = object(chain.dependencies, "workflow_policy.plugin_chain.dependencies");
  const jobs = Object.keys(dependencies);
  if (jobs.length === 0) {
    fail("workflow_policy.plugin_chain.dependencies must declare at least one job");
  }
  const declared = new Set([PLUGIN_CHAIN_ROOT, ...jobs]);
  // Null-prototype so a job named after an Object member cannot smuggle a dependency list past the
  // reachability walk below.
  const resolved = Object.create(null);
  for (const job of jobs) {
    nonEmptyText(job, "workflow_policy.plugin_chain.dependencies job");
    if (job === PLUGIN_CHAIN_ROOT) {
      fail(`workflow_policy.plugin_chain.dependencies must not redeclare ${PLUGIN_CHAIN_ROOT}`);
    }
    resolved[job] = stringArray(
      dependencies[job],
      `workflow_policy.plugin_chain.dependencies.${job}`,
      { nonEmpty: true },
    );
    for (const dependency of resolved[job]) {
      if (!declared.has(dependency)) {
        fail(`workflow_policy.plugin_chain.dependencies.${job} names undeclared job ${dependency}`);
      }
    }
  }
  for (const job of jobs) {
    const ancestors = pluginChainAncestors(resolved, job);
    if (ancestors.has(job)) {
      fail(`workflow_policy.plugin_chain.dependencies.${job} cannot depend on itself`);
    }
    if (!ancestors.has(PLUGIN_CHAIN_ROOT)) {
      fail(`workflow_policy.plugin_chain.dependencies.${job} must run behind ${PLUGIN_CHAIN_ROOT}`);
    }
  }
  for (const [job, required] of PLUGIN_CHAIN_ORDER) {
    if (!declared.has(job) || !declared.has(required)) {
      fail(`workflow_policy.plugin_chain.dependencies must declare ${job} and ${required}`);
    }
    if (!pluginChainAncestors(resolved, job).has(required)) {
      fail(`workflow_policy.plugin_chain.dependencies.${job} must run behind ${required}`);
    }
  }
}

export function canonicalReleaseClaimValue(value) {
  if (Array.isArray(value)) return value.map(canonicalReleaseClaimValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value).sort().map((key) => [key, canonicalReleaseClaimValue(value[key])]),
    );
  }
  return value;
}

export function releaseClaimGraphDigest(graph) {
  return createHash("sha256")
    .update(`${JSON.stringify(canonicalReleaseClaimValue(graph))}\n`)
    .digest("hex");
}

export function validateReleaseClaimGraph(graph) {
  object(graph, "release claim graph");
  if (graph.schema !== GRAPH_SCHEMA || graph.graph_version !== GRAPH_VERSION) {
    fail(`release claim graph must use ${GRAPH_SCHEMA} graph_version ${GRAPH_VERSION}`);
  }
  nonEmptyText(graph.graph_id, "release claim graph.graph_id");
  const standardReleaseClaims = stringArray(
    graph.standard_release_claims,
    "release claim graph.standard_release_claims",
    { nonEmpty: true },
  ).sort();
  if (JSON.stringify(standardReleaseClaims) !== JSON.stringify(STANDARD_RELEASE_CLAIMS)) {
    fail(`release claim graph standard release must require exactly ${STANDARD_RELEASE_CLAIMS.join(", ")}`);
  }
  const optionalEvaluations = stringArray(
    graph.optional_evaluations,
    "release claim graph.optional_evaluations",
    { nonEmpty: true },
  ).sort();
  if (JSON.stringify(optionalEvaluations) !== JSON.stringify(OPTIONAL_EVALUATIONS)) {
    fail(`release claim graph optional evaluations must be exactly ${OPTIONAL_EVALUATIONS.join(", ")}`);
  }
  if (standardReleaseClaims.some((claim) => optionalEvaluations.includes(claim))) {
    fail("release claim graph standard release claims and optional evaluations must be disjoint");
  }
  const evidencePolicy = object(graph.evidence_policy, "release claim graph.evidence_policy");
  if (evidencePolicy.selection !== "all_matching_rows_must_pass") {
    fail("release claim graph evidence selection must be all_matching_rows_must_pass");
  }
  if (evidencePolicy.validity !== "observed_at_expires_at") {
    fail("release claim graph evidence validity must be observed_at_expires_at");
  }
  const identityBinding = stringArray(evidencePolicy.identity_binding, "release claim graph.evidence_policy.identity_binding", { nonEmpty: true });
  if (JSON.stringify(identityBinding) !== JSON.stringify(["repository", "commit", "source_tree"])) {
    fail("release claim graph evidence identity binding must be repository, commit, source_tree");
  }
  const identityFormats = object(evidencePolicy.identity_formats, "release claim graph.evidence_policy.identity_formats");
  for (const [key, format] of Object.entries(identityFormats)) {
    nonEmptyText(key, "release claim graph evidence identity key");
    if (!IDENTITY_FORMATS.has(format)) fail(`identity ${key} uses unknown format ${String(format)}`);
  }
  for (const key of identityBinding) {
    if (!identityFormats[key]) fail(`identity ${key} must declare a format`);
  }
  validateReuseBindings(evidencePolicy, identityBinding);

  const exceptionPolicy = object(graph.exception_policy, "release claim graph.exception_policy");
  if (exceptionPolicy.schema !== "codestory.model-microbenchmark-exception/v1") {
    fail("release claim graph exception policy uses an unsupported schema");
  }
  if (exceptionPolicy.eligible_evidence_type !== "performance") {
    fail("release claim graph exceptions must be limited to performance evidence");
  }
  if (exceptionPolicy.regression_class !== "model_microbenchmark") {
    fail("release claim graph exceptions must be limited to model_microbenchmark regressions");
  }
  if (exceptionPolicy.minimum_regression_percent !== 5) {
    fail("release claim graph model exceptions must require regressions over 5 percent");
  }
  if (exceptionPolicy.minimum_repeats !== 3) {
    fail("release claim graph model exceptions must require at least three repeats");
  }
  if (exceptionPolicy.maximum_validity_days !== 14) {
    fail("release claim graph model exceptions must expire within 14 days");
  }
  if (exceptionPolicy.artifact_binding !== "candidate_sha256") {
    fail("release claim graph model exceptions must bind the exact candidate_sha256 artifact");
  }
  if (exceptionPolicy.release_binding !== "release_key") {
    fail("release claim graph model exceptions must bind the selected release_key");
  }
  if (exceptionPolicy.full_product_benefit_evidence_type !== "answer_quality") {
    fail("release claim graph model exceptions must cite answer_quality full-product evidence");
  }
  stringArray(
    exceptionPolicy.non_waivable_metrics,
    "release claim graph.exception_policy.non_waivable_metrics",
    { nonEmpty: true },
  );

  const tiers = uniqueById(graph.proof_tiers, "release claim graph.proof_tiers");
  const ranks = new Set();
  for (const [id, tier] of tiers) {
    if (!Number.isInteger(tier.rank) || tier.rank <= 0) fail(`proof tier ${id} rank must be a positive integer`);
    if (ranks.has(tier.rank)) fail(`proof tier rank ${tier.rank} is duplicated`);
    ranks.add(tier.rank);
  }

  const evidenceTypes = uniqueById(graph.evidence_types, "release claim graph.evidence_types");
  for (const type of [
    exceptionPolicy.eligible_evidence_type,
    exceptionPolicy.full_product_benefit_evidence_type,
  ]) {
    if (!evidenceTypes.has(type)) fail(`release claim graph exception policy references unknown evidence type ${type}`);
  }
  for (const [id, evidence] of evidenceTypes) {
    if (!tiers.has(evidence.tier)) fail(`evidence type ${id} references unknown tier ${evidence.tier}`);
    stringArray(evidence.proof_lanes, `evidence type ${id}.proof_lanes`, { nonEmpty: true });
    if (evidence.validity !== "expires_at") fail(`evidence type ${id}.validity must be expires_at`);
    if (!Number.isInteger(evidence.maximum_validity_hours) || evidence.maximum_validity_hours <= 0) {
      fail(`evidence type ${id}.maximum_validity_hours must be a positive integer`);
    }
    const identity = stringArray(evidence.required_identity, `evidence type ${id}.required_identity`, { nonEmpty: true });
    for (const required of ["repository", "commit", "source_tree"]) {
      if (!identity.includes(required)) fail(`evidence type ${id} must require ${required} identity`);
    }
    for (const key of identity) {
      if (!identityFormats[key]) fail(`evidence type ${id} identity ${key} must declare a format`);
    }
    const constraints = object(evidence.identity_constraints ?? {}, `evidence type ${id}.identity_constraints`);
    for (const [key, value] of Object.entries(constraints)) {
      if (!identity.includes(key)) fail(`evidence type ${id} constrains non-required identity ${key}`);
      if (!identityMatchesFormat(value, identityFormats[key])) {
        fail(`evidence type ${id} constraint ${key} does not match ${identityFormats[key]}`);
      }
    }
  }

  const claims = uniqueById(graph.claims, "release claim graph.claims");
  if (JSON.stringify([...claims.keys()].sort()) !== JSON.stringify(REQUIRED_CLAIMS)) {
    fail(`release claim graph must define exactly ${REQUIRED_CLAIMS.join(", ")}`);
  }
  for (const id of [...standardReleaseClaims, ...optionalEvaluations]) {
    if (!claims.has(id)) fail(`release claim graph classifies unknown claim ${id}`);
  }
  for (const [id, claim] of claims) {
    if (!tiers.has(claim.minimum_tier)) fail(`claim ${id} references unknown minimum tier ${claim.minimum_tier}`);
    const dependencies = stringArray(claim.depends_on_claims, `claim ${id}.depends_on_claims`);
    for (const dependency of dependencies) {
      if (!claims.has(dependency)) fail(`claim ${id} depends on unknown claim ${dependency}`);
      if (dependency === id) fail(`claim ${id} cannot depend on itself`);
    }
    const requirements = stringArray(claim.required_evidence, `claim ${id}.required_evidence`, { nonEmpty: true });
    for (const requirement of requirements) {
      if (!evidenceTypes.has(requirement)) fail(`claim ${id} requires unknown evidence type ${requirement}`);
    }
    const minimumRank = tiers.get(claim.minimum_tier).rank;
    if (!requirements.some((requirement) => tiers.get(evidenceTypes.get(requirement).tier).rank >= minimumRank)) {
      fail(`claim ${id} has no requirement at or above minimum tier ${claim.minimum_tier}`);
    }
    stringArray(claim.prerequisites, `claim ${id}.prerequisites`, { nonEmpty: true });
    if (!Array.isArray(claim.prerequisite_checks) || claim.prerequisite_checks.length === 0) {
      fail(`claim ${id}.prerequisite_checks must be a non-empty array`);
    }
    const checkIds = new Set();
    for (const [index, checkValue] of claim.prerequisite_checks.entries()) {
      const check = object(checkValue, `claim ${id}.prerequisite_checks[${index}]`);
      const checkId = nonEmptyText(check.id, `claim ${id}.prerequisite_checks[${index}].id`);
      if (checkIds.has(checkId)) fail(`claim ${id}.prerequisite_checks duplicates ${checkId}`);
      checkIds.add(checkId);
      nonEmptyText(check.command, `claim ${id}.prerequisite_checks[${index}].command`);
    }
    stringArray(claim.non_claims, `claim ${id}.non_claims`, { nonEmpty: true });
    stringArray(claim.accepted_risks, `claim ${id}.accepted_risks`);
  }
  const visiting = new Set();
  const visited = new Set();
  const visitClaim = (id) => {
    if (visiting.has(id)) fail(`release claim graph contains dependency cycle at ${id}`);
    if (visited.has(id)) return;
    visiting.add(id);
    for (const dependency of claims.get(id).depends_on_claims) visitClaim(dependency);
    visiting.delete(id);
    visited.add(id);
  };
  for (const id of claims.keys()) visitClaim(id);

  const controls = uniqueById(graph.failure_controls, "release claim graph.failure_controls");
  if (JSON.stringify([...controls.keys()].sort()) !== JSON.stringify(REQUIRED_FAILURE_CONTROLS)) {
    fail(`release claim graph must map exactly ${REQUIRED_FAILURE_CONTROLS.join(", ")}`);
  }
  for (const [id, control] of controls) {
    if (!claims.has(control.claim)) fail(`failure control ${id} references unknown claim ${control.claim}`);
    if (control.control !== "negative_gate") fail(`failure control ${id} must be a negative_gate`);
    const command = nonEmptyText(control.command, `failure control ${id}.command`);
    if (id === "benchmark_leakage") {
      if (command !== BENCHMARK_LEAKAGE_COMMAND) {
        fail(`failure control ${id} must be exactly ${BENCHMARK_LEAKAGE_COMMAND}`);
      }
    } else if (!command.startsWith("cargo test --locked ")) {
      fail(`failure control ${id} must name a locked executable Cargo test`);
    }
  }

  const closeout = object(graph.closeout, "release claim graph.closeout");
  if (closeout.schema !== "codestory.release-closeout/v1") {
    fail("release claim graph.closeout.schema must be codestory.release-closeout/v1");
  }
  for (const [key, expected] of Object.entries({
    manifest_schema: "codestory.release-cell-manifest/v1",
    ledger_schema: "codestory.release-closeout-ledger/v1",
    summary_schema: "codestory.release-closeout-summary/v1",
  })) {
    if (closeout[key] !== expected) fail(`release claim graph.closeout.${key} must be ${expected}`);
  }
  const phases = stringArray(closeout.phases, "release claim graph.closeout.phases", { nonEmpty: true });
  if (JSON.stringify(phases) !== JSON.stringify(["pre_publish", "post_publish"])) {
    fail("release claim graph.closeout.phases must be pre_publish, post_publish");
  }
  const cellGroups = uniqueById(closeout.cell_groups, "release claim graph.closeout.cell_groups");
  const requiredCellGroups = [
    "accelerator_execution",
    "candidate_installed_behavior",
    "installed_runtime_behavior",
    "package_identity",
    "platform_support",
    "post_publish_bytes",
    "retrieval_readiness",
    "source_behavior",
  ];
  if (JSON.stringify([...cellGroups.keys()].sort()) !== JSON.stringify(requiredCellGroups)) {
    fail(`release claim graph closeout must define exactly ${requiredCellGroups.join(", ")}`);
  }
  for (const [id, group] of cellGroups) {
    if (!phases.includes(group.phase)) fail(`closeout cell group ${id} has unknown phase ${String(group.phase)}`);
    const claim = claims.get(nonEmptyText(group.claim, `closeout cell group ${id}.claim`));
    if (!claim) fail(`closeout cell group ${id} references unknown claim ${group.claim}`);
    const evidenceTypeId = nonEmptyText(group.evidence_type, `closeout cell group ${id}.evidence_type`);
    const evidenceType = evidenceTypes.get(evidenceTypeId);
    if (!evidenceType) fail(`closeout cell group ${id} references unknown evidence type ${evidenceTypeId}`);
    if (!claim.required_evidence.includes(evidenceTypeId)) {
      fail(`closeout cell group ${id} evidence type ${evidenceTypeId} does not satisfy claim ${group.claim}`);
    }
    if (!new Set(["singleton", "package_matrix", "instances"]).has(group.expansion)) {
      fail(`closeout cell group ${id} has unknown expansion ${String(group.expansion)}`);
    }
    if (!new Set(["none", "pre_publish", "post_publish_compare"]).has(group.archive_role)) {
      fail(`closeout cell group ${id} has unknown archive_role ${String(group.archive_role)}`);
    }
    // A group admits cross-run evidence by naming a binding the reuse policy declares -- and only
    // by that. What the binding may then equate is the binding's business, stated once beside the
    // proof, never a per-group exception.
    if (group.reuse_binding !== undefined) {
      const binding = nonEmptyText(group.reuse_binding, `closeout cell group ${id}.reuse_binding`);
      if (!Object.hasOwn(evidencePolicy.reuse.bindings, binding)) {
        fail(`closeout cell group ${id} names undeclared reuse binding ${binding}`);
      }
    }
    const requiredIdentity = stringArray(
      group.required_identity,
      `closeout cell group ${id}.required_identity`,
      { nonEmpty: true },
    );
    for (const key of evidenceType.required_identity) {
      if (!requiredIdentity.includes(key)) {
        fail(`closeout cell group ${id} must retain evidence identity ${key}`);
      }
    }
    for (const key of requiredIdentity) {
      if (!identityFormats[key]) fail(`closeout cell group ${id} identity ${key} must declare a format`);
    }
    const constraints = object(group.identity_constraints ?? {}, `closeout cell group ${id}.identity_constraints`);
    for (const [key, value] of Object.entries(constraints)) {
      if (!requiredIdentity.includes(key)) fail(`closeout cell group ${id} constrains non-required identity ${key}`);
      if (!identityMatchesFormat(value, identityFormats[key])) {
        fail(`closeout cell group ${id} constraint ${key} does not match ${identityFormats[key]}`);
      }
    }
    for (const key of stringArray(group.singleton_identity ?? [], `closeout cell group ${id}.singleton_identity`)) {
      if (!requiredIdentity.includes(key)) fail(`closeout cell group ${id} singleton identity ${key} is not required`);
    }
    if (group.expansion === "package_matrix" && !requiredIdentity.includes("target")) {
      fail(`closeout cell group ${id} package_matrix expansion must require target`);
    }
    if (group.expansion === "instances") {
      const instances = uniqueById(group.instances, `closeout cell group ${id}.instances`);
      for (const [instanceId, instance] of instances) {
        const instanceConstraints = object(
          instance.identity_constraints,
          `closeout cell group ${id} instance ${instanceId}.identity_constraints`,
        );
        for (const [key, value] of Object.entries(instanceConstraints)) {
          if (!requiredIdentity.includes(key)) {
            fail(`closeout cell group ${id} instance ${instanceId} constrains non-required identity ${key}`);
          }
          if (!identityMatchesFormat(value, identityFormats[key])) {
            fail(`closeout cell group ${id} instance ${instanceId} constraint ${key} does not match ${identityFormats[key]}`);
          }
        }
      }
    } else if (group.instances !== undefined) {
      fail(`closeout cell group ${id} may declare instances only with instances expansion`);
    }
  }

  validateNonClaimPolicy(graph, cellGroups);

  const policy = object(graph.workflow_policy, "release claim graph.workflow_policy");
  if (!Number.isInteger(policy.artifact_retention_days) || policy.artifact_retention_days <= 0) {
    fail("workflow_policy.artifact_retention_days must be a positive integer");
  }
  validateProofFloor(policy.proof_floor);
  if (!Array.isArray(policy.package_matrix) || policy.package_matrix.length !== 3) {
    fail("workflow_policy.package_matrix must define three release package rows");
  }
  const targets = new Set();
  for (const [index, rowValue] of policy.package_matrix.entries()) {
    const row = object(rowValue, `workflow_policy.package_matrix[${index}]`);
    for (const key of ["os", "rust_target", "asset_target", "extension"]) {
      nonEmptyText(row[key], `workflow_policy.package_matrix[${index}].${key}`);
    }
    if (typeof row.exe_suffix !== "string") fail(`workflow_policy.package_matrix[${index}].exe_suffix must be a string`);
    if (targets.has(row.asset_target)) fail(`workflow_policy.package_matrix duplicates ${row.asset_target}`);
    if (!KNOWN_PACKAGE_TARGETS.has(row.asset_target)) {
      fail(`workflow_policy.package_matrix names unknown target ${row.asset_target}`);
    }
    targets.add(row.asset_target);
  }
  validateWindowsPackageGraph(policy.windows_package_graph);
  validateCandidateArchiveCache(policy.candidate_archive_cache);
  validateModelMaterialCache(policy.model_material_cache);
  validateCalibrationPolicy(policy.calibration);
  validateQualificationPolicy(policy.qualification);
  validatePublicSupport(graph, targets, cellGroups);
  if (!Array.isArray(policy.protected_jobs) || policy.protected_jobs.length === 0) {
    fail("workflow_policy.protected_jobs must be a non-empty array");
  }
  const protectedJobs = new Set();
  for (const [index, rowValue] of policy.protected_jobs.entries()) {
    const row = object(rowValue, `workflow_policy.protected_jobs[${index}]`);
    const key = `${nonEmptyText(row.workflow, `workflow_policy.protected_jobs[${index}].workflow`)}/${nonEmptyText(row.job, `workflow_policy.protected_jobs[${index}].job`)}`;
    if (protectedJobs.has(key)) fail(`workflow_policy.protected_jobs duplicates ${key}`);
    protectedJobs.add(key);
    stringArray(row.runner, `workflow_policy.protected_jobs[${index}].runner`, { nonEmpty: true });
    nonEmptyText(row.environment, `workflow_policy.protected_jobs[${index}].environment`);
    object(row.permissions, `workflow_policy.protected_jobs[${index}].permissions`);
    stringArray(row.secrets, `workflow_policy.protected_jobs[${index}].secrets`);
  }
  const releaseChain = object(policy.release_chain, "workflow_policy.release_chain");
  const exactShaJobs = stringArray(
    releaseChain.exact_sha_jobs,
    "workflow_policy.release_chain.exact_sha_jobs",
    { nonEmpty: true },
  );
  if (exactShaJobs.includes("release-evidence")) {
    fail("optional release evidence must not be an exact-SHA job in the standard release chain");
  }
  const dependencies = object(releaseChain.dependencies, "workflow_policy.release_chain.dependencies");
  for (const [job, needsValue] of Object.entries(dependencies)) {
    nonEmptyText(job, "workflow_policy.release_chain.dependencies job");
    const needs = stringArray(
      needsValue,
      `workflow_policy.release_chain.dependencies.${job}`,
      { nonEmpty: true },
    );
    if (job === "release-evidence" || needs.includes("release-evidence")) {
      fail("optional release evidence must not block a standard release job");
    }
  }
  validatePluginChain(policy.plugin_chain);
  validateCatalogDelivery(policy, dependencies, cellGroups);
  stringArray(policy.artifact_workflows, "workflow_policy.artifact_workflows", { nonEmpty: true });
  const promotion = object(policy.promotion, "workflow_policy.promotion");
  nonEmptyText(promotion.source_branch, "workflow_policy.promotion.source_branch");
  nonEmptyText(promotion.release_branch, "workflow_policy.promotion.release_branch");
  nonEmptyText(promotion.exact_sha_expression, "workflow_policy.promotion.exact_sha_expression");
  nonEmptyText(promotion.proof_run_sha_expression, "workflow_policy.promotion.proof_run_sha_expression");
  nonEmptyText(promotion.manual_pr_ref_hint, "workflow_policy.promotion.manual_pr_ref_hint");
  nonEmptyText(promotion.source_cache_namespace, "workflow_policy.promotion.source_cache_namespace");
  nonEmptyText(promotion.packaged_cache_namespace, "workflow_policy.promotion.packaged_cache_namespace");
  const labelRouted = stringArray(
    promotion.label_routed_workflows,
    "workflow_policy.promotion.label_routed_workflows",
  );
  const requiredEvents = stringArray(
    promotion.required_events,
    "workflow_policy.promotion.required_events",
  );
  if (labelRouted.length !== 0 || requiredEvents.length !== 0) {
    fail("workflow_policy.promotion must not admit label-routed proof workflows");
  }

  const freeze = object(
    policy.release_freeze_barrier,
    "workflow_policy.release_freeze_barrier",
  );
  if (freeze.schema !== 3) {
    fail("workflow_policy.release_freeze_barrier.schema must be 3");
  }
  nonEmptyText(freeze.script, "workflow_policy.release_freeze_barrier.script");
  nonEmptyText(
    freeze.status_context_prefix,
    "workflow_policy.release_freeze_barrier.status_context_prefix",
  );
  stringArray(
    freeze.allowed_future_source_changes,
    "workflow_policy.release_freeze_barrier.allowed_future_source_changes",
    { nonEmpty: true },
  );
  stringArray(
    freeze.required_hostile_mutations,
    "workflow_policy.release_freeze_barrier.required_hostile_mutations",
    { nonEmpty: true },
  );
  stringArray(
    freeze.broad_entry_workflows,
    "workflow_policy.release_freeze_barrier.broad_entry_workflows",
    { nonEmpty: true },
  );
  if (freeze.invalidation_workflow !== "release-freeze-invalidation.yml") {
    fail(
      "workflow_policy.release_freeze_barrier.invalidation_workflow must name "
      + "release-freeze-invalidation.yml",
    );
  }
  stringArray(
    freeze.coordinator_only_workflows,
    "workflow_policy.release_freeze_barrier.coordinator_only_workflows",
    { nonEmpty: true },
  );
  const acceptance = object(
    freeze.acceptance,
    "workflow_policy.release_freeze_barrier.acceptance",
  );
  for (const field of [
    "producer_workflow",
    "receipt_authority",
    "receipt_artifact",
    "receipt_file",
    "receipt_producer_job",
    "status_scope",
    "event",
    "hostile_job",
    "hostile_step",
    "windows_job",
    "windows_step",
    "publisher_job",
    "publisher_step",
    "status_creator",
    "job_manifest",
    "job_manifest_sha256",
  ]) {
    nonEmptyText(
      acceptance[field],
      `workflow_policy.release_freeze_barrier.acceptance.${field}`,
    );
  }
  const windowsRunner = stringArray(
    acceptance.windows_runner,
    "workflow_policy.release_freeze_barrier.acceptance.windows_runner",
    { nonEmpty: true },
  );
  if (
    JSON.stringify([...windowsRunner].sort())
      !== JSON.stringify([
        "self-hosted",
        "Windows",
        "X64",
        "codestory-vulkan",
      ].sort())
  ) {
    fail(
      "workflow_policy.release_freeze_barrier.acceptance.windows_runner "
      + "must name the protected Windows Vulkan runner",
    );
  }
  if (
    acceptance.producer_workflow !== "source-proof.yml"
    || acceptance.receipt_authority !== "github_actions"
    || acceptance.receipt_artifact
      !== "release-freeze-receipt-attempt-${{ github.run_attempt }}"
    || acceptance.receipt_file !== "release-freeze-receipt.json"
    || acceptance.receipt_producer_job !== "resolve"
    || acceptance.status_scope !== "exact_candidate_head"
    || acceptance.later_commit_revokes !== true
    || acceptance.event !== "workflow_dispatch"
    || acceptance.windows_probe_max_seconds !== 90
    || acceptance.status_creator !== "github-actions[bot]"
    || acceptance.job_manifest
      !== ".github/scripts/release-freeze-acceptance-jobs.json"
    || !SHA256.test(acceptance.job_manifest_sha256)
  ) {
    fail(
      "workflow_policy.release_freeze_barrier.acceptance must bind the exact "
      + "Actions receipt authority, immutable artifact, producer, event, protected "
      + "probe budget, status scope, revocation, and status creator",
    );
  }
  const freezePhases = object(
    acceptance.phases,
    "workflow_policy.release_freeze_barrier.acceptance.phases",
  );
  if (
    JSON.stringify(Object.keys(freezePhases).sort())
      !== JSON.stringify(["calibration_source", "frozen_candidate"])
  ) {
    fail(
      "workflow_policy.release_freeze_barrier.acceptance.phases must define "
      + "exactly calibration_source and frozen_candidate",
    );
  }
  const constantSet =
    "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";
  const calibrationSource = object(
    freezePhases.calibration_source,
    "workflow_policy.release_freeze_barrier.acceptance.phases.calibration_source",
  );
  const calibrationFuture = stringArray(
    calibrationSource.known_future_source_changes,
    "workflow_policy.release_freeze_barrier.acceptance.phases.calibration_source.known_future_source_changes",
    { nonEmpty: true },
  );
  const calibrationActions = stringArray(
    calibrationSource.planned_actions,
    "workflow_policy.release_freeze_barrier.acceptance.phases.calibration_source.planned_actions",
    { nonEmpty: true },
  );
  if (
    JSON.stringify(calibrationFuture) !== JSON.stringify([constantSet])
    || JSON.stringify(calibrationActions) !== JSON.stringify([
      "calibration-source-acceptance",
      "calibration",
      "generated-constant-freeze",
      "frozen-candidate-acceptance",
      "source-proof",
      "qualification",
      "release",
    ])
    || calibrationSource.next_permitted_mutation !== constantSet
  ) {
    fail(
      "workflow_policy.release_freeze_barrier.acceptance.phases.calibration_source "
      + "must permit only calibration then the generated constant-set freeze before source proof",
    );
  }
  const frozenCandidate = object(
    freezePhases.frozen_candidate,
    "workflow_policy.release_freeze_barrier.acceptance.phases.frozen_candidate",
  );
  const frozenFuture = stringArray(
    frozenCandidate.known_future_source_changes,
    "workflow_policy.release_freeze_barrier.acceptance.phases.frozen_candidate.known_future_source_changes",
  );
  const frozenActions = stringArray(
    frozenCandidate.planned_actions,
    "workflow_policy.release_freeze_barrier.acceptance.phases.frozen_candidate.planned_actions",
    { nonEmpty: true },
  );
  if (
    frozenFuture.length !== 0
    || JSON.stringify(frozenActions) !== JSON.stringify([
      "frozen-candidate-acceptance",
      "source-proof",
      "qualification",
      "release",
    ])
    || frozenCandidate.next_permitted_mutation !== null
  ) {
    fail(
      "workflow_policy.release_freeze_barrier.acceptance.phases.frozen_candidate "
      + "must permit no future source mutation before its sole source proof",
    );
  }
  const actionlint = object(policy.actionlint, "workflow_policy.actionlint");
  if (actionlint.version !== "1.7.12") fail("workflow_policy.actionlint.version must be 1.7.12");
  nonEmptyText(actionlint.config, "workflow_policy.actionlint.config");
  const assets = object(actionlint.assets, "workflow_policy.actionlint.assets");
  const requiredAssets = ["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-arm64", "win32-x64"];
  if (JSON.stringify(Object.keys(assets).sort()) !== JSON.stringify(requiredAssets)) {
    fail(`workflow_policy.actionlint.assets must define exactly ${requiredAssets.join(", ")}`);
  }
  for (const key of requiredAssets) {
    const asset = object(assets[key], `workflow_policy.actionlint.assets.${key}`);
    nonEmptyText(asset.archive, `workflow_policy.actionlint.assets.${key}.archive`);
    if (!SHA256.test(asset.sha256)) fail(`workflow_policy.actionlint.assets.${key}.sha256 must be SHA-256`);
  }
  return graph;
}

export function loadReleaseClaimGraph(repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")) {
  const graphPath = path.join(repoRoot, "release-claims.json");
  let graph;
  try {
    graph = JSON.parse(readFileSync(graphPath, "utf8"));
  } catch (error) {
    fail(`failed to read release claim graph ${graphPath}: ${error.message}`);
  }
  return validateReleaseClaimGraph(graph);
}

const PUBLIC_SUPPORT_START = "<!-- codestory-public-support:start -->";
const PUBLIC_SUPPORT_END = "<!-- codestory-public-support:end -->";
const PUBLIC_SUPPORT_DOCUMENTS = [
  "README.md",
  "CHANGELOG.md",
  "docs/users/README.md",
  "docs/users/codex.md",
  "plugins/codestory/README.md",
];

// Pages that hand-maintain their own platform table instead of embedding the generated block.
// They cannot be regenerated, but every supported platform must at least appear in them: both of
// these silently kept telling Linux x64 readers their platform had no release path.
const PLATFORM_NARRATIVE_DOCUMENTS = [
  "docs/ops/retrieval-engine.md",
  "docs/architecture/retrieval-design.md",
];

export function renderPublicSupport(graph) {
  validateReleaseClaimGraph(graph);
  const rows = graph.public_support.packages.map((row) => {
    const accelerator = {
      metal: "Metal",
      vulkan: "Vulkan",
      none: "CPU",
    }[row.accelerator_claim];
    return `| ${row.label} | Supported with ${accelerator} |`;
  });
  const unsupported = graph.public_support.unsupported.map(
    ({ label }) => `| ${label} | Unsupported |`,
  );
  return [
    PUBLIC_SUPPORT_START,
    "| Platform | Release support |",
    "| --- | --- |",
    ...rows,
    ...unsupported,
    PUBLIC_SUPPORT_END,
  ].join("\n");
}

export function validatePublicSupportDocuments(graph, repoRoot) {
  const expected = renderPublicSupport(graph);
  for (const relative of PUBLIC_SUPPORT_DOCUMENTS) {
    const document = readFileSync(path.join(repoRoot, relative), "utf8");
    const start = document.indexOf(PUBLIC_SUPPORT_START);
    const end = document.indexOf(PUBLIC_SUPPORT_END);
    if (
      start < 0
      || end < start
      || document.indexOf(PUBLIC_SUPPORT_START, start + 1) >= 0
      || document.indexOf(PUBLIC_SUPPORT_END, end + 1) >= 0
    ) {
      fail(`${relative} must contain exactly one generated public support block`);
    }
    const actual = document.slice(start, end + PUBLIC_SUPPORT_END.length);
    if (actual !== expected) {
      fail(`${relative} public support block is stale`);
    }
  }
}

// How each shipped target is named in prose. These pages predate the generated block and word
// platforms their own way, so the check is on identity rather than on the marketing label.
const PLATFORM_NARRATIVE_TOKENS = new Map([
  ["macos-arm64", ["Apple Silicon"]],
  ["windows-x64", ["Windows x64"]],
  ["linux-x64", ["Linux x64"]],
]);

export function validatePlatformNarrativeDocuments(graph, repoRoot) {
  validateReleaseClaimGraph(graph);
  for (const relative of PLATFORM_NARRATIVE_DOCUMENTS) {
    const document = readFileSync(path.join(repoRoot, relative), "utf8");
    for (const { target } of graph.public_support.packages) {
      const tokens = PLATFORM_NARRATIVE_TOKENS.get(target);
      if (!tokens) {
        fail(`${target} ships but has no prose spelling; add one to PLATFORM_NARRATIVE_TOKENS`);
      }
      if (!tokens.some((token) => document.includes(token))) {
        fail(`${relative} does not mention the supported platform ${target}`);
      }
    }
  }
}

export const RELEASE_CLOSEOUT_SUMMARY_ASSET = "release-closeout-summary.json";

export function releaseAssetNames(graph, version) {
  validateReleaseClaimGraph(graph);
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u.test(version)) {
    fail("release asset version must be semver");
  }
  return [
    ...graph.workflow_policy.package_matrix.map(
      ({ asset_target: target, extension }) =>
        `codestory-cli-v${version}-${target}.${extension}`,
    ),
    "SHA256SUMS.txt",
    // The one machine-readable statement of what this release did and did not prove. It ships with
    // the release because the README tells readers to consult the ledger rather than the platform
    // table, and an Actions artifact with a 30-day retention is not something a release consumer
    // can reach.
    RELEASE_CLOSEOUT_SUMMARY_ASSET,
  ];
}

/// Which package target each protected accelerator host speaks for, derived from the cells the host
/// withholds rather than from a second copy of the mapping.
function acceleratorHostByTarget(graph) {
  const byTarget = new Map();
  for (const host of graph.non_claim_policy.hosts) {
    for (const cellId of host.withheld_cells) {
      const [group, instance] = cellId.split(":");
      if (group === "candidate_installed_behavior") byTarget.set(instance, host);
    }
  }
  return byTarget;
}

/// The platform table that goes into the published GitHub release notes.
///
/// This is the surface a release consumer actually reads, so it is rendered from the accepted
/// closeout ledger, never from the static graph alone. Rendering it from the graph is how a release
/// whose Vulkan proof was withheld still announced "supported with Vulkan": the graph says what the
/// repository intends to support, and only the ledger says what this release proved. A withheld
/// accelerator is stated in the notes, in the same words the ledger recorded it in.
export function renderReleasePlatformNotes(graph, ledger) {
  validateReleaseClaimGraph(graph);
  const closeout = object(ledger, "closeout ledger");
  const withheldCells = new Set(stringArray(closeout.withheld_cells, "closeout ledger.withheld_cells"));
  const hostByTarget = acceleratorHostByTarget(graph);
  const reason = graph.non_claim_policy.reason;
  const packages = graph.public_support.packages.map(({ label, target, accelerator_claim: claim }) => {
    const accelerator = claim === "metal" ? "Metal" : "Vulkan";
    const host = hostByTarget.get(target);
    const withheld = host !== undefined
      && host.withheld_cells.some((cellId) =>
        cellId.startsWith("accelerator_execution:") && withheldCells.has(cellId));
    return withheld
      ? `- ${label}: ${accelerator} not proven for this release (${reason})`
      : `- ${label}: supported with ${accelerator}`;
  });
  const unsupported = graph.public_support.unsupported.map(
    ({ label }) => `- ${label}: unsupported`,
  );
  const withheldNote = withheldCells.size === 0
    ? []
    : [
      "",
      `This release withheld ${withheldCells.size} evidence cell(s); `
      + "release-closeout-summary.json names every one of them.",
    ];
  return [
    "## Platform support",
    "",
    ...packages,
    ...unsupported,
    ...withheldNote,
  ].join("\n");
}

function sortedFailures(failures) {
  return failures.sort((left, right) =>
    (FAILURE_ORDER.get(left.class) ?? 99) - (FAILURE_ORDER.get(right.class) ?? 99)
      || left.claim.localeCompare(right.claim)
      || String(left.evidence ?? "").localeCompare(String(right.evidence ?? ""))
      || left.message.localeCompare(right.message));
}

function addFailure(failures, failureClass, claim, evidence, message) {
  failures.push({
    class: failureClass,
    claim,
    ...(evidence ? { evidence } : {}),
    message,
  });
}

function relativeChangePercent(baseline, measured, direction) {
  if (!(baseline > 0) || !(measured > 0)) return null;
  if (direction === "max") return ((measured - baseline) / baseline) * 100;
  if (direction === "min") return ((baseline - measured) / baseline) * 100;
  return null;
}

function exceptionProblems(
  exception,
  trustedException,
  trustedIdentity,
  expectedCommit,
  evaluatedAt,
  evidenceId,
  evidenceRow,
  evidenceByType,
  requestedClaimIds,
  policy,
) {
  const problems = [];
  if (exception === null || typeof exception !== "object" || Array.isArray(exception)) {
    return [`${evidenceId} pass_with_exception requires structured exception evidence`];
  }
  if (exception.schema !== "codestory.release-claim-exception/v1") {
    problems.push(`${evidenceId} exception uses an unsupported schema`);
  }
  if (evidenceRow.type !== policy.eligible_evidence_type) {
    problems.push(`${evidenceId} is not eligible for a model microbenchmark exception`);
  }
  if (!requestedClaimIds.has(policy.full_product_benefit_evidence_type)) {
    problems.push(
      `${evidenceId} exception requires the ${policy.full_product_benefit_evidence_type} claim in the same evaluation`,
    );
  }
  if (!Array.isArray(exception.approvals) || exception.approvals.length === 0) {
    problems.push(`${evidenceId} exception must contain at least one approval`);
  } else {
    for (const [index, value] of exception.approvals.entries()) {
      const label = `${evidenceId} exception approval ${index}`;
      if (value === null || typeof value !== "object" || Array.isArray(value)) {
        problems.push(`${label} must be an object`);
        continue;
      }
      for (const key of [
        "profile",
        "baseline_id",
        "metric",
        "owner",
        "rationale",
        "rollback_evidence",
        "release_key",
        "regression_class",
      ]) {
        if (typeof value[key] !== "string" || value[key].trim() === "") {
          problems.push(`${label} must bind non-empty ${key}`);
        }
      }
      for (const key of [policy.artifact_binding, "baseline_sha256"]) {
        if (!SHA256.test(String(value[key] ?? ""))) problems.push(`${label} ${key} must be SHA-256`);
      }
      if (value.commit !== expectedCommit) problems.push(`${label} commit does not match ${expectedCommit}`);
      for (const key of [
        "profile",
        "baseline_id",
        "baseline_sha256",
        policy.artifact_binding,
        policy.release_binding,
      ]) {
        if (value[key] !== trustedIdentity[key]) {
          problems.push(`${label} ${key} does not match the evidence identity`);
        }
      }
      if (value.regression_class !== policy.regression_class) {
        problems.push(`${label} regression_class must be ${policy.regression_class}`);
      }
      if (policy.non_waivable_metrics.includes(value.metric)) {
        problems.push(`${label} metric ${value.metric} is non-waivable`);
      }
      for (const key of ["baseline_value", "measured_value", "threshold", "regression_percent"]) {
        if (typeof value[key] !== "number" || !Number.isFinite(value[key]) || value[key] <= 0) {
          problems.push(`${label} ${key} must be finite and positive`);
        }
      }
      if (!new Set(["max", "min"]).has(value.direction)) {
        problems.push(`${label} direction must be max or min`);
      }
      const regressionPercent = relativeChangePercent(
        value.baseline_value,
        value.measured_value,
        value.direction,
      );
      if (regressionPercent === null
          || regressionPercent <= policy.minimum_regression_percent
          || Math.abs(regressionPercent - value.regression_percent) > 1e-9) {
        problems.push(
          `${label} must bind a repeatable model regression over ${policy.minimum_regression_percent} percent`,
        );
      }
      const breachesThreshold = value.direction === "max"
        ? value.measured_value > value.threshold
        : value.measured_value < value.threshold;
      if (!breachesThreshold) problems.push(`${label} measured value does not breach its threshold`);
      if (!Number.isInteger(value.repeats) || value.repeats < policy.minimum_repeats) {
        problems.push(`${label} repeats must be at least ${policy.minimum_repeats}`);
      }
      if (!validIsoDate(value.approved_at) || !validIsoDate(value.expires_at)) {
        problems.push(`${label} approval and expiry must be valid ISO dates`);
      } else {
        const evaluatedDate = new Date(evaluatedAt).toISOString().slice(0, 10);
        const approvedAt = Date.parse(`${value.approved_at}T00:00:00.000Z`);
        const expiresAt = Date.parse(`${value.expires_at}T00:00:00.000Z`);
        const maximumValidityMs = policy.maximum_validity_days * 24 * 60 * 60 * 1000;
        if (value.approved_at > evaluatedDate
            || value.expires_at < value.approved_at
            || value.expires_at < evaluatedDate) {
          problems.push(`${label} is future-dated, expired, or expires before approval`);
        }
        if (expiresAt - approvedAt > maximumValidityMs) {
          problems.push(`${label} expires more than ${policy.maximum_validity_days} days after approval`);
        }
      }

      const benefit = value.full_product_benefit;
      if (benefit === null || typeof benefit !== "object" || Array.isArray(benefit)) {
        problems.push(`${label} must bind structured same-run full_product_benefit evidence`);
      } else {
        for (const key of ["evidence_id", "metric", "direction", "observed_at"]) {
          if (typeof benefit[key] !== "string" || benefit[key].trim() === "") {
            problems.push(`${label} full_product_benefit must bind non-empty ${key}`);
          }
        }
        if (!SHA256.test(String(benefit.artifact_sha256 ?? ""))) {
          problems.push(`${label} full_product_benefit artifact_sha256 must be SHA-256`);
        }
        for (const key of ["baseline_value", "measured_value", "improvement_percent"]) {
          if (typeof benefit[key] !== "number" || !Number.isFinite(benefit[key]) || benefit[key] <= 0) {
            problems.push(`${label} full_product_benefit ${key} must be finite and positive`);
          }
        }
        const improvementPercent = benefit.direction === "increase"
          ? ((benefit.measured_value - benefit.baseline_value) / benefit.baseline_value) * 100
          : benefit.direction === "decrease"
            ? ((benefit.baseline_value - benefit.measured_value) / benefit.baseline_value) * 100
            : null;
        if (!new Set(["increase", "decrease"]).has(benefit.direction)
            || improvementPercent === null
            || improvementPercent <= 0
            || Math.abs(improvementPercent - benefit.improvement_percent) > 1e-9) {
          problems.push(`${label} full_product_benefit must bind a positive measured improvement`);
        }

        const benefitRows = (evidenceByType.get(policy.full_product_benefit_evidence_type) ?? [])
          .filter((row) => String(row.id ?? `${row.type}[${row._index}]`) === benefit.evidence_id);
        if (benefitRows.length !== 1) {
          problems.push(
            `${label} full_product_benefit must identify exactly one ${policy.full_product_benefit_evidence_type} row`,
          );
        } else {
          const benefitRow = benefitRows[0];
          const benefitIdentity = benefitRow.identity ?? {};
          const regressionIdentity = evidenceRow.identity ?? {};
          if (benefitRow.status !== "pass") {
            problems.push(`${label} full_product_benefit evidence must pass without exception`);
          }
          if (benefit.observed_at !== evidenceRow.observed_at
              || benefitRow.observed_at !== evidenceRow.observed_at) {
            problems.push(`${label} full_product_benefit evidence is not from the same run`);
          }
          if (benefit.artifact_sha256 !== benefitIdentity.artifact_sha256) {
            problems.push(`${label} full_product_benefit artifact does not match its evidence row`);
          }
          for (const key of [
            "repository",
            "commit",
            "source_tree",
            "profile",
            "corpus_id",
            "cache_id",
            "machine_fingerprint",
            policy.release_binding,
          ]) {
            if (benefitIdentity[key] !== regressionIdentity[key]) {
              problems.push(`${label} full_product_benefit ${key} does not match the regression run`);
            }
          }
        }
      }
    }
  }
  if (trustedException === undefined) {
    problems.push(`${evidenceId} exception is not present in separately trusted inputs`);
  } else if (JSON.stringify(canonicalReleaseClaimValue(exception))
      !== JSON.stringify(canonicalReleaseClaimValue(trustedException))) {
    problems.push(`${evidenceId} exception does not match separately trusted approval evidence`);
  }
  return problems;
}

export function evaluateReleaseClaims({ graph, requested_claims: requestedClaims, evidence, expected }) {
  validateReleaseClaimGraph(graph);
  if (!Array.isArray(requestedClaims) || requestedClaims.length === 0) fail("requested_claims must be a non-empty array");
  if (!Array.isArray(evidence)) fail("evidence must be an array");
  object(expected, "expected");
  const expectedCommit = nonEmptyText(expected.commit, "expected.commit").toLowerCase();
  if (!FULL_SHA.test(expectedCommit)) fail("expected.commit must be a full lowercase Git SHA");
  const expectedIdentity = object(expected.identity ?? {}, "expected.identity");
  if (expectedIdentity.commit !== undefined && expectedIdentity.commit !== expectedCommit) {
    fail("expected.identity.commit conflicts with expected.commit");
  }
  for (const key of ["repository", "source_tree"]) {
    nonEmptyText(expectedIdentity[key], `expected.identity.${key}`);
  }
  if (!FULL_SHA.test(expectedIdentity.source_tree)) {
    fail("expected.identity.source_tree must be a full lowercase Git tree SHA");
  }
  const expectedExceptions = object(expected.exceptions ?? {}, "expected.exceptions");
  const evaluatedAtText = nonEmptyText(expected.evaluated_at ?? new Date().toISOString(), "expected.evaluated_at");
  const evaluatedAt = Date.parse(evaluatedAtText);
  if (!Number.isFinite(evaluatedAt) || new Date(evaluatedAt).toISOString() !== evaluatedAtText) {
    fail("expected.evaluated_at must be a canonical ISO timestamp");
  }
  const graphDigest = releaseClaimGraphDigest(graph);
  const tiers = new Map(graph.proof_tiers.map((tier) => [tier.id, tier]));
  const evidenceTypes = new Map(graph.evidence_types.map((type) => [type.id, type]));
  const claims = new Map(graph.claims.map((claim) => [claim.id, claim]));
  const evidenceByType = new Map();
  for (const [index, rowValue] of evidence.entries()) {
    const row = object(rowValue, `evidence[${index}]`);
    const type = nonEmptyText(row.type, `evidence[${index}].type`);
    const rows = evidenceByType.get(type) ?? [];
    rows.push({ ...row, _index: index });
    evidenceByType.set(type, rows);
  }
  for (const rows of evidenceByType.values()) {
    rows.sort((left, right) => String(left.id ?? left._index).localeCompare(String(right.id ?? right._index)));
  }

  const requests = new Map();
  for (const [index, requestValue] of requestedClaims.entries()) {
    const request = typeof requestValue === "string" ? { id: requestValue, accepted_risks: [] } : object(requestValue, `requested_claims[${index}]`);
    const claimId = nonEmptyText(request.id, `requested_claims[${index}].id`);
    if (requests.has(claimId)) fail(`requested_claims contains duplicate ${claimId}`);
    requests.set(claimId, {
      id: claimId,
      accepted_risks: stringArray(request.accepted_risks ?? [], `requested_claims[${index}].accepted_risks`),
    });
  }

  const orderedClaims = [];
  const scheduled = new Set();
  const schedule = (claimId) => {
    const claim = claims.get(claimId);
    if (!claim || scheduled.has(claimId)) return;
    scheduled.add(claimId);
    for (const dependency of claim.depends_on_claims) schedule(dependency);
    orderedClaims.push(claimId);
  };
  for (const claimId of requests.keys()) schedule(claimId);

  const failures = [];
  const results = [];
  for (const claimId of requests.keys()) {
    if (!claims.has(claimId)) {
      addFailure(failures, "unsupported_claim", claimId, null, `claim ${claimId} is not declared by ${graph.schema}`);
      results.push({ id: claimId, status: "fail", evidence: [] });
    }
  }
  for (const claimId of orderedClaims) {
    const claim = claims.get(claimId);
    const explicitlyRequested = requests.has(claimId);
    const request = requests.get(claimId) ?? { id: claimId, accepted_risks: [] };
    const acceptedRisks = new Set(request.accepted_risks);
    if (!explicitlyRequested && claim.accepted_risks.length > 0) {
      addFailure(
        failures,
        "accepted_risk",
        claimId,
        null,
        `risk-bearing dependency ${claimId} must be explicitly requested with its own accepted_risks`,
      );
    } else {
      for (const risk of claim.accepted_risks) {
        if (!acceptedRisks.has(risk)) {
          addFailure(failures, "accepted_risk", claimId, null, `claim ${claimId} requires explicit acceptance of ${risk}`);
        }
      }
    }
    for (const unknownRisk of acceptedRisks) {
      if (!claim.accepted_risks.includes(unknownRisk)) {
        addFailure(failures, "accepted_risk", claimId, null, `claim ${claimId} does not declare accepted risk ${unknownRisk}`);
      }
    }

    const requirementResults = [];
    for (const dependency of claim.depends_on_claims) {
      const dependencyResult = results.find((result) => result.id === dependency);
      if (!new Set(["pass", "pass_with_exception"]).has(dependencyResult?.status)) {
        addFailure(failures, "failed_evidence", claimId, `claim:${dependency}`, `claim ${claimId} dependency ${dependency} did not pass`);
      }
    }
    for (const requirement of claim.required_evidence) {
      const definition = evidenceTypes.get(requirement);
      const trustedIdentity = { ...expectedIdentity, commit: expectedCommit };
      for (const [key, value] of Object.entries(definition.identity_constraints ?? {})) {
        if (expectedIdentity[key] !== undefined && expectedIdentity[key] !== value) {
          fail(`expected.identity.${key} conflicts with the release claim graph`);
        }
        trustedIdentity[key] = value;
      }
      const rows = evidenceByType.get(requirement) ?? [];
      if (rows.length === 0) {
        addFailure(failures, "missing", claimId, requirement, `claim ${claimId} is missing ${requirement} evidence`);
        requirementResults.push({ type: requirement, status: "missing" });
        continue;
      }
      let allPassing = true;
      let hasException = false;
      const requirementExceptions = [];
      for (const row of rows) {
        const evidenceId = String(row.id ?? `${requirement}[${row._index}]`);
        const before = failures.length;
        let boundException = null;
        if (row.graph_sha256 !== graphDigest) {
          addFailure(failures, "stale_evidence", claimId, evidenceId, `${evidenceId} is bound to a stale release claim graph`);
        }
        const identity = object(row.identity ?? {}, `${evidenceId}.identity`);
        if (identity.commit !== expectedCommit) {
          addFailure(failures, "stale_sha", claimId, evidenceId, `${evidenceId} commit does not match ${expectedCommit}`);
        }
        if (!FULL_SHA.test(String(identity.source_tree ?? "")) || identity.source_tree !== expectedIdentity.source_tree) {
          addFailure(failures, "stale_sha", claimId, evidenceId, `${evidenceId} source tree does not match the requested release`);
        }
        const observedAt = Date.parse(String(row.observed_at ?? ""));
        const expiresAt = Date.parse(String(row.expires_at ?? ""));
        const canonicalValidity = Number.isFinite(observedAt)
          && Number.isFinite(expiresAt)
          && new Date(observedAt).toISOString() === row.observed_at
          && new Date(expiresAt).toISOString() === row.expires_at;
        const maximumValidityMs = definition.maximum_validity_hours * 60 * 60 * 1000;
        if (!canonicalValidity || observedAt > evaluatedAt || expiresAt <= evaluatedAt || expiresAt <= observedAt || expiresAt - observedAt > maximumValidityMs) {
          addFailure(failures, "stale_evidence", claimId, evidenceId, `${evidenceId} is expired or has invalid validity bounds`);
        }
        const actualTier = tiers.get(row.tier);
        const definitionTier = tiers.get(definition.tier);
        const minimumTier = tiers.get(claim.minimum_tier);
        if (!actualTier || row.tier !== definition.tier || actualTier.rank < minimumTier.rank || definitionTier.rank < minimumTier.rank) {
          addFailure(failures, "incompatible_tier_identity", claimId, evidenceId, `${evidenceId} tier ${String(row.tier)} cannot satisfy ${claim.minimum_tier}`);
        }
        for (const key of definition.required_identity) {
          const format = graph.evidence_policy.identity_formats[key];
          if (!identityMatchesFormat(trustedIdentity[key], format)) {
            addFailure(failures, "incompatible_tier_identity", claimId, evidenceId, `${evidenceId} has no trusted ${key} identity matching ${format}`);
          } else if (!identityMatchesFormat(identity[key], format)) {
            addFailure(failures, "incompatible_tier_identity", claimId, evidenceId, `${evidenceId} identity ${key} does not match ${format}`);
          } else if (identity[key] !== trustedIdentity[key]) {
            addFailure(failures, "incompatible_tier_identity", claimId, evidenceId, `${evidenceId} identity ${key} does not match the requested release`);
          }
        }
        if (row.status === "pass_with_exception") {
          hasException = true;
          const problems = exceptionProblems(
            row.exception,
            expectedExceptions[evidenceId],
            trustedIdentity,
            expectedCommit,
            evaluatedAt,
            evidenceId,
            row,
            evidenceByType,
            new Set(requests.keys()),
            graph.exception_policy,
          );
          for (const problem of problems) {
            addFailure(failures, "failed_evidence", claimId, evidenceId, problem);
          }
          if (problems.length === 0) {
            boundException = { evidence: evidenceId, ...structuredClone(row.exception) };
          }
        } else if (row.status !== "pass") {
          addFailure(failures, "failed_evidence", claimId, evidenceId, `${evidenceId} status is ${String(row.status)}`);
        }
        if (failures.length !== before) {
          allPassing = false;
        } else if (boundException) {
          requirementExceptions.push(boundException);
        }
      }
      const requirementStatus = !allPassing
        ? "fail"
        : hasException ? "pass_with_exception" : "pass";
      requirementResults.push({
        type: requirement,
        status: requirementStatus,
        ...(requirementExceptions.length > 0 ? { exceptions: requirementExceptions } : {}),
      });
    }
    const claimFailures = failures.filter((failure) => failure.claim === claimId);
    const carriesException = requirementResults.some(({ status }) => status === "pass_with_exception")
      || claim.depends_on_claims.some((dependency) => results.find(({ id }) => id === dependency)?.status === "pass_with_exception");
    const claimStatus = claimFailures.length > 0
      ? "fail"
      : carriesException ? "pass_with_exception" : "pass";
    const directExceptions = requirementResults.flatMap(({ exceptions = [] }) => exceptions);
    const inheritedExceptions = claim.depends_on_claims.flatMap((dependency) => {
      const dependencyResult = results.find(({ id }) => id === dependency);
      return (dependencyResult?.exceptions ?? []).map((exception) => ({
        ...structuredClone(exception),
        inherited_from_claim: dependency,
      }));
    });
    const claimExceptions = [...directExceptions, ...inheritedExceptions];
    results.push({
      id: claimId,
      minimum_tier: claim.minimum_tier,
      status: claimStatus,
      evidence: requirementResults,
      accepted_risks: [...acceptedRisks].sort(),
      non_claims: [...claim.non_claims],
      ...(claimExceptions.length > 0 ? { exceptions: claimExceptions } : {}),
    });
  }
  sortedFailures(failures);
  results.sort((left, right) => left.id.localeCompare(right.id));
  const evaluationStatus = failures.length > 0
    ? "fail"
    : results.some(({ status }) => status === "pass_with_exception") ? "pass_with_exception" : "pass";
  return {
    schema: "codestory.release-claim-evaluation/v1",
    status: evaluationStatus,
    graph_schema: graph.schema,
    graph_sha256: graphDigest,
    evidence_selection: graph.evidence_policy.selection,
    expected_commit: expectedCommit,
    evaluated_at: evaluatedAtText,
    claims: results,
    failures,
  };
}

function parseArgs(argv) {
  const command = argv.shift();
  const values = {};
  while (argv.length > 0) {
    const key = argv.shift();
    const value = argv.shift();
    if (!key?.startsWith("--") || value === undefined) fail("arguments must be --key value pairs");
    values[key.slice(2)] = value;
  }
  return { command, values };
}

function main() {
  const { command, values } = parseArgs(process.argv.slice(2));
  const repoRoot = path.resolve(values.repo ?? path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."));
  const graph = loadReleaseClaimGraph(repoRoot);
  if (command === "validate") {
    validatePublicSupportDocuments(graph, repoRoot);
    validatePlatformNarrativeDocuments(graph, repoRoot);
    console.log(`Release claim graph passed: ${releaseClaimGraphDigest(graph)}`);
    return;
  }
  if (command === "release-assets") {
    console.log(releaseAssetNames(graph, nonEmptyText(values.version, "--version")).join("\n"));
    return;
  }
  if (command === "release-platform-notes") {
    // The ledger is required, not optional: an optional ledger would mean the published notes can
    // still be produced from the graph alone, which is the exact fail-open this command had.
    const ledgerPath = nonEmptyText(values.ledger, "--ledger");
    console.log(renderReleasePlatformNotes(graph, JSON.parse(readFileSync(ledgerPath, "utf8"))));
    return;
  }
  if (command === "evaluate") {
    const document = JSON.parse(readFileSync(nonEmptyText(values.evidence, "--evidence"), "utf8"));
    const gitIdentity = deriveTrustedGitIdentity({ repoRoot, expectedSha: values["expected-sha"] });
    const suppliedIdentity = values["expected-identity"]
      ? object(JSON.parse(readFileSync(values["expected-identity"], "utf8")), "--expected-identity")
      : {};
    for (const key of ["repository", "commit", "source_tree"]) {
      if (suppliedIdentity[key] !== undefined && suppliedIdentity[key] !== gitIdentity[key]) {
        fail(`--expected-identity ${key} conflicts with Git identity derived from --repo`);
      }
    }
    const suppliedExceptions = values["expected-exceptions"]
      ? object(JSON.parse(readFileSync(values["expected-exceptions"], "utf8")), "--expected-exceptions")
      : {};
    const evaluation = evaluateReleaseClaims({
      graph,
      requested_claims: document.requested_claims,
      evidence: document.evidence,
      expected: {
        commit: gitIdentity.commit,
        evaluated_at: values["evaluated-at"],
        identity: { ...suppliedIdentity, ...gitIdentity },
        exceptions: suppliedExceptions,
      },
    });
    console.log(JSON.stringify(evaluation, null, 2));
    if (evaluation.status === "fail") process.exitCode = 1;
    return;
  }
  fail("command must be validate, release-assets, release-platform-notes, or evaluate");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main();
}
