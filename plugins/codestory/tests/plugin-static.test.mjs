import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { pathCandidates, resolveProject } from "../scripts/codestory-mcp.mjs";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));

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
  assert.match(result.stderr, /powershell -ExecutionPolicy Bypass -File/);
});

test("windows path lookup does not select cmd or bat shims", async () => {
  const temp = await mkdtemp(join(tmpdir(), "codestory-path-"));
  const originalPath = process.env.PATH;
  try {
    await writeFile(join(temp, "codestory-cli.cmd"), "@echo off\r\n");
    await writeFile(join(temp, "codestory-cli.bat"), "@echo off\r\n");
    process.env.PATH = temp;

    const candidates = pathCandidates("codestory-cli");
    assert.equal(candidates.some((candidate) => candidate.endsWith(".cmd")), false);
    assert.equal(candidates.some((candidate) => candidate.endsWith(".bat")), false);
  } finally {
    process.env.PATH = originalPath;
    await rm(temp, { recursive: true, force: true });
  }
});

test("plugin cwd falls back to repository root project", () => {
  const originalCwd = process.cwd();
  const originalProject = process.env.CODESTORY_PROJECT;
  const originalWorkspace = process.env.CODESTORY_WORKSPACE;
  const originalCodexWorkspace = process.env.CODEX_WORKSPACE;
  try {
    delete process.env.CODESTORY_PROJECT;
    delete process.env.CODESTORY_WORKSPACE;
    delete process.env.CODEX_WORKSPACE;
    process.chdir(pluginRoot);

    assert.equal(resolveProject(), repoRoot);
  } finally {
    process.chdir(originalCwd);
    restoreEnv("CODESTORY_PROJECT", originalProject);
    restoreEnv("CODESTORY_WORKSPACE", originalWorkspace);
    restoreEnv("CODEX_WORKSPACE", originalCodexWorkspace);
  }
});

function restoreEnv(name, value) {
  if (value === undefined) {
    delete process.env[name];
  } else {
    process.env[name] = value;
  }
}
