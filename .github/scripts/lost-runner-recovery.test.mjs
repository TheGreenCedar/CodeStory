import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  JOB_ASSERTION_FAILURE,
  LOST_RUNNER_ANNOTATION,
  MAXIMUM_RUN_ATTEMPTS,
  NOT_A_RUNNER_COMMUNICATION_LOSS,
  RECOVERY_BOUND_REACHED,
  RERUN_DISPATCH_SCHEMA,
  RERUN_PLAN_SCHEMA,
  RETRY_REQUESTED,
  RUNNER_COMMUNICATION_LOSS,
  SUPERSEDED_BY_LATER_EXECUTION,
  classifyJobFailure,
  countLostExecutions,
  planAcceleratorNonClaim,
  planLostRunnerRerun,
  planRerunDispatch,
} from "./lost-runner-recovery.mjs";

const script = fileURLToPath(new URL("./lost-runner-recovery.mjs", import.meta.url));

function lostJob(overrides = {}) {
  return {
    id: 41,
    name: "linux-vulkan-proof / Packaged Linux Vulkan engine",
    status: "completed",
    conclusion: "failure",
    run_attempt: "1",
    log_uploaded: false,
    annotations: [{ level: "failure", message: LOST_RUNNER_ANNOTATION }],
    steps: [
      { name: "Checkout exact source", status: "completed", conclusion: "success" },
      { name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: null },
      { name: "Upload Linux Vulkan proof artifacts", status: "completed", conclusion: null },
    ],
    ...overrides,
  };
}

function assertionJob(overrides = {}) {
  return {
    id: 42,
    name: "linux-vulkan-proof / Packaged Linux Vulkan engine",
    status: "completed",
    conclusion: "failure",
    run_attempt: "1",
    log_uploaded: true,
    annotations: [{ level: "failure", message: "Process completed with exit code 1." }],
    steps: [
      { name: "Checkout exact source", status: "completed", conclusion: "success" },
      { name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: "failure" },
    ],
    ...overrides,
  };
}

const linuxHost = { id: "linux-x64-vulkan", job_name: "Packaged Linux Vulkan engine" };

function runCli(command, input) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-lost-runner-"));
  const inputPath = path.join(directory, "input.json");
  const outPath = path.join(directory, "plan.json");
  const outputsPath = path.join(directory, "outputs.txt");
  writeFileSync(inputPath, JSON.stringify(input));
  writeFileSync(outputsPath, "");
  const result = spawnSync(
    process.execPath,
    [script, command, "--input", inputPath, "--out", outPath],
    { encoding: "utf8", env: { ...process.env, GITHUB_OUTPUT: outputsPath } },
  );
  return {
    status: result.status,
    stderr: result.stderr,
    plan: JSON.parse(readFileSync(outPath, "utf8")),
    outputs: readFileSync(outputsPath, "utf8"),
  };
}

test("the lost-runner verdict needs the whole signature, not any one part of it", () => {
  assert.equal(classifyJobFailure(lostJob()).signature, RUNNER_COMMUNICATION_LOSS);

  // Each single-part removal must fall back to an assertion failure. Any one of these alone is
  // reachable without a lost runner, so a partial match must never unlock a retry.
  assert.equal(
    classifyJobFailure(lostJob({ annotations: [{ message: "Process completed with exit code 1." }] })).signature,
    JOB_ASSERTION_FAILURE,
  );
  assert.equal(
    classifyJobFailure(lostJob({
      steps: [{ name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: "failure" }],
    })).signature,
    JOB_ASSERTION_FAILURE,
  );
  assert.equal(classifyJobFailure(lostJob({ log_uploaded: true })).signature, JOB_ASSERTION_FAILURE);

  // A near-miss annotation is not the annotation.
  assert.equal(
    classifyJobFailure(lostJob({
      annotations: [{ message: `${LOST_RUNNER_ANNOTATION} Retrying.` }],
    })).signature,
    JOB_ASSERTION_FAILURE,
  );
  // Surrounding whitespace in the Actions payload is not meaningful.
  assert.equal(
    classifyJobFailure(lostJob({ annotations: [{ message: `\n${LOST_RUNNER_ANNOTATION}\n` }] })).signature,
    RUNNER_COMMUNICATION_LOSS,
  );

  // The verdict is reached without ever reading the job name.
  assert.equal(
    classifyJobFailure(lostJob({ name: "some-unrelated-job / Brand new proof" })).signature,
    RUNNER_COMMUNICATION_LOSS,
  );
  assert.equal(
    classifyJobFailure(assertionJob({ name: "linux-vulkan-proof / Packaged Linux Vulkan engine" })).signature,
    JOB_ASSERTION_FAILURE,
  );
});

/// The same host lost again on a later attempt: a *second* execution of the same job name, which is
/// what actually spends the one automatic recovery.
function lostAgain(attempt = MAXIMUM_RUN_ATTEMPTS) {
  return lostJob({ id: 40 + attempt, run_attempt: String(attempt) });
}

test("only lost jobs are re-dispatched and the recovery bound counts recoveries", () => {
  const lost = planLostRunnerRerun({
    runAttempt: 1,
    runConclusion: "failure",
    jobs: [lostJob(), { id: 9, name: "Release / Workflow policy", conclusion: "success", run_attempt: "1" }],
  });
  assert.equal(lost.rerun, true);
  assert.equal(lost.reason, "runner_communication_loss");
  assert.deepEqual(lost.rerun_job_ids, [41]);
  assert.equal(lost.maximum_run_attempts, MAXIMUM_RUN_ATTEMPTS);
  assert.deepEqual(lost.lost_jobs.map(({ lost_executions: spent }) => spent), [1]);

  // An assertion failure alongside a lost runner is reported and left untouched: it is not in the
  // re-dispatch list, so the rerun cannot turn it green.
  const mixed = planLostRunnerRerun({
    runAttempt: 1,
    runConclusion: "failure",
    jobs: [lostJob(), assertionJob({ id: 77, name: "windows-vulkan-proof / Packaged Windows Vulkan engine" })],
  });
  assert.deepEqual(mixed.rerun_job_ids, [41]);
  assert.deepEqual(mixed.not_retried_jobs.map(({ id }) => id), [77]);

  // A run whose only failure is an assertion failure is never re-dispatched.
  const assertionOnly = planLostRunnerRerun({
    runAttempt: 1,
    runConclusion: "failure",
    jobs: [assertionJob()],
  });
  assert.equal(assertionOnly.rerun, false);
  assert.equal(assertionOnly.reason, "no_runner_communication_loss");
  assert.deepEqual(assertionOnly.rerun_job_ids, []);

  // The bound is two lost executions of the same job: the second loss gets no third try. The
  // decision is reported against the execution that is current -- the attempt-1 execution the
  // re-dispatch already consumed is reported as superseded, never as a second pending decision.
  const bounded = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [lostJob(), lostAgain()],
  });
  assert.equal(bounded.rerun, false);
  assert.equal(bounded.reason, "recovery_bound_reached");
  assert.deepEqual(bounded.lost_jobs.map(({ id }) => id), [42]);
  assert.deepEqual(bounded.lost_jobs.map(({ retry_decision: decision }) => decision), [
    RECOVERY_BOUND_REACHED,
  ]);
  assert.deepEqual(bounded.lost_jobs.map(({ lost_executions: spent }) => spent), [2]);
  assert.deepEqual(bounded.not_retried_jobs.map(({ id, retry_decision: decision }) => [id, decision]), [
    [41, SUPERSEDED_BY_LATER_EXECUTION],
  ]);

  // A run that reached attempt 2 for an unrelated reason has still never re-dispatched this host,
  // and the first loss of its runner is owed its one automatic recovery. Reading the run-attempt
  // number as a recovery counter refused the retry here and withheld the claim with zero recoveries.
  const rerunForOtherReasons = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [
      { id: 9, name: "Release / Workflow policy", conclusion: "success", run_attempt: "1" },
      lostJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) }),
    ],
  });
  assert.equal(rerunForOtherReasons.rerun, true);
  assert.equal(rerunForOtherReasons.reason, "runner_communication_loss");
  assert.deepEqual(rerunForOtherReasons.rerun_job_ids, [41]);
  assert.deepEqual(rerunForOtherReasons.lost_jobs.map(({ lost_executions: spent }) => spent), [1]);

  // A job Actions carried forward unchanged is listed by every later attempt under the same id.
  // Counting those listings would spend a recovery that never happened.
  const carriedForward = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [lostJob(), lostJob()],
  });
  assert.equal(carriedForward.rerun, true);
  assert.deepEqual(carriedForward.lost_jobs.map(({ lost_executions: spent }) => spent), [1]);

  // A green run is never re-dispatched even if a prior attempt left failure rows behind.
  const green = planLostRunnerRerun({ runAttempt: 1, runConclusion: "success", jobs: [lostJob()] });
  assert.equal(green.rerun, false);
  assert.equal(green.reason, "run_did_not_fail");
});

// The Metal host is a second name in the same run, used wherever the point is that one host's
// decision must not be read off another host's rows.
function metalLostJob(overrides = {}) {
  return lostJob({
    id: 51,
    name: "macos-metal-proof / Packaged Apple Silicon Metal engine",
    ...overrides,
  });
}

test("a job a later execution already recovered is out of the plan, and its stale id is never named", () => {
  // The recovery worked: the host was lost at attempt 1 and reported at attempt 2. Another job
  // then failed its own assertions, so the run as a whole is red and the planner runs again.
  const recovered = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [
      lostJob(),
      lostJob({
        id: 43,
        run_attempt: String(MAXIMUM_RUN_ATTEMPTS),
        conclusion: "success",
        annotations: [],
        log_uploaded: true,
        steps: [{ name: "Prove offline Linux Vulkan retrieval", conclusion: "success" }],
      }),
      assertionJob({ id: 77, name: "windows-vulkan-proof / Packaged Windows Vulkan engine" }),
    ],
  });
  assert.equal(recovered.rerun, false);
  assert.equal(recovered.reason, "no_runner_communication_loss");
  assert.deepEqual(recovered.rerun_job_ids, []);
  assert.deepEqual(recovered.lost_jobs, []);
  const stale = recovered.not_retried_jobs.find(({ id }) => id === 41);
  assert.equal(stale.retry_decision, SUPERSEDED_BY_LATER_EXECUTION);
  assert.deepEqual(stale.superseded_by, { id: 43, run_attempt: "2", conclusion: "success" });
  // The assertion failure that made the run red is still reported, and still not retried.
  const refused = recovered.not_retried_jobs.find(({ id }) => id === 77);
  assert.equal(refused.retry_decision, NOT_A_RUNNER_COMMUNICATION_LOSS);
  assert.equal(refused.signature, JOB_ASSERTION_FAILURE);

  // Actions also lists a carried-forward job under the newer attempt number, so the attempt alone
  // cannot order two executions of one name. Job ids are allocated in creation order inside a run,
  // so the execution the re-dispatch created is the higher id even when both read attempt 2.
  const sameAttemptListing = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [
      lostJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) }),
      lostJob({
        id: 43,
        run_attempt: String(MAXIMUM_RUN_ATTEMPTS),
        conclusion: "success",
        annotations: [],
        log_uploaded: true,
        steps: [{ name: "Prove offline Linux Vulkan retrieval", conclusion: "success" }],
      }),
      assertionJob({ id: 77, name: "windows-vulkan-proof / Packaged Windows Vulkan engine" }),
    ],
  });
  assert.equal(sameAttemptListing.rerun, false);
  assert.deepEqual(sameAttemptListing.rerun_job_ids, []);
  assert.equal(
    sameAttemptListing.not_retried_jobs.find(({ id }) => id === 41).retry_decision,
    SUPERSEDED_BY_LATER_EXECUTION,
  );

  // The host was lost, re-dispatched, and then disagreed with the product on its own terms. The
  // stale lost execution must not put the assertion failure back into the queue: a proof that ran
  // and refused to pass is never re-dispatched, whatever an earlier execution of it looked like.
  const recoveredIntoAssertionFailure = planLostRunnerRerun({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    runConclusion: "failure",
    jobs: [lostJob(), assertionJob({ id: 43, run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
  });
  assert.equal(recoveredIntoAssertionFailure.rerun, false);
  assert.equal(recoveredIntoAssertionFailure.reason, "no_runner_communication_loss");
  assert.deepEqual(recoveredIntoAssertionFailure.rerun_job_ids, []);
  assert.deepEqual(
    recoveredIntoAssertionFailure.not_retried_jobs
      .map(({ id, retry_decision: decision }) => [id, decision]),
    [[41, SUPERSEDED_BY_LATER_EXECUTION], [43, NOT_A_RUNNER_COMMUNICATION_LOSS]],
  );

  // Two hosts lost in the same incident are both owed a recovery, and the request names the
  // execution each of them is actually sitting in.
  const both = planLostRunnerRerun({
    runAttempt: 1,
    runConclusion: "failure",
    jobs: [lostJob(), metalLostJob()],
  });
  assert.equal(both.rerun, true);
  assert.deepEqual(both.rerun_job_ids, [41, 51]);
  assert.deepEqual(both.lost_jobs.map(({ retry_decision: decision }) => decision), [
    RETRY_REQUESTED,
    RETRY_REQUESTED,
  ]);
  assert.equal(both.schema, RERUN_PLAN_SCHEMA);
});

test("one refused re-dispatch does not consume the recovery the other lost hosts are owed", () => {
  const attempted = [];
  const partial = planRerunDispatch({
    jobIds: [41, 51],
    dispatch: (jobId) => {
      attempted.push(jobId);
      return jobId === 41
        ? { dispatched: false, detail: "refused_exit_1: HTTP 403" }
        : { dispatched: true, detail: "accepted" };
    },
  });
  // Every named id got its own request: the refusal of the first did not end the loop.
  assert.deepEqual(attempted, [41, 51]);
  assert.equal(partial.schema, RERUN_DISPATCH_SCHEMA);
  assert.equal(partial.recovered, true);
  assert.equal(partial.reason, "partially_dispatched");
  assert.deepEqual(partial.dispatched_job_ids, [51]);
  assert.deepEqual(partial.refused_job_ids, [41]);
  assert.equal(partial.results.find(({ job_id: id }) => id === 41).detail, "refused_exit_1: HTTP 403");

  const none = planRerunDispatch({
    jobIds: [41, 51],
    dispatch: () => ({ dispatched: false, detail: "refused_exit_1: HTTP 403" }),
  });
  assert.equal(none.recovered, false);
  assert.equal(none.reason, "every_job_refused");
  assert.deepEqual(none.dispatched_job_ids, []);

  const all = planRerunDispatch({
    jobIds: [41, 51],
    dispatch: () => ({ dispatched: true, detail: "accepted" }),
  });
  assert.equal(all.recovered, true);
  assert.equal(all.reason, "every_job_dispatched");
});

function stubbedGh(script) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-gh-stub-"));
  const binary = path.join(directory, "gh");
  writeFileSync(binary, script, { mode: 0o755 });
  return { directory, log: path.join(directory, "calls.txt") };
}

function runDispatchCli(plan, ghScript) {
  const { directory, log } = stubbedGh(ghScript);
  const planPath = path.join(directory, "rerun-plan.json");
  const outPath = path.join(directory, "rerun-dispatch.json");
  const outputsPath = path.join(directory, "outputs.txt");
  writeFileSync(planPath, JSON.stringify(plan));
  writeFileSync(outputsPath, "");
  const result = spawnSync(
    process.execPath,
    [
      script,
      "dispatch-rerun",
      "--plan", planPath,
      "--repository", "TheGreenCedar/CodeStory",
      "--out", outPath,
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${directory}${path.delimiter}${process.env.PATH}`,
        GITHUB_OUTPUT: outputsPath,
        CODESTORY_GH_STUB_LOG: log,
      },
    },
  );
  return {
    status: result.status,
    stderr: result.stderr,
    receipt: existsSync(outPath) ? JSON.parse(readFileSync(outPath, "utf8")) : undefined,
    outputs: readFileSync(outputsPath, "utf8"),
    calls: existsSync(log) ? readFileSync(log, "utf8").trim().split("\n") : [],
  };
}

test("the dispatch CLI asks Actions for every lost job separately and records what each answered", () => {
  const plan = {
    schema: RERUN_PLAN_SCHEMA,
    rerun: true,
    reason: RUNNER_COMMUNICATION_LOSS,
    rerun_job_ids: [41, 51],
  };
  // The first id is refused the way Actions refuses a job it will not re-run; the second must
  // still be asked for, and the recovery as a whole must count as having happened.
  const partial = runDispatchCli(
    plan,
    [
      "#!/usr/bin/env bash",
      'printf "%s\\n" "$*" >> "$CODESTORY_GH_STUB_LOG"',
      'case "$*" in',
      '  *"/jobs/41/rerun"*) echo "HTTP 403: cannot re-run this job" >&2; exit 1 ;;',
      '  *) echo "{}" ;;',
      "esac",
      "",
    ].join("\n"),
  );
  assert.equal(partial.status, 0);
  // The per-job endpoint, named id by id: re-running every failed job would sweep an assertion
  // failure back into the queue alongside the lost one.
  assert.deepEqual(partial.calls, [
    "api --method POST repos/TheGreenCedar/CodeStory/actions/jobs/41/rerun",
    "api --method POST repos/TheGreenCedar/CodeStory/actions/jobs/51/rerun",
  ]);
  assert.equal(partial.receipt.recovered, true);
  assert.deepEqual(partial.receipt.dispatched_job_ids, [51]);
  assert.match(partial.receipt.results.find(({ job_id: id }) => id === 41).detail, /HTTP 403/u);
  assert.match(partial.outputs, /recovered=true/u);
  assert.match(partial.outputs, /dispatched_job_ids=51/u);

  // Nothing was recovered, so the recovery workflow itself has to go red.
  const refused = runDispatchCli(
    plan,
    ["#!/usr/bin/env bash", 'printf "%s\\n" "$*" >> "$CODESTORY_GH_STUB_LOG"', "exit 1", ""].join("\n"),
  );
  assert.equal(refused.status, 1);
  assert.equal(refused.calls.length, 2);
  assert.equal(refused.receipt.recovered, false);
  assert.equal(refused.receipt.reason, "every_job_refused");

  // A plan that did not ask for a rerun can never be turned into re-dispatch requests.
  const notPlanned = runDispatchCli(
    { ...plan, rerun: false, reason: "no_runner_communication_loss", rerun_job_ids: [] },
    ["#!/usr/bin/env bash", 'printf "%s\\n" "$*" >> "$CODESTORY_GH_STUB_LOG"', "echo '{}'", ""].join("\n"),
  );
  assert.notEqual(notPlanned.status, 0);
  assert.deepEqual(notPlanned.calls, []);
  assert.match(notPlanned.stderr, /did not ask for a rerun/u);

  // A plan written by an older contract is refused rather than reinterpreted.
  const staleSchema = runDispatchCli(
    { ...plan, schema: "codestory.lost-runner-rerun-plan/v1" },
    ["#!/usr/bin/env bash", 'printf "%s\\n" "$*" >> "$CODESTORY_GH_STUB_LOG"', "echo '{}'", ""].join("\n"),
  );
  assert.notEqual(staleSchema.status, 0);
  assert.deepEqual(staleSchema.calls, []);
});

test("a non-claim is reachable only from a spent retry bound on a lost runner", () => {
  const proven = planAcceleratorNonClaim({
    runAttempt: 1,
    hosts: [linuxHost],
    jobs: [lostJob({ conclusion: "success", steps: [], annotations: [], log_uploaded: true })],
  });
  assert.deepEqual(proven.hosts.map(({ state }) => state), ["proven"]);
  assert.deepEqual(proven.withheld_hosts, []);
  assert.deepEqual(proven.blocked_hosts, []);

  // Attempts still owed: the run must be re-dispatched before anything may be withheld.
  const pending = planAcceleratorNonClaim({ runAttempt: 1, hosts: [linuxHost], jobs: [lostJob()] });
  assert.deepEqual(pending.hosts.map(({ state }) => state), ["retry_pending"]);
  assert.deepEqual(pending.withheld_hosts, []);
  assert.deepEqual(pending.blocked_hosts, ["linux-x64-vulkan"]);

  const withheld = planAcceleratorNonClaim({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [lostJob(), lostAgain()],
  });
  assert.deepEqual(withheld.hosts.map(({ state }) => state), ["withheld"]);
  assert.deepEqual(withheld.withheld_hosts, ["linux-x64-vulkan"]);
  assert.deepEqual(withheld.blocked_hosts, []);
  assert.equal(withheld.hosts[0].lost_executions, MAXIMUM_RUN_ATTEMPTS);

  // Withholding is the end of the bounded recovery path, never a shortcut around it. A release
  // sitting at attempt 2 for an unrelated reason has spent no recovery on this host, so its first
  // lost runner is owed one -- the earlier code withheld the claim here with zero recoveries.
  const firstLossOnAReRunRelease = planAcceleratorNonClaim({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [lostJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
  });
  assert.deepEqual(firstLossOnAReRunRelease.hosts.map(({ state }) => state), ["retry_pending"]);
  assert.equal(firstLossOnAReRunRelease.hosts[0].lost_executions, 1);
  assert.deepEqual(firstLossOnAReRunRelease.withheld_hosts, []);
  assert.deepEqual(firstLossOnAReRunRelease.blocked_hosts, ["linux-x64-vulkan"]);
  // The two halves agree: what the non-claim refuses to withhold, the rerun plan agrees to retry.
  assert.equal(
    planLostRunnerRerun({
      runAttempt: MAXIMUM_RUN_ATTEMPTS,
      runConclusion: "failure",
      jobs: [lostJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
    }).rerun,
    true,
  );

  // The proof ran and refused to pass: exhausting attempts must not convert that into a non-claim.
  const assertionFailure = planAcceleratorNonClaim({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [assertionJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
  });
  assert.deepEqual(assertionFailure.hosts.map(({ state }) => state), ["blocked"]);
  assert.equal(assertionFailure.hosts[0].detail, JOB_ASSERTION_FAILURE);
  assert.deepEqual(assertionFailure.withheld_hosts, []);

  // A cancelled proof and a proof that never ran are both blocked, never withheld.
  for (const jobs of [
    [lostJob({ conclusion: "cancelled", run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
    [],
  ]) {
    const blocked = planAcceleratorNonClaim({
      runAttempt: MAXIMUM_RUN_ATTEMPTS,
      hosts: [linuxHost],
      jobs,
    });
    assert.deepEqual(blocked.withheld_hosts, []);
    assert.deepEqual(blocked.blocked_hosts, ["linux-x64-vulkan"]);
  }
});

// ── The shell collector ─────────────────────────────────────────────────────────────────────

const collector = fileURLToPath(new URL("./collect-actions-job-evidence.sh", import.meta.url));

const collectorJobs = {
  jobs: [
    {
      id: 41,
      name: "linux-vulkan-proof / Packaged Linux Vulkan engine",
      status: "completed",
      conclusion: "failure",
      run_attempt: 1,
      steps: [
        { name: "Checkout exact source", status: "completed", conclusion: "success" },
        { name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: null },
      ],
    },
    {
      id: 42,
      name: "Release / Workflow policy",
      status: "completed",
      conclusion: "success",
      run_attempt: 1,
      steps: [{ name: "Enforce workflow policy", status: "completed", conclusion: "success" }],
    },
  ],
};

/// A `gh api` stand-in faithful enough to answer the three questions the collector asks, including
/// the ones it must refuse to answer. `annotations` and `logs` each take an HTTP status; the stub
/// reproduces gh's real behaviour for it -- a non-2xx exits 1 and prints the status line only when
/// `--include` was passed, which is exactly how the collector distinguishes "no log" from "no
/// answer".
function runCollector({
  annotationsStatus = 200,
  logsStatus = 404,
  runAttempt = 1,
  jobsByAttempt = { 1: collectorJobs },
} = {}) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-collect-"));
  const bin = path.join(directory, "bin");
  mkdirSync(bin);
  for (const [attempt, listing] of Object.entries(jobsByAttempt)) {
    writeFileSync(path.join(directory, `jobs-${attempt}.json`), JSON.stringify(listing));
  }
  const annotations = JSON.stringify([
    { annotation_level: "failure", message: LOST_RUNNER_ANNOTATION },
  ]);
  writeFileSync(path.join(bin, "gh"), `#!/bin/sh
include=0
for arg in "$@"; do
  case "$arg" in --include|-i) include=1 ;; esac
done
answer() {
  status="$1"
  body="$2"
  case "$status" in
    2*) [ "$include" = 1 ] && printf 'HTTP/2.0 %s OK\\r\\n\\r\\n' "$status"
        [ -n "$body" ] && printf '%s\\n' "$body"
        exit 0 ;;
    *)  [ "$include" = 1 ] && printf 'HTTP/2.0 %s Refused\\r\\n\\r\\n' "$status"
        echo "gh: HTTP $status" >&2
        exit 1 ;;
  esac
}
requested_attempt() {
  for arg in "$@"; do
    case "$arg" in
      *"/attempts/"*"/jobs"*)
        printf '%s' "$arg" | sed -e 's|.*/attempts/||' -e 's|/jobs.*||'
        return ;;
    esac
  done
}
case "$*" in
  *"/actions/runs/"*"/jobs"*)
    listing='${directory}/jobs-'"$(requested_attempt "$@")"'.json'
    [ -f "$listing" ] || { echo "gh: no such attempt" >&2; exit 1; }
    jq -c '.jobs[]' "$listing" ;;
  *"/check-runs/"*"/annotations"*) answer '${annotationsStatus}' '${annotations}' ;;
  *"/actions/jobs/"*"/logs"*) answer '${logsStatus}' '' ;;
  *) printf '[]\\n' ;;
esac
`, { mode: 0o755 });
  const output = path.join(directory, "evidence.json");
  const collected = spawnSync("bash", [collector, "7", String(runAttempt), output], {
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      GITHUB_REPOSITORY: "TheGreenCedar/CodeStory",
    },
  });
  return {
    status: collected.status,
    stderr: collected.stderr,
    rows: existsSync(output) ? JSON.parse(readFileSync(output, "utf8")) : null,
  };
}

test("the shell collector hands the classifier the whole signature", () => {
  // The three signature parts live in three different Actions endpoints, so the collector is the
  // only place they are joined. A collector that dropped one of them would silently turn every
  // lost job into an assertion failure, which no unit test of the classifier can catch.
  const collected = runCollector();
  assert.equal(collected.status, 0, collected.stderr);
  assert.equal(collected.rows.length, 2);
  const plan = planLostRunnerRerun({ runAttempt: 1, runConclusion: "failure", jobs: collected.rows });
  assert.equal(plan.rerun, true);
  assert.deepEqual(plan.rerun_job_ids, [41]);
  assert.equal(plan.lost_jobs[0].evidence.log_uploaded, false);
  assert.equal(plan.lost_jobs[0].evidence.annotation_matched, true);
  assert.deepEqual(plan.lost_jobs[0].evidence.empty_conclusion_steps, [
    "Prove offline Linux Vulkan retrieval",
  ]);
  // The probe result is recorded, so a reader can see which answer produced the verdict.
  const failed = collected.rows.find(({ id }) => id === 41);
  assert.deepEqual(failed.evidence_probe, { annotations_read: true, log_http_status: 404 });
});

test("a failed job that did upload its log is an assertion failure, not a lost runner", () => {
  // The permissive direction of the log probe. Everything else about job 41 matches the lost-runner
  // signature exactly; only the uploaded log separates it from one, so this is the branch that
  // keeps an ordinary red proof out of the retry-and-withhold lane.
  const collected = runCollector({ logsStatus: 200 });
  assert.equal(collected.status, 0, collected.stderr);
  const failed = collected.rows.find(({ id }) => id === 41);
  assert.equal(failed.log_uploaded, true);
  assert.deepEqual(failed.evidence_probe, { annotations_read: true, log_http_status: 200 });

  const plan = planLostRunnerRerun({ runAttempt: 1, runConclusion: "failure", jobs: collected.rows });
  assert.equal(plan.rerun, false);
  assert.equal(plan.reason, "no_runner_communication_loss");
  assert.deepEqual(plan.rerun_job_ids, []);
  assert.deepEqual(plan.not_retried_jobs.map(({ signature }) => signature), [JOB_ASSERTION_FAILURE]);

  // And it can never become a withheld claim, however many attempts are spent on it.
  const nonClaim = planAcceleratorNonClaim({
    runAttempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: collected.rows,
  });
  assert.deepEqual(nonClaim.withheld_hosts, []);
  assert.deepEqual(nonClaim.blocked_hosts, ["linux-x64-vulkan"]);
  assert.equal(nonClaim.hosts[0].detail, JOB_ASSERTION_FAILURE);
});

test("the collector reads every attempt so the recovery bound counts recoveries", () => {
  // The recovery counter needs history the current attempt alone does not have. Attempt 1 lost the
  // Linux host; attempt 2 re-executed it (a new job id) and lost it again, and also lists the
  // policy job Actions carried forward unchanged under its original id.
  const lostAt = (id, attempt) => ({
    id,
    name: "linux-vulkan-proof / Packaged Linux Vulkan engine",
    status: "completed",
    conclusion: "failure",
    run_attempt: attempt,
    steps: [
      { name: "Checkout exact source", status: "completed", conclusion: "success" },
      { name: "Prove offline Linux Vulkan retrieval", status: "completed", conclusion: null },
    ],
  });
  const carriedForward = {
    id: 42,
    name: "Release / Workflow policy",
    status: "completed",
    conclusion: "success",
    run_attempt: 1,
    steps: [{ name: "Enforce workflow policy", status: "completed", conclusion: "success" }],
  };
  const collected = runCollector({
    runAttempt: 2,
    jobsByAttempt: {
      1: { jobs: [lostAt(41, 1), carriedForward] },
      2: { jobs: [lostAt(43, 2), carriedForward] },
    },
  });
  assert.equal(collected.status, 0, collected.stderr);
  // Three executions, not four: the carried-forward job is listed by both attempts under one id.
  assert.deepEqual(collected.rows.map(({ id }) => id), [41, 42, 43]);

  assert.equal(countLostExecutions(collected.rows, "Packaged Linux Vulkan engine"), 2);
  const plan = planLostRunnerRerun({ runAttempt: 2, runConclusion: "failure", jobs: collected.rows });
  assert.equal(plan.rerun, false);
  assert.equal(plan.reason, "recovery_bound_reached");
  const nonClaim = planAcceleratorNonClaim({
    runAttempt: 2,
    hosts: [linuxHost],
    jobs: collected.rows,
  });
  assert.deepEqual(nonClaim.withheld_hosts, ["linux-x64-vulkan"]);
  assert.equal(nonClaim.hosts[0].lost_executions, 2);

  // A host that succeeded on attempt 1 and was not re-executed is still `proven`, whether or not
  // Actions carries it into the attempt-2 listing. Reading only the current attempt would report
  // `job_absent_from_run` for every healthy host and block the recovery a second way.
  const macos = {
    id: 44,
    name: "macos-metal-proof / Packaged Apple Silicon Metal engine",
    status: "completed",
    conclusion: "success",
    run_attempt: 1,
    steps: [{ name: "Prove Metal retrieval", status: "completed", conclusion: "success" }],
  };
  const onlyTheLostJobRetried = runCollector({
    runAttempt: 2,
    jobsByAttempt: {
      1: { jobs: [lostAt(41, 1), macos, carriedForward] },
      2: { jobs: [lostAt(43, 2)] },
    },
  });
  assert.equal(onlyTheLostJobRetried.status, 0, onlyTheLostJobRetried.stderr);
  assert.deepEqual(
    planAcceleratorNonClaim({
      runAttempt: 2,
      hosts: [linuxHost, { id: "macos-arm64-metal", job_name: "Packaged Apple Silicon Metal engine" }],
      jobs: onlyTheLostJobRetried.rows,
    }).hosts.map(({ host, state }) => [host, state]),
    [["linux-x64-vulkan", "withheld"], ["macos-arm64-metal", "proven"]],
  );

  // The same run with only one loss so far still owes a recovery, and is refused a non-claim.
  const onlyOnce = runCollector({
    runAttempt: 2,
    jobsByAttempt: {
      1: { jobs: [carriedForward] },
      2: { jobs: [lostAt(43, 2), carriedForward] },
    },
  });
  assert.equal(onlyOnce.status, 0, onlyOnce.stderr);
  assert.equal(countLostExecutions(onlyOnce.rows, "Packaged Linux Vulkan engine"), 1);
  assert.equal(
    planAcceleratorNonClaim({ runAttempt: 2, hosts: [linuxHost], jobs: onlyOnce.rows })
      .hosts[0].state,
    "retry_pending",
  );
  assert.equal(
    planLostRunnerRerun({ runAttempt: 2, runConclusion: "failure", jobs: onlyOnce.rows }).rerun,
    true,
  );
});

test("the collector refuses an answer it did not get, rather than reporting an absence", () => {
  // A 403 on the annotations endpoint is what a token without `checks: read` produces. Reporting
  // it as "this job had no annotations" is the fail-open that made the whole recovery path inert:
  // the signature can never match, so a genuinely lost runner reads as an assertion failure.
  const forbidden = runCollector({ annotationsStatus: 403 });
  assert.equal(forbidden.status, 1);
  assert.equal(forbidden.rows, null);
  assert.match(forbidden.stderr, /Cannot read annotations for job 41/u);
  assert.match(forbidden.stderr, /checks: read/u);

  // The same rule for the log blob: only 404 means "no log was uploaded".
  for (const logsStatus of [403, 429, 500]) {
    const refused = runCollector({ logsStatus });
    assert.equal(refused.status, 1, `logs ${logsStatus}`);
    assert.equal(refused.rows, null, `logs ${logsStatus}`);
    assert.match(refused.stderr, /Log blob probe for job 41 answered/u);
  }
});

test("an evidence row without an established log_uploaded fact is refused", () => {
  // The collector is the only thing that can know this, so the classifier must not invent it. An
  // absent field used to read as `false` -- the half of the signature a lost runner needs.
  const { log_uploaded: _dropped, ...withoutProbe } = lostJob();
  assert.throws(() => classifyJobFailure(withoutProbe), /log_uploaded must be a boolean/u);
  assert.throws(() => classifyJobFailure(lostJob({ log_uploaded: "false" })), /must be a boolean/u);
  assert.throws(
    () => planAcceleratorNonClaim({
      runAttempt: MAXIMUM_RUN_ATTEMPTS,
      hosts: [linuxHost],
      jobs: [withoutProbe],
    }),
    /log_uploaded must be a boolean/u,
  );
  // Annotations are the same: an absent list is an unread endpoint, not an empty one.
  const { annotations: _dropped2, ...withoutAnnotations } = lostJob();
  assert.throws(() => classifyJobFailure(withoutAnnotations), /annotations must be an array/u);
});

test("the CLI fails closed for every host it cannot decide", () => {
  const rerun = runCli("plan-rerun", {
    run_attempt: 1,
    conclusion: "failure",
    jobs: [lostJob()],
  });
  assert.equal(rerun.status, 0);
  assert.equal(rerun.plan.rerun, true);
  assert.match(rerun.outputs, /^rerun=true$/mu);
  assert.match(rerun.outputs, /^job_ids=41$/mu);

  const refused = runCli("plan-rerun", {
    run_attempt: 1,
    conclusion: "failure",
    jobs: [assertionJob()],
  });
  assert.equal(refused.status, 0);
  assert.equal(refused.plan.rerun, false);
  assert.match(refused.outputs, /^rerun=false$/mu);
  assert.match(refused.outputs, /^job_ids=$/mu);

  const withheld = runCli("plan-non-claim", {
    run_attempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [lostJob(), lostAgain()],
  });
  assert.equal(withheld.status, 0);
  assert.match(withheld.outputs, /^withheld_hosts=linux-x64-vulkan$/mu);

  // One loss on a run that reached attempt 2 for its own reasons is still owed a recovery, so the
  // CLI refuses to withhold and exits non-zero rather than recording an unearned non-claim.
  const owed = runCli("plan-non-claim", {
    run_attempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [lostJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
  });
  assert.equal(owed.status, 1);
  assert.match(owed.stderr, /automatic_rerun_still_owed/u);
  assert.match(owed.outputs, /^withheld_hosts=$/mu);

  const blocked = runCli("plan-non-claim", {
    run_attempt: MAXIMUM_RUN_ATTEMPTS,
    hosts: [linuxHost],
    jobs: [assertionJob({ run_attempt: String(MAXIMUM_RUN_ATTEMPTS) })],
  });
  assert.equal(blocked.status, 1);
  assert.match(blocked.stderr, /job_assertion_failure/u);
  assert.match(blocked.outputs, /^withheld_hosts=$/mu);
});
