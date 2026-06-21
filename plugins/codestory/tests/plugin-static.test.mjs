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
  await assert.rejects(access(join(pluginRoot, "scripts", "codestory-mcp.mjs")));
});

test("install guidance is cross-platform and release-bound", async () => {
  const readme = await readFile(join(pluginRoot, "README.md"), "utf8");
  const skill = await readFile(join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"), "utf8");
  const required = [
    "release-bound to CodeStory `v0.11.0`",
    "codestory-cli-v0.11.0-windows-x64.zip",
    "codestory-cli-v0.11.0-windows-arm64.zip",
    "codestory-cli-v0.11.0-macos-arm64.tar.gz",
    "macOS x64",
    "codestory-cli-v0.11.0-linux-x64.tar.gz",
    "codestory-cli-v0.11.0-linux-arm64.tar.gz",
    "Source fallback",
    "Codex host `PATH`",
    "codestory://status",
    "retrieval_mode=full",
  ];

  for (const text of [readme, skill]) {
    for (const phrase of required) {
      assert.equal(text.includes(phrase), true, phrase);
    }
  }
});
