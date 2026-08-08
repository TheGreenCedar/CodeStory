// Drive the real catalog publisher against real Git repositories.
//
// The marketplace is a second repository reached over https, so the script's clone URL is fixed.
// `url.<base>.insteadOf` in a scratch global config redirects that exact URL to a local bare repo,
// which lets the whole script run -- clone, edit, commit, push -- with nothing stubbed out. The
// assertions are made against the bare repo's own contents, not against the script's stdout.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsDir = path.dirname(fileURLToPath(import.meta.url));
const publisher = path.join(scriptsDir, "publish-marketplace-catalog.mjs");
const CATALOG = ".agents/plugins/marketplace.json";
const SOURCE_URL = "https://github.com/TheGreenCedar/CodeStory.git";
const TOKEN = "test-token";
const CLONE_URL = `https://x-access-token:${TOKEN}@github.com/TheGreenCedar/AgentPluginMarketplace.git`;

function git(cwd, ...args) {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" }).trim();
}

function commitAll(repo, message) {
  git(repo, "add", "-A");
  git(repo, "-c", "user.email=t@t.invalid", "-c", "user.name=t", "commit", "--message", message);
  return git(repo, "rev-parse", "HEAD");
}

/// A source repository whose plugin manifest carries each version at its own commit, which is what
/// the publisher checks before it is willing to pin a commit.
function sourceRepository(root, versions) {
  const repo = path.join(root, "source");
  fs.mkdirSync(path.join(repo, "plugins/codestory/.codex-plugin"), { recursive: true });
  git(root, "init", "--quiet", repo);
  const commits = {};
  for (const version of versions) {
    fs.writeFileSync(
      path.join(repo, "plugins/codestory/.codex-plugin/plugin.json"),
      `${JSON.stringify({ name: "codestory", version }, null, 2)}\n`,
    );
    commits[version] = commitAll(repo, `codestory ${version}`);
  }
  return { repo, commits };
}

function marketplaceRepository(root, entry) {
  const work = path.join(root, "marketplace-work");
  const bare = path.join(root, "marketplace.git");
  fs.mkdirSync(path.join(work, path.dirname(CATALOG)), { recursive: true });
  git(root, "init", "--quiet", work);
  fs.writeFileSync(
    path.join(work, CATALOG),
    `${JSON.stringify({ plugins: [entry] }, null, 2)}\n`,
  );
  commitAll(work, "seed catalog");
  git(root, "init", "--bare", "--quiet", bare);
  git(work, "remote", "add", "origin", bare);
  git(work, "push", "--quiet", "origin", "HEAD:main");
  git(bare, "symbolic-ref", "HEAD", "refs/heads/main");
  return bare;
}

function catalogEntry(version, sha) {
  return {
    name: "codestory",
    version,
    source: { url: SOURCE_URL, path: "plugins/codestory", sha },
  };
}

function readCatalog(bare) {
  return JSON.parse(execFileSync("git", ["-C", bare, "show", "main:" + CATALOG], { encoding: "utf8" }));
}

function scratch() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-catalog-"));
  const configPath = path.join(root, "gitconfig");
  fs.writeFileSync(configPath, `[url "file://${path.join(root, "marketplace.git")}"]\n\tinsteadOf = ${CLONE_URL}\n`);
  return { root, configPath };
}

function runPublisher({ root, configPath }, repo, { commit, version, ...extra }) {
  const outputPath = path.join(root, `output-${Math.random().toString(36).slice(2)}.txt`);
  fs.writeFileSync(outputPath, "");
  const result = spawnSync(
    process.execPath,
    [
      publisher,
      "--source-repository",
      repo,
      "--commit",
      commit,
      "--version",
      version,
      ...Object.entries(extra).flatMap(([key, value]) => [
        `--${key.replaceAll("_", "-")}`,
        value,
      ]),
      "--github-output",
      outputPath,
    ],
    {
      encoding: "utf8",
      env: { ...process.env, GH_TOKEN: TOKEN, GIT_CONFIG_GLOBAL: configPath, GIT_CONFIG_NOSYSTEM: "1" },
    },
  );
  const outputs = Object.fromEntries(
    fs
      .readFileSync(outputPath, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const index = line.indexOf("=");
        return [line.slice(0, index), line.slice(index + 1)];
      }),
  );
  return { ...result, outputs };
}

test("the catalog move records the pin it replaced, and that pin is a working rollback target", () => {
  const env = scratch();
  try {
    const { repo, commits } = sourceRepository(env.root, ["0.1.0", "0.2.0"]);
    const bare = marketplaceRepository(env.root, catalogEntry("0.1.0", commits["0.1.0"]));
    const seeded = git(bare, "rev-parse", "main");

    const forward = runPublisher(env, repo, { commit: commits["0.2.0"], version: "0.2.0" });
    assert.equal(forward.status, 0, `${forward.stdout}${forward.stderr}`);
    assert.equal(readCatalog(bare).plugins[0].source.sha, commits["0.2.0"]);
    assert.equal(readCatalog(bare).plugins[0].version, "0.2.0");
    // The previous pin is the release the catalog was serving, not the one just published.
    assert.deepEqual(
      {
        previous_marketplace_revision: forward.outputs.previous_marketplace_revision,
        previous_plugin_sha: forward.outputs.previous_plugin_sha,
        previous_plugin_version: forward.outputs.previous_plugin_version,
      },
      {
        previous_marketplace_revision: seeded,
        previous_plugin_sha: commits["0.1.0"],
        previous_plugin_version: "0.1.0",
      },
    );
    assert.equal(forward.outputs.marketplace_revision, git(bare, "rev-parse", "main"));
    assert.notEqual(forward.outputs.marketplace_revision, seeded);

    // Executable rollback: the recorded pin is enough to put the catalog back, with no new
    // version and no hand-editing of the other repository.
    const back = runPublisher(env, repo, {
      commit: forward.outputs.previous_plugin_sha,
      version: forward.outputs.previous_plugin_version,
      expected_current_revision: forward.outputs.marketplace_revision,
      expected_current_plugin_sha: commits["0.2.0"],
      expected_current_plugin_version: "0.2.0",
    });
    assert.equal(back.status, 0, `${back.stdout}${back.stderr}`);
    assert.equal(readCatalog(bare).plugins[0].source.sha, commits["0.1.0"]);
    assert.equal(readCatalog(bare).plugins[0].version, "0.1.0");
    assert.equal(back.outputs.previous_plugin_version, "0.2.0");
    assert.equal(back.outputs.catalog_operation, "restore");
    assert.equal(back.outputs.catalog_changed, "true");

    const idempotent = runPublisher(env, repo, {
      commit: forward.outputs.previous_plugin_sha,
      version: forward.outputs.previous_plugin_version,
      expected_current_revision: forward.outputs.marketplace_revision,
      expected_current_plugin_sha: commits["0.2.0"],
      expected_current_plugin_version: "0.2.0",
    });
    assert.equal(idempotent.status, 0, `${idempotent.stdout}${idempotent.stderr}`);
    assert.equal(idempotent.outputs.catalog_operation, "restore");
    assert.equal(idempotent.outputs.catalog_changed, "false");
  } finally {
    fs.rmSync(env.root, { recursive: true, force: true });
  }
});

test("an already-synced catalog reports the pin it is serving without pushing", () => {
  const env = scratch();
  try {
    const { repo, commits } = sourceRepository(env.root, ["0.1.0"]);
    const bare = marketplaceRepository(env.root, catalogEntry("0.1.0", commits["0.1.0"]));
    const seeded = git(bare, "rev-parse", "main");
    const result = runPublisher(env, repo, { commit: commits["0.1.0"], version: "0.1.0" });
    assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
    assert.match(result.stdout, /nothing to push/u);
    assert.equal(git(bare, "rev-parse", "main"), seeded);
    assert.equal(result.outputs.marketplace_revision, seeded);
    assert.equal(result.outputs.previous_marketplace_revision, seeded);
    assert.equal(result.outputs.previous_plugin_sha, commits["0.1.0"]);
  } finally {
    fs.rmSync(env.root, { recursive: true, force: true });
  }
});

// A move whose previous pin cannot be named is a move nobody can undo. Refusing leaves the catalog
// serving the release it already served, which is the direction that never offers a plugin that
// does not exist.
test("a catalog whose current pin is not a rollback target is never moved", () => {
  for (const [label, entry] of [
    ["an abbreviated sha", catalogEntry("0.1.0", "abc1234")],
    ["a missing sha", { name: "codestory", version: "0.1.0", source: { url: SOURCE_URL, path: "plugins/codestory" } }],
    ["an unparsable version", catalogEntry("latest", "b".repeat(40))],
  ]) {
    const env = scratch();
    try {
      const { repo, commits } = sourceRepository(env.root, ["0.2.0"]);
      const bare = marketplaceRepository(env.root, entry);
      const seeded = git(bare, "rev-parse", "main");
      const result = runPublisher(env, repo, {
        commit: commits["0.2.0"],
        version: "0.2.0",
        expected_current_revision: seeded,
        expected_current_plugin_sha: String(entry.source?.sha ?? "0".repeat(40)),
        expected_current_plugin_version: String(entry.version ?? "0.1.0"),
      });
      assert.equal(result.status, 1, `${label}: ${result.stdout}${result.stderr}`);
      assert.match(result.stderr, /::error::.*is not a rollback target/u, label);
      assert.equal(git(bare, "rev-parse", "main"), seeded, `${label} moved the catalog`);
      assert.equal(result.outputs.marketplace_revision, undefined, label);
    } finally {
      fs.rmSync(env.root, { recursive: true, force: true });
    }
  }
});
