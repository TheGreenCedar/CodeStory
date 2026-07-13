import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  argumentsForPlatform,
  implementationForPlatform,
} from "../codex-worktree-setup.mjs";

const scriptsDirectory = dirname(dirname(fileURLToPath(import.meta.url)));

test("dispatcher selects PowerShell only on Windows", () => {
  const selected = implementationForPlatform("win32", scriptsDirectory);
  assert.equal(selected.command, "powershell");
  assert.deepEqual(selected.arguments.slice(0, 3), ["-NoProfile", "-ExecutionPolicy", "Bypass"]);
  assert.equal(selected.arguments.at(-1), join(scriptsDirectory, "codex-worktree-setup.ps1"));
});

test("dispatcher selects the POSIX setup on macOS and Linux", () => {
  for (const platform of ["darwin", "linux"]) {
    const selected = implementationForPlatform(platform, scriptsDirectory);
    assert.equal(selected.command, join(scriptsDirectory, "codex-worktree-setup.sh"));
    assert.deepEqual(selected.arguments, []);
  }
});

test("dispatcher translates portable options for PowerShell", () => {
  assert.deepEqual(
    argumentsForPlatform("win32", ["--project", "C:\\repo", "--self-test"]),
    ["-Project", "C:\\repo", "-SelfTest"],
  );
  assert.deepEqual(
    argumentsForPlatform("darwin", ["--project", "/tmp/repo", "--self-test"]),
    ["--project", "/tmp/repo", "--self-test"],
  );
});

test("dispatcher runs the host implementation self-test", () => {
  const result = spawnSync(process.execPath, [
    join(scriptsDirectory, "codex-worktree-setup.mjs"),
    "--self-test",
  ], { encoding: "utf8" });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.match(result.stdout, /codex-worktree-setup (POSIX )?self-test: ok/);
});

test("POSIX setup honors the branch-head proof environment override", () => {
  if (process.platform === "win32") return;
  const result = spawnSync(process.execPath, [
    join(scriptsDirectory, "codex-worktree-setup.mjs"),
    "--self-test",
  ], {
    encoding: "utf8",
    env: { ...process.env, CODESTORY_BRANCH_HEAD_PROOF: "yes" },
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
});
