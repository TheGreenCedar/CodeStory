import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";

const script = "scripts/codestory-manual-friction-check.mjs";

test("manual friction harness no longer exposes embedding setup", () => {
  const help = spawnSync(process.execPath, [script, "--help"], { encoding: "utf8" });
  assert.equal(help.status, 0, help.stderr);
  assert.doesNotMatch(help.stdout, /setup-embeddings|setup embeddings/iu);

  const removed = spawnSync(process.execPath, [script, "--setup-embeddings"], { encoding: "utf8" });
  assert.equal(removed.status, 2);
  assert.match(removed.stderr, /Unknown argument: --setup-embeddings/u);
});
