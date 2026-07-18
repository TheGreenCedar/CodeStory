#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const GRAPH_SCHEMA = "codestory.release-claims/v1";
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
const REQUIRED_FAILURE_CONTROLS = [
  "benchmark_leakage",
  "observational_read_mutation",
  "project_identity_drift",
  "sidecar_runtime_mismatch",
  "stale_or_partial_publication",
];
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
  if (graph.schema !== GRAPH_SCHEMA || graph.graph_version !== 3) {
    fail(`release claim graph must use ${GRAPH_SCHEMA} graph_version 3`);
  }
  nonEmptyText(graph.graph_id, "release claim graph.graph_id");
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
    if (!command.startsWith("cargo test --locked ")) fail(`failure control ${id} must name a locked executable Cargo test`);
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
    "answer_quality",
    "installed_runtime_behavior",
    "package_identity",
    "performance",
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

  const policy = object(graph.workflow_policy, "release claim graph.workflow_policy");
  if (!Number.isInteger(policy.artifact_retention_days) || policy.artifact_retention_days <= 0) {
    fail("workflow_policy.artifact_retention_days must be a positive integer");
  }
  if (!Array.isArray(policy.package_matrix) || policy.package_matrix.length !== 6) {
    fail("workflow_policy.package_matrix must define six native package rows");
  }
  const targets = new Set();
  for (const [index, rowValue] of policy.package_matrix.entries()) {
    const row = object(rowValue, `workflow_policy.package_matrix[${index}]`);
    for (const key of ["os", "rust_target", "asset_target", "extension"]) {
      nonEmptyText(row[key], `workflow_policy.package_matrix[${index}].${key}`);
    }
    if (typeof row.exe_suffix !== "string") fail(`workflow_policy.package_matrix[${index}].exe_suffix must be a string`);
    if (targets.has(row.asset_target)) fail(`workflow_policy.package_matrix duplicates ${row.asset_target}`);
    targets.add(row.asset_target);
  }
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
  nonEmptyText(releaseChain.evidence_workflow, "workflow_policy.release_chain.evidence_workflow");
  nonEmptyText(releaseChain.evidence_profile, "workflow_policy.release_chain.evidence_profile");
  nonEmptyText(releaseChain.drill_manifest, "workflow_policy.release_chain.drill_manifest");
  stringArray(releaseChain.exact_sha_jobs, "workflow_policy.release_chain.exact_sha_jobs", { nonEmpty: true });
  const dependencies = object(releaseChain.dependencies, "workflow_policy.release_chain.dependencies");
  for (const [job, needsValue] of Object.entries(dependencies)) {
    nonEmptyText(job, "workflow_policy.release_chain.dependencies job");
    stringArray(needsValue, `workflow_policy.release_chain.dependencies.${job}`, { nonEmpty: true });
  }
  stringArray(policy.artifact_workflows, "workflow_policy.artifact_workflows", { nonEmpty: true });
  const promotion = object(policy.promotion, "workflow_policy.promotion");
  nonEmptyText(promotion.source_branch, "workflow_policy.promotion.source_branch");
  nonEmptyText(promotion.release_branch, "workflow_policy.promotion.release_branch");
  nonEmptyText(promotion.exact_sha_expression, "workflow_policy.promotion.exact_sha_expression");
  nonEmptyText(promotion.proof_run_sha_expression, "workflow_policy.promotion.proof_run_sha_expression");
  nonEmptyText(promotion.manual_pr_ref_hint, "workflow_policy.promotion.manual_pr_ref_hint");
  nonEmptyText(promotion.source_cache_namespace, "workflow_policy.promotion.source_cache_namespace");
  nonEmptyText(promotion.macos_source_cache_namespace, "workflow_policy.promotion.macos_source_cache_namespace");
  nonEmptyText(promotion.packaged_cache_namespace, "workflow_policy.promotion.packaged_cache_namespace");
  stringArray(promotion.label_routed_workflows, "workflow_policy.promotion.label_routed_workflows", { nonEmpty: true });
  stringArray(promotion.required_events, "workflow_policy.promotion.required_events", { nonEmpty: true });

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
    console.log(`Release claim graph passed: ${releaseClaimGraphDigest(graph)}`);
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
  fail("command must be validate or evaluate");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main();
}
