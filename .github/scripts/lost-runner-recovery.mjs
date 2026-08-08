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

import { spawnSync } from "node:child_process";
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
export const RERUN_PLAN_SCHEMA = "codestory.lost-runner-rerun-plan/v2";
export const NON_CLAIM_PLAN_SCHEMA = "codestory.accelerator-non-claim-plan/v1";
export const RERUN_DISPATCH_SCHEMA = "codestory.lost-runner-rerun-dispatch/v1";
export const RESERVATION_RECEIPT_SCHEMA = "codestory.protected-runner-reservation/v1";

/// Reservation states, typed per protected host BEFORE any proof is dispatched.
///
///   * `alive`: a heartbeat job completed successfully on the host within the freshness tolerance,
///     so the release may dispatch its proof.
///   * `held_by_active_run`: another workflow run is executing a job on the host right now. A busy
///     host is provably alive, so this BLOCKS (fail-red) rather than withholds -- withholding it
///     would overstate unavailability.
///   * `recheck_pending`: no fresh heartbeat yet, but the bounded recheck has not been recorded.
///     Never a final verdict: a receipt left in this state vouches for nothing.
///   * `unproven`: no fresh heartbeat after the recorded recheck spent the observation bound. The
///     proof is not dispatched, and this is the only state a pre-assignment withhold may cite.
export const RUNNER_ALIVE = "alive";
export const RUNNER_HELD_BY_ACTIVE_RUN = "held_by_active_run";
export const RUNNER_RECHECK_PENDING = "recheck_pending";
export const RUNNER_UNPROVEN = "unproven";

/// The one new typed non-claim detail: the proof job is ABSENT from the run because this run's own
/// reservation receipt typed the host `unproven` before dispatch.
export const RUNNER_UNPROVEN_PRE_ASSIGNMENT = "runner_unproven_pre_assignment";

/// Why an execution that failed is not in the re-dispatch list. Every failed execution the planner
/// saw carries exactly one of these, so a reader of the plan never has to infer an omission.
export const RETRY_REQUESTED = "retry_requested";
export const RECOVERY_BOUND_REACHED = "recovery_bound_reached";
export const SUPERSEDED_BY_LATER_EXECUTION = "superseded_by_later_execution";
export const NOT_A_RUNNER_COMMUNICATION_LOSS = "not_a_runner_communication_loss";

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

/// Order two executions of the same job name. Actions allocates job ids in creation order inside a
/// run, so the execution a re-dispatch created always ranks above the execution it replaced --
/// whether or not the carried-forward row is re-listed under the newer attempt number. Ranking on
/// the attempt alone answers wrongly under that second listing shape.
function ranksAbove(candidate, incumbent, name) {
  const candidateAttempt = positiveInteger(candidate?.run_attempt, `${name} run attempt`);
  const incumbentAttempt = positiveInteger(incumbent?.run_attempt, `${name} run attempt`);
  if (candidateAttempt !== incumbentAttempt) return candidateAttempt > incumbentAttempt;
  return positiveInteger(candidate?.id, `${name} job id`)
    > positiveInteger(incumbent?.id, `${name} job id`);
}

/// The newest execution of every job name, and the executions it superseded.
///
/// Recovery decisions are about *what the job is doing now*, so they are made from the newest
/// execution only. Deciding from every execution ever collected kept a job that a later attempt
/// already recovered in the plan forever, and named its stale, already-consumed job id.
function latestExecutionsByName(jobs) {
  const byName = new Map();
  for (const job of distinctExecutions(jobs)) {
    const name = leafJobName(job?.name);
    const previous = byName.get(name);
    if (previous === undefined) {
      byName.set(name, { name, latest: job, superseded: [] });
      continue;
    }
    if (ranksAbove(job, previous.latest, name)) {
      previous.superseded.push(previous.latest);
      previous.latest = job;
    } else {
      previous.superseded.push(job);
    }
  }
  return [...byName.values()];
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

function executionReference(job, name) {
  return {
    id: positiveInteger(job?.id, `${name} job id`),
    run_attempt: String(positiveInteger(job?.run_attempt, `${name} run attempt`)),
    conclusion: job?.conclusion === null || job?.conclusion === undefined
      ? null
      : String(job.conclusion),
  };
}

/// Decide which individual jobs to re-dispatch. Only jobs carrying the lost-runner signature are
/// ever re-dispatched -- the plan names them one by one instead of asking Actions to rerun every
/// failed job, so a proof that failed its own assertions is left exactly as it is and keeps the run
/// red. No approval gate is consulted: recovery is a machine decision or it does not happen.
///
/// Selection reads each job name's *newest* execution and nothing else. An execution a later
/// execution superseded is already spent: re-dispatching its id asks Actions to re-run a job the
/// run has moved past, and when the newest execution succeeded there is nothing to recover at all.
/// Both of those used to be in the plan, so a run that failed for an unrelated reason re-dispatched
/// an already-recovered host and led the request with a stale id.
///
/// The recovery bound is per job, not per run: see `countLostExecutions`.
export function planLostRunnerRerun({ runAttempt, runConclusion, jobs }) {
  const attempt = positiveInteger(runAttempt, "run attempt");
  const rows = [];
  for (const { name, latest, superseded } of latestExecutionsByName(jobs)) {
    const spent = countLostExecutions(jobs, name);
    for (const stale of superseded) {
      if (String(stale?.conclusion ?? "") !== "failure") continue;
      rows.push({
        ...classifyJobFailure(stale),
        lost_executions: spent,
        retry_decision: SUPERSEDED_BY_LATER_EXECUTION,
        superseded_by: executionReference(latest, name),
      });
    }
    if (String(latest?.conclusion ?? "") !== "failure") continue;
    const classified = classifyJobFailure(latest);
    const decision = classified.signature !== RUNNER_COMMUNICATION_LOSS
      ? NOT_A_RUNNER_COMMUNICATION_LOSS
      : spent < MAXIMUM_RUN_ATTEMPTS
        ? RETRY_REQUESTED
        : RECOVERY_BOUND_REACHED;
    rows.push({ ...classified, lost_executions: spent, retry_decision: decision });
  }
  const lost = rows.filter(({ retry_decision: decision }) =>
    decision === RETRY_REQUESTED || decision === RECOVERY_BOUND_REACHED);
  const notRetried = rows.filter(({ retry_decision: decision }) =>
    decision !== RETRY_REQUESTED && decision !== RECOVERY_BOUND_REACHED);
  const retryable = rows.filter(({ retry_decision: decision }) => decision === RETRY_REQUESTED);
  const reason = String(runConclusion ?? "") !== "failure"
    ? "run_did_not_fail"
    : lost.length === 0
      ? "no_runner_communication_loss"
      : retryable.length === 0
        ? "recovery_bound_reached"
        : RUNNER_COMMUNICATION_LOSS;
  return {
    schema: RERUN_PLAN_SCHEMA,
    rerun: reason === RUNNER_COMMUNICATION_LOSS,
    reason,
    run_attempt: attempt,
    maximum_run_attempts: MAXIMUM_RUN_ATTEMPTS,
    rerun_job_ids: reason === RUNNER_COMMUNICATION_LOSS
      ? retryable.map(({ id }) => id).sort((left, right) => left - right)
      : [],
    lost_jobs: lost,
    not_retried_jobs: notRetried,
  };
}

/// Ask Actions to re-run each named job, one id at a time, and record what each id answered.
///
/// The bound this has to respect is that one refused id may not consume the recovery of the others.
/// The release owns one host per accelerator and the hosts share a machine, so a run can lose two
/// of them at once; a sequential loop that stops at the first non-zero exit would then recover the
/// lowest-numbered job and silently abandon the rest. Every id therefore gets its own attempt and
/// its own recorded outcome, and the command fails only when the recovery as a whole did not
/// happen -- no id succeeded even though ids were named.
export function planRerunDispatch({ jobIds, dispatch }) {
  const ids = list(jobIds, "rerun job ids").map((id) => positiveInteger(id, "rerun job id"));
  const results = ids.map((id) => {
    const outcome = dispatch(id);
    return {
      job_id: id,
      dispatched: outcome.dispatched === true,
      detail: text(outcome.detail, `job ${id} dispatch detail`),
    };
  });
  const dispatched = results.filter(({ dispatched: ok }) => ok);
  const refused = results.filter(({ dispatched: ok }) => !ok);
  return {
    schema: RERUN_DISPATCH_SCHEMA,
    requested_job_ids: ids,
    dispatched_job_ids: dispatched.map(({ job_id: id }) => id),
    refused_job_ids: refused.map(({ job_id: id }) => id),
    results,
    recovered: ids.length > 0 && dispatched.length > 0,
    reason: ids.length === 0
      ? "no_job_requested"
      : dispatched.length === 0
        ? "every_job_refused"
        : refused.length === 0
          ? "every_job_dispatched"
          : "partially_dispatched",
  };
}

function canonicalEpoch(value, label) {
  const parsed = Date.parse(text(value, label));
  if (!Number.isFinite(parsed)) fail(`${label} must be a parseable timestamp`);
  return parsed;
}

/// One heartbeat evidence row, validated fail-closed: a row that exists but cannot be read is an
/// error, never a verdict. Rows that are still queued or in progress are real evidence of "no
/// completed heartbeat yet" and simply never count as fresh.
function heartbeatEvidenceRow(row, label) {
  const host = text(row?.host, `${label} host`);
  const status = text(row?.status, `${label} status`);
  if (status !== "completed") {
    return { host, status, conclusion: null, completed_at: null };
  }
  const conclusion = text(row?.conclusion, `${label} conclusion`);
  const completedAt = canonicalEpoch(row?.completed_at, `${label} completed_at`);
  return { host, status, conclusion, completed_at: String(row.completed_at), completed_epoch: completedAt };
}

/// Type each protected host BEFORE the release dispatches any proof to it.
///
/// A protected runner that is offline at dispatch produces a job that queues invisibly (~24h) and
/// terminally classifies as an ordinary blocked failure -- indistinguishable from a proof that ran
/// and refused. GITHUB_TOKEN cannot list self-hosted runners (the runner-administration endpoints
/// need an `administration` scope workflow `permissions:` cannot grant), so liveness is proven by
/// scheduled heartbeat jobs that run ON the hosts, and this planner reads that evidence.
///
/// `heartbeatJobs` is the list of recorded observation rounds -- each `{observed_at, jobs}` with
/// per-host heartbeat job rows. `unproven` is reachable only when EVERY recorded round is stale and
/// the number of rounds has spent the observation bound, which mirrors `MAXIMUM_RUN_ATTEMPTS`: one
/// automatic recheck, then the verdict. Fewer rounds leave the host `recheck_pending`, which is
/// deliberately not a verdict at all. Unreadable evidence anywhere is an error, never a state.
export function planProtectedRunnerReservation({ now, tolerance, hosts, heartbeatJobs, activeHolders }) {
  const nowEpoch = canonicalEpoch(now, "reservation now");
  const toleranceMinutes = positiveInteger(tolerance, "reservation tolerance minutes");
  const declaredHosts = list(hosts, "protected hosts").map((host) =>
    text(host?.id, "protected host id"));
  if (declaredHosts.length === 0) fail("reservation requires at least one protected host");
  if (new Set(declaredHosts).size !== declaredHosts.length) {
    fail("reservation hosts must be distinct");
  }
  const rounds = list(heartbeatJobs, "heartbeat observations");
  if (rounds.length === 0) fail("reservation requires at least one recorded heartbeat observation");
  if (rounds.length > MAXIMUM_RUN_ATTEMPTS) {
    fail(`reservation records at most ${MAXIMUM_RUN_ATTEMPTS} heartbeat observations`);
  }
  const observedRows = rounds.flatMap((round, index) => {
    canonicalEpoch(round?.observed_at, `heartbeat observation ${index + 1} observed_at`);
    return list(round?.jobs, `heartbeat observation ${index + 1} jobs`)
      .map((row, rowIndex) =>
        heartbeatEvidenceRow(row, `heartbeat observation ${index + 1} job ${rowIndex + 1}`));
  });
  for (const row of observedRows) {
    if (!declaredHosts.includes(row.host)) {
      fail(`heartbeat evidence names undeclared host ${row.host}`);
    }
  }
  const holders = list(activeHolders ?? [], "active holders").map((holder, index) => ({
    host: text(holder?.host, `active holder ${index + 1} host`),
    run_id: positiveInteger(holder?.run_id, `active holder ${index + 1} run id`),
    workflow: text(holder?.workflow, `active holder ${index + 1} workflow`),
    job_name: text(holder?.job_name, `active holder ${index + 1} job name`),
  }));
  for (const holder of holders) {
    if (!declaredHosts.includes(holder.host)) {
      fail(`active holder names undeclared host ${holder.host}`);
    }
  }
  const toleranceMs = toleranceMinutes * 60 * 1000;
  const rows = declaredHosts.map((hostId) => {
    const holder = holders.find(({ host }) => host === hostId);
    if (holder !== undefined) {
      // A busy host is provably alive -- something is executing on it right now -- but it cannot
      // take this release's proof, and recording it "unproven" would overstate unavailability.
      // The receipt names the holder and the reservation fails red.
      return {
        host: hostId,
        state: RUNNER_HELD_BY_ACTIVE_RUN,
        detail: "another_run_is_executing_on_this_host",
        observation_attempts: rounds.length,
        holder: { run_id: holder.run_id, workflow: holder.workflow, job_name: holder.job_name },
      };
    }
    const fresh = observedRows
      .filter((row) => row.host === hostId
        && row.status === "completed"
        && row.conclusion === "success"
        && nowEpoch - row.completed_epoch <= toleranceMs)
      .sort((left, right) => right.completed_epoch - left.completed_epoch);
    if (fresh.length > 0) {
      return {
        host: hostId,
        state: RUNNER_ALIVE,
        detail: "fresh_heartbeat",
        observation_attempts: rounds.length,
        heartbeat_completed_at: fresh[0].completed_at,
      };
    }
    if (rounds.length < MAXIMUM_RUN_ATTEMPTS) {
      return {
        host: hostId,
        state: RUNNER_RECHECK_PENDING,
        detail: "no_fresh_heartbeat_before_recorded_recheck",
        observation_attempts: rounds.length,
      };
    }
    return {
      host: hostId,
      state: RUNNER_UNPROVEN,
      detail: "no_fresh_heartbeat_after_recorded_recheck",
      observation_attempts: rounds.length,
    };
  });
  const byState = (state) => rows.filter((row) => row.state === state).map(({ host }) => host);
  return {
    schema: RESERVATION_RECEIPT_SCHEMA,
    now: text(now, "reservation now"),
    tolerance_minutes: toleranceMinutes,
    observation_attempts: rounds.length,
    maximum_observation_attempts: MAXIMUM_RUN_ATTEMPTS,
    hosts: rows,
    dispatch_hosts: byState(RUNNER_ALIVE),
    held_hosts: byState(RUNNER_HELD_BY_ACTIVE_RUN),
    unproven_hosts: byState(RUNNER_UNPROVEN),
    recheck: byState(RUNNER_RECHECK_PENDING).length > 0,
    recheck_hosts: byState(RUNNER_RECHECK_PENDING),
  };
}

/// Read this run's own reservation receipt, fail-closed. A receipt that exists but cannot be read,
/// carries another schema, or records a different observation bound is an error and never a
/// verdict: reinterpreting it permissively is exactly how a stale contract would leak a withhold.
function validatedReservationReceipt(receipt) {
  if (receipt === null || receipt === undefined) return null;
  if (typeof receipt !== "object" || Array.isArray(receipt)) {
    fail("reservation receipt must be an object");
  }
  if (receipt.schema !== RESERVATION_RECEIPT_SCHEMA) {
    fail(`reservation receipt must carry ${RESERVATION_RECEIPT_SCHEMA}`);
  }
  if (receipt.maximum_observation_attempts !== MAXIMUM_RUN_ATTEMPTS) {
    fail(`reservation receipt must record the ${MAXIMUM_RUN_ATTEMPTS}-observation recheck bound`);
  }
  const rows = list(receipt.hosts, "reservation receipt hosts").map((row) => ({
    host: text(row?.host, "reservation receipt host"),
    state: text(row?.state, "reservation receipt host state"),
    observation_attempts: positiveInteger(
      row?.observation_attempts,
      "reservation receipt host observation attempts",
    ),
  }));
  return { rows };
}

/// Whether this run's own receipt vouches `unproven` for the host, with the recheck bound spent.
/// Any other state -- alive, held, recheck still pending, or the host missing from the receipt --
/// vouches for nothing, so the absent job stays `blocked`.
function receiptVouchesUnproven(receipt, hostId) {
  if (receipt === null) return false;
  const rows = receipt.rows.filter(({ host }) => host === hostId);
  if (rows.length !== 1) return false;
  return rows[0].state === RUNNER_UNPROVEN
    && rows[0].observation_attempts >= MAXIMUM_RUN_ATTEMPTS;
}

export const HOST_PROVEN = "proven";
export const HOST_WITHHELD = "withheld";
export const HOST_RETRY_PENDING = "retry_pending";
export const HOST_BLOCKED = "blocked";

/// Decide, per protected accelerator host, whether this run may record a populated non-claim.
///
/// `withheld` is reachable from exactly two machine-checkable facts, one per typed reason:
///
///   * the lost-runner signature on a *present* job, *after* the retry bound is spent, and
///   * a job ABSENT from the run whose absence this run's own reservation receipt explains by
///     typing the host `unproven` with the recorded recheck bound spent.
///
/// Every other shape -- a proof that failed its own assertions, a cancelled job, an absent job
/// with no vouching receipt -- is `blocked`, which the CLI turns into a non-zero exit. A present
/// job that failed is decided from its own evidence and never from the receipt, so a receipt can
/// never mask a proof that ran and refused. Withholding is therefore never the fallback for
/// "something went wrong": it is the fallback for exactly one machine-checkable fact per reason.
export function planAcceleratorNonClaim({ runAttempt, hosts, jobs, reservation = null }) {
  const attempt = positiveInteger(runAttempt, "run attempt");
  const receipt = validatedReservationReceipt(reservation);
  const inspected = distinctExecutions(jobs);
  const rows = list(hosts, "protected hosts").map((host) => {
    const hostId = text(host?.id, "protected host id");
    const jobName = text(host?.job_name, `${hostId} producer job name`);
    const occurrences = inspected.filter((job) => leafJobName(job?.name) === jobName);
    if (occurrences.length === 0) {
      if (receiptVouchesUnproven(receipt, hostId)) {
        return {
          host: hostId,
          job_name: jobName,
          state: HOST_WITHHELD,
          detail: RUNNER_UNPROVEN_PRE_ASSIGNMENT,
          reservation: {
            schema: RESERVATION_RECEIPT_SCHEMA,
            state: RUNNER_UNPROVEN,
            observation_attempts: receipt.rows.find(({ host: id }) => id === hostId)
              .observation_attempts,
            maximum_observation_attempts: MAXIMUM_RUN_ATTEMPTS,
          },
        };
      }
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

/// One re-dispatch request. A refusal is data -- the receipt says which id Actions declined and
/// what it said -- and never an exception, because the ids after it still have their own recovery
/// owed to them.
function rerunJobThroughGh(repository, jobId) {
  const result = spawnSync(
    "gh",
    ["api", "--method", "POST", `repos/${repository}/actions/jobs/${jobId}/rerun`],
    { encoding: "utf8" },
  );
  if (result.error) {
    return { dispatched: false, detail: `gh_not_invoked: ${result.error.message}` };
  }
  if (result.status === 0) return { dispatched: true, detail: "accepted" };
  const stderr = String(result.stderr ?? "").replace(/\s+/gu, " ").trim().slice(0, 200);
  return {
    dispatched: false,
    detail: `refused_exit_${String(result.status)}: ${stderr === "" ? "no stderr" : stderr}`,
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
  if (command === "dispatch-rerun") {
    const plan = readJson(values.plan);
    if (plan.schema !== RERUN_PLAN_SCHEMA) {
      fail(`rerun plan must carry ${RERUN_PLAN_SCHEMA}`);
    }
    if (plan.rerun !== true) fail("dispatch-rerun refuses a plan that did not ask for a rerun");
    const repository = text(
      values.repository ?? process.env.GITHUB_REPOSITORY,
      "repository",
    );
    const receipt = planRerunDispatch({
      jobIds: plan.rerun_job_ids,
      dispatch: (jobId) => rerunJobThroughGh(repository, jobId),
    });
    writeJson(values.out ?? "target/lost-runner/rerun-dispatch.json", receipt);
    emitOutputs({
      recovered: String(receipt.recovered),
      dispatched_job_ids: receipt.dispatched_job_ids.join(" "),
      refused_job_ids: receipt.refused_job_ids.join(" "),
    });
    console.log(JSON.stringify(receipt, null, 2));
    for (const row of receipt.results.filter(({ dispatched }) => !dispatched)) {
      console.error(`::error::Actions refused the rerun of job ${row.job_id}: ${row.detail}`);
    }
    if (!receipt.recovered) process.exitCode = 1;
    return;
  }
  if (command === "plan-reservation") {
    const input = readJson(values.input);
    const receipt = planProtectedRunnerReservation({
      now: input.now,
      tolerance: input.tolerance_minutes,
      hosts: input.hosts,
      heartbeatJobs: input.observations,
      activeHolders: input.active_holders,
    });
    const stamped = {
      ...receipt,
      run_id: positiveInteger(input.run_id, "reservation run id"),
      run_attempt: positiveInteger(input.run_attempt, "reservation run attempt"),
    };
    writeJson(values.out ?? "target/runner-reservation/receipt.json", stamped);
    const outputs = {
      recheck: String(receipt.recheck),
      dispatch_hosts: receipt.dispatch_hosts.join(" "),
      held_hosts: receipt.held_hosts.join(" "),
      unproven_hosts: receipt.unproven_hosts.join(" "),
    };
    for (const row of receipt.hosts) {
      outputs[`dispatch_${row.host.replaceAll(/[^A-Za-z0-9_]/gu, "_")}`] =
        String(row.state === RUNNER_ALIVE);
    }
    emitOutputs(outputs);
    console.log(JSON.stringify(stamped, null, 2));
    // A held host is a final fail-red verdict: a busy host is provably alive, so the release
    // stops instead of overstating unavailability. While a recheck is still pending nothing is
    // final yet -- the caller performs the one bounded recheck and plans again.
    if (!receipt.recheck && receipt.held_hosts.length > 0) {
      for (const row of receipt.hosts.filter(({ state }) => state === RUNNER_HELD_BY_ACTIVE_RUN)) {
        console.error(
          `::error::${row.host} is held by active run ${row.holder.run_id} `
          + `(${row.holder.workflow} / ${row.holder.job_name}); a busy host blocks the release.`,
        );
      }
      process.exitCode = 1;
    }
    return;
  }
  if (command === "plan-non-claim") {
    const input = readJson(values.input);
    const plan = planAcceleratorNonClaim({
      runAttempt: input.run_attempt,
      hosts: input.hosts,
      jobs: input.jobs,
      reservation: values.reservation === undefined ? null : readJson(values.reservation),
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
