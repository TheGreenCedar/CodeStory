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
const CANCEL_POLL_ATTEMPTS = Number.parseInt(
  process.env.CODESTORY_FREEZE_CANCEL_POLL_ATTEMPTS ?? "10",
  10,
);
const CANCEL_POLL_MS = Number.parseInt(
  process.env.CODESTORY_FREEZE_CANCEL_POLL_MS ?? "1000",
  10,
);

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

function elapsedSeconds(step) {
  const started = Date.parse(String(step?.started_at ?? ""));
  const completed = Date.parse(String(step?.completed_at ?? ""));
  if (!Number.isFinite(started) || !Number.isFinite(completed) || completed < started) {
    fail(`acceptance step ${step?.name ?? "<unknown>"} has invalid Actions timing`);
  }
  return (completed - started) / 1000;
}

export function validateAcceptanceProvenance({
  status,
  run,
  jobs,
  repository,
  commit,
  tree,
  digest,
}) {
  const escapedRepository = repository.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const target = new RegExp(
    `^https://github\\.com/${escapedRepository}/actions/runs/([1-9][0-9]*)$`,
    "u",
  ).exec(String(status?.target_url ?? ""));
  if (
    status?.state !== "success"
    || status?.context !== `${STATUS_PREFIX}/${digest}`
    || status?.description !== `tree=${tree}`
    || status?.creator?.login !== "github-actions[bot]"
    || status?.creator?.type !== "Bot"
    || !target
  ) {
    fail("release freeze status is not authenticated Actions acceptance");
  }
  if (
    String(run?.id) !== target[1]
    || run?.head_sha !== commit
    || run?.path !== ".github/workflows/source-proof.yml"
    || run?.event !== "workflow_dispatch"
    || run?.status !== "completed"
    || run?.conclusion !== "success"
    || run?.head_repository?.full_name !== repository
  ) {
    fail("release freeze acceptance run provenance changed");
  }
  if (!Array.isArray(jobs)) {
    fail("release freeze acceptance jobs are missing");
  }
  const requiredJobs = new Map([
    ["freeze-hostile-mutations", "Execute exact-head hostile mutation matrix"],
    ["freeze-windows-native-probe", "Run exact-head Windows native probe"],
    ["freeze-acceptance", "Publish executable release freeze"],
  ]);
  for (const [jobName, stepName] of requiredJobs) {
    const job = jobs.find((candidate) => candidate?.name === jobName);
    if (
      job?.status !== "completed"
      || job?.conclusion !== "success"
      || job?.head_sha !== commit
      || String(job?.run_id) !== String(run.id)
      || String(job?.run_attempt) !== String(run.run_attempt)
    ) {
      fail(`release freeze acceptance job ${jobName} is not a successful exact-run job`);
    }
    const step = job.steps?.find((candidate) => candidate?.name === stepName);
    if (step?.status !== "completed" || step?.conclusion !== "success") {
      fail(`release freeze acceptance step ${stepName} did not execute successfully`);
    }
    if (jobName === "freeze-windows-native-probe") {
      const labels = new Set(job.labels ?? []);
      for (const label of ["self-hosted", "Windows", "X64", "codestory-vulkan"]) {
        if (!labels.has(label)) {
          fail(`Windows native probe did not run on protected label ${label}`);
        }
      }
      if (elapsedSeconds(step) >= 90) {
        fail("Windows native probe must complete in under 90 seconds");
      }
    }
  }
  return Number(target[1]);
}

export function validateReceipt(receipt, { commit, tree }) {
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

function waitForSupersededRunsToStop({ repository, commit, workflows }) {
  if (
    !Number.isInteger(CANCEL_POLL_ATTEMPTS)
    || CANCEL_POLL_ATTEMPTS < 1
    || !Number.isInteger(CANCEL_POLL_MS)
    || CANCEL_POLL_MS < 0
  ) {
    fail("cancellation polling configuration is invalid");
  }
  for (let attempt = 0; attempt < CANCEL_POLL_ATTEMPTS; attempt += 1) {
    const remaining = currentRuns(repository).filter((entry) =>
      workflows.includes(entry.workflowName) && entry.headSha !== commit
    );
    if (remaining.length === 0) {
      return;
    }
    if (attempt + 1 < CANCEL_POLL_ATTEMPTS) {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, CANCEL_POLL_MS);
    }
  }
  fail("superseded broad proof remains queued or running after cancellation");
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
  waitForSupersededRunsToStop({ repository, commit, workflows });
  process.stdout.write(`${JSON.stringify({ cancelled })}\n`);
}

function invalidateSuperseded(args) {
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const workflows = values(args, "--broad-workflow");
  if (workflows.length === 0) {
    fail("--broad-workflow is required");
  }
  const cancelled = cancelSupersededRuns({
    repository,
    commit,
    workflows,
    runs: currentRuns(repository),
  });
  waitForSupersededRunsToStop({ repository, commit, workflows });
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
  const supportPrNumbers = values(args, "--support-pr");
  const knownFutureChanges = values(args, "--known-future-change");
  const plannedProofActions = values(args, "--planned-proof-action");
  const reusableEvidence = values(args, "--reusable-evidence");
  const invalidatedEvidence = values(args, "--invalidated-evidence");
  const nextMutation = required(args, "--next-permitted-mutation");
  const broadWorkflows = values(args, "--broad-workflow");
  if (
    plannedProofActions.length === 0
    || broadWorkflows.length === 0
  ) {
    fail("release freeze requires planned proof actions and broad workflow names");
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

  const acceptedReleasePr = releasePr(repository, releasePrNumber, {
    branch,
    commit,
  });
  const integratedSupportPrs = supportPrNumbers.map(
    (number) => supportPr(repository, number, commit, repo),
  );

  const runs = currentRuns(repository);
  const duplicate = runs.find((entry) =>
    broadWorkflows.includes(entry.workflowName) && entry.headSha === commit
  );
  if (duplicate) {
    fail(
      `unchanged head ${commit} already has active ${duplicate.workflowName} run ${duplicate.databaseId}`,
    );
  }
  const cancelledRuns = cancelSupersededRuns({
    repository,
    commit,
    workflows: broadWorkflows,
    runs,
  });
  if (cancelledRuns.length > 0) {
    waitForSupersededRunsToStop({
      repository,
      commit,
      workflows: broadWorkflows,
    });
  }
  const remainingRuns = currentRuns(repository);
  const remainingBroadRun = remainingRuns.find((entry) =>
    broadWorkflows.includes(entry.workflowName)
  );
  if (remainingBroadRun) {
    fail(
      `broad proof ${remainingBroadRun.databaseId} remains active before freeze declaration`,
    );
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
    reusable_evidence: reusableEvidence,
    invalidated_evidence: invalidatedEvidence,
    running_workflows: remainingRuns,
    cancelled_superseded_runs: cancelledRuns,
    next_permitted_mutation: nextMutation,
  };
  receipt.digest = receiptDigest(receipt);
  validateReceipt(receipt, { commit, tree });
  writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`);

  if (!has(args, "--no-publish-status")) {
    gh([
      "api",
      "--method",
      "POST",
      `repos/${repository}/statuses/${commit}`,
      "-f",
      "state=pending",
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
  });
  process.stdout.write(`${receipt.digest}\n`);
}

function matchingStatus({ repository, commit, tree, digest, state }) {
  const statuses = JSON.parse(gh([
    "api",
    `repos/${repository}/commits/${commit}/statuses?per_page=100`,
  ]));
  return statuses.find((status) =>
    status?.state === state
    && status?.context === `${STATUS_PREFIX}/${digest}`
    && status?.description === `tree=${tree}`
  );
}

function verifyPending(args) {
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  const digest = required(args, "--receipt-digest");
  if (!matchingStatus({ repository, commit, tree, digest, state: "pending" })) {
    fail("no pending local release freeze declaration matches this exact commit and tree");
  }
  process.stdout.write(`${digest}\n`);
}

function verifyStatus(args) {
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  const digest = required(args, "--receipt-digest");
  const status = matchingStatus({
    repository,
    commit,
    tree,
    digest,
    state: "success",
  });
  if (!status) {
    fail("no successful exact-head release freeze status matches this receipt digest and tree");
  }
  const target = /\/actions\/runs\/([1-9][0-9]*)$/u.exec(String(status.target_url ?? ""));
  if (!target) {
    fail("release freeze success status has no authenticated Actions run");
  }
  const run = JSON.parse(gh([
    "api",
    `repos/${repository}/actions/runs/${target[1]}`,
  ]));
  const jobsPayload = JSON.parse(gh([
    "api",
    `repos/${repository}/actions/runs/${target[1]}/jobs?per_page=100`,
  ]));
  validateAcceptanceProvenance({
    status,
    run,
    jobs: jobsPayload.jobs,
    repository,
    commit,
    tree,
    digest,
  });
  process.stdout.write(`${digest}\n`);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "declare") {
    declare(args);
  } else if (command === "verify-file") {
    verifyFile(args);
  } else if (command === "verify-pending") {
    verifyPending(args);
  } else if (command === "verify-status") {
    verifyStatus(args);
  } else if (command === "cancel-superseded") {
    cancelSuperseded(args);
  } else if (command === "invalidate-superseded") {
    invalidateSuperseded(args);
  } else {
    fail(
      "usage: release-freeze-barrier.mjs "
      + "<declare|verify-file|verify-pending|verify-status|cancel-superseded"
      + "|invalidate-superseded> ...",
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
