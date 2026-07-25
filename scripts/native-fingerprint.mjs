#!/usr/bin/env node
// The native fingerprint: what the shipped binaries are built from, minus the version stamp.
//
// A release bump edits version lines in every crate manifest, Cargo.lock, and the model
// contract, so a naive tree hash always changes and could never justify inheriting evidence.
// This hash covers the native-relevant paths with the known version-bump lines normalized
// away: when two commits share a fingerprint, their native build inputs differ only by the
// version string embedded in the binaries. Accelerator behavior is invariant under that
// delta, which is exactly the claim `evidence_policy.native_reuse = "version_only_delta"`
// authorizes inheriting; packaging and signing still rerun because the bytes differ.
//
//   node scripts/native-fingerprint.mjs [--ref HEAD]

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/// Everything that shapes the built native artifacts.
const NATIVE_PATHS = [
  "crates",
  "vendor",
  "Cargo.toml",
  "Cargo.lock",
  ".cargo",
  "rust-toolchain.toml",
  ".github/scripts/package-codestory-release.py",
  ".github/scripts/install-linux-vulkan-build-deps.sh",
  ".github/scripts/install-windows-vulkan-sdk.ps1",
];

/// Files whose version-bump lines are normalized rather than hashed raw.
const VERSION_STAMPED = new Set([
  "Cargo.lock",
  "crates/codestory-llama-sys/model-contract.json",
  ...[
    "codestory-llama-sys",
    "codestory-contracts",
    "codestory-workspace",
    "codestory-store",
    "codestory-indexer",
    "codestory-retrieval",
    "codestory-runtime",
    "codestory-cli",
    "codestory-bench",
  ].map((crate) => `crates/${crate}/Cargo.toml`),
]);

function git(args) {
  return execFileSync("git", args, { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 1 << 28 });
}

/// Strip exactly the lines the version bump rewrites; everything else stays byte-exact.
function normalizeVersionStamp(relative, content) {
  if (relative === "Cargo.lock") {
    // Only codestory-* package versions move on a bump; foreign versions must stay significant.
    return content.replace(
      /(name = "codestory-[a-z-]+"\nversion = ")[^"]+(")/gu,
      "$1<bump>$2",
    );
  }
  if (relative.endsWith("model-contract.json")) {
    return content.replace(/("version"\s*:\s*")\d+\.\d+\.\d+[^"]*(")/u, "$1<bump>$2");
  }
  // Crate manifests: the [package] version line only.
  return content.replace(/(^\[package\][\s\S]*?^version\s*=\s*")[^"]+(")/mu, "$1<bump>$2");
}

function main() {
  const refIndex = process.argv.indexOf("--ref");
  const ref = refIndex >= 0 ? process.argv[refIndex + 1] : "HEAD";

  const listing = git(["ls-tree", "-r", ref, "--", ...NATIVE_PATHS])
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [meta, relative] = line.split("\t");
      const [mode, , objectId] = meta.split(/\s+/u);
      return { mode, objectId, relative };
    })
    .sort((left, right) => (left.relative < right.relative ? -1 : 1));

  if (listing.length === 0) {
    console.error("::error::native fingerprint saw no tracked native paths");
    process.exit(1);
  }

  const hash = createHash("sha256");
  hash.update("codestory-native-fingerprint-v1\0");
  for (const { mode, objectId, relative } of listing) {
    if (VERSION_STAMPED.has(relative)) {
      const content = git(["cat-file", "blob", objectId]);
      hash.update(`${mode} ${relative}\0`);
      hash.update(normalizeVersionStamp(relative, content));
      hash.update("\0");
    } else {
      // Unstamped files contribute their git object id, which already binds content.
      hash.update(`${mode} ${relative} ${objectId}\0`);
    }
  }
  console.log(hash.digest("hex"));
}

main();
