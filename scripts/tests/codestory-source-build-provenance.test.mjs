import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  compareSourceBuildProvenance,
  recordSourceBuildProvenance,
} from "../lib/codestory-source-build-provenance.mjs";

function git(root, ...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

test("source-build provenance binds a clean exact tree to artifact installer and live hashes", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-source-build-"));
  try {
    await mkdir(path.join(root, "target"), { recursive: true });
    await writeFile(path.join(root, "Cargo.toml"), "[workspace]\n", "utf8");
    const artifact = path.join(root, "target", "codestory-cli");
    await writeFile(artifact, "exact source build", "utf8");
    git(root, "init", "-q");
    git(root, "config", "user.email", "fixture@example.invalid");
    git(root, "config", "user.name", "Fixture");
    git(root, "add", ".");
    git(root, "commit", "-qm", "fixture");

    const record = recordSourceBuildProvenance({
      artifact,
      buildCommand: ["cargo", "build", "--locked", "-p", "codestory-cli"],
      repoRoot: root,
    });
    assert.equal(record.source.head, git(root, "rev-parse", "HEAD"));
    assert.equal(record.source.tree, git(root, "rev-parse", "HEAD^{tree}"));
    assert.equal(record.artifact.path, "target/codestory-cli");
    assert.equal(record.build.command.join(" "), "cargo build --locked -p codestory-cli");
    assert.deepEqual(
      compareSourceBuildProvenance(record, {
        installerSha256: record.artifact.sha256,
        liveSha256: record.artifact.sha256,
      }),
      { state: "bound" },
    );

    await writeFile(path.join(root, "dirty"), "not clean", "utf8");
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: ["cargo", "build", "--locked"], repoRoot: root }),
      /source_build_repository_not_clean/u,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
