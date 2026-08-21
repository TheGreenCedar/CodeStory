import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";

const SHA256 = /^[0-9a-f]{64}$/u;

function git(repoRoot, ...args) {
  return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8" }).trim();
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function cleanRepository(repoRoot) {
  const status = git(repoRoot, "status", "--porcelain=v1", "--untracked-files=all");
  if (status) throw new Error(`source_build_repository_not_clean:${status.split(/\r?\n/u)[0]}`);
}

function exactBuildCommand(command) {
  const expected = ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"];
  if (!Array.isArray(command) || command.length !== expected.length || command.some((entry, index) => entry !== expected[index])) {
    throw new Error("source_build_command_not_exact");
  }
}

function safeArtifactRelative(relative) {
  const components = relative.split("/");
  return relative === "target/release/codestory-cli"
    && components.every((component) => component && component !== "." && component !== ".." && !component.includes("\\"));
}

export function recordSourceBuildProvenance({ artifact, buildCommand, repoRoot, runBuild }) {
  const root = path.resolve(repoRoot);
  const artifactPath = path.resolve(artifact);
  exactBuildCommand(buildCommand);
  const relative = path.relative(root, artifactPath);
  const normalizedRelative = relative.split(path.sep).join("/");
  if (!safeArtifactRelative(normalizedRelative)) {
    throw new Error("source_build_artifact_outside_repository");
  }
  if (fs.existsSync(artifactPath)) throw new Error("source_build_artifact_must_be_absent");
  cleanRepository(root);
  const before = { head: git(root, "rev-parse", "HEAD"), tree: git(root, "rev-parse", "HEAD^{tree}") };
  const completed = (runBuild || ((command, options) => spawnSync(command[0], command.slice(1), {
    ...options,
    encoding: "utf8",
    shell: false,
  })))(buildCommand, { cwd: root });
  if (completed.error || completed.status !== 0) throw new Error("source_build_command_failed");
  if (!fs.existsSync(artifactPath) || !fs.statSync(artifactPath).isFile()) {
    throw new Error("source_build_artifact_not_file");
  }
  const after = { head: git(root, "rev-parse", "HEAD"), tree: git(root, "rev-parse", "HEAD^{tree}") };
  if (before.head !== after.head || before.tree !== after.tree) throw new Error("source_build_source_changed");
  const metadata = fs.statSync(artifactPath);
  return {
    schema_version: 1,
    purpose: "codestory-source-build-provenance",
    source: {
      ...before,
    },
    build: { command: [...buildCommand] },
    artifact: {
      path: normalizedRelative,
      bytes: metadata.size,
      sha256: sha256(artifactPath),
    },
  };
}

export function compareSourceBuildProvenance(record, { installerSha256, liveSha256 }) {
  const command = record?.build?.command;
  const artifactPath = record?.artifact?.path;
  if (
    record?.schema_version !== 1
    || record?.purpose !== "codestory-source-build-provenance"
    || !/^[0-9a-f]{40}$/u.test(record?.source?.head || "")
    || !/^[0-9a-f]{40}$/u.test(record?.source?.tree || "")
    || !Array.isArray(command)
    || command.join("\0") !== ["cargo", "build", "--locked", "-p", "codestory-cli", "--profile", "release", "--target-dir", "target"].join("\0")
    || typeof artifactPath !== "string"
    || !safeArtifactRelative(artifactPath)
    || !Number.isSafeInteger(record?.artifact?.bytes)
    || record.artifact.bytes <= 0
    || !SHA256.test(record?.artifact?.sha256 || "")
  ) {
    return { state: "invalid_record" };
  }
  if (!SHA256.test(installerSha256 || "") || !SHA256.test(liveSha256 || "")) {
    return { state: "identity_unavailable" };
  }
  if (record.artifact.sha256 !== installerSha256) return { state: "installer_mismatch" };
  if (record.artifact.sha256 !== liveSha256) return { state: "live_mismatch" };
  return { state: "bound" };
}
