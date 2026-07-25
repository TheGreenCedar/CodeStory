#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";

const SHA40 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;

function fail(message) {
  throw new Error(message);
}

function nonEmpty(value, name) {
  if (typeof value !== "string" || value.trim() === "") {
    fail(`${name} must be non-empty`);
  }
  return value.trim();
}

function slug(value, name) {
  const normalized = nonEmpty(value, name)
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/gu, "-")
    .replace(/^-+|-+$/gu, "");
  if (normalized === "") fail(`${name} has no cache-safe characters`);
  return normalized;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
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

function validatedDigest(value, name) {
  const digest = nonEmpty(value, name).toLowerCase();
  if (!SHA256.test(digest)) fail(`${name} must be a SHA-256 digest`);
  return digest;
}

export function buildCacheContract({
  namespace,
  exactSha,
  os,
  target,
  rustVersion,
  features,
  nativeToolchain,
  generator,
  cmakeVersion,
  ninjaVersion,
  sccacheVersion,
  cargoLockSha256,
  cargoConfigSha256,
  relevantInputs = {},
  extraIdentity = {},
}) {
  const exact = nonEmpty(exactSha, "exactSha");
  if (!SHA40.test(exact)) fail("exactSha must be a full lowercase commit SHA");

  const normalizedInputs = Object.fromEntries(
    Object.entries(relevantInputs)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([name, digest]) => [
        nonEmpty(name, "relevant input name"),
        validatedDigest(digest, `relevant input ${name}`),
      ]),
  );
  const normalizedExtra = Object.fromEntries(
    Object.entries(extraIdentity)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([name, value]) => [
        nonEmpty(name, "extra identity name"),
        nonEmpty(value, `extra identity ${name}`),
      ]),
  );

  const common = {
    os: nonEmpty(os, "os"),
    target: nonEmpty(target, "target"),
    rust_version: nonEmpty(rustVersion, "rustVersion"),
    cargo_lock_sha256: validatedDigest(cargoLockSha256, "cargoLockSha256"),
    cargo_config_sha256: validatedDigest(cargoConfigSha256, "cargoConfigSha256"),
  };
  const dependencyIdentity = {
    schema: "codestory-cargo-dependencies/v1",
    ...common,
  };
  const compilerIdentity = {
    schema: "codestory-compiler-objects/v1",
    ...common,
    features: nonEmpty(features, "features"),
    native_toolchain: nonEmpty(nativeToolchain, "nativeToolchain"),
    generator: nonEmpty(generator, "generator"),
    cmake_version: nonEmpty(cmakeVersion, "cmakeVersion"),
    ninja_version: nonEmpty(ninjaVersion, "ninjaVersion"),
    sccache_version: nonEmpty(sccacheVersion, "sccacheVersion"),
    relevant_inputs: normalizedInputs,
    extra_identity: normalizedExtra,
  };

  const namespaceSlug = slug(namespace, "namespace");
  const osSlug = slug(common.os, "os");
  const targetSlug = slug(common.target, "target");
  const rustSlug = slug(common.rust_version, "rustVersion");
  const dependencyHash = sha256(canonicalJson(dependencyIdentity));
  const compatibilityHash = sha256(canonicalJson(compilerIdentity));
  const dependencyKey = [
    "codestory",
    namespaceSlug,
    "dependencies-v1",
    osSlug,
    targetSlug,
    rustSlug,
    dependencyHash,
  ].join("-");
  const compilerPrefix = [
    "codestory",
    namespaceSlug,
    "compiler-v1",
    osSlug,
    targetSlug,
    rustSlug,
    compatibilityHash,
    "",
  ].join("-");
  const compilerKey = `${compilerPrefix}${exact}`;
  if (!compilerKey.endsWith(exact) || compilerKey.slice(0, -exact.length).includes(exact)) {
    fail("exact SHA must appear only as the compiler save-key suffix");
  }

  return {
    dependencyIdentity,
    compilerIdentity,
    dependencyHash,
    compatibilityHash,
    dependencyKey,
    compilerPrefix,
    compilerKey,
  };
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (!token.startsWith("--")) fail(`unexpected argument: ${token}`);
    const name = token.slice(2);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) fail(`missing value for --${name}`);
    if (!values.has(name)) values.set(name, []);
    values.get(name).push(value);
    index += 1;
  }
  return values;
}

function one(args, name, { required = true, fallback = "" } = {}) {
  const values = args.get(name) ?? [];
  if (values.length > 1) fail(`--${name} may be supplied only once`);
  if (values.length === 0) {
    if (required) fail(`missing --${name}`);
    return fallback;
  }
  return values[0];
}

function pairs(values, name) {
  const result = {};
  for (const value of values) {
    const separator = value.indexOf("=");
    if (separator <= 0 || separator === value.length - 1) {
      fail(`--${name} must use name=value`);
    }
    const key = value.slice(0, separator);
    if (Object.hasOwn(result, key)) fail(`duplicate --${name} key: ${key}`);
    result[key] = value.slice(separator + 1);
  }
  return result;
}

function hashFile(file) {
  return sha256(fs.readFileSync(file));
}

function relevantInputDigests(files) {
  const result = {};
  for (const file of [...files].sort()) {
    const normalized = file.replaceAll("\\", "/");
    if (Object.hasOwn(result, normalized)) fail(`duplicate relevant input: ${normalized}`);
    if (!fs.statSync(file).isFile()) fail(`relevant input is not a file: ${file}`);
    result[normalized] = hashFile(file);
  }
  return result;
}

function appendOutput(entries) {
  const output = process.env.GITHUB_OUTPUT;
  if (!output) fail("GITHUB_OUTPUT is required");
  fs.appendFileSync(
    output,
    `${Object.entries(entries).map(([key, value]) => `${key}=${value}`).join("\n")}\n`,
  );
}

function appendSummary(markdown) {
  const summary = process.env.GITHUB_STEP_SUMMARY;
  if (summary) fs.appendFileSync(summary, `${markdown}\n`);
}

function directoryBytes(root) {
  if (!fs.existsSync(root)) return 0;
  const pending = [root];
  let bytes = 0;
  while (pending.length > 0) {
    const current = pending.pop();
    const entry = fs.lstatSync(current);
    if (entry.isDirectory()) {
      for (const child of fs.readdirSync(current)) {
        pending.push(path.join(current, child));
      }
    } else {
      bytes += entry.size;
    }
  }
  return bytes;
}

function cacheBytes(paths) {
  if (paths.length === 0) fail("at least one --path is required");
  return paths.reduce((total, root) => total + directoryBytes(root), 0);
}

function cacheHitType(cacheHit, matchedKey) {
  if (cacheHit === "true") return "exact";
  if (matchedKey !== "") return "compatible";
  return "miss";
}

function identityCommand(args) {
  const lockFile = one(args, "lock-file");
  const contract = buildCacheContract({
    namespace: one(args, "namespace"),
    exactSha: one(args, "exact-sha"),
    os: one(args, "os"),
    target: one(args, "target"),
    rustVersion: one(args, "rust-version"),
    features: one(args, "features"),
    nativeToolchain: one(args, "native-toolchain"),
    generator: one(args, "generator"),
    cmakeVersion: one(args, "cmake-version"),
    ninjaVersion: one(args, "ninja-version"),
    sccacheVersion: one(args, "sccache-version"),
    cargoLockSha256: hashFile(lockFile),
    cargoConfigSha256: hashFile(one(args, "cargo-config")),
    relevantInputs: relevantInputDigests(args.get("relevant-input") ?? []),
    extraIdentity: pairs(args.get("identity") ?? [], "identity"),
  });
  appendOutput({
    "dependency-key": contract.dependencyKey,
    "compiler-prefix": contract.compilerPrefix,
    "compiler-key": contract.compilerKey,
    "dependency-hash": contract.dependencyHash,
    "compatibility-hash": contract.compatibilityHash,
  });
  console.log(`dependency cache requested key: ${contract.dependencyKey}`);
  console.log(`compiler cache compatibility prefix: ${contract.compilerPrefix}`);
  console.log(`compiler cache requested key: ${contract.compilerKey}`);
  console.log(`compiler cache identity: ${canonicalJson(contract.compilerIdentity)}`);
  appendSummary([
    "### Build cache request",
    "",
    `- Dependency key: \`${contract.dependencyKey}\``,
    `- Compiler compatibility prefix: \`${contract.compilerPrefix}\``,
    `- Compiler requested key: \`${contract.compilerKey}\``,
  ].join("\n"));
}

function restoreCommand(args) {
  const kind = one(args, "kind");
  const requestedKey = one(args, "requested-key");
  const matchedKey = one(args, "matched-key", { required: false });
  const prefix = one(args, "compatibility-prefix", { required: false });
  const hitType = cacheHitType(one(args, "cache-hit"), matchedKey);
  const restoredBytes = cacheBytes(args.get("path") ?? []);
  appendOutput({
    "hit-type": hitType,
    "restored-bytes": restoredBytes,
  });
  console.log(`${kind} cache requested key: ${requestedKey}`);
  console.log(`${kind} cache restored key: ${matchedKey || "(none)"}`);
  console.log(`${kind} cache compatibility prefix: ${prefix || "(exact-only)"}`);
  console.log(`${kind} cache hit type: ${hitType}`);
  console.log(`${kind} cache restored bytes: ${restoredBytes}`);
  appendSummary([
    `### ${kind} cache restore`,
    "",
    `- Requested key: \`${requestedKey}\``,
    `- Restored key: \`${matchedKey || "(none)"}\``,
    `- Compatibility prefix: \`${prefix || "(exact-only)"}\``,
    `- Hit type: \`${hitType}\``,
    `- Restored bytes: \`${restoredBytes}\``,
  ].join("\n"));
}

function startCommand() {
  appendOutput({ "started-ms": Date.now() });
}

function stopCommand() {
  appendOutput({ "ended-ms": Date.now() });
}

function sizeCommand(args) {
  const kind = one(args, "kind");
  const maxBytes = Number.parseInt(one(args, "max-bytes"), 10);
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    fail("--max-bytes must be a positive integer");
  }
  const currentBytes = cacheBytes(args.get("path") ?? []);
  const withinLimit = currentBytes <= maxBytes;
  appendOutput({
    "cache-bytes": currentBytes,
    "within-limit": withinLimit,
  });
  console.log(`${kind} cache bytes: ${currentBytes}`);
  console.log(`${kind} cache byte limit: ${maxBytes}`);
  console.log(`${kind} cache within limit: ${withinLimit}`);
  appendSummary([
    `### ${kind} cache size`,
    "",
    `- Current bytes: \`${currentBytes}\``,
    `- Maximum bytes: \`${maxBytes}\``,
    `- Within limit: \`${withinLimit}\``,
  ].join("\n"));
}

function saveCommand(args) {
  const kind = one(args, "kind");
  const requestedKey = one(args, "requested-key");
  const matchedKey = one(args, "matched-key", { required: false });
  const prefix = one(args, "compatibility-prefix", { required: false });
  const hitType = one(args, "hit-type");
  const restoredBytes = one(args, "restored-bytes");
  const startedMs = Number.parseInt(one(args, "started-ms"), 10);
  if (!Number.isSafeInteger(startedMs) || startedMs <= 0) fail("--started-ms must be a positive integer");
  const endedMs = Number.parseInt(one(args, "ended-ms"), 10);
  if (!Number.isSafeInteger(endedMs) || endedMs < startedMs) {
    fail("--ended-ms must be an integer no earlier than --started-ms");
  }
  const saveStartedMs = Number.parseInt(one(args, "save-started-ms"), 10);
  if (!Number.isSafeInteger(saveStartedMs) || saveStartedMs < endedMs) {
    fail("--save-started-ms must be an integer no earlier than --ended-ms");
  }
  const currentBytes = cacheBytes(args.get("path") ?? []);
  const compileSeconds = Math.max(0, Math.round((endedMs - startedMs) / 1000));
  const saveSeconds = Math.max(0, Math.round((Date.now() - saveStartedMs) / 1000));
  const saveResult = one(args, "save-result");
  appendOutput({
    "compile-seconds": compileSeconds,
    "save-seconds": saveSeconds,
    "cache-bytes": currentBytes,
    "save-result": saveResult,
  });
  console.log(`${kind} cache requested key: ${requestedKey}`);
  console.log(`${kind} cache restored key: ${matchedKey || "(none)"}`);
  console.log(`${kind} cache compatibility prefix: ${prefix || "(exact-only)"}`);
  console.log(`${kind} cache hit type: ${hitType}`);
  console.log(`${kind} cache restored bytes: ${restoredBytes}`);
  console.log(`${kind} compile duration seconds: ${compileSeconds}`);
  console.log(`${kind} cache save duration seconds: ${saveSeconds}`);
  console.log(`${kind} cache bytes after compilation: ${currentBytes}`);
  console.log(`${kind} cache save result: ${saveResult}`);
  appendSummary([
    `### ${kind} cache save`,
    "",
    `- Requested key: \`${requestedKey}\``,
    `- Restored key: \`${matchedKey || "(none)"}\``,
    `- Compatibility prefix: \`${prefix || "(exact-only)"}\``,
    `- Hit type: \`${hitType}\``,
    `- Restored bytes: \`${restoredBytes}\``,
    `- Compile duration: \`${compileSeconds}s\``,
    `- Cache save duration: \`${saveSeconds}s\``,
    `- Cache bytes after compilation: \`${currentBytes}\``,
    `- Save result: \`${saveResult}\``,
  ].join("\n"));
}

function main() {
  const [command, ...rest] = process.argv.slice(2);
  const args = parseArguments(rest);
  if (command === "identity") return identityCommand(args);
  if (command === "restore") return restoreCommand(args);
  if (command === "start") return startCommand();
  if (command === "stop") return stopCommand();
  if (command === "size") return sizeCommand(args);
  if (command === "save") return saveCommand(args);
  fail("usage: cargo-cache-contract.mjs <identity|restore|start|stop|size|save> [options]");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
