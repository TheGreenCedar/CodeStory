import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

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

export function recordSourceBuildProvenance({ artifact, buildCommand, repoRoot }) {
  const root = path.resolve(repoRoot);
  const artifactPath = path.resolve(artifact);
  if (!Array.isArray(buildCommand) || buildCommand.length === 0 || !buildCommand.includes("--locked")) {
    throw new Error("source_build_command_must_be_locked");
  }
  if (!fs.statSync(artifactPath).isFile()) throw new Error("source_build_artifact_not_file");
  const relative = path.relative(root, artifactPath);
  if (!relative || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error("source_build_artifact_outside_repository");
  }
  cleanRepository(root);
  const metadata = fs.statSync(artifactPath);
  return {
    schema_version: 1,
    purpose: "codestory-source-build-provenance",
    source: {
      head: git(root, "rev-parse", "HEAD"),
      tree: git(root, "rev-parse", "HEAD^{tree}"),
    },
    build: { command: [...buildCommand] },
    artifact: {
      path: relative.split(path.sep).join("/"),
      bytes: metadata.size,
      sha256: sha256(artifactPath),
    },
  };
}

export function compareSourceBuildProvenance(record, { installerSha256, liveSha256 }) {
  if (record?.purpose !== "codestory-source-build-provenance" || !SHA256.test(record?.artifact?.sha256 || "")) {
    return { state: "invalid_record" };
  }
  if (!SHA256.test(installerSha256 || "") || !SHA256.test(liveSha256 || "")) {
    return { state: "identity_unavailable" };
  }
  if (record.artifact.sha256 !== installerSha256) return { state: "installer_mismatch" };
  if (record.artifact.sha256 !== liveSha256) return { state: "live_mismatch" };
  return { state: "bound" };
}
