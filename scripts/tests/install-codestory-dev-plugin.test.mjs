import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
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
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { installDevPlugin } from "../install-codestory-dev-plugin.mjs";

const require = createRequire(import.meta.url);
const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const sourcePlugin = path.join(repoRoot, "plugins", "codestory");
const contract = require(path.join(sourcePlugin, "scripts", "codestory-dev-cli-contract.cjs"));
const version = JSON.parse(
  await readFile(path.join(sourcePlugin, ".codex-plugin", "plugin.json"), "utf8"),
).version;

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function writeFakeCli(cliPath, cliVersion = version, suffix = "") {
  await mkdir(path.dirname(cliPath), { recursive: true });
  const body = `#!/bin/sh\necho "codestory-cli ${cliVersion}"\n${suffix}\n`;
  await writeFile(cliPath, body, "utf8");
  await chmod(cliPath, 0o755);
}

function git(root, ...args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}

async function writeFakeCodex(script) {
  const body = `#!${process.execPath}
const fs=require("fs");
const path=require("path");
const args=process.argv.slice(2);
const stage=process.env.FAKE_STAGE_ROOT;
const cache=process.env.FAKE_CACHE_ROOT;
const version=process.env.FAKE_PLUGIN_VERSION;
const installed=path.join(cache,"CodeStoryDev","codestory",version);
const command=args[1];
if(command==="list"){
  process.stdout.write(JSON.stringify({installed:fs.existsSync(installed)?[{pluginId:"codestory@CodeStoryDev"}]:[]}));
  process.exit(0);
}
if(command==="remove"){
  fs.rmSync(installed,{recursive:true,force:true});
  process.stdout.write(JSON.stringify({pluginId:"codestory@CodeStoryDev",name:"codestory",marketplaceName:"CodeStoryDev"}));
  process.exit(0);
}
if(command!=="add")process.exit(11);
if(process.env.FAKE_ADD_MODE==="exit")process.exit(12);
fs.mkdirSync(path.dirname(installed),{recursive:true});
fs.cpSync(stage,installed,{recursive:true});
if(process.env.FAKE_ADD_MODE==="mutate-package")fs.appendFileSync(path.join(installed,"README.md"),"mutated");
if(process.env.FAKE_ADD_MODE==="mutate-cli"){
  const name=process.platform==="win32"?"codestory-cli.exe":"codestory-cli";
  fs.appendFileSync(path.join(installed,"bin",name),"mutated");
}
if(process.env.FAKE_ADD_MODE==="invalid-json"){
  process.stdout.write("not-json");
  process.exit(0);
}
const response={pluginId:"codestory@CodeStoryDev",name:"codestory",marketplaceName:"CodeStoryDev",version,installedPath:installed,authPolicy:"ON_INSTALL"};
if(process.env.FAKE_ADD_MODE==="wrong-identity")response.pluginId="codestory@Elsewhere";
if(process.env.FAKE_ADD_MODE==="wrong-version")response.version="9.9.9";
if(process.env.FAKE_ADD_MODE==="relative-path")response.installedPath="relative";
process.stdout.write(JSON.stringify(response));
`;
  await writeFile(script, body, "utf8");
  await chmod(script, 0o755);
}

async function fixture() {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), "codestory-dev-install-"));
  const checkout = path.join(root, "repo");
  const plugin = path.join(checkout, "plugins", "codestory");
  const home = path.join(root, "home");
  const stagingRoot = path.join(home, ".codex", "dev-plugins", "codestory");
  const marketplacePlugin = path.join(
    home,
    ".codex",
    "dev-marketplaces",
    "CodeStoryDev",
    "plugins",
    "codestory",
  );
  const pluginData = path.join(home, ".codex", "plugins", "data", "codestory-CodeStoryDev");
  const cacheRoot = path.join(home, ".codex", "plugins", "cache");
  const cli = path.join(root, contract.expectedBinaryName());
  const codex = path.join(root, "codex-fixture");
  await mkdir(path.dirname(plugin), { recursive: true });
  await cp(sourcePlugin, plugin, { recursive: true });
  git(checkout, "init", "-q");
  git(checkout, "config", "user.email", "fixture@example.invalid");
  git(checkout, "config", "user.name", "Fixture");
  git(checkout, "add", ".");
  git(checkout, "commit", "-qm", "fixture");
  await mkdir(path.dirname(marketplacePlugin), { recursive: true });
  await symlink(stagingRoot, marketplacePlugin, "dir");
  await mkdir(pluginData, { recursive: true });
  await writeFile(path.join(pluginData, "sentinel"), "keep", "utf8");
  await writeFakeCli(cli);
  await writeFakeCodex(codex);
  const options = {
    cli,
    codex,
    env: {
      FAKE_CACHE_ROOT: cacheRoot,
      FAKE_PLUGIN_VERSION: version,
      FAKE_STAGE_ROOT: stagingRoot,
    },
    marketplacePlugin,
    pluginData,
    pluginSource: plugin,
    repoRoot: checkout,
    stagingRoot,
  };
  return {
    cacheRoot,
    cli,
    codex,
    home,
    marketplacePlugin,
    options,
    plugin,
    pluginData,
    root,
    stagingRoot,
  };
}

async function cachedReceiptFixture() {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), "codestory-dev-receipt-"));
  const pluginRoot = path.join(
    root,
    ".codex",
    "plugins",
    "cache",
    "CodeStoryDev",
    "codestory",
    version,
  );
  await mkdir(path.dirname(pluginRoot), { recursive: true });
  await cp(sourcePlugin, pluginRoot, { recursive: true });
  const sourcePackageSha256 = contract.directoryContractSha256(pluginRoot);
  const cliName = contract.expectedBinaryName();
  const cliPath = path.join(pluginRoot, "bin", cliName);
  await writeFakeCli(cliPath);
  const cliBytes = await readFile(cliPath);
  const receipt = {
    schema_version: contract.receiptSchemaVersion,
    purpose: contract.receiptPurpose,
    plugin_id: contract.receiptPluginId,
    plugin_name: contract.receiptPluginName,
    plugin_version: version,
    source_commit: "a".repeat(40),
    source_package_sha256: sourcePackageSha256,
    target: contract.assetTarget(),
    cli: {
      path: `bin/${cliName}`,
      name: cliName,
      bytes: cliBytes.length,
      sha256: digest(cliBytes),
      version,
    },
  };
  const receiptPath = path.join(pluginRoot, contract.receiptName);
  await writeFile(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`, "utf8");
  return { cliPath, pluginRoot, receipt, receiptPath, root };
}

function install(fixtureValue, overrides = {}) {
  return installDevPlugin({
    ...fixtureValue.options,
    ...overrides,
    env: {
      ...fixtureValue.options.env,
      ...(overrides.env || {}),
    },
  });
}

test("CodeStoryDev installer stages and refreshes an exact receipt while preserving plugin data", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  const value = await fixture();
  try {
    const first = install(value);
    assert.equal(first.plugin_id, "codestory@CodeStoryDev");
    assert.equal(first.plugin_version, version);
    assert.equal(first.target, contract.assetTarget());
    assert.equal(first.cli.sha256, digest(await readFile(value.cli)));
    assert.equal(await readFile(path.join(value.pluginData, "sentinel"), "utf8"), "keep");
    assert.equal(first.plugin_data_preserved, true);
    assert.equal(
      contract.validateDevCliReceipt(first.installed_plugin_root, {
        expectedPluginVersion: version,
      }).state,
      "verified",
    );

    await writeFakeCli(value.cli, version, "# second exact build");
    const second = install(value);
    assert.notEqual(second.cli.sha256, first.cli.sha256);
    assert.equal(await readFile(path.join(value.pluginData, "sentinel"), "utf8"), "keep");
    assert.equal(
      contract.validateDevCliReceipt(second.installed_plugin_root, {
        expectedPluginVersion: version,
      }).sha256,
      second.cli.sha256,
    );
  } finally {
    await rm(value.root, { recursive: true, force: true });
  }
});

test("CodeStoryDev installer rejects missing, wrong-name, wrong-version, and symlink CLIs before plugin install", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  for (const variant of ["missing", "wrong-name", "wrong-version", "symlink"]) {
    const value = await fixture();
    try {
      let candidate = value.cli;
      if (variant === "missing") {
        candidate = path.join(value.root, "missing", contract.expectedBinaryName());
      } else if (variant === "wrong-name") {
        candidate = path.join(value.root, "other-cli");
        await writeFakeCli(candidate);
      } else if (variant === "wrong-version") {
        await writeFakeCli(candidate, "9.9.9");
      } else {
        const realCli = path.join(value.root, "real", contract.expectedBinaryName());
        await writeFakeCli(realCli);
        await rm(candidate, { force: true });
        await symlink(realCli, candidate);
      }
      assert.throws(
        () => install(value, { cli: candidate }),
        /codestory_cli|ENOENT/u,
        variant,
      );
      assert.equal(fs.existsSync(path.join(value.cacheRoot, "CodeStoryDev")), false);
    } finally {
      await rm(value.root, { recursive: true, force: true });
    }
  }
});

test("CodeStoryDev receipt rejects forged identity, digest, size, version, target, path escape, symlink, and package mutation", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  const mutations = [
    ["identity", (value) => { value.receipt.plugin_id = "codestory@Elsewhere"; }],
    ["digest", (value) => { value.receipt.cli.sha256 = "f".repeat(64); }],
    ["size", (value) => { value.receipt.cli.bytes += 1; }],
    ["version", (value) => { value.receipt.cli.version = "9.9.9"; }],
    ["target", (value) => { value.receipt.target = "wrong-target"; }],
    ["path escape", (value) => { value.receipt.cli.path = "../../outside"; }],
    ["source digest", (value) => { value.receipt.source_package_sha256 = "0".repeat(64); }],
    ["package mutation", async (value) => {
      await writeFile(path.join(value.pluginRoot, "README.md"), "changed", "utf8");
    }],
    ["cli symlink", async (value) => {
      const outside = path.join(value.root, "outside-cli");
      await writeFakeCli(outside);
      await rm(value.cliPath);
      await symlink(outside, value.cliPath);
    }],
    ["dangling receipt", async (value) => {
      await rm(value.receiptPath);
      await symlink(path.join(value.root, "missing-receipt"), value.receiptPath);
    }],
  ];
  for (const [label, mutate] of mutations) {
    const value = await cachedReceiptFixture();
    try {
      await mutate(value);
      if (!["package mutation", "cli symlink", "dangling receipt"].includes(label)) {
        await writeFile(value.receiptPath, `${JSON.stringify(value.receipt, null, 2)}\n`, "utf8");
      }
      const result = contract.validateDevCliReceipt(value.pluginRoot, {
        expectedPluginVersion: version,
      });
      assert.equal(result.state, "invalid", `${label}: ${JSON.stringify(result)}`);
    } finally {
      await rm(value.root, { recursive: true, force: true });
    }
  }
});

test("CodeStoryDev receipt detects CLI and package changes during version proof", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  for (const variant of ["cli", "package"]) {
    const value = await cachedReceiptFixture();
    try {
      const result = contract.validateDevCliReceipt(value.pluginRoot, {
        expectedPluginVersion: version,
        probeVersion(candidate) {
          if (variant === "cli") {
            fs.appendFileSync(candidate, "changed");
          } else {
            fs.appendFileSync(path.join(value.pluginRoot, "README.md"), "changed");
          }
          return { status: 0, stdout: `codestory-cli ${version}\n`, stderr: "" };
        },
      });
      assert.equal(result.state, "invalid", `${variant}: ${JSON.stringify(result)}`);
      assert.match(result.reason, /changed/u);
    } finally {
      await rm(value.root, { recursive: true, force: true });
    }
  }
});

test("CodeStoryDev installer rejects plugin-add errors and mutated cache copies", {
  skip: process.platform === "win32" ? "fixture uses a POSIX executable" : false,
}, async () => {
  for (const mode of [
    "exit",
    "invalid-json",
    "wrong-identity",
    "wrong-version",
    "relative-path",
    "mutate-package",
    "mutate-cli",
  ]) {
    const value = await fixture();
    try {
      assert.throws(
        () => install(value, {
          env: { FAKE_ADD_MODE: mode },
        }),
        /codex_plugin|installed_dev_receipt/u,
        mode,
      );
      assert.equal(await readFile(path.join(value.pluginData, "sentinel"), "utf8"), "keep");
      assert.equal(
        fs.existsSync(path.join(value.cacheRoot, "CodeStoryDev", "codestory", version)),
        false,
        `${mode} left an installed cache`,
      );
    } finally {
      await rm(value.root, { recursive: true, force: true });
    }
  }
});
