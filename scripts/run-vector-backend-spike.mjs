#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, openSync, closeSync, fsyncSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const root = resolve(process.cwd());
const binary = process.env.CODESTORY_VECTOR_SPIKE_BINARY ?? join(root, "target", "release", "vector_backend_spike");
const source = required("CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE");
const fixture = required("CODESTORY_VECTOR_SPIKE_FIXTURE_JSON");
const output = resolve(required("CODESTORY_VECTOR_SPIKE_OUTPUT_ROOT"));
const counts = [1000, 10000, 25000, 100000];
const blocks = 6;
const timeoutMs = Number(process.env.CODESTORY_VECTOR_SPIKE_TIMEOUT_MS ?? 20 * 60_000);

if (existsSync(output)) throw new Error(`evidence root must be new: ${output}`);
mkdirSync(output, { recursive: true });
const input = {
  source,
  source_sha256: sha256(readFileSync(source)),
  fixture,
  fixture_sha256: sha256(readFileSync(fixture)),
  binary,
  started_at: new Date().toISOString(),
  counts,
  blocks,
  order: "six paired blocks: sqlite-vec/usearch then usearch/sqlite-vec, repeated three times",
};
atomicJson(join(output, "input.json"), input);

for (const cleanRoot of ["clean-a", "clean-b"]) {
  const clean = join(output, cleanRoot);
  mkdirSync(clean, { recursive: true });
  const journal = join(clean, "journal.jsonl");
  for (const count of counts) {
    const oracle = join(clean, `oracle-${count}.json`);
    atomicJson(oracle, await invoke(["oracle", "--source", source, "--fixture", fixture, "--count", String(count)]));
    for (let block = 0; block < blocks; block += 1) {
      const order = block % 2 === 0 ? ["sqlite-vec", "usearch"] : ["usearch", "sqlite-vec"];
      for (const [position, backend] of order.entries()) {
        const workdir = join(clean, "work", `${count}`, `block-${block + 1}`, backend);
        const event = { clean_root: cleanRoot, count, block: block + 1, order_position: position + 1, backend, started_at: new Date().toISOString() };
        appendJournal(journal, { ...event, status: "started" });
        try {
          const result = await invoke(["candidate", "--source", source, "--fixture", fixture, "--oracle", oracle, "--count", String(count), "--backend", backend, "--workdir", workdir]);
          const artifact = join(clean, "observations", `${count}`, `block-${block + 1}-${position + 1}-${backend}.json`);
          atomicJson(artifact, { ...event, ...result, completed_at: new Date().toISOString() });
          appendJournal(journal, { ...event, status: "complete", artifact, result_sha256: sha256(readFileSync(artifact)) });
        } catch (error) {
          appendJournal(journal, { ...event, status: "failed", error: String(error), failed_at: new Date().toISOString() });
          throw error;
        }
      }
    }
  }
  atomicJson(join(clean, "complete.json"), { clean_root: cleanRoot, completed_at: new Date().toISOString(), journal_sha256: sha256(readFileSync(journal)) });
}

atomicJson(join(output, "complete.json"), { completed_at: new Date().toISOString(), input_sha256: sha256(readFileSync(join(output, "input.json"))), disposition: "measurements complete; backend selection remains prohibited until all #1202 acceptance evidence is independently reviewed" });

function required(name) { const value = process.env[name]; if (!value) throw new Error(`${name} is required`); return value; }
function sha256(bytes) { return createHash("sha256").update(bytes).digest("hex"); }
function atomicJson(path, value) { mkdirSync(dirname(path), { recursive: true }); const temporary = `${path}.${process.pid}.${Date.now()}.tmp`; writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx" }); const fd = openSync(temporary, "r"); fsyncSync(fd); closeSync(fd); renameSync(temporary, path); const parent = openSync(dirname(path), "r"); fsyncSync(parent); closeSync(parent); }
function appendJournal(path, value) { mkdirSync(dirname(path), { recursive: true }); const fd = openSync(path, "a"); writeFileSync(fd, `${JSON.stringify(value)}\n`); fsyncSync(fd); closeSync(fd); }
function invoke(args) { return new Promise((resolvePromise, reject) => { const child = spawn(binary, args, { cwd: root, detached: true, stdio: ["ignore", "pipe", "pipe"] }); let stdout = ""; let stderr = ""; child.stdout.on("data", chunk => { stdout += chunk; }); child.stderr.on("data", chunk => { stderr += chunk; }); const timer = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} }, timeoutMs); child.on("error", error => { clearTimeout(timer); reject(error); }); child.on("close", code => { clearTimeout(timer); if (code !== 0) return reject(new Error(`${args[0]} exited ${code}: ${stderr.trim()}`)); try { resolvePromise(JSON.parse(stdout)); } catch (error) { reject(new Error(`${args[0]} emitted invalid JSON: ${error}; stderr: ${stderr.trim()}`)); } }); }); }
