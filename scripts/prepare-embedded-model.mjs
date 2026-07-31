#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  appendFile,
  copyFile,
  link,
  lstat,
  mkdir,
  open,
  readFile,
  realpath,
  rm,
} from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { dirname, parse, relative, resolve, sep } from "node:path";
import { Readable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const downloadAttemptsPerSource = 3;
const downloadBackoffMs = 500;

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const defaultContract = resolve(
  scriptDirectory,
  "../crates/codestory-llama-sys/model-contract.json",
);

function argument(name) {
  const index = process.argv.indexOf(name);
  if (index < 0) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error(`${name} requires a value`);
  return value;
}

function requiredObject(value, name) {
  const field = value?.[name];
  if (!field || typeof field !== "object" || Array.isArray(field)) {
    throw new Error(`model contract field ${name} must be an object`);
  }
  return field;
}

function requiredString(value, name) {
  const field = value?.[name];
  if (typeof field !== "string" || field.length === 0) {
    throw new Error(`model contract field ${name} must be a non-empty string`);
  }
  return field;
}

function requiredStringAllowEmpty(value, name) {
  const field = value?.[name];
  if (typeof field !== "string") {
    throw new Error(`model contract field ${name} must be a string`);
  }
  return field;
}

function contractDigest(domain, value) {
  const hash = createHash("sha256");
  for (const text of [domain, value]) {
    const bytes = Buffer.from(text);
    const length = Buffer.alloc(8);
    length.writeBigUInt64LE(BigInt(bytes.length));
    hash.update(length);
    hash.update(bytes);
  }
  return hash.digest("hex");
}

async function loadContract(path) {
  const contract = JSON.parse(await readFile(path, "utf8"));
  if (contract.schema_version !== 1) {
    throw new Error("unsupported model contract schema_version");
  }
  const model = requiredObject(contract, "model");
  const fileName = requiredString(model, "file_name");
  if (
    fileName === "." ||
    fileName === ".." ||
    fileName.includes("/") ||
    fileName.includes("\\") ||
    !/^[A-Za-z0-9._-]+$/u.test(fileName)
  ) {
    throw new Error("model.file_name must be a safe file name");
  }
  const size = model.size_bytes;
  if (!Number.isSafeInteger(size) || size <= 0) {
    throw new Error("model.size_bytes must be a positive integer");
  }
  const sha256 = requiredString(model, "sha256");
  if (!/^[0-9a-f]{64}$/u.test(sha256)) {
    throw new Error("model.sha256 must be a lowercase SHA-256 digest");
  }
  if (!Array.isArray(model.sources) || model.sources.length === 0) {
    throw new Error("model.sources must be a non-empty array");
  }
  const urls = model.sources.map((source, index) => {
    const url = requiredString(source, "url");
    const revision = requiredString(source, "revision");
    const parsed = new URL(url);
    if (!/^[0-9a-f]{40}$/u.test(revision)) {
      throw new Error(`model.sources[${index}].revision must be a lowercase commit`);
    }
    if (parsed.protocol !== "https:") {
      throw new Error(`model.sources[${index}].url must use HTTPS`);
    }
    if (!parsed.pathname.includes(`/resolve/${revision}/`)) {
      throw new Error(`model.sources[${index}].url must contain its pinned revision`);
    }
    return url;
  });
  const embedding = requiredObject(contract, "embedding");
  const dimension = embedding.dimension;
  if (!Number.isSafeInteger(dimension) || dimension <= 0) {
    throw new Error("embedding.dimension must be a positive integer");
  }
  const queryPrefix = requiredString(embedding, "query_prefix");
  requiredStringAllowEmpty(embedding, "document_prefix");
  const pooling = requiredString(embedding, "pooling");
  const normalization = requiredString(embedding, "normalization");
  if (pooling !== "cls" || normalization !== "l2") {
    throw new Error("unsupported embedding pooling or normalization");
  }
  if (
    requiredString(embedding, "element_type") !== "f32_le" ||
    embedding.vector_schema_version !== 2
  ) {
    throw new Error("unsupported embedding vector format");
  }
  const tokenizerConfig = requiredObject(contract, "tokenizer_config");
  if (requiredString(tokenizerConfig, "container") !== "gguf") {
    throw new Error("unsupported tokenizer_config.container");
  }
  const tokenizerSha256 = requiredString(tokenizerConfig, "tokenizer_sha256");
  const configSha256 = requiredString(tokenizerConfig, "config_sha256");
  if (tokenizerSha256 !== contractDigest("tokenizer", sha256)) {
    throw new Error("tokenizer_config.tokenizer_sha256 does not match the model identity");
  }
  if (configSha256 !== contractDigest("config", `${sha256}:${dimension}:${pooling}:${normalization}`)) {
    throw new Error("tokenizer_config.config_sha256 does not match the embedding semantics");
  }
  const producer = requiredObject(contract, "producer");
  requiredString(producer, "name");
  requiredString(producer, "version");
  const license = requiredObject(contract, "license");
  if (requiredString(license, "spdx_id") !== "MIT") {
    throw new Error("unsupported model license");
  }
  if (new URL(requiredString(license, "source_url")).protocol !== "https:") {
    throw new Error("license.source_url must use HTTPS");
  }
  return { fileName, sha256, size, urls };
}

async function digest(path) {
  const file = await open(path, "r");
  const hash = createHash("sha256");
  try {
    for await (const chunk of file.createReadStream({ autoClose: false })) hash.update(chunk);
  } finally {
    await file.close();
  }
  return hash.digest("hex");
}

async function valid(path, contract) {
  try {
    const metadata = await lstat(path);
    return (
      metadata.isFile() &&
      metadata.nlink === 1 &&
      metadata.size === contract.size &&
      (await digest(path)) === contract.sha256
    );
  } catch {
    return false;
  }
}

async function publishSource(source, destination, contract) {
  if (!(await valid(source, contract))) {
    throw new Error(`invalid pinned model source: ${source}`);
  }
  const temporary = `${destination}.${process.pid}.partial`;
  try {
    await copyFile(source, temporary, fsConstants.COPYFILE_EXCL);
    if (!(await valid(temporary, contract))) {
      throw new Error(`copied model source failed verification: ${source}`);
    }
    await publishNoReplace(temporary, destination, contract);
  } catch (error) {
    await rm(temporary, { force: true });
    throw error;
  }
}

async function download(url, destination, contract) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`${url} returned HTTP ${response.status}`);
  }
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared !== contract.size) {
    throw new Error(`${url} declared ${declared} bytes; expected ${contract.size}`);
  }
  const temporary = `${destination}.${process.pid}.partial`;
  const file = await open(temporary, "wx");
  const hash = createHash("sha256");
  let bytes = 0;
  try {
    for await (const chunk of Readable.fromWeb(response.body)) {
      bytes += chunk.length;
      if (bytes > contract.size) {
        throw new Error(`${url} exceeded the pinned ${contract.size}-byte size`);
      }
      hash.update(chunk);
      await file.write(chunk);
    }
    await file.sync();
  } catch (error) {
    await file.close();
    await rm(temporary, { force: true });
    throw error;
  }
  await file.close();
  const observed = hash.digest("hex");
  if (bytes !== contract.size || observed !== contract.sha256) {
    await rm(temporary, { force: true });
    throw new Error(`${url} failed the pinned model size or digest check`);
  }
  try {
    if (!(await valid(temporary, contract))) {
      throw new Error(`${url} failed verification after closing the staged file`);
    }
    await publishNoReplace(temporary, destination, contract);
  } catch (error) {
    await rm(temporary, { force: true });
    throw error;
  }
}

async function downloadWithRetry(url, destination, contract, failures) {
  for (let attempt = 1; attempt <= downloadAttemptsPerSource; attempt += 1) {
    try {
      await download(url, destination, contract);
      return true;
    } catch (error) {
      failures.push({ attempt, error, url });
      if (attempt < downloadAttemptsPerSource) {
        await delay(downloadBackoffMs * attempt);
      }
    }
  }
  return false;
}

function aggregatedDownloadError(failures) {
  const details = failures
    .map(({ attempt, error, url }) => `${url} attempt ${attempt}: ${error?.message ?? error}`)
    .join("; ");
  return new AggregateError(
    failures.map(({ error }) => error),
    `model download failed after ${failures.length} attempts: ${details}`,
  );
}

async function destinationMetadata(path) {
  try {
    return await lstat(path);
  } catch (error) {
    if (error?.code === "ENOENT") return undefined;
    throw error;
  }
}

function pathIdentity(path) {
  const resolved = resolve(path);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

async function ensureRealDirectoryTree(directory, label) {
  const resolved = resolve(directory);
  const root = parse(resolved).root;
  const components = relative(root, resolved).split(sep).filter(Boolean);
  let cursor = root;
  for (const component of components) {
    cursor = resolve(cursor, component);
    try {
      await mkdir(cursor);
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
    }
    const metadata = await lstat(cursor);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error(`${label} must not traverse symbolic links, reparse points, or files: ${cursor}`);
    }
    const canonical = await realpath(cursor);
    if (pathIdentity(canonical) !== pathIdentity(cursor)) {
      throw new Error(`${label} must have real directory ancestry: ${cursor}`);
    }
  }
  return resolved;
}

async function requireAbsentDestination(path) {
  if (await destinationMetadata(path)) {
    throw new Error(`refusing to overwrite model destination: ${path}`);
  }
}

async function publishNoReplace(temporary, destination, contract) {
  await requireAbsentDestination(destination);
  try {
    await link(temporary, destination);
  } catch (error) {
    if (error?.code === "EEXIST") {
      throw new Error(`refusing to overwrite model destination: ${destination}`);
    }
    throw error;
  }
  try {
    await rm(temporary);
  } catch (error) {
    await rm(destination, { force: true });
    throw error;
  }
  if (!(await valid(destination, contract))) {
    await rm(destination, { force: true });
    throw new Error(`published model failed verification: ${destination}`);
  }
}

const contract = await loadContract(resolve(argument("--contract") ?? defaultContract));
const outputArgument = argument("--output");
const cacheRootArgument = argument("--cache-root");
if (outputArgument && cacheRootArgument) {
  throw new Error("--output and --cache-root are mutually exclusive");
}
const cacheRoot = cacheRootArgument
  ? await ensureRealDirectoryTree(cacheRootArgument, "model cache root")
  : undefined;
const destination = resolve(
  outputArgument ??
    (cacheRoot
      ? resolve(cacheRoot, "sha256", contract.sha256, contract.fileName)
      : `target/build-assets/sha256/${contract.sha256}/${contract.fileName}`),
);
if (cacheRoot) {
  await ensureRealDirectoryTree(dirname(destination), "model cache path");
} else {
  await mkdir(dirname(destination), { recursive: true });
}
const existingDestination = await destinationMetadata(destination);
if (existingDestination && !existingDestination.isFile()) {
  throw new Error(`model destination is not a regular file: ${destination}`);
}
if (!(await valid(destination, contract))) {
  await rm(destination, { force: true });
  const source = argument("--source");
  if (source) {
    await publishSource(resolve(source), destination, contract);
  } else if (process.argv.includes("--offline")) {
    throw new Error(`offline model preparation requires a valid --source or existing ${destination}`);
  } else {
    const failures = [];
    let published = false;
    for (const url of contract.urls) {
      if (await downloadWithRetry(url, destination, contract, failures)) {
        published = true;
        break;
      }
    }
    if (!published) throw aggregatedDownloadError(failures);
  }
}
if (cacheRoot) {
  await ensureRealDirectoryTree(dirname(destination), "model cache path");
}

if (process.env.GITHUB_ENV) {
  await appendFile(process.env.GITHUB_ENV, `CODESTORY_EMBED_MODEL_SOURCE=${destination}\n`);
}
console.log(destination);
