import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  acceptedFreezeStatus,
  receiptDigest,
  validateAcceptanceProvenance,
  validateReceipt,
} from "./release-freeze-barrier.mjs";

const REPOSITORY = "TheGreenCedar/CodeStory";
const COMMIT = "1".repeat(40);
const TREE = "2".repeat(40);
const RUN_ID = 77;
const RUN_ATTEMPT = 2;
const NEXT_PERMITTED_MUTATION =
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";
const PLANNED_PROOF_ACTIONS = [
  "acceptance",
  "source-proof",
  "calibration",
  "qualification",
  "release",
];

function receipt(overrides = {}) {
  const candidate = {
    schema: 2,
    authority: "github_actions",
    repository: REPOSITORY,
    branch: "codex/release",
    commit: COMMIT,
    tree: TREE,
    worktree_clean: true,
    remote_head: COMMIT,
    release_pr: {
      number: 1597,
      base: "dev/codestory-next",
      base_commit: "0".repeat(40),
      head: "codex/release",
      head_commit: COMMIT,
    },
    integrated_support_prs: [],
    known_future_source_changes: [NEXT_PERMITTED_MUTATION],
    planned_proof_actions: [...PLANNED_PROOF_ACTIONS],
    proof_triggering_labels: [],
    proof_triggering_actions: [...PLANNED_PROOF_ACTIONS],
    reusable_evidence: [],
    invalidated_evidence: [],
    running_workflows: [],
    cancelled_superseded_runs: [],
    next_permitted_mutation: NEXT_PERMITTED_MUTATION,
    acceptance_run: {
      id: RUN_ID,
      attempt: RUN_ATTEMPT,
      workflow: ".github/workflows/source-proof.yml",
      event: "workflow_dispatch",
    },
    ...overrides,
  };
  candidate.digest = receiptDigest(candidate);
  return candidate;
}

const RECEIPT_CONTEXT = {
  repository: REPOSITORY,
  commit: COMMIT,
  tree: TREE,
  runId: String(RUN_ID),
  runAttempt: String(RUN_ATTEMPT),
};

test("an exact clean pushed Actions receipt passes", () => {
  validateReceipt(receipt(), RECEIPT_CONTEXT);
});

test("a newer invalidation status revokes an older accepted freeze", () => {
  const acceptedReceipt = receipt();
  const context = `codestory/release-freeze/${acceptedReceipt.digest}`;
  const accepted = {
    id: "9007199254740993",
    state: "success",
    context,
    description: `tree=${TREE}`,
  };
  assert.equal(
    acceptedFreezeStatus([accepted], {
      tree: TREE,
      digest: acceptedReceipt.digest,
    }),
    accepted,
  );
  assert.equal(
    acceptedFreezeStatus([
      accepted,
      {
        id: "9007199254740994",
        state: "error",
        context,
        description: `superseded-by=${"3".repeat(40)}`,
      },
    ], {
      tree: TREE,
      digest: acceptedReceipt.digest,
    }),
    undefined,
  );
});

for (const [name, mutate, pattern] of [
  ["later commit", (value) => { value.commit = "3".repeat(40); }, /exact commit and tree/u],
  ["later tree", (value) => { value.tree = "4".repeat(40); }, /exact commit and tree/u],
  ["dirty worktree", (value) => { value.worktree_clean = false; }, /clean worktree/u],
  ["unpushed head", (value) => { value.remote_head = "5".repeat(40); }, /clean worktree/u],
  ["moved release PR", (value) => {
    value.release_pr.head_commit = "5".repeat(40);
  }, /bind the open release PR/u],
  ["unbound release base", (value) => {
    value.release_pr.base_commit = "";
  }, /bind the open release PR/u],
  ["undeclared source change", (value) => {
    value.known_future_source_changes.push(".github/workflows/release.yml");
  }, /only the generated constant-set change/u],
  ["caller-selected proof actions", (value) => {
    value.planned_proof_actions = ["source-proof"];
  }, /exact proof-triggering actions/u],
  ["proof-triggering label", (value) => {
    value.proof_triggering_labels = ["source-proof"];
  }, /exact proof-triggering actions/u],
  ["cross-attempt receipt", (value) => {
    value.acceptance_run.attempt = RUN_ATTEMPT + 1;
  }, /exact Actions run and attempt/u],
  ["missing handoff field", (value) => { delete value.running_workflows; }, /running_workflows/u],
  ["missing next mutation", (value) => {
    value.next_permitted_mutation = "";
  }, /generated constant set as the next mutation/u],
  ["tampered receipt", (value) => {
    value.reusable_evidence.push("unauthenticated evidence");
  }, /digest/u],
]) {
  test(`freeze barrier rejects ${name}`, () => {
    const candidate = receipt();
    mutate(candidate);
    if (name !== "tampered receipt") {
      candidate.digest = receiptDigest(candidate);
    }
    assert.throws(
      () => validateReceipt(candidate, RECEIPT_CONTEXT),
      pattern,
    );
  });
}

function acceptanceProvenance() {
  const acceptedReceipt = receipt();
  const digest = acceptedReceipt.digest;
  const startedAt = "2026-07-30T12:00:00Z";
  const completedAt = "2026-07-30T12:00:06Z";
  const job = (name, stepName, labels = ["ubuntu-latest"]) => ({
    name,
    status: "completed",
    conclusion: "success",
    head_sha: COMMIT,
    run_id: RUN_ID,
    run_attempt: RUN_ATTEMPT,
    labels,
    steps: [{
      name: stepName,
      status: "completed",
      conclusion: "success",
      started_at: startedAt,
      completed_at: completedAt,
    }],
  });
  return {
    status: {
      state: "success",
      context: `codestory/release-freeze/${digest}`,
      description: `tree=${TREE}`,
      target_url: `https://github.com/${REPOSITORY}/actions/runs/${RUN_ID}`,
      creator: { login: "github-actions[bot]", type: "Bot" },
    },
    run: {
      id: RUN_ID,
      run_attempt: RUN_ATTEMPT,
      head_sha: COMMIT,
      path: ".github/workflows/source-proof.yml",
      event: "workflow_dispatch",
      status: "completed",
      conclusion: "success",
      head_repository: { full_name: REPOSITORY },
    },
    jobs: [
      job("freeze-hostile-mutations", "Execute exact-head hostile mutation matrix"),
      job(
        "freeze-windows-native-probe",
        "Run exact-head Windows native probe",
        ["self-hosted", "Windows", "X64", "codestory-vulkan"],
      ),
      job("freeze-acceptance", "Publish executable release freeze"),
    ],
    artifact: {
      name: `release-freeze-receipt-attempt-${RUN_ATTEMPT}`,
      expired: false,
      workflow_run: { id: RUN_ID },
    },
    receipt: acceptedReceipt,
    repository: REPOSITORY,
    commit: COMMIT,
    tree: TREE,
    digest,
  };
}

test("acceptance trusts exact Actions run, job, step, host, and duration provenance", () => {
  assert.equal(validateAcceptanceProvenance(acceptanceProvenance()), 77);
});

for (const [name, mutate, pattern] of [
  ["caller-authored success", (value) => {
    value.status.creator = { login: "TheGreenCedar", type: "User" };
  }, /not authenticated Actions acceptance/u],
  ["cross-head run", (value) => {
    value.run.head_sha = "3".repeat(40);
  }, /run provenance changed/u],
  ["wrong workflow", (value) => {
    value.run.path = ".github/workflows/release.yml";
  }, /run provenance changed/u],
  ["skipped hostile mutations", (value) => {
    value.jobs[0].conclusion = "skipped";
  }, /not a successful exact-run job/u],
  ["unprotected Windows runner", (value) => {
    value.jobs[1].labels = ["self-hosted", "Windows", "X64"];
  }, /protected label codestory-vulkan/u],
  ["90-second Windows probe", (value) => {
    value.jobs[1].steps[0].completed_at = "2026-07-30T12:01:30Z";
  }, /under 90 seconds/u],
  ["fabricated native step", (value) => {
    value.jobs[1].steps[0].conclusion = "failure";
  }, /did not execute successfully/u],
  ["wrong receipt artifact", (value) => {
    value.artifact.name = "release-freeze-receipt-attempt-999";
  }, /receipt artifact provenance changed/u],
  ["expired receipt artifact", (value) => {
    value.artifact.expired = true;
  }, /receipt artifact provenance changed/u],
  ["cross-run receipt artifact", (value) => {
    value.artifact.workflow_run.id = RUN_ID + 1;
  }, /receipt artifact provenance changed/u],
  ["cross-attempt receipt artifact", (value) => {
    value.artifact.name = `release-freeze-receipt-attempt-${RUN_ATTEMPT + 1}`;
  }, /receipt artifact provenance changed/u],
  ["tampered receipt artifact", (value) => {
    value.receipt.running_workflows.push({ id: 123 });
  }, /digest/u],
]) {
  test(`acceptance rejects ${name}`, () => {
    const value = acceptanceProvenance();
    mutate(value);
    assert.throws(() => validateAcceptanceProvenance(value), pattern);
  });
}

test("verify-file is executable and rejects a later commit", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-"));
  const receiptPath = path.join(root, "receipt.json");
  writeFileSync(receiptPath, `${JSON.stringify(receipt(), null, 2)}\n`);
  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const accepted = spawnSync(
    process.execPath,
    [
      script.pathname,
      "verify-file",
      "--receipt",
      receiptPath,
      "--repository",
      REPOSITORY,
      "--commit",
      COMMIT,
      "--tree",
      TREE,
      "--run-id",
      String(RUN_ID),
      "--run-attempt",
      String(RUN_ATTEMPT),
    ],
    { encoding: "utf8" },
  );
  assert.equal(accepted.status, 0, accepted.stderr);
  assert.equal(accepted.stdout.trim(), receipt().digest);

  const rejected = spawnSync(
    process.execPath,
    [
      script.pathname,
      "verify-file",
      "--receipt",
      receiptPath,
      "--repository",
      REPOSITORY,
      "--commit",
      "8".repeat(40),
      "--tree",
      TREE,
      "--run-id",
      String(RUN_ID),
      "--run-attempt",
      String(RUN_ATTEMPT),
    ],
    { encoding: "utf8" },
  );
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /exact commit and tree/u);
});

test("record-actions-receipt refuses to mint authority outside GitHub Actions", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-outside-actions-"));
  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      script.pathname,
      "record-actions-receipt",
      "--repo",
      root,
      "--repository",
      REPOSITORY,
      "--branch",
      "codex/release",
      "--commit",
      COMMIT,
      "--tree",
      TREE,
      "--release-pr",
      "1",
      "--output",
      path.join(root, "receipt.json"),
      "--run-id",
      String(RUN_ID),
      "--run-attempt",
      String(RUN_ATTEMPT),
      "--support-prs-json",
      "[]",
      "--broad-workflow",
      "Exact-head source proof",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        GITHUB_ACTIONS: "",
        GITHUB_EVENT_NAME: "",
      },
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(
    result.stderr,
    /canonical release freeze receipt may be produced only by workflow_dispatch/u,
  );
});

test("cancel-superseded rejects a cancellation request that leaves the run active", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-gh-"));
  const fakeGh = path.join(root, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2" = "run list" ]; then
  printf '%s\\n' '[{"databaseId":123,"workflowName":"Exact-head source proof","headSha":"${"9".repeat(40)}","headBranch":"old","status":"in_progress","event":"workflow_dispatch","url":"https://example.invalid/123"}]'
  exit 0
fi
if [ "$1 $2" = "run cancel" ]; then
  exit 0
fi
exit 1
`,
  );
  chmodSync(fakeGh, 0o755);
  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      script.pathname,
      "cancel-superseded",
      "--repository",
      "TheGreenCedar/CodeStory",
      "--commit",
      COMMIT,
      "--broad-workflow",
      "Exact-head source proof",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        CODESTORY_FREEZE_CANCEL_POLL_ATTEMPTS: "2",
        CODESTORY_FREEZE_CANCEL_POLL_MS: "0",
        PATH: `${root}${path.delimiter}${process.env.PATH}`,
      },
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /remains queued or running after cancellation/u);
});

test("cancel-superseded rejects another active broad run on the unchanged head", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-duplicate-gh-"));
  const fakeGh = path.join(root, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2" = "run list" ]; then
  printf '%s\\n' '[{"databaseId":456,"workflowName":"Exact-head source proof","headSha":"${COMMIT}","headBranch":"candidate","status":"in_progress","event":"workflow_dispatch","url":"https://example.invalid/456"}]'
  exit 0
fi
exit 1
`,
  );
  chmodSync(fakeGh, 0o755);
  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      script.pathname,
      "cancel-superseded",
      "--repository",
      "TheGreenCedar/CodeStory",
      "--commit",
      COMMIT,
      "--broad-workflow",
      "Exact-head source proof",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${root}${path.delimiter}${process.env.PATH}`,
      },
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /unchanged head.*already has active/u);
});

test("automatic invalidation preserves an active proof for the new exact head", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-current-gh-"));
  const fakeGh = path.join(root, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2" = "run list" ]; then
  printf '%s\\n' '[{"databaseId":789,"workflowName":"Exact-head source proof","headSha":"${COMMIT}","headBranch":"candidate","status":"in_progress","event":"workflow_dispatch","url":"https://example.invalid/789"}]'
  exit 0
fi
if [ "$1 $2" = "run cancel" ]; then
  exit 9
fi
exit 1
`,
  );
  chmodSync(fakeGh, 0o755);
  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      script.pathname,
      "invalidate-superseded",
      "--repository",
      "TheGreenCedar/CodeStory",
      "--commit",
      COMMIT,
      "--broad-workflow",
      "Exact-head source proof",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        CODESTORY_FREEZE_CANCEL_POLL_ATTEMPTS: "2",
        CODESTORY_FREEZE_CANCEL_POLL_MS: "0",
        PATH: `${root}${path.delimiter}${process.env.PATH}`,
      },
    },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), { cancelled: [] });
});
