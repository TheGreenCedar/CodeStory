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

function commitFixture(root) {
  run("git", ["init", "-q"], { cwd: root });
  run("git", ["add", "."], { cwd: root });
  run(
    "git",
    [
      "-c",
      "user.name=fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "commit",
      "-qm",
      "fixture",
    ],
    { cwd: root },
  );
  return run("git", ["rev-parse", "HEAD"], { cwd: root });
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
    commitFixture(pluginSourceRoot);
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
          },
          policy: {
            installation: "AVAILABLE",
            authentication: "ON_INSTALL",
          },
          category: "Developer Tools",
        }],
      }, null, 2)}\n`,
    );
    const marketplaceRevision = commitFixture(marketplaceRoot);
    const sourceCommit = run("git", ["rev-parse", "HEAD"], {
      cwd: repositoryRoot,
    });
    const sourceTree = run("git", ["rev-parse", "HEAD^{tree}"], {
      cwd: repositoryRoot,
    });
    const pluginManifest = JSON.parse(
      readFileSync(
        path.join(repositoryRoot, "plugins", "codestory", ".codex-plugin", "plugin.json"),
      ),
    );
    const codexHome = path.join(root, "codex-home");
    const pluginData = path.join(codexHome, "plugin-data");
    const attestationPath = path.join(root, "attestation.json");
    run(process.execPath, [
      helper,
      "--codex-package-root",
      packageRoot,
      "--codex-home",
      codexHome,
      "--plugin-data",
      pluginData,
      "--marketplace-source",
      marketplaceRoot,
      "--marketplace-name",
      "Fixture",
      "--marketplace-revision",
      marketplaceRevision,
      "--local-fixture",
      "true",
      "--expected-version",
      pluginManifest.version,
      "--source-commit",
      sourceCommit,
      "--source-tree",
      sourceTree,
      "--attestation",
      attestationPath,
    ]);

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
    const config = readFileSync(path.join(codexHome, "config.toml"), "utf8");
    assert.match(config, /\[marketplaces\.Fixture\]/u);
    assert.match(config, /\[plugins\."codestory@Fixture"\]/u);
    assert.match(config, /enabled = true/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
