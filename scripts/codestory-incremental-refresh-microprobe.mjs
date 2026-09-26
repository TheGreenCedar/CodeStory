#!/usr/bin/env node

import { createHash, randomUUID } from "node:crypto";
import { readFile, realpath, stat, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { spawn } from "node:child_process";

const MAX_SOURCE_BYTES = 16 * 1024 * 1024;
const MAX_PROCESS_OUTPUT_BYTES = 8 * 1024 * 1024;
const PROCESS_TIMEOUT_MS = 5 * 60 * 1000;

function usage() {
  return [
    "Usage:",
    "  node scripts/codestory-incremental-refresh-microprobe.mjs \\",
    "    --cli <codestory-cli> --project <repo> --source <project-relative-path> \\",
    "    --cache-dir <isolated-cache-dir> [--repeats 5] \\",
    "    [--transport fresh-cli|persistent-mcp] [--query <search-query>]",
  ].join("\n");
}

function parseArgs(argv) {
  const values = { repeats: 5, transport: "fresh-cli", query: "server" };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      process.stdout.write(`${usage()}\n`);
      process.exit(0);
    }
    if (!arg.startsWith("--")) throw new Error(`unexpected argument: ${arg}`);
    const key = arg.slice(2).replaceAll("-", "_");
    const value = argv[index + 1];
    if (value == null || value.startsWith("--")) throw new Error(`${arg} requires a value`);
    values[key] = value;
    index += 1;
  }
  for (const required of ["cli", "project", "source", "cache_dir"]) {
    if (!values[required]) throw new Error(`--${required.replaceAll("_", "-")} is required`);
  }
  values.repeats = Number.parseInt(values.repeats, 10);
  if (!Number.isInteger(values.repeats) || values.repeats < 1 || values.repeats > 20) {
    throw new Error("--repeats must be an integer from 1 through 20");
  }
  if (!new Set(["fresh-cli", "persistent-mcp"]).has(values.transport)) {
    throw new Error("--transport must be fresh-cli or persistent-mcp");
  }
  if (typeof values.query !== "string" || values.query.trim() === "") {
    throw new Error("--query must be non-empty");
  }
  return values;
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function percentile(values, probability) {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.max(0, Math.ceil(probability * ordered.length) - 1)];
}

async function runCli(cli, args, cacheDir) {
  return await new Promise((resolve, reject) => {
    const started = performance.now();
    const child = spawn(cli, args, {
      env: {
        ...process.env,
        CODESTORY_CACHE_ROOT: cacheDir,
        CODESTORY_STDIO_CACHE_ROOT: cacheDir,
        CODESTORY_LOG_CORRELATION_ID: randomUUID(),
        RUST_LOG: process.env.RUST_LOG ?? "codestory::activation=warn",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    let outputBytes = 0;
    const timeout = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`codestory-cli exceeded ${PROCESS_TIMEOUT_MS}ms`));
    }, PROCESS_TIMEOUT_MS);
    const collect = (target) => (chunk) => {
      outputBytes += chunk.length;
      if (outputBytes > MAX_PROCESS_OUTPUT_BYTES) {
        child.kill("SIGKILL");
        reject(new Error(`codestory-cli output exceeded ${MAX_PROCESS_OUTPUT_BYTES} bytes`));
        return;
      }
      target.push(chunk);
    };
    child.stdout.on("data", collect(stdout));
    child.stderr.on("data", collect(stderr));
    child.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timeout);
      const wallMs = Math.round((performance.now() - started) * 1000) / 1000;
      const stdoutText = Buffer.concat(stdout).toString("utf8");
      const stderrText = Buffer.concat(stderr).toString("utf8");
      if (code !== 0) {
        reject(new Error(
          `codestory-cli failed with code=${code} signal=${signal ?? "none"}: `
          + `${stderrText || stdoutText}`.slice(-4000),
        ));
        return;
      }
      let payload;
      try {
        payload = JSON.parse(stdoutText);
      } catch (error) {
        reject(new Error(`codestory-cli returned invalid JSON: ${error.message}`));
        return;
      }
      resolve({
        wall_ms: wallMs,
        payload,
      });
    });
  });
}

function boundedDelay(ms) {
  return new Promise((resolve) => setTimeout(resolve, Math.max(0, Math.min(ms, 5_000))));
}

class PersistentMcpClient {
  constructor(child, cacheDir, correlationId) {
    this.child = child;
    this.cacheDir = cacheDir;
    this.correlationId = correlationId;
    this.nextId = 1;
    this.pending = new Map();
    this.stderr = [];
    this.outputBytes = 0;
    this.closed = false;

    const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
    lines.on("line", (line) => this.onLine(line));
    child.stderr.on("data", (chunk) => this.collectStderr(chunk));
    child.on("error", (error) => this.failAll(error));
    child.on("close", (code, signal) => {
      this.closed = true;
      if (code !== 0 && code !== null) {
        this.failAll(new Error(
          `persistent MCP closed with code=${code} signal=${signal ?? "none"}: `
          + Buffer.concat(this.stderr).toString("utf8").slice(-4_000),
        ));
      } else {
        this.failAll(new Error("persistent MCP closed before responding"));
      }
    });
  }

  static async start(cli, project, cacheDir) {
    const started = performance.now();
    const correlationId = randomUUID();
    const child = spawn(cli, [
      "serve",
      "--stdio",
      "--project",
      project,
      "--cache-dir",
      cacheDir,
      "--refresh",
      "none",
    ], {
      env: {
        ...process.env,
        CODESTORY_CACHE_ROOT: cacheDir,
        CODESTORY_STDIO_CACHE_ROOT: cacheDir,
        CODESTORY_LOG: process.env.CODESTORY_LOG ?? "warn",
        CODESTORY_LOG_CORRELATION_ID: correlationId,
        RUST_LOG: process.env.RUST_LOG ?? "codestory::activation=warn",
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const client = new PersistentMcpClient(child, cacheDir, correlationId);
    await client.request("initialize", {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "codestory-incremental-refresh-microprobe", version: "1" },
    });
    client.notify("notifications/initialized", {});
    return {
      client,
      startup_ms: Math.round((performance.now() - started) * 1000) / 1000,
    };
  }

  collectStderr(chunk) {
    this.outputBytes += chunk.length;
    if (this.outputBytes > MAX_PROCESS_OUTPUT_BYTES) {
      this.child.kill("SIGKILL");
      this.failAll(new Error(`persistent MCP output exceeded ${MAX_PROCESS_OUTPUT_BYTES} bytes`));
      return;
    }
    this.stderr.push(chunk);
  }

  onLine(line) {
    this.outputBytes += Buffer.byteLength(line) + 1;
    if (this.outputBytes > MAX_PROCESS_OUTPUT_BYTES) {
      this.child.kill("SIGKILL");
      this.failAll(new Error(`persistent MCP output exceeded ${MAX_PROCESS_OUTPUT_BYTES} bytes`));
      return;
    }
    let payload;
    try {
      payload = JSON.parse(line);
    } catch (error) {
      this.child.kill("SIGKILL");
      this.failAll(new Error(`persistent MCP returned invalid JSON: ${error.message}`));
      return;
    }
    const pending = this.pending.get(String(payload.id));
    if (!pending) return;
    this.pending.delete(String(payload.id));
    clearTimeout(pending.timeout);
    if (payload.error) {
      pending.reject(new Error(`persistent MCP JSON-RPC error: ${JSON.stringify(payload.error)}`));
      return;
    }
    pending.resolve(payload.result);
  }

  request(method, params) {
    if (this.closed) return Promise.reject(new Error("persistent MCP is closed"));
    const id = String(this.nextId++);
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        this.child.kill("SIGKILL");
        reject(new Error(`persistent MCP ${method} exceeded ${PROCESS_TIMEOUT_MS}ms`));
      }, PROCESS_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timeout });
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    });
  }

  notify(method, params) {
    if (!this.closed) {
      this.child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
    }
  }

  async callUntilReady(project, query, previousPublication = null) {
    const started = performance.now();
    const activationReceiptCursor = (await this.activationReceipts()).length;
    let attempts = 0;
    let preparingResponses = 0;
    let unchangedReadyResponses = 0;
    let staleRefusals = 0;
    const requestAttempts = [];
    while ((performance.now() - started) < PROCESS_TIMEOUT_MS) {
      attempts += 1;
      const requestStarted = performance.now();
      const result = await this.request("tools/call", {
        name: "search",
        arguments: { project, query, repo_text: "off", limit: 1 },
      });
      const content = result?.structuredContent;
      const preparing = content?.kind === "preparing"
        || content?.code === "codestory_preparing";
      requestAttempts.push({
        request_ms: Math.round((performance.now() - requestStarted) * 1000) / 1000,
        disposition: preparing
          ? "preparing"
          : (result?.isError === true ? "error" : content?.kind ?? "unknown"),
        retry_after_ms: content?.retry_after_ms ?? null,
      });
      if (preparing) {
        preparingResponses += 1;
        await boundedDelay(Number(content?.retry_after_ms ?? 50));
        continue;
      }
      if (result?.isError === true && previousPublication) {
        const detail = JSON.stringify(result);
        if (detail.includes("fresh complete core publication")
          || detail.includes("publication_changed")) {
          staleRefusals += 1;
          await boundedDelay(25);
          continue;
        }
      }
      if (result?.isError === true) {
        throw new Error(`persistent MCP search failed: ${JSON.stringify(result)}`);
      }
      if (content?.kind !== "complete" || content?.retrieval?.state !== "full") {
        throw new Error(`persistent MCP search returned no ready structured content: ${JSON.stringify(result)}`);
      }
      const coreGeneration = content?.publication?.core?.generation_id ?? null;
      const coreRun = content?.publication?.core?.run_id ?? null;
      const retrievalCoreGeneration = content?.publication?.retrieval?.core_generation_id ?? null;
      const retrievalCoreRun = content?.publication?.retrieval?.core_run_id ?? null;
      if (coreGeneration !== retrievalCoreGeneration || coreRun !== retrievalCoreRun) {
        throw new Error(`persistent MCP returned an incoherent core/retrieval pair: ${JSON.stringify(content.publication)}`);
      }
      if (previousPublication
        && coreGeneration === previousPublication.core_generation
        && coreRun === previousPublication.core_run) {
        unchangedReadyResponses += 1;
        await boundedDelay(25);
        continue;
      }
      const activationReceipts = (await this.activationReceipts()).slice(activationReceiptCursor);
      const refreshReceipt = activationReceipts.at(-1) ?? null;
      return {
        whole_search_wall_ms: Math.round((performance.now() - started) * 1000) / 1000,
        refresh_ms: refreshReceipt?.total_ms ?? null,
        refresh_receipt: refreshReceipt,
        attempts,
        preparing_responses: preparingResponses,
        unchanged_ready_responses: unchangedReadyResponses,
        stale_refusals: staleRefusals,
        request_attempts: requestAttempts,
        retrieval_generation: content?.publication?.retrieval?.retrieval_generation ?? null,
        core_generation: coreGeneration,
        core_run: coreRun,
      };
    }
    throw new Error(`persistent MCP search did not become ready within ${PROCESS_TIMEOUT_MS}ms`);
  }

  async activationReceipts() {
    const diagnosticsPath = path.join(this.cacheDir, "diagnostics", "codestory.jsonl");
    let body;
    try {
      body = await readFile(diagnosticsPath, "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") return [];
      throw error;
    }
    const receipts = [];
    for (const line of body.split(/\r?\n/u)) {
      if (line === "") continue;
      let record;
      try {
        record = JSON.parse(line);
      } catch {
        continue;
      }
      const fields = record?.fields;
      if (record?.correlation_id !== this.correlationId
        || !record?.code_file?.endsWith("crates/codestory-runtime/src/services.rs")
        || !Number.isInteger(fields?.total_ms)
        || !Number.isInteger(fields?.preflight_ms)
        || !Number.isInteger(fields?.core_refresh_ms)
        || !Number.isInteger(fields?.retrieval_finalization_ms)
        || !Number.isInteger(fields?.validation_ms)) {
        continue;
      }
      receipts.push({
        preflight_ms: fields.preflight_ms,
        core_refresh_ms: fields.core_refresh_ms,
        search_preparation_ms: fields.search_preparation_ms,
        dense_preparation_ms: fields.dense_preparation_ms,
        retrieval_finalization_ms: fields.retrieval_finalization_ms,
        validation_ms: fields.validation_ms,
        source_validation_mode: fields.source_validation_mode,
        unattributed_ms: fields.unattributed_ms,
        total_ms: fields.total_ms,
      });
    }
    return receipts;
  }

  failAll(error) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
    }
    this.pending.clear();
  }

  async close() {
    if (this.closed) return;
    this.child.stdin.end();
    await Promise.race([
      new Promise((resolve) => this.child.once("close", resolve)),
      boundedDelay(1_000).then(() => {
        if (!this.closed) this.child.kill("SIGTERM");
      }),
    ]);
    if (!this.closed) this.child.kill("SIGKILL");
  }

}

function incrementalArgs(project, cacheDir) {
  return [
    "retrieval",
    "index",
    "--project",
    project,
    "--cache-dir",
    cacheDir,
    "--profile",
    "agent",
    "--refresh",
    "incremental",
    "--format",
    "json",
  ];
}

function compactRun(repeat, run) {
  return {
    repeat,
    wall_ms: run.wall_ms,
    manifest_generation: run.payload?.manifest?.sidecar_generation ?? null,
    core_phase_timings: run.payload?.core_phase_timings ?? null,
    retrieval_phase_timings: run.payload?.retrieval_phase_timings ?? null,
    retrieval_component_work: run.payload?.retrieval_component_work ?? null,
  };
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const cli = await realpath(path.resolve(opts.cli));
  const project = await realpath(path.resolve(opts.project));
  const cacheDir = path.resolve(opts.cache_dir);
  const requestedSource = path.resolve(project, opts.source);
  const source = await realpath(requestedSource);
  const relativeSource = path.relative(project, source);
  if (relativeSource === "" || relativeSource.startsWith(`..${path.sep}`) || path.isAbsolute(relativeSource)) {
    throw new Error("--source must resolve to a file inside --project");
  }
  const sourceStat = await stat(source);
  if (!sourceStat.isFile()) throw new Error("--source must resolve to a regular file");
  const original = await readFile(source);
  if (original.length > MAX_SOURCE_BYTES) {
    throw new Error(`source exceeds ${MAX_SOURCE_BYTES} bytes`);
  }
  const mutation = Buffer.concat([original, Buffer.from("\n", "utf8")]);
  const originalSha256 = sha256(original);
  const mutatedSha256 = sha256(mutation);
  const runs = [];
  let activeClient = null;
  let interruptedSignal = null;
  let restorePromise = null;
  const restoreExactSource = () => {
    if (restorePromise == null) {
      restorePromise = (async () => {
        await writeFile(source, original);
        if (sha256(await readFile(source)) !== originalSha256) {
          throw new Error("signal cleanup did not restore the exact source bytes");
        }
      })();
    }
    return restorePromise;
  };
  const interrupt = (signal) => {
    if (interruptedSignal != null) return;
    interruptedSignal = signal;
    void (async () => {
      try {
        await restoreExactSource();
        await activeClient?.close();
      } catch (error) {
        process.stderr.write(`interrupt cleanup failed: ${error.stack ?? error.message}\n`);
      }
      process.exitCode = signal === "SIGINT" ? 130 : 143;
    })();
  };
  process.once("SIGINT", () => interrupt("SIGINT"));
  process.once("SIGTERM", () => interrupt("SIGTERM"));

  if (opts.transport === "persistent-mcp") {
    const { client, startup_ms: startupMs } = await PersistentMcpClient.start(
      cli,
      project,
      cacheDir,
    );
    activeClient = client;
    let warm;
    try {
      warm = await client.callUntilReady(project, opts.query);
      for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
        let mutationWritten = false;
        let mutatedRun;
        let primaryError;
        try {
          await writeFile(source, mutation);
          mutationWritten = true;
          if (sha256(await readFile(source)) !== mutatedSha256) {
            throw new Error(`repeat ${repeat} did not observe the exact append-LF mutation`);
          }
          mutatedRun = await client.callUntilReady(project, opts.query, warm);
          runs.push({ repeat, ...mutatedRun });
        } catch (error) {
          primaryError = error;
        } finally {
          if (mutationWritten) await writeFile(source, original);
          if (sha256(await readFile(source)) !== originalSha256) {
            throw new Error(`repeat ${repeat} did not restore the exact source bytes`);
          }
          if (mutationWritten) {
            try {
              warm = await client.callUntilReady(project, opts.query, mutatedRun ?? null);
            } catch (restoreError) {
              if (primaryError) {
                primaryError.message += `; restore refresh also failed: ${restoreError.message}`;
              } else {
                throw restoreError;
              }
            }
          }
        }
        if (primaryError) throw primaryError;
      }
    } finally {
      await restoreExactSource();
      await client.close();
      activeClient = null;
    }
    if (interruptedSignal != null) return;
    const refresh = runs.map((run) => run.refresh_ms);
    if (refresh.some((elapsed) => !Number.isFinite(elapsed))) {
      throw new Error("persistent MCP refresh omitted its activation wall receipt");
    }
    process.stdout.write(`${JSON.stringify({
      contract: "codestory.incremental-refresh-microprobe/v1",
      transport: "persistent_stdio_project_bound",
      cli,
      project,
      cache_dir: cacheDir,
      source: relativeSource.split(path.sep).join("/"),
      query: opts.query,
      mutation: "append_one_lf_v1",
      original_sha256: originalSha256,
      mutated_sha256: mutatedSha256,
      startup_ms: startupMs,
      warm,
      repeats: runs.length,
      p50_ms: percentile(refresh, 0.5),
      p95_ms: percentile(refresh, 0.95),
      acceptance: {
        p50_lt_2000: percentile(refresh, 0.5) < 2000,
        p95_lt_5000: percentile(refresh, 0.95) < 5000,
      },
      runs,
    }, null, 2)}\n`);
    return;
  }

  for (let repeat = 1; repeat <= opts.repeats; repeat += 1) {
    if (interruptedSignal != null) return;
    let mutatedRun;
    let mutationWritten = false;
    try {
      await writeFile(source, mutation);
      mutationWritten = true;
      if (sha256(await readFile(source)) !== mutatedSha256) {
        throw new Error(`repeat ${repeat} did not observe the exact append-LF mutation`);
      }
      mutatedRun = await runCli(cli, incrementalArgs(project, cacheDir), cacheDir);
    } finally {
      if (mutationWritten) await writeFile(source, original);
      if (sha256(await readFile(source)) !== originalSha256) {
        throw new Error(`repeat ${repeat} did not restore the exact source bytes`);
      }
      if (mutationWritten) {
        await runCli(cli, incrementalArgs(project, cacheDir), cacheDir);
      }
    }
    runs.push(compactRun(repeat, mutatedRun));
  }

  const wall = runs.map((run) => run.wall_ms);
  process.stdout.write(`${JSON.stringify({
    contract: "codestory.incremental-refresh-microprobe/v1",
    transport: "fresh_cli",
    cli,
    project,
    cache_dir: cacheDir,
    source: relativeSource.split(path.sep).join("/"),
    mutation: "append_one_lf_v1",
    original_sha256: originalSha256,
    mutated_sha256: mutatedSha256,
    repeats: runs.length,
    p50_ms: percentile(wall, 0.5),
    p95_ms: percentile(wall, 0.95),
    acceptance: {
      p50_lt_2000: percentile(wall, 0.5) < 2000,
      p95_lt_5000: percentile(wall, 0.95) < 5000,
    },
    runs,
  }, null, 2)}\n`);
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message}\n`);
  process.exitCode = 1;
});
