import assert from "node:assert/strict";
import test from "node:test";

import {
  ARMS,
  REQUIRED_TASK_IDS,
  abbaRunPlan,
  focusedAbbaReceiptTimingClaims,
  focusedAbbaTiming,
  focusedAbbaRawRowBlockers,
  validateFocusedAbbaRows,
  transientEmbeddingServerTransition,
} from "../codestory-focused-abba-preflight.mjs";

test("focused timing preflight schedules five paired ABBA rows per arm and task", () => {
  const plan = abbaRunPlan();
  assert.equal(plan.length, 40);
  for (const taskId of REQUIRED_TASK_IDS) {
    const rows = plan.filter((row) => row.task_id === taskId);
    assert.deepEqual(rows.slice(0, 4).map((row) => row.arm), [
      "published_0_17_5",
      "candidate_0_18",
      "candidate_0_18",
      "published_0_17_5",
    ]);
    assert.deepEqual(rows.map((row) => row.arm), [
      "published_0_17_5",
      "candidate_0_18",
      "candidate_0_18",
      "published_0_17_5",
      "published_0_17_5",
      "candidate_0_18",
      "candidate_0_18",
      "published_0_17_5",
      "published_0_17_5",
      "candidate_0_18",
    ]);
    for (const arm of ARMS) {
      assert.deepEqual(
        rows.filter((row) => row.arm === arm).map((row) => row.repeat),
        [1, 2, 3, 4, 5],
      );
    }
  }
});

function timingFixture() {
  const raw = {
    status: "pass",
    quality: { pass: true },
    installed_agent_timing_eligible: true,
    task_id: "dart-http-client-flow",
    arm: "with_codestory",
    repeat: 1,
    agent_runner_wall_ms: 100.2,
    wall_ms: 125.4,
    codestory_harness_prelude: {
      time_to_first_packet_ms: 20.1,
      continuation_ms: 5.1,
    },
    installed_agent_timing: {
      timing_cohort_id: "f".repeat(64),
      agent_runner_ms: 100,
      time_to_first_packet_ms: 20,
      continuation_ms: 5,
      time_to_final_packet_ms: 25,
      whole_task_wall_ms: 125,
    },
  };
  const dimensions = {
    execution_window_id: "window-1",
    host: {
      platform: "darwin",
      arch: "arm64",
      cpu_model: "Apple M5",
      logical_cpu_count: 10,
      total_memory_bytes: 24 * 1024 ** 3,
    },
    model: "gpt-5.6-sol",
    load_policy: "fresh_cli_fresh_agent_session",
    task_id: "dart-http-client-flow",
    repeat: 1,
  };
  return { raw, dimensions };
}

test("focused timing preflight gives paired arms the same cohort id", () => {
  const { raw, dimensions } = timingFixture();
  const published = focusedAbbaTiming(raw, { ...dimensions, arm: "published_0_17_5" });
  const candidate = focusedAbbaTiming(raw, { ...dimensions, arm: "candidate_0_18" });
  assert.equal(published.timing_cohort_id, candidate.timing_cohort_id);
  assert.deepEqual(published, {
    timing_cohort_id: candidate.timing_cohort_id,
    agent_runner_ms: 100,
    time_to_first_packet_ms: 20,
    continuation_ms: 5,
    time_to_final_packet_ms: 25,
    whole_task_wall_ms: 125,
  });
  assert.throws(() => focusedAbbaTiming({ ...raw, status: "fail" }, dimensions), /status.*fail/u);
});

test("focused timing preflight rejects failed incomplete reused and ineligible raw rows with reasons", () => {
  const { raw, dimensions } = timingFixture();
  assert.deepEqual(focusedAbbaRawRowBlockers(raw), []);
  for (const [mutate, reason] of [
    [(row) => { row.status = "fail"; }, /status=fail/u],
    [(row) => { row.status = "cancelled"; }, /status=cancelled/u],
    [(row) => { delete row.status; }, /status=missing/u],
    [(row) => { row.quality.pass = false; }, /quality/u],
    [(row) => { delete row.quality; }, /quality/u],
    [(row) => { row.installed_agent_timing_eligible = false; }, /not eligible/u],
    [(row) => { delete row.installed_agent_timing_eligible; }, /not eligible/u],
    [(row) => { row.comparative_wall_time_eligible = false; }, /comparative/u],
    [(row) => { row.comparator_reuse_provenance = {}; }, /reused/u],
    [(row) => { row.resume_provenance = {}; }, /not fresh/u],
    [(row) => { row.installed_agent_timing_ineligibility_reason = "preparation_overlap"; }, /preparation_overlap/u],
    [(row) => { delete row.installed_agent_timing; }, /missing measured/u],
    [(row) => { row.installed_agent_timing.agent_runner_ms = null; }, /agent_runner_ms/u],
    [(row) => { row.installed_agent_timing.time_to_first_packet_ms = -1; }, /time_to_first_packet_ms/u],
    [(row) => { row.installed_agent_timing.continuation_ms = Number.NaN; }, /continuation_ms/u],
    [(row) => { delete row.installed_agent_timing.whole_task_wall_ms; }, /whole_task_wall_ms/u],
    [(row) => { row.installed_agent_timing.time_to_final_packet_ms += 1; }, /inconsistent/u],
    [(row) => { row.task_id = "other-task"; }, /scheduled task/u],
    [(row) => { row.arm = "native_tools"; }, /single-run cell/u],
    [(row) => { row.repeat = 2; }, /single-run cell/u],
  ]) {
    const row = structuredClone(raw);
    mutate(row);
    assert.throws(() => focusedAbbaTiming(row, dimensions), reason);
  }
  const noPrelude = structuredClone(raw);
  Object.assign(noPrelude.installed_agent_timing, {
    time_to_first_packet_ms: 0, continuation_ms: 0, time_to_final_packet_ms: 0,
  });
  delete noPrelude.codestory_harness_prelude;
  assert.equal(focusedAbbaTiming(noPrelude, dimensions).time_to_final_packet_ms, 0);
});

test("focused timing receipt requires complete unique eligible paired cells", () => {
  const { raw } = timingFixture();
  const plan = abbaRunPlan();
  const rows = plan.map((cell) => ({ ...structuredClone(raw), ...cell }));
  assert.equal(validateFocusedAbbaRows(rows), true);
  assert.throws(() => validateFocusedAbbaRows(rows.slice(1)), /incomplete/u);
  const duplicate = structuredClone(rows);
  duplicate[1] = structuredClone(duplicate[0]);
  assert.throws(() => validateFocusedAbbaRows(duplicate), /duplicate/u);
  const failed = structuredClone(rows);
  failed[1].status = "fail";
  assert.throws(() => validateFocusedAbbaRows(failed), /timing-ineligible.*status=fail/u);
  const wrong = structuredClone(rows);
  wrong[1].task_id = "unexpected";
  assert.throws(() => validateFocusedAbbaRows(wrong), /unexpected paired cell/u);
});

test("focused timing preflight retries only a zero-row embedding-server transition", () => {
  const transition = {
    completed_rows: 0,
    first_failure: {
      kind: "preparation_failed",
      error: "embedding_server_draining: incompatible engine contract",
    },
  };
  assert.equal(transientEmbeddingServerTransition(transition), true);
  assert.equal(
    transientEmbeddingServerTransition({ ...transition, completed_rows: 1 }),
    false,
  );
  assert.equal(
    transientEmbeddingServerTransition({
      ...transition,
      first_failure: { kind: "preparation_failed", error: "retrieval unavailable" },
    }),
    false,
  );
});

test("focused ABBA receipt does not claim an unmeasured persistent MCP cell", () => {
  const claims = focusedAbbaReceiptTimingClaims();
  assert.equal(Object.hasOwn(claims, "persistent_installed_mcp_measured"), false);
  assert.deepEqual(claims.timing_cells_measured, ["fresh_cli_fresh_agent_session"]);
  assert.equal(claims.load_policy, "fresh_cli_fresh_agent_session");
});
