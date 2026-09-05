import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { readFile, writeFile, mkdir, realpath } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { sha256, fragmentId } from "./lib/etr1-evidence.mjs";
import { referencePacket, sourcePacket, readerPrompt, READER_SCHEMA,
  validateReaderEvents } from "./lib/packet-reader-evidence.mjs";

const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const FROZEN_READER_INPUTS = Object.freeze({
  preparation: "30b84d4d848f96bd4fe799f2e0f28b9114971da0e47bf98ebe54fe36242199fd",
  questions: "8e7219a59c973c02f8ea93120bb680da46a75b8272153986c76e55bfb73ca3b6",
  annotations: "52b0cc223292bc70f1e4fa3f52b67bf42a91e4d4b9ed997aa12c648c068e9ade",
});
const readJson = async file => JSON.parse(await readFile(file, "utf8"));
async function readBound(file, digest) {
  const bytes = await readFile(file);
  assert.match(digest, /^[a-f0-9]{64}$/);
  assert.equal(sha256(bytes), digest, "input digest changed: " + file);
  return JSON.parse(bytes);
}
async function save(file, value) {
  const bytes = JSON.stringify(value, null, 2) + "\n";
  await writeFile(file, bytes, { flag: "wx", mode: 0o600 });
  return { path: file, sha256: sha256(bytes), bytes: Buffer.byteLength(bytes) };
}
function git(root, ...args) {
  return execFileSync("git", ["-C", root, ...args], { encoding: "utf8" }).trim();
}
function buildIdentity(requireClean) {
  const status = git(sourceRoot, "status", "--porcelain=v1");
  if (requireClean) assert.equal(status, "", "reader corpus run requires clean source");
  return { commit: git(sourceRoot, "rev-parse", "HEAD"),
    tree: git(sourceRoot, "rev-parse", "HEAD^{tree}"), status };
}
async function sourceFiles(repository, fragments) {
  assert.equal(git(repository.local_root, "rev-parse", "HEAD"), repository.commit,
    "repository commit changed");
  const root = await realpath(repository.local_root), result = new Map();
  for (const relative of [...new Set(fragments.map(f => f.path))]) {
    assert.ok(!path.isAbsolute(relative) && !relative.includes("\\")
      && relative.split("/").every(part => part && part !== "." && part !== ".."), "invalid source path");
    const file = await realpath(path.join(root, relative));
    assert.ok(file.startsWith(root + path.sep), "source escapes repository");
    result.set(relative, await readFile(file));
  }
  return result;
}

async function reconstructInputs({ preparationPath, preparationDigest, annotationsPath,
  annotationsDigest, questionsPath, questionsDigest }) {
  assert.deepEqual({ preparation: preparationDigest, annotations: annotationsDigest, questions: questionsDigest },
    FROZEN_READER_INPUTS, "inputs differ from independently frozen diagnostic contract");
  const preparation = await readBound(preparationPath, preparationDigest);
  const questions = await readBound(questionsPath, questionsDigest);
  const annotations = await readBound(annotationsPath, annotationsDigest);
  assert.equal(preparation.fixed_inputs.questions.sha256, questionsDigest, "preparation questions changed");
  assert.equal(annotations.questions_sha256, questionsDigest, "annotation questions changed");
  assert.equal(annotations.authority, "visible_development_only");
  const cases = questions.cases;
  const groups = [...new Set(cases.map(c => c.group))].sort();
  assert.equal(groups.length, 4, "reader diagnostic requires four frozen groups");
  const selected = groups.flatMap(group => cases.filter(c => c.group === group)
    .toSorted((a, b) => a.case_id.localeCompare(b.case_id)).slice(0, 2));
  assert.equal(selected.length, 8);
  const fragmentsById = new Map(preparation.fragments.map(f => [f.fragment_id, f]));
  const inputs = [], privateSelections = [];
  for (const c of selected) {
    const repository = preparation.repositories.find(r => r.repository_id === c.repository_id);
    assert.ok(repository, "missing repository");
    const fragments = repository.fragment_ids.map(id => {
      assert.ok(fragmentsById.has(id), "fragment missing"); return fragmentsById.get(id);
    });
    const sources = await sourceFiles(repository, fragments);
    const annotation = annotations.cases.find(a => a.case_id === c.case_id);
    assert.ok(annotation, "missing annotation");
    const { packet, selection } = referencePacket(annotation, fragments, repository.publication, sources);
    inputs.push({ case_id: c.case_id, group: c.group, question: c.question, packet });
    privateSelections.push({ case_id: c.case_id, selection });
  }
  return { inputs, privateSelections };
}

export async function prepareReader(options) {
  const { preparationPath, preparationDigest, annotationsPath, annotationsDigest,
    questionsPath, questionsDigest, output } = options;
  const reconstructed = await reconstructInputs(options);
  await mkdir(output, { mode: 0o700 });
  const inputs = [];
  for (const input of reconstructed.inputs)
    inputs.push(await save(path.join(output, input.case_id + ".input.json"), input));
  const privateBinding = await save(path.join(output, "private-selections.json"), reconstructed.privateSelections);
  return save(path.join(output, "manifest.json"), {
    contract: "codestory.packet-reader-inputs/v1", authority: "visible_reference_diagnostic_only",
    product_decision: "not_evaluated", build: buildIdentity(false),
    inputs: { preparation: { path: preparationPath, sha256: preparationDigest },
      questions: { path: questionsPath, sha256: questionsDigest },
      annotations: { path: annotationsPath, sha256: annotationsDigest } },
    sampling: "first_two_case_ids_per_group_original_question", private_selections: privateBinding,
    reader_inputs: inputs,
  });
}

export function readerArgs(model, directory, schema) {
  return ["exec", "--ignore-user-config", "--ignore-rules", "--ephemeral", "--skip-git-repo-check",
    "--sandbox", "read-only", "--json", "--color", "never", "--model", model,
    "--cd", directory, "--output-schema", schema,
    "-c", "approval_policy=\"never\"", "-c", "web_search=\"disabled\"",
    "-c", "project_doc_max_bytes=0", "-c", "model_reasoning_effort=\"low\"",
    ...["shell_tool", "apps", "plugins", "multi_agent", "hooks", "memories", "code_mode_host",
      "computer_use", "browser_use", "in_app_browser", "image_generation", "workspace_dependencies"
    ].flatMap(name => ["--disable", name]), "-"];
}

export function readerEnvironment() {
  return Object.fromEntries(["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "CODEX_HOME", "__CF_USER_TEXT_ENCODING"]
    .filter(key => typeof process.env[key] === "string").map(key => [key, process.env[key]]));
}

export async function readerProcess(command, args, prompt, {
  timeoutMs = 120000, signal, cwd, env = readerEnvironment(),
} = {}) {
  const start = performance.now();
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"], detached: true, cwd, env });
    child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
    let stdout = "", stderr = "", failure = null, escalation;
    const terminate = reason => {
      if (failure) return;
      failure = reason;
      try { process.kill(-child.pid, "SIGTERM"); } catch {}
      escalation = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} }, 1000);
      escalation.unref();
    };
    const timer = setTimeout(() => terminate("reader_deadline_exceeded"), timeoutMs);
    const cancel = () => terminate("reader_cancelled");
    signal?.addEventListener("abort", cancel, { once: true });
    if (signal?.aborted) cancel();
    child.stdout.on("data", bytes => {
      stdout += bytes.toString("utf8");
      if (Buffer.byteLength(stdout) > 1024 * 1024) terminate("reader_stdout_budget");
    });
    child.stderr.on("data", bytes => {
      stderr += bytes.toString("utf8");
      if (Buffer.byteLength(stderr) > 1024 * 1024) terminate("reader_stderr_budget");
    });
    child.on("error", error => { clearTimeout(timer); clearTimeout(escalation);
      signal?.removeEventListener("abort", cancel); reject(error); });
    child.stdin.on("error", () => {});
    child.on("close", (exitCode, exitSignal) => {
      clearTimeout(timer); clearTimeout(escalation);
      signal?.removeEventListener("abort", cancel);
      resolve({ exit_code: exitCode, signal: exitSignal, failure, stdout, stderr,
        wall_ms: performance.now() - start });
    });
    child.stdin.end(prompt);
  });
}

async function runInput({ input, command, model, output, signal }) {
  const value = await readBound(input.path, input.sha256);
  // The model runs in an empty directory, never beside annotations or repository files.
  const directory = path.join(output, value.case_id);
  await mkdir(directory, { mode: 0o700 });
  const schema = await save(path.join(directory, "answer-schema.json"), READER_SCHEMA);
  const prompt = readerPrompt(value.question, value.packet);
  const args = readerArgs(model, directory, schema.path);
  const env = readerEnvironment();
  const request = await save(path.join(directory, "request.json"), {
    command, args, cwd: directory, environment: env, model, input, schema,
    prompt_sha256: sha256(prompt), timeout_ms: 120000,
  });
  const execution = await readerProcess(command, args, prompt, { signal, cwd: directory, env });
  const raw = await save(path.join(directory, "execution.json"), {
    request, input, model, command, args, prompt_sha256: sha256(prompt), ...execution });
  assert.equal(execution.failure, null, execution.failure ?? "reader failed");
  assert.equal(execution.exit_code, 0, execution.stderr);
  const events = execution.stdout.trim().split("\n").map(line => JSON.parse(line));
  const answer = validateReaderEvents(events, value.packet);
  return { case_id: value.case_id, input, execution: raw,
    answer: await save(path.join(directory, "answer.json"), answer) };
}

export async function validateCanary(file, digest, build, binary, model) {
  const receipt = await readBound(file, digest);
  assert.equal(receipt.contract, "codestory.packet-reader-run/v1");
  assert.equal(receipt.authority, "synthetic_canary_only");
  assert.equal(receipt.experiment_status, "execution_valid");
  assert.deepEqual(receipt.build, build, "canary source differs");
  assert.deepEqual(receipt.reader_binary, binary, "canary executable differs");
  assert.equal(receipt.model, model, "canary model differs");
  assert.equal(receipt.rows.length, 1);
  const row = receipt.rows[0];
  const input = await readBound(row.input.path, row.input.sha256);
  assert.deepEqual(input, syntheticInput(), "canary input changed");
  const execution = await readBound(row.execution.path, row.execution.sha256);
  const request = await readBound(execution.request.path, execution.request.sha256);
  assert.equal(request.command, binary.path);
  assert.equal(request.model, model);
  assert.deepEqual(request.input, row.input);
  assert.deepEqual(request.args, readerArgs(model, request.cwd, request.schema.path));
  assert.equal(request.prompt_sha256, sha256(readerPrompt(input.question, input.packet)));
  assert.deepEqual(await readBound(request.schema.path, request.schema.sha256), READER_SCHEMA);
  assert.equal(execution.exit_code, 0); assert.equal(execution.failure, null);
  const answer = validateReaderEvents(execution.stdout.trim().split("\n").map(JSON.parse), input.packet);
  assert.deepEqual(await readBound(row.answer.path, row.answer.sha256), answer);
  assert.ok(answer.claims.some(c => /\b42\b/.test(c.text)), "canary did not return the expected value");
}

async function run({ manifestPath, manifestDigest, command, model, output, signal, canary = false,
  canaryPath, canaryDigest }) {
  const build = buildIdentity(!canary);
  command = await realpath(command);
  const binary = { path: command, sha256: sha256(await readFile(command)) };
  const manifest = await readBound(manifestPath, manifestDigest);
  assert.equal(manifest.contract, "codestory.packet-reader-inputs/v1");
  assert.equal(manifest.authority, canary ? "synthetic_canary_only" : "visible_reference_diagnostic_only");
  assert.equal(manifest.reader_inputs.length, canary ? 1 : 8, "reader row count changed");
  if (!canary) {
    await validateCanary(canaryPath, canaryDigest, build, binary, model);
    const expected = await reconstructInputs({
      preparationPath: manifest.inputs.preparation.path, preparationDigest: manifest.inputs.preparation.sha256,
      annotationsPath: manifest.inputs.annotations.path, annotationsDigest: manifest.inputs.annotations.sha256,
      questionsPath: manifest.inputs.questions.path, questionsDigest: manifest.inputs.questions.sha256,
    });
    for (const [index, input] of manifest.reader_inputs.entries())
      assert.deepEqual(await readBound(input.path, input.sha256), expected.inputs[index], "reader input drift");
  }
  await mkdir(output, { mode: 0o700 });
  const executionFreeze = await save(path.join(output, "execution-freeze.json"), {
    build, reader_binary: binary, model, timeout_ms: 120000,
    manifest: { path: manifestPath, sha256: manifestDigest },
    arguments: readerArgs(model, "<empty-case-directory>", "<schema-file>"),
    schema_sha256: sha256(JSON.stringify(READER_SCHEMA)),
    canary: canary ? null : { path: canaryPath, sha256: canaryDigest },
  });
  const rows = [];
  let error = null;
  try {
    for (const input of manifest.reader_inputs)
      rows.push(await runInput({ input, command, model, output, signal }));
    assert.equal(sha256(await readFile(command)), binary.sha256, "reader executable changed during run");
    assert.deepEqual(buildIdentity(!canary), build, "reader source changed during run");
  } catch (cause) { error = cause.message; }
  return save(path.join(output, "run.json"), {
    contract: "codestory.packet-reader-run/v1",
    authority: canary ? "synthetic_canary_only" : "visible_reference_diagnostic_only",
    experiment_status: error ? "invalid" : "execution_valid", product_decision: "not_evaluated",
    build, reader_binary: binary, execution_freeze: executionFreeze,
    model, manifest: { path: manifestPath, sha256: manifestDigest }, rows, error,
  });
}

function syntheticInput() {
  const source = "const result = 19 + 23;\nreturn result;\n", bytes = Buffer.from(source);
  const fragment = { project_id: "synthetic", path: "example.js", source, content_digest: sha256(bytes),
    byte_range: { start: 0, end: bytes.length }, line_range: { start: 1, end: 2 } };
  fragment.fragment_id = fragmentId(fragment);
  const packet = sourcePacket([fragment], { project_id: "synthetic", core_generation_id: "core",
    retrieval_generation: "retrieval" }, new Map([["example.js", bytes]]));
  return { case_id: "canary",
    question: "What value does the supplied source return? Cite the calculation and return.",
    packet };
}

async function canary({ command, model, output, signal }) {
  await mkdir(output, { mode: 0o700 });
  const input = await save(path.join(output, "input.json"), syntheticInput());
  const manifest = await save(path.join(output, "manifest.json"), {
    contract: "codestory.packet-reader-inputs/v1", authority: "synthetic_canary_only",
    reader_inputs: [input],
  });
  return run({ manifestPath: manifest.path, manifestDigest: manifest.sha256,
    command, model, output: path.join(output, "run"), signal, canary: true });
}

async function main() {
  const { values, positionals } = parseArgs({ allowPositionals: true, options: Object.fromEntries([
    "preparation", "preparation-sha256", "annotations", "annotations-sha256",
    "questions", "questions-sha256", "manifest", "manifest-sha256", "output", "reader", "model",
    "canary", "canary-sha256",
  ].map(name => [name, { type: "string" }])) });
  assert.ok(values.output, "--output is required");
  const signal = new AbortController();
  process.once("SIGINT", () => signal.abort()); process.once("SIGTERM", () => signal.abort());
  let receipt;
  if (positionals[0] === "prepare") receipt = await prepareReader({
    preparationPath: values.preparation, preparationDigest: values["preparation-sha256"],
    annotationsPath: values.annotations, annotationsDigest: values["annotations-sha256"],
    questionsPath: values.questions, questionsDigest: values["questions-sha256"], output: values.output,
  });
  else {
    assert.ok(values.reader && path.isAbsolute(values.reader), "--reader must name the exact binary");
    assert.ok(values.model, "--model required");
    const args = { command: values.reader, model: values.model, output: values.output, signal: signal.signal };
    if (positionals[0] === "canary") receipt = await canary(args);
    else {
      assert.equal(positionals[0], "run", "expected prepare, canary or run");
      receipt = await run({ ...args, manifestPath: values.manifest, manifestDigest: values["manifest-sha256"],
        canaryPath: values.canary, canaryDigest: values["canary-sha256"] });
    }
  }
  console.log(JSON.stringify(receipt));
  if (positionals[0] !== "prepare" && (await readJson(receipt.path)).experiment_status === "invalid")
    process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
