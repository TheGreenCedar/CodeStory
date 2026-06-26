import test from "node:test";
import assert from "node:assert/strict";

import { ghArgs, parseArgs, validateCommentBody } from "../github-status-comment.mjs";

test("comment validation accepts real newlines", () => {
  assert.doesNotThrow(() => validateCommentBody("Status:\n- ready\n"));
});

test("comment validation rejects literal newline escapes", () => {
  assert.throws(
    () => validateCommentBody("Status:\\n- not rendered"),
    /literal \\\\n/,
  );
});

test("comment helper posts through body files", () => {
  const opts = parseArgs(["--issue", "641", "--repo", "TheGreenCedar/CodeStory", "--body-file", "body.md"]);

  assert.deepEqual(ghArgs(opts, "body.md"), [
    "issue",
    "comment",
    "641",
    "--repo",
    "TheGreenCedar/CodeStory",
    "--body-file",
    "body.md",
  ]);
});
