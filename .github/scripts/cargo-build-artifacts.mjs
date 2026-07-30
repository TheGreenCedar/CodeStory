#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SCHEMA = "codestory.cargo-build-artifacts/v1";
const SHA40 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const ALIAS = /^[a-z][a-z0-9_]*$/u;
const WINDOWS_TARGET = "x86_64-pc-windows-msvc";
const RELEASE_OPT_LEVELS = new Set(["1", "2", "3", "s", "z"]);
const ARTIFACT_CONTRACT = {
  cli: {
    packageName: "codestory-cli",
    kind: "bin",
    targetName: "codestory-cli",
  },
  runtime: {
    packageName: "codestory-cli",
    kind: "bin",
    targetName: "codestory-cli-runtime",
  },
  native_staging: {
    packageName: "codestory-llama-sys",
    kind: "test",
    targetName: "native_staging",
  },
  windows_path_identity: {
    packageName: "codestory-workspace",
    kind: "test",
    targetName: "windows_path_identity",
  },
  qualification_driver: {
    packageName: "codestory-bench",
    kind: "bin",
    targetName: "codestory_embedding_qualification",
  },
};
const REQUIRED_ALIASES = [
  "cli",
  "runtime",
  "native_staging",
  "windows_path_identity",
];

function fail(message) {
  throw new Error(message);
}

function exactKeys(value, keys, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${label} keys changed`);
  }
}

function requireSha(value, label) {
  if (!SHA40.test(value)) {
    fail(`${label} must be a full lowercase Git digest`);
  }
}

function requireWindowsTarget(value) {
  if (value !== WINDOWS_TARGET) {
    fail(`unsupported Cargo artifact target ${value}`);
  }
}

function packageNameFromId(packageId) {
  if (typeof packageId !== "string") {
    fail("Cargo compiler artifact package_id must be a string");
  }
  const fragment = packageId.lastIndexOf("#");
  if (fragment >= 0) {
    const packageAndVersion = packageId.slice(fragment + 1);
    const separator = packageAndVersion.lastIndexOf("@");
    if (separator > 0) return packageAndVersion.slice(0, separator);
  }
  const legacy = /^([^\s]+)\s+\d+\.\d+\.\d+(?:[-+][^\s]+)?(?:\s|$)/u.exec(packageId);
  if (legacy) return legacy[1];
  return null;
}

function packageNameFromArtifact(message) {
  const fromId = packageNameFromId(message.package_id);
  if (fromId) return fromId;
  if (typeof message.manifest_path !== "string" || message.manifest_path === "") {
    fail(`Cargo artifact package name is unavailable for ${message.package_id}`);
  }
  const manifest = fs.readFileSync(message.manifest_path, "utf8");
  let inPackage = false;
  for (const line of manifest.split(/\r?\n/u)) {
    const section = /^\s*\[([^\]]+)\]\s*$/u.exec(line);
    if (section) {
      inPackage = section[1] === "package";
      continue;
    }
    if (!inPackage) continue;
    const name = /^\s*name\s*=\s*["']([^"']+)["']\s*(?:#.*)?$/u.exec(line);
    if (name) return name[1];
  }
  fail(`Cargo package manifest has no package name: ${message.manifest_path}`);
}

function parseExpectation(value) {
  if (typeof value !== "string") fail("artifact expectation must be a string");
  const separator = value.indexOf("=");
  if (separator <= 0 || separator === value.length - 1) {
    fail("artifact expectation must use alias=package:kind:target");
  }
  const alias = value.slice(0, separator);
  const fields = value.slice(separator + 1).split(":");
  if (!ALIAS.test(alias)) fail(`invalid artifact alias ${alias}`);
  if (fields.length !== 3 || fields.some((field) => field === "")) {
    fail(`invalid artifact expectation ${value}`);
  }
  const [packageName, kind, targetName] = fields;
  if (!["bin", "test"].includes(kind)) {
    fail(`unsupported artifact kind ${kind}`);
  }
  return { alias, packageName, kind, targetName };
}

function requireArtifactContract(expectations) {
  const aliases = expectations.map(({ alias }) => alias).sort();
  const required = [...REQUIRED_ALIASES].sort();
  const withDriver = [...REQUIRED_ALIASES, "qualification_driver"].sort();
  if (
    JSON.stringify(aliases) !== JSON.stringify(required)
    && JSON.stringify(aliases) !== JSON.stringify(withDriver)
  ) {
    fail("Windows release graph artifact set changed");
  }
  for (const expectation of expectations) {
    const contract = ARTIFACT_CONTRACT[expectation.alias];
    if (
      !contract
      || expectation.packageName !== contract.packageName
      || expectation.kind !== contract.kind
      || expectation.targetName !== contract.targetName
    ) {
      fail(`Windows release graph artifact contract changed for ${expectation.alias}`);
    }
  }
}

function parseCargoMessages(jsonLines) {
  const messages = [];
  for (const [index, line] of jsonLines.split(/\r?\n/u).entries()) {
    if (line.trim() === "") continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      fail(`Cargo JSON output line ${index + 1} is not valid JSON`);
    }
    if (message?.reason === "compiler-artifact") messages.push(message);
  }
  return messages;
}

function releaseProfile(message, kind) {
  const profile = message.profile;
  exactKeys(
    profile,
    [
      "opt_level",
      "debuginfo",
      "debug_assertions",
      "overflow_checks",
      "test",
    ],
    "Cargo artifact profile",
  );
  const optLevel = String(profile.opt_level);
  if (
    !RELEASE_OPT_LEVELS.has(optLevel)
    || profile.debug_assertions !== false
    || profile.overflow_checks !== false
  ) {
    fail("Cargo artifact was not built with the release profile");
  }
  const expectedTest = kind === "test";
  if (profile.test !== expectedTest) {
    fail(`Cargo artifact profile.test did not match ${kind}`);
  }
  return {
    opt_level: optLevel,
    debuginfo: profile.debuginfo,
    debug_assertions: false,
    overflow_checks: false,
    test: expectedTest,
  };
}

function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return (
    relative !== ""
    && relative !== ".."
    && !relative.startsWith(`..${path.sep}`)
    && !path.isAbsolute(relative)
  );
}

function hashFile(file) {
  const hash = crypto.createHash("sha256");
  const handle = fs.openSync(file, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const bytesRead = fs.readSync(handle, buffer, 0, buffer.length, null);
      if (bytesRead === 0) break;
      hash.update(buffer.subarray(0, bytesRead));
    }
  } finally {
    fs.closeSync(handle);
  }
  return hash.digest("hex");
}

function validatedExecutable(message, {
  kind,
  profileRoot,
  targetName,
}) {
  if (typeof message.executable !== "string" || message.executable === "") {
    fail(`Cargo artifact ${targetName} did not emit an executable`);
  }
  if (
    !Array.isArray(message.filenames)
    || !message.filenames.some(
      (filename) => path.resolve(filename) === path.resolve(message.executable),
    )
  ) {
    fail(`Cargo artifact ${targetName} executable is not one of its emitted filenames`);
  }
  const executable = path.resolve(message.executable);
  const metadata = fs.lstatSync(executable);
  if (
    !metadata.isFile()
    || metadata.isSymbolicLink()
    || metadata.nlink !== 1
  ) {
    fail(
      `Cargo artifact ${targetName} must be a regular, non-symlink, singly linked file`,
    );
  }
  if (path.extname(executable).toLowerCase() !== ".exe") {
    fail(`Cargo artifact ${targetName} is not a Windows executable`);
  }

  const realProfileRoot = fs.realpathSync(profileRoot);
  const realExecutable = fs.realpathSync(executable);
  if (!isWithin(realProfileRoot, realExecutable)) {
    fail(`Cargo artifact ${targetName} escaped the exact target release directory`);
  }
  const relative = path.relative(realProfileRoot, realExecutable);
  if (kind === "test" && path.dirname(relative) !== "deps") {
    fail(`Cargo test artifact ${targetName} was not emitted under release/deps`);
  }
  if (kind === "bin" && path.dirname(relative) !== ".") {
    fail(`Cargo binary artifact ${targetName} was not emitted at the release root`);
  }

  return {
    path: executable,
    relative_path: relative.split(path.sep).join("/"),
    bytes: metadata.size,
    sha256: hashFile(executable),
  };
}

function matchingArtifacts(messages, expectation, workspaceRoot) {
  const expectedManifest = fs.realpathSync(
    path.join(workspaceRoot, "crates", expectation.packageName, "Cargo.toml"),
  );
  return messages.filter((message) => {
    if (message?.target?.name !== expectation.targetName) return false;
    if (!Array.isArray(message.target.kind)) return false;
    if (!message.target.kind.includes(expectation.kind)) return false;
    if (packageNameFromArtifact(message) !== expectation.packageName) return false;
    return fs.realpathSync(message.manifest_path) === expectedManifest;
  });
}

export function buildCargoArtifactManifest({
  exactSha,
  exactTree,
  expectations,
  jsonLines,
  rustTarget,
  targetDir,
  workspaceRoot,
}) {
  requireSha(exactSha, "source SHA");
  requireSha(exactTree, "source tree");
  requireWindowsTarget(rustTarget);
  if (!Array.isArray(expectations) || expectations.length === 0) {
    fail("at least one Cargo artifact expectation is required");
  }
  const parsedExpectations = expectations.map((value) =>
    typeof value === "string" ? parseExpectation(value) : value
  );
  requireArtifactContract(parsedExpectations);
  const aliases = new Set();
  const identities = new Set();
  for (const expectation of parsedExpectations) {
    if (!ALIAS.test(expectation.alias)) fail(`invalid artifact alias ${expectation.alias}`);
    if (aliases.has(expectation.alias)) fail(`duplicate artifact alias ${expectation.alias}`);
    aliases.add(expectation.alias);
    const identity =
      `${expectation.packageName}:${expectation.kind}:${expectation.targetName}`;
    if (identities.has(identity)) fail(`duplicate artifact expectation ${identity}`);
    identities.add(identity);
  }

  const resolvedTargetDir = path.resolve(targetDir);
  const resolvedWorkspaceRoot = fs.realpathSync(workspaceRoot);
  const profileRoot = path.join(resolvedTargetDir, rustTarget, "release");
  if (!fs.statSync(profileRoot).isDirectory()) {
    fail("exact target release directory is missing");
  }
  const messages = parseCargoMessages(jsonLines);
  const artifacts = {};
  for (const expectation of parsedExpectations) {
    const matches = matchingArtifacts(messages, expectation, resolvedWorkspaceRoot);
    if (matches.length !== 1) {
      fail(
        `expected exactly one Cargo artifact for ${expectation.alias}, found ${matches.length}`,
      );
    }
    const [message] = matches;
    exactKeys(
      message.target,
      [
        "kind",
        "crate_types",
        "name",
        "src_path",
        "edition",
        "doc",
        "doctest",
        "test",
      ],
      `Cargo artifact target ${expectation.targetName}`,
    );
    if (
      !Array.isArray(message.target.crate_types)
      || !message.target.crate_types.includes("bin")
      || message.target.test !== (expectation.kind === "test")
    ) {
      fail(`Cargo artifact target contract changed for ${expectation.alias}`);
    }
    const profile = releaseProfile(message, expectation.kind);
    artifacts[expectation.alias] = {
      package: expectation.packageName,
      target: expectation.targetName,
      kind: expectation.kind,
      profile,
      ...validatedExecutable(message, {
        kind: expectation.kind,
        profileRoot,
        targetName: expectation.targetName,
      }),
    };
  }

  return {
    schema: SCHEMA,
    source: {
      commit: exactSha,
      tree: exactTree,
    },
    build: {
      rust_target: rustTarget,
      profile: "release",
      target_dir: resolvedTargetDir,
      workspace_root: resolvedWorkspaceRoot,
    },
    artifacts,
  };
}

function validateManifestShape(manifest) {
  exactKeys(manifest, ["schema", "source", "build", "artifacts"], "artifact manifest");
  if (manifest.schema !== SCHEMA) fail("artifact manifest schema changed");
  exactKeys(manifest.source, ["commit", "tree"], "artifact manifest source");
  exactKeys(
    manifest.build,
    ["rust_target", "profile", "target_dir", "workspace_root"],
    "artifact manifest build",
  );
  if (manifest.build.profile !== "release") fail("artifact manifest profile is not release");
  requireWindowsTarget(manifest.build.rust_target);
  requireSha(manifest.source.commit, "manifest source SHA");
  requireSha(manifest.source.tree, "manifest source tree");
  if (
    manifest.artifacts === null
    || typeof manifest.artifacts !== "object"
    || Array.isArray(manifest.artifacts)
    || Object.keys(manifest.artifacts).length === 0
  ) {
    fail("artifact manifest must contain artifacts");
  }
  const aliases = Object.keys(manifest.artifacts).sort();
  const required = [...REQUIRED_ALIASES].sort();
  const withDriver = [...REQUIRED_ALIASES, "qualification_driver"].sort();
  if (
    JSON.stringify(aliases) !== JSON.stringify(required)
    && JSON.stringify(aliases) !== JSON.stringify(withDriver)
  ) {
    fail("artifact manifest release graph changed");
  }
}

export function verifyCargoArtifactManifest({
  exactSha,
  exactTree,
  manifest,
  rustTarget,
  workspaceRoot,
}) {
  validateManifestShape(manifest);
  if (
    manifest.source.commit !== exactSha
    || manifest.source.tree !== exactTree
  ) {
    fail("artifact manifest source identity does not match the exact checkout");
  }
  if (manifest.build.rust_target !== rustTarget) {
    fail("artifact manifest Rust target does not match");
  }
  if (
    fs.realpathSync(manifest.build.workspace_root) !== fs.realpathSync(workspaceRoot)
  ) {
    fail("artifact manifest workspace root does not match");
  }

  const profileRoot = path.join(
    path.resolve(manifest.build.target_dir),
    rustTarget,
    "release",
  );
  const realProfileRoot = fs.realpathSync(profileRoot);
  const verified = {};
  for (const [alias, artifact] of Object.entries(manifest.artifacts)) {
    if (!ALIAS.test(alias)) fail(`invalid artifact manifest alias ${alias}`);
    exactKeys(
      artifact,
      [
        "package",
        "target",
        "kind",
        "profile",
        "path",
        "relative_path",
        "bytes",
        "sha256",
      ],
      `artifact manifest entry ${alias}`,
    );
    if (
      typeof artifact.package !== "string"
      || artifact.package === ""
      || typeof artifact.target !== "string"
      || artifact.target === ""
      || !["bin", "test"].includes(artifact.kind)
      || !Number.isSafeInteger(artifact.bytes)
      || artifact.bytes < 0
      || !SHA256.test(artifact.sha256)
      || typeof artifact.relative_path !== "string"
      || artifact.relative_path === ""
    ) {
      fail(`artifact manifest entry ${alias} values changed`);
    }
    const contract = ARTIFACT_CONTRACT[alias];
    if (
      !contract
      || artifact.package !== contract.packageName
      || artifact.kind !== contract.kind
      || artifact.target !== contract.targetName
    ) {
      fail(`artifact manifest contract changed for ${alias}`);
    }
    releaseProfile({ profile: artifact.profile }, artifact.kind);
    const executable = path.resolve(artifact.path);
    const metadata = fs.lstatSync(executable);
    if (
      !metadata.isFile()
      || metadata.isSymbolicLink()
      || metadata.nlink !== 1
    ) {
      fail(
        `artifact ${alias} is no longer a regular, non-symlink, singly linked file`,
      );
    }
    const realExecutable = fs.realpathSync(executable);
    if (!isWithin(realProfileRoot, realExecutable)) {
      fail(`artifact ${alias} escaped the exact target release directory`);
    }
    const relative = path.relative(realProfileRoot, realExecutable)
      .split(path.sep)
      .join("/");
    if (
      (artifact.kind === "test" && path.posix.dirname(relative) !== "deps")
      || (artifact.kind === "bin" && path.posix.dirname(relative) !== ".")
      || path.extname(executable).toLowerCase() !== ".exe"
    ) {
      fail(`artifact ${alias} no longer has its expected release-graph path`);
    }
    if (
      relative !== artifact.relative_path
      || metadata.size !== artifact.bytes
      || hashFile(executable) !== artifact.sha256
    ) {
      fail(`artifact ${alias} no longer matches its authenticated build output`);
    }
    verified[alias] = executable;
  }
  return verified;
}

function parseArguments(argv) {
  const [command, ...rest] = argv;
  const values = new Map();
  for (let index = 0; index < rest.length; index += 1) {
    const flag = rest[index];
    if (!flag?.startsWith("--")) fail(`unexpected argument ${flag ?? "<end>"}`);
    const value = rest[index + 1];
    if (value === undefined || value.startsWith("--")) fail(`missing value for ${flag}`);
    if (!values.has(flag)) values.set(flag, []);
    values.get(flag).push(value);
    index += 1;
  }
  return { command, values };
}

function one(values, flag, { required = true } = {}) {
  const entries = values.get(flag) ?? [];
  if (entries.length > 1) fail(`${flag} may be supplied only once`);
  if (entries.length === 0) {
    if (required) fail(`missing ${flag}`);
    return "";
  }
  return entries[0];
}

function writeManifest(file, manifest) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
    mode: 0o600,
  });
}

function appendOutputs(file, manifestFile, artifacts) {
  const rows = [`manifest=${path.resolve(manifestFile)}`];
  for (const [alias, artifact] of Object.entries(artifacts)) {
    rows.push(`${alias}=${artifact.path ?? artifact}`);
  }
  fs.appendFileSync(file, `${rows.join("\n")}\n`, "utf8");
}

function runSelect(values) {
  const expectations = values.get("--expect") ?? [];
  const input = one(values, "--input");
  const output = one(values, "--out");
  const manifest = buildCargoArtifactManifest({
    exactSha: one(values, "--source-sha"),
    exactTree: one(values, "--source-tree"),
    expectations,
    jsonLines: fs.readFileSync(input, "utf8"),
    rustTarget: one(values, "--rust-target"),
    targetDir: one(values, "--target-dir"),
    workspaceRoot: one(values, "--workspace-root"),
  });
  writeManifest(output, manifest);
  const githubOutput = one(values, "--github-output", { required: false });
  if (githubOutput) appendOutputs(githubOutput, output, manifest.artifacts);
}

function runVerify(values) {
  const manifestFile = one(values, "--manifest");
  const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
  const artifacts = verifyCargoArtifactManifest({
    exactSha: one(values, "--source-sha"),
    exactTree: one(values, "--source-tree"),
    manifest,
    rustTarget: one(values, "--rust-target"),
    workspaceRoot: one(values, "--workspace-root"),
  });
  const githubOutput = one(values, "--github-output", { required: false });
  if (githubOutput) appendOutputs(githubOutput, manifestFile, artifacts);
}

function main(argv) {
  const { command, values } = parseArguments(argv);
  if (command === "select") {
    runSelect(values);
  } else if (command === "verify") {
    runVerify(values);
  } else {
    fail(`unsupported command ${command ?? "<missing>"}`);
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
