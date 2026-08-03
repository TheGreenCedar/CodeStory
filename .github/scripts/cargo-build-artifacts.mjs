#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SCHEMA = "codestory.cargo-build-artifacts/v2";
const SHA40 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const DECIMAL_BIGINT = /^(?:0|[1-9][0-9]*)$/u;
const ALIAS = /^[a-z][a-z0-9_]*$/u;
const WINDOWS_TARGET = "x86_64-pc-windows-msvc";
const RELEASE_OPT_LEVELS = new Set(["1", "2", "3", "s", "z"]);
const SHIPPING_BINARIES = [
  {
    packageName: "codestory-cli",
    targetName: "codestory-cli",
  },
  {
    packageName: "codestory-cli",
    targetName: "codestory-cli-runtime",
  },
];
const FORBIDDEN_SHIPPING_FEATURES = new Map([
  ["codestory-retrieval", new Set(["benchmark-support", "test-support"])],
  ["codestory-runtime", new Set(["benchmark-support", "test-support"])],
]);
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
  qualification_driver: {
    packageName: "codestory-bench",
    kind: "bin",
    targetName: "codestory_embedding_qualification",
  },
};
const REQUIRED_ALIASES = ["cli", "runtime"];

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
  if (packageId.startsWith("path+file:")) {
    const source = packageId.slice("path+".length).split("#", 1)[0];
    try {
      const directory = path.posix.basename(new URL(source).pathname);
      if (directory !== "") return decodeURIComponent(directory);
    } catch {
      // Fall through to the manifest-backed parser below.
    }
  }
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
  return parseCargoMessageStream(jsonLines).compilerArtifacts;
}

function parseCargoMessageStream(jsonLines) {
  const compilerArtifacts = [];
  const buildFinished = [];
  for (const [index, line] of jsonLines.split(/\r?\n/u).entries()) {
    if (line.trim() === "") continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      fail(`Cargo JSON output line ${index + 1} is not valid JSON`);
    }
    if (message?.reason === "compiler-artifact") compilerArtifacts.push(message);
    if (message?.reason === "build-finished") buildFinished.push(message);
  }
  return { compilerArtifacts, buildFinished };
}

export function assertShippingFeatureContract({
  jsonLines,
  workspaceRoot,
}) {
  const resolvedWorkspaceRoot = fs.realpathSync(workspaceRoot);
  if (!fs.statSync(resolvedWorkspaceRoot).isDirectory()) {
    fail("shipping Cargo workspace root is not a directory");
  }
  const { compilerArtifacts, buildFinished } = parseCargoMessageStream(jsonLines);
  if (
    buildFinished.length !== 1
    || buildFinished[0]?.success !== true
  ) {
    fail("shipping Cargo message stream did not finish successfully");
  }

  const artifacts = compilerArtifacts.map((message) => {
    const packageName = packageNameFromArtifact(message);
    const targetName = message?.target?.name;
    const targetKinds = message?.target?.kind;
    if (
      typeof targetName !== "string"
      || targetName === ""
      || !Array.isArray(targetKinds)
      || targetKinds.some((kind) => typeof kind !== "string")
      || message?.profile === null
      || typeof message?.profile !== "object"
    ) {
      fail(`Cargo compiler artifact shape changed for ${packageName}`);
    }
    if (
      message.profile.test === true
      || targetKinds.includes("test")
      || targetKinds.includes("bench")
    ) {
      fail(
        `shipping Cargo graph emitted a test or benchmark target: `
          + `${packageName}:${targetName}`,
      );
    }
    return {
      message,
      packageName,
      targetKinds,
      targetName,
    };
  });

  for (const expected of SHIPPING_BINARIES) {
    const matches = artifacts.filter(({ message, packageName, targetKinds, targetName }) =>
      packageName === expected.packageName
      && targetName === expected.targetName
      && JSON.stringify(targetKinds) === JSON.stringify(["bin"])
      && message.profile.test === false
    );
    if (matches.length !== 1) {
      fail(
        `shipping Cargo graph must emit exactly one production binary `
          + `${expected.packageName}:${expected.targetName}; found ${matches.length}`,
      );
    }
  }

  for (const [packageName, forbidden] of FORBIDDEN_SHIPPING_FEATURES) {
    const matches = artifacts.filter((artifact) =>
      artifact.packageName === packageName
    );
    if (matches.length === 0) {
      fail(`shipping Cargo graph omitted feature evidence for ${packageName}`);
    }
    for (const { message } of matches) {
      if (
        !Array.isArray(message.features)
        || message.features.some((feature) => typeof feature !== "string")
      ) {
        fail(`shipping Cargo feature evidence changed for ${packageName}`);
      }
      for (const feature of message.features) {
        if (forbidden.has(feature)) {
          fail(
            `shipping Cargo graph enabled forbidden feature `
              + `${packageName}/${feature}`,
          );
        }
      }
    }
  }
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

function nativeFileMetadata(file, label) {
  const metadata = fs.lstatSync(file, { bigint: true });
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    fail(`${label} must be a regular, non-symlink file`);
  }
  if (
    metadata.dev < 0n
    || metadata.ino <= 0n
    || metadata.nlink <= 0n
    || metadata.size < 0n
    || metadata.size > BigInt(Number.MAX_SAFE_INTEGER)
  ) {
    fail(`${label} has unusable native filesystem metadata`);
  }
  return metadata;
}

function nativeIdentity(metadata) {
  return {
    device: metadata.dev.toString(),
    inode: metadata.ino.toString(),
  };
}

function sameNativeIdentity(left, right) {
  return left.dev === right.dev && left.ino === right.ino;
}

function sameNativeSnapshot(left, right) {
  return (
    sameNativeIdentity(left, right)
    && left.nlink === right.nlink
    && left.size === right.size
  );
}

function profileRelativePath(realProfileRoot, candidate, label) {
  const realCandidate = fs.realpathSync(candidate);
  if (!isWithin(realProfileRoot, realCandidate)) {
    fail(`${label} escaped the exact target release directory`);
  }
  return path.relative(realProfileRoot, realCandidate)
    .split(path.sep)
    .join("/");
}

function inspectNativeLinks({
  executable,
  kind,
  realProfileRoot,
  targetName,
}) {
  const label = `Cargo artifact ${targetName}`;
  const before = nativeFileMetadata(executable, label);
  if (before.nlink > BigInt(Number.MAX_SAFE_INTEGER)) {
    fail(`${label} has an unusable native hardlink count`);
  }
  const linkCount = Number(before.nlink);
  const selectedRelative = profileRelativePath(
    realProfileRoot,
    executable,
    label,
  );
  const paths = [selectedRelative];

  if (kind === "test") {
    if (linkCount !== 1) {
      fail(`${label} test executable has native aliases outside its Cargo output path`);
    }
  } else if (linkCount === 2) {
    const depsRoot = path.join(realProfileRoot, "deps");
    const depsMetadata = fs.lstatSync(depsRoot);
    if (!depsMetadata.isDirectory() || depsMetadata.isSymbolicLink()) {
      fail(`${label} hardlink peer directory is missing`);
    }
    const peer = path.join(
      depsRoot,
      `${targetName.replaceAll("-", "_")}.exe`,
    );
    if (!fs.existsSync(peer)) {
      fail(
        `${label} hardlinks are not exactly the release-root executable and one release/deps peer`,
      );
    }
    const peerMetadata = nativeFileMetadata(peer, `${label} hardlink peer`);
    if (!sameNativeIdentity(before, peerMetadata)) {
      fail(
        `${label} hardlinks are not exactly the release-root executable and one release/deps peer`,
      );
    }
    paths.push(profileRelativePath(realProfileRoot, peer, label));
  } else if (linkCount !== 1) {
    fail(`${label} has an unsupported native hardlink count ${linkCount}`);
  }

  paths.sort();
  if (paths.length !== linkCount || new Set(paths).size !== paths.length) {
    fail(`${label} native hardlink accounting is incomplete`);
  }
  const after = nativeFileMetadata(executable, label);
  if (!sameNativeSnapshot(before, after)) {
    fail(`${label} native identity changed while its hardlinks were inspected`);
  }
  return {
    metadata: after,
    nativeLinks: {
      ...nativeIdentity(after),
      count: linkCount,
      paths,
    },
  };
}

function requireStableNativeInspection({
  executable,
  expectedMetadata,
  expectedNativeLinks,
  kind,
  label,
  realProfileRoot,
  targetName,
}) {
  const current = inspectNativeLinks({
    executable,
    kind,
    realProfileRoot,
    targetName,
  });
  if (
    !sameNativeSnapshot(expectedMetadata, current.metadata)
    || JSON.stringify(expectedNativeLinks) !== JSON.stringify(current.nativeLinks)
  ) {
    fail(`${label} native identity changed while its contents were authenticated`);
  }
}

function validatedExecutable(message, {
  kind,
  profileRoot,
  targetName,
}) {
  if (message.fresh !== false) {
    fail(`Cargo artifact ${targetName} was not produced by the exact build invocation`);
  }
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
  if (path.extname(executable).toLowerCase() !== ".exe") {
    fail(`Cargo artifact ${targetName} is not a Windows executable`);
  }

  const realProfileRoot = fs.realpathSync(profileRoot);
  const relative = profileRelativePath(
    realProfileRoot,
    executable,
    `Cargo artifact ${targetName}`,
  );
  if (kind === "test" && path.posix.dirname(relative) !== "deps") {
    fail(`Cargo test artifact ${targetName} was not emitted under release/deps`);
  }
  if (
    kind === "bin"
    && (
      path.posix.dirname(relative) !== "."
      || path.posix.basename(relative) !== `${targetName}.exe`
    )
  ) {
    fail(`Cargo binary artifact ${targetName} was not emitted at the release root`);
  }
  const { metadata, nativeLinks } = inspectNativeLinks({
    executable,
    kind,
    realProfileRoot,
    targetName,
  });
  const sha256 = hashFile(executable);
  requireStableNativeInspection({
    executable,
    expectedMetadata: metadata,
    expectedNativeLinks: nativeLinks,
    kind,
    label: `Cargo artifact ${targetName}`,
    realProfileRoot,
    targetName,
  });

  return {
    path: executable,
    relative_path: relative,
    bytes: Number(metadata.size),
    sha256,
    native_links: nativeLinks,
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
  assertShippingFeatureContract({ jsonLines, workspaceRoot });
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
      || JSON.stringify(message.target.kind) !== JSON.stringify([expectation.kind])
      || JSON.stringify(message.target.crate_types) !== JSON.stringify(["bin"])
      || typeof message.target.test !== "boolean"
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

function validateNativeLinksShape(nativeLinks, artifact, alias) {
  exactKeys(
    nativeLinks,
    ["device", "inode", "count", "paths"],
    `artifact manifest native links ${alias}`,
  );
  if (
    typeof nativeLinks.device !== "string"
    || !DECIMAL_BIGINT.test(nativeLinks.device)
    || typeof nativeLinks.inode !== "string"
    || !DECIMAL_BIGINT.test(nativeLinks.inode)
    || BigInt(nativeLinks.inode) <= 0n
    || !Number.isSafeInteger(nativeLinks.count)
    || ![1, 2].includes(nativeLinks.count)
    || !Array.isArray(nativeLinks.paths)
    || nativeLinks.paths.length !== nativeLinks.count
    || new Set(nativeLinks.paths).size !== nativeLinks.paths.length
    || nativeLinks.paths.some(
      (entry) =>
        typeof entry !== "string"
        || entry === ""
        || entry.includes("\\")
        || path.posix.isAbsolute(entry)
        || entry === ".."
        || entry.startsWith("../"),
    )
    || JSON.stringify(nativeLinks.paths) !== JSON.stringify([...nativeLinks.paths].sort())
    || !nativeLinks.paths.includes(artifact.relative_path)
  ) {
    fail(`artifact manifest native links ${alias} values changed`);
  }
  const peerPaths = nativeLinks.paths.filter(
    (entry) => entry !== artifact.relative_path,
  );
  if (
    (artifact.kind === "test" && nativeLinks.count !== 1)
    || (
      artifact.kind === "bin"
      && nativeLinks.count === 2
      && (
        peerPaths.length !== 1
        || path.posix.dirname(peerPaths[0]) !== "deps"
        || path.posix.basename(peerPaths[0])
          !== `${artifact.target.replaceAll("-", "_")}.exe`
      )
    )
    || (artifact.kind === "bin" && nativeLinks.count === 1 && peerPaths.length !== 0)
  ) {
    fail(`artifact manifest native link topology changed for ${alias}`);
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
        "native_links",
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
    validateNativeLinksShape(artifact.native_links, artifact, alias);
    const executable = path.resolve(artifact.path);
    const realExecutable = fs.realpathSync(executable);
    if (!isWithin(realProfileRoot, realExecutable)) {
      fail(`artifact ${alias} escaped the exact target release directory`);
    }
    const relative = path.relative(realProfileRoot, realExecutable)
      .split(path.sep)
      .join("/");
    if (
      (artifact.kind === "test" && path.posix.dirname(relative) !== "deps")
      || (
        artifact.kind === "bin"
        && (
          path.posix.dirname(relative) !== "."
          || path.posix.basename(relative) !== `${artifact.target}.exe`
        )
      )
      || path.extname(executable).toLowerCase() !== ".exe"
    ) {
      fail(`artifact ${alias} no longer has its expected release-graph path`);
    }
    const { metadata, nativeLinks } = inspectNativeLinks({
      executable,
      kind: artifact.kind,
      realProfileRoot,
      targetName: artifact.target,
    });
    if (
      JSON.stringify(nativeLinks) !== JSON.stringify(artifact.native_links)
    ) {
      fail(`artifact ${alias} no longer matches its authenticated native links`);
    }
    const sha256 = hashFile(executable);
    requireStableNativeInspection({
      executable,
      expectedMetadata: metadata,
      expectedNativeLinks: nativeLinks,
      kind: artifact.kind,
      label: `artifact ${alias}`,
      realProfileRoot,
      targetName: artifact.target,
    });
    if (
      relative !== artifact.relative_path
      || Number(metadata.size) !== artifact.bytes
      || sha256 !== artifact.sha256
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

function runFeatures(values) {
  assertShippingFeatureContract({
    jsonLines: fs.readFileSync(one(values, "--input"), "utf8"),
    workspaceRoot: one(values, "--workspace-root"),
  });
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
  if (command === "features") {
    runFeatures(values);
  } else if (command === "select") {
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
