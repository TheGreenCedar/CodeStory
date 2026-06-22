import assert from "node:assert/strict";
import test from "node:test";
import { access, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));

function readCargoVersion(manifestText) {
  let inPackage = false;
  for (const line of manifestText.split(/\r?\n/u)) {
    if (/^\[[^\]]+\]/u.test(line)) {
      inPackage = line.trim() === "[package]";
      continue;
    }
    if (!inPackage) {
      continue;
    }
    const versionMatch = line.match(/^version\s*=\s*"([^"]+)"/u);
    if (versionMatch) {
      return versionMatch[1];
    }
  }
  assert.fail("Cargo package must declare version");
}

test("plugin metadata maps skill and direct stdio server", async () => {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const mcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));

  assert.equal(manifest.name, "codestory");
  assert.equal(manifest.skills, "./skills/");
  assert.equal(manifest.mcpServers, "./.mcp.json");
  assert.equal(manifest.interface.capabilities.includes("Read"), true);
  assert.equal(mcp.mcpServers.codestory.command, "codestory-cli");
  assert.deepEqual(mcp.mcpServers.codestory.args, [
    "serve",
    "--stdio",
    "--refresh",
    "none",
  ]);
  assert.equal(Object.hasOwn(mcp.mcpServers.codestory, "cwd"), false);
});

test("plugin package version tracks the codestory-cli release version", async () => {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const cliManifest = await readFile(
    join(repoRoot, "crates", "codestory-cli", "Cargo.toml"),
    "utf8",
  );

  assert.equal(manifest.version, readCargoVersion(cliManifest));
});

test("codestory repo ships plugin source, not marketplace catalog or adapter runtime", async () => {
  await assert.rejects(
    access(join(repoRoot, ".agents", "plugins", "marketplace.json")),
  );
  await assert.rejects(
    access(
      join(repoRoot, ".agents", "skills", "codestory-grounding", "SKILL.md"),
    ),
  );
  await assert.rejects(
    access(join(pluginRoot, "scripts", "codestory-mcp.mjs")),
  );
});

test("plugin docs are agent-first, marketplace-aware, and latest-release aware", async () => {
  const rootReadme = await readFile(join(repoRoot, "README.md"), "utf8");
  const readme = await readFile(join(pluginRoot, "README.md"), "utf8");
  const skill = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
    "utf8",
  );
  const sharedRequired = [
    "latest GitHub release",
    "codestory-cli --version",
    "SHA256SUMS.txt",
    "retrieval_mode=full",
  ];
  const skillReleaseRequired = [
    "missing or outdated",
    "codestory-cli-vX.Y.Z-windows-x64.zip",
    "codestory-cli-vX.Y.Z-windows-arm64.zip",
    "codestory-cli-vX.Y.Z-macos-arm64.tar.gz",
    "codestory-cli-vX.Y.Z-linux-x64.tar.gz",
    "codestory-cli-vX.Y.Z-linux-arm64.tar.gz",
    "Source fallback",
  ];
  const forbidden = [
    "release-bound to " + "CodeStory `v" + "0.11.1`",
    "use that version " + "unless",
    "Install plugin entry `codestory` from the external marketplace catalog:",
  ];
  const forbiddenPatterns = [
    /\bcodestory-cli-v\d+\.\d+\.\d+/,
    /release-bound to CodeStory `v\d+\.\d+\.\d+`/,
  ];
  const readmeRequired = [
    "The human job is simple",
    "The CLI is still there, but it is the escape hatch and repair surface",
    "codestory://status",
    "codestory://grounding",
    "Inspect indexed file inventory and coverage.",
    "Map changed files to likely impact.",
    "For normal Codex use, install the plugin through the Codex plugin flow for your",
    "/plugins",
    "TheGreenCedar -> codestory -> Install plugin",
    "add or refresh this marketplace first",
    "codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace",
    "The marketplace catalog repo is `TheGreenCedar/AgentPluginMarketplace`",
    "marketplace display/name concept is `TheGreenCedar`",
    "plugin source at `https://github.com/TheGreenCedar/CodeStory.git`",
    "source path `plugins/codestory`",
    "The CodeStory repo does not contain the marketplace catalog",
    "workspace plugin settings are managed from the Codex Apps/Plugins UI",
    "UI path when the CLI marketplace command is",
    "Start a new Codex thread after installation or refresh",
    "The first run should be agent-owned",
    "installs the latest matching release asset",
    "uses source fallback only when no release asset fits the host",
    "Agent runtime bootstrap",
    "the skill tells the human that a Codex host/app restart may be needed",
    "The plugin does not bundle the binary",
    "Use source fallback only when no release asset fits the host",
    "Source docs, marketplace source checkout/cache, and the active installed MCP",
    "active runtime surface",
    "agent host `PATH`",
    "Set `CODESTORY_CLI` only for manual CLI fallback commands",
    ".mcp.json` does not launch through that variable",
    "python <path-to-plugin-creator>\\scripts\\validate_plugin.py plugins\\codestory",
    "The plugin validator path is maintainer-local",
    "For normal Codex use, refresh or uninstall the plugin from the Codex plugin",
    "codex plugin marketplace upgrade TheGreenCedar",
    "codex plugin marketplace remove TheGreenCedar",
    "commands only for source registration",
    "The plugin does not bundle the binary",
    "Marketplace catalog repo",
    "Marketplace display/name",
    "Plugin entry",
    "git-subdir",
    "https://github.com/TheGreenCedar/CodeStory.git",
    "plugins/codestory",
    "codestory-cli serve --stdio --refresh none",
  ];
  const skillRequired = [
    "download and unpack only",
    "Use `CODESTORY_CLI` only for manual CLI/source",
    "not as the installed MCP launch path",
    "plugin MCP process may need",
    "Codex host/app restart before a new agent thread",
    "new agent thread",
    "Read `codestory://grounding`",
    "Always pass `--project <target-workspace>` explicitly",
  ];
  const rootReadmeRequired = [
    "Install details, binary bootstrap",
    "[plugin README](plugins/codestory/README.md)",
    "`codestory-cli serve --stdio --refresh none`",
  ];

  for (const text of [readme, skill]) {
    for (const phrase of sharedRequired) {
      assert.equal(text.includes(phrase), true, phrase);
    }
    for (const phrase of forbidden) {
      assert.equal(text.includes(phrase), false, phrase);
    }
    for (const pattern of forbiddenPatterns) {
      assert.equal(pattern.test(text), false, String(pattern));
    }
  }
  for (const phrase of forbidden) {
    assert.equal(rootReadme.includes(phrase), false, phrase);
  }
  assert.equal(
    readme.includes("C:\\Users\\alber"),
    false,
    "public plugin README must not contain maintainer workstation paths",
  );
  for (const pattern of forbiddenPatterns) {
    assert.equal(pattern.test(rootReadme), false, String(pattern));
  }
  for (const phrase of readmeRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
  }
  for (const phrase of skillReleaseRequired) {
    assert.equal(skill.includes(phrase), true, phrase);
    assert.equal(readme.includes(phrase), false, phrase);
  }
  for (const phrase of rootReadmeRequired) {
    assert.equal(rootReadme.includes(phrase), true, phrase);
  }
  for (const phrase of skillRequired) {
    assert.equal(skill.includes(phrase), true, phrase);
  }
});

test("canonical grounding skill ships with plugin references", async () => {
  await access(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "ground.md",
    ),
  );
  await access(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "packet.md",
    ),
  );
  await access(
    join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.ps1"),
  );
  await access(
    join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.sh"),
  );
});
