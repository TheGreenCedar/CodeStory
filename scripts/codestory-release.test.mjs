import assert from "node:assert/strict";
import test from "node:test";
import {
  RECEIPT_SCHEMA,
  initReceipt,
  validatePhase,
} from "../.github/scripts/release-driver-receipt.mjs";
import {
  COORDINATOR_SCHEMA,
  SOURCE_PROOF_WORKFLOW,
  RELEASE_WORKFLOW,
  BROAD_WORKFLOWS,
  createTestHost,
  execute,
} from "./codestory-release.mjs";

const C = "c".repeat(40);
const C_TREE = "1".repeat(40);
const F = "f".repeat(40);
const F_TREE = "2".repeat(40);
const NEXT_SHA = C;

function startArgs(extra = []) {
  return ["start", "--version", "0.17.5", "--lane", "native", ...extra];
}

async function started(overrides = {}) {
  const host = createTestHost({
    heads: { next: { commit: C, tree: C_TREE }, main: { commit: "0".repeat(40), tree: "9".repeat(40) } },
    ...overrides,
  });
  const result = await execute(startArgs(overrides.rehearse ? ["--rehearse"] : []), host);
  return { host, result };
}

function runRow(overrides = {}) {
  return {
    id: 501,
    workflow: SOURCE_PROOF_WORKFLOW,
    headSha: C,
    status: "in_progress",
    conclusion: null,
    event: "workflow_dispatch",
    attempt: 1,
    inputs: {
      acceptance_only: "true",
      acceptance_phase: "source_stabilization",
      expected_head_sha: C,
      version: "0.17.5",
    },
    ...overrides,
  };
}

test("start writes a GitHub-backed driver receipt and compact status", async () => {
  const { host, result } = await started();
  assert.equal(result.phase, "preflight");
  assert.equal(result.sha, C);
  assert.equal(result.tree, C_TREE);
  assert.equal(result.blocker, null);
  assert.match(result.next_action, /preflight|source.stabilization/i);
  assert.equal(typeof result.elapsed, "string");
  assert.equal(typeof result.estimated_critical_path, "string");
  assert.equal(result.record.schema, COORDINATOR_SCHEMA);
  assert.equal(result.record.receipt.schema, RECEIPT_SCHEMA);
  assert.equal(result.record.receipt.version, "0.17.5");
  assert.equal(result.record.lane, "native");
  assert.ok(host.issues.length >= 1);
  assert.ok(host.comments.some((comment) => comment.body.includes(COORDINATOR_SCHEMA)));
  assert.equal(result.record.receipt.groups["calibration-source"].value.commit, C);
});

test("status reconstructs the record from GitHub without copied run IDs", async () => {
  const { host } = await started();
  host.runs.push(runRow());
  const status = await execute(["status"], host);
  assert.equal(status.phase, "source_stabilization");
  assert.equal(status.active_runs.length, 1);
  assert.equal(status.active_runs[0].workflow, SOURCE_PROOF_WORKFLOW);
  assert.match(status.next_action, /wait/i);
});

test("advance dispatches source stabilization with an explicit acceptance phase, never the frozen default", async () => {
  const { host } = await started();
  const advanced = await execute(["advance"], host);
  assert.equal(advanced.dispatched.workflow, SOURCE_PROOF_WORKFLOW);
  assert.equal(advanced.dispatched.inputs.acceptance_only, "true");
  assert.equal(advanced.dispatched.inputs.acceptance_phase, "source_stabilization");
  assert.notEqual(advanced.dispatched.inputs.acceptance_phase, "frozen_candidate");
  assert.equal(advanced.dispatched.inputs.expected_head_sha, C);
  const again = await execute(["advance"], host);
  assert.equal(again.dispatched, null);
  assert.match(again.next_action, /wait/i);
});

test("wrong acceptance mode is rejected before dispatch", async () => {
  const { host } = await started({
    probes: { acceptanceMode: "frozen_candidate" },
  });
  await assert.rejects(
    () => execute(["advance"], host),
    /acceptance_phase=source_stabilization/,
  );
  assert.equal(host.dispatches.length, 0);
});

test("expensive qualification cannot dispatch before preflight", async () => {
  const { host } = await started({
    probes: { runners: { "macos-arm64-metal": "unproven" } },
  });
  host.record.phase = "qualification";
  host.persist();
  await assert.rejects(
    () => execute(["advance"], host),
    /preflight|runner|heartbeat|unproven/i,
  );
  assert.equal(host.dispatches.length, 0);
  const status = await execute(["status"], host);
  assert.match(status.blocker, /runner|heartbeat|unproven/i);
  assert.equal(status.next_action.split("\n").length, 1);
});

test("macOS zombie or unreadable identity fails closed with one next action", async () => {
  const { host } = await started({ probes: { macosZombie: "zombie", macosIdentity: "unreadable" } });
  await assert.rejects(() => execute(["advance"], host), /zombie|identity/i);
  const status = await execute(["status"], host);
  assert.match(status.blocker, /zombie|identity/i);
  assert.doesNotMatch(status.next_action, /\n/);
});

test("Windows staging failure fails closed before another package dispatch", async () => {
  const { host } = await started({ probes: { windowsStaging: "fail" } });
  host.forcePhase("package");
  await assert.rejects(() => execute(["advance"], host), /windows|staging|path/i);
  assert.equal(host.dispatches.length, 0);
});

test("source drift cancels superseded work and resumes at the earliest valid phase", async () => {
  const { host } = await started();
  await execute(["advance"], host);
  host.completeActiveRuns();
  const moved = "d".repeat(40);
  host.heads.next = { commit: moved, tree: "3".repeat(40) };
  host.runs.push(runRow({ id: 777, status: "in_progress", headSha: C }));
  const status = await execute(["status"], host);
  assert.equal(status.phase, "preflight");
  assert.ok(host.cancelled.includes(777) || host.freezeBarrierCalls.some((call) => call[0] === "cancel-superseded"));
  assert.ok(status.record.receipt.invalidations.length >= 1);
  assert.match(status.next_action, /source.stabilization|preflight/i);
});

test("ignored ordinary cancellation stops with force-cancel as the next action", async () => {
  const { host } = await started({ cancelHonored: false });
  await execute(["advance"], host);
  const moved = "d".repeat(40);
  host.heads.next = { commit: moved, tree: "3".repeat(40) };
  host.runs.push(runRow({ id: 888, status: "in_progress", headSha: C }));
  const status = await execute(["status"], host);
  assert.match(status.blocker ?? "", /cancel/i);
  assert.match(status.next_action, /invalidate-superseded|force-cancel/i);
  assert.equal(host.dispatches.filter((row) => row.workflow !== SOURCE_PROOF_WORKFLOW).length, 0);
});

test("optional evaluation does not delay a standard native claim", async () => {
  const { host } = await started();
  host.forcePhase("qualification");
  host.runs.push({
    id: 900,
    workflow: "frozen-candidate-quality.yml",
    headSha: F,
    status: "in_progress",
    conclusion: null,
    event: "workflow_dispatch",
    attempt: 1,
    inputs: { qualify_linux_vulkan: "true" },
    optional: true,
  });
  const advanced = await execute(["advance"], host);
  assert.notEqual(advanced.phase, "qualification");
  assert.ok(!advanced.blocker || !/quality|optional|performance/i.test(advanced.blocker));
});

test("qualification success does not publish", async () => {
  const { host } = await started();
  host.forcePhase("qualification");
  host.completeActiveRuns();
  const advanced = await execute(["advance"], host);
  assert.equal(
    host.dispatches.some((row) =>
      row.workflow === RELEASE_WORKFLOW && row.inputs?.publish_release === "true"
    ),
    false,
  );
  assert.equal(advanced.phase, "awaiting_approval");
  assert.match(advanced.next_action, /approval/i);
});

test("plugin-only and native asset expectations are not interchangeable", async () => {
  const native = await started({ lane: "native" });
  native.host.forcePhase("publication");
  native.host.assets = ["plugin-only"];
  await assert.rejects(() => execute(["advance"], native.host), /native|archive|asset/i);

  const plugin = createTestHost({
    heads: { next: { commit: C, tree: C_TREE }, main: { commit: "0".repeat(40), tree: "9".repeat(40) } },
  });
  await execute(["start", "--version", "0.17.5", "--lane", "plugin"], plugin);
  plugin.forcePhase("publication");
  plugin.assets = ["linux-x64", "macos-arm64", "windows-x64"];
  const status = await execute(["status"], plugin);
  assert.match(status.blocker ?? status.next_action, /plugin|lane|asset/i);
});

test("deferred marketplace is an honest closeout state, not published", async () => {
  const { host } = await started({ rehearse: true });
  host.forcePhase("closeout");
  host.marketplace = { state: "deferred", installer_identity: "codex_marketplace_deferred_fixture" };
  const status = await execute(["status"], host);
  assert.equal(status.record.receipt.groups["catalog-delivery"].value.state, "deferred");
  assert.notEqual(status.record.receipt.groups["catalog-delivery"].value.state, "published");
});

test("resume after interruption does not require operator-copied identifiers", async () => {
  const { host } = await started();
  await execute(["advance"], host);
  const runId = host.dispatches[0].id;
  const issue = host.issues[0].number;
  const resumedHost = createTestHost({
    cloneFrom: host,
    argvIssue: undefined,
  });
  const resumed = await execute(["resume"], resumedHost);
  assert.equal(resumed.record.issue_number, issue);
  assert.ok(resumed.active_runs.some((row) => row.id === runId) || resumed.phase === "source_stabilization");
  assert.equal(resumedHost.operatorSuppliedRunIds.length, 0);
});

test("rehearse walks the machine without tagging or publish_release true", async () => {
  const { host } = await started({ rehearse: true });
  let guard = 0;
  let result = await execute(["advance"], host);
  while (result.phase !== "complete" && guard < 40) {
    host.completeActiveRuns();
    if (result.phase === "awaiting_approval") {
      result = await execute(["advance", "--record-approval", "--approver", "Albert"], host);
    } else {
      result = await execute(["advance"], host);
    }
    guard += 1;
  }
  assert.equal(result.phase, "complete");
  assert.equal(result.rehearse, true);
  assert.equal(host.tags.length, 0);
  assert.equal(
    host.dispatches.some((row) => row.inputs?.publish_release === "true"),
    false,
  );
  assert.equal(host.promotedToMain, false);
});

test("promotion and publication require recorded approval, not green CI", async () => {
  const { host } = await started();
  host.forcePhase("awaiting_approval");
  host.ciGreen = true;
  await assert.rejects(() => execute(["advance"], host), /approval/i);
  assert.equal(host.promotedToMain, false);
  const approved = await execute(["advance", "--record-approval", "--approver", "Albert"], host);
  assert.ok(approved.record.approval?.approver);
  assert.equal(approved.record.approval.approver, "Albert");
});

test("initReceipt remains the durable group store underneath the coordinator", async () => {
  const receipt = initReceipt("0.17.5");
  assert.equal(receipt.schema, RECEIPT_SCHEMA);
  assert.throws(() => validatePhase(receipt, "pre-freeze"));
});
