// The fixture stands in for the live catalog during release preflight, so its
// shape has to be the live catalog's shape. When it was not, the resolver
// refused it with `missing field \`name\`` and the first release to exercise the
// fixture path failed at preflight.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

/// The keys the live catalog carries, recorded here so a divergence fails at PR
/// time instead of at the release that first depends on it.
const CATALOG_KEYS = ["interface", "name", "plugins"];
const PLUGIN_KEYS = ["category", "name", "policy", "source"];
const SOURCE_KEYS = ["path", "sha", "source", "url"];

function buildFixture() {
  const out = mkdtempSync(path.join(tmpdir(), "codestory-marketplace-fixture-"));
  const commit = execFileSync("git", ["-C", repositoryRoot, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
  execFileSync(
    process.execPath,
    [
      path.join(repositoryRoot, ".github/scripts/build-marketplace-fixture.mjs"),
      "--source-repository",
      repositoryRoot,
      "--out",
      out,
      "--commit",
      commit,
    ],
    { encoding: "utf8" },
  );
  const catalog = JSON.parse(
    readFileSync(path.join(out, ".agents/plugins/marketplace.json"), "utf8"),
  );
  return { out, catalog, commit };
}

test("the fixture catalog carries the fields the resolver requires", () => {
  const { out, catalog, commit } = buildFixture();
  try {
    assert.deepEqual(Object.keys(catalog).sort(), CATALOG_KEYS);
    assert.equal(typeof catalog.name, "string");
    assert.ok(catalog.name.length > 0, "the resolver rejects a nameless catalog");

    assert.equal(catalog.plugins.length, 1);
    const [plugin] = catalog.plugins;
    assert.deepEqual(Object.keys(plugin).sort(), PLUGIN_KEYS);
    assert.equal(plugin.name, "codestory");
    assert.deepEqual(Object.keys(plugin.source).sort(), SOURCE_KEYS);
    assert.equal(plugin.source.sha, commit);
    assert.equal(plugin.source.path, "plugins/codestory");
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test("the fixture states no version, because the live catalog states none", () => {
  // The pinned `sha` selects the plugin. A version field here would prove an
  // install path the live catalog cannot produce.
  const { out, catalog } = buildFixture();
  try {
    assert.equal(catalog.version, undefined);
    assert.equal(catalog.plugins[0].version, undefined);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

// A fixture catalog is a DISTINCT delivery state, not a stand-in that may pass for the live one.
// The installed-runtime predicate refuses a deferred install whose marketplace root does not carry
// this marker naming the exact commit the catalog pins, so an arbitrary local git directory -- or
// a clone of the live marketplace -- cannot satisfy the deferred shape by accident.
test("the fixture identifies itself and the commit it pins", () => {
  const { out, commit } = buildFixture();
  try {
    const marker = JSON.parse(
      readFileSync(path.join(out, ".codestory-marketplace-fixture.json"), "utf8"),
    );
    assert.deepEqual(Object.keys(marker).sort(), [
      "pinned_commit",
      "plugin_version",
      "purpose",
      "schema_version",
    ]);
    assert.equal(marker.schema_version, 1);
    assert.equal(marker.purpose, "codestory-candidate-pinned-marketplace-fixture");
    assert.equal(marker.pinned_commit, commit);
    // The marker must be committed, or a clean-tree check would pass over a fixture that had
    // been re-marked after the fact.
    assert.equal(
      execFileSync("git", ["-C", out, "status", "--porcelain"], { encoding: "utf8" }).trim(),
      "",
    );
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

// The fixture deliberately has no `origin`. Pointing one at the live marketplace URL would make it
// claim an identity it does not have, and the predicate asserts the absence positively rather than
// treating a failed probe as proof of anything. The predicate's own probe used to hard-fail here,
// which is what made the deferred path unprovable in the first place.
test("the fixture is local-only and never claims the live marketplace as its origin", () => {
  const { out } = buildFixture();
  try {
    const probe = spawnSync("git", ["-C", out, "remote", "get-url", "origin"], {
      encoding: "utf8",
    });
    assert.notEqual(probe.status, 0, "a candidate-pinned fixture must have no origin remote");
    assert.match(probe.stderr, /No such remote/u);
    assert.equal(
      execFileSync("git", ["-C", out, "remote"], { encoding: "utf8" }).trim(),
      "",
    );
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

// The resolver reads .agents/plugins/marketplace.json and nothing else, so the marker must not
// change the catalog the resolver sees.
test("the marker sits outside the catalog the resolver reads", () => {
  const { out, catalog } = buildFixture();
  try {
    assert.equal(catalog.fixture, undefined);
    assert.equal(
      existsSync(path.join(out, ".agents", "plugins", ".codestory-marketplace-fixture.json")),
      false,
    );
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
