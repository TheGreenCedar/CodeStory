import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync, spawnSync } from "node:child_process";

const SHA256 = /^[0-9a-f]{64}$/u;
const BUILD_COMMAND = Object.freeze([
  "cargo",
  "build",
  "--locked",
  "-p",
  "codestory-cli",
  "--profile",
  "release",
  "--target-dir",
  "target",
]);

function git(repoRoot, ...args) {
  return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8" }).trim();
}

function artifactIdentityMatches(left, right) {
  return left.dev === right.dev && left.ino === right.ino;
}

function artifactMetadataMatches(left, right) {
  return artifactIdentityMatches(left, right)
    && left.size === right.size
    && left.mtimeNs === right.mtimeNs
    && left.ctimeNs === right.ctimeNs;
}

function cleanRepository(repoRoot) {
  const status = git(repoRoot, "status", "--porcelain=v1", "--untracked-files=all");
  if (status) throw new Error(`source_build_repository_not_clean:${status.split(/\r?\n/u)[0]}`);
}

function safeArtifactRelative(relative) {
  const components = relative.split("/");
  return ["target/release/codestory-cli", "target/release/codestory-cli.exe"].includes(relative)
    && components.every((component) => component && component !== "." && component !== ".." && !component.includes("\\"));
}

function artifactEntryExists(file) {
  try { fs.lstatSync(file); return true; } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

function directArtifactMetadata(root, relative) {
  let current = root;
  const components = relative.split("/");
  for (const [index, component] of components.entries()) {
    current = path.join(current, component);
    const entry = fs.lstatSync(current, { bigint: true });
    const final = index === components.length - 1;
    if (entry.isSymbolicLink()) throw new Error("source_build_artifact_not_direct_regular_file");
    if (!final && !entry.isDirectory()) throw new Error("source_build_artifact_not_direct_regular_file");
    if (final && (!entry.isFile() || entry.nlink !== 1n)) {
      throw new Error("source_build_artifact_not_direct_regular_file");
    }
  }
  return fs.lstatSync(current, { bigint: true });
}

export function recordSourceBuildProvenance({ repoRoot }) {
  const root = path.resolve(repoRoot);
  const normalizedRelative = process.platform === "win32"
    ? "target/release/codestory-cli.exe"
    : "target/release/codestory-cli";
  const artifactPath = path.join(root, ...normalizedRelative.split("/"));
  if (artifactEntryExists(artifactPath)) throw new Error("source_build_artifact_must_be_absent");
  cleanRepository(root);
  const before = { head: git(root, "rev-parse", "HEAD"), tree: git(root, "rev-parse", "HEAD^{tree}") };
  const completed = spawnSync(BUILD_COMMAND[0], BUILD_COMMAND.slice(1), {
    cwd: root,
    encoding: "utf8",
    shell: false,
  });
  if (completed.error || completed.status !== 0) throw new Error("source_build_command_failed");
  let pathnameMetadata;
  try { pathnameMetadata = directArtifactMetadata(root, normalizedRelative); } catch (error) {
    if (error?.code === "ENOENT") throw new Error("source_build_artifact_not_file");
    throw error;
  }
  if (!pathnameMetadata.isFile()) {
    throw new Error("source_build_artifact_not_file");
  }
  let descriptor;
  try {
    descriptor = fs.openSync(artifactPath, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
    const openedMetadata = fs.fstatSync(descriptor, { bigint: true });
    if (!openedMetadata.isFile() || openedMetadata.nlink !== 1n || !artifactIdentityMatches(pathnameMetadata, openedMetadata)) {
      throw new Error("source_build_artifact_changed");
    }
    const after = { head: git(root, "rev-parse", "HEAD"), tree: git(root, "rev-parse", "HEAD^{tree}") };
    if (before.head !== after.head || before.tree !== after.tree) throw new Error("source_build_source_changed");
    cleanRepository(root);
    const contents = fs.readFileSync(descriptor);
    const hashedMetadata = fs.fstatSync(descriptor, { bigint: true });
    let finalPathnameMetadata;
    try { finalPathnameMetadata = directArtifactMetadata(root, normalizedRelative); } catch (error) {
      if (error?.code === "ENOENT") throw new Error("source_build_artifact_changed");
      throw error;
    }
    if (
      !artifactMetadataMatches(openedMetadata, hashedMetadata)
      || !artifactIdentityMatches(hashedMetadata, finalPathnameMetadata)
      || BigInt(contents.byteLength) !== hashedMetadata.size
    ) {
      throw new Error("source_build_artifact_changed");
    }
    return {
      schema_version: 1,
      purpose: "codestory-source-build-provenance",
      source: {
        ...before,
      },
      build: { command: [...BUILD_COMMAND] },
      artifact: {
        path: normalizedRelative,
        bytes: contents.byteLength,
        sha256: createHash("sha256").update(contents).digest("hex"),
      },
    };
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
}

export function compareSourceBuildProvenance(record, { installerSha256, liveSha256, expectedSource }) {
  const command = record?.build?.command;
  const artifactPath = record?.artifact?.path;
  if (
    record?.schema_version !== 1
    || record?.purpose !== "codestory-source-build-provenance"
    || !/^[0-9a-f]{40}$/u.test(record?.source?.head || "")
    || !/^[0-9a-f]{40}$/u.test(record?.source?.tree || "")
    || !Array.isArray(command)
    || command.join("\0") !== BUILD_COMMAND.join("\0")
    || typeof artifactPath !== "string"
    || !safeArtifactRelative(artifactPath)
    || !Number.isSafeInteger(record?.artifact?.bytes)
    || record.artifact.bytes <= 0
    || !SHA256.test(record?.artifact?.sha256 || "")
  ) {
    return { state: "invalid_record" };
  }
  if (!expectedSource || !/^[0-9a-f]{40}$/u.test(expectedSource.head || "") || !/^[0-9a-f]{40}$/u.test(expectedSource.tree || "")) {
    return { state: "expected_identity_unavailable" };
  }
  if (record.source.head !== expectedSource.head || record.source.tree !== expectedSource.tree) return { state: "source_mismatch" };
  if (!SHA256.test(installerSha256 || "") || !SHA256.test(liveSha256 || "")) {
    return { state: "identity_unavailable" };
  }
  if (record.artifact.sha256 !== installerSha256) return { state: "installer_mismatch" };
  if (record.artifact.sha256 !== liveSha256) return { state: "live_mismatch" };
  return { state: "bound" };
}
