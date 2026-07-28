// The fixture stands in for the live catalog during release preflight, so its
// shape has to be the live catalog's shape. When it was not, the resolver
// refused it with `missing field \`name\`` and the first release to exercise the
// fixture path failed at preflight.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
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
