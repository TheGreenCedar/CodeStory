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

const pinnedSourceKeys = ["path", "sha", "source", "url"];

function pinnedPluginSource(source, expectedUrl) {
  if (
    !source
    || Object.keys(source).sort().join(",") !== pinnedSourceKeys.join(",")
    || source.source !== "git-subdir"
    || source.path !== "plugins/codestory"
    || !/^[0-9a-f]{40}$/u.test(source.sha)
    || (expectedUrl && source.url !== expectedUrl)
  ) {
    fail("Codex marketplace plugin source is not pinned to one immutable commit");
  }
  return source;
}

function samePinnedSource(left, right) {
  return pinnedSourceKeys.every((key) => left[key] === right[key]);
}

function containedPath(root, candidate, label) {
  const relative = path.relative(root, candidate);
  if (relative.startsWith("..") || path.isAbsolute(relative)) {
    fail(`${label} is outside the isolated Codex home`);
  }
}

function marketplaceRevisionAt(root) {
  const revision = run("git", ["-C", root, "rev-parse", "HEAD"]);
  if (!/^[0-9a-f]{40}$/u.test(revision)) {
    fail(`marketplace checkout has invalid Git identity: ${revision}`);
  }
  return revision;
}

function prepareInstallation(rawArgs) {
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
  const sourceRepository = realpathSync(path.resolve(required(args, "source_repository")));
  const releaseTree = run("git", ["-C", sourceRepository, "rev-parse", "HEAD^{tree}"]);
  if (!/^[0-9a-f]{40}$/u.test(releaseTree)) {
    fail("release tree must be an immutable Git identity");
  }
  const sourcePluginRoot = realpathSync(
    path.join(sourceRepository, "plugins", "codestory"),
  );
  return {
    args,
    codexExecutable,
    codexHome,
    pluginData,
    marketplaceSource,
    marketplaceName,
    marketplaceRevision,
    expectedVersion,
    sourceRepository,
    releaseTree,
    expectedPackageSha256: directoryDigest(sourcePluginRoot),
  };
}

function installMarketplace(setup) {
  const env = { ...process.env, CODEX_HOME: setup.codexHome };
  const codex = (...command) => run(setup.codexExecutable, command, { env });
  const codexVersion = codex("--version");
  const addArguments = ["plugin", "marketplace", "add", setup.marketplaceSource];
  if (setup.args.local_fixture !== "true") {
    addArguments.push("--ref", setup.marketplaceRevision);
  }
  addArguments.push("--json");
  const marketplaceAdd = parseJson("marketplace add", codex(...addArguments));
  const marketplaceList = parseJson(
    "marketplace list",
    codex("plugin", "marketplace", "list", "--json"),
  );
  const pluginAdd = parseJson(
    "plugin add",
    codex("plugin", "add", `codestory@${setup.marketplaceName}`, "--json"),
  );
  const pluginList = parseJson("plugin list", codex("plugin", "list", "--json"));
  return { codexVersion, marketplaceAdd, marketplaceList, pluginAdd, pluginList };
}

function verifyInstallation(setup, installed) {
  const pluginRoot = realpathSync(installed.pluginAdd.installedPath);
  const expectedPluginRoot = realpathSync(
    path.join(
      setup.codexHome,
      "plugins",
      "cache",
      setup.marketplaceName,
      "codestory",
      setup.expectedVersion,
    ),
  );
  if (pluginRoot !== expectedPluginRoot) {
    fail(`Codex installed the plugin at an unexpected cache path: ${pluginRoot}`);
  }
  containedPath(setup.codexHome, pluginRoot, "installed plugin");
  if (
    installed.marketplaceAdd.marketplaceName !== setup.marketplaceName
    || installed.marketplaceAdd.alreadyAdded !== false
    || installed.pluginAdd.pluginId !== `codestory@${setup.marketplaceName}`
    || installed.pluginAdd.version !== setup.expectedVersion
  ) {
    fail("Codex marketplace or plugin add result has an unexpected identity");
  }
  const installedPlugins = installed.pluginList.installed;
  const availablePlugins = installed.pluginList.available;
  const pluginListEntry = installedPlugins?.[0];
  const expectedSourceUrl = setup.args.local_fixture === "true"
    ? undefined
    : "https://github.com/TheGreenCedar/CodeStory.git";
  if (
    !Array.isArray(installedPlugins)
    || installedPlugins.length !== 1
    || !Array.isArray(availablePlugins)
    || availablePlugins.length !== 0
    || pluginListEntry?.pluginId !== `codestory@${setup.marketplaceName}`
  ) {
    fail("Codex plugin list has an unexpected identity");
  }
  const pluginSource = pinnedPluginSource(pluginListEntry.source, expectedSourceUrl);
  const pluginSourceCommit = run("git", [
    "-C",
    setup.sourceRepository,
    "rev-parse",
    `${pluginSource.sha}^{commit}`,
  ]);
  const pluginSourceTree = run("git", [
    "-C",
    setup.sourceRepository,
    "rev-parse",
    `${pluginSource.sha}^{tree}`,
  ]);
  if (
    pluginSourceCommit !== pluginSource.sha
    || pluginSourceTree !== setup.releaseTree
  ) {
    fail("pinned marketplace plugin source does not match the release source tree");
  }
  const listedMarketplaces = installed.marketplaceList.marketplaces;
  const marketplaceListEntry = listedMarketplaces?.[0];
  if (
    !Array.isArray(listedMarketplaces)
    || listedMarketplaces.length !== 1
    || marketplaceListEntry?.name !== setup.marketplaceName
  ) {
    fail("Codex marketplace list has an unexpected identity");
  }
  const marketplaceAddRoot = realpathSync(installed.marketplaceAdd.installedRoot);
  const marketplaceListRoot = realpathSync(marketplaceListEntry.root);
  const marketplaceAddRevision = marketplaceRevisionAt(marketplaceAddRoot);
  const marketplaceListRevision = marketplaceRevisionAt(marketplaceListRoot);
  if (
    marketplaceAddRoot !== marketplaceListRoot
    || marketplaceAddRevision !== setup.marketplaceRevision
    || marketplaceListRevision !== setup.marketplaceRevision
  ) {
    fail("Codex marketplace provenance does not match the requested immutable revision");
  }
  const catalog = JSON.parse(
    readFileSync(
      path.join(marketplaceAddRoot, ".agents", "plugins", "marketplace.json"),
      "utf8",
    ),
  );
  const catalogEntries = catalog.plugins?.filter(({ name }) => name === "codestory") ?? [];
  const catalogSource = pinnedPluginSource(
    catalogEntries[0]?.source,
    expectedSourceUrl,
  );
  if (
    catalogEntries.length !== 1
    || !samePinnedSource(catalogSource, pluginSource)
  ) {
    fail("Codex plugin list source does not match the pinned marketplace catalog");
  }
  const packageSha256 = directoryDigest(pluginRoot);
  if (packageSha256 !== setup.expectedPackageSha256) {
    fail("installed plugin bytes do not match the checked-out CodeStory package");
  }
  return {
    pluginRoot,
    packageSha256,
    marketplaceAddRoot,
    marketplaceListRoot,
    marketplaceAddRevision,
    marketplaceListRevision,
    pluginSourceCommit,
    pluginSourceTree,
  };
}

function attestInstallation(setup, installed, verified) {
  const attestation = {
    schema_version: 2,
    installation_source: "codex_marketplace_install",
    installation: {
      codex_home: setup.codexHome,
      plugin_root: verified.pluginRoot,
      plugin_data: setup.pluginData,
    },
    plugin: {
      id: "codestory",
      version: setup.expectedVersion,
      source_commit: verified.pluginSourceCommit,
      source_tree: verified.pluginSourceTree,
      package_sha256: verified.packageSha256,
    },
    marketplace: {
      repository: setup.marketplaceSource,
      revision: setup.marketplaceRevision,
      provenance: {
        add: {
          root: verified.marketplaceAddRoot,
          revision: verified.marketplaceAddRevision,
        },
        list: {
          root: verified.marketplaceListRoot,
          revision: verified.marketplaceListRevision,
        },
      },
      codex_cli_version: installed.codexVersion,
      add_result: installed.marketplaceAdd,
      list_result: installed.marketplaceList,
      plugin_add_result: installed.pluginAdd,
      plugin_list_result: installed.pluginList,
    },
  };
  const attestationPath = path.resolve(required(setup.args, "attestation"));
  writeFileSync(attestationPath, `${JSON.stringify(attestation, null, 2)}\n`);
  if (setup.args.github_output) {
    appendFileSync(
      path.resolve(setup.args.github_output),
      [
        `plugin_root=${verified.pluginRoot}`,
        `plugin_data=${setup.pluginData}`,
        `attestation=${attestationPath}`,
        "",
      ].join("\n"),
    );
  }
  return attestation;
}

export function installMarketplaceProof(rawArgs) {
  const setup = prepareInstallation(rawArgs);
  const installed = installMarketplace(setup);
  const verified = verifyInstallation(setup, installed);
  return attestInstallation(setup, installed, verified);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  installMarketplaceProof(process.argv.slice(2));
}
