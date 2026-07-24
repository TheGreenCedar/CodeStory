#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

function fail(message) {
  throw new Error(message);
}

function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value == null) fail(`invalid argument: ${key}`);
    values[key.slice(2).replaceAll("-", "_")] = value;
  }
  return values;
}

function required(values, key) {
  const value = values[key];
  if (!value) fail(`--${key.replaceAll("_", "-")} is required`);
  return value;
}

function run(executable, args, options = {}) {
  let command = executable;
  let commandArgs = args;
  if (process.platform === "win32") {
    const quote = (value) => {
      if (/[\r\n"]/u.test(value)) fail("Codex command arguments must be single-line");
      return `"${value.replaceAll("%", "%%")}"`;
    };
    command = process.env.ComSpec || "cmd.exe";
    commandArgs = ["/d", "/s", "/c", [executable, ...args].map(quote).join(" ")];
  }
  const result = spawnSync(command, commandArgs, {
    ...options,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    fail(`${path.basename(executable)} ${args.join(" ")} failed: ${result.stderr.trim()}`);
  }
  return result.stdout.trim();
}

function parseJson(label, raw) {
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`${label} did not emit JSON: ${error.message}`);
  }
}

function filesUnder(root, relative = "") {
  const directory = path.join(root, relative);
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const child = path.join(relative, entry.name);
    if (entry.isSymbolicLink()) fail(`installed plugin contains a symlink: ${child}`);
    return entry.isDirectory() ? filesUnder(root, child) : [child];
  });
}

function directoryDigest(root) {
  const digest = createHash("sha256");
  const files = filesUnder(root).sort();
  if (files.length === 0) fail("installed plugin package is empty");
  for (const relative of files) {
    const normalized = relative.split(path.sep).join("/");
    const name = Buffer.from(normalized);
    const payload = readFileSync(path.join(root, relative));
    const nameLength = Buffer.alloc(8);
    const payloadLength = Buffer.alloc(8);
    nameLength.writeBigUInt64LE(BigInt(name.length));
    payloadLength.writeBigUInt64LE(BigInt(payload.length));
    digest.update(nameLength);
    digest.update(name);
    digest.update(payloadLength);
    digest.update(payload);
  }
  return digest.digest("hex");
}

function containedPath(root, candidate, label) {
  const relative = path.relative(root, candidate);
  if (relative.startsWith("..") || path.isAbsolute(relative)) {
    fail(`${label} is outside the isolated Codex home`);
  }
}

export function installMarketplaceProof(rawArgs) {
  const args = parseArgs(rawArgs);
  const codexPackageRoot = path.resolve(required(args, "codex_package_root"));
  const codexExecutable = path.join(
    codexPackageRoot,
    "node_modules",
    ".bin",
    process.platform === "win32" ? "codex.cmd" : "codex",
  );
  if (!statSync(codexExecutable).isFile()) fail(`pinned Codex CLI is missing: ${codexExecutable}`);

  const codexHomeInput = path.resolve(required(args, "codex_home"));
  mkdirSync(codexHomeInput, { recursive: true });
  const codexHome = realpathSync(codexHomeInput);
  const pluginDataInput = path.resolve(required(args, "plugin_data"));
  mkdirSync(pluginDataInput, { recursive: true });
  const pluginData = realpathSync(pluginDataInput);
  containedPath(codexHome, pluginData, "plugin data");

  const marketplaceSource = required(args, "marketplace_source");
  const marketplaceName = required(args, "marketplace_name");
  const marketplaceRevision = required(args, "marketplace_revision");
  if (!/^[0-9a-f]{40}$/u.test(marketplaceRevision)) {
    fail("marketplace revision must be an immutable commit");
  }
  const expectedVersion = required(args, "expected_version");
  const sourceCommit = required(args, "source_commit");
  const sourceTree = required(args, "source_tree");
  for (const [label, value] of [["source commit", sourceCommit], ["source tree", sourceTree]]) {
    if (!/^[0-9a-f]{40}$/u.test(value)) fail(`${label} must be an immutable Git identity`);
  }

  const env = { ...process.env, CODEX_HOME: codexHome };
  const codex = (...command) => run(codexExecutable, command, { env });
  const codexVersion = codex("--version");
  const addArguments = ["plugin", "marketplace", "add", marketplaceSource];
  if (args.local_fixture !== "true") addArguments.push("--ref", marketplaceRevision);
  addArguments.push("--json");
  const marketplaceAdd = parseJson("marketplace add", codex(...addArguments));
  const marketplaceList = parseJson(
    "marketplace list",
    codex("plugin", "marketplace", "list", "--json"),
  );
  const pluginAdd = parseJson(
    "plugin add",
    codex("plugin", "add", `codestory@${marketplaceName}`, "--json"),
  );
  const pluginList = parseJson("plugin list", codex("plugin", "list", "--json"));

  const pluginRoot = realpathSync(pluginAdd.installedPath);
  const expectedPluginRoot = path.join(
    codexHome,
    "plugins",
    "cache",
    marketplaceName,
    "codestory",
    expectedVersion,
  );
  if (pluginRoot !== expectedPluginRoot) {
    fail(`Codex installed the plugin at an unexpected cache path: ${pluginRoot}`);
  }
  containedPath(codexHome, pluginRoot, "installed plugin");
  if (
    marketplaceAdd.marketplaceName !== marketplaceName
    || marketplaceAdd.alreadyAdded !== false
    || pluginAdd.pluginId !== `codestory@${marketplaceName}`
    || pluginAdd.version !== expectedVersion
  ) {
    fail("Codex marketplace or plugin add result has an unexpected identity");
  }

  const attestation = {
    schema_version: 2,
    installation_source: "codex_marketplace_install",
    installation: {
      codex_home: codexHome,
      plugin_root: pluginRoot,
      plugin_data: pluginData,
    },
    plugin: {
      id: "codestory",
      version: expectedVersion,
      source_commit: sourceCommit,
      source_tree: sourceTree,
      package_sha256: directoryDigest(pluginRoot),
    },
    marketplace: {
      repository: marketplaceSource,
      revision: marketplaceRevision,
      codex_cli_version: codexVersion,
      add_result: marketplaceAdd,
      list_result: marketplaceList,
      plugin_add_result: pluginAdd,
      plugin_list_result: pluginList,
    },
  };
  const attestationPath = path.resolve(required(args, "attestation"));
  writeFileSync(attestationPath, `${JSON.stringify(attestation, null, 2)}\n`);
  if (args.github_output) {
    appendFileSync(
      path.resolve(args.github_output),
      [
        `plugin_root=${pluginRoot}`,
        `plugin_data=${pluginData}`,
        `attestation=${attestationPath}`,
        "",
      ].join("\n"),
    );
  }
  return attestation;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  installMarketplaceProof(process.argv.slice(2));
}
