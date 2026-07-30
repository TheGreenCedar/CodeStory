#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  lstatSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import path from "node:path";
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
const NEXT_PERMITTED_MUTATION =
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";
const PLANNED_PROOF_ACTIONS = [
  "acceptance",
  "source-proof",
  "calibration",
  "qualification",
  "release",
];
const RECEIPT_ARTIFACT_PREFIX = "release-freeze-receipt-attempt-";
const RECEIPT_FILE = "release-freeze-receipt.json";
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
  artifact,
  receipt,
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
  const artifactName = `${RECEIPT_ARTIFACT_PREFIX}${run.run_attempt}`;
  if (
    artifact?.name !== artifactName
    || artifact?.expired !== false
    || String(artifact?.workflow_run?.id) !== String(run.id)
    || receipt?.digest !== digest
  ) {
    fail("release freeze receipt artifact provenance changed");
  }
  validateReceipt(receipt, {
    repository,
    commit,
    tree,
    runId: String(run.id),
    runAttempt: String(run.run_attempt),
  });
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

export function validateReceipt(
  receipt,
  { repository, commit, tree, runId, runAttempt },
) {
  if (receipt?.schema !== 2 || receipt?.authority !== "github_actions") {
    fail("freeze receipt must use the GitHub Actions authority schema");
  }
  if (
    receipt.repository !== repository
    || receipt.commit !== commit
    || receipt.tree !== tree
  ) {
    fail("freeze receipt does not match the exact commit and tree");
  }
  if (receipt.worktree_clean !== true || receipt.remote_head !== commit) {
    fail("freeze receipt must prove a clean worktree pushed at the exact commit");
  }
  if (
    !Number.isInteger(receipt?.release_pr?.number)
    || receipt.release_pr.number <= 0
    || receipt?.release_pr?.head_commit !== commit
    || receipt?.release_pr?.head !== receipt.branch
    || receipt?.release_pr?.base !== "dev/codestory-next"
    || !/^[0-9a-f]{40}$/u.test(String(receipt?.release_pr?.base_commit ?? ""))
  ) {
    fail("freeze receipt must bind the open release PR at this exact head");
  }
  if (
    !Array.isArray(receipt.integrated_support_prs)
    || new Set(receipt.integrated_support_prs.map(entry => entry?.number)).size
      !== receipt.integrated_support_prs.length
  ) {
    fail("freeze receipt must contain unique integrated support PRs");
  }
  if (
    !Array.isArray(receipt.known_future_source_changes)
    || receipt.known_future_source_changes.length !== 1
    || !ALLOWED_FUTURE_CHANGES.has(receipt.known_future_source_changes[0])
  ) {
    fail("freeze receipt must declare only the generated constant-set change");
  }
  if (
    JSON.stringify(receipt.planned_proof_actions) !== JSON.stringify(PLANNED_PROOF_ACTIONS)
    || JSON.stringify(receipt.proof_triggering_labels) !== "[]"
    || JSON.stringify(receipt.proof_triggering_actions) !== JSON.stringify(
      PLANNED_PROOF_ACTIONS,
    )
  ) {
    fail("freeze receipt must record the exact proof-triggering actions and no labels");
  }
  for (const field of [
    "reusable_evidence",
    "invalidated_evidence",
    "running_workflows",
    "cancelled_superseded_runs",
  ]) {
    if (!Array.isArray(receipt[field])) {
      fail(`freeze receipt must contain ${field}`);
    }
  }
  if (receipt.next_permitted_mutation !== NEXT_PERMITTED_MUTATION) {
    fail("freeze receipt must name the generated constant set as the next mutation");
  }
  if (
    String(receipt?.acceptance_run?.id) !== String(runId)
    || String(receipt?.acceptance_run?.attempt) !== String(runAttempt)
    || receipt?.acceptance_run?.workflow !== ".github/workflows/source-proof.yml"
    || receipt?.acceptance_run?.event !== "workflow_dispatch"
  ) {
    fail("freeze receipt must bind its exact Actions run and attempt");
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
  const pr = JSON.parse(gh(["api", `repos/${repository}/pulls/${number}`]));
  if (
    pr.state !== "open"
    || pr?.base?.ref !== "dev/codestory-next"
    || pr?.head?.ref !== branch
    || pr?.head?.sha !== commit
    || pr?.head?.repo?.full_name !== repository
    || !/^[0-9a-f]{40}$/u.test(String(pr?.base?.sha ?? ""))
  ) {
    fail(
      `release PR #${number} must be an open same-repository ${branch} -> `
      + `dev/codestory-next PR at exact head ${commit}`,
    );
  }
  const comparison = JSON.parse(gh([
    "api",
    `repos/${repository}/compare/${pr.base.sha}...${commit}`,
  ]));
  if (!["ahead", "identical"].includes(comparison?.status)) {
    fail(
      `release PR #${number} head ${commit} does not contain current dev base ${pr.base.sha}`,
    );
  }
  return {
    number: pr.number,
    base: pr.base.ref,
    base_commit: pr.base.sha,
    head: pr.head.ref,
    head_commit: pr.head.sha,
  };
}

function jsonArray(args, name, label) {
  let parsed;
  try {
    parsed = JSON.parse(value(args, name, "[]"));
  } catch (error) {
    fail(`${label} must be valid JSON: ${error.message}`);
  }
  if (!Array.isArray(parsed)) {
    fail(`${label} must be a JSON array`);
  }
  return parsed;
}

function stringArray(args, name, label) {
  const parsed = jsonArray(args, name, label);
  if (!parsed.every(entry => typeof entry === "string" && entry.length > 0)) {
    fail(`${label} must contain only non-empty strings`);
  }
  return parsed;
}

function recordActionsReceipt(args) {
  const repo = value(args, "--repo", process.cwd());
  const repository = required(args, "--repository");
  const branch = required(args, "--branch");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  const output = required(args, "--output");
  const releasePrNumber = required(args, "--release-pr");
  const runId = required(args, "--run-id");
  const runAttempt = required(args, "--run-attempt");
  const supportPrNumbers = jsonArray(args, "--support-prs-json", "support PRs");
  if (
    !supportPrNumbers.every(number => Number.isInteger(number) && number > 0)
    || new Set(supportPrNumbers).size !== supportPrNumbers.length
  ) {
    fail("support PRs must contain unique positive integers");
  }
  const reusableEvidence = stringArray(
    args,
    "--reusable-evidence-json",
    "reusable evidence",
  );
  const invalidatedEvidence = stringArray(
    args,
    "--invalidated-evidence-json",
    "invalidated evidence",
  );
  const cancelledRuns = jsonArray(
    args,
    "--cancelled-runs-json",
    "cancelled superseded runs",
  );
  const broadWorkflows = values(args, "--broad-workflow");
  if (broadWorkflows.length === 0) {
    fail("release freeze requires broad workflow names");
  }
  if (
    process.env.GITHUB_ACTIONS !== "true"
    || process.env.GITHUB_EVENT_NAME !== "workflow_dispatch"
  ) {
    fail("the canonical release freeze receipt may be produced only by workflow_dispatch");
  }

  if (git(["status", "--porcelain=v1", "--untracked-files=all"], repo) !== "") {
    fail("release freeze requires a clean worktree, including untracked files");
  }
  if (
    git(["rev-parse", "HEAD"], repo) !== commit
    || git(["rev-parse", "HEAD^{tree}"], repo) !== tree
  ) {
    fail("checked-out Actions source does not match the declared commit and tree");
  }

  const acceptedReleasePr = releasePr(repository, releasePrNumber, {
    branch,
    commit,
  });
  const integratedSupportPrs = supportPrNumbers.map(
    (number) => supportPr(repository, number, commit, repo),
  );

  const remainingRuns = currentRuns(repository).filter(
    entry => String(entry.databaseId) !== String(runId),
  );
  const remainingBroadRun = remainingRuns.find((entry) =>
    broadWorkflows.includes(entry.workflowName)
  );
  if (remainingBroadRun) {
    fail(
      `broad proof ${remainingBroadRun.databaseId} remains active before freeze declaration`,
    );
  }

  const receipt = {
    schema: 2,
    authority: "github_actions",
    repository,
    branch,
    commit,
    tree,
    worktree_clean: true,
    remote_head: commit,
    release_pr: acceptedReleasePr,
    integrated_support_prs: integratedSupportPrs,
    known_future_source_changes: [NEXT_PERMITTED_MUTATION],
    planned_proof_actions: PLANNED_PROOF_ACTIONS,
    proof_triggering_labels: [],
    proof_triggering_actions: PLANNED_PROOF_ACTIONS,
    reusable_evidence: reusableEvidence,
    invalidated_evidence: invalidatedEvidence,
    running_workflows: remainingRuns,
    cancelled_superseded_runs: cancelledRuns,
    next_permitted_mutation: NEXT_PERMITTED_MUTATION,
    acceptance_run: {
      id: Number(runId),
      attempt: Number(runAttempt),
      workflow: ".github/workflows/source-proof.yml",
      event: "workflow_dispatch",
    },
  };
  receipt.digest = receiptDigest(receipt);
  validateReceipt(receipt, {
    repository,
    commit,
    tree,
    runId,
    runAttempt,
  });
  writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`);
  const githubOutput = value(args, "--github-output");
  if (githubOutput) {
    writeFileSync(
      githubOutput,
      `digest=${receipt.digest}\nartifact_name=${RECEIPT_ARTIFACT_PREFIX}${runAttempt}\n`,
      { flag: "a" },
    );
  }
  process.stdout.write(`${receipt.digest}\n`);
}

function verifyFile(args) {
  const receipt = parseJsonFile(required(args, "--receipt"), "freeze receipt");
  const repository = required(args, "--repository");
  const commit = required(args, "--commit");
  const tree = required(args, "--tree");
  validateReceipt(receipt, {
    repository,
    commit,
    tree,
    runId: required(args, "--run-id"),
    runAttempt: required(args, "--run-attempt"),
  });
  process.stdout.write(`${receipt.digest}\n`);
}

export function acceptedFreezeStatus(statuses, { tree, digest }) {
  if (!Array.isArray(statuses)) {
    fail("release freeze statuses are missing");
  }
  const context = `${STATUS_PREFIX}/${digest}`;
  const newest = statuses
    .filter(status => status?.context === context)
    .reduce((latest, status) => {
      if (!latest) {
        return status;
      }
      const latestId = BigInt(String(latest.id ?? "0"));
      const statusId = BigInt(String(status.id ?? "0"));
      return statusId > latestId ? status : latest;
    }, undefined);
  if (newest?.state !== "success" || newest?.description !== `tree=${tree}`) {
    return undefined;
  }
  return newest;
}

function matchingStatus({ repository, commit, tree, digest }) {
  const statuses = JSON.parse(gh([
    "api",
    `repos/${repository}/commits/${commit}/statuses?per_page=100`,
  ]));
  return acceptedFreezeStatus(statuses, { tree, digest });
}

function downloadAuthenticatedReceipt({ repository, run }) {
  const artifactName = `${RECEIPT_ARTIFACT_PREFIX}${run.run_attempt}`;
  const payload = JSON.parse(gh([
    "api",
    `repos/${repository}/actions/runs/${run.id}/artifacts?per_page=100`,
  ]));
  const matches = (payload.artifacts ?? []).filter(
    artifact => artifact?.name === artifactName && artifact?.expired === false,
  );
  if (matches.length !== 1) {
    fail(`acceptance run must retain exactly one unexpired ${artifactName}`);
  }
  const directory = mkdtempSync(path.join(tmpdir(), "codestory-freeze-receipt-"));
  try {
    gh([
      "run",
      "download",
      String(run.id),
      "--repo",
      repository,
      "--name",
      artifactName,
      "--dir",
      directory,
    ]);
    const entries = readdirSync(directory);
    if (
      entries.length !== 1
      || entries[0] !== RECEIPT_FILE
      || !lstatSync(path.join(directory, RECEIPT_FILE)).isFile()
      || lstatSync(path.join(directory, RECEIPT_FILE)).nlink !== 1
    ) {
      fail("release freeze artifact must contain one singly linked canonical receipt");
    }
    return {
      artifact: matches[0],
      receipt: parseJsonFile(
        path.join(directory, RECEIPT_FILE),
        "authenticated freeze receipt",
      ),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
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
  const { artifact, receipt } = downloadAuthenticatedReceipt({
    repository,
    run,
  });
  const currentReleasePr = releasePr(repository, receipt?.release_pr?.number, {
    branch: receipt?.branch,
    commit,
  });
  if (currentReleasePr.base_commit !== receipt?.release_pr?.base_commit) {
    fail("release PR base advanced after freeze acceptance");
  }
  const jobsPayload = JSON.parse(gh([
    "api",
    `repos/${repository}/actions/runs/${target[1]}/jobs?per_page=100`,
  ]));
  validateAcceptanceProvenance({
    status,
    run,
    jobs: jobsPayload.jobs,
    artifact,
    receipt,
    repository,
    commit,
    tree,
    digest,
  });
  process.stdout.write(`${digest}\n`);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "record-actions-receipt") {
    recordActionsReceipt(args);
  } else if (command === "verify-file") {
    verifyFile(args);
  } else if (command === "verify-status") {
    verifyStatus(args);
  } else if (command === "cancel-superseded") {
    cancelSuperseded(args);
  } else if (command === "invalidate-superseded") {
    invalidateSuperseded(args);
  } else {
    fail(
      "usage: release-freeze-barrier.mjs "
      + "<record-actions-receipt|verify-file|verify-status|cancel-superseded"
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
