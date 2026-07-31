#!/usr/bin/env node

import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  writeSync,
} from "node:fs";
import { createHash, randomBytes } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

const RECORD_SCHEMA = "codestory-candidate-archive-store/v1";
const SHA = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u;
const TARGET = /^[a-z0-9][a-z0-9._-]{0,63}$/u;
const COMPANION_ROLES = new Set([
  "archive_checksum",
  "checksum_manifest",
]);
const RECORD_FILE = "candidate-archive-record.json";
const PAYLOAD_DIRECTORY = "payload";
const BUFFER_BYTES = 1024 * 1024;
const O_NOFOLLOW = constants.O_NOFOLLOW ?? 0;
const PORTABLE_COMPONENT = /^[A-Za-z0-9](?:[A-Za-z0-9._+-]*[A-Za-z0-9])?$/u;
const WINDOWS_RESERVED_COMPONENT =
  /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$/iu;

// GitHub producer/run authentication stays in the workflow. This helper owns
// the inner byte boundary after that authentication: an exact record selects
// <source SHA>/<target>/<archive SHA-256>, and every use revalidates the full
// allowlisted payload before copying it out of the protected-host store.

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

function requireString(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    fail(`${label} must be a non-empty string`);
  }
  return value;
}

function requireDigest(value, pattern, label) {
  requireString(value, label);
  if (!pattern.test(value)) {
    fail(`${label} has an invalid digest`);
  }
  return value;
}

function requireBytes(value, label) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    fail(`${label} must be a positive safe integer`);
  }
  return value;
}

function simpleName(value, label) {
  requireString(value, label);
  if (
    value === "."
    || value === ".."
    || value.includes("/")
    || value.includes("\\")
    || value.includes("\0")
    || path.basename(value) !== value
    || !PORTABLE_COMPONENT.test(value)
    || WINDOWS_RESERVED_COMPONENT.test(value)
  ) {
    fail(`${label} must be a portable simple filename`);
  }
  return value;
}

function relativePayloadPath(value, label) {
  requireString(value, label);
  if (
    value.includes("\\")
    || value.includes("\0")
    || path.posix.isAbsolute(value)
    || path.win32.isAbsolute(value)
    || path.posix.normalize(value) !== value
    || value === "."
    || value === ".."
    || value.startsWith("../")
    || value.split("/").some((component) => component === "" || component === "." || component === "..")
  ) {
    fail(`${label} must be a normalized relative POSIX path`);
  }
  for (const component of value.split("/")) {
    if (
      !PORTABLE_COMPONENT.test(component)
      || WINDOWS_RESERVED_COMPONENT.test(component)
    ) {
      fail(`${label} must contain only portable path components`);
    }
  }
  return value;
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function pathIdentity(value) {
  const resolved = path.resolve(value);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function requireRealRoot(root, label) {
  const resolved = path.resolve(root);
  const metadata = lstatSync(resolved);
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    fail(`${label} must be a real directory`);
  }
  const canonical = realpathSync.native(resolved);
  if (pathIdentity(canonical) !== pathIdentity(resolved)) {
    fail(`${label} must not have symbolic-link or reparse ancestry`);
  }
  return resolved;
}

function containedRelativePath(root, candidate, label, { allowEqual = false } = {}) {
  const relative = path.relative(root, candidate);
  if (
    (!allowEqual && relative === "")
    || relative === ".."
    || relative.startsWith(`..${path.sep}`)
    || path.isAbsolute(relative)
  ) {
    fail(`${label} must be a descendant of its trusted root`);
  }
  return relative;
}

function inspectPathAncestry({
  allowMissing,
  candidate,
  label,
  root,
}) {
  const resolvedRoot = requireRealRoot(root, `${label} trusted root`);
  const resolvedCandidate = path.resolve(candidate);
  const relative = containedRelativePath(resolvedRoot, resolvedCandidate, label);
  let cursor = resolvedRoot;
  const components = relative.split(path.sep);
  for (let index = 0; index < components.length; index += 1) {
    cursor = path.join(cursor, components[index]);
    if (!existsSync(cursor)) {
      if (allowMissing) return { resolvedCandidate, resolvedRoot };
      fail(`${label} path component is missing`);
    }
    const metadata = lstatSync(cursor);
    if (metadata.isSymbolicLink()) {
      fail(`${label} must not traverse symbolic links or reparse points`);
    }
    if (index < components.length - 1 && !metadata.isDirectory()) {
      fail(`${label} ancestor must be a directory`);
    }
  }
  return { resolvedCandidate, resolvedRoot };
}

function safeRegularFile(file, label, { requireSingleLink }) {
  const metadata = lstatSync(file, { bigint: true });
  if (
    metadata.isSymbolicLink()
    || !metadata.isFile()
    || (requireSingleLink && metadata.nlink !== 1n)
  ) {
    fail(
      requireSingleLink
        ? `${label} must be a regular, non-symlink, singly linked file`
        : `${label} must be a regular non-symlink file`,
    );
  }
  const handle = openSync(file, constants.O_RDONLY | O_NOFOLLOW);
  const opened = fstatSync(handle, { bigint: true });
  if (
    !opened.isFile()
    || opened.dev !== metadata.dev
    || opened.ino !== metadata.ino
    || opened.size !== metadata.size
    || (requireSingleLink && opened.nlink !== 1n)
  ) {
    closeSync(handle);
    fail(`${label} changed while it was opened`);
  }
  if (opened.size > BigInt(Number.MAX_SAFE_INTEGER)) {
    closeSync(handle);
    fail(`${label} is too large`);
  }
  return {
    handle,
    size: Number(opened.size),
  };
}

function digestHandle(handle) {
  const digest = createHash("sha256");
  const buffer = Buffer.allocUnsafe(BUFFER_BYTES);
  let position = 0;
  for (;;) {
    const bytesRead = readSync(handle, buffer, 0, buffer.length, position);
    if (bytesRead === 0) break;
    digest.update(buffer.subarray(0, bytesRead));
    position += bytesRead;
  }
  return digest.digest("hex");
}

function readHandle(handle, bytes, label) {
  const output = Buffer.allocUnsafe(bytes);
  let position = 0;
  while (position < bytes) {
    const count = readSync(
      handle,
      output,
      position,
      bytes - position,
      position,
    );
    if (count === 0) {
      fail(`${label} changed while it was read`);
    }
    position += count;
  }
  const sentinel = Buffer.allocUnsafe(1);
  if (readSync(handle, sentinel, 0, 1, position) !== 0) {
    fail(`${label} changed while it was read`);
  }
  return output;
}

function sha256File(file, label, { requireSingleLink = true } = {}) {
  const opened = safeRegularFile(file, label, { requireSingleLink });
  try {
    return {
      bytes: opened.size,
      sha256: digestHandle(opened.handle),
    };
  } finally {
    closeSync(opened.handle);
  }
}

function writeAll(handle, buffer, position) {
  let written = 0;
  while (written < buffer.length) {
    const count = writeSync(
      handle,
      buffer,
      written,
      buffer.length - written,
      position + written,
    );
    if (count <= 0) fail("short write while materializing candidate archive payload");
    written += count;
  }
}

function copyVerifiedFile({
  destination,
  expected,
  label,
  source,
  sourceRequiresSingleLink,
}) {
  const opened = safeRegularFile(source, `${label} source`, {
    requireSingleLink: sourceRequiresSingleLink,
  });
  let output;
  try {
    output = openSync(
      destination,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
      0o600,
    );
    const digest = createHash("sha256");
    const buffer = Buffer.allocUnsafe(BUFFER_BYTES);
    let position = 0;
    for (;;) {
      const bytesRead = readSync(opened.handle, buffer, 0, buffer.length, position);
      if (bytesRead === 0) break;
      const chunk = buffer.subarray(0, bytesRead);
      digest.update(chunk);
      writeAll(output, chunk, position);
      position += bytesRead;
    }
    fsyncSync(output);
    const actualDigest = digest.digest("hex");
    if (position !== expected.bytes || actualDigest !== expected.sha256) {
      fail(`${label} source does not match its expected size and SHA-256`);
    }
  } finally {
    closeSync(opened.handle);
    if (output !== undefined) closeSync(output);
  }
  const retained = sha256File(destination, `${label} destination`);
  if (retained.bytes !== expected.bytes || retained.sha256 !== expected.sha256) {
    fail(`${label} destination changed after materialization`);
  }
}

function companionPathContract(role, relativePath, archiveName) {
  if (role === "archive_checksum") {
    if (relativePath !== `${archiveName}.sha256`) {
      fail("archive checksum companion path does not match the archive name");
    }
    return;
  }
  if (role === "checksum_manifest") {
    if (relativePath !== "SHA256SUMS.txt") {
      fail("checksum manifest companion must be SHA256SUMS.txt");
    }
    return;
  }
  fail(`${role} is not a public candidate archive companion`);
}

function normalizeCompanions(companions, archiveName) {
  if (!Array.isArray(companions)) {
    fail("candidate archive companions must be an array");
  }
  const roles = new Set();
  const paths = new Set();
  const normalized = companions.map((companion, index) => {
    exactKeys(
      companion,
      ["role", "relative_path", "bytes", "sha256"],
      `candidate archive companion ${index}`,
    );
    const role = requireString(
      companion.role,
      `candidate archive companion ${index} role`,
    );
    if (!COMPANION_ROLES.has(role) || roles.has(role)) {
      fail("candidate archive companion roles must be unique and supported");
    }
    roles.add(role);
    const relativePath = relativePayloadPath(
      companion.relative_path,
      `candidate archive companion ${role} path`,
    );
    companionPathContract(role, relativePath, archiveName);
    const pathKey = relativePath.toLowerCase();
    if (paths.has(pathKey)) {
      fail("candidate archive payload paths must remain distinct on Windows");
    }
    paths.add(pathKey);
    return {
      role,
      relative_path: relativePath,
      bytes: requireBytes(
        companion.bytes,
        `candidate archive companion ${role} bytes`,
      ),
      sha256: requireDigest(
        companion.sha256,
        SHA256,
        `candidate archive companion ${role} SHA-256`,
      ),
    };
  });
  if (
    roles.size !== COMPANION_ROLES.size
    || [...COMPANION_ROLES].some((role) => !roles.has(role))
  ) {
    fail("candidate archive must retain exactly its two public checksum companions");
  }
  const archiveChecksum = normalized.find(
    (companion) => companion.role === "archive_checksum",
  );
  const checksumManifest = normalized.find(
    (companion) => companion.role === "checksum_manifest",
  );
  if (
    archiveChecksum.bytes !== checksumManifest.bytes
    || archiveChecksum.sha256 !== checksumManifest.sha256
  ) {
    fail("per-candidate checksum files must retain the same checksum line");
  }
  return normalized.sort((left, right) => left.role.localeCompare(right.role));
}

export function buildCandidateArchiveRecord({
  archive,
  companions = [],
  repository,
  sourceSha,
  sourceTree,
  target,
}) {
  if (!REPOSITORY.test(requireString(repository, "repository"))) {
    fail("repository must use the exact owner/name form");
  }
  requireDigest(sourceSha, SHA, "source SHA");
  requireDigest(sourceTree, SHA, "source tree");
  if (!TARGET.test(requireString(target, "target"))) {
    fail("target must use a stable lowercase target name");
  }
  exactKeys(
    archive,
    ["name", "relative_path", "bytes", "sha256"],
    "candidate archive",
  );
  const name = simpleName(archive.name, "candidate archive name");
  const relativePath = relativePayloadPath(
    archive.relative_path,
    "candidate archive relative path",
  );
  if (relativePath !== name) {
    fail("candidate archive must be at the payload root under its exact name");
  }
  const archivePathKey = relativePath.toLowerCase();
  const normalizedCompanions = normalizeCompanions(companions, name);
  if (
    normalizedCompanions.some(
      (companion) => companion.relative_path.toLowerCase() === archivePathKey,
    )
  ) {
    fail("candidate archive and companion paths must be distinct");
  }
  return {
    schema: RECORD_SCHEMA,
    repository,
    source: {
      commit: sourceSha,
      tree: sourceTree,
    },
    target,
    archive: {
      name,
      relative_path: relativePath,
      bytes: requireBytes(archive.bytes, "candidate archive bytes"),
      sha256: requireDigest(
        archive.sha256,
        SHA256,
        "candidate archive SHA-256",
      ),
    },
    companions: normalizedCompanions,
  };
}

export function validateCandidateArchiveRecord(record) {
  exactKeys(
    record,
    ["schema", "repository", "source", "target", "archive", "companions"],
    "candidate archive record",
  );
  if (record.schema !== RECORD_SCHEMA) {
    fail("candidate archive record schema changed");
  }
  exactKeys(record.source, ["commit", "tree"], "candidate archive source");
  const normalized = buildCandidateArchiveRecord({
    archive: record.archive,
    companions: record.companions,
    repository: record.repository,
    sourceSha: record.source.commit,
    sourceTree: record.source.tree,
    target: record.target,
  });
  if (canonicalJson(normalized) !== canonicalJson(record)) {
    fail("candidate archive record is not canonical");
  }
  return normalized;
}

export function candidateArchiveStoreKey(record) {
  const expected = validateCandidateArchiveRecord(record);
  return [
    expected.source.commit,
    expected.target,
    expected.archive.sha256,
  ].join("/");
}

function entryPaths(storeRoot, record) {
  const root = requireRealRoot(storeRoot, "candidate archive store root");
  const key = candidateArchiveStoreKey(record);
  const entry = path.join(root, "objects", "v1", ...key.split("/"));
  return {
    entry,
    key,
    parent: path.dirname(entry),
    payload: path.join(entry, PAYLOAD_DIRECTORY),
    recordFile: path.join(entry, RECORD_FILE),
    root,
  };
}

function requiredPayloads(record) {
  return [
    {
      role: "archive",
      relative_path: record.archive.relative_path,
      bytes: record.archive.bytes,
      sha256: record.archive.sha256,
    },
    ...record.companions,
  ];
}

function expectedDirectories(payloads) {
  const directories = new Set();
  for (const payload of payloads) {
    const components = payload.relative_path.split("/");
    components.pop();
    let cursor = "";
    for (const component of components) {
      cursor = cursor === "" ? component : `${cursor}/${component}`;
      directories.add(cursor);
    }
  }
  return directories;
}

function walkPayloadTree(root, label, { requireSingleLink }) {
  requireRealRoot(root, label);
  const files = new Map();
  const directories = new Set();
  const pending = [{ absolute: root, relative: "" }];
  while (pending.length > 0) {
    const current = pending.pop();
    for (const name of readdirSync(current.absolute)) {
      const absolute = path.join(current.absolute, name);
      const relative = current.relative === ""
        ? name
        : `${current.relative}/${name}`;
      relativePayloadPath(relative, `${label} entry`);
      const metadata = lstatSync(absolute);
      if (metadata.isSymbolicLink()) {
        fail(`${label} must not contain symbolic links or reparse points`);
      }
      if (metadata.isDirectory()) {
        directories.add(relative);
        pending.push({ absolute, relative });
      } else if (metadata.isFile()) {
        if (requireSingleLink && metadata.nlink !== 1) {
          fail(`${label} files must be singly linked`);
        }
        files.set(relative, absolute);
      } else {
        fail(`${label} must contain only regular files and directories`);
      }
    }
  }
  return { directories, files };
}

function verifyPayloadTree(root, record, label, { requireSingleLink = true } = {}) {
  const payloads = requiredPayloads(record);
  const expectedFiles = new Set(payloads.map((payload) => payload.relative_path));
  const expectedDirs = expectedDirectories(payloads);
  const actual = walkPayloadTree(root, label, { requireSingleLink });
  if (
    canonicalJson([...actual.files.keys()].sort())
      !== canonicalJson([...expectedFiles].sort())
    || canonicalJson([...actual.directories].sort())
      !== canonicalJson([...expectedDirs].sort())
  ) {
    fail(`${label} does not contain the exact candidate payload allowlist`);
  }
  for (const payload of payloads) {
    const file = actual.files.get(payload.relative_path);
    const measured = sha256File(file, `${label} ${payload.role}`, {
      requireSingleLink,
    });
    if (measured.bytes !== payload.bytes || measured.sha256 !== payload.sha256) {
      fail(`${label} ${payload.role} does not match its retained size and SHA-256`);
    }
  }
  return actual;
}

function readRecordFile(recordFile, label) {
  const resolved = path.resolve(recordFile);
  const canonical = realpathSync.native(resolved);
  if (pathIdentity(canonical) !== pathIdentity(resolved)) {
    fail(`${label} must not have symbolic-link or reparse ancestry`);
  }
  const opened = safeRegularFile(
    resolved,
    label,
    { requireSingleLink: true },
  );
  if (opened.size > 64 * 1024) {
    closeSync(opened.handle);
    fail(`${label} is too large`);
  }
  let encoded;
  try {
    encoded = readHandle(
      opened.handle,
      opened.size,
      label,
    ).toString("utf8");
  } finally {
    closeSync(opened.handle);
  }
  let parsed;
  try {
    parsed = JSON.parse(encoded);
  } catch {
    fail(`${label} is not valid JSON`);
  }
  return validateCandidateArchiveRecord(parsed);
}

export function readCandidateArchiveRecord(recordFile) {
  return readRecordFile(recordFile, "candidate archive authenticated record");
}

export function writeCandidateArchiveRecord(recordFile, record) {
  const expected = validateCandidateArchiveRecord(record);
  const resolved = path.resolve(recordFile);
  const parent = requireRealRoot(
    path.dirname(resolved),
    "candidate archive record output directory",
  );
  containedRelativePath(
    parent,
    resolved,
    "candidate archive record output",
  );
  writeStoredRecord(resolved, expected);
  return resolved;
}

function sameRecord(left, right) {
  return canonicalJson(left) === canonicalJson(right);
}

function verifyStoreEntry(storeRoot, expectedRecord) {
  const expected = validateCandidateArchiveRecord(expectedRecord);
  const paths = entryPaths(storeRoot, expected);
  inspectPathAncestry({
    allowMissing: false,
    candidate: paths.entry,
    label: "candidate archive store entry",
    root: paths.root,
  });
  const entryMetadata = lstatSync(paths.entry);
  if (entryMetadata.isSymbolicLink() || !entryMetadata.isDirectory()) {
    fail("candidate archive store entry must be a real directory");
  }
  const entries = readdirSync(paths.entry).sort();
  if (canonicalJson(entries) !== canonicalJson([PAYLOAD_DIRECTORY, RECORD_FILE].sort())) {
    fail("candidate archive store entry contains unexpected files");
  }
  const stored = readRecordFile(
    paths.recordFile,
    "candidate archive stored record",
  );
  if (!sameRecord(stored, expected)) {
    fail("candidate archive store entry belongs to a different candidate");
  }
  verifyPayloadTree(paths.payload, expected, "candidate archive store payload");
  return { paths, record: stored };
}

function ensureRealDescendantDirectories(root, candidate, label) {
  const resolvedRoot = requireRealRoot(root, `${label} root`);
  const resolvedCandidate = path.resolve(candidate);
  const relative = containedRelativePath(
    resolvedRoot,
    resolvedCandidate,
    label,
    { allowEqual: true },
  );
  if (relative === "") return resolvedCandidate;
  let cursor = resolvedRoot;
  for (const component of relative.split(path.sep)) {
    cursor = path.join(cursor, component);
    if (!existsSync(cursor)) {
      mkdirSync(cursor, { mode: 0o700 });
    }
    const metadata = lstatSync(cursor);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      fail(`${label} must not traverse symbolic links, reparse points, or files`);
    }
  }
  return resolvedCandidate;
}

function writeStoredRecord(file, record) {
  const output = openSync(
    file,
    constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
    0o600,
  );
  try {
    const bytes = Buffer.from(`${JSON.stringify(record, null, 2)}\n`, "utf8");
    writeAll(output, bytes, 0);
    fsyncSync(output);
  } finally {
    closeSync(output);
  }
}

function uniqueTemporarySibling(parent, basename) {
  return path.join(
    parent,
    `.${basename}.partial-${process.pid}-${randomBytes(12).toString("hex")}`,
  );
}

function requireOwnedCorruptEntry(paths) {
  inspectPathAncestry({
    allowMissing: false,
    candidate: paths.entry,
    label: "candidate archive corrupt store entry",
    root: paths.root,
  });
  const entryMetadata = lstatSync(paths.entry, { bigint: true });
  if (entryMetadata.isSymbolicLink() || !entryMetadata.isDirectory()) {
    fail("candidate archive corrupt store entry must be a real directory");
  }
  const canonical = realpathSync.native(paths.entry);
  if (pathIdentity(canonical) !== pathIdentity(paths.entry)) {
    fail("candidate archive corrupt store entry must have real ancestry");
  }
  const pending = [paths.entry];
  while (pending.length > 0) {
    const directory = pending.pop();
    for (const name of readdirSync(directory)) {
      const entry = path.join(directory, name);
      const metadata = lstatSync(entry, { bigint: true });
      if (metadata.isSymbolicLink()) {
        fail("candidate archive corrupt store entry contains a symbolic link or reparse point");
      }
      if (metadata.isDirectory()) {
        pending.push(entry);
      } else if (!metadata.isFile() || metadata.nlink !== 1n) {
        fail("candidate archive corrupt store entry contains an unowned file");
      }
    }
  }
  return entryMetadata;
}

function quarantineCorruptStoreEntry(paths) {
  const before = requireOwnedCorruptEntry(paths);
  const rejectedEntry = path.join(
    paths.parent,
    `.${path.basename(paths.entry)}.rejected-${process.pid}-${randomBytes(12).toString("hex")}`,
  );
  renameSync(paths.entry, rejectedEntry);
  const after = lstatSync(rejectedEntry, { bigint: true });
  if (
    !after.isDirectory()
    || after.isSymbolicLink()
    || after.dev !== before.dev
    || after.ino !== before.ino
  ) {
    fail("candidate archive rejected store entry changed during quarantine");
  }
  return rejectedEntry;
}

function removeOwnedTemporary(directory, parent, basename) {
  const resolvedParent = path.resolve(parent);
  const resolved = path.resolve(directory);
  containedRelativePath(resolvedParent, resolved, "owned temporary directory");
  if (!path.basename(resolved).startsWith(`.${basename}.partial-${process.pid}-`)) {
    fail("refusing to remove an unowned temporary directory");
  }
  rmSync(resolved, { force: true, recursive: true });
}

function copyPayloadTree({
  destination,
  record,
  source,
  sourceRequiresSingleLink,
}) {
  const payloads = requiredPayloads(record);
  mkdirSync(destination, { mode: 0o700 });
  for (const directory of [...expectedDirectories(payloads)].sort()) {
    ensureRealDescendantDirectories(destination, path.join(destination, ...directory.split("/")), "candidate payload directory");
  }
  for (const payload of payloads) {
    const components = payload.relative_path.split("/");
    copyVerifiedFile({
      destination: path.join(destination, ...components),
      expected: payload,
      label: `candidate payload ${payload.role}`,
      source: path.join(source, ...components),
      sourceRequiresSingleLink,
    });
  }
  verifyPayloadTree(destination, record, "materialized candidate payload");
}

function publishStoreEntry(storeRoot, inputRoot, record) {
  const expected = validateCandidateArchiveRecord(record);
  const paths = entryPaths(storeRoot, expected);
  verifyPayloadTree(
    requireRealRoot(inputRoot, "candidate archive input root"),
    expected,
    "candidate archive input payload",
  );
  if (existsSync(paths.entry)) {
    return { admitted: false, ...verifyStoreEntry(storeRoot, expected) };
  }
  ensureRealDescendantDirectories(paths.root, paths.parent, "candidate archive store parent");
  const temporary = uniqueTemporarySibling(paths.parent, path.basename(paths.entry));
  try {
    mkdirSync(temporary, { mode: 0o700 });
    const temporaryPayload = path.join(temporary, PAYLOAD_DIRECTORY);
    copyPayloadTree({
      destination: temporaryPayload,
      record: expected,
      source: inputRoot,
      sourceRequiresSingleLink: true,
    });
    writeStoredRecord(path.join(temporary, RECORD_FILE), expected);
    const temporaryEntries = readdirSync(temporary).sort();
    if (
      canonicalJson(temporaryEntries)
        !== canonicalJson([PAYLOAD_DIRECTORY, RECORD_FILE].sort())
    ) {
      fail("candidate archive temporary entry changed before publication");
    }
    const temporaryRecord = readRecordFile(
      path.join(temporary, RECORD_FILE),
      "candidate archive temporary record",
    );
    if (!sameRecord(temporaryRecord, expected)) {
      fail("candidate archive temporary record changed before publication");
    }
    verifyPayloadTree(
      temporaryPayload,
      expected,
      "candidate archive temporary payload",
    );

    if (existsSync(paths.entry)) {
      const concurrent = verifyStoreEntry(storeRoot, expected);
      removeOwnedTemporary(temporary, paths.parent, path.basename(paths.entry));
      return { admitted: false, ...concurrent };
    }
    const prepared = lstatSync(temporary, { bigint: true });
    try {
      renameSync(temporary, paths.entry);
    } catch (error) {
      if (!["EEXIST", "ENOTEMPTY"].includes(error?.code) || !existsSync(paths.entry)) {
        throw error;
      }
      const concurrent = verifyStoreEntry(storeRoot, expected);
      removeOwnedTemporary(temporary, paths.parent, path.basename(paths.entry));
      return { admitted: false, ...concurrent };
    }
    const published = lstatSync(paths.entry, { bigint: true });
    if (
      !published.isDirectory()
      || published.isSymbolicLink()
      || published.dev !== prepared.dev
      || published.ino !== prepared.ino
    ) {
      fail("candidate archive store entry was not published by atomic directory rename");
    }
    return { admitted: true, ...verifyStoreEntry(storeRoot, expected) };
  } catch (error) {
    if (existsSync(temporary)) {
      removeOwnedTemporary(temporary, paths.parent, path.basename(paths.entry));
    }
    throw error;
  }
}

function materializeStoreEntry({
  outputDir,
  outputRoot,
  record,
  storeRoot,
}) {
  const verified = verifyStoreEntry(storeRoot, record);
  const trustedOutputRoot = requireRealRoot(
    outputRoot,
    "candidate archive output root",
  );
  const resolvedOutput = path.resolve(outputDir);
  containedRelativePath(
    trustedOutputRoot,
    resolvedOutput,
    "candidate archive output directory",
  );
  const outputParent = ensureRealDescendantDirectories(
    trustedOutputRoot,
    path.dirname(resolvedOutput),
    "candidate archive output parent",
  );
  if (existsSync(resolvedOutput)) {
    fail("candidate archive output directory must not already exist");
  }
  const temporary = uniqueTemporarySibling(outputParent, path.basename(resolvedOutput));
  try {
    copyPayloadTree({
      destination: temporary,
      record: verified.record,
      source: verified.paths.payload,
      sourceRequiresSingleLink: true,
    });
    if (existsSync(resolvedOutput)) {
      fail("candidate archive output directory appeared during materialization");
    }
    renameSync(temporary, resolvedOutput);
    const materialized = verifyPayloadTree(
      resolvedOutput,
      verified.record,
      "candidate archive restored payload",
    );
    const companions = Object.fromEntries(
      verified.record.companions.map((companion) => [
        companion.role,
        materialized.files.get(companion.relative_path),
      ]),
    );
    return {
      archive: materialized.files.get(verified.record.archive.relative_path),
      companions,
      key: verified.paths.key,
      outputDir: resolvedOutput,
      record: verified.record,
    };
  } catch (error) {
    if (existsSync(temporary)) {
      removeOwnedTemporary(
        temporary,
        outputParent,
        path.basename(resolvedOutput),
      );
    }
    throw error;
  }
}

export function restoreCandidateArchive({
  outputDir,
  outputRoot,
  record,
  storeRoot,
}) {
  const expected = validateCandidateArchiveRecord(record);
  const paths = entryPaths(storeRoot, expected);
  if (!existsSync(paths.entry)) {
    return {
      hit: false,
      key: paths.key,
      record: expected,
    };
  }
  try {
    verifyStoreEntry(storeRoot, expected);
  } catch (error) {
    const rejectedEntry = quarantineCorruptStoreEntry(paths);
    return {
      hit: false,
      key: paths.key,
      record: expected,
      rejectedCorrupt: true,
      rejectedEntry,
      rejection: error.message,
    };
  }
  return {
    hit: true,
    ...materializeStoreEntry({
      outputDir,
      outputRoot,
      record: expected,
      storeRoot,
    }),
  };
}

export function admitCandidateArchive({
  inputRoot,
  outputDir,
  outputRoot,
  record,
  storeRoot,
}) {
  const expected = validateCandidateArchiveRecord(record);
  const stored = publishStoreEntry(storeRoot, inputRoot, expected);
  return {
    admitted: stored.admitted,
    hit: !stored.admitted,
    ...materializeStoreEntry({
      outputDir,
      outputRoot,
      record: expected,
      storeRoot,
    }),
  };
}

function parseArguments(argv) {
  const [command, ...rest] = argv;
  const values = new Map();
  for (let index = 0; index < rest.length; index += 2) {
    const flag = rest[index];
    const value = rest[index + 1];
    if (!flag?.startsWith("--") || value === undefined) {
      fail(`invalid argument near ${flag ?? "<end>"}`);
    }
    if (!values.has(flag)) values.set(flag, []);
    values.get(flag).push(value);
  }
  return { command, values };
}

function one(values, flag) {
  const found = values.get(flag) ?? [];
  if (found.length !== 1) {
    fail(`${flag} must be supplied exactly once`);
  }
  return found[0];
}

function parsePositiveInteger(value, label) {
  if (!/^[1-9][0-9]*$/u.test(value)) {
    fail(`${label} must be a positive integer`);
  }
  return requireBytes(Number.parseInt(value, 10), label);
}

function parseCompanion(value) {
  const parts = value.split("|");
  if (parts.length !== 4) {
    fail("--companion must use role|relative_path|bytes|sha256");
  }
  return {
    role: parts[0],
    relative_path: parts[1],
    bytes: parsePositiveInteger(parts[2], "companion bytes"),
    sha256: parts[3],
  };
}

function exactFlags(values, allowed) {
  const actual = [...values.keys()].sort();
  const expected = [...allowed].sort();
  if (canonicalJson(actual) !== canonicalJson(expected)) {
    fail("candidate archive store arguments changed");
  }
}

function recordFromArguments(values) {
  if (values.has("--record")) {
    return readCandidateArchiveRecord(one(values, "--record"));
  }
  const archiveName = one(values, "--archive-name");
  return buildCandidateArchiveRecord({
    repository: one(values, "--repository"),
    sourceSha: one(values, "--source-sha"),
    sourceTree: one(values, "--source-tree"),
    target: one(values, "--target"),
    archive: {
      name: archiveName,
      relative_path: archiveName,
      bytes: parsePositiveInteger(
        one(values, "--archive-bytes"),
        "archive bytes",
      ),
      sha256: one(values, "--archive-sha256"),
    },
    companions: (values.get("--companion") ?? []).map(parseCompanion),
  });
}

function usage() {
  return [
    "Usage:",
    "  candidate-archive-store.mjs record --output JSON --repository OWNER/REPO --source-sha SHA --source-tree TREE --target TARGET --archive-name NAME --archive-bytes BYTES --archive-sha256 SHA256 --companion 'role|relative_path|bytes|sha256'...",
    "  candidate-archive-store.mjs restore --record AUTHENTICATED_JSON --store-root DIR --output-root DIR --output-dir DIR",
    "  candidate-archive-store.mjs admit --record AUTHENTICATED_JSON --input-root DIR --store-root DIR --output-root DIR --output-dir DIR",
    "",
    "Explicit record fields are retained for bounded tooling and tests:",
    "  candidate-archive-store.mjs restore --store-root DIR --output-root DIR --output-dir DIR --repository OWNER/REPO --source-sha SHA --source-tree TREE --target TARGET --archive-name NAME --archive-bytes BYTES --archive-sha256 SHA256 [--companion 'role|relative_path|bytes|sha256']...",
    "  candidate-archive-store.mjs admit --input-root DIR --store-root DIR --output-root DIR --output-dir DIR --repository OWNER/REPO --source-sha SHA --source-tree TREE --target TARGET --archive-name NAME --archive-bytes BYTES --archive-sha256 SHA256 [--companion 'role|relative_path|bytes|sha256']...",
  ].join("\n");
}

function main(argv) {
  const { command, values } = parseArguments(argv);
  const operational = [
    "--output-dir",
    "--output-root",
    "--store-root",
  ];
  const recordFields = [
    "--archive-bytes",
    "--archive-name",
    "--archive-sha256",
    "--repository",
    "--source-sha",
    "--source-tree",
    "--target",
  ];
  const usesRecord = values.has("--record");
  if (
    usesRecord
    && [...recordFields, "--companion"].some((flag) => values.has(flag))
  ) {
    fail("--record cannot be combined with explicit record fields");
  }
  const allowed = usesRecord
    ? [...operational, "--record"]
    : [
      ...operational,
      ...recordFields,
      ...(values.has("--companion") ? ["--companion"] : []),
    ];
  if (command === "record") {
    if (usesRecord) {
      fail("record production requires explicit authenticated fields");
    }
    exactFlags(values, [
      ...recordFields,
      "--output",
      ...(values.has("--companion") ? ["--companion"] : []),
    ]);
    const written = writeCandidateArchiveRecord(
      one(values, "--output"),
      recordFromArguments(values),
    );
    process.stdout.write(`${JSON.stringify({ record: written })}\n`);
    return;
  }
  if (command === "restore") {
    exactFlags(values, allowed);
  } else if (command === "admit") {
    exactFlags(values, [...allowed, "--input-root"]);
  } else {
    fail(usage());
  }
  const commonArguments = {
    outputDir: one(values, "--output-dir"),
    outputRoot: one(values, "--output-root"),
    record: recordFromArguments(values),
    storeRoot: one(values, "--store-root"),
  };
  const result = command === "restore"
    ? restoreCandidateArchive(commonArguments)
    : admitCandidateArchive({
      ...commonArguments,
      inputRoot: one(values, "--input-root"),
    });
  if (result.rejectedCorrupt) {
    process.stderr.write(
      `candidate archive cache rejected corrupt entry ${result.key}: `
      + `${result.rejection}; quarantined at ${result.rejectedEntry}\n`,
    );
  }
  process.stdout.write(`${JSON.stringify({
    admitted: result.admitted ?? false,
    archive: result.archive ?? null,
    companions: result.companions ?? {},
    hit: result.hit,
    key: result.key,
    output_dir: result.outputDir ?? null,
    rejected_corrupt: result.rejectedCorrupt ?? false,
    rejected_entry: result.rejectedEntry ?? null,
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
