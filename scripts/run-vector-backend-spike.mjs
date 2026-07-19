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
const catalogInput = resolve(
  process.env.CODESTORY_VECTOR_SPIKE_CATALOG_JSON
    ?? join(root, "benchmarks", "vector-backend-spike", "query-catalog-linux-37e2f878.json"),
);
assertRegularFileWithoutSymlinks(catalogInput, "reviewed source-truth catalog");
const catalog = realpathSync(catalogInput);
const projectRootInput = resolve(required("CODESTORY_VECTOR_SPIKE_PROJECT_ROOT"));
assertDirectoryWithoutSymlinks(projectRootInput, "catalog source project root");
const projectRoot = realpathSync(projectRootInput);
const storageInput = resolve(required("CODESTORY_VECTOR_SPIKE_STORAGE"));
assertRegularFileWithoutSymlinks(storageInput, "core storage");
const storage = realpathSync(storageInput);
const sourceManifestInput = join(dirname(source), "vector-generation-manifest.json");
assertRegularFileWithoutSymlinks(sourceManifestInput, "source generation manifest");
const sourceManifest = realpathSync(sourceManifestInput);
const output = resolve(required("CODESTORY_VECTOR_SPIKE_OUTPUT_ROOT"));
const counts = [1000, 10000, 25000, 100000];
const blocks = 6;
const cleanRoots = ["clean-a", "clean-b"];
const timeoutMs = Number(process.env.CODESTORY_VECTOR_SPIKE_TIMEOUT_MS ?? 20 * 60_000);

if (existsSync(output)) throw new Error(`evidence root must be new: ${output}`);
assertNoSymlinkAncestors(dirname(output), "evidence root parent");
rejectSqliteSidecars(source);

const hostEvidence = approvedHostEvidence(binary);
mkdirSync(output, { recursive: true });
assertDirectoryWithoutSymlinks(output, "evidence root");
const embeddingAuthority = join(output, "embedding-authority");
mkdirSync(embeddingAuthority, { mode: 0o700 });
chmodSync(embeddingAuthority, 0o700);
const embeddingAuthorityNonce = `vector-spike-${hostEvidence.binary.sha256.slice(0, 24)}-${process.pid}`;
atomicJson(join(output, "host-evidence.json"), hostEvidence);
sealFrozenArtifact(join(output, "host-evidence.json"));

const frozenRoot = join(output, "inputs");
mkdirSync(frozenRoot, { recursive: true });
const frozenPublicationRoot = join(frozenRoot, "collections", basename(dirname(source)));
mkdirSync(frozenPublicationRoot, { recursive: true });
const sourceArtifact = freezeArtifact(source, join(frozenPublicationRoot, "vectors.sqlite3"));
const sourceManifestArtifact = freezeArtifact(sourceManifest, join(frozenPublicationRoot, "vector-generation-manifest.json"));
const fixtureArtifact = freezeArtifact(fixture, join(frozenRoot, "fixture.json"));
const catalogArtifact = freezeArtifact(catalog, join(frozenRoot, "catalog.json"));
const hostEvidencePath = join(output, "host-evidence.json");
const fixtureVerificationPath = join(output, "fixture-verification.json");
const verificationInputDigests = new Map([
  [sourceArtifact, sha256(readFileSync(sourceArtifact))],
  [sourceManifestArtifact, sha256(readFileSync(sourceManifestArtifact))],
  [fixtureArtifact, sha256(readFileSync(fixtureArtifact))],
  [catalogArtifact, sha256(readFileSync(catalogArtifact))],
]);
const fixtureVerification = await invokeBinary([
  "verify-fixture",
  "--project-root", projectRoot,
  "--storage", storage,
  "--source", sourceArtifact,
  "--source-generation-manifest", sourceManifestArtifact,
  "--fixture", fixtureArtifact,
  "--catalog", catalogArtifact,
], () => {
  for (const [path, digest] of verificationInputDigests) {
    assertArtifactDigest(path, "fixture-verification input", digest);
  }
}, {
  CODESTORY_EMBED_QUALIFICATION_DIR: embeddingAuthority,
  CODESTORY_EMBED_QUALIFICATION_NONCE: embeddingAuthorityNonce,
});
atomicJson(fixtureVerificationPath, fixtureVerification);
sealFrozenArtifact(fixtureVerificationPath);
const input = {
  schema_version: 2,
  source: relativeArtifact(output, sourceArtifact),
  source_generation_manifest: relativeArtifact(output, sourceManifestArtifact),
  fixture: relativeArtifact(output, fixtureArtifact),
  catalog: relativeArtifact(output, catalogArtifact),
  fixture_verification: relativeArtifact(output, fixtureVerificationPath),
  binary_sha256: hostEvidence.binary.sha256,
  host_evidence: relativeArtifact(output, hostEvidencePath),
};
const inputPath = join(output, "input.json");
atomicJson(inputPath, input);
sealFrozenArtifact(inputPath);
const inputSha256 = sha256(readFileSync(inputPath));
assertFrozenInputs(input, inputPath, inputSha256);

for (const cleanRoot of cleanRoots) {
  const clean = join(output, cleanRoot);
  mkdirSync(clean, { recursive: true });
  fsyncDirectory(clean);
  const journal = join(clean, "journal.jsonl");
  for (const count of counts) {
    const oracle = join(clean, `oracle-${count}.json`);
    const oracleEvent = {
      kind: "oracle",
      clean_root: cleanRoot,
      count,
      input_manifest_sha256: inputSha256,
      started_at: new Date().toISOString(),
    };
    appendJournal(journal, { ...oracleEvent, status: "started" });
    try {
      atomicJson(oracle, await invoke(["oracle", "--count", String(count)]));
      appendJournal(journal, {
        ...oracleEvent,
        status: "complete",
        artifact: oracle,
        result_sha256: sha256(readFileSync(oracle)),
      });
    } catch (error) {
      appendJournal(journal, {
        ...oracleEvent,
        status: "failed",
        error: String(error),
        failed_at: new Date().toISOString(),
      });
      throw error;
    }
    for (let block = 0; block < blocks; block += 1) {
      const order = counterbalancedOrder(block + 1);
      for (const [position, backend] of order.entries()) {
        const workdir = join(clean, "work", `${count}`, `block-${block + 1}`, backend);
        const event = {
          kind: "candidate",
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
  const replay = validateCleanEvidence(cleanRoot, clean, journal, inputSha256);
  atomicJson(join(clean, "complete.json"), {
    clean_root: cleanRoot,
    completed_at: new Date().toISOString(),
    input_manifest_sha256: inputSha256,
    fixture_verification_sha256: sha256(readFileSync(fixtureVerificationPath)),
    journal_sha256: sha256(readFileSync(journal)),
    replay,
  });
}

assertFrozenInputs(input, inputPath, inputSha256);
const completedCleanRoots = cleanRoots.map(cleanRoot => validateCompletedCleanEvidence(cleanRoot, inputSha256));
atomicJson(join(output, "complete.json"), {
  completed_at: new Date().toISOString(),
  input_manifest_sha256: inputSha256,
  fixture_verification_sha256: sha256(readFileSync(fixtureVerificationPath)),
  host_evidence_sha256: sha256(readFileSync(hostEvidencePath)),
  clean_roots: completedCleanRoots,
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
  fsyncFile(destination);
  const frozen = sha256(readFileSync(destination));
  const after = sha256(readFileSync(sourcePath));
  if (before !== frozen || before !== after) throw new Error(`source changed while freezing ${sourcePath}`);
  sealFrozenArtifact(destination);
  return destination;
}

function sealFrozenArtifact(path) {
  assertRegularFileWithoutSymlinks(path, "frozen artifact");
  chmodSync(path, 0o444);
  fsyncFile(path);
  fsyncDirectory(dirname(path));
}

function relativeArtifact(rootPath, artifactPath) {
  assertRegularFileWithoutSymlinks(artifactPath, "frozen artifact");
  const relativePath = relative(rootPath, artifactPath);
  if (!relativePath || relativePath.startsWith("..")) throw new Error(`frozen artifact escaped evidence root: ${artifactPath}`);
  return { path: relativePath, sha256: sha256(readFileSync(artifactPath)) };
}

function assertFrozenInputs(inputManifest, manifestPath, expectedManifestSha256) {
  if (inputManifest.schema_version !== 2) throw new Error("unsupported frozen input manifest schema");
  assertRegularFileWithoutSymlinks(manifestPath, "frozen input manifest");
  if (sha256(readFileSync(manifestPath)) !== expectedManifestSha256) throw new Error("frozen input manifest changed during the run");
  for (const [label, artifact] of Object.entries({
    source: inputManifest.source,
    source_generation_manifest: inputManifest.source_generation_manifest,
    fixture: inputManifest.fixture,
    catalog: inputManifest.catalog,
    fixture_verification: inputManifest.fixture_verification,
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
  const verification = JSON.parse(
    readFileSync(resolve(dirname(manifestPath), inputManifest.fixture_verification.path), "utf8"),
  );
  if (verification.schema_version !== 1
    || verification.source_database_sha256 !== inputManifest.source.sha256
    || verification.source_generation_manifest_sha256 !== inputManifest.source_generation_manifest.sha256
    || verification.fixture_sha256 !== inputManifest.fixture.sha256
    || verification.catalog_sha256 !== inputManifest.catalog.sha256
    || verification.binary_sha256 !== inputManifest.binary_sha256
    || verification.selection_seed !== "codestory-1202-vector-spike-v1"
    || typeof verification.query_vector_digest !== "string"
    || typeof verification.expected_document_digest !== "string") {
    throw new Error("fixture verification does not bind the reviewed frozen inputs");
  }
}

function assertArtifactDigest(path, label, expectedSha256) {
  assertRegularFileWithoutSymlinks(path, label);
  if (sha256(readFileSync(path)) !== expectedSha256) throw new Error(`${label} changed: ${path}`);
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
  fsyncDirectory(dirname(path));
}

function appendJournal(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  const fd = openSync(path, "a");
  writeFileSync(fd, `${JSON.stringify(value)}\n`);
  fsyncSync(fd);
  closeSync(fd);
  fsyncDirectory(dirname(path));
}

function counterbalancedOrder(block) {
  if (!Number.isInteger(block) || block < 1 || block > blocks) {
    throw new Error(`invalid paired block ${block}`);
  }
  return block % 2 === 1 ? ["sqlite-vec", "usearch"] : ["usearch", "sqlite-vec"];
}

function expectedJournalRows(cleanRoot, inputManifestSha256) {
  const rows = new Map();
  for (const count of counts) {
    rows.set(`oracle/${count}`, {
      kind: "oracle",
      clean_root: cleanRoot,
      count,
      input_manifest_sha256: inputManifestSha256,
    });
    for (let block = 1; block <= blocks; block += 1) {
      for (const [position, backend] of counterbalancedOrder(block).entries()) {
        rows.set(`candidate/${count}/${block}/${position + 1}/${backend}`, {
          kind: "candidate",
          clean_root: cleanRoot,
          count,
          block,
          order_position: position + 1,
          backend,
          input_manifest_sha256: inputManifestSha256,
        });
      }
    }
  }
  return rows;
}

function journalKey(event) {
  if (event.kind === "oracle") return `oracle/${event.count}`;
  if (event.kind === "candidate") {
    return `candidate/${event.count}/${event.block}/${event.order_position}/${event.backend}`;
  }
  throw new Error(`journal event has unsupported kind: ${event.kind}`);
}

function validateCleanEvidence(cleanRoot, clean, journal, inputManifestSha256) {
  assertFrozenInputs(input, inputPath, inputSha256);
  assertDirectoryWithoutSymlinks(clean, `clean evidence root ${cleanRoot}`);
  assertRegularFileWithoutSymlinks(journal, `journal for ${cleanRoot}`);
  const expected = expectedJournalRows(cleanRoot, inputManifestSha256);
  const observed = new Map([...expected.keys()].map(key => [key, { started: [], complete: [] }]));
  const lines = readFileSync(journal, "utf8").split("\n").filter(Boolean);
  if (lines.length === 0) throw new Error(`journal for ${cleanRoot} is empty`);
  for (const [index, line] of lines.entries()) {
    let event;
    try {
      event = JSON.parse(line);
    } catch (error) {
      throw new Error(`journal for ${cleanRoot} has malformed JSON at row ${index + 1}: ${error}`);
    }
    const key = journalKey(event);
    const expectedEvent = expected.get(key);
    if (!expectedEvent) throw new Error(`journal for ${cleanRoot} has unexpected row ${key}`);
    for (const [field, expectedValue] of Object.entries(expectedEvent)) {
      if (event[field] !== expectedValue) {
        throw new Error(`journal row ${key} has invalid ${field}: ${event[field]}`);
      }
    }
    if (event.status === "failed") throw new Error(`journal row ${key} recorded a failed child`);
    if (event.status !== "started" && event.status !== "complete") {
      throw new Error(`journal row ${key} has unsupported status: ${event.status}`);
    }
    observed.get(key)[event.status].push({ event, index });
  }
  for (const [key, expectedEvent] of expected) {
    const rows = observed.get(key);
    if (rows.started.length !== 1 || rows.complete.length !== 1) {
      throw new Error(`journal for ${cleanRoot} is missing or duplicates ${key}`);
    }
    if (rows.started[0].index >= rows.complete[0].index) {
      throw new Error(`journal for ${cleanRoot} completed ${key} before it started`);
    }
    validateJournalArtifact(clean, rows.complete[0].event, expectedEvent, inputManifestSha256);
  }
  return {
    schema_version: 1,
    expected_rows: expected.size,
    completed_rows: expected.size,
    journal_sha256: sha256(readFileSync(journal)),
  };
}

function validateJournalArtifact(clean, event, expectedEvent, inputManifestSha256) {
  if (typeof event.artifact !== "string" || typeof event.result_sha256 !== "string") {
    throw new Error(`journal ${journalKey(event)} has no artifact digest binding`);
  }
  const cleanReal = realpathSync(clean);
  const artifact = resolve(event.artifact);
  if (!artifact.startsWith(`${cleanReal}/`)) {
    throw new Error(`journal ${journalKey(event)} artifact escapes its clean evidence root`);
  }
  assertRegularFileWithoutSymlinks(artifact, `journal artifact ${journalKey(event)}`);
  const bytes = readFileSync(artifact);
  if (sha256(bytes) !== event.result_sha256) {
    throw new Error(`journal ${journalKey(event)} artifact digest no longer matches`);
  }
  let result;
  try {
    result = JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    throw new Error(`journal ${journalKey(event)} artifact is not JSON: ${error}`);
  }
  const expectedSchemaVersion = expectedEvent.kind === "candidate" ? 3 : 2;
  if (result.schema_version !== expectedSchemaVersion
    || result.input_manifest_sha256 !== inputManifestSha256
    || result.count !== expectedEvent.count) {
    throw new Error(`journal ${journalKey(event)} artifact does not bind the frozen input/count`);
  }
  if (expectedEvent.kind === "candidate") {
    if (result.backend !== expectedEvent.backend
      || result.clean_root !== expectedEvent.clean_root
      || result.block !== expectedEvent.block
      || result.order_position !== expectedEvent.order_position) {
      throw new Error(`journal ${journalKey(event)} candidate artifact does not match its matrix row`);
    }
    const requiredFaultProof = [
      "reader_publish_barrier_old_readers_pinned",
      "reader_publish_barrier_post_publish_reader_matches_truth",
      "pinned_old_reader_after_publication",
      "old_generation_unchanged",
      "corrupt_candidate_publish_rejected",
      "incomplete_candidate_publish_rejected",
      "failed_candidate_preserved_current_pointer",
      "cancelled_candidate_publish_rejected",
      "cancelled_candidate_preserved_current_pointer",
      "rollback_pointer_readable",
      "referenced_generation_tamper_rejected",
      "pinned_reader_after_referenced_tamper",
    ];
    if (requiredFaultProof.some(field => result[field] !== true)
      || result.cancellation_signal !== "candidate_build_cancelled_after_vectors"
      || result.cancellation_observed_after_vectors !== 8) {
      throw new Error(`journal ${journalKey(event)} candidate artifact lacks required publication/cancellation proof`);
    }
  }
}

function validateCompletedCleanEvidence(cleanRoot, inputManifestSha256) {
  const clean = join(output, cleanRoot);
  const journal = join(clean, "journal.jsonl");
  const replay = validateCleanEvidence(cleanRoot, clean, journal, inputManifestSha256);
  const markerPath = join(clean, "complete.json");
  assertRegularFileWithoutSymlinks(markerPath, `complete marker for ${cleanRoot}`);
  let marker;
  try {
    marker = JSON.parse(readFileSync(markerPath, "utf8"));
  } catch (error) {
    throw new Error(`complete marker for ${cleanRoot} is not JSON: ${error}`);
  }
  if (marker.clean_root !== cleanRoot
    || marker.input_manifest_sha256 !== inputManifestSha256
    || marker.fixture_verification_sha256 !== sha256(readFileSync(fixtureVerificationPath))
    || marker.journal_sha256 !== replay.journal_sha256
    || marker.replay?.schema_version !== replay.schema_version
    || marker.replay?.expected_rows !== replay.expected_rows
    || marker.replay?.completed_rows !== replay.completed_rows
    || marker.replay?.journal_sha256 !== replay.journal_sha256) {
    throw new Error(`complete marker for ${cleanRoot} does not match replayed evidence`);
  }
  return {
    clean_root: cleanRoot,
    complete_sha256: sha256(readFileSync(markerPath)),
    journal_sha256: replay.journal_sha256,
  };
}

function fsyncFile(path) {
  const fd = openSync(path, "r");
  fsyncSync(fd);
  closeSync(fd);
}

function fsyncDirectory(path) {
  const fd = openSync(path, "r");
  fsyncSync(fd);
  closeSync(fd);
}

function invoke(args) {
  assertFrozenInputs(input, inputPath, inputSha256);
  const childArgs = [...args, "--inputs", inputPath, "--expected-input-sha256", inputSha256];
  return invokeBinary(childArgs, () => assertFrozenInputs(input, inputPath, inputSha256));
}

function invokeBinary(args, verifyAfter, childEnv = {}) {
  verifyAfter?.();
  return new Promise((resolvePromise, reject) => {
    const child = spawn(binary, args, {
      cwd: root,
      detached: true,
      env: { ...process.env, ...childEnv },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    child.stdout.on("data", chunk => { stdout += chunk; });
    child.stderr.on("data", chunk => { stderr += chunk; });
    const timer = setTimeout(() => {
      timedOut = true;
      try { process.kill(-child.pid, "SIGKILL"); } catch {}
    }, timeoutMs);
    child.on("error", error => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (code !== 0) {
        const outcome = timedOut
          ? `timed_out_after_ms=${timeoutMs} signal=${signal ?? "unknown"}`
          : `exit_code=${code ?? "null"} signal=${signal ?? "none"}`;
        return reject(new Error(`${args[0]} failed ${outcome}: ${stderr.trim()}`));
      }
      try {
        verifyAfter?.();
        resolvePromise(JSON.parse(stdout));
      } catch (error) {
        reject(new Error(`${args[0]} emitted invalid JSON or changed frozen input: ${error}; stderr: ${stderr.trim()}`));
      }
    });
  });
}
