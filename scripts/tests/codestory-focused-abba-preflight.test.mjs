import assert from "node:assert/strict";
import test from "node:test";

import {
  ARMS,
  REQUIRED_TASK_IDS,
  abbaRunPlan,
  focusedAbbaReceiptTimingClaims,
  focusedAbbaTiming,
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

test("focused timing preflight gives paired arms the same cohort id", () => {
  const raw = {
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
