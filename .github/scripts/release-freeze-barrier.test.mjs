import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
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
const SOURCE_STABILIZATION_ACTIONS = [
  "source-stabilization",
  "calibration",
  "generated-constant-freeze",
  "frozen-candidate-acceptance",
  "qualification",
  "release",
];
const FROZEN_CANDIDATE_ACTIONS = [
  "frozen-candidate-acceptance",
  "qualification",
  "release",
];

function receipt(overrides = {}) {
  const phase = overrides.phase ?? "source_stabilization";
  const frozen = phase === "frozen_candidate";
  const plannedActions = frozen
    ? FROZEN_CANDIDATE_ACTIONS
    : SOURCE_STABILIZATION_ACTIONS;
  const candidate = {
    schema: 3,
    authority: "github_actions",
    phase,
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
    known_future_source_changes: frozen ? [] : [NEXT_PERMITTED_MUTATION],
    planned_proof_actions: [...plannedActions],
    proof_triggering_labels: [],
    proof_triggering_actions: [...plannedActions],
    reusable_evidence: [],
    invalidated_evidence: [],
    running_workflows: [],
    cancelled_superseded_runs: [],
    next_permitted_mutation: frozen ? null : NEXT_PERMITTED_MUTATION,
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
  phase: "source_stabilization",
};

test("an exact clean pushed source-stabilization Actions receipt passes", () => {
  validateReceipt(receipt(), RECEIPT_CONTEXT);
});

test("a next-head bind passes when next is already the exact head", () => {
  validateReceipt(receipt({
    branch: "dev/codestory-next",
    release_pr: {
      number: 0,
      bind: "next_head",
      base: "dev/codestory-next",
      base_commit: "0".repeat(40),
      head: "dev/codestory-next",
      head_commit: COMMIT,
    },
  }), RECEIPT_CONTEXT);
});

test("a frozen-candidate receipt carries no future mutation and passes", () => {
  const frozen = receipt({ phase: "frozen_candidate" });
  validateReceipt(frozen, {
    ...RECEIPT_CONTEXT,
    phase: "frozen_candidate",
  });
  assert.deepEqual(frozen.known_future_source_changes, []);
  assert.equal(frozen.next_permitted_mutation, null);
});

test("source stabilization finishes before calibration and has no later source proof", () => {
  const actions = receipt().planned_proof_actions;
  assert.ok(actions.indexOf("source-stabilization") < actions.indexOf("calibration"));
  assert.ok(actions.indexOf("calibration") < actions.indexOf("generated-constant-freeze"));
  assert.equal(actions.includes("source-proof"), false);
});

test("receipts cannot cross the source-stabilization and frozen-candidate phases", () => {
  assert.throws(
    () => validateReceipt(receipt(), {
      ...RECEIPT_CONTEXT,
      phase: "frozen_candidate",
    }),
    /authority schema/u,
  );
  assert.throws(
    () => validateReceipt(receipt({ phase: "frozen_candidate" }), RECEIPT_CONTEXT),
    /authority schema/u,
  );
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
  }, /bind the open release PR or next-head/u],
  ["unbound release base", (value) => {
    value.release_pr.base_commit = "";
  }, /bind the open release PR or next-head/u],
  ["undeclared source change", (value) => {
    value.known_future_source_changes.push(".github/workflows/release.yml");
  }, /future changes do not match source_stabilization/u],
  ["caller-selected proof actions", (value) => {
    value.planned_proof_actions = ["source-proof"];
  }, /exact source_stabilization actions/u],
  ["proof-triggering label", (value) => {
    value.proof_triggering_labels = ["source-proof"];
  }, /exact source_stabilization actions/u],
  ["cross-attempt receipt", (value) => {
    value.acceptance_run.attempt = RUN_ATTEMPT + 1;
  }, /exact Actions run and attempt/u],
  ["missing handoff field", (value) => { delete value.running_workflows; }, /running_workflows/u],
  ["missing next mutation", (value) => {
    value.next_permitted_mutation = "";
  }, /next mutation does not match source_stabilization/u],
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
    phase: "source_stabilization",
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
      "--phase",
      "source_stabilization",
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
      "--phase",
      "source_stabilization",
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
      "--phase",
      "source_stabilization",
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

test("record-actions-receipt accepts an empty --release-pr for next-head bind", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-empty-release-pr-"));
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
      "dev/codestory-next",
      "--commit",
      COMMIT,
      "--tree",
      TREE,
      "--release-pr",
      "",
      "--output",
      path.join(root, "receipt.json"),
      "--run-id",
      String(RUN_ID),
      "--run-attempt",
      String(RUN_ATTEMPT),
      "--phase",
      "source_stabilization",
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
  assert.doesNotMatch(result.stderr, /--release-pr requires a value/u);
  assert.match(
    result.stderr,
    /canonical release freeze receipt may be produced only by workflow_dispatch/u,
  );
});

test("record-actions-receipt rejects a PR whose snapshot omits the live dev head", () => {
  const sandbox = mkdtempSync(path.join(tmpdir(), "codestory-freeze-stale-base-"));
  const root = path.join(sandbox, "repo");
  mkdirSync(root);
  execFileSync("git", ["init", "-q", "-b", "codex/release", root]);
  execFileSync("git", ["-C", root, "config", "user.email", "test@example.com"]);
  execFileSync("git", ["-C", root, "config", "user.name", "Test"]);
  writeFileSync(path.join(root, "tracked.txt"), "candidate\n");
  execFileSync("git", ["-C", root, "add", "tracked.txt"]);
  execFileSync("git", ["-C", root, "commit", "-qm", "candidate"]);
  const commit = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
  const tree = execFileSync("git", ["-C", root, "rev-parse", "HEAD^{tree}"], {
    encoding: "utf8",
  }).trim();
  const staleBase = "a".repeat(40);
  const liveBase = "b".repeat(40);
  const fakeGh = path.join(sandbox, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "repos/${REPOSITORY}/pulls/1597" ]; then
  printf '%s\\n' '{"number":1597,"state":"open","base":{"ref":"dev/codestory-next","sha":"${staleBase}"},"head":{"ref":"codex/release","sha":"${commit}","repo":{"full_name":"${REPOSITORY}"}}}'
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "repos/${REPOSITORY}/git/ref/heads/dev/codestory-next" ]; then
  printf '%s\\n' '{"object":{"sha":"${liveBase}"}}'
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "repos/${REPOSITORY}/compare/${liveBase}...${commit}" ]; then
  printf '%s\\n' '{"status":"diverged"}'
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "repos/${REPOSITORY}/compare/${staleBase}...${commit}" ]; then
  printf '%s\\n' '{"status":"ahead"}'
  exit 0
fi
if [ "$1 $2" = "run list" ]; then
  printf '%s\\n' '[]'
  exit 0
fi
exit 9
`,
  );
  chmodSync(fakeGh, 0o755);
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
      commit,
      "--tree",
      tree,
      "--release-pr",
      "1597",
      "--output",
      path.join(root, "receipt.json"),
      "--run-id",
      String(RUN_ID),
      "--run-attempt",
      String(RUN_ATTEMPT),
      "--phase",
      "source_stabilization",
      "--support-prs-json",
      "[]",
      "--reusable-evidence-json",
      "[]",
      "--invalidated-evidence-json",
      "[]",
      "--cancelled-runs-json",
      "[]",
      "--broad-workflow",
      "Exact-head source proof",
    ],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        GITHUB_ACTIONS: "true",
        GITHUB_EVENT_NAME: "workflow_dispatch",
        PATH: `${sandbox}${path.delimiter}${process.env.PATH}`,
      },
    },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /does not contain current dev base/u);
});

test("cancel-superseded rejects a cancellation request that leaves the run active", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-gh-"));
  const fakeGh = path.join(root, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2 $3" = "api --paginate --slurp" ]; then
  case "$4" in
    *status=in_progress*)
      printf '%s\\n' '[
        {"workflow_runs":[{"id":123,"name":"Exact-head source proof","head_sha":"${"9".repeat(40)}","head_branch":"old","status":"in_progress","event":"workflow_dispatch","html_url":"https://example.invalid/123"}]}
      ]'
      ;;
    *) printf '%s\\n' '[{"workflow_runs":[]}]' ;;
  esac
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

test("cancel-superseded finds an obsolete proof on a later active-run page", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-paginated-gh-"));
  const fakeGh = path.join(root, "gh");
  const cancelledMarker = path.join(root, "cancelled");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2 $3" = "api --paginate --slurp" ]; then
  case "$4" in
    *status=in_progress*)
      if [ -f "${cancelledMarker}" ]; then
        printf '%s\\n' '[{"workflow_runs":[]}]'
      else
        printf '%s\\n' '[
          {"workflow_runs":[{"id":1,"name":"Draft source checks","head_sha":"${COMMIT}","head_branch":"candidate","status":"in_progress","event":"pull_request","html_url":"https://example.invalid/1"}]},
          {"workflow_runs":[{"id":999,"name":"Exact-head source proof","head_sha":"${"9".repeat(40)}","head_branch":"obsolete","status":"in_progress","event":"workflow_dispatch","html_url":"https://example.invalid/999"}]}
        ]'
      fi
      ;;
    *) printf '%s\\n' '[{"workflow_runs":[]}]' ;;
  esac
  exit 0
fi
if [ "$1 $2 $3" = "run cancel 999" ]; then
  : > "${cancelledMarker}"
  exit 0
fi
exit 9
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
      REPOSITORY,
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
  assert.deepEqual(JSON.parse(result.stdout), {
    cancelled: [{
      database_id: 999,
      head_sha: "9".repeat(40),
      workflow: "Exact-head source proof",
    }],
  });
});

test("cancel-superseded rejects another active broad run on the unchanged head", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-freeze-duplicate-gh-"));
  const fakeGh = path.join(root, "gh");
  writeFileSync(
    fakeGh,
    `#!/bin/sh
if [ "$1 $2 $3" = "api --paginate --slurp" ]; then
  case "$4" in
    *status=in_progress*)
      printf '%s\\n' '[
        {"workflow_runs":[{"id":456,"name":"Exact-head source proof","head_sha":"${COMMIT}","head_branch":"candidate","status":"in_progress","event":"workflow_dispatch","html_url":"https://example.invalid/456"}]}
      ]'
      ;;
    *) printf '%s\\n' '[{"workflow_runs":[]}]' ;;
  esac
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
if [ "$1 $2 $3" = "api --paginate --slurp" ]; then
  case "$4" in
    *status=in_progress*)
      printf '%s\\n' '[
        {"workflow_runs":[{"id":789,"name":"Exact-head source proof","head_sha":"${COMMIT}","head_branch":"candidate","status":"in_progress","event":"workflow_dispatch","html_url":"https://example.invalid/789"}]}
      ]'
      ;;
    *) printf '%s\\n' '[{"workflow_runs":[]}]' ;;
  esac
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
