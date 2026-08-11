import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import {
  chmod,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  cursorLocalOverrideFileName,
  installCursorPlugin,
} from "../install-codestory-cursor-plugin.mjs";

const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const sourcePlugin = path.join(repoRoot, "plugins", "codestory");
const version = JSON.parse(
  await readFile(path.join(sourcePlugin, ".cursor-plugin", "plugin.json"), "utf8"),
).version;

function git(root, ...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

async function writeFakeCli(cliPath, cliVersion = version) {
  await mkdir(path.dirname(cliPath), { recursive: true });
  await writeFile(cliPath, `#!/bin/sh\necho "codestory-cli ${cliVersion}"\n`, "utf8");
  await chmod(cliPath, 0o755);
}

async function fixture() {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), "codestory-cursor-install-"));
  const checkout = path.join(root, "repo");
  const pluginSource = path.join(checkout, "plugins", "codestory");
  const home = path.join(root, "home");
  const installRoot = path.join(home, ".cursor", "plugins", "local", "codestory");
  const pluginData = path.join(home, ".cursor", "plugins", "data", "codestory");
  const cli = path.join(root, process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
  await mkdir(path.dirname(pluginSource), { recursive: true });
  await cp(sourcePlugin, pluginSource, { recursive: true });
  git(checkout, "init", "-q");
  git(checkout, "config", "user.email", "fixture@example.invalid");
  git(checkout, "config", "user.name", "Fixture");
  git(checkout, "add", ".");
  git(checkout, "commit", "-qm", "fixture");
  return {
    cli,
    checkout,
    home,
    installRoot,
    pluginData,
    pluginSource,
    root,
  };
}

function install(value, overrides = {}) {
  return installCursorPlugin({
    home: value.home,
    installRoot: value.installRoot,
    pluginData: value.pluginData,
    pluginSource: value.pluginSource,
    repoRoot: value.checkout,
    ...overrides,
  });
}

test("Cursor installer links the committed plugin and writes only the local CLI override", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  const value = await fixture();
  const secret = "cursor-secret-must-not-leak";
  process.env.CURSOR_INSTALL_TEST_SECRET = secret;
  try {
    await writeFakeCli(value.cli);
    const first = install(value, { cli: value.cli });
    assert.equal(first.plugin, "codestory");
    assert.equal(first.version, version);
    assert.equal(first.linked, true);
    assert.equal(first.cli_override, "configured");
    assert.equal(await realpath(value.installRoot), await realpath(value.pluginSource));

    const overridePath = path.join(value.pluginData, cursorLocalOverrideFileName);
    const overrideText = await readFile(overridePath, "utf8");
    assert.deepEqual(JSON.parse(overrideText), {
      schema_version: 1,
      CODESTORY_CLI: value.cli,
    });
    assert.doesNotMatch(JSON.stringify(first), new RegExp(secret, "u"));
    assert.doesNotMatch(overrideText, new RegExp(secret, "u"));

    const second = install(value, { cli: value.cli });
    assert.equal(second.linked, false);
    assert.equal(await realpath(value.installRoot), await realpath(value.pluginSource));
  } finally {
    delete process.env.CURSOR_INSTALL_TEST_SECRET;
    await rm(value.root, { recursive: true, force: true });
  }
});

test("Cursor installer defaults to the managed runtime and clears its owned local override", async () => {
  const value = await fixture();
  try {
    await mkdir(value.pluginData, { recursive: true });
    const overridePath = path.join(value.pluginData, cursorLocalOverrideFileName);
    await writeFile(overridePath, "stale", "utf8");
    const result = install(value);
    assert.equal(result.cli_override, "managed");
    assert.equal(fs.existsSync(overridePath), false);
    assert.equal(await realpath(value.installRoot), await realpath(value.pluginSource));
  } finally {
    await rm(value.root, { recursive: true, force: true });
  }
});

test("Cursor installer refuses dirty source and an existing link to another package", async () => {
  for (const variant of ["dirty", "wrong-link"]) {
    const value = await fixture();
    try {
      if (variant === "dirty") {
        await writeFile(path.join(value.pluginSource, "uncommitted.txt"), "dirty", "utf8");
        assert.throws(() => install(value), /plugin_source_not_committed/u);
      } else {
        const other = path.join(value.root, "other-plugin");
        await mkdir(other, { recursive: true });
        await mkdir(path.dirname(value.installRoot), { recursive: true });
        await symlink(other, value.installRoot, process.platform === "win32" ? "junction" : "dir");
        assert.throws(() => install(value), /cursor_plugin_install_wrong_target/u);
      }
    } finally {
      await rm(value.root, { recursive: true, force: true });
    }
  }
});
