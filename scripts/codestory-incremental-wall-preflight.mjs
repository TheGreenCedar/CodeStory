#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdir, readFile, realpath, stat, writeFile } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { withExactSourceMutation } from "./codestory-agent-ab-benchmark.mjs";

const MAX_CAPTURE_BYTES = 1024 * 1024;
const MAX_RECEIPT_BYTES = 16 * 1024 * 1024;
const PROCESS_TIMEOUT_MS = 10 * 60 * 1000;

function fail(message) {
  throw new Error(message);
}

function parseOptions(argv) {
  const names = new Set(["--cli", "--project", "--cache-dir", "--source", "--out-dir", "--repeats"]);
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    const value = argv[index + 1];
    if (!names.has(name) || !value || value.startsWith("--")) fail(`invalid option ${name ?? "<missing>"}`);
    values[name.slice(2).replaceAll("-", "_")] = value;
  }
  for (const name of ["cli", "project", "cache_dir", "source", "out_dir"]) {
    if (!isAbsolute(values[name] ?? "")) fail(`--${name.replaceAll("_", "-")} must be absolute`);
  }
  const repeats = Number(values.repeats);
  if (!Number.isInteger(repeats) || repeats < 1 || repeats > 20) fail("--repeats must be an integer from 1 to 20");
  return { ...values, repeats };
}

function inside(root, path) {
  const value = relative(root, path);
  return value !== ".." && !value.startsWith(`..${sep}`) && !isAbsolute(value);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function readBoundedJson(path) {
  const metadata = await stat(path);
  if (!metadata.isFile() || metadata.size < 1 || metadata.size > MAX_RECEIPT_BYTES) {
    fail(`${path} is not a bounded regular receipt`);
  }
  return JSON.parse(await readFile(path, "utf8"));
}

async function runCli(cli, args) {
  return new Promise((accept, reject) => {
    const started = performance.now();
    const child = spawn(cli, args, { stdio: ["ignore", "pipe", "pipe"] });
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGTERM");
    }, PROCESS_TIMEOUT_MS);
    const collect = (chunks, field) => (chunk) => {
      const bytes = Buffer.from(chunk);
      if (field === "stdout") stdoutBytes += bytes.length;
      else stderrBytes += bytes.length;
      if ((field === "stdout" ? stdoutBytes : stderrBytes) <= MAX_CAPTURE_BYTES) chunks.push(bytes);
    };
    child.stdout.on("data", collect(stdout, "stdout"));
    child.stderr.on("data", collect(stderr, "stderr"));
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      accept({
        code,
        signal,
        timed_out: timedOut,
        wall_ms: Math.round((performance.now() - started) * 1000) / 1000,
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      });
    });
  });
}

function validateWallReceipt(receipt, expectedPath) {
  const wall = receipt?.incremental_wall_receipt;
  if (wall?.contract !== "codestory.incremental-wall-receipt/v1") fail("incremental wall receipt is missing");
  if (wall.accounted_ms !== wall.total_ms || wall.reconciliation_basis_points !== 10_000) {
    fail("incremental wall receipt does not reconcile exactly");
  }
  const scheduled = wall.scheduled_paths?.find(({ path }) => path === expectedPath);
  if (scheduled?.action !== "index" || typeof scheduled?.reason !== "string") {
    fail(`incremental receipt did not schedule ${expectedPath} with an index reason`);
  }
  return wall;
}

async function runIndex(options, outputPath) {
  const result = await runCli(options.cli, [
    "retrieval", "index",
    "--project", options.project,
    "--cache-dir", options.cache_dir,
    "--refresh", "incremental",
    "--format", "json",
    "--output-file", outputPath,
  ]);
  if (result.code !== 0 || result.timed_out) {
    fail(`incremental index failed: ${result.stderr.trim() || result.stdout.trim() || `exit ${result.code}`}`);
  }
  return { process: result, receipt: await readBoundedJson(outputPath) };
}

export async function runIncrementalWallPreflight(options) {
  const project = await realpath(options.project);
  const source = await realpath(options.source);
  const cli = await realpath(options.cli);
  const sourceMetadata = await stat(source);
  const cliMetadata = await stat(cli);
  if (!inside(project, source) || !sourceMetadata.isFile()) fail("--source must be a regular file inside --project");
  if (!cliMetadata.isFile()) fail("--cli must be a regular file");
  const relativeSource = relative(project, source).split(sep).join("/");
  await mkdir(options.cache_dir, { recursive: true });
  await mkdir(options.out_dir, { recursive: true });
  const originalBytes = await readFile(source);
  const originalSha256 = sha256(originalBytes);
  const rows = [];
  for (let repeat = 1; repeat <= options.repeats; repeat += 1) {
    const mutatedOutput = resolve(options.out_dir, `repeat-${repeat}-mutated.json`);
    const restoreOutput = resolve(options.out_dir, `repeat-${repeat}-restored.json`);
    const mutation = await withExactSourceMutation(source, async (identity) => {
      const run = await runIndex({ ...options, project, source, cli }, mutatedOutput);
      const wall = validateWallReceipt(run.receipt, relativeSource);
      return { identity, run, wall };
    }, async () => {
      const restore = await runIndex({ ...options, project, source, cli }, restoreOutput);
      validateWallReceipt(restore.receipt, relativeSource);
    });
    rows.push({
      repeat,
      source_path: relativeSource,
      mutation: "append_one_lf_v1",
      original_sha256: mutation.original_sha256,
      mutated_sha256: mutation.mutated_sha256,
      restored_sha256: mutation.restored_sha256,
      process_wall_ms: mutation.result.run.process.wall_ms,
      command_wall_ms: mutation.result.wall.total_ms,
      core_refresh_ms: mutation.result.wall.core_refresh_ms,
      retrieval_finalize_ms: mutation.result.wall.retrieval_finalize_ms,
      reconciliation_basis_points: mutation.result.wall.reconciliation_basis_points,
      attributed_basis_points: mutation.result.wall.attributed_basis_points,
      phases: mutation.result.wall.phases,
      scheduled_paths: mutation.result.wall.scheduled_paths,
      receipt_path: mutatedOutput,
      restore_receipt_path: restoreOutput,
    });
  }
  const restoredBytes = await readFile(source);
  if (sha256(restoredBytes) !== originalSha256 || !restoredBytes.equals(originalBytes)) {
    fail("source was not restored byte-for-byte after preflight");
  }
  const summary = {
    contract: "codestory.incremental-wall-preflight/v1",
    cli,
    cli_sha256: sha256(await readFile(cli)),
    project,
    source_path: relativeSource,
    source_sha256: originalSha256,
    repeats: rows,
  };
  const summaryPath = resolve(options.out_dir, "summary.json");
  await writeFile(summaryPath, `${JSON.stringify(summary, null, 2)}\n`, { flag: "wx" });
  return { summary, summaryPath };
}

async function main() {
  const result = await runIncrementalWallPreflight(parseOptions(process.argv.slice(2)));
  process.stdout.write(`${result.summaryPath}\n`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main().catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`);
    process.exitCode = 1;
  });
}
