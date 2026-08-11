#!/usr/bin/env node

import { randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const defaultRepoRoot = path.dirname(path.dirname(scriptPath));
export const cursorLocalOverrideFileName = "local-overrides.json";

function fail(message) {
  throw new Error(message);
}

export function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    if (!argument.startsWith("--")) fail(`unknown argument: ${argument}`);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail(`${argument} requires a value`);
    const name = argument.slice(2).replaceAll("-", "_");
    if (!["cli", "install_root", "plugin_data", "plugin_source", "repo_root"].includes(name)) {
      fail(`unknown argument: ${argument}`);
    }
    options[name] = value;
    index += 1;
  }
  return options;
}

function usage() {
  return `Usage:
  node scripts/install-codestory-cursor-plugin.mjs [--cli <absolute-codestory-cli>]

Options:
  --install-root <path>   Default: ~/.cursor/plugins/local/codestory
  --plugin-data <path>    Default: ~/.cursor/plugins/data/codestory

The installer links the clean committed plugins/codestory package into Cursor.
With --cli, Cursor uses that exact local build instead of downloading the managed runtime.`;
}

function runGit(repoRoot, args) {
  const completed = spawnSync("git", args, {
    cwd: repoRoot,
    encoding: "utf8",
    shell: false,
    windowsHide: true,
  });
  if (completed.error) fail(`git_${args[0]}_spawn:${completed.error.message}`);
  if (completed.status !== 0) {
    fail(`git_${args[0]}_failed:${String(completed.stderr || "").trim()}`);
  }
  return completed.stdout.trim();
}

function directDirectory(directory, label) {
  const metadata = fs.lstatSync(directory);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    fail(`${label}_not_direct_directory:${directory}`);
  }
}

function listFiles(root, relative = "", files = []) {
  const directory = relative ? path.join(root, relative) : root;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const childRelative = relative ? path.join(relative, entry.name) : entry.name;
    const child = path.join(root, childRelative);
    const metadata = fs.lstatSync(child);
    if (metadata.isSymbolicLink()) fail(`source_package_symlink:${childRelative}`);
    if (metadata.isDirectory()) listFiles(root, childRelative, files);
    else if (metadata.isFile()) files.push(childRelative.split(path.sep).join("/"));
    else fail(`source_package_non_file:${childRelative}`);
  }
  return files.sort();
}

function verifyCommittedPluginSource(repoRoot, pluginSource) {
  directDirectory(repoRoot, "repository_root");
  directDirectory(pluginSource, "plugin_source");
  const expected = path.join(repoRoot, "plugins", "codestory");
  if (fs.realpathSync(pluginSource) !== fs.realpathSync(expected)) {
    fail(`plugin_source_not_repository_package:${pluginSource}`);
  }
  const relative = path.relative(repoRoot, pluginSource).split(path.sep).join("/");
  const status = runGit(repoRoot, [
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
    "--ignored=matching",
    "--",
    relative,
  ]);
  if (status) fail(`plugin_source_not_committed:${status.split(/\r?\n/u)[0]}`);
  const tracked = runGit(repoRoot, ["ls-files", "-z", "--", relative])
    .split("\0")
    .filter(Boolean)
    .map((entry) => path.relative(relative, entry).split(path.sep).join("/"))
    .sort();
  if (JSON.stringify(tracked) !== JSON.stringify(listFiles(pluginSource))) {
    fail("plugin_source_inventory_not_committed");
  }
  return runGit(repoRoot, ["rev-parse", "HEAD"]);
}

function pluginVersion(pluginSource) {
  const manifestPath = path.join(pluginSource, ".cursor-plugin", "plugin.json");
  const metadata = fs.lstatSync(manifestPath);
  if (!metadata.isFile() || metadata.isSymbolicLink()) fail("cursor_plugin_manifest_not_direct_file");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (manifest?.name !== "codestory" || !/^\d+\.\d+\.\d+$/u.test(manifest?.version || "")) {
    fail("cursor_plugin_manifest_identity");
  }
  return manifest.version;
}

function verifyCli(cli, expectedVersion) {
  if (!path.isAbsolute(cli)) fail("codestory_cli_must_be_absolute");
  const resolved = path.resolve(cli);
  const metadata = fs.lstatSync(resolved);
  if (!metadata.isFile() || metadata.isSymbolicLink()) fail("codestory_cli_not_direct_file");
  const expectedName = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  if (path.basename(resolved) !== expectedName) fail(`codestory_cli_name:expected=${expectedName}`);
  if (process.platform !== "win32" && (metadata.mode & 0o111) === 0) {
    fail("codestory_cli_not_executable");
  }
  const completed = spawnSync(resolved, ["--version"], {
    encoding: "utf8",
    shell: false,
    timeout: 3_000,
    windowsHide: true,
  });
  if (completed.error || completed.status !== 0) fail("codestory_cli_version_probe_failed");
  const output = `${completed.stdout || ""}\n${completed.stderr || ""}`;
  const match = output.match(/\bcodestory-cli\s+v?(\d+\.\d+\.\d+)\b/u);
  if (match?.[1] !== expectedVersion) {
    fail(`codestory_cli_version:expected=${expectedVersion}:actual=${match?.[1] || "unknown"}`);
  }
  return resolved;
}

function ensurePluginLink(pluginSource, installRoot) {
  fs.mkdirSync(path.dirname(installRoot), { recursive: true });
  try {
    const metadata = fs.lstatSync(installRoot);
    if (!metadata.isSymbolicLink()) fail(`cursor_plugin_install_not_symlink:${installRoot}`);
    const target = path.resolve(path.dirname(installRoot), fs.readlinkSync(installRoot));
    if (fs.realpathSync(target) !== fs.realpathSync(pluginSource)) {
      fail(`cursor_plugin_install_wrong_target:${installRoot}`);
    }
    return false;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  fs.symlinkSync(pluginSource, installRoot, process.platform === "win32" ? "junction" : "dir");
  return true;
}

function writeCursorOverride(pluginData, cli) {
  fs.mkdirSync(pluginData, { recursive: true, mode: 0o700 });
  const overridePath = path.join(pluginData, cursorLocalOverrideFileName);
  if (!cli) {
    fs.rmSync(overridePath, { force: true });
    return null;
  }
  const temporary = `${overridePath}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
  try {
    fs.writeFileSync(
      temporary,
      `${JSON.stringify({ schema_version: 1, CODESTORY_CLI: cli }, null, 2)}\n`,
      { encoding: "utf8", mode: 0o600, flag: "wx" },
    );
    fs.renameSync(temporary, overridePath);
  } finally {
    fs.rmSync(temporary, { force: true });
  }
  return overridePath;
}

export function installCursorPlugin(options = {}) {
  const home = options.home || os.homedir();
  const repoRoot = path.resolve(options.repoRoot || defaultRepoRoot);
  const pluginSource = path.resolve(options.pluginSource || path.join(repoRoot, "plugins", "codestory"));
  const installRoot = path.resolve(
    options.installRoot || path.join(home, ".cursor", "plugins", "local", "codestory"),
  );
  const pluginData = path.resolve(
    options.pluginData || path.join(home, ".cursor", "plugins", "data", "codestory"),
  );
  const sourceCommit = verifyCommittedPluginSource(repoRoot, pluginSource);
  const version = pluginVersion(pluginSource);
  const cli = options.cli ? verifyCli(options.cli, version) : null;
  const linked = ensurePluginLink(pluginSource, installRoot);
  const overridePath = writeCursorOverride(pluginData, cli);
  return {
    plugin: "codestory",
    version,
    source_commit: sourceCommit,
    install_root: installRoot,
    plugin_data: pluginData,
    linked,
    cli_override: overridePath ? "configured" : "managed",
  };
}

function main(argv = process.argv.slice(2)) {
  const parsed = parseArgs(argv);
  if (parsed.help) {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const result = installCursorPlugin({
    cli: parsed.cli,
    installRoot: parsed.install_root,
    pluginData: parsed.plugin_data,
    pluginSource: parsed.plugin_source,
    repoRoot: parsed.repo_root,
  });
  process.stdout.write([
    `Installed codestory ${result.version} for Cursor at ${result.install_root}.`,
    "Reload the Cursor window, open Customize, and enable the codestory MCP server.",
    "Then ask: Where is request validation implemented, who calls it, and which tests cover it?",
    "",
  ].join("\n"));
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
