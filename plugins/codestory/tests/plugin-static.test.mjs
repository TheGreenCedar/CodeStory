import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));

test("plugin metadata maps skill and stdio server", async () => {
  const manifest = JSON.parse(await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"));
  const mcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));

  assert.equal(manifest.name, "codestory");
  assert.equal(manifest.skills, "./skills/");
  assert.equal(manifest.mcpServers, "./.mcp.json");
  assert.equal(manifest.interface.capabilities.includes("Read"), true);
  assert.equal(mcp.mcpServers.codestory.command, "node");
  assert.deepEqual(mcp.mcpServers.codestory.args, ["./scripts/codestory-mcp.mjs"]);
});

test("missing binary prints setup action", () => {
  const result = spawnSync(process.execPath, [join(pluginRoot, "scripts", "codestory-mcp.mjs")], {
    encoding: "utf8",
    env: { CODESTORY_PROJECT: "C:/repo", PATH: "" },
    windowsHide: true,
  });

  assert.equal(result.status, 127);
  assert.match(result.stderr, /codestory-cli was not found/);
  assert.match(result.stderr, /scripts[\\/]install-codestory\.ps1/);
});
