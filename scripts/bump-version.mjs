#!/usr/bin/env node
// Set the release version across every surface that must agree.
//
// AGENTS.md lists these by hand and check-codestory-release.py fails the release when any one of
// them drifts, so doing it manually is a step that can only go wrong. This writes all of them and
// then runs the same validator the release does.
//
//   node scripts/bump-version.mjs --version 0.17.0
//   node scripts/bump-version.mjs --version 0.17.0 --check

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u;

/// Every codestory-* crate, in workspace order.
const WORKSPACE_MEMBERS = [
  "codestory-llama-sys",
  "codestory-contracts",
  "codestory-workspace",
  "codestory-store",
  "codestory-indexer",
  "codestory-retrieval",
  "codestory-runtime",
  "codestory-cli",
  "codestory-bench",
];

const PLUGIN_MANIFESTS = [
  "plugins/codestory/.codex-plugin/plugin.json",
  "plugins/codestory/.claude-plugin/plugin.json",
  "plugins/codestory/.github/plugin/plugin.json",
];

const MODEL_CONTRACT = "crates/codestory-llama-sys/model-contract.json";
const CHANGELOG = "CHANGELOG.md";

function fail(message) {
  console.error(`bump-version: ${message}`);
  process.exit(1);
}

function parseArguments(argv) {
  const values = { check: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--check") {
      values.check = true;
      continue;
    }
    const [name, inline] = argument.split("=");
    if (name !== "--version") fail(`unknown argument ${argument}`);
    values.version = inline ?? argv[++index];
  }
  if (!values.version) fail("--version is required");
  values.version = values.version.replace(/^v/u, "");
  if (!SEMVER.test(values.version)) {
    fail(`--version must be strict semver like 0.17.0, got ${values.version}`);
  }
  return values;
}

/// Rewrite a file and report whether it changed, so --check can report drift without writing.
function rewrite(relative, transform, changes, { check }) {
  const absolute = path.join(repositoryRoot, relative);
  const before = readFileSync(absolute, "utf8");
  const after = transform(before);
  if (after === before) return;
  changes.push(relative);
  if (!check) writeFileSync(absolute, after);
}

/// Replace only the `[package]` version, never a dependency's.
function setPackageVersion(source, version) {
  const packageSection = /(^\[package\][\s\S]*?^version\s*=\s*")([^"]+)(")/mu;
  if (!packageSection.test(source)) fail("crate manifest has no [package] version");
  return source.replace(packageSection, `$1${version}$3`);
}

/// Replace one JSON string value in place.
///
/// Deliberately textual: reserializing would reformat files that are also read by other tooling and
/// reviewed as diffs, turning a one-line version bump into a whole-file rewrite.
function setJsonVersion(source, version, pointer) {
  let target = JSON.parse(source);
  for (const key of pointer.slice(0, -1)) target = target?.[key];
  if (!target || typeof target[pointer.at(-1)] !== "string") {
    fail(`missing string at ${pointer.join(".")}`);
  }
  const current = target[pointer.at(-1)];
  if (current === version) return source;

  const key = pointer.at(-1);
  const escaped = current.replaceAll(".", "\\.");
  const occurrence = new RegExp(`("${key}"\\s*:\\s*")${escaped}(")`, "gu");
  const matches = source.match(occurrence) ?? [];
  if (matches.length !== 1) {
    fail(`expected exactly one "${key}": "${current}" in the file, found ${matches.length}`);
  }
  return source.replace(occurrence, `$1${version}$2`);
}

/// Promote the `Unreleased` section to the version being released.
function promoteChangelog(source, version) {
  if (source.includes(`\n## ${version}\n`)) return source;
  if (!source.includes("\n## Unreleased\n")) {
    fail("CHANGELOG.md has no `## Unreleased` section to promote");
  }
  return source.replace("\n## Unreleased\n", `\n## Unreleased\n\n## ${version}\n`);
}

function main() {
  const { version, check } = parseArguments(process.argv.slice(2));
  const changes = [];

  for (const member of WORKSPACE_MEMBERS) {
    rewrite(
      `crates/${member}/Cargo.toml`,
      (source) => setPackageVersion(source, version),
      changes,
      { check },
    );
  }
  for (const manifest of PLUGIN_MANIFESTS) {
    rewrite(manifest, (source) => setJsonVersion(source, version, ["version"]), changes, {
      check,
    });
  }
  rewrite(
    MODEL_CONTRACT,
    (source) => setJsonVersion(source, version, ["producer", "version"]),
    changes,
    { check },
  );
  rewrite(CHANGELOG, (source) => promoteChangelog(source, version), changes, { check });

  if (check) {
    if (changes.length > 0) {
      fail(`these surfaces do not carry ${version}:\n  ${changes.join("\n  ")}`);
    }
    console.log(`Every release surface already carries ${version}.`);
    return;
  }

  // Cargo.lock records each workspace crate's version and is validated with the rest.
  execFileSync("cargo", ["update", "--workspace", "--offline"], {
    cwd: repositoryRoot,
    stdio: "inherit",
  });

  // Fail here rather than in CI if a surface was missed.
  execFileSync(
    "python3",
    [".github/scripts/check-codestory-release.py", "--version", version],
    { cwd: repositoryRoot, stdio: "inherit" },
  );

  console.log(
    changes.length > 0
      ? `Set ${version} across ${changes.length} files:\n  ${changes.join("\n  ")}`
      : `Every release surface already carried ${version}.`,
  );
}

main();
