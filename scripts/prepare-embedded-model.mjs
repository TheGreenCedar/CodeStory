#!/usr/bin/env node

import { createHash } from "node:crypto";
import { appendFile, copyFile, mkdir, open, rename, rm, stat } from "node:fs/promises";
import { Readable } from "node:stream";
import { dirname, resolve } from "node:path";

const FILE_NAME = "bge-base-en-v1.5.Q8_0.gguf";
const SIZE = 117_974_304;
const SHA256 = "ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c";
const URLS = [
  "https://huggingface.co/BAAI/bge-base-en-v1.5-GGUF/resolve/main/bge-base-en-v1.5.Q8_0.gguf",
  "https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q8_0.gguf",
];

function argument(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
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

async function valid(path) {
  try {
    return (await stat(path)).size === SIZE && (await digest(path)) === SHA256;
  } catch {
    return false;
  }
}

async function publishSource(source, destination) {
  if (!(await valid(source))) throw new Error(`invalid pinned model source: ${source}`);
  const temporary = `${destination}.${process.pid}.partial`;
  await rm(temporary, { force: true });
  await copyFile(source, temporary);
  await rename(temporary, destination);
}

async function download(url, destination) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`${url} returned HTTP ${response.status}`);
  }
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared !== SIZE) {
    throw new Error(`${url} declared ${declared} bytes; expected ${SIZE}`);
  }
  const temporary = `${destination}.${process.pid}.partial`;
  const file = await open(temporary, "wx");
  const hash = createHash("sha256");
  let bytes = 0;
  try {
    for await (const chunk of Readable.fromWeb(response.body)) {
      bytes += chunk.length;
      if (bytes > SIZE) throw new Error(`${url} exceeded the pinned ${SIZE}-byte size`);
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
  if (bytes !== SIZE || observed !== SHA256) {
    await rm(temporary, { force: true });
    throw new Error(`${url} failed the pinned model size or digest check`);
  }
  await rename(temporary, destination);
}

const destination = resolve(
  argument("--output") ?? `target/build-assets/sha256/${SHA256}/${FILE_NAME}`,
);
await mkdir(dirname(destination), { recursive: true });
if (!(await valid(destination))) {
  await rm(destination, { force: true });
  const source = argument("--source");
  if (source) {
    await publishSource(resolve(source), destination);
  } else {
    let lastError;
    for (const url of URLS) {
      try {
        await download(url, destination);
        lastError = undefined;
        break;
      } catch (error) {
        lastError = error;
      }
    }
    if (lastError) throw lastError;
  }
}

if (process.env.GITHUB_ENV) {
  await appendFile(process.env.GITHUB_ENV, `CODESTORY_EMBED_MODEL_SOURCE=${destination}\n`);
}
console.log(destination);
