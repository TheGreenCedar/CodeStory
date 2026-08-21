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

const BUILD_ARGUMENTS = ["build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"];

function git(root, ...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

function nativeArtifact(root) {
  return path.join(root, "target", "release", process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
}

function installFakeCargo(harnessRoot) {
  const bin = path.join(harnessRoot, "fake-bin");
  const source = path.join(bin, "fake-cargo.rs");
  const executable = path.join(bin, process.platform === "win32" ? "cargo.exe" : "cargo");
  fs.mkdirSync(bin, { recursive: true });
  fs.writeFileSync(source, String.raw`
use std::{env, fs, path::PathBuf, process};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let trace = PathBuf::from(env::var_os("CODESTORY_FAKE_CARGO_TRACE").expect("trace path"));
    fs::write(&trace, args.join("\n")).expect("write argument trace");
    fs::write(format!("{}.cwd", trace.display()), env::current_dir().expect("cwd").display().to_string())
        .expect("write cwd trace");
    let mode = env::var("CODESTORY_FAKE_CARGO_MODE").unwrap_or_else(|_| "success".to_string());
    if mode == "fail" {
        process::exit(17);
    }
    let target_index = args.iter().position(|arg| arg == "--target-dir").expect("target-dir argument");
    let release_dir = env::current_dir()
        .expect("cwd")
        .join(&args[target_index + 1])
        .join("release");
    fs::create_dir_all(&release_dir).expect("create release directory");
    let artifact = release_dir.join(if cfg!(windows) { "codestory-cli.exe" } else { "codestory-cli" });
    if mode == "missing" {
        return;
    }
    if mode == "wrong" {
        fs::write(release_dir.join("not-codestory-cli"), b"wrong artifact").expect("write wrong artifact");
        return;
    }
    if mode == "symlink" {
        #[cfg(unix)]
        std::os::unix::fs::symlink(env::current_dir().unwrap().join("Cargo.toml"), &artifact).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(env::current_dir().unwrap().join("Cargo.toml"), &artifact).unwrap();
        return;
    }
    if mode == "hardlink" {
        fs::hard_link(env::current_dir().unwrap().join("Cargo.toml"), &artifact).unwrap();
        return;
    }
    fs::write(&artifact, b"exact source build").expect("write artifact");
    if mode == "mutate_source" {
        fs::write(env::current_dir().unwrap().join("Cargo.toml"), b"[workspace]\n# changed\n").unwrap();
    }
}
`, "utf8");
  execFileSync("rustc", [source, "-o", executable]);
  return bin;
}

function installGitOnlyPath(harnessRoot) {
  const bin = path.join(harnessRoot, "git-only-bin");
  const resolver = process.platform === "win32" ? "where.exe" : "which";
  const gitExecutable = execFileSync(resolver, ["git"], { encoding: "utf8" }).trim().split(/\r?\n/u)[0];
  fs.mkdirSync(bin, { recursive: true });
  if (process.platform === "win32") {
    fs.writeFileSync(path.join(bin, "git.cmd"), `@"${gitExecutable}" %*\r\n`, "utf8");
  } else {
    fs.symlinkSync(gitExecutable, path.join(bin, "git"));
  }
  return bin;
}

async function sourceFixture(prefix) {
  const harnessRoot = await mkdtemp(path.join(os.tmpdir(), prefix));
  const root = path.join(harnessRoot, "repository");
  fs.mkdirSync(root);
  await writeFile(path.join(root, "Cargo.toml"), "[workspace]\n", "utf8");
  await writeFile(path.join(root, ".gitignore"), "target/\n", "utf8");
  git(root, "init", "-q");
  git(root, "config", "user.email", "fixture@example.invalid");
  git(root, "config", "user.name", "Fixture");
  git(root, "add", ".");
  git(root, "commit", "-qm", "fixture");
  return {
    harnessRoot,
    root,
    fakeCargoBin: installFakeCargo(harnessRoot),
    gitOnlyBin: installGitOnlyPath(harnessRoot),
  };
}

function withFakeCargo({ harnessRoot, fakeCargoBin }, mode, callback) {
  const originalPath = process.env.PATH;
  const originalMode = process.env.CODESTORY_FAKE_CARGO_MODE;
  const originalTrace = process.env.CODESTORY_FAKE_CARGO_TRACE;
  const trace = path.join(harnessRoot, `cargo-${mode}-${Date.now()}-${Math.random()}.json`);
  process.env.PATH = `${fakeCargoBin}${path.delimiter}${originalPath || ""}`;
  process.env.CODESTORY_FAKE_CARGO_MODE = mode;
  process.env.CODESTORY_FAKE_CARGO_TRACE = trace;
  try {
    return callback(trace);
  } finally {
    if (originalPath === undefined) delete process.env.PATH;
    else process.env.PATH = originalPath;
    if (originalMode === undefined) delete process.env.CODESTORY_FAKE_CARGO_MODE;
    else process.env.CODESTORY_FAKE_CARGO_MODE = originalMode;
    if (originalTrace === undefined) delete process.env.CODESTORY_FAKE_CARGO_TRACE;
    else process.env.CODESTORY_FAKE_CARGO_TRACE = originalTrace;
  }
}

test("source-build provenance executes the fixed Cargo build and binds its artifact", async () => {
  const fixture = await sourceFixture("codestory-source-build-");
  const artifact = nativeArtifact(fixture.root);
  try {
    const record = withFakeCargo(fixture, "success", (trace) => {
      const value = recordSourceBuildProvenance({ repoRoot: fixture.root });
      assert.deepEqual(fs.readFileSync(trace, "utf8").split("\n"), BUILD_ARGUMENTS);
      assert.equal(fs.readFileSync(`${trace}.cwd`, "utf8"), fs.realpathSync(fixture.root));
      return value;
    });
    assert.equal(record.source.head, git(fixture.root, "rev-parse", "HEAD"));
    assert.equal(record.source.tree, git(fixture.root, "rev-parse", "HEAD^{tree}"));
    assert.equal(
      record.artifact.path,
      process.platform === "win32" ? "target/release/codestory-cli.exe" : "target/release/codestory-cli",
    );
    assert.deepEqual(record.build.command, ["cargo", ...BUILD_ARGUMENTS]);
    assert.deepEqual(
      compareSourceBuildProvenance(record, {
        installerSha256: record.artifact.sha256,
        liveSha256: record.artifact.sha256,
        expectedSource: record.source,
      }),
      { state: "bound" },
    );

    withFakeCargo(fixture, "success", (trace) => {
      assert.throws(
        () => recordSourceBuildProvenance({ repoRoot: fixture.root }),
        /source_build_artifact_must_be_absent/u,
      );
      assert.equal(fs.existsSync(trace), false, "a pre-existing artifact must stop before Cargo executes");
    });

    await rm(artifact);
    let callerRunnerInvoked = false;
    assert.throws(
      () => withFakeCargo(fixture, "fail", () => recordSourceBuildProvenance({
        repoRoot: fixture.root,
        runBuild() {
          callerRunnerInvoked = true;
          fs.mkdirSync(path.dirname(artifact), { recursive: true });
          fs.writeFileSync(artifact, "forged callback bytes");
          return { status: 0 };
        },
      })),
      /source_build_command_failed/u,
      "a production caller cannot replace Cargo execution",
    );
    assert.equal(callerRunnerInvoked, false);
    assert.equal(fs.existsSync(artifact), false);

    const originalPath = process.env.PATH;
    process.env.PATH = fixture.gitOnlyBin;
    try {
      assert.throws(
        () => recordSourceBuildProvenance({ repoRoot: fixture.root }),
        /source_build_command_failed/u,
        "a Cargo executable that cannot be resolved must not produce a record",
      );
    } finally {
      if (originalPath === undefined) delete process.env.PATH;
      else process.env.PATH = originalPath;
    }
    assert.equal(fs.existsSync(artifact), false);
  } finally {
    await rm(fixture.harnessRoot, { recursive: true, force: true });
  }
});

test("source-build provenance rejects failed commands, wrong outputs, source drift, and incomplete bindings", async () => {
  const fixture = await sourceFixture("codestory-source-build-hostile-");
  const artifact = nativeArtifact(fixture.root);
  try {
    for (const [mode, error] of [
      ["fail", /source_build_command_failed/u],
      ["missing", /source_build_artifact_not_file/u],
      ["wrong", /source_build_artifact_not_file/u],
      ["symlink", /source_build_artifact_not_direct_regular_file/u],
      ["hardlink", /source_build_artifact_not_direct_regular_file/u],
      ["mutate_source", /source_build_repository_not_clean/u],
    ]) {
      assert.throws(
        () => withFakeCargo(fixture, mode, () => recordSourceBuildProvenance({ repoRoot: fixture.root })),
        error,
        mode,
      );
      await rm(artifact, { force: true });
      if (mode === "mutate_source") git(fixture.root, "checkout", "--", "Cargo.toml");
    }

    const record = withFakeCargo(fixture, "success", () => recordSourceBuildProvenance({ repoRoot: fixture.root }));
    const boundInputs = {
      installerSha256: record.artifact.sha256,
      liveSha256: record.artifact.sha256,
      expectedSource: record.source,
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
      assert.deepEqual(compareSourceBuildProvenance({ ...record, ...mutation }, boundInputs), { state: "invalid_record" });
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
      compareSourceBuildProvenance({ ...record, artifact: { ...record.artifact, path: "target/release/codestory-cli.exe" } }, boundInputs),
      { state: "bound" },
      "the fixed native Windows artifact name is admitted without allowing arbitrary paths",
    );
  } finally {
    await rm(fixture.harnessRoot, { recursive: true, force: true });
  }
});

test("source-build provenance rejects artifact replacement between validation and hashing", async () => {
  const fixture = await sourceFixture("codestory-source-build-swap-");
  const artifact = nativeArtifact(fixture.root);
  const replacement = path.join(fixture.root, "target", "release", "replacement");
  const originalReadFileSync = fs.readFileSync;
  const originalReadSync = fs.readSync;
  let replaced = false;
  try {
    const replaceArtifact = () => {
      if (replaced) return;
      fs.writeFileSync(replacement, "replacement bytes");
      fs.renameSync(replacement, artifact);
      replaced = true;
    };
    fs.readFileSync = function readFileSyncWithReplacement(file, ...args) {
      if ((typeof file === "number" || path.resolve(file) === artifact) && !replaced) replaceArtifact();
      return originalReadFileSync.call(this, file, ...args);
    };
    fs.readSync = function readSyncWithReplacement(...args) {
      replaceArtifact();
      return originalReadSync.apply(this, args);
    };

    assert.throws(
      () => withFakeCargo(fixture, "success", () => recordSourceBuildProvenance({ repoRoot: fixture.root })),
      /source_build_artifact_changed/u,
    );
    assert.equal(replaced, true, "the hostile replacement must run during artifact hashing");
  } finally {
    fs.readFileSync = originalReadFileSync;
    fs.readSync = originalReadSync;
    await rm(fixture.harnessRoot, { recursive: true, force: true });
  }
});
