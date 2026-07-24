import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptRoot = path.dirname(fileURLToPath(import.meta.url));
const repositoryRoot = path.resolve(scriptRoot, "..", "..");
const helper = path.join(scriptRoot, "install-codestory-marketplace-proof.mjs");
const codexVersion = "0.144.5";

function run(executable, args, options = {}) {
  const result = spawnSync(executable, args, {
    ...options,
    encoding: "utf8",
    shell: process.platform === "win32",
  });
  assert.equal(
    result.status,
    0,
    `${path.basename(executable)} ${args.join(" ")} failed:\n${result.stderr}`,
  );
  return result.stdout.trim();
}

function commitFixture(root, message = "fixture", { init = false, allowEmpty = false } = {}) {
  if (init) run("git", ["init", "-q"], { cwd: root });
  run("git", ["add", "."], { cwd: root });
  const commitArgs = [
    "-c",
    "user.name=fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "commit",
  ];
  if (allowEmpty) commitArgs.push("--allow-empty");
  commitArgs.push("-qm", message);
  run("git", commitArgs, { cwd: root });
  return run("git", ["rev-parse", "HEAD"], { cwd: root });
}

function proofArgs({
  packageRoot,
  proofRoot,
  marketplaceRoot,
  marketplaceRevision,
  expectedVersion,
  sourceRepository,
}) {
  const codexHome = path.join(proofRoot, "codex-home");
  return [
    helper,
    "--codex-package-root",
    packageRoot,
    "--codex-home",
    codexHome,
    "--plugin-data",
    path.join(codexHome, "plugin-data"),
    "--marketplace-source",
    marketplaceRoot,
    "--marketplace-name",
    "Fixture",
    "--marketplace-revision",
    marketplaceRevision,
    "--local-fixture",
    "true",
    "--expected-version",
    expectedVersion,
    "--source-repository",
    sourceRepository,
    "--attestation",
    path.join(proofRoot, "attestation.json"),
  ];
}

function assertFailedProof(args, message) {
  const result = spawnSync(process.execPath, args, {
    encoding: "utf8",
    shell: process.platform === "win32",
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, message);
}

test("pinned Codex installs a local marketplace fixture into the attested cache", () => {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-marketplace-proof-"));
  try {
    const packageRoot = path.join(root, "codex-package");
    const npm = process.platform === "win32" ? "npm.cmd" : "npm";
    run(npm, [
      "install",
      "--prefix",
      packageRoot,
      "--no-audit",
      "--no-fund",
      `@openai/codex@${codexVersion}`,
    ]);

    const marketplaceRoot = path.join(root, "marketplace");
    const pluginSourceRoot = path.join(root, "plugin-source");
    cpSync(
      path.join(repositoryRoot, "plugins", "codestory"),
      path.join(pluginSourceRoot, "plugins", "codestory"),
      { recursive: true },
    );
    const pluginSourceCommit = commitFixture(pluginSourceRoot, "fixture", { init: true });
    mkdirSync(path.join(marketplaceRoot, ".agents", "plugins"), { recursive: true });
    writeFileSync(
      path.join(marketplaceRoot, ".agents", "plugins", "marketplace.json"),
      `${JSON.stringify({
        name: "Fixture",
        interface: { displayName: "Fixture" },
        plugins: [{
          name: "codestory",
          source: {
            source: "git-subdir",
            url: pathToFileURL(pluginSourceRoot).href,
            path: "plugins/codestory",
            sha: pluginSourceCommit,
          },
          policy: {
            installation: "AVAILABLE",
            authentication: "ON_INSTALL",
          },
          category: "Developer Tools",
        }],
      }, null, 2)}\n`,
    );
    const marketplaceRevision = commitFixture(marketplaceRoot, "fixture", {
      init: true,
    });
    const releaseSourceCommit = commitFixture(
      pluginSourceRoot,
      "release merge",
      { allowEmpty: true },
    );
    assert.notEqual(releaseSourceCommit, pluginSourceCommit);
    assert.equal(
      run("git", ["rev-parse", `${pluginSourceCommit}^{tree}`], {
        cwd: pluginSourceRoot,
      }),
      run("git", ["rev-parse", "HEAD^{tree}"], { cwd: pluginSourceRoot }),
    );
    const pluginManifest = JSON.parse(
      readFileSync(
        path.join(repositoryRoot, "plugins", "codestory", ".codex-plugin", "plugin.json"),
      ),
    );
    const codexHome = path.join(root, "codex-home");
    const attestationPath = path.join(root, "attestation.json");
    run(process.execPath, proofArgs({
      packageRoot,
      proofRoot: root,
      marketplaceRoot,
      marketplaceRevision,
      expectedVersion: pluginManifest.version,
      sourceRepository: pluginSourceRoot,
    }));

    const attestation = JSON.parse(readFileSync(attestationPath));
    const expectedPluginRoot = path.join(
      realpathSync(codexHome),
      "plugins",
      "cache",
      "Fixture",
      "codestory",
      pluginManifest.version,
    );
    assert.equal(attestation.schema_version, 2);
    assert.equal(attestation.marketplace.codex_cli_version, `codex-cli ${codexVersion}`);
    assert.equal(attestation.marketplace.revision, marketplaceRevision);
    assert.equal(
      attestation.plugin.source_commit,
      pluginSourceCommit,
    );
    assert.equal(
      attestation.plugin.source_tree,
      run("git", ["rev-parse", "HEAD^{tree}"], { cwd: pluginSourceRoot }),
    );
    assert.equal(attestation.marketplace.provenance.add.revision, marketplaceRevision);
    assert.equal(attestation.marketplace.provenance.list.revision, marketplaceRevision);
    assert.equal(
      attestation.marketplace.provenance.add.root,
      attestation.marketplace.provenance.list.root,
    );
    assert.equal(attestation.installation.plugin_root, expectedPluginRoot);
    assert.equal(attestation.marketplace.plugin_add_result.installedPath, expectedPluginRoot);
    assert.deepEqual(
      attestation.marketplace.plugin_list_result.installed.map(({ pluginId }) => pluginId),
      ["codestory@Fixture"],
    );
    assert.equal(
      attestation.marketplace.plugin_list_result.installed[0].source.sha,
      pluginSourceCommit,
    );
    const config = readFileSync(path.join(codexHome, "config.toml"), "utf8");
    assert.match(config, /\[marketplaces\.Fixture\]/u);
    assert.match(config, /\[plugins\."codestory@Fixture"\]/u);
    assert.match(config, /enabled = true/u);

    writeFileSync(
      path.join(pluginSourceRoot, "plugins", "codestory", "mismatched.txt"),
      "different package bytes\n",
    );
    assertFailedProof(
      proofArgs({
        packageRoot,
        proofRoot: path.join(root, "mismatch"),
        marketplaceRoot,
        marketplaceRevision,
        expectedVersion: pluginManifest.version,
        sourceRepository: pluginSourceRoot,
      }),
      /installed plugin bytes do not match the checked-out CodeStory package/u,
    );
    commitFixture(pluginSourceRoot, "change release tree");
    assertFailedProof(
      proofArgs({
        packageRoot,
        proofRoot: path.join(root, "changed-tree"),
        marketplaceRoot,
        marketplaceRevision,
        expectedVersion: pluginManifest.version,
        sourceRepository: pluginSourceRoot,
      }),
      /pinned marketplace plugin source does not match the release source tree/u,
    );

    const catalogPath = path.join(
      marketplaceRoot,
      ".agents",
      "plugins",
      "marketplace.json",
    );
    const unpinnedCatalog = JSON.parse(readFileSync(catalogPath));
    delete unpinnedCatalog.plugins[0].source.sha;
    writeFileSync(catalogPath, `${JSON.stringify(unpinnedCatalog, null, 2)}\n`);
    const unpinnedMarketplaceRevision = commitFixture(
      marketplaceRoot,
      "remove source pin",
    );
    assertFailedProof(
      proofArgs({
        packageRoot,
        proofRoot: path.join(root, "unpinned"),
        marketplaceRoot,
        marketplaceRevision: unpinnedMarketplaceRevision,
        expectedVersion: pluginManifest.version,
        sourceRepository: pluginSourceRoot,
      }),
      /marketplace plugin source is not pinned to one immutable commit/u,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
