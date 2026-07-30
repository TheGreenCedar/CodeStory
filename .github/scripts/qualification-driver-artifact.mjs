#!/usr/bin/env node

import {
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HEX_SHA = /^[0-9a-f]{40}$/u;
const HEX_SHA256 = /^[0-9a-f]{64}$/u;
const VERSION = /^[0-9]+\.[0-9]+\.[0-9]+$/u;
const TARGETS = new Map([
  ["linux-x64", {
    archiveExtension: "tar.gz",
    binary: "codestory_embedding_qualification",
    rustTarget: "x86_64-unknown-linux-gnu",
  }],
  ["macos-arm64", {
    archiveExtension: "tar.gz",
    binary: "codestory_embedding_qualification",
    rustTarget: "aarch64-apple-darwin",
  }],
  ["windows-x64", {
    archiveExtension: "zip",
    binary: "codestory_embedding_qualification.exe",
    rustTarget: "x86_64-pc-windows-msvc",
  }],
]);

function fail(message) {
  throw new Error(message);
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const values = new Map();
  for (let index = 0; index < rest.length; index += 2) {
    const flag = rest[index];
    const value = rest[index + 1];
    if (!flag?.startsWith("--") || value === undefined) {
      fail(`invalid argument near ${flag ?? "<end>"}`);
    }
    if (values.has(flag)) {
      fail(`duplicate argument ${flag}`);
    }
    values.set(flag, value);
  }
  return { command, values };
}

function required(values, flag) {
  const value = values.get(flag);
  if (!value) {
    fail(`missing ${flag}`);
  }
  return value;
}

function requireExactFlags(values, expected) {
  const actual = [...values.keys()].sort();
  const allowed = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(allowed)) {
    fail("qualification driver helper arguments changed");
  }
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

function targetContract(assetTarget) {
  const contract = TARGETS.get(assetTarget);
  if (!contract) {
    fail(`unsupported asset target ${assetTarget}`);
  }
  return contract;
}

function requireSha(value, label) {
  if (!HEX_SHA.test(value)) {
    fail(`${label} must be a full lowercase commit digest`);
  }
}

function requireVersion(value) {
  if (!VERSION.test(value)) {
    fail("release version must be plain semver");
  }
}

function regularFile(file, label) {
  const metadata = lstatSync(file);
  if (
    metadata.isSymbolicLink()
    || !metadata.isFile()
    || metadata.nlink !== 1
  ) {
    fail(`${label} must be a regular, non-symlink, singly linked file`);
  }
  return metadata;
}

function regularBuildOutput(file, label) {
  const metadata = lstatSync(file);
  if (
    metadata.isSymbolicLink()
    || !metadata.isFile()
    || !Number.isSafeInteger(metadata.nlink)
    || metadata.nlink < 1
  ) {
    fail(`${label} must be a regular, non-symlink build output`);
  }
  return metadata;
}

function regularDirectory(directory, label) {
  const metadata = lstatSync(directory);
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    fail(`${label} must be a real non-symlink directory`);
  }
  return metadata;
}

function containedRelativePath(root, candidate, label) {
  const relative = path.relative(root, candidate);
  if (
    relative === ""
    || relative === ".."
    || relative.startsWith(`..${path.sep}`)
    || path.isAbsolute(relative)
  ) {
    fail(`${label} must be a descendant of its trusted root`);
  }
  return relative;
}

function rejectSymlinkedPath({
  allowMissing,
  candidate,
  label,
  root,
}) {
  const resolvedRoot = path.resolve(root);
  const resolvedCandidate = path.resolve(candidate);
  const relative = containedRelativePath(resolvedRoot, resolvedCandidate, label);
  const rootMetadata = lstatSync(resolvedRoot);
  if (
    !rootMetadata.isDirectory()
    || rootMetadata.isSymbolicLink()
  ) {
    fail(`${label} trusted root must be a real directory`);
  }

  let cursor = resolvedRoot;
  for (const component of relative.split(path.sep)) {
    cursor = path.join(cursor, component);
    if (!existsSync(cursor)) {
      if (allowMissing) return;
      fail(`${label} path component is missing`);
    }
    if (lstatSync(cursor).isSymbolicLink()) {
      fail(`${label} must not traverse symbolic links`);
    }
  }
}

function sha256(file) {
  const hash = createHash("sha256");
  const handle = openSync(file, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const bytesRead = readSync(handle, buffer, 0, buffer.length, null);
      if (bytesRead === 0) break;
      hash.update(buffer.subarray(0, bytesRead));
    }
  } finally {
    closeSync(handle);
  }
  return hash.digest("hex");
}

function canonicalIdentity({
  archiveBytes,
  archiveDigest,
  archiveFile,
  assetTarget,
  binary,
  bytes,
  digest,
  sourceSha,
  sourceTree,
  version,
}) {
  return {
    schema_version: 1,
    source: {
      commit: sourceSha,
      tree: sourceTree,
    },
    release_version: version,
    asset_target: assetTarget,
    archive: {
      file: archiveFile,
      bytes: archiveBytes,
      sha256: archiveDigest,
    },
    driver: {
      file: binary,
      bytes,
      sha256: digest,
    },
  };
}

function writeJsonAtomically(output, value) {
  const temporary = `${output}.tmp-${process.pid}`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
    mode: 0o600,
  });
  renameSync(temporary, output);
}

export function produceQualificationDriverArtifact({
  archive,
  assetTarget,
  outDir,
  sourceSha,
  sourceTree,
  targetDir = process.env.CARGO_TARGET_DIR || "target",
  trustedRoot = process.cwd(),
  version,
}) {
  const contract = targetContract(assetTarget);
  requireSha(sourceSha, "source SHA");
  requireSha(sourceTree, "source tree");
  requireVersion(version);

  const expectedArchiveFile =
    `codestory-cli-v${version}-${assetTarget}.${contract.archiveExtension}`;
  const archivePath = path.resolve(archive);
  rejectSymlinkedPath({
    allowMissing: false,
    candidate: archivePath,
    label: "candidate archive",
    root: trustedRoot,
  });
  const archiveMetadata = regularFile(archivePath, "candidate archive");
  if (path.basename(archivePath) !== expectedArchiveFile) {
    fail("candidate archive name does not match the release target");
  }

  const source = path.resolve(
    targetDir,
    contract.rustTarget,
    "release",
    contract.binary,
  );
  rejectSymlinkedPath({
    allowMissing: false,
    candidate: source,
    label: "qualification driver source",
    root: targetDir,
  });
  const sourceMetadata = regularBuildOutput(
    source,
    "qualification driver source",
  );
  if (process.platform !== "win32" && (sourceMetadata.mode & 0o111) === 0) {
    fail("qualification driver must be executable");
  }

  const outputDirectory = path.resolve(outDir);
  rejectSymlinkedPath({
    allowMissing: true,
    candidate: outputDirectory,
    label: "qualification driver artifact directory",
    root: trustedRoot,
  });
  mkdirSync(outputDirectory, { recursive: true, mode: 0o700 });
  rejectSymlinkedPath({
    allowMissing: false,
    candidate: outputDirectory,
    label: "qualification driver artifact directory",
    root: trustedRoot,
  });
  regularDirectory(outputDirectory, "qualification driver artifact directory");
  if (readdirSync(outputDirectory).length !== 0) {
    fail("qualification driver artifact directory must start empty");
  }
  const staged = path.join(outputDirectory, contract.binary);
  const identityPath = path.join(
    outputDirectory,
    "qualification-driver-identity.json",
  );
  if (path.resolve(source) === path.resolve(staged)) {
    fail("qualification driver source and artifact paths must differ");
  }
  copyFileSync(source, staged);
  if (process.platform !== "win32") {
    chmodSync(staged, 0o755);
  }
  const stagedMetadata = regularFile(staged, "staged qualification driver");
  const digest = sha256(staged);
  const identity = canonicalIdentity({
    archiveBytes: archiveMetadata.size,
    archiveDigest: sha256(archivePath),
    archiveFile: expectedArchiveFile,
    assetTarget,
    binary: contract.binary,
    bytes: stagedMetadata.size,
    digest,
    sourceSha,
    sourceTree,
    version,
  });
  writeJsonAtomically(identityPath, identity);
  return { driver: staged, identity, identityPath };
}

export function verifyQualificationDriverArtifact({
  archive,
  artifactDir,
  assetTarget,
  sourceSha,
  sourceTree,
  trustedRoot = process.cwd(),
  version,
}) {
  const contract = targetContract(assetTarget);
  requireSha(sourceSha, "expected source SHA");
  requireSha(sourceTree, "expected source tree");
  requireVersion(version);

  const expectedArchiveFile =
    `codestory-cli-v${version}-${assetTarget}.${contract.archiveExtension}`;
  const archivePath = path.resolve(archive);
  rejectSymlinkedPath({
    allowMissing: false,
    candidate: archivePath,
    label: "candidate archive",
    root: trustedRoot,
  });
  const archiveMetadata = regularFile(archivePath, "candidate archive");
  if (path.basename(archivePath) !== expectedArchiveFile) {
    fail("candidate archive name does not match the release target");
  }

  const directory = path.resolve(artifactDir);
  rejectSymlinkedPath({
    allowMissing: false,
    candidate: directory,
    label: "qualification driver artifact directory",
    root: trustedRoot,
  });
  regularDirectory(directory, "qualification driver artifact directory");
  const identityPath = path.join(
    directory,
    "qualification-driver-identity.json",
  );
  regularFile(identityPath, "qualification driver identity");
  const identity = JSON.parse(readFileSync(identityPath, "utf8"));
  exactKeys(
    identity,
    [
      "schema_version",
      "source",
      "release_version",
      "asset_target",
      "archive",
      "driver",
    ],
    "qualification driver identity",
  );
  exactKeys(
    identity.source,
    ["commit", "tree"],
    "qualification driver source",
  );
  exactKeys(
    identity.archive,
    ["file", "bytes", "sha256"],
    "qualification driver archive identity",
  );
  exactKeys(
    identity.driver,
    ["file", "bytes", "sha256"],
    "qualification driver file identity",
  );
  if (
    identity.schema_version !== 1
    || identity.source.commit !== sourceSha
    || identity.source.tree !== sourceTree
    || identity.release_version !== version
    || identity.asset_target !== assetTarget
    || identity.archive.file !== expectedArchiveFile
    || !Number.isSafeInteger(identity.archive.bytes)
    || identity.archive.bytes <= 0
    || !HEX_SHA256.test(identity.archive.sha256)
    || identity.driver.file !== contract.binary
    || !Number.isSafeInteger(identity.driver.bytes)
    || identity.driver.bytes <= 0
    || !HEX_SHA256.test(identity.driver.sha256)
  ) {
    fail("qualification driver identity does not match the expected candidate");
  }
  if (
    archiveMetadata.size !== identity.archive.bytes
    || sha256(archivePath) !== identity.archive.sha256
  ) {
    fail("candidate archive digest changed");
  }

  const driver = path.join(directory, identity.driver.file);
  const metadata = regularFile(driver, "qualification driver artifact");
  if (
    metadata.size !== identity.driver.bytes
    || sha256(driver) !== identity.driver.sha256
  ) {
    fail("qualification driver artifact digest changed");
  }
  const expectedDirectoryEntries = [
    "qualification-driver-identity.json",
    contract.binary,
  ].sort();
  if (
    JSON.stringify(readdirSync(directory).sort())
      !== JSON.stringify(expectedDirectoryEntries)
  ) {
    fail("qualification driver artifact directory contains unexpected files");
  }
  // GitHub artifact extraction does not preserve Unix execute bits. Restore
  // execution only after the downloaded regular file has matched the retained
  // source/tree/target identity and byte digest.
  if (process.platform !== "win32") {
    chmodSync(driver, 0o755);
  }
  return { driver, identity, identityPath };
}

function usage() {
  return [
    "Usage:",
    "  qualification-driver-artifact.mjs produce --asset-target TARGET --source-sha SHA --source-tree TREE --version VERSION --archive FILE --trusted-root DIR --target-dir DIR --out-dir DIR",
    "  qualification-driver-artifact.mjs verify --asset-target TARGET --source-sha SHA --source-tree TREE --version VERSION --archive FILE --trusted-root DIR --artifact-dir DIR",
  ].join("\n");
}

function main(argv) {
  const { command, values } = parseArgs(argv);
  const commonFlags = [
    "--archive",
    "--asset-target",
    "--source-sha",
    "--source-tree",
    "--trusted-root",
    "--version",
  ];
  if (command === "produce") {
    requireExactFlags(values, [...commonFlags, "--out-dir", "--target-dir"]);
  } else if (command === "verify") {
    requireExactFlags(values, [...commonFlags, "--artifact-dir"]);
  } else {
    fail(usage());
  }
  const common = {
    assetTarget: required(values, "--asset-target"),
    archive: required(values, "--archive"),
    sourceSha: required(values, "--source-sha"),
    sourceTree: required(values, "--source-tree"),
    trustedRoot: required(values, "--trusted-root"),
    version: required(values, "--version").replace(/^v/u, ""),
  };
  let result;
  if (command === "produce") {
    result = produceQualificationDriverArtifact({
      ...common,
      outDir: required(values, "--out-dir"),
      targetDir: required(values, "--target-dir"),
    });
  } else if (command === "verify") {
    result = verifyQualificationDriverArtifact({
      ...common,
      artifactDir: required(values, "--artifact-dir"),
    });
  }
  process.stdout.write(`${JSON.stringify({
    driver: result.driver,
    sha256: result.identity.driver.sha256,
  })}\n`);
}

const isMain = process.argv[1]
  && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMain) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
