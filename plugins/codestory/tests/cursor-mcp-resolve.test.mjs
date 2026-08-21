import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const require = createRequire(import.meta.url);
const resolverPath = join(pluginRoot, "scripts", "cursor-mcp-resolve.cjs");

async function writePluginPackage(root, marker) {
  await mkdir(join(root, "scripts"), { recursive: true });
  await writeFile(
    join(root, "plugin.json"),
    `${JSON.stringify({ name: "codestory", version: "0.0.0" })}\n`,
    "utf8",
  );
  await writeFile(
    join(root, "scripts", "codestory-mcp.cjs"),
    `"use strict";
const fs = require("fs");
if (require.main === module) {
  fs.writeFileSync(process.env.CODESTORY_CURSOR_MCP_SENTINEL, ${JSON.stringify(marker)});
} else {
  module.exports = { _test: { marker: ${JSON.stringify(marker)} } };
}
`,
    "utf8",
  );
  return join(root, "scripts", "codestory-mcp.cjs");
}

test("Cursor MCP resolve prefers CURSOR_PLUGIN_ROOT over a lying PLUGIN_ROOT", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-home-"));
  const project = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-project-"));
  const cachePlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "thegreencedar-codestory",
    "codestory",
    "deadbeef",
  );
  try {
    const cachedLauncher = await writePluginPackage(cachePlugin, "cache");
    await writeFile(join(project, "README.md"), "not a plugin\n", "utf8");
    const { resolveCodestoryCursorLauncher } = require(resolverPath);
    const fs = require("fs");
    const path = require("path");
    assert.equal(
      resolveCodestoryCursorLauncher(
        {
          CURSOR_PLUGIN_ROOT: cachePlugin,
          PLUGIN_ROOT: project,
        },
        home,
        fs,
        path,
      ),
      require("fs").realpathSync(cachedLauncher),
    );
  } finally {
    await rm(home, { recursive: true, force: true });
    await rm(project, { recursive: true, force: true });
  }
});

test("Cursor MCP resolve finds the cached plugin when PLUGIN_ROOT is an unrelated project", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-home-"));
  const project = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-project-"));
  const cachePlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "thegreencedar-codestory",
    "codestory",
    "cafef00d",
  );
  try {
    const cachedLauncher = await writePluginPackage(cachePlugin, "cache");
    const { resolveCodestoryCursorLauncher } = require(resolverPath);
    const fs = require("fs");
    const path = require("path");
    assert.equal(
      resolveCodestoryCursorLauncher({ PLUGIN_ROOT: project }, home, fs, path),
      require("fs").realpathSync(cachedLauncher),
    );
  } finally {
    await rm(home, { recursive: true, force: true });
    await rm(project, { recursive: true, force: true });
  }
});

test("Cursor MCP resolve prefers the local plugin package over cache", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-home-"));
  const cachePlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "thegreencedar-codestory",
    "codestory",
    "oldrev",
  );
  const localPlugin = join(home, ".cursor", "plugins", "local", "codestory");
  try {
    await writePluginPackage(cachePlugin, "cache");
    const localLauncher = await writePluginPackage(localPlugin, "local");
    const { resolveCodestoryCursorLauncher } = require(resolverPath);
    const fs = require("fs");
    const path = require("path");
    assert.equal(
      resolveCodestoryCursorLauncher({}, home, fs, path),
      require("fs").realpathSync(localLauncher),
    );
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("requiring a main-guarded launcher does not start it", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-home-"));
  const cachePlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "thegreencedar-codestory",
    "codestory",
    "deadbeef",
  );
  const sentinel = join(home, "sentinel.txt");
  try {
    const launcher = await writePluginPackage(cachePlugin, "must-not-start");
    const result = spawnSync(
      process.execPath,
      ["-e", `require(${JSON.stringify(launcher)})`],
      {
        encoding: "utf8",
        env: {
          ...process.env,
          CODESTORY_CURSOR_MCP_SENTINEL: sentinel,
        },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    await assert.rejects(readFile(sentinel, "utf8"), { code: "ENOENT" });
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("Cursor mcp.cursor.json inline entry starts the cached launcher from a foreign cwd", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-home-"));
  const project = await mkdtemp(join(tmpdir(), "codestory-cursor-mcp-project-"));
  const sentinel = join(home, "sentinel.txt");
  const cachePlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "thegreencedar-codestory",
    "codestory",
    "abc123",
  );
  try {
    await writePluginPackage(cachePlugin, "started-from-cache");
    const cursorMcp = JSON.parse(
      await readFile(join(pluginRoot, "mcp.cursor.json"), "utf8"),
    );
    const { INLINE_ENTRY } = require(resolverPath);
    assert.equal(cursorMcp.mcpServers.codestory.command, "node");
    assert.deepEqual(cursorMcp.mcpServers.codestory.args, ["-e", INLINE_ENTRY]);
    assert.equal(Object.hasOwn(cursorMcp.mcpServers.codestory, "cwd"), false);
    assert.doesNotMatch(
      JSON.stringify(cursorMcp),
      /\$\{PLUGIN_ROOT\}|\$\{CURSOR_PLUGIN_ROOT\}|\$\{workspaceFolder\}/u,
    );
    assert.match(INLINE_ENTRY, /Module\.runMain\s*\(/u);
    assert.doesNotMatch(INLINE_ENTRY, /require\(resolveCodestoryCursorLauncher/u);

    const result = spawnSync(process.execPath, cursorMcp.mcpServers.codestory.args, {
      cwd: project,
      encoding: "utf8",
      env: {
        HOME: home,
        USERPROFILE: home,
        PLUGIN_ROOT: project,
        CODESTORY_CURSOR_MCP_SENTINEL: sentinel,
      },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(sentinel, "utf8"), "started-from-cache");
  } finally {
    await rm(home, { recursive: true, force: true });
    await rm(project, { recursive: true, force: true });
  }
});
