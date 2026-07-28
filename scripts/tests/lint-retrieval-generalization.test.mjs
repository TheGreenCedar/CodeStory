// The generalization lint bans benchmark identifiers from production. Its corpus is
// derived from benchmarks/tasks/**, which includes tasks whose subject is this
// repository - those name our own symbols, so they must not become bans. The rule
// that excludes them can fail in two directions, and both are silent: under-firing
// makes the product illegal to itself, over-firing disables the lint for a whole
// holdout repository.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

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

test("a task about this repository cannot ban this repository's own symbols", () => {
  // RefreshMode is a codestory-workspace product type. It reaches the corpus only
  // through readme-with-without/codestory-index-refresh-mode.task.json, whose subject
  // is CodeStory itself, so banning it would forbid the product from naming its own
  // API - which is what happened before the self-subject rule existed.
  const banned = bannedPatternsOver("crates/codestory-runtime/src");
  for (const own of ["RefreshMode", "crates"]) {
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
  const banned = bannedPatternsOver("crates/codestory-runtime/src");
  assert.ok(banned.size > 0, "the lint reported no banned patterns at all");
  for (const foreign of ["TicTacToe", "createServer"]) {
    assert.ok(
      [...banned].some((pattern) => pattern.includes(foreign)),
      `${foreign} belongs to a holdout repository and must stay banned, got: ${[...banned].join(", ")}`,
    );
  }
});
