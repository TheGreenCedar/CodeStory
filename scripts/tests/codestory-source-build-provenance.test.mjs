import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
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
    await writeFile(path.join(root, "Cargo.toml"), "[workspace]\n", "utf8");
    await writeFile(path.join(root, ".gitignore"), "target/\n", "utf8");
    const artifact = path.join(root, "target", "release", "codestory-cli");
    git(root, "init", "-q");
    git(root, "config", "user.email", "fixture@example.invalid");
    git(root, "config", "user.name", "Fixture");
    git(root, "add", ".");
    git(root, "commit", "-qm", "fixture");

    let buildRuns = 0;
    const record = recordSourceBuildProvenance({
      artifact,
      buildCommand: ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"],
      repoRoot: root,
      runBuild(command, options) {
        buildRuns += 1;
        assert.deepEqual(command, ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"]);
        assert.equal(options.cwd, root);
        fs.mkdirSync(path.dirname(artifact), { recursive: true });
        fs.writeFileSync(artifact, "exact source build");
        return { status: 0 };
      },
    });
    assert.equal(buildRuns, 1);
    assert.equal(record.source.head, git(root, "rev-parse", "HEAD"));
    assert.equal(record.source.tree, git(root, "rev-parse", "HEAD^{tree}"));
    assert.equal(record.artifact.path, "target/release/codestory-cli");
    assert.equal(record.build.command.join(" "), "cargo build --locked -p codestory-cli --profile release --target-dir target");
    assert.deepEqual(
      compareSourceBuildProvenance(record, {
        installerSha256: record.artifact.sha256,
        liveSha256: record.artifact.sha256,
        expectedSource: record.source,
        expectedBuildCommand: record.build.command,
      }),
      { state: "bound" },
    );
    assert.throws(
      () => recordSourceBuildProvenance({
        artifact,
        buildCommand: ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"],
        repoRoot: root,
        runBuild: () => ({ status: 0 }),
      }),
      /source_build_artifact_must_be_absent/u,
      "the artifact must be created by this build invocation",
    );
    assert.deepEqual(
      compareSourceBuildProvenance({
        purpose: "codestory-source-build-provenance",
        artifact: { sha256: record.artifact.sha256 },
      }, {
        installerSha256: record.artifact.sha256,
        liveSha256: record.artifact.sha256,
      }),
      { state: "invalid_record" },
      "an artifact hash plus a claimed purpose is not source-build provenance",
    );

    await rm(artifact);
    await writeFile(path.join(root, "dirty"), "not clean", "utf8");
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"], repoRoot: root }),
      /source_build_repository_not_clean/u,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("source-build provenance rejects forged commands, paths, outputs, and incomplete bindings", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "codestory-source-build-hostile-"));
  try {
    await writeFile(path.join(root, "Cargo.toml"), "[workspace]\n", "utf8");
    await writeFile(path.join(root, ".gitignore"), "target/\n", "utf8");
    git(root, "init", "-q");
    git(root, "config", "user.email", "fixture@example.invalid");
    git(root, "config", "user.name", "Fixture");
    git(root, "add", ".");
    git(root, "commit", "-qm", "fixture");
    const artifact = path.join(root, "target", "release", "codestory-cli");
    const command = ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"];

    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: ["true", "--locked"], repoRoot: root, runBuild: () => ({ status: 0 }) }),
      /source_build_command_not_exact/u,
    );
    assert.throws(
      () => recordSourceBuildProvenance({ artifact: path.join(root, "target", "..", "..", "escape"), buildCommand: command, repoRoot: root, runBuild: () => ({ status: 0 }) }),
      /source_build_artifact_outside_repository/u,
    );
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: command, repoRoot: root, runBuild: () => ({ status: 0 }) }),
      /source_build_artifact_not_file/u,
    );
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: command, repoRoot: root, runBuild: () => {
        fs.mkdirSync(path.dirname(artifact), { recursive: true });
        fs.symlinkSync(path.join(root, "Cargo.toml"), artifact);
        return { status: 0 };
      } }),
      /source_build_artifact_not_direct_regular_file/u,
    );
    await rm(artifact);
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: command, repoRoot: root, runBuild: () => {
        fs.linkSync(path.join(root, "Cargo.toml"), artifact);
        return { status: 0 };
      } }),
      /source_build_artifact_not_direct_regular_file/u,
    );
    await rm(artifact);
    assert.throws(
      () => recordSourceBuildProvenance({ artifact, buildCommand: command, repoRoot: root, runBuild: () => {
        fs.mkdirSync(path.dirname(artifact), { recursive: true });
        fs.writeFileSync(artifact, "runner-produced bytes");
        fs.writeFileSync(path.join(root, "Cargo.toml"), "[workspace]\n# changed\n");
        return { status: 0 };
      } }),
      /source_build_repository_not_clean/u,
    );
    git(root, "checkout", "--", "Cargo.toml");
    await rm(artifact);

    const record = recordSourceBuildProvenance({
      artifact,
      buildCommand: command,
      repoRoot: root,
      runBuild: () => {
        fs.mkdirSync(path.dirname(artifact), { recursive: true });
        fs.writeFileSync(artifact, "runner-produced bytes");
        return { status: 0 };
      },
    });
    const boundInputs = {
      installerSha256: record.artifact.sha256,
      liveSha256: record.artifact.sha256,
      expectedSource: record.source,
      expectedBuildCommand: record.build.command,
    };
    for (const mutation of [
      { schema_version: 2 },
      { source: { ...record.source, head: "not-a-head" } },
      { source: { ...record.source, tree: "not-a-tree" } },
      { build: { command: ["cargo", "test", "--locked"] } },
      { artifact: { ...record.artifact, path: "target/../../escape" } },
      { artifact: { ...record.artifact, bytes: 0 } },
      { artifact: { ...record.artifact, sha256: "not-a-sha" } },
    ]) {
      assert.deepEqual(
        compareSourceBuildProvenance({ ...record, ...mutation }, boundInputs),
        { state: "invalid_record" },
      );
    }
    assert.deepEqual(
      compareSourceBuildProvenance(record, { ...boundInputs, installerSha256: "a".repeat(64) }),
      { state: "installer_mismatch" },
    );
    assert.deepEqual(
      compareSourceBuildProvenance(record, { ...boundInputs, liveSha256: "b".repeat(64) }),
      { state: "live_mismatch" },
    );
    assert.deepEqual(
      compareSourceBuildProvenance(record, { ...boundInputs, expectedSource: { ...record.source, head: "a".repeat(40) } }),
      { state: "source_mismatch" },
    );
    assert.deepEqual(
      compareSourceBuildProvenance(record, { ...boundInputs, expectedBuildCommand: ["cargo", "test", "--locked"] }),
      { state: "build_command_mismatch" },
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
