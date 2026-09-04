import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { maximizeCoveredAtoms, scoreOrder, selectSuccessors, sha256,
  validateVector } from "./lib/etr1-evidence.mjs";

async function main() {
  const { values } = parseArgs({ options: { diagnostic: { type: "string" },
    "state-root": { type: "string" }, "output-dir": { type: "string" } } });
  for (const name of ["diagnostic", "state-root", "output-dir"])
    assert.ok(values[name] && path.isAbsolute(values[name]), `missing absolute --${name}`);
  await mkdir(values["output-dir"], { mode: 0o700 });
  const documents = [
    { id: "seed", text: "pub fn process() { dispatch(); }\n" },
    { id: "target", text: "fn dispatch() { persist(); }\n" },
    { id: "noise-a", text: "fn render_banner() { paint(); }\n" },
    { id: "noise-b", text: "fn parse_flags() { validate(); }\n" },
  ];
  const question = "How does process reach persistence?";
  const input = { contract: "codestory.embedding-diagnostic-input/v1", records: [
    ...documents.map(({ id, text }) => ({ id, purpose: "document", text })),
    { id: "control", purpose: "query", text: question },
    { id: "candidate", purpose: "query", text: `${question}\n\n${documents[0].text}` },
  ] };
  const inputBytes = Buffer.from(JSON.stringify(input)), inputPath = path.join(values["output-dir"], "input.json");
  await writeFile(inputPath, inputBytes, { flag: "wx", mode: 0o600 });
  const vectorPath = path.join(values["state-root"], "canary-vectors.json");
  const execution = spawnSync(values.diagnostic, ["--input", inputPath, "--input-sha256", sha256(inputBytes),
    "--state-root", values["state-root"], "--output", vectorPath],
  { encoding: "utf8", timeout: 180_000, maxBuffer: 4 * 1024 * 1024, env: process.env });
  assert.equal(execution.error, undefined, `embedding diagnostic failed to launch: ${execution.error}`);
  assert.equal(execution.status, 0, `embedding diagnostic failed: ${execution.stderr}`);
  const vectorBytes = await readFile(vectorPath), artifact = JSON.parse(vectorBytes);
  assert.equal(artifact.contract, "codestory.embedding-diagnostic-output/v1");
  assert.equal(artifact.input_sha256, sha256(inputBytes));
  assert.equal(artifact.records.length, input.records.length);
  const vectors = new Map(artifact.records.map((record, index) => {
    assert.equal(record.id, input.records[index].id);
    assert.equal(record.text_sha256, sha256(input.records[index].text));
    validateVector(record.vector, `canary vector ${record.id}`);
    return [record.id, record.vector];
  }));
  const dot = (left, right) => left.reduce((sum, value, index) =>
    Math.fround(sum + Math.fround(Math.fround(value) * Math.fround(right[index]))), 0);
  const frontiers = {};
  for (const arm of ["control", "candidate"]) {
    const scores = documents.map(({ id }) => dot(vectors.get(arm), vectors.get(id)));
    const successors = selectSuccessors(scoreOrder(documents.map(({ id }) => id), scores),
      new Set(["seed"]), new Set());
    frontiers[arm] = { scores, successors, legal_pool: ["seed", ...successors] };
  }
  const frontierBytes = Buffer.from(JSON.stringify(frontiers));
  await writeFile(path.join(values["output-dir"], "frontiers.json"), frontierBytes,
    { flag: "wx", mode: 0o600 });
  // Synthetic truth is constructed only after both frontier outputs are frozen.
  const costs = new Map(documents.map(({ id, text }) => [id, Buffer.byteLength(text) + 128]));
  const evaluated = Object.fromEntries(Object.entries(frontiers).map(([arm, frontier]) => {
    const requirement = frontier.legal_pool.includes("target") ? [["target"]] : [null];
    return [arm, maximizeCoveredAtoms(requirement, costs, 256)];
  }));
  const events = await readFile(path.join(values["state-root"], "ipc",
    `${process.env.CODESTORY_EMBED_QUALIFICATION_NONCE}.events.jsonl`));
  const completed = events.toString("utf8").trimEnd().split("\n").map(JSON.parse)
    .filter((event) => event.action === "completed_tokens");
  assert.ok(completed.length >= 2 && completed.every((event) => Number(event.details.completed_tokens) > 0),
    "canary token completion evidence missing");
  const receipt = { contract: "codestory.etr1-synthetic-canary/v1", experiment_status: "valid",
    packet_decision: "not_evaluated", input_sha256: sha256(inputBytes),
    vectors_sha256: sha256(vectorBytes), frontiers_sha256: sha256(frontierBytes),
    diagnostic_binary_sha256: sha256(await readFile(values.diagnostic)),
    qualification_events_sha256: sha256(events), completed_token_events: completed.length,
    vector_artifact_bytes: (await stat(vectorPath)).size, evaluated };
  const receiptBytes = Buffer.from(`${JSON.stringify(receipt, null, 2)}\n`);
  const output = path.join(values["output-dir"], "receipt.json");
  await writeFile(output, receiptBytes, { flag: "wx", mode: 0o600 });
  console.log(`${sha256(receiptBytes)}  ${output}`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
