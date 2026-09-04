import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile, realpath, lstat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { sha256 } from "./etr1-evidence.mjs";

export const ANALYSIS_FILES = ["scripts/codestory-etr1-validate.mjs",
  "scripts/codestory-etr1-evaluate.mjs", "scripts/lib/etr1-evidence.mjs",
  "scripts/lib/etr1-execution.mjs"];

export async function fileBinding(file) {
  const bytes = await readFile(file);
  return { path: await realpath(file), sha256: sha256(bytes), bytes: bytes.length };
}

export async function readExecutionBinding(binding) {
  const bytes = await readFile(binding.path);
  assert.equal(bytes.length, binding.bytes, "execution artifact length changed");
  assert.equal(sha256(bytes), binding.sha256, "execution artifact digest changed");
  return JSON.parse(bytes);
}

export async function analysisIdentity(sourceRoot) {
  const git = (...args) => execFileSync("git", ["-C", sourceRoot, ...args], {
    encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" } }).trim();
  assert.equal(git("status", "--porcelain", "--untracked-files=no"), "", "analysis checkout is dirty");
  return { source_commit: git("rev-parse", "HEAD"), source_tree: git("rev-parse", "HEAD^{tree}"),
    files: await Promise.all(ANALYSIS_FILES.map((file) => fileBinding(path.join(sourceRoot, file)))),
    node: await fileBinding(process.execPath), node_version: process.version };
}

export async function validateExecution(binding, { role, input, output, sourceRoot }) {
  assert.ok(binding, "independent execution receipt required");
  const receipt = await readExecutionBinding(binding);
  assert.equal(receipt.contract, "codestory.etr1-execution/v1");
  assert.equal(receipt.role, role, "execution role changed");
  assert.equal(receipt.experiment_status, "completed", "execution did not complete");
  assert.equal(receipt.exit_code, 0);
  assert.equal(receipt.signal, null);
  assert.equal(receipt.annotation_access, "not_accessed");
  const request = await readExecutionBinding(receipt.request);
  assert.equal(request.contract, "codestory.etr1-execution-request/v1");
  assert.equal(request.role, role);
  if (role === "paired_run") assert.equal(request.deadline_ms, 1_800_000, "paired deadline changed");
  assert.deepEqual(request.analysis, await analysisIdentity(sourceRoot), "analysis identity changed");
  assert.deepEqual(await fileBinding(request.executable.path), request.executable, "producer binary changed");
  for (const expected of request.inputs)
    assert.deepEqual(await fileBinding(expected.path), expected, "execution input changed");
  assert.ok(request.inputs.some((item) => item.sha256 === input.sha256 && item.path === input.path),
    "execution input not independently bound");
  assert.ok(request.output_paths.includes(output.path), "execution output not declared before launch");
  assert.ok(receipt.outputs.some((item) => item.sha256 === output.sha256 && item.path === output.path),
    "execution output not independently bound");
  for (const expected of [...receipt.outputs, receipt.stdout, receipt.stderr, receipt.events])
    assert.deepEqual(await fileBinding(expected.path), expected, "execution result changed");
  assert.ok(Number.isSafeInteger(receipt.wall_ns) && receipt.wall_ns > 0);
  assert.ok(Number.isSafeInteger(receipt.pid) && receipt.pid > 0);
  return { request, receipt };
}

// The supervisor freezes inputs before launch and captures outputs before any
// evaluator can run. It does not claim to defend against a hostile host owner.
export async function executeRecorded({ role, executable, args, inputs, outputPaths, eventsPath,
  directory, sourceRoot, env, cancelFile, deadlineMs = 30 * 60 * 1000 }) {
  assert.ok(["documents", "paired_run"].includes(role), "unsupported ETR execution role");
  if (role === "paired_run") assert.equal(deadlineMs, 1_800_000, "paired deadline changed");
  for (const file of [...outputPaths, eventsPath]) {
    const exists = await lstat(file).then(() => true, (error) => {
      if (error.code === "ENOENT") return false;
      throw error;
    });
    assert.equal(exists, false, `execution output already exists: ${file}`);
  }
  await mkdir(directory, { mode: 0o700 });
  const recordedEnv = Object.fromEntries(Object.entries(env).filter(([key]) =>
    /^(CODESTORY_(CACHE_ROOT|EMBED_ALLOW_CPU|EMBED_QUALIFICATION_[A-Z_]+|EMBEDDING_[A-Z_]+)|XDG_CACHE_HOME|TMPDIR|DYLD_LIBRARY_PATH)$/u.test(key)
    && !/TOKEN|SECRET|PASSWORD|CREDENTIAL/u.test(key)));
  const request = { contract: "codestory.etr1-execution-request/v1", role,
    executable: await fileBinding(executable), args, environment: recordedEnv,
    inputs: await Promise.all(inputs.map(fileBinding)), output_paths: outputPaths,
    events_path: eventsPath, cancel_file: cancelFile ?? null, deadline_ms: deadlineMs,
    analysis: await analysisIdentity(sourceRoot) };
  const requestPath = path.join(directory, "request.json");
  await writeFile(requestPath, JSON.stringify(request), { flag: "wx", mode: 0o600 });
  const requestBinding = await fileBinding(requestPath);
  const started = process.hrtime.bigint();
  const child = spawn(executable, args, { env, stdio: ["ignore", "pipe", "pipe"] });
  const stdout = [], stderr = [];
  child.stdout.on("data", (value) => stdout.push(value));
  child.stderr.on("data", (value) => stderr.push(value));
  let cancelled = false, forcedKill;
  const cancel = async () => {
    cancelled = true;
    if (cancelFile) await writeFile(cancelFile, "cancel\n", { flag: "wx", mode: 0o600 }).catch(() => {});
    else child.kill("SIGTERM");
    forcedKill ??= setTimeout(() => child.kill("SIGKILL"), 5000);
  };
  const onSignal = () => { void cancel(); };
  process.once("SIGINT", onSignal);
  process.once("SIGTERM", onSignal);
  const timer = setTimeout(onSignal, deadlineMs);
  const terminal = await new Promise((resolve) => {
    child.once("error", (error) => resolve({ exit_code: null, signal: null, error: error.message }));
    child.once("close", (code, signal) => resolve({ exit_code: code, signal }));
  });
  clearTimeout(timer);
  clearTimeout(forcedKill);
  process.removeListener("SIGINT", onSignal);
  process.removeListener("SIGTERM", onSignal);
  const wall_ns = Number(process.hrtime.bigint() - started);
  const stdoutPath = path.join(directory, "stdout.log"), stderrPath = path.join(directory, "stderr.log");
  await writeFile(stdoutPath, Buffer.concat(stdout), { flag: "wx", mode: 0o600 });
  await writeFile(stderrPath, Buffer.concat(stderr), { flag: "wx", mode: 0o600 });
  let outputs = [], events = null, bindingError;
  try {
    outputs = await Promise.all(outputPaths.map(fileBinding));
    events = await fileBinding(eventsPath);
  } catch (error) { bindingError = error.message; }
  const completed = terminal.exit_code === 0 && !terminal.signal && !cancelled && !bindingError;
  const receipt = { contract: "codestory.etr1-execution/v1", role, request: requestBinding,
    experiment_status: completed ? "completed" : "invalid", decision: "not_evaluated",
    annotation_access: "not_accessed", pid: child.pid ?? null, ...terminal, cancelled,
    error: terminal.error ?? bindingError ?? null, wall_ns, outputs, events,
    stdout: await fileBinding(stdoutPath), stderr: await fileBinding(stderrPath) };
  const receiptPath = path.join(directory, "receipt.json");
  await writeFile(receiptPath, JSON.stringify(receipt), { flag: "wx", mode: 0o600 });
  return { binding: await fileBinding(receiptPath), receipt };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const main = async () => {
    assert.equal(process.argv.length, 3, "usage: node scripts/lib/etr1-execution.mjs /absolute/job.json");
    assert.ok(path.isAbsolute(process.argv[2]));
    const job = JSON.parse(await readFile(process.argv[2], "utf8"));
    const result = await executeRecorded({ ...job,
      sourceRoot: path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../.."),
      env: { ...process.env, ...job.environment } });
    console.log(JSON.stringify(result.binding));
    if (result.receipt.experiment_status !== "completed") process.exitCode = 1;
  };
  main().catch((error) => { console.error(error.stack); process.exitCode = 1; });
}
