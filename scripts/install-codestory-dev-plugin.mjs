#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const {
  assetTarget,
  directoryContractSha256,
  expectedBinaryName,
  receiptName,
  receiptPluginId,
  receiptPluginName,
  receiptPurpose,
  receiptSchemaVersion,
  validateDevCliReceipt,
} = require("../plugins/codestory/scripts/codestory-dev-cli-contract.cjs");

const scriptPath = fileURLToPath(import.meta.url);
const defaultRepoRoot = path.dirname(path.dirname(scriptPath));

function fail(message) {
  throw new Error(message);
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    if (!argument.startsWith("--")) fail(`unknown argument: ${argument}`);
    const name = argument.slice(2).replaceAll("-", "_");
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail(`${argument} requires a value`);
    options[name] = value;
    index += 1;
  }
  return options;
}

function usage() {
  return `Usage:
  node scripts/install-codestory-dev-plugin.mjs --cli <absolute-codestory-cli>

Options:
  --staging-root <path>       Default: ~/.codex/dev-plugins/codestory
  --marketplace-plugin <path> Default: ~/.codex/dev-marketplaces/CodeStoryDev/plugins/codestory
  --plugin-data <path>        Default: ~/.codex/plugins/data/codestory-CodeStoryDev
  --codex <path-or-command>   Default: codex

The source package is the clean committed plugins/codestory directory from this checkout.
The script stages one exact CLI and receipt, refreshes codestory@CodeStoryDev, and leaves
the production marketplace package and plugin data untouched.`;
}

function sha256(pathname) {
  return createHash("sha256").update(fs.readFileSync(pathname)).digest("hex");
}

function directFile(pathname, label) {
  const metadata = fs.lstatSync(pathname);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    fail(`${label}_not_direct_file:${pathname}`);
  }
  return metadata;
}

function directDirectory(pathname, label) {
  const metadata = fs.lstatSync(pathname);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    fail(`${label}_not_direct_directory:${pathname}`);
  }
}

function run(command, args, options = {}) {
  const completed = spawnSync(command, args, {
    cwd: options.cwd,
    encoding: "utf8",
    env: options.env,
    shell: false,
    timeout: options.timeout ?? 30_000,
    windowsHide: true,
  });
  if (completed.error) fail(`${options.label || command}_spawn:${completed.error.message}`);
  if (completed.status !== 0) {
    fail(
      `${options.label || command}_failed:status=${completed.status}:stderr=${String(completed.stderr || "").trim()}`,
    );
  }
  return completed.stdout;
}

function git(repoRoot, args) {
  return run("git", args, {
    cwd: repoRoot,
    env: process.env,
    label: `git_${args[0]}`,
  }).trim();
}

function listFiles(root, relative = "", files = []) {
  const directory = relative ? path.join(root, relative) : root;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const childRelative = relative ? path.join(relative, entry.name) : entry.name;
    const child = path.join(root, childRelative);
    const metadata = fs.lstatSync(child);
    if (metadata.isSymbolicLink()) fail(`source_package_symlink:${childRelative}`);
    if (metadata.isDirectory()) {
      listFiles(root, childRelative, files);
    } else if (metadata.isFile()) {
      files.push(childRelative.split(path.sep).join("/"));
    } else {
      fail(`source_package_non_file:${childRelative}`);
    }
  }
  return files.sort();
}

function verifyCommittedPluginSource(repoRoot, pluginSource) {
  directDirectory(repoRoot, "repository_root");
  directDirectory(pluginSource, "plugin_source");
  const expectedSource = path.join(repoRoot, "plugins", "codestory");
  if (fs.realpathSync(pluginSource) !== fs.realpathSync(expectedSource)) {
    fail(`plugin_source_not_repository_package:${pluginSource}`);
  }
  const relativeSource = path.relative(repoRoot, pluginSource).split(path.sep).join("/");
  const status = git(repoRoot, [
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
    "--ignored=matching",
    "--",
    relativeSource,
  ]);
  if (status) fail(`plugin_source_not_committed:${status.split(/\r?\n/u)[0]}`);
  const tracked = git(repoRoot, ["ls-files", "-z", "--", relativeSource])
    .split("\0")
    .filter(Boolean)
    .map((entry) => path.relative(relativeSource, entry).split(path.sep).join("/"))
    .sort();
  const actual = listFiles(pluginSource);
  if (JSON.stringify(tracked) !== JSON.stringify(actual)) {
    fail("plugin_source_inventory_not_committed");
  }
  return {
    commit: git(repoRoot, ["rev-parse", "HEAD"]),
    tree: git(repoRoot, ["rev-parse", `HEAD:${relativeSource}`]),
    sha256: directoryContractSha256(pluginSource),
  };
}

function pluginManifest(pluginSource) {
  const manifestPath = path.join(pluginSource, ".codex-plugin", "plugin.json");
  directFile(manifestPath, "plugin_manifest");
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  } catch (error) {
    fail(`plugin_manifest_json:${error.message}`);
  }
  if (
    manifest?.name !== receiptPluginName
    || !/^\d+\.\d+\.\d+$/u.test(String(manifest?.version || ""))
  ) {
    fail("plugin_manifest_identity");
  }
  return manifest;
}

function verifyCli(cliPath, version, options = {}) {
  if (!path.isAbsolute(cliPath)) fail("codestory_cli_must_be_absolute");
  const resolved = path.resolve(cliPath);
  const metadata = directFile(resolved, "codestory_cli");
  const binary = expectedBinaryName(options.platform);
  if (path.basename(resolved) !== binary) {
    fail(`codestory_cli_name:expected=${binary}`);
  }
  if ((options.platform || process.platform) !== "win32" && (metadata.mode & 0o111) === 0) {
    fail("codestory_cli_not_executable");
  }
  const completed = (options.probeVersion || ((candidate) => spawnSync(candidate, ["--version"], {
    encoding: "utf8",
    shell: false,
    timeout: 3_000,
    windowsHide: true,
  })))(resolved);
  if (completed.error || completed.status !== 0) fail("codestory_cli_version_probe_failed");
  const output = `${completed.stdout || ""}\n${completed.stderr || ""}`;
  const match = output.match(/\bcodestory-cli\s+v?(\d+\.\d+\.\d+)\b/u);
  if (!match || match[1] !== version) {
    fail(`codestory_cli_version:expected=${version}:actual=${match?.[1] || "unknown"}`);
  }
  return {
    path: resolved,
    name: binary,
    bytes: metadata.size,
    sha256: sha256(resolved),
    version,
  };
}

function marketplaceTargetsStaging(marketplacePlugin, stagingRoot) {
  const metadata = fs.lstatSync(marketplacePlugin);
  if (!metadata.isSymbolicLink()) fail("codestory_dev_marketplace_plugin_not_symlink");
  const link = fs.readlinkSync(marketplacePlugin);
  const target = path.resolve(path.dirname(marketplacePlugin), link);
  if (target !== path.resolve(stagingRoot)) {
    fail("codestory_dev_marketplace_plugin_wrong_target");
  }
}

function codexJson(codex, args, options) {
  const output = run(codex, args, {
    cwd: options.repoRoot,
    env: options.env,
    label: `codex_plugin_${args[1] || args[0]}`,
  });
  try {
    return JSON.parse(output);
  } catch (error) {
    fail(`codex_plugin_json:${error.message}`);
  }
}

function installedEntry(list) {
  if (!list || !Array.isArray(list.installed)) fail("codex_plugin_list_shape");
  return list.installed.find((entry) => entry?.pluginId === receiptPluginId) || null;
}

function verifyRemoveResponse(response) {
  if (
    response?.pluginId !== receiptPluginId
    || response?.name !== receiptPluginName
    || response?.marketplaceName !== "CodeStoryDev"
  ) {
    fail("codex_plugin_remove_identity");
  }
}

function verifyAddResponse(response, expectedVersion) {
  if (
    response?.pluginId !== receiptPluginId
    || response?.name !== receiptPluginName
    || response?.marketplaceName !== "CodeStoryDev"
    || response?.version !== expectedVersion
    || response?.authPolicy !== "ON_INSTALL"
    || typeof response?.installedPath !== "string"
    || !path.isAbsolute(response.installedPath)
  ) {
    fail("codex_plugin_add_identity");
  }
  return path.resolve(response.installedPath);
}

function replaceDirectory(candidate, destination) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const backup = `${destination}.backup-${process.pid}-${Date.now()}`;
  let previous = false;
  if (fs.existsSync(destination)) {
    directDirectory(destination, "codestory_dev_staging_root");
    fs.renameSync(destination, backup);
    previous = true;
  }
  try {
    fs.renameSync(candidate, destination);
    return {
      backup: previous ? backup : null,
      commit() {
        if (previous) fs.rmSync(backup, { recursive: true, force: true });
      },
      rollback() {
        fs.rmSync(destination, { recursive: true, force: true });
        if (previous) fs.renameSync(backup, destination);
      },
    };
  } catch (error) {
    if (previous && fs.existsSync(backup) && !fs.existsSync(destination)) {
      fs.renameSync(backup, destination);
    }
    throw error;
  }
}

function stageCandidate({
  cli,
  pluginSource,
  sourceIdentity,
  stagingRoot,
  version,
  platform,
  arch,
  probeVersion,
}) {
  const parent = path.dirname(stagingRoot);
  fs.mkdirSync(parent, { recursive: true });
  const candidate = fs.mkdtempSync(path.join(parent, `.${path.basename(stagingRoot)}.staging-`));
  try {
    fs.cpSync(pluginSource, candidate, {
      recursive: true,
      dereference: false,
      errorOnExist: false,
      preserveTimestamps: true,
    });
    const binDir = path.join(candidate, "bin");
    fs.mkdirSync(binDir, { mode: 0o755 });
    const destination = path.join(binDir, cli.name);
    fs.copyFileSync(cli.path, destination, fs.constants.COPYFILE_EXCL);
    if ((platform || process.platform) !== "win32") fs.chmodSync(destination, 0o755);
    const receipt = {
      schema_version: receiptSchemaVersion,
      purpose: receiptPurpose,
      plugin_id: receiptPluginId,
      plugin_name: receiptPluginName,
      plugin_version: version,
      source_commit: sourceIdentity.commit,
      source_package_sha256: sourceIdentity.sha256,
      target: assetTarget(platform, arch),
      cli: {
        path: `bin/${cli.name}`,
        name: cli.name,
        bytes: cli.bytes,
        sha256: cli.sha256,
        version: cli.version,
      },
    };
    fs.writeFileSync(path.join(candidate, receiptName), `${JSON.stringify(receipt, null, 2)}\n`, {
      encoding: "utf8",
      mode: 0o600,
      flag: "wx",
    });
    const verified = validateDevCliReceipt(candidate, {
      arch,
      expectedPluginVersion: version,
      platform,
      probeVersion,
      requireCacheIdentity: false,
    });
    if (verified.state !== "verified") {
      fail(`staged_dev_receipt_invalid:${verified.reason}`);
    }
    return candidate;
  } catch (error) {
    fs.rmSync(candidate, { recursive: true, force: true });
    throw error;
  }
}

export function installDevPlugin(rawOptions = {}) {
  const repoRoot = path.resolve(rawOptions.repoRoot || defaultRepoRoot);
  const pluginSource = path.resolve(rawOptions.pluginSource || path.join(repoRoot, "plugins", "codestory"));
  const home = rawOptions.home || os.homedir();
  const stagingRoot = path.resolve(
    rawOptions.stagingRoot || path.join(home, ".codex", "dev-plugins", "codestory"),
  );
  const marketplacePlugin = path.resolve(
    rawOptions.marketplacePlugin
      || path.join(home, ".codex", "dev-marketplaces", "CodeStoryDev", "plugins", "codestory"),
  );
  const pluginData = path.resolve(
    rawOptions.pluginData
      || path.join(home, ".codex", "plugins", "data", "codestory-CodeStoryDev"),
  );
  const codex = rawOptions.codex || "codex";
  const env = { ...process.env, ...(rawOptions.env || {}) };
  const cliPath = rawOptions.cli;
  if (!cliPath) fail("--cli is required");

  const sourceIdentity = verifyCommittedPluginSource(repoRoot, pluginSource);
  const manifest = pluginManifest(pluginSource);
  const cli = verifyCli(path.resolve(cliPath), manifest.version, rawOptions);
  const target = assetTarget(rawOptions.platform, rawOptions.arch);
  if (!target) fail(`unsupported_target:${rawOptions.platform || process.platform}-${rawOptions.arch || process.arch}`);
  marketplaceTargetsStaging(marketplacePlugin, stagingRoot);

  const candidate = stageCandidate({
    arch: rawOptions.arch,
    cli,
    platform: rawOptions.platform,
    pluginSource,
    probeVersion: rawOptions.probeVersion,
    sourceIdentity,
    stagingRoot,
    version: manifest.version,
  });
  const replacement = replaceDirectory(candidate, stagingRoot);
  let previouslyInstalled = false;
  let addAttempted = false;
  try {
    const list = codexJson(codex, ["plugin", "list", "--json"], { env, repoRoot });
    previouslyInstalled = Boolean(installedEntry(list));
    if (previouslyInstalled) {
      verifyRemoveResponse(
        codexJson(codex, ["plugin", "remove", receiptPluginId, "--json"], { env, repoRoot }),
      );
    }
    addAttempted = true;
    const added = codexJson(codex, ["plugin", "add", receiptPluginId, "--json"], { env, repoRoot });
    const installedPath = verifyAddResponse(added, manifest.version);
    directDirectory(installedPath, "installed_plugin_root");
    const installed = validateDevCliReceipt(installedPath, {
      arch: rawOptions.arch,
      expectedPluginVersion: manifest.version,
      platform: rawOptions.platform,
      probeVersion: rawOptions.probeVersion,
    });
    if (installed.state !== "verified") {
      fail(`installed_dev_receipt_invalid:${installed.reason}`);
    }
    if (
      installed.sha256 !== cli.sha256
      || installed.sourcePackageSha256 !== sourceIdentity.sha256
      || installed.sourceCommit !== sourceIdentity.commit
    ) {
      fail("installed_dev_receipt_identity_changed");
    }
    replacement.commit();
    return {
      schema_version: 1,
      purpose: "codestory-dev-plugin-install-result",
      plugin_id: receiptPluginId,
      plugin_version: manifest.version,
      source_commit: sourceIdentity.commit,
      source_tree: sourceIdentity.tree,
      source_package_sha256: sourceIdentity.sha256,
      target,
      staged_plugin_root: stagingRoot,
      installed_plugin_root: installedPath,
      plugin_data: pluginData,
      plugin_data_preserved: fs.existsSync(pluginData),
      cli: {
        path: installed.path,
        name: cli.name,
        bytes: cli.bytes,
        sha256: cli.sha256,
        version: cli.version,
      },
    };
  } catch (error) {
    replacement.rollback();
    if ((addAttempted || previouslyInstalled) && rawOptions.restoreOnFailure !== false) {
      try {
        codexJson(codex, ["plugin", "remove", receiptPluginId, "--json"], { env, repoRoot });
      } catch {
        // Best-effort cleanup before restoring the prior package or empty state.
      }
    }
    if (previouslyInstalled && rawOptions.restoreOnFailure !== false) {
      try {
        codexJson(codex, ["plugin", "add", receiptPluginId, "--json"], { env, repoRoot });
      } catch {
        // Preserve the original failure. The staged prior package remains available.
      }
    }
    throw error;
  }
}

export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.help) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const result = installDevPlugin({
    cli: args.cli,
    codex: args.codex,
    marketplacePlugin: args.marketplace_plugin,
    pluginData: args.plugin_data,
    stagingRoot: args.staging_root,
  });
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) {
  main().catch((error) => {
    process.stderr.write(`install-codestory-dev-plugin: ${error.message}\n`);
    process.exitCode = 1;
  });
}
