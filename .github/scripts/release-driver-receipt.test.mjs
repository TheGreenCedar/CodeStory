import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  RECEIPT_SCHEMA,
  initReceipt,
  invalidateReceipt,
  recordGroup,
  summarizeReceipt,
  validatePhase,
} from "./release-driver-receipt.mjs";

const SCRIPT = new URL("./release-driver-receipt.mjs", import.meta.url).pathname;
const C = "1".repeat(40);
const C_TREE = "2".repeat(40);
const F = "3".repeat(40);
const F_TREE = "4".repeat(40);
const P = "5".repeat(40);
const DIGEST = "a".repeat(64);
const CONSTANT_SET_PATH =
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";

function runEvidence(
  lane,
  runId,
  attempt = 1,
  conclusion = "success",
  commit = F,
  tree = F_TREE,
) {
  return {
    lane,
    run_id: runId,
    attempt,
    artifact: `${lane}-run-${runId}-attempt-${attempt}`,
    digest: DIGEST,
    identity: `${lane}-identity`,
    conclusion,
    commit,
    tree,
  };
}

function record(receipt, group, value) {
  return recordGroup(receipt, group, value);
}

function preFreezeReceipt() {
  let receipt = initReceipt("0.17.0");
  receipt = record(receipt, "calibration-source", { commit: C, tree: C_TREE });
  receipt = record(receipt, "pull-requests", {
    release_pr: 1873,
    integrated_support_prs: [1869, 1871],
  });
  receipt = record(
    receipt,
    "calibration-source-acceptance",
    runEvidence("calibration-source-acceptance", 100, 1, "success", C, C_TREE),
  );
  receipt = record(receipt, "evidence", {
    reusable: [{
      identity: "focused-checks@C",
      commit: C,
      tree: C_TREE,
      artifact: "focused-checks-at-C",
      digest: DIGEST,
      machine_policy: "release-claims.json/workflow_policy",
      evidence_window: "v0.17.0-candidate",
    }],
    invalidated: [{
      identity: "obsolete-package-run",
      reason: "candidate advanced",
      replacing_sha: C,
    }],
  });
  return record(receipt, "next-action", {
    action: "run calibration",
    owner: "release-driver",
  });
}

function frozenReceipt() {
  let receipt = preFreezeReceipt();
  receipt = record(receipt, "calibration", [
    runEvidence("metal-1", 101, 1, "success", C, C_TREE),
    runEvidence("metal-2", 102, 1, "success", C, C_TREE),
    runEvidence("metal-3", 103, 1, "success", C, C_TREE),
  ]);
  receipt = record(receipt, "frozen-candidate", { commit: F, tree: F_TREE });
  receipt = record(
    receipt,
    "frozen-candidate-acceptance",
    runEvidence("frozen-candidate-acceptance", 104),
  );
  receipt = record(receipt, "source-proof", runEvidence("source-proof", 105));
  receipt = record(receipt, "package", runEvidence("package", 106));
  receipt = record(receipt, "hardware", runEvidence("hardware", 107));
  receipt = record(
    receipt,
    "installed-candidate",
    runEvidence("installed-candidate", 108),
  );
  receipt = record(receipt, "qualification", runEvidence("qualification", 109));
  receipt = record(receipt, "pre-publish-ledger", {
    artifact: "pre-publish-ledger-attempt-1",
    digest: DIGEST,
  });
  return record(receipt, "next-action", {
    action: "obtain promotion approval",
    owner: "release-driver",
  });
}

function publishedReceipt() {
  let receipt = frozenReceipt();
  receipt = record(receipt, "promotion", {
    pull_request: 1900,
    approver: "TheGreenCedar",
    approved_at: "2026-08-08T15:00:00Z",
  });
  receipt = record(receipt, "publication", {
    commit: P,
    tree: F_TREE,
    tag: "v0.17.0",
    release_url: "https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.17.0",
    release_run: runEvidence("release", 110, 1, "success", P, F_TREE),
  });
  return record(receipt, "next-action", {
    action: "run post-publish closeout",
    owner: "release-driver",
  });
}

test("init creates a versioned structured receipt", () => {
  assert.deepEqual(initReceipt("0.17.0"), {
    schema: RECEIPT_SCHEMA,
    version: "0.17.0",
    sequence: 0,
    groups: {},
    attempt_history: [],
    invalidations: [],
  });
});

test("record refuses a run evidence row without its digest by name", () => {
  const incomplete = runEvidence("source-proof", 200);
  delete incomplete.digest;
  assert.throws(
    () => record(initReceipt("0.17.0"), "source-proof", incomplete),
    /source-proof\[0\] digest/u,
  );
});

test("record refuses an older success after a newer failed attempt in the same lane", () => {
  let receipt = record(
    initReceipt("0.17.0"),
    "source-proof",
    runEvidence("source-proof", 202, 2, "failure"),
  );
  assert.throws(
    () => record(
      receipt,
      "source-proof",
      runEvidence("source-proof", 202, 1, "success"),
    ),
    /newer failed run 202 attempt 2 exists/u,
  );
  receipt = record(
    receipt,
    "source-proof",
    runEvidence("source-proof", 203, 1, "success"),
  );
  assert.equal(receipt.groups["source-proof"].status, "active");
});

test("frozen and later phases require the qualification run identity", () => {
  const receipt = frozenReceipt();
  delete receipt.groups.qualification;
  assert.throws(() => validatePhase(receipt, "frozen"), /requires field group qualification/u);
  assert.throws(() => validatePhase(receipt, "published"), /requires field group qualification/u);
  assert.throws(() => validatePhase(receipt, "closeout"), /requires field group qualification/u);
});

test("post-calibration and post-freeze commits cascade invalidation", () => {
  const frozen = frozenReceipt();
  const afterCalibrationChange = invalidateReceipt(frozen, {
    event: "post-calibration-commit",
    reason: "unplanned workflow fix",
    replacingSha: "6".repeat(40),
    changedPaths: [".github/workflows/release.yml"],
  });
  assert.equal(
    afterCalibrationChange.groups["calibration-source-acceptance"].status,
    "invalidated",
  );
  assert.equal(afterCalibrationChange.groups.qualification.status, "invalidated");
  assert.equal(afterCalibrationChange.groups["calibration-source"].status, "invalidated");
  assert.equal(afterCalibrationChange.invalidations[0].recalibration_required, true);
  assert.equal(afterCalibrationChange.invalidations[0].resume_phase, "calibration-source");
  assert.throws(
    () => validatePhase(afterCalibrationChange, "frozen"),
    /invalidated without replacement/u,
  );

  const constantOnly = invalidateReceipt(frozen, {
    event: "post-freeze-commit",
    reason: "replace frozen candidate",
    replacingSha: "7".repeat(40),
    changedPaths: [CONSTANT_SET_PATH],
  });
  assert.equal(constantOnly.groups.calibration.status, "active");
  assert.equal(constantOnly.groups["frozen-candidate"].status, "invalidated");
  assert.equal(
    constantOnly.groups["frozen-candidate-acceptance"].status,
    "invalidated",
  );
  assert.equal(constantOnly.invalidations[0].recalibration_required, false);
  assert.equal(constantOnly.invalidations[0].resume_phase, "frozen-candidate");

  const sourceChange = invalidateReceipt(frozen, {
    event: "post-freeze-commit",
    reason: "documentation changed after freeze",
    replacingSha: "8".repeat(40),
    changedPaths: ["docs/contributors/release-runbook.md"],
  });
  assert.equal(sourceChange.groups.calibration.status, "invalidated");
  assert.equal(
    sourceChange.groups["calibration-source-acceptance"].status,
    "invalidated",
  );
  assert.equal(sourceChange.invalidations[0].recalibration_required, true);
});

test("the planned constant-set child is not accepted as an invalidation event", () => {
  assert.throws(
    () => invalidateReceipt(preFreezeReceipt(), {
      event: "post-calibration-commit",
      reason: "generated child",
      replacingSha: F,
      changedPaths: [CONSTANT_SET_PATH],
    }),
    /planned and must not invalidate/u,
  );
});

test("phase validation gates missing future evidence and rejects later evidence backwards", () => {
  const preFreeze = preFreezeReceipt();
  assert.equal(validatePhase(preFreeze, "pre-freeze"), true);
  assert.throws(
    () => validatePhase(preFreeze, "frozen"),
    /frozen requires field group calibration/u,
  );

  const frozen = frozenReceipt();
  assert.equal(validatePhase(frozen, "frozen"), true);
  assert.throws(
    () => validatePhase(frozen, "pre-freeze"),
    /pre-freeze does not allow later field group calibration/u,
  );
});

test("a receipt advances through the complete happy-path lifecycle", () => {
  const preFreeze = preFreezeReceipt();
  assert.equal(validatePhase(preFreeze, "pre-freeze"), true);

  const frozen = frozenReceipt();
  assert.equal(validatePhase(frozen, "frozen"), true);

  const published = publishedReceipt();
  assert.equal(validatePhase(published, "published"), true);

  let closeout = record(published, "post-publish-ledger", {
    artifact: "post-publish-ledger-attempt-1",
    digest: DIGEST,
  });
  closeout = record(closeout, "catalog-delivery", {
    state: "published",
    installer_identity: "codex_marketplace_live",
  });
  closeout = record(closeout, "next-action", {
    action: "retain evidence and close release children",
    owner: "release-driver",
  });
  assert.equal(validatePhase(closeout, "closeout"), true);
  assert.match(summarizeReceipt(closeout), /closeout: ready/u);
});

test("the CLI initializes, records, shows, and validates a receipt", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "release-driver-receipt-"));
  const receiptPath = path.join(directory, "receipt.json");
  execFileSync(process.execPath, [
    SCRIPT,
    "init",
    "--version",
    "0.17.0",
    "--receipt",
    receiptPath,
  ]);
  for (const [group, data] of Object.entries(preFreezeReceipt().groups)) {
    execFileSync(process.execPath, [
      SCRIPT,
      "record",
      group,
      "--receipt",
      receiptPath,
      "--data-json",
      JSON.stringify(data.value),
    ]);
  }
  assert.equal(
    execFileSync(process.execPath, [
      SCRIPT,
      "validate",
      "--phase",
      "pre-freeze",
      "--receipt",
      receiptPath,
    ], { encoding: "utf8" }),
    "pre-freeze: valid\n",
  );
  assert.match(
    execFileSync(process.execPath, [SCRIPT, "show", "--receipt", receiptPath], {
      encoding: "utf8",
    }),
    /Release 0\.17\.0/u,
  );
  assert.equal(JSON.parse(readFileSync(receiptPath, "utf8")).schema, RECEIPT_SCHEMA);

  const failed = spawnSync(process.execPath, [
    SCRIPT,
    "record",
    "source-proof",
    "--receipt",
    receiptPath,
    "--data-json",
    JSON.stringify({
      lane: "source-proof",
      run_id: 300,
      attempt: 1,
      artifact: "source-proof-attempt-1",
      conclusion: "success",
    }),
  ], { encoding: "utf8" });
  assert.equal(failed.status, 1);
  assert.match(failed.stderr, /source-proof\[0\] digest/u);
});
