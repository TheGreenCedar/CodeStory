import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  receiptDigest,
  validateMutationReceipt,
  validatePlatformEvidence,
  validateReceipt,
} from "./release-freeze-barrier.mjs";

const COMMIT = "1".repeat(40);
const TREE = "2".repeat(40);
const REQUIRED = ["cpu-reentry", "duplicate-source-proof"];

function mutationReceipt(overrides = {}) {
  return {
    commit: COMMIT,
    tree: TREE,
    cases: REQUIRED.map((id) => ({ id, status: "passed" })),
    ...overrides,
  };
}

function platformEvidence(overrides = {}) {
  return {
    failures: [{
      run_id: 77,
      platform: "windows",
    }],
    probes: [{
      failure_run_id: 77,
      platform: "windows",
      commit: COMMIT,
      tree: TREE,
      status: "passed",
      duration_seconds: 5,
      mutation: "junction replacement",
    }],
    ...overrides,
  };
}

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
    hostile_mutations: mutationReceipt(),
    platform_evidence: platformEvidence(),
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

test("an exact clean pushed receipt with hostile and native evidence passes", () => {
  validateReceipt(receipt(), {
    commit: COMMIT,
    tree: TREE,
    requiredMutationIds: REQUIRED,
  });
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
  ["mutation from another head", (value) => {
    value.hostile_mutations.commit = "6".repeat(40);
  }, /exact frozen commit and tree/u],
  ["named mutation not run", (value) => {
    value.hostile_mutations.cases[0].status = "skipped";
  }, /did not pass/u],
  ["native probe from another head", (value) => {
    value.platform_evidence.probes[0].commit = "7".repeat(40);
  }, /native probe under 90 seconds/u],
  ["native probe at 90 seconds", (value) => {
    value.platform_evidence.probes[0].duration_seconds = 90;
  }, /native probe under 90 seconds/u],
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
      () => validateReceipt(candidate, {
        commit: COMMIT,
        tree: TREE,
        requiredMutationIds: REQUIRED,
      }),
      pattern,
    );
  });
}

test("mutation and native evidence validators reject malformed arrays", () => {
  assert.throws(
    () => validateMutationReceipt({ commit: COMMIT, tree: TREE }, {
      commit: COMMIT,
      tree: TREE,
      requiredIds: REQUIRED,
    }),
    /contain cases/u,
  );
  assert.throws(
    () => validatePlatformEvidence({ failures: {}, probes: [] }, {
      commit: COMMIT,
      tree: TREE,
    }),
    /failure and probe arrays/u,
  );
});

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
      "--required-mutation",
      REQUIRED[0],
      "--required-mutation",
      REQUIRED[1],
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
      "--mutation-receipt",
      path.join(root, "missing-mutations.json"),
      "--platform-evidence",
      path.join(root, "missing-platform.json"),
      "--next-permitted-mutation",
      "none",
      "--required-mutation",
      "cpu-reentry",
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
