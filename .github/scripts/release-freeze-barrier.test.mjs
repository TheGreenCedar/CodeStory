import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  receiptDigest,
  validateAcceptanceProvenance,
  validateReceipt,
} from "./release-freeze-barrier.mjs";

const COMMIT = "1".repeat(40);
const TREE = "2".repeat(40);
const DIGEST = "a".repeat(64);

function receipt(overrides = {}) {
  const candidate = {
    schema: 1,
    repository: "TheGreenCedar/CodeStory",
    branch: "codex/release",
    commit: COMMIT,
    tree: TREE,
    worktree_clean: true,
    remote_head: COMMIT,
    release_pr: {
      number: 1597,
      base: "dev/codestory-next",
      head: "codex/release",
      head_commit: COMMIT,
    },
    integrated_support_prs: [],
    known_future_source_changes: [
      "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
    ],
    planned_proof_actions: ["source-proof", "calibration", "qualification"],
    reusable_evidence: [],
    invalidated_evidence: [],
    running_workflows: [],
    cancelled_superseded_runs: [],
    next_permitted_mutation: "generated constant set only",
    ...overrides,
  };
  candidate.digest = receiptDigest(candidate);
  return candidate;
}

test("an exact clean pushed local declaration passes", () => {
  validateReceipt(receipt(), { commit: COMMIT, tree: TREE });
});

for (const [name, mutate, pattern] of [
  ["later commit", (value) => { value.commit = "3".repeat(40); }, /exact commit and tree/u],
  ["later tree", (value) => { value.tree = "4".repeat(40); }, /exact commit and tree/u],
  ["dirty worktree", (value) => { value.worktree_clean = false; }, /clean worktree/u],
  ["unpushed head", (value) => { value.remote_head = "5".repeat(40); }, /clean worktree/u],
  ["moved release PR", (value) => {
    value.release_pr.head_commit = "5".repeat(40);
  }, /bind the open release PR/u],
  ["undeclared source change", (value) => {
    value.known_future_source_changes.push(".github/workflows/release.yml");
  }, /unsupported future source change/u],
  ["missing handoff field", (value) => { delete value.running_workflows; }, /running_workflows/u],
  ["missing next mutation", (value) => { value.next_permitted_mutation = ""; }, /next permitted mutation/u],
  ["tampered receipt", (value) => {
    value.planned_proof_actions.push("second-source-proof");
  }, /digest/u],
]) {
  test(`freeze barrier rejects ${name}`, () => {
    const candidate = receipt();
    mutate(candidate);
    if (name !== "tampered receipt") {
      candidate.digest = receiptDigest(candidate);
    }
    assert.throws(
      () => validateReceipt(candidate, { commit: COMMIT, tree: TREE }),
      pattern,
    );
  });
}

function acceptanceProvenance() {
  const runId = 77;
  const runAttempt = 2;
  const startedAt = "2026-07-30T12:00:00Z";
  const completedAt = "2026-07-30T12:00:06Z";
  const job = (name, stepName, labels = ["ubuntu-latest"]) => ({
    name,
    status: "completed",
    conclusion: "success",
    head_sha: COMMIT,
    run_id: runId,
    run_attempt: runAttempt,
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
      context: `codestory/release-freeze/${DIGEST}`,
      description: `tree=${TREE}`,
      target_url: `https://github.com/TheGreenCedar/CodeStory/actions/runs/${runId}`,
      creator: { login: "github-actions[bot]", type: "Bot" },
    },
    run: {
      id: runId,
      run_attempt: runAttempt,
      head_sha: COMMIT,
      path: ".github/workflows/source-proof.yml",
      event: "workflow_dispatch",
      status: "completed",
      conclusion: "success",
      head_repository: { full_name: "TheGreenCedar/CodeStory" },
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
    repository: "TheGreenCedar/CodeStory",
    commit: COMMIT,
    tree: TREE,
    digest: DIGEST,
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
      "--commit",
      COMMIT,
      "--tree",
      TREE,
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
      "--commit",
      "8".repeat(40),
      "--tree",
      TREE,
    ],
    { encoding: "utf8" },
  );
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /exact commit and tree/u);
});

test("declare rejects a dirty worktree before publishing a status", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-repo-"));
  execFileSync("git", ["init", "-q", root]);
  execFileSync("git", ["-C", root, "config", "user.email", "test@example.com"]);
  execFileSync("git", ["-C", root, "config", "user.name", "Test"]);
  writeFileSync(path.join(root, "tracked.txt"), "one\n");
  execFileSync("git", ["-C", root, "add", "tracked.txt"]);
  execFileSync("git", ["-C", root, "commit", "-qm", "initial"]);
  writeFileSync(path.join(root, "untracked.txt"), "dirty\n");

  const script = new URL("./release-freeze-barrier.mjs", import.meta.url);
  const result = spawnSync(
    process.execPath,
    [
      script.pathname,
      "declare",
      "--repo",
      root,
      "--repository",
      "TheGreenCedar/CodeStory",
      "--release-pr",
      "1",
      "--output",
      path.join(root, "receipt.json"),
      "--next-permitted-mutation",
      "none",
      "--planned-proof-action",
      "source-proof",
      "--broad-workflow",
      "Exact-head source proof",
      "--no-publish-status",
    ],
    { encoding: "utf8" },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /clean worktree, including untracked files/u);
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
