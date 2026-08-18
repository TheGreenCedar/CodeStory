#!/usr/bin/env node
// Point the public marketplace catalog at a published CodeStory release.
//
// The catalog pins codestory to an exact commit of this repository. Until now a human edited it
// by hand in the other repository *before* merging to main, timed so the pinned commit's tree
// would still match after the merge. This runs after publication instead: the release exists, so
// the commit being pinned is one whose plugin sources are already downloadable.
//
// Idempotent: if the catalog already names this commit, it succeeds without pushing, which is
// what makes marketplace-sync.yml a safe recovery for a failed publish.
//
//   node .github/scripts/publish-marketplace-catalog.mjs \
//     --source-repository DIR --commit SHA --version X.Y.Z [--github-output FILE]
//
// Automatic restore supplies the just-pushed catalog revision, plugin SHA, and version through
// the three --expected-current-* arguments. The target --commit/--version is the recorded prior
// pin. All three fences are required together so restore cannot overwrite an independently moved
// catalog.

import { execFileSync } from "node:child_process";
import { appendFileSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const MARKETPLACE = "TheGreenCedar/AgentPluginMarketplace";
const CATALOG = ".agents/plugins/marketplace.json";
const SOURCE_URL = "https://github.com/TheGreenCedar/CodeStory.git";

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
for (const key of ["source_repository", "commit", "version"]) {
  if (!args[key]) fail(`--${key.replaceAll("_", "-")} is required`);
}
const restoreCoordinates = [
  "expected_current_revision",
  "expected_current_plugin_sha",
  "expected_current_plugin_version",
];
const restoreCoordinateCount = restoreCoordinates.filter((key) => args[key]).length;
if (restoreCoordinateCount !== 0 && restoreCoordinateCount !== restoreCoordinates.length) {
  fail("automatic restore requires the expected current catalog revision, plugin SHA, and plugin version together");
}
const restoring = restoreCoordinateCount === restoreCoordinates.length;
const token = process.env.GH_TOKEN;
if (!token) fail("GH_TOKEN must carry a marketplace-scoped installation token");

const sourceRepository = path.resolve(args.source_repository);
const commit = execFileSync(
  "git",
  ["-C", sourceRepository, "rev-parse", `${args.commit}^{commit}`],
  { encoding: "utf8" },
).trim();
if (!/^[0-9a-f]{40}$/u.test(commit)) fail("commit must resolve to an immutable Git identity");

// Never pin a commit whose plugin sources cannot be resolved by an installing host.
const pluginManifest = JSON.parse(
  execFileSync(
    "git",
    ["-C", sourceRepository, "show", `${commit}:plugins/codestory/.codex-plugin/plugin.json`],
    { encoding: "utf8" },
  ),
);
if (pluginManifest.version !== args.version) {
  fail(
    `commit ${commit} carries plugin version ${pluginManifest.version}, not the published ${args.version}`,
  );
}

const checkout = mkdtempSync(path.join(os.tmpdir(), "codestory-marketplace-"));
const git = (...command) =>
  execFileSync("git", ["-C", checkout, ...command], { encoding: "utf8" }).trim();

execFileSync(
  "git",
  ["clone", "--depth", "1", `https://x-access-token:${token}@github.com/${MARKETPLACE}.git`, checkout],
  { stdio: ["ignore", "ignore", "inherit"] },
);

const catalogPath = path.join(checkout, CATALOG);
const catalog = JSON.parse(readFileSync(catalogPath, "utf8"));
const entry = (catalog.plugins ?? []).find(({ name }) => name === "codestory");
if (!entry) fail(`${CATALOG} has no codestory entry`);
if (entry.source?.url !== SOURCE_URL || entry.source?.path !== "plugins/codestory") {
  fail(`${CATALOG} codestory entry does not point at this repository's plugin directory`);
}

// What the catalog served before this run touched it. Recorded BEFORE the push, because a
// rollback needs a target and the live catalog stops being able to answer that question the
// moment this script succeeds. Refused rather than recorded empty: a catalog move whose previous
// pin cannot be named is a move nobody can undo, and this job is `continue-on-error`, so refusing
// leaves the catalog serving the release it already served -- the safe direction.
const previousSha = String(entry.source.sha ?? "");
const previousVersion = String(entry.version ?? "");
if (!/^[0-9a-f]{40}$/u.test(previousSha)) {
  fail(`${CATALOG} codestory entry pins ${JSON.stringify(entry.source.sha ?? null)}, which is not a rollback target`);
}
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/u.test(previousVersion)) {
  fail(`${CATALOG} codestory entry names version ${JSON.stringify(entry.version ?? null)}, which is not a rollback target`);
}
const previousRevision = git("rev-parse", "HEAD");

const output = (revision, changed) => {
  console.log(
    changed
      ? `Catalog ${restoring ? "restored" : "now pins"} codestory ${args.version} at ${commit} (${revision}).`
      : `Catalog already pins codestory ${args.version} at ${commit} (${revision}); nothing to push.`,
  );
  console.log(
    `Previous catalog pin: codestory ${previousVersion} at ${previousSha} (catalog ${previousRevision}).`,
  );
  if (args.github_output) {
    appendFileSync(
      args.github_output,
      `marketplace_revision=${revision}\n`
        + `previous_marketplace_revision=${previousRevision}\n`
        + `previous_plugin_sha=${previousSha}\n`
        + `previous_plugin_version=${previousVersion}\n`
        + `catalog_changed=${changed}\n`
        + `catalog_operation=${restoring ? "restore" : "publish"}\n`,
    );
  }
};

if (entry.source.sha === commit && entry.version === args.version) {
  output(previousRevision, false);
  process.exit(0);
}

if (restoring) {
  const expectedRevision = String(args.expected_current_revision);
  const expectedSha = String(args.expected_current_plugin_sha);
  const expectedVersion = String(args.expected_current_plugin_version);
  if (
    previousRevision !== expectedRevision
    || previousSha !== expectedSha
    || previousVersion !== expectedVersion
  ) {
    fail(
      `catalog no longer names the just-published rollback source ${expectedVersion} at ${expectedSha} (${expectedRevision}); refusing automatic restore`,
    );
  }
}

entry.version = args.version;
entry.source.sha = commit;
writeFileSync(catalogPath, `${JSON.stringify(catalog, null, 2)}\n`);

git("config", "user.email", "release@codestory.invalid");
git("config", "user.name", "CodeStory release");
git("add", CATALOG);
git("commit", "--message", `${restoring ? "restore" : "codestory"} ${args.version}`);
git("push", "origin", "HEAD:main");
output(git("rev-parse", "HEAD"), true);
