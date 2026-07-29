// The generalization lint bans benchmark identifiers from production. Its corpus is
// derived from benchmarks/tasks/**, which includes tasks whose subject is this
// repository - those name our own symbols, so they must not become bans. The rule
// that excludes them can fail in two directions, and both are silent: under-firing
// makes the product illegal to itself, over-firing disables the lint for a whole
// holdout repository.
//
// Both directions are probed against a planted fixture rather than against the real
// source tree. A scan of `crates/**` only reports a foreign symbol while some file
// there happens to contain one, so an over-firing rule and a tree that simply stopped
// naming holdout fixtures are indistinguishable - the over-firing guard would go
// quietly vacuous the day the tree was cleaned up. The fixture names all four probe
// symbols itself, so each assertion fails for exactly one reason.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs, { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

// `RefreshMode` is a codestory-workspace product type and `crates/...` is where our
// code lives. Both reach the corpus only through
// readme-with-without/codestory-index-refresh-mode.task.json, whose subject is
// CodeStory itself. `TicTacToe` and `createServer` come from foreign holdout
// manifests. One fixture carries all four, so a single lint run answers both
// directions over identical input.
const PROBE_FIXTURE = [
  "pub fn generalization_probe() -> [&'static str; 4] {",
  '    ["RefreshMode", "crates/codestory-workspace/src/lib.rs", "TicTacToe", "createServer"]',
  "}",
  "",
].join("\n");

const OWN_IDENTIFIERS = ["RefreshMode", "crates"];
const FOREIGN_IDENTIFIERS = ["TicTacToe", "createServer"];

/// Run the lint over one directory and return every banned pattern it reported.
function bannedPatternsOver(scanRoot) {
  let output;
  try {
    output = execFileSync(
      process.execPath,
      [path.join(repositoryRoot, "scripts/lint-retrieval-generalization.mjs")],
      {
        cwd: repositoryRoot,
        encoding: "utf8",
        env: {
          ...process.env,
          CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS: scanRoot,
        },
      },
    );
  } catch (error) {
    // A failing lint still prints its findings; the exit code is the point.
    output = `${error.stdout ?? ""}${error.stderr ?? ""}`;
  }
  return new Set(
    [...output.matchAll(/Banned pattern \/(.+?)\/ in/gu)].map((match) => match[1]),
  );
}

/// Every banned pattern the lint reports against a fixture naming all four probes.
function bannedPatternsOverProbeFixture() {
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), "generalization-probe-"));
  try {
    fs.writeFileSync(path.join(fixtureRoot, "probe.rs"), PROBE_FIXTURE);
    return bannedPatternsOver(fixtureRoot);
  } finally {
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  }
}

test("a task about this repository cannot ban this repository's own symbols", () => {
  const banned = bannedPatternsOverProbeFixture();
  // The fixture names foreign symbols too, so an empty report means the lint never
  // ran rather than that our own identifiers were spared.
  assert.ok(banned.size > 0, "the lint reported no banned patterns at all");
  for (const own of OWN_IDENTIFIERS) {
    assert.ok(
      ![...banned].some((pattern) => pattern.includes(own)),
      `${own} is a CodeStory identifier and must not be banned, got: ${[...banned].join(", ")}`,
    );
  }
});

test("tasks about other repositories still ban their symbols", () => {
  // The guard against over-firing: if the self-subject rule ever matched every task,
  // the lint would report nothing and pass silently. These come from foreign holdout
  // manifests and must survive.
  const banned = bannedPatternsOverProbeFixture();
  assert.ok(banned.size > 0, "the lint reported no banned patterns at all");
  for (const foreign of FOREIGN_IDENTIFIERS) {
    assert.ok(
      [...banned].some((pattern) => pattern.includes(foreign)),
      `${foreign} belongs to a holdout repository and must stay banned, got: ${[...banned].join(", ")}`,
    );
  }
});

/// Run the lint with one extra task manifest, additively, and report the banned
/// patterns over a production tree of our choosing. The extra root never touches
/// the checked-in corpus every other run reads.
function bannedPatternsWithExtraTask(task) {
  const root = mkdtempSync(path.join(os.tmpdir(), "generalization-lint-"));
  try {
    const taskRoot = path.join(root, "tasks");
    const scanRoot = path.join(root, "src");
    mkdirSync(taskRoot);
    mkdirSync(scanRoot);
    writeFileSync(path.join(taskRoot, "probe.task.json"), JSON.stringify(task));
    writeFileSync(path.join(scanRoot, "probe.rs"), "pub fn probe() {}\n");
    const dumpPath = path.join(root, "patterns.json");
    try {
      execFileSync(
        process.execPath,
        [path.join(repositoryRoot, "scripts/lint-retrieval-generalization.mjs")],
        {
          cwd: repositoryRoot,
          encoding: "utf8",
          env: {
            ...process.env,
            CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS: scanRoot,
            CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS: taskRoot,
            CODESTORY_RETRIEVAL_GENERALIZATION_DUMP_PATTERNS: dumpPath,
          },
        },
      );
    } catch (error) {
      throw new Error(`lint failed to dump patterns: ${error.stderr ?? error}`);
    }
    return JSON.parse(readFileSync(dumpPath, "utf8"));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("a holdout named after one of our crates is not mistaken for this repository", () => {
  // The self-subject rule decides which tasks may not contribute symbol bans.
  // Deciding it on any crate-name token would hand that exemption to a holdout
  // repository called `store`, `runtime`, or `bench` and switch this lint off
  // for it silently. Only this repository's own name may claim the exemption.
  for (const impostor of ["store", "runtime", "bench", "indexer"]) {
    const { derived } = bannedPatternsWithExtraTask({
      id: `${impostor}-probe-task`,
      repo: { name: impostor, url: `https://github.com/example/${impostor}.git` },
      prompt: "Explain how the probe repository handles its own requests end to end.",
      expected_symbols: [
        { name: "probeGadgetHandler", path: "src/probe_gadget.rs", kind: "function" },
      ],
    });
    assert.ok(
      derived.some((pattern) => pattern.includes("probeGadgetHandler")),
      `a holdout named \`${impostor}\` must still ban its own symbols`,
    );
  }
});

test("this repository's own name still claims the self-subject exemption", () => {
  const { derived } = bannedPatternsWithExtraTask({
    id: "codestory-probe-task",
    repo: { name: "codestory", url: "https://github.com/TheGreenCedar/CodeStory.git" },
    prompt: "Explain how the probe repository handles its own requests end to end.",
    expected_symbols: [
      { name: "probeGadgetHandler", path: "src/probe_gadget.rs", kind: "function" },
    ],
  });
  assert.ok(
    !derived.some((pattern) => pattern.includes("probeGadgetHandler")),
    "a task whose subject is this repository must not ban this repository's symbols",
  );
});
