#!/usr/bin/env node
// Build a marketplace catalog pinned to one commit of this repository.
//
// The release preflight proves the Codex install path works for the candidate it is about to
// publish. The live catalog cannot serve that proof: it points at the previous release until
// marketplace-publish updates it after publication. Requiring a live match beforehand is what
// forced someone to hand-edit another repository before every merge to main.
//
// The catalog produced here is byte-identical in shape to the live one, so the proof exercises
// the same resolver and the same pinned git-subdir source; only the commit differs.
//
//   node .github/scripts/build-marketplace-fixture.mjs --out DIR --source-repository DIR --commit SHA

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

import {
  FIXTURE_MARKER_FILENAME,
  FIXTURE_MARKER_PURPOSE,
} from "./marketplace-delivery-identity.mjs";

function fail(message) {
  console.error(`::error::${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value == null) fail(`invalid argument: ${key}`);
    values[key.slice(2).replaceAll("-", "_")] = value;
  }
  return values;
}

const args = parseArgs(process.argv.slice(2));
for (const key of ["out", "source_repository", "commit"]) {
  if (!args[key]) fail(`--${key.replaceAll("_", "-")} is required`);
}

const sourceRepository = path.resolve(args.source_repository);
const commit = execFileSync("git", ["-C", sourceRepository, "rev-parse", `${args.commit}^{commit}`], {
  encoding: "utf8",
}).trim();
if (!/^[0-9a-f]{40}$/u.test(commit)) fail("commit must resolve to an immutable Git identity");

const manifest = JSON.parse(
  execFileSync(
    "git",
    ["-C", sourceRepository, "show", `${commit}:plugins/codestory/.codex-plugin/plugin.json`],
    { encoding: "utf8" },
  ),
);
if (typeof manifest.version !== "string" || !manifest.version) {
  fail("pinned commit has no plugin version");
}

const fixtureRoot = path.resolve(args.out);
const catalogDirectory = path.join(fixtureRoot, ".agents", "plugins");
mkdirSync(catalogDirectory, { recursive: true });
// Mirrors the live catalog's shape at .agents/plugins/marketplace.json. The
// resolver rejects a catalog missing `name`, and the live catalog carries no
// version field at either level - the pinned `sha` is what selects the plugin,
// so inventing one here would prove an install path the real catalog cannot
// produce.
const catalog = {
  name: "TheGreenCedar",
  interface: {
    displayName: "TheGreenCedar",
  },
  plugins: [
    {
      name: "codestory",
      source: {
        source: "git-subdir",
        url: "https://github.com/TheGreenCedar/CodeStory.git",
        path: "plugins/codestory",
        sha: commit,
      },
      policy: {
        installation: "AVAILABLE",
        authentication: "ON_INSTALL",
      },
      category: "Developer Tools",
    },
  ],
};
writeFileSync(
  path.join(catalogDirectory, "marketplace.json"),
  `${JSON.stringify(catalog, null, 2)}\n`,
);

// A fixture catalog is a DISTINCT delivery state, not a stand-in that may pass for the live
// catalog, so it says so in its own bytes. The installed-runtime predicate refuses to accept a
// deferred installation unless the resolved marketplace root carries this marker naming the
// exact commit the catalog pins -- an arbitrary local git directory, or a clone of the live
// marketplace, cannot satisfy the deferred shape by accident.
//
// This file sits beside .agents/, not inside it: the resolver reads only
// .agents/plugins/marketplace.json, so the catalog the resolver sees stays byte-identical in
// shape to the live one.
writeFileSync(
  path.join(fixtureRoot, FIXTURE_MARKER_FILENAME),
  `${JSON.stringify(
    {
      schema_version: 1,
      purpose: FIXTURE_MARKER_PURPOSE,
      pinned_commit: commit,
      plugin_version: manifest.version,
    },
    null,
    2,
  )}\n`,
);

// The fixture must be a git repository: the Codex resolver reads it like the live catalog.
// It deliberately has NO `origin` remote. Pointing one at the live marketplace URL would make
// the fixture claim an identity it does not have, and the deferred predicate asserts the
// absence positively rather than tolerating a failed probe.
const git = (...command) =>
  execFileSync("git", ["-C", fixtureRoot, ...command], { encoding: "utf8" });
git("init", "--quiet", "--initial-branch", "main");
git("config", "user.email", "release@codestory.invalid");
git("config", "user.name", "CodeStory release");
git("add", "--all");
git("commit", "--quiet", "--message", `pin codestory ${manifest.version} at ${commit}`);

console.log(`Marketplace fixture pins codestory ${manifest.version} at ${commit}.`);
