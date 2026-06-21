import assert from "node:assert/strict";
import test from "node:test";
import { access, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));

test("plugin metadata maps skill and direct stdio server", async () => {
  const manifest = JSON.parse(await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"));
  const mcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));

  assert.equal(manifest.name, "codestory");
  assert.equal(manifest.skills, "./skills/");
  assert.equal(manifest.mcpServers, "./.mcp.json");
  assert.equal(manifest.interface.capabilities.includes("Read"), true);
  assert.equal(mcp.mcpServers.codestory.command, "codestory-cli");
  assert.deepEqual(mcp.mcpServers.codestory.args, ["serve", "--stdio", "--refresh", "none"]);
  assert.equal(Object.hasOwn(mcp.mcpServers.codestory, "cwd"), false);
});

test("codestory repo ships plugin source, not marketplace catalog or adapter runtime", async () => {
  await assert.rejects(access(join(repoRoot, ".agents", "plugins", "marketplace.json")));
  await assert.rejects(access(join(repoRoot, ".agents", "skills", "codestory-grounding", "SKILL.md")));
  await assert.rejects(access(join(pluginRoot, "scripts", "codestory-mcp.mjs")));
});

test("plugin docs are agent-first, cross-platform, and latest-release aware", async () => {
  const readme = await readFile(join(pluginRoot, "README.md"), "utf8");
  const skill = await readFile(join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"), "utf8");
  const sharedRequired = [
    "latest GitHub release",
    "codestory-cli --version",
    "missing or outdated",
    "codestory-cli-vX.Y.Z-windows-x64.zip",
    "codestory-cli-vX.Y.Z-windows-arm64.zip",
    "codestory-cli-vX.Y.Z-macos-arm64.tar.gz",
    "macOS x64",
    "codestory-cli-vX.Y.Z-linux-x64.tar.gz",
    "codestory-cli-vX.Y.Z-linux-arm64.tar.gz",
    "Source fallback",
    "SHA256SUMS.txt",
    "retrieval_mode=full",
  ];
  const forbidden = [
    "release-bound to " + "CodeStory `v" + "0.11.1`",
    "codestory-cli-v" + "0.11.1",
    "use that version " + "unless",
  ];
  const readmeRequired = [
    "The human job is simple",
    "The CLI is still there, but it is the escape hatch and repair surface",
    "codestory://status",
    "codestory://grounding",
    "The plugin does not bundle the binary",
    "TheGreenCedar/AgentPluginMarketplace",
    "git-subdir",
    "https://github.com/TheGreenCedar/CodeStory.git",
    "plugins/codestory",
    "codestory-cli serve --stdio --refresh none",
  ];
  const skillRequired = [
    "download and unpack only",
    "plugin MCP process may need",
    "new agent thread",
    "Read `codestory://grounding`",
    "Always pass `--project <target-workspace>` explicitly",
  ];

  for (const text of [readme, skill]) {
    for (const phrase of sharedRequired) {
      assert.equal(text.includes(phrase), true, phrase);
    }
    for (const phrase of forbidden) {
      assert.equal(text.includes(phrase), false, phrase);
    }
  }
  for (const phrase of readmeRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
  }
  for (const phrase of skillRequired) {
    assert.equal(skill.includes(phrase), true, phrase);
  }
});

test("canonical grounding skill ships with plugin references", async () => {
  await access(join(pluginRoot, "skills", "codestory-grounding", "references", "ground.md"));
  await access(join(pluginRoot, "skills", "codestory-grounding", "references", "packet.md"));
  await access(join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.ps1"));
  await access(join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.sh"));
});
