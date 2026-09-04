import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile, realpath } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { executeRecorded, fileBinding } from "./lib/etr1-execution.mjs";
import { validateEtr1 } from "./codestory-etr1-validate.mjs";
import { evaluateEtr1 } from "./codestory-etr1-evaluate.mjs";

async function main() {
  const { values } = parseArgs({ options: { diagnostic: { type: "string" },
    runner: { type: "string" }, "output-dir": { type: "string" } } });
  for (const name of ["diagnostic", "runner", "output-dir"])
    assert.ok(values[name] && path.isAbsolute(values[name]), `missing absolute --${name}`);
  await mkdir(values["output-dir"], { mode: 0o700 });
  const root = await realpath(values["output-dir"]);
  const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const project = path.join(root, "repository"), prepared = path.join(root, "prepared");
  await mkdir(project, { mode: 0o700 });
  const source = Array.from({ length: 32 }, (_, i) =>
    `fn commonneedle_${i}() { commonneedle(); ${i === 0 ? "raremarker();" : ""} }\n`).join("");
  await writeFile(path.join(project, "canary.rs"), source, { flag: "wx", mode: 0o600 });
  const git = (...args) => execFileSync("git", ["-C", project, "-c", "core.hooksPath=/dev/null", ...args],
    { encoding: "utf8", stdio: "pipe" });
  git("init", "--quiet"); git("add", "canary.rs");
  git("-c", "user.name=ETR canary", "-c", "user.email=canary@invalid.local", "-c", "commit.gpgsign=false",
    "commit", "--quiet", "-m", "freeze synthetic source");
  execFileSync(values.runner, ["prepare-canary", "--project-root", project, "--output-dir", prepared],
    { encoding: "utf8", stdio: "pipe", timeout: 60_000 });
  const preparationPath = path.join(prepared, "preparation.json");
  const preparation = JSON.parse(await readFile(preparationPath, "utf8"));
  const preparationBinding = await fileBinding(preparationPath);
  const makeState = async (name) => {
    const state = path.join(root, name), ipc = path.join(state, "ipc"), cache = path.join(state, "cache");
    await mkdir(state, { mode: 0o700 });
    await mkdir(ipc, { mode: 0o700 }); await mkdir(cache, { mode: 0o700 });
    const nonce = `etr1-canary-${randomUUID()}`;
    return { state, events: path.join(ipc, `${nonce}.events.jsonl`),
      env: { ...process.env, CODESTORY_CACHE_ROOT: cache, CODESTORY_EMBED_ALLOW_CPU: "false",
        CODESTORY_EMBED_QUALIFICATION_DIR: ipc, CODESTORY_EMBED_QUALIFICATION_NONCE: nonce } };
  };
  const documentState = await makeState("documents"), vectorPath = path.join(documentState.state, "vectors.json");
  const documents = await executeRecorded({ role: "documents", executable: values.diagnostic,
    args: ["--input", preparation.embedding_input.path, "--input-sha256", preparation.embedding_input.sha256,
      "--state-root", documentState.state, "--output", vectorPath],
    inputs: [preparation.embedding_input.path], outputPaths: [vectorPath], eventsPath: documentState.events,
    directory: path.join(root, "document-execution"), sourceRoot, env: documentState.env });
  assert.equal(documents.receipt.experiment_status, "completed", "canary document execution failed");
  const vectors = await fileBinding(vectorPath), queryState = await makeState("queries");
  const runDirectory = path.join(root, "run"), runPath = path.join(runDirectory, "run.json");
  const cancelFile = path.join(root, "cancel");
  const execution = await executeRecorded({ role: "paired_run", executable: values.runner,
    args: ["run", "--prepared", preparationPath, "--prepared-sha256", preparationBinding.sha256,
      "--fragment-vectors", vectorPath, "--fragment-vectors-sha256", vectors.sha256,
      "--document-execution", documents.binding.path, "--document-execution-sha256", documents.binding.sha256,
      "--state-root", queryState.state, "--output-dir", runDirectory, "--cancel-file", cancelFile],
    inputs: [preparationPath, vectorPath, documents.binding.path], outputPaths: [runPath],
    eventsPath: queryState.events, directory: path.join(root, "run-execution"), sourceRoot,
    env: queryState.env, cancelFile });
  assert.equal(execution.receipt.experiment_status, "completed", "canary paired execution failed");
  const runBinding = await fileBinding(runPath);
  const validated = await validateEtr1({ runBinding, sourceRoot, executionBinding: execution.binding, allowCanary: true });
  const validationPath = path.join(root, "validation.json");
  await writeFile(validationPath, JSON.stringify({ contract: "codestory.etr1-validation/v1",
    authority: "synthetic_canary_only", experiment_status: "valid", decision: "not_evaluated",
    annotation_access: "not_accessed", run: runBinding, execution: execution.binding,
    binary_sha256: validated.run.build.binary_sha256 }), { flag: "wx", mode: 0o600 });
  // Synthetic truth enters only after the real paired run and validator finish.
  const first = preparation.fragments[0];
  const annotationsPath = path.join(root, "annotations.json");
  const annotations = { authority: "synthetic_canary_only", cases: preparation.wordings.map((row) => ({
    case_id: row.case_id, acceptable_sets: [{ set_id: "first-fragment", required_relation_atoms: [],
      required_source_atoms: [{ atom_id: "first", source_range: { path: first.path,
        content_digest: first.content_digest, byte_range: first.byte_range, line_range: first.line_range } }] }] })) };
  await writeFile(annotationsPath, JSON.stringify(annotations), { flag: "wx", mode: 0o600 });
  const validation = await fileBinding(validationPath), annotationBinding = await fileBinding(annotationsPath);
  const evaluated = await evaluateEtr1({ validationPath, validationSha256: validation.sha256,
    annotationsPath, annotationsSha256: annotationBinding.sha256, sourceRoot, allowCanary: true });
  const receiptPath = path.join(root, "receipt.json");
  await writeFile(receiptPath, JSON.stringify({ contract: "codestory.etr1-synthetic-canary/v2",
    authority: "synthetic_canary_only", experiment_status: "valid", packet_decision: "not_evaluated",
    preparation: preparationBinding, documents: documents.binding, execution: execution.binding,
    validation, evaluated }), { flag: "wx", mode: 0o600 });
  console.log(JSON.stringify(await fileBinding(receiptPath)));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.stack); process.exitCode = 1; });
