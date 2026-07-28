#!/usr/bin/env node

// A self-hosted runner that drops its connection mid-job is reported by Actions as an ordinary job
// failure, which is indistinguishable from a proof that ran and refused to pass unless the run is
// inspected. GitHub does leave a precise, machine-readable signature behind:
//
//   1. a job annotation whose text is exactly LOST_RUNNER_ANNOTATION,
//   2. at least one step that completed with an EMPTY conclusion -- the steps queued behind the
//      point where the connection died were never resolved, and
//   3. no log blob: the runner never uploaded one, so the logs endpoint has nothing to serve.
//
// A proof that executed and failed its own assertions has none of those: it has a real conclusion
// on every step and a log blob. This module keys on the signature, never on job names, so that a
// renamed or newly added proof job cannot silently become retryable.

import { readFileSync, writeFileSync, appendFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const LOST_RUNNER_ANNOTATION =
  "The self-hosted runner lost communication with the server. "
  + "Verify the machine is running and has a healthy network connection.";

/// Total executions of one job permitted for a release run, counting the original. Two means one
/// automatic recovery attempt: the bound exists so a permanently sick host cannot loop forever, and
/// it is the same bound the withheld-claim fallback waits for before it stops expecting a proof.
export const MAXIMUM_RUN_ATTEMPTS = 2;

export const RUNNER_COMMUNICATION_LOSS = "runner_communication_loss";
export const JOB_ASSERTION_FAILURE = "job_assertion_failure";
export const RERUN_PLAN_SCHEMA = "codestory.lost-runner-rerun-plan/v1";
export const NON_CLAIM_PLAN_SCHEMA = "codestory.accelerator-non-claim-plan/v1";

function fail(message) {
  throw new Error(message);
}

function text(value, label) {
  if (typeof value !== "string" || value === "") fail(`${label} must be non-empty text`);
  return value;
}

function positiveInteger(value, label) {
  const selected = String(value ?? "");
  if (!/^[1-9]\d*$/u.test(selected)) fail(`${label} must be a positive integer`);
  return Number(selected);
}

function list(value, label) {
  if (!Array.isArray(value)) fail(`${label} must be an array`);
  return value;
}

/// Actions renders a reusable workflow's job as "<caller job> / <called job>"; the leaf is the name
/// the release claim graph binds its producers to.
export function leafJobName(name) {
  return text(name, "Actions job name").split(" / ").at(-1);
}

function emptyConclusionSteps(job) {
  return list(job.steps ?? [], "job steps")
    .filter((step) => step?.conclusion === null || step?.conclusion === "")
    .map((step) => String(step?.name ?? ""));
}

function annotationMatches(job) {
  if (!Array.isArray(job.annotations)) fail("job annotations must be an array");
  return job.annotations
    .some((annotation) => String(annotation?.message ?? "").trim() === LOST_RUNNER_ANNOTATION);
}

/// Whether the runner uploaded a log blob. This must be a fact the collector actually established,
/// not a field that happens to be absent: an absent field used to read as `false`, which is the
/// half of the signature a lost runner needs, so a collector that silently stopped probing would
/// have made every failure look lost. Absence is an error here and nowhere near a verdict.
function logUploaded(job) {
  if (typeof job.log_uploaded !== "boolean") {
    fail("job log_uploaded must be a boolean established by the evidence collector");
  }
  return job.log_uploaded;
}

/// Classify one *failed* job. The lost-runner verdict requires all three signature parts at once:
/// any one of them alone is reachable by an ordinary failure (a cancelled step leaves an empty
/// conclusion, a log can be expired), so partial matches stay assertion failures and are never
/// retried and never converted into a withheld claim.
export function classifyJobFailure(job) {
  const name = leafJobName(job?.name);
  const conclusion = job?.conclusion === null || job?.conclusion === undefined
    ? null
    : String(job.conclusion);
  const emptySteps = emptyConclusionSteps(job ?? {});
  const evidence = {
    annotation_matched: annotationMatches(job ?? {}),
    empty_conclusion_steps: emptySteps,
    log_uploaded: logUploaded(job ?? {}),
  };
  const lost = conclusion === "failure"
    && evidence.annotation_matched
    && emptySteps.length > 0
    && evidence.log_uploaded === false;
  return {
    id: positiveInteger(job?.id, "Actions job id"),
    name,
    conclusion,
    run_attempt: String(positiveInteger(job?.run_attempt, "Actions job run attempt")),
    signature: lost ? RUNNER_COMMUNICATION_LOSS : JOB_ASSERTION_FAILURE,
    evidence,
  };
}

/// One row per *execution*. The collector reads every attempt of a run, so a job that Actions
/// carried forward unchanged appears once per attempt listing under the same id; those are the same
/// execution and must be counted once.
function distinctExecutions(jobs) {
  const byId = new Map();
  for (const job of list(jobs, "Actions jobs")) {
    const id = positiveInteger(job?.id, "Actions job id");
    const attempt = positiveInteger(job?.run_attempt, "Actions job run attempt");
    const previous = byId.get(id);
    // Keep the richest sighting of an execution: a later attempt's listing carries the same facts,
    // but only the listing taken from the attempt the job ran in has its evidence probed.
    if (previous === undefined || positiveInteger(previous.run_attempt, "run attempt") < attempt) {
      byId.set(id, job);
    }
  }
  return [...byId.values()];
}

function failedJobs(jobs) {
  return distinctExecutions(jobs).filter((job) => String(job?.conclusion ?? "") === "failure");
}

/// How many *executions* of one job name were lost to their runner, across every attempt collected.
///
/// This is the recovery counter, and it is deliberately not `GITHUB_RUN_ATTEMPT`. A release run
/// reaches attempt 2 for any reason a maintainer likes -- a flaky unrelated job, a re-run to pick
/// up a secret -- and the run-attempt number cannot tell that apart from "the automatic recovery
/// for this host has already been spent". Counting lost executions of the job itself can: a host
/// that has been lost once is owed a re-dispatch no matter what attempt the run is on, and a host
/// that has been lost twice has had its one automatic recovery and gets no more.
export function countLostExecutions(jobs, jobName) {
  return distinctExecutions(jobs)
    .filter((job) => leafJobName(job?.name) === jobName)
    .filter((job) => String(job?.conclusion ?? "") === "failure")
    .filter((job) => classifyJobFailure(job).signature === RUNNER_COMMUNICATION_LOSS)
    .length;
}

/// Decide which individual jobs to re-dispatch. Only jobs carrying the lost-runner signature are
/// ever re-dispatched -- the plan names them one by one instead of asking Actions to rerun every
/// failed job, so a proof that failed its own assertions is left exactly as it is and keeps the run
/// red. No approval gate is consulted: recovery is a machine decision or it does not happen.
///
/// The bound is per job, not per run: see `countLostExecutions`.
export function planLostRunnerRerun({ runAttempt, runConclusion, jobs }) {
  const attempt = positiveInteger(runAttempt, "run attempt");
  const classified = failedJobs(jobs).map(classifyJobFailure);
  const withRecoveries = classified.map((job) => ({
    ...job,
    lost_executions: countLostExecutions(jobs, job.name),
  }));
  const lost = withRecoveries.filter(({ signature }) => signature === RUNNER_COMMUNICATION_LOSS);
  const notRetried = withRecoveries.filter(({ signature }) => signature !== RUNNER_COMMUNICATION_LOSS);
  const retryable = lost.filter(({ lost_executions: spent }) => spent < MAXIMUM_RUN_ATTEMPTS);
  const reason = String(runConclusion ?? "") !== "failure"
    ? "run_did_not_fail"
    : lost.length === 0
      ? "no_runner_communication_loss"
      : retryable.length === 0
        ? "recovery_bound_reached"
        : "runner_communication_loss";
  return {
    schema: RERUN_PLAN_SCHEMA,
    rerun: reason === "runner_communication_loss",
    reason,
    run_attempt: attempt,
    maximum_run_attempts: MAXIMUM_RUN_ATTEMPTS,
    rerun_job_ids: reason === "runner_communication_loss" ? retryable.map(({ id }) => id) : [],
    lost_jobs: lost,
    not_retried_jobs: notRetried,
  };
}

export const HOST_PROVEN = "proven";
export const HOST_WITHHELD = "withheld";
export const HOST_RETRY_PENDING = "retry_pending";
export const HOST_BLOCKED = "blocked";

/// Decide, per protected accelerator host, whether this run may record a populated non-claim.
///
/// `withheld` is reachable only from the lost-runner signature *after* the retry bound is spent.
/// Every other shape -- a proof that failed its own assertions, a cancelled job, a job that never
/// appeared in the run -- is `blocked`, which the CLI turns into a non-zero exit. Withholding is
/// therefore never the fallback for "something went wrong": it is the fallback for exactly one
/// machine-checkable fact.
export function planAcceleratorNonClaim({ runAttempt, hosts, jobs }) {
  const attempt = positiveInteger(runAttempt, "run attempt");
  const inspected = distinctExecutions(jobs);
  const rows = list(hosts, "protected hosts").map((host) => {
    const hostId = text(host?.id, "protected host id");
    const jobName = text(host?.job_name, `${hostId} producer job name`);
    const occurrences = inspected.filter((job) => leafJobName(job?.name) === jobName);
    if (occurrences.length === 0) {
      return { host: hostId, job_name: jobName, state: HOST_BLOCKED, detail: "job_absent_from_run" };
    }
    const latestAttempt = Math.max(
      ...occurrences.map((job) => positiveInteger(job?.run_attempt, `${jobName} run attempt`)),
    );
    const latest = occurrences.filter((job) => Number(job.run_attempt) === latestAttempt);
    if (latest.length !== 1) {
      return { host: hostId, job_name: jobName, state: HOST_BLOCKED, detail: "job_is_ambiguous" };
    }
    const job = latest[0];
    if (String(job.status ?? "") === "completed" && String(job.conclusion ?? "") === "success") {
      return { host: hostId, job_name: jobName, state: HOST_PROVEN, detail: "proof_succeeded" };
    }
    if (String(job.conclusion ?? "") !== "failure") {
      return {
        host: hostId,
        job_name: jobName,
        state: HOST_BLOCKED,
        detail: `job_conclusion_${String(job.conclusion ?? "none")}`,
      };
    }
    const classified = classifyJobFailure(job);
    if (classified.signature !== RUNNER_COMMUNICATION_LOSS) {
      return {
        host: hostId,
        job_name: jobName,
        state: HOST_BLOCKED,
        detail: JOB_ASSERTION_FAILURE,
        job: classified,
      };
    }
    // The bound that has to be spent is this host's own recovery, counted in lost executions of
    // its job. A release run sitting at attempt 2 for an unrelated reason has still never
    // re-dispatched this host, and the first loss of a runner is owed its one automatic retry.
    const spent = countLostExecutions(jobs, jobName);
    if (spent < MAXIMUM_RUN_ATTEMPTS || attempt < MAXIMUM_RUN_ATTEMPTS) {
      return {
        host: hostId,
        job_name: jobName,
        state: HOST_RETRY_PENDING,
        detail: "automatic_rerun_still_owed",
        lost_executions: spent,
        job: classified,
      };
    }
    return {
      host: hostId,
      job_name: jobName,
      state: HOST_WITHHELD,
      detail: RUNNER_COMMUNICATION_LOSS,
      lost_executions: spent,
      job: classified,
    };
  });
  return {
    schema: NON_CLAIM_PLAN_SCHEMA,
    run_attempt: attempt,
    maximum_run_attempts: MAXIMUM_RUN_ATTEMPTS,
    hosts: rows,
    withheld_hosts: rows.filter(({ state }) => state === HOST_WITHHELD).map(({ host }) => host),
    blocked_hosts: rows
      .filter(({ state }) => state === HOST_BLOCKED || state === HOST_RETRY_PENDING)
      .map(({ host }) => host),
  };
}

function readJson(filePath) {
  return JSON.parse(readFileSync(path.resolve(text(filePath, "input path")), "utf8"));
}

function writeJson(filePath, value) {
  const absolute = path.resolve(text(filePath, "output path"));
  mkdirSync(path.dirname(absolute), { recursive: true });
  writeFileSync(absolute, `${JSON.stringify(value, null, 2)}\n`);
}

function emitOutputs(entries) {
  const target = process.env.GITHUB_OUTPUT;
  if (!target) return;
  for (const [key, value] of Object.entries(entries)) {
    appendFileSync(target, `${key}=${value}\n`);
  }
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
  if (command === "plan-rerun") {
    const input = readJson(values.input);
    const plan = planLostRunnerRerun({
      runAttempt: input.run_attempt,
      runConclusion: input.conclusion,
      jobs: input.jobs,
    });
    writeJson(values.out ?? "target/lost-runner/rerun-plan.json", plan);
    emitOutputs({ rerun: String(plan.rerun), job_ids: plan.rerun_job_ids.join(" ") });
    console.log(JSON.stringify(plan, null, 2));
    return;
  }
  if (command === "plan-non-claim") {
    const input = readJson(values.input);
    const plan = planAcceleratorNonClaim({
      runAttempt: input.run_attempt,
      hosts: input.hosts,
      jobs: input.jobs,
    });
    writeJson(values.out ?? "target/lost-runner/non-claim-plan.json", plan);
    emitOutputs({ withheld_hosts: plan.withheld_hosts.join(" ") });
    console.log(JSON.stringify(plan, null, 2));
    if (plan.blocked_hosts.length > 0) {
      const blocked = plan.hosts.filter(({ state }) => state !== HOST_PROVEN && state !== HOST_WITHHELD);
      for (const row of blocked) {
        console.error(`::error::${row.host} cannot record a non-claim: ${row.detail}`);
      }
      process.exitCode = 1;
    }
    return;
  }
  fail(`unknown command ${String(command)}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main();
}
