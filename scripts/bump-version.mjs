#!/usr/bin/env node
// Set the release version across every surface that must agree.
//
// AGENTS.md lists these by hand and check-codestory-release.py fails the release when any one of
// them drifts, so doing it manually is a step that can only go wrong. This writes all of them and
// then runs the same validator the release does.
//
//   node scripts/bump-version.mjs --version 0.17.0
//   node scripts/bump-version.mjs --version 0.17.0 --check
//   node scripts/bump-version.mjs --version 0.17.1 --lane plugin
//   node scripts/bump-version.mjs --version 0.17.1 --lane plugin --check

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { parsePublishedArchiveDigests } from "./lib/pinned-archive-digests.mjs";
import { workspaceMemberNames } from "./lib/workspace-members.mjs";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const STABLE_RELEASE_VERSION = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u;

/// Every codestory-* crate, in workspace order, read from the workspace itself.
///
/// Derived rather than listed so that adding a crate cannot leave one version surface behind:
/// `check-codestory-release.py` and `native-fingerprint.mjs` read the same membership (#1673).
const WORKSPACE_MEMBERS = workspaceMemberNames(
  readFileSync(path.join(repositoryRoot, "Cargo.toml"), "utf8"),
);

const PLUGIN_MANIFESTS = [
  "plugins/codestory/plugin.json",
  "plugins/codestory/.codex-plugin/plugin.json",
  "plugins/codestory/.cursor-plugin/plugin.json",
  "plugins/codestory/.claude-plugin/plugin.json",
  "plugins/codestory/.github/plugin/plugin.json",
];

const MODEL_CONTRACT = "crates/codestory-llama-sys/model-contract.json";
const CHANGELOG = "CHANGELOG.md";
const CLI_VERSION_PIN = "plugins/codestory/cli-version.json";
const CARGO_LOCK = "Cargo.lock";
const RELEASE_REPOSITORY = "TheGreenCedar/CodeStory";
const PYTHON = process.env.CODESTORY_PYTHON || "python3";

function fail(message) {
  console.error(`bump-version: ${message}`);
  process.exit(1);
}

function parseArguments(argv) {
  const values = { check: false, lane: "native" };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--check") {
      values.check = true;
      continue;
    }
    const separator = argument.indexOf("=");
    const name = separator < 0 ? argument : argument.slice(0, separator);
    const inline = separator < 0 ? undefined : argument.slice(separator + 1);
    const next = argv[index + 1];
    const value =
      inline ?? (next && !next.startsWith("--") ? argv[++index] : undefined);
    if (!value) fail(`${name} requires a value`);
    if (name === "--version") values.version = value;
    else if (name === "--lane") values.lane = value;
    else if (name === "--archive-checksums") values.archiveChecksums = value;
    else fail(`unknown argument ${argument}`);
  }
  if (!values.version) fail("--version is required");
  values.version = values.version.replace(/^v/u, "");
  if (!STABLE_RELEASE_VERSION.test(values.version)) {
    fail(`--version must be a stable release version like 0.17.0, got ${values.version}`);
  }
  if (!new Set(["native", "plugin"]).has(values.lane)) {
    fail(`--lane must be native or plugin, got ${values.lane}`);
  }
  if (values.lane === "native" && values.archiveChecksums) {
    fail("--archive-checksums belongs only to the plugin lane");
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

function packageVersion(source) {
  const packageSection = /(^\[package\][\s\S]*?^version\s*=\s*")([^"]+)(")/mu;
  const version = packageSection.exec(source)?.[2];
  if (!version) fail("crate manifest has no [package] version");
  return version;
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

function cargoLockVersions(source) {
  const versions = new Map();
  for (const block of source.split(/(?=^\[\[package\]\]$)/mu)) {
    const name = /^name\s*=\s*"([^"]+)"/mu.exec(block)?.[1];
    const version = /^version\s*=\s*"([^"]+)"/mu.exec(block)?.[1];
    if (!name || !version || !WORKSPACE_MEMBERS.includes(name)) continue;
    if (versions.has(name)) fail(`Cargo.lock contains duplicate workspace package ${name}`);
    versions.set(name, version);
  }
  for (const member of WORKSPACE_MEMBERS) {
    if (!versions.has(member)) fail(`Cargo.lock has no package entry for ${member}`);
  }
  return versions;
}

function cargoLockCarriesVersion(source, version) {
  return [...cargoLockVersions(source).values()].every((current) => current === version);
}

function compareSemverCore(left, right) {
  const parse = (value) => value.split("-")[0].split(".").map(Number);
  const [leftMajor, leftMinor, leftPatch] = parse(left);
  const [rightMajor, rightMinor, rightPatch] = parse(right);
  if (leftMajor !== rightMajor) return leftMajor - rightMajor;
  if (leftMinor !== rightMinor) return leftMinor - rightMinor;
  return leftPatch - rightPatch;
}

async function publishedArchiveDigests(pin, checksumSource) {
  if (
    !pin ||
    typeof pin.cli_version !== "string" ||
    !STABLE_RELEASE_VERSION.test(pin.cli_version) ||
    pin.release_tag !== `v${pin.cli_version}`
  ) {
    fail(`${CLI_VERSION_PIN} does not name a valid published CLI release`);
  }
  const source =
    checksumSource ??
    `https://github.com/${RELEASE_REPOSITORY}/releases/download/` +
      `${encodeURIComponent(pin.release_tag)}/SHA256SUMS.txt`;
  let contents;
  if (/^https:\/\//u.test(source)) {
    const response = await fetch(source, { signal: AbortSignal.timeout(30_000) });
    if (!response.ok) {
      fail(`could not fetch ${source}: HTTP ${response.status}`);
    }
    contents = await response.text();
  } else {
    contents = readFileSync(path.resolve(repositoryRoot, source), "utf8");
  }
  try {
    return parsePublishedArchiveDigests(contents, pin.cli_version);
  } catch (error) {
    fail(error.message);
  }
}

async function main() {
  const { version, check, lane, archiveChecksums } = parseArguments(
    process.argv.slice(2),
  );
  const changes = [];
  const cargoLockBefore = readFileSync(path.join(repositoryRoot, CARGO_LOCK), "utf8");

  if (lane === "plugin") {
    const cliVersion = packageVersion(
      readFileSync(
        path.join(repositoryRoot, "crates/codestory-cli/Cargo.toml"),
        "utf8",
      ),
    );
    const pinSource = readFileSync(path.join(repositoryRoot, CLI_VERSION_PIN), "utf8");
    const pin = JSON.parse(pinSource);
    if (pin.cli_version !== cliVersion) {
      fail(
        `${CLI_VERSION_PIN} pins ${pin.cli_version}, but the workspace CLI is ${cliVersion}`,
      );
    }
    if (compareSemverCore(version, cliVersion) <= 0) {
      fail(`plugin version ${version} must be ahead of pinned CLI ${cliVersion}`);
    }
    const archives = await publishedArchiveDigests(pin, archiveChecksums);

    for (const manifest of PLUGIN_MANIFESTS) {
      rewrite(
        manifest,
        (source) => setJsonVersion(source, version, ["version"]),
        changes,
        { check },
      );
    }
    rewrite(
      CLI_VERSION_PIN,
      (source) => {
        const current = JSON.parse(source);
        const next = { ...current, archives };
        const after = `${JSON.stringify(next, null, 2)}\n`;
        return after === source ? source : after;
      },
      changes,
      { check },
    );
    rewrite(CHANGELOG, (source) => promoteChangelog(source, version), changes, { check });

    if (check) {
      if (changes.length > 0) {
        fail(`these plugin surfaces do not carry ${version}:\n  ${changes.join("\n  ")}`);
      }
      console.log(
        `Every plugin release surface already carries ${version} with pinned CLI digests.`,
      );
      return;
    }

    execFileSync(
      PYTHON,
      [
        ".github/scripts/check-codestory-release.py",
        "--version",
        version,
        "--lane",
        "plugin",
      ],
      { cwd: repositoryRoot, stdio: "inherit" },
    );
    console.log(
      changes.length > 0
        ? `Set plugin ${version} across ${changes.length} files:\n  ${changes.join("\n  ")}`
        : `Every plugin release surface already carried ${version}.`,
    );
    return;
  }

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
  // producer.version only. producer.embedding_revision keys persisted vectors and is left
  // alone on purpose: bumping it would discard every user's dense sidecars on a release that
  // changed nothing about embeddings.
  rewrite(
    MODEL_CONTRACT,
    (source) => setJsonVersion(source, version, ["producer", "version"]),
    changes,
    { check },
  );
  // A native release publishes new archives, so the pin follows the release version and its
  // digests are dropped: they do not exist until the release builds them. The plugin fast lane
  // re-adds digests when it pins an already-published CLI.
  rewrite(
    CLI_VERSION_PIN,
    (source) => {
      const pin = JSON.parse(source);
      if (pin.cli_version === version) return source;
      delete pin.archives;
      pin.cli_version = version;
      pin.release_tag = `v${version}`;
      return `${JSON.stringify(pin, null, 2)}\n`;
    },
    changes,
    { check },
  );
  rewrite(CHANGELOG, (source) => promoteChangelog(source, version), changes, { check });

  if (check) {
    if (!cargoLockCarriesVersion(cargoLockBefore, version)) changes.push(CARGO_LOCK);
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
  if (readFileSync(path.join(repositoryRoot, CARGO_LOCK), "utf8") !== cargoLockBefore) {
    changes.push(CARGO_LOCK);
  }

  // Fail here rather than in CI if a surface was missed.
  execFileSync(
    PYTHON,
    [".github/scripts/check-codestory-release.py", "--version", version],
    { cwd: repositoryRoot, stdio: "inherit" },
  );

  console.log(
    changes.length > 0
      ? `Set ${version} across ${changes.length} files:\n  ${changes.join("\n  ")}`
      : `Every release surface already carried ${version}.`,
  );
}

main().catch((error) => fail(error.message));
