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

const catalogDirectory = path.join(path.resolve(args.out), ".agents", "plugins");
mkdirSync(catalogDirectory, { recursive: true });
const catalog = {
  version: 1,
  plugins: [
    {
      name: "codestory",
      version: manifest.version,
      source: {
        source: "git-subdir",
        path: "plugins/codestory",
        url: "https://github.com/TheGreenCedar/CodeStory.git",
        sha: commit,
      },
    },
  ],
};
writeFileSync(
  path.join(catalogDirectory, "marketplace.json"),
  `${JSON.stringify(catalog, null, 2)}\n`,
);

// The fixture must be a git repository: the Codex resolver clones it like the live catalog.
const git = (...command) =>
  execFileSync("git", ["-C", path.resolve(args.out), ...command], { encoding: "utf8" });
git("init", "--quiet", "--initial-branch", "main");
git("config", "user.email", "release@codestory.invalid");
git("config", "user.name", "CodeStory release");
git("add", "--all");
git("commit", "--quiet", "--message", `pin codestory ${manifest.version} at ${commit}`);

console.log(`Marketplace fixture pins codestory ${manifest.version} at ${commit}.`);
