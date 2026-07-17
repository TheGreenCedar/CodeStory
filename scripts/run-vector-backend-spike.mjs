#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  realpathSync,
  renameSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";

const root = resolve(process.cwd());
const binaryInput = resolve(process.env.CODESTORY_VECTOR_SPIKE_BINARY ?? join(root, "target", "release", "vector_backend_spike"));
assertRegularFileWithoutSymlinks(binaryInput, "vector spike binary");
const binary = realpathSync(binaryInput);
const sourceInput = resolve(required("CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE"));
assertRegularFileWithoutSymlinks(sourceInput, "source database");
const source = realpathSync(sourceInput);
const fixtureInput = resolve(required("CODESTORY_VECTOR_SPIKE_FIXTURE_JSON"));
assertRegularFileWithoutSymlinks(fixtureInput, "fixture");
const fixture = realpathSync(fixtureInput);
const sourceManifestInput = join(dirname(source), "vector-generation-manifest.json");
assertRegularFileWithoutSymlinks(sourceManifestInput, "source generation manifest");
const sourceManifest = realpathSync(sourceManifestInput);
const output = resolve(required("CODESTORY_VECTOR_SPIKE_OUTPUT_ROOT"));
const counts = [1000, 10000, 25000, 100000];
const blocks = 6;
const timeoutMs = Number(process.env.CODESTORY_VECTOR_SPIKE_TIMEOUT_MS ?? 20 * 60_000);

if (existsSync(output)) throw new Error(`evidence root must be new: ${output}`);
assertNoSymlinkAncestors(dirname(output), "evidence root parent");
rejectSqliteSidecars(source);

const hostEvidence = approvedHostEvidence(binary);
mkdirSync(output, { recursive: true });
assertDirectoryWithoutSymlinks(output, "evidence root");
atomicJson(join(output, "host-evidence.json"), hostEvidence);

const frozenRoot = join(output, "inputs");
mkdirSync(frozenRoot, { recursive: true });
const frozenPublicationRoot = join(frozenRoot, "collections", basename(dirname(source)));
mkdirSync(frozenPublicationRoot, { recursive: true });
const sourceArtifact = freezeArtifact(source, join(frozenPublicationRoot, "vectors.sqlite3"));
const sourceManifestArtifact = freezeArtifact(sourceManifest, join(frozenPublicationRoot, "vector-generation-manifest.json"));
const fixtureArtifact = freezeArtifact(fixture, join(frozenRoot, "fixture.json"));
const hostEvidencePath = join(output, "host-evidence.json");
const input = {
  schema_version: 1,
  source: relativeArtifact(output, sourceArtifact),
  source_generation_manifest: relativeArtifact(output, sourceManifestArtifact),
  fixture: relativeArtifact(output, fixtureArtifact),
  binary_sha256: hostEvidence.binary.sha256,
  host_evidence: relativeArtifact(output, hostEvidencePath),
};
const inputPath = join(output, "input.json");
atomicJson(inputPath, input);
const inputSha256 = sha256(readFileSync(inputPath));
assertFrozenInputs(input, inputPath, inputSha256);

for (const cleanRoot of ["clean-a", "clean-b"]) {
  const clean = join(output, cleanRoot);
  mkdirSync(clean, { recursive: true });
  const journal = join(clean, "journal.jsonl");
  for (const count of counts) {
    const oracle = join(clean, `oracle-${count}.json`);
    atomicJson(oracle, await invoke(["oracle", "--count", String(count)]));
    for (let block = 0; block < blocks; block += 1) {
      const order = block % 2 === 0 ? ["sqlite-vec", "usearch"] : ["usearch", "sqlite-vec"];
      for (const [position, backend] of order.entries()) {
        const workdir = join(clean, "work", `${count}`, `block-${block + 1}`, backend);
        const event = {
          clean_root: cleanRoot,
          count,
          block: block + 1,
          order_position: position + 1,
          backend,
          input_manifest_sha256: inputSha256,
          started_at: new Date().toISOString(),
        };
        appendJournal(journal, { ...event, status: "started" });
        try {
          const result = await invoke([
            "candidate",
            "--oracle", oracle,
            "--count", String(count),
            "--backend", backend,
            "--workdir", workdir,
          ]);
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
  assertFrozenInputs(input, inputPath, inputSha256);
  atomicJson(join(clean, "complete.json"), {
    clean_root: cleanRoot,
    completed_at: new Date().toISOString(),
    input_manifest_sha256: inputSha256,
    journal_sha256: sha256(readFileSync(journal)),
  });
}

assertFrozenInputs(input, inputPath, inputSha256);
atomicJson(join(output, "complete.json"), {
  completed_at: new Date().toISOString(),
  input_manifest_sha256: inputSha256,
  host_evidence_sha256: sha256(readFileSync(hostEvidencePath)),
  disposition: "measurements complete; backend selection remains prohibited until all #1202 acceptance evidence is independently reviewed",
});

function required(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function approvedHostEvidence(binaryPath) {
  if (process.platform !== "darwin" || process.arch !== "arm64") {
    throw new Error(`approved vector evidence host is native macOS arm64; observed ${process.platform}/${process.arch}`);
  }
  const resolvedBinary = realpathSync(binaryPath);
  const unameMachine = command("uname", ["-m"]);
  if (unameMachine !== "arm64") throw new Error(`approved vector evidence host requires uname -m arm64; observed ${unameMachine}`);
  const translated = command("sysctl", ["-n", "sysctl.proc_translated"]);
  if (translated !== "0") throw new Error(`approved vector evidence host cannot run under Rosetta; sysctl.proc_translated=${translated}`);
  const fileDescription = command("file", ["-b", resolvedBinary]);
  const architectures = command("lipo", ["-archs", resolvedBinary]);
  const rustcVersion = command("rustc", ["-Vv"]);
  if (!fileDescription.includes("Mach-O") || !fileDescription.includes("arm64") || !architectures.split(/\s+/).includes("arm64")) {
    throw new Error(`vector spike binary is not an arm64 Mach-O: ${fileDescription}; lipo=${architectures}`);
  }
  if (!/^host:\s*aarch64-apple-darwin$/m.test(rustcVersion)) {
    throw new Error("approved vector evidence host requires an aarch64-apple-darwin Rust toolchain");
  }
  const lockPath = join(root, "Cargo.lock");
  return {
    schema_version: 1,
    approved_profile: "macOS arm64",
    os: process.platform,
    arch: process.arch,
    uname: command("uname", ["-a"]),
    os_version: command("sw_vers", ["-productVersion"]),
    rosetta_translated: translated,
    hardware: {
      model: command("sysctl", ["-n", "hw.model"]),
      cpu_brand: command("sysctl", ["-n", "machdep.cpu.brand_string"]),
      memory_bytes: command("sysctl", ["-n", "hw.memsize"]),
    },
    binary: {
      path: resolvedBinary,
      sha256: sha256(readFileSync(resolvedBinary)),
      size_bytes: statSync(resolvedBinary).size,
      file: fileDescription,
      architectures,
    },
    toolchain: {
      rustc: rustcVersion,
      cargo: command("cargo", ["-V"]),
      node: process.version,
      cargo_lock_sha256: sha256(readFileSync(lockPath)),
    },
  };
}

function command(executable, args) {
  const result = spawnSync(executable, args, { cwd: root, encoding: "utf8" });
  if (result.error) throw new Error(`failed to run ${executable}: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${executable} ${args.join(" ")} failed: ${String(result.stderr).trim()}`);
  return String(result.stdout).trim();
}

function assertNoSymlinkAncestors(path, label) {
  let current = resolve(path);
  while (!existsSync(current)) {
    const parent = dirname(current);
    if (parent === current) throw new Error(`${label} has no existing ancestor: ${path}`);
    current = parent;
  }
  for (;;) {
    if (lstatSync(current).isSymbolicLink()) throw new Error(`${label} traverses a symbolic link: ${current}`);
    const parent = dirname(current);
    if (parent === current) return;
    current = parent;
  }
}

function assertRegularFileWithoutSymlinks(path, label) {
  assertNoSymlinkAncestors(path, label);
  const stat = lstatSync(path);
  if (!stat.isFile()) throw new Error(`${label} must be an ordinary regular file: ${path}`);
}

function assertDirectoryWithoutSymlinks(path, label) {
  assertNoSymlinkAncestors(path, label);
  const stat = lstatSync(path);
  if (!stat.isDirectory()) throw new Error(`${label} must be an ordinary directory: ${path}`);
}

function rejectSqliteSidecars(path) {
  for (const suffix of ["-wal", "-shm", "-journal"]) {
    if (existsSync(`${path}${suffix}`)) throw new Error(`unbound SQLite sidecar exists beside ${path}`);
  }
}

function freezeArtifact(sourcePath, destination) {
  assertRegularFileWithoutSymlinks(sourcePath, "source artifact");
  assertNoSymlinkAncestors(dirname(destination), "frozen artifact parent");
  const before = sha256(readFileSync(sourcePath));
  copyFileSync(sourcePath, destination, 0);
  assertRegularFileWithoutSymlinks(destination, "frozen artifact");
  const frozen = sha256(readFileSync(destination));
  const after = sha256(readFileSync(sourcePath));
  if (before !== frozen || before !== after) throw new Error(`source changed while freezing ${sourcePath}`);
  chmodSync(destination, 0o444);
  return destination;
}

function relativeArtifact(rootPath, artifactPath) {
  assertRegularFileWithoutSymlinks(artifactPath, "frozen artifact");
  const relativePath = relative(rootPath, artifactPath);
  if (!relativePath || relativePath.startsWith("..")) throw new Error(`frozen artifact escaped evidence root: ${artifactPath}`);
  return { path: relativePath, sha256: sha256(readFileSync(artifactPath)) };
}

function assertFrozenInputs(inputManifest, manifestPath, expectedManifestSha256) {
  assertRegularFileWithoutSymlinks(manifestPath, "frozen input manifest");
  if (sha256(readFileSync(manifestPath)) !== expectedManifestSha256) throw new Error("frozen input manifest changed during the run");
  for (const [label, artifact] of Object.entries({
    source: inputManifest.source,
    source_generation_manifest: inputManifest.source_generation_manifest,
    fixture: inputManifest.fixture,
    host_evidence: inputManifest.host_evidence,
  })) {
    const resolved = resolve(dirname(manifestPath), artifact.path);
    if (!resolved.startsWith(`${dirname(manifestPath)}/`)) throw new Error(`frozen ${label} escapes the evidence root`);
    assertRegularFileWithoutSymlinks(resolved, `frozen ${label}`);
    if (sha256(readFileSync(resolved)) !== artifact.sha256) throw new Error(`frozen ${label} changed during the run`);
  }
  assertRegularFileWithoutSymlinks(binary, "vector spike binary");
  if (sha256(readFileSync(binary)) !== inputManifest.binary_sha256) throw new Error("vector spike binary changed during the run");
  const host = JSON.parse(readFileSync(resolve(dirname(manifestPath), inputManifest.host_evidence.path), "utf8"));
  if (host.os !== "darwin" || host.arch !== "arm64" || host.binary?.sha256 !== inputManifest.binary_sha256) {
    throw new Error("host evidence no longer matches the approved binary/profile");
  }
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function atomicJson(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${Date.now()}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx" });
  const fd = openSync(temporary, "r");
  fsyncSync(fd);
  closeSync(fd);
  renameSync(temporary, path);
  const parent = openSync(dirname(path), "r");
  fsyncSync(parent);
  closeSync(parent);
}

function appendJournal(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  const fd = openSync(path, "a");
  writeFileSync(fd, `${JSON.stringify(value)}\n`);
  fsyncSync(fd);
  closeSync(fd);
}

function invoke(args) {
  assertFrozenInputs(input, inputPath, inputSha256);
  const childArgs = [...args, "--inputs", inputPath, "--expected-input-sha256", inputSha256];
  return new Promise((resolvePromise, reject) => {
    const child = spawn(binary, childArgs, { cwd: root, detached: true, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", chunk => { stdout += chunk; });
    child.stderr.on("data", chunk => { stderr += chunk; });
    const timer = setTimeout(() => {
      try { process.kill(-child.pid, "SIGKILL"); } catch {}
    }, timeoutMs);
    child.on("error", error => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", code => {
      clearTimeout(timer);
      if (code !== 0) return reject(new Error(`${args[0]} exited ${code}: ${stderr.trim()}`));
      try {
        assertFrozenInputs(input, inputPath, inputSha256);
        resolvePromise(JSON.parse(stdout));
      } catch (error) {
        reject(new Error(`${args[0]} emitted invalid JSON or changed frozen input: ${error}; stderr: ${stderr.trim()}`));
      }
    });
  });
}
