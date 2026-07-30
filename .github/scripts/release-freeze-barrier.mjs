#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import process from "node:process";

const ACTIVE_RUN_STATES = new Set([
  "queued",
  "waiting",
  "requested",
  "pending",
  "in_progress",
]);
const ALLOWED_FUTURE_CHANGES = new Set([
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
]);
const STATUS_PREFIX = "codestory/release-freeze";

function fail(message) {
  throw new Error(message);
}

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  }).trim();
}

function git(args, repo) {
  return run("git", ["-C", repo, ...args]);
}

function gh(args) {
  return run("gh", args);
}

function values(args, name) {
  const result = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name) {
      const value = args[index + 1];
      if (!value || value.startsWith("--")) {
        fail(`${name} requires a value`);
      }
      result.push(value);
      index += 1;
    }
  }
  return result;
}

function value(args, name, fallback = undefined) {
  const found = values(args, name);
  if (found.length > 1) {
    fail(`${name} may be specified only once`);
  }
  return found[0] ?? fallback;
}

function required(args, name) {
  const result = value(args, name);
  if (!result) {
    fail(`${name} is required`);
  }
  return result;
}

function has(args, name) {
  return args.includes(name);
}

function parseJsonFile(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(`${label} is not valid JSON: ${error.message}`);
  }
}

function stable(value) {
  if (Array.isArray(value)) {
    return value.map(stable);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, stable(item)]),
    );
  }
  return value;
}

export function receiptDigest(receipt) {
  const withoutDigest = { ...receipt };
  delete withoutDigest.digest;
  return createHash("sha256")
    .update(`${JSON.stringify(stable(withoutDigest))}\n`)
    .digest("hex");
}

export function validateMutationReceipt(receipt, { commit, tree, requiredIds }) {
  if (receipt?.commit !== commit || receipt?.tree !== tree) {
    fail("hostile mutation evidence must name the exact frozen commit and tree");
  }
  if (!Array.isArray(receipt.cases)) {
    fail("hostile mutation evidence must contain cases");
  }
  const cases = new Map(receipt.cases.map((entry) => [entry?.id, entry]));
  for (const id of requiredIds) {
    const entry = cases.get(id);
    if (!entry || entry.status !== "passed") {
      fail(`hostile mutation ${id} did not pass on the exact frozen head`);
    }
  }
}

export function validatePlatformEvidence(evidence, { commit, tree }) {
  const failures = evidence?.failures ?? [];
  const probes = evidence?.probes ?? [];
  if (!Array.isArray(failures) || !Array.isArray(probes)) {
    fail("platform evidence must contain failure and probe arrays");
  }
  for (const failure of failures) {
    const probe = probes.find((candidate) =>
      candidate?.failure_run_id === failure?.run_id
      && candidate?.platform === failure?.platform
      && candidate?.commit === commit
      && candidate?.tree === tree
      && candidate?.status === "passed"
      && Number.isFinite(candidate?.duration_seconds)
      && candidate.duration_seconds < 90
      && typeof candidate?.mutation === "string"
      && candidate.mutation.length > 0
    );
    if (!probe) {
      fail(
        `platform failure ${failure?.run_id ?? "<unknown>"} lacks an exact-head native probe under 90 seconds`,
      );
    }
  }
}

export function validateReceipt(receipt, { commit, tree, requiredMutationIds = [] }) {
  if (receipt?.schema !== 1) {
    fail("freeze receipt schema must be 1");
  }
  if (receipt.commit !== commit || receipt.tree !== tree) {
    fail("freeze receipt does not match the exact commit and tree");
  }
  if (receipt.worktree_clean !== true || receipt.remote_head !== commit) {
    fail("freeze receipt must prove a clean worktree pushed at the exact commit");
  }
  if (
    receipt?.release_pr?.head_commit !== commit
    || receipt?.release_pr?.head !== receipt.branch
    || receipt?.release_pr?.base !== "dev/codestory-next"
  ) {
    fail("freeze receipt must bind the open release PR at this exact head");
  }
  if (!Array.isArray(receipt.known_future_source_changes)) {
    fail("freeze receipt must declare known future source changes");
  }
  for (const path of receipt.known_future_source_changes) {
    if (!ALLOWED_FUTURE_CHANGES.has(path)) {
      fail(`freeze receipt admits an unsupported future source change: ${path}`);
    }
  }
  for (const field of [
    "planned_proof_actions",
    "reusable_evidence",
    "invalidated_evidence",
    "running_workflows",
  ]) {
    if (!Array.isArray(receipt[field])) {
      fail(`freeze receipt must contain ${field}`);
    }
  }
  if (typeof receipt.next_permitted_mutation !== "string"
      || receipt.next_permitted_mutation.length === 0) {
    fail("freeze receipt must name the next permitted mutation");
  }
  validateMutationReceipt(receipt.hostile_mutations, {
    commit,
    tree,
    requiredIds: requiredMutationIds,
  });
  validatePlatformEvidence(receipt.platform_evidence, { commit, tree });
  if (receipt.digest !== receiptDigest(receipt)) {
    fail("freeze receipt digest does not match its contents");
  }
}

function currentRuns(repository) {
  const raw = gh([
    "run",
    "list",
    "--repo",
    repository,
    "--limit",
    "100",
    "--json",
    "databaseId,workflowName,headSha,headBranch,status,event,url",
  ]);
  const parsed = JSON.parse(raw || "[]");
  return parsed.filter((entry) => ACTIVE_RUN_STATES.has(entry.status));
}

function cancelSupersededRuns({ repository, commit, workflows, runs }) {
  const allowlist = new Set(workflows);
  const cancelled = [];
  for (const entry of runs) {
    if (!allowlist.has(entry.workflowName) || entry.headSha === commit) {
      continue;
    }
    gh(["run", "cancel", String(entry.databaseId), "--repo", repository]);
    cancelled.push({
      database_id: entry.databaseId,
      head_sha: entry.headSha,
      workflow: entry.workflowName,
    });
  }
  return cancelled;
}

function cancelSuperseded(args) {
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const workflows = values(args, "--broad-workflow");
  if (workflows.length === 0) {
    fail("--broad-workflow is required");
  }
  const before = currentRuns(repository);
  const duplicate = before.find((entry) =>
    workflows.includes(entry.workflowName)
    && entry.headSha === commit
    && String(entry.databaseId) !== String(process.env.GITHUB_RUN_ID ?? "")
  );
  if (duplicate) {
    fail(
      `unchanged head ${commit} already has active ${duplicate.workflowName} run ${duplicate.databaseId}`,
    );
  }
  const cancelled = cancelSupersededRuns({
    repository,
    commit,
    workflows,
    runs: before,
  });
  const cancelledIds = new Set(cancelled.map((entry) => String(entry.database_id)));
  const remaining = currentRuns(repository).filter((entry) =>
    workflows.includes(entry.workflowName)
      && entry.headSha !== commit
      && !cancelledIds.has(String(entry.databaseId))
  );
  if (remaining.length > 0) {
    fail("superseded broad proof remains queued or running after cancellation");
  }
  process.stdout.write(`${JSON.stringify({ cancelled })}\n`);
}

function supportPr(repository, number, commit, repo) {
  const pr = JSON.parse(gh([
    "pr",
    "view",
    String(number),
    "--repo",
    repository,
    "--json",
    "number,state,mergedAt,mergeCommit,baseRefName,headRefName",
  ]));
  const mergeCommit = pr?.mergeCommit?.oid;
  if (pr.state !== "MERGED" || !pr.mergedAt || !mergeCommit) {
    fail(`support PR #${number} is not merged`);
  }
  try {
    git(["merge-base", "--is-ancestor", mergeCommit, commit], repo);
  } catch {
    fail(`support PR #${number} merge ${mergeCommit} is not integrated into ${commit}`);
  }
  return {
    number: pr.number,
    merge_commit: mergeCommit,
    base: pr.baseRefName,
    head: pr.headRefName,
  };
}

function releasePr(repository, number, { branch, commit }) {
  const pr = JSON.parse(gh([
    "pr",
    "view",
    String(number),
    "--repo",
    repository,
    "--json",
    "number,state,baseRefName,headRefName,headRefOid,headRepository",
  ]));
  if (
    pr.state !== "OPEN"
    || pr.baseRefName !== "dev/codestory-next"
    || pr.headRefName !== branch
    || pr.headRefOid !== commit
    || pr?.headRepository?.nameWithOwner !== repository
  ) {
    fail(
      `release PR #${number} must be an open same-repository ${branch} -> `
      + `dev/codestory-next PR at exact head ${commit}`,
    );
  }
  return {
    number: pr.number,
    base: pr.baseRefName,
    head: pr.headRefName,
    head_commit: pr.headRefOid,
  };
}

function declare(args) {
  const repo = value(args, "--repo", process.cwd());
  const repository = required(args, "--repository");
  const branch = value(args, "--branch", git(["branch", "--show-current"], repo));
  const output = required(args, "--output");
  const releasePrNumber = required(args, "--release-pr");
  const mutationPath = required(args, "--mutation-receipt");
  const platformPath = required(args, "--platform-evidence");
  const requiredMutationIds = values(args, "--required-mutation");
  const supportPrNumbers = values(args, "--support-pr");
  const knownFutureChanges = values(args, "--known-future-change");
  const plannedProofActions = values(args, "--planned-proof-action");
  const reusableEvidence = values(args, "--reusable-evidence");
  const invalidatedEvidence = values(args, "--invalidated-evidence");
  const nextMutation = required(args, "--next-permitted-mutation");
  const broadWorkflows = values(args, "--broad-workflow");
  if (
    requiredMutationIds.length === 0
    || plannedProofActions.length === 0
    || broadWorkflows.length === 0
  ) {
    fail(
      "release freeze requires hostile mutations, planned proof actions, and broad workflow names",
    );
  }

  if (git(["status", "--porcelain=v1", "--untracked-files=all"], repo) !== "") {
    fail("release freeze requires a clean worktree, including untracked files");
  }
  const commit = git(["rev-parse", "HEAD"], repo);
  const tree = git(["rev-parse", "HEAD^{tree}"], repo);
  const remoteLine = git(["ls-remote", "--exit-code", "origin", `refs/heads/${branch}`], repo);
  const remoteHead = remoteLine.split(/\s+/u)[0];
  if (remoteHead !== commit) {
    fail(`origin/${branch} is ${remoteHead}, not local HEAD ${commit}`);
  }
  for (const path of knownFutureChanges) {
    if (!ALLOWED_FUTURE_CHANGES.has(path)) {
      fail(`unsupported future source change: ${path}`);
    }
  }

  const mutationReceipt = parseJsonFile(mutationPath, "mutation receipt");
  validateMutationReceipt(mutationReceipt, {
    commit,
    tree,
    requiredIds: requiredMutationIds,
  });
  const platformEvidence = parseJsonFile(platformPath, "platform evidence");
  validatePlatformEvidence(platformEvidence, { commit, tree });
  const acceptedReleasePr = releasePr(repository, releasePrNumber, {
    branch,
    commit,
  });
  const integratedSupportPrs = supportPrNumbers.map(
    (number) => supportPr(repository, number, commit, repo),
  );

  const runs = currentRuns(repository);
  const cancelledRuns = has(args, "--cancel-superseded")
    ? cancelSupersededRuns({
      repository,
      commit,
      workflows: broadWorkflows,
      runs,
    })
    : [];
  const cancelledIds = new Set(
    cancelledRuns.map((entry) => String(entry.database_id)),
  );
  const remainingRuns = currentRuns(repository);
  const superseded = remainingRuns.filter(
    (entry) =>
      broadWorkflows.includes(entry.workflowName)
      && entry.headSha !== commit
      && !cancelledIds.has(String(entry.databaseId)),
  );
  if (superseded.length > 0) {
    fail("superseded broad proof remains queued or running");
  }

  const receipt = {
    schema: 1,
    repository,
    branch,
    commit,
    tree,
    worktree_clean: true,
    remote_head: remoteHead,
    release_pr: acceptedReleasePr,
    integrated_support_prs: integratedSupportPrs,
    known_future_source_changes: knownFutureChanges,
    planned_proof_actions: plannedProofActions,
    hostile_mutations: mutationReceipt,
    platform_evidence: platformEvidence,
    reusable_evidence: reusableEvidence,
    invalidated_evidence: invalidatedEvidence,
    running_workflows: remainingRuns,
    cancelled_superseded_runs: cancelledRuns,
    next_permitted_mutation: nextMutation,
  };
  receipt.digest = receiptDigest(receipt);
  validateReceipt(receipt, { commit, tree, requiredMutationIds });
  writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`);

  if (!has(args, "--no-publish-status")) {
    gh([
      "api",
      "--method",
      "POST",
      `repos/${repository}/statuses/${commit}`,
      "-f",
      "state=success",
      "-f",
      `context=${STATUS_PREFIX}/${receipt.digest}`,
      "-f",
      `description=tree=${tree}`,
    ]);
  }
  process.stdout.write(`${receipt.digest}\n`);
}

function verifyFile(args) {
  const receipt = parseJsonFile(required(args, "--receipt"), "freeze receipt");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  validateReceipt(receipt, {
    commit,
    tree,
    requiredMutationIds: values(args, "--required-mutation"),
  });
  process.stdout.write(`${receipt.digest}\n`);
}

function verifyStatus(args) {
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  const digest = required(args, "--receipt-digest");
  const statuses = JSON.parse(gh([
    "api",
    `repos/${repository}/commits/${commit}/statuses?per_page=100`,
  ]));
  const accepted = statuses.some((status) =>
    status?.state === "success"
    && status?.context === `${STATUS_PREFIX}/${digest}`
    && status?.description === `tree=${tree}`
  );
  if (!accepted) {
    fail("no successful exact-head release freeze status matches this receipt digest and tree");
  }
  process.stdout.write(`${digest}\n`);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "declare") {
    declare(args);
  } else if (command === "verify-file") {
    verifyFile(args);
  } else if (command === "verify-status") {
    verifyStatus(args);
  } else if (command === "cancel-superseded") {
    cancelSuperseded(args);
  } else {
    fail(
      "usage: release-freeze-barrier.mjs "
      + "<declare|verify-file|verify-status|cancel-superseded> ...",
    );
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`release freeze rejected: ${error.message}\n`);
    process.exitCode = 1;
  }
}
