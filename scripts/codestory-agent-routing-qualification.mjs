#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { existsSync, lstatSync, realpathSync } from "node:fs";
import {
  chmod,
  copyFile,
  cp,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import {
  MCP_PROTOCOL_REVISIONS,
  ROUTING_PACKET_QUESTIONS,
  ROUTING_SCENARIOS,
  materializeRoutingRequests,
  validateInstalledSession,
  validateRoutingRequestCorpus,
  validateStaticHostParity,
} from "./codestory-agent-routing-conformance.mjs";

const SHA256 = /^[0-9a-f]{64}$/u;
const COMMIT = /^[0-9a-f]{40}$/u;
const EXPECTED_SESSION_COUNT = 32;
const MAX_TRANSCRIPT_BYTES = 16 * 1024 * 1024;
const MAX_CANDIDATE_CLI_BYTES = 1024 * 1024 * 1024;
const MAX_PLUGIN_ARCHIVE_BYTES = 256 * 1024 * 1024;
const PROCESS_TIMEOUT_MS = 10 * 60 * 1000;
const CODEX_MARKETPLACE = "RoutingCandidate";
const DEFAULT_CODEX_MODEL = "gpt-5.6-sol";
const CODESTORY_MCP_ENV_VARS = Object.freeze([
  "CODESTORY_PLUGIN_DATA",
  "PLUGIN_DATA",
  "CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256",
  "CODESTORY_EMBED_QUALIFICATION_DIR",
  "CODESTORY_EMBED_QUALIFICATION_NONCE",
  "CODESTORY_CACHE_ROOT",
  "CODESTORY_STDIO_CACHE_ROOT",
  "CODESTORY_EMBED_MODEL_SOURCE",
]);
const PINNED_CODEX_CONFIG = Object.freeze([
  'approval_policy="never"',
  'model_reasoning_effort="xhigh"',
  'service_tier="default"',
  'personality="pragmatic"',
  'model_verbosity="low"',
  'features.remote_plugin=false',
]);
const STATIC_ROSTER_PATHS = [
  "plugin.json",
  "cli-version.json",
  "generated-mcp-catalog.json",
  "mcp.json",
  "scripts/codestory-mcp.cjs",
  ".claude-plugin/plugin.json",
  ".github/plugin/plugin.json",
  ".cursor-plugin/plugin.json",
  "hooks/claude-codex-hooks.json",
  "hooks/codestory-activate.cjs",
  "hooks/copilot-hooks.json",
  "hooks/cursor-hooks.json",
  "mcp.cursor.json",
  "rules/codestory.mdc",
  "skills/codestory-grounding/SKILL.md",
  "skills/codestory-grounding/agents/openai.yaml",
  "skills/codestory-grounding/references/generated-mcp-syntax.md",
  "skills/codestory-grounding/references/status-contract.md",
  "skills/codestory-grounding/references/ground.md",
  "skills/codestory-grounding/references/files.md",
  "skills/codestory-grounding/references/affected.md",
  "skills/codestory-grounding/references/packet.md",
  "skills/codestory-grounding/references/search.md",
  "skills/codestory-grounding/references/context.md",
  "skills/codestory-grounding/references/symbol.md",
  "skills/codestory-grounding/references/trail.md",
  "skills/codestory-grounding/references/snippet.md",
];
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

class QualificationError extends Error {
  constructor(message) {
    super(message);
    this.name = "QualificationError";
  }
}

function fail(message) {
  throw new QualificationError(message);
}

function plainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value, keys, label) {
  if (!plainObject(value)
      || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify([...keys].sort())) {
    fail(`${label} does not match its required schema`);
  }
}

function digestBytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function digestFile(path) {
  return digestBytes(await readFile(path));
}

async function ingestImmutable(sourcePath, destinationPath, maxBytes, label) {
  const source = await open(sourcePath, "r");
  let destination;
  try {
    const before = await source.stat();
    if (!before.isFile() || !Number.isSafeInteger(before.size) || before.size < 0 || before.size > maxBytes) {
      fail(`${label} exceeds its byte bound or is not a regular file`);
    }
    await mkdir(dirname(destinationPath), { recursive: true, mode: 0o700 });
    destination = await open(destinationPath, "wx", 0o600);
    const digest = createHash("sha256");
    const buffer = Buffer.allocUnsafe(Math.min(64 * 1024, maxBytes + 1));
    let bytes = 0;
    while (true) {
      const { bytesRead } = await source.read(buffer, 0, Math.min(buffer.length, maxBytes + 1 - bytes), null);
      if (bytesRead === 0) break;
      bytes += bytesRead;
      if (bytes > maxBytes) fail(`${label} exceeds its byte bound`);
      const chunk = buffer.subarray(0, bytesRead);
      digest.update(chunk);
      let offset = 0;
      while (offset < chunk.length) {
        const { bytesWritten } = await destination.write(chunk, offset, chunk.length - offset);
        if (bytesWritten < 1) fail(`${label} immutable staging stopped early`);
        offset += bytesWritten;
      }
    }
    const after = await source.stat();
    if (after.size !== bytes) fail(`${label} changed while being staged`);
    await destination.sync();
    await destination.chmod(0o400);
    return { path: destinationPath, bytes, sha256: digest.digest("hex") };
  } finally {
    await destination?.close().catch(() => {});
    await source.close().catch(() => {});
  }
}

async function readJson(path, label) {
  try {
    const value = JSON.parse(await readFile(path, "utf8"));
    if (!plainObject(value)) fail(`${label} must be an object`);
    return value;
  } catch (error) {
    if (error instanceof QualificationError) throw error;
    fail(`${label} is not valid JSON: ${error.message}`);
  }
}

function requiredDigest(value, label) {
  if (!SHA256.test(String(value)) || /^0{64}$/u.test(String(value))) fail(`${label} must be a nonzero SHA-256`);
  return value;
}

function inside(root, candidate, label) {
  const base = realpathSync(root);
  const path = realpathSync(candidate);
  const rel = relative(base, path);
  if (rel === ".." || rel.startsWith(`..${sep}`)) fail(`${label} escapes its authenticated root`);
  return path;
}

export function buildRoutingHostCommand({
  host,
  executable,
  projectRoot,
  pluginRoot,
  installedPluginRoot = null,
  codexHome = null,
  model,
  prompt,
}) {
  if (host === "codex") {
    if (!codexHome) fail("Codex qualification requires an isolated CODEX_HOME");
    if (!installedPluginRoot) fail("Codex qualification requires the installed plugin root");
    if (!model) fail("Codex qualification requires a pinned model");
    const codestoryMcpConfig = [
      `mcp_servers.codestory.command=${JSON.stringify("node")}`,
      `mcp_servers.codestory.args=${JSON.stringify(["./scripts/codestory-mcp.cjs"])}`,
      `mcp_servers.codestory.cwd=${JSON.stringify(installedPluginRoot)}`,
      `mcp_servers.codestory.env_vars=${JSON.stringify(CODESTORY_MCP_ENV_VARS)}`,
      `mcp_servers.codestory.default_tools_approval_mode=${JSON.stringify("approve")}`,
    ];
    return {
      command: executable,
      args: [
        "exec", "--json", "--ephemeral", "--skip-git-repo-check",
        ...PINNED_CODEX_CONFIG.flatMap((value) => ["--config", value]),
        ...codestoryMcpConfig.flatMap((value) => ["--config", value]),
        "--sandbox", "workspace-write", "--cd", projectRoot, "--model", model, "-",
      ],
      stdin: prompt,
      cwd: projectRoot,
      env: { CODEX_HOME: codexHome },
    };
  }
  if (host === "cursor") {
    if (!model) fail("Cursor qualification requires a pinned model");
    if (!/^composer-/iu.test(model)) fail("Cursor qualification requires a Composer model");
    const args = [
      "--print", "--output-format", "stream-json", "--stream-partial-output",
      "--mode", "ask", "--approve-mcps", "--trust", "--force", "--model", model,
      "--plugin-dir", pluginRoot,
      prompt,
    ];
    if (basename(executable).toLowerCase() === "cursor") args.unshift("agent");
    return { command: executable, args, stdin: null, cwd: projectRoot, env: {} };
  }
  fail(`unsupported qualification host ${JSON.stringify(host)}`);
}

export function parseRoutingQualificationOptions(argv) {
  const valueOptions = new Set([
    "--out", "--candidate-source-root",
    "--qualification-nonce", "--fixture-project",
    "--codex-command", "--cursor-command", "--codex-auth", "--codex-model", "--cursor-model",
  ]);
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    if (!valueOptions.has(option)) fail(`unknown option ${option}`);
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail(`${option} requires a value`);
    values[option.slice(2).replaceAll("-", "_")] = value;
    index += 1;
  }
  return values;
}

export function validateRoutingArtifactMatrix(rows) {
  if (!Array.isArray(rows) || rows.length !== EXPECTED_SESSION_COUNT) {
    fail(`routing qualification requires exactly ${EXPECTED_SESSION_COUNT} host sessions`);
  }
  const expected = new Set(
    ["codex", "cursor"].flatMap((host) => ROUTING_SCENARIOS.map(({ id }) => `${host}:${id}`)),
  );
  const observed = new Set();
  for (const row of rows) {
    const key = `${row?.host}:${row?.scenario_id}`;
    if (!expected.has(key) || observed.has(key)) fail(`routing qualification must contain ${key} exactly once`);
    if (typeof row.transcript !== "string" || row.transcript.length === 0
        || row.report?.status !== "pass" || row.report?.host !== row.host
        || row.report?.scenario_id !== row.scenario_id) fail(`${key} is not a completed validated host session`);
    observed.add(key);
  }
  if (observed.size !== expected.size) fail("routing qualification is missing a host session");
  return true;
}

export async function writeRoutingQualificationArtifacts(outDir, rows, identity = null) {
  validateRoutingArtifactMatrix(rows);
  await mkdir(outDir, { recursive: true });
  for (const row of rows) {
    const hostDir = join(outDir, row.host);
    await mkdir(hostDir, { recursive: true });
    await writeFile(join(hostDir, `${row.scenario_id}.jsonl`), row.transcript, "utf8");
    await writeFile(join(hostDir, `${row.scenario_id}.report.json`), `${JSON.stringify(row.report, null, 2)}\n`, "utf8");
  }
  const summary = {
    schema_version: 1,
    status: "pass",
    expected_sessions: EXPECTED_SESSION_COUNT,
    completed_sessions: rows.length,
    hosts: ["codex", "cursor"],
    scenarios: ROUTING_SCENARIOS.map(({ id }) => id),
    identity,
  };
  await writeFile(join(outDir, "summary.json"), `${JSON.stringify(summary, null, 2)}\n`, "utf8");
  return summary;
}

export async function writeRoutingSessionCapture(outDir, host, scenarioId, session) {
  if (!["codex", "cursor"].includes(host) || !ROUTING_SCENARIOS.some(({ id }) => id === scenarioId)
      || !plainObject(session) || typeof session.stdout !== "string" || typeof session.stderr !== "string") {
    fail("routing session capture is invalid");
  }
  const directory = join(resolve(outDir), "captures", host);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const stdoutPath = join(directory, `${scenarioId}.stdout.jsonl`);
  const stderrPath = join(directory, `${scenarioId}.stderr.txt`);
  const metadataPath = join(directory, `${scenarioId}.capture.json`);
  const stdout = Buffer.from(session.stdout, "utf8");
  const stderr = Buffer.from(session.stderr, "utf8");
  const metadata = Buffer.from(`${JSON.stringify({
    schema_version: 1,
    host,
    scenario_id: scenarioId,
    stdout_bytes: stdout.length,
    stdout_sha256: digestBytes(stdout),
    stderr_bytes: stderr.length,
    stderr_sha256: digestBytes(stderr),
    exit_code: session.code,
    signal: session.signal,
  }, null, 2)}\n`, "utf8");
  for (const [path, bytes] of [[stdoutPath, stdout], [stderrPath, stderr], [metadataPath, metadata]]) {
    await writeFile(path, bytes, { mode: 0o600 });
    await chmod(path, 0o600);
  }
  return { stdoutPath, stderrPath, metadataPath };
}

async function walkFiles(root) {
  const files = [];
  const visit = async (directory) => {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) fail(`authenticated tree contains a symbolic link: ${path}`);
      if (entry.isDirectory()) await visit(path);
      else if (entry.isFile()) files.push(path);
      else fail(`authenticated tree contains a non-file entry: ${path}`);
    }
  };
  await visit(root);
  return files;
}

export async function directoryContractSha256(root) {
  return directoryContractSha256Ignoring(root, new Set());
}

async function directoryContractSha256Ignoring(root, ignoredRootFiles) {
  const digest = createHash("sha256");
  const files = (await walkFiles(root)).filter(
    (path) => !ignoredRootFiles.has(relative(root, path).split(sep).join("/")),
  );
  if (files.length === 0) fail("plugin package root is empty");
  for (const path of files) {
    const name = Buffer.from(relative(root, path).split(sep).join("/"), "utf8");
    const bytes = await readFile(path);
    const nameLength = Buffer.alloc(8);
    const byteLength = Buffer.alloc(8);
    nameLength.writeBigUInt64LE(BigInt(name.length));
    byteLength.writeBigUInt64LE(BigInt(bytes.length));
    digest.update(nameLength).update(name).update(byteLength).update(bytes);
  }
  return digest.digest("hex");
}

function requireContainedDirectory(root, candidate, label) {
  const base = realpathSync(root);
  const relativePath = relative(base, resolve(candidate));
  if (!relativePath || relativePath === ".." || relativePath.startsWith(`..${sep}`)) {
    fail(`${label} escapes its private Cursor home`);
  }
  let current = base;
  for (const component of relativePath.split(sep)) {
    current = join(current, component);
    let metadata;
    try {
      metadata = lstatSync(current);
    } catch (error) {
      fail(`${label} is missing: ${error.message}`);
    }
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
      fail(`${label} contains a symbolic link or non-directory component`);
    }
  }
  if (inside(base, current, label) !== resolve(current)) {
    fail(`${label} does not preserve native path identity`);
  }
  return current;
}

async function cursorProviderRoots(parent, resolveProvider, label) {
  let revisions;
  try {
    revisions = await readdir(parent, { withFileTypes: true });
  } catch (error) {
    fail(`${label} is unavailable after Composer materialization: ${error.message}`);
  }
  const providers = [];
  for (const revision of revisions) {
    if (revision.isSymbolicLink()) fail(`${label} contains a symbolic link`);
    if (!revision.isDirectory()) continue;
    const providerRoot = resolveProvider(revision.name);
    let metadata;
    try {
      metadata = await lstat(providerRoot);
    } catch {
      continue;
    }
    if (metadata.isSymbolicLink()) fail(`${label} contains a symbolic link`);
    if (!metadata.isDirectory()) continue;
    providers.push({ revision: revision.name, providerRoot });
  }
  if (providers.length !== 1) fail(`${label} must contain exactly one Cursor ${label.includes("cache") ? "cache" : "marketplace clone"} provider`);
  let manifest;
  try {
    manifest = JSON.parse(await readFile(join(providers[0].providerRoot, "plugin.json"), "utf8"));
  } catch (error) {
    fail(`${label} manifest is invalid: ${error.message}`);
  }
  if (manifest?.name !== "codestory") fail(`${label} manifest is not CodeStory`);
  return providers[0];
}

export async function discoverCursorQualificationProviders(cursorHome) {
  const home = realpathSync(cursorHome);
  const cacheParent = join(
    home, ".cursor", "plugins", "cache", "thegreencedar-codestory", "codestory",
  );
  const cloneParent = join(
    home, ".cursor", "plugins", "marketplaces", "github.com", "thegreencedar", "codestory",
  );
  const cache = await cursorProviderRoots(
    cacheParent,
    (revision) => join(cacheParent, revision),
    "Cursor cache provider",
  );
  const clone = await cursorProviderRoots(
    cloneParent,
    (revision) => join(cloneParent, revision, "plugins", "codestory"),
    "Cursor marketplace clone provider",
  );
  if (cache.revision !== clone.revision) fail("Cursor account provider revisions disagree");
  const cacheRoot = requireContainedDirectory(home, cache.providerRoot, "Cursor cache provider");
  const cloneRoot = requireContainedDirectory(home, clone.providerRoot, "Cursor marketplace clone provider");
  return {
    cursorHome: home,
    cacheRoot,
    cloneRoot,
    dataDir: join(home, ".cursor", "plugins", "data", "codestory"),
  };
}

export async function verifyCursorQualificationProvider(provider) {
  const discovered = await discoverCursorQualificationProviders(provider.cursorHome);
  if (discovered.cacheRoot !== provider.cacheRoot || discovered.cloneRoot !== provider.cloneRoot
      || discovered.dataDir !== provider.dataDir) {
    fail("Cursor account provider identity drifted");
  }
  const cacheDigest = await directoryContractSha256Ignoring(provider.cacheRoot, new Set([".cache-complete"]));
  const cloneDigest = await directoryContractSha256(provider.cloneRoot);
  if (cacheDigest !== provider.candidatePluginSha256 || cloneDigest !== provider.candidatePluginSha256) {
    fail("Cursor candidate provider bytes drifted");
  }
  const cliMetadata = await lstat(provider.candidateCli);
  if (!cliMetadata.isFile() || cliMetadata.isSymbolicLink()
      || await digestFile(provider.candidateCli) !== provider.candidateCliSha256) {
    fail("Cursor candidate CLI bytes drifted");
  }
  const overrideMetadata = await lstat(provider.localOverridePath);
  if (!overrideMetadata.isFile() || overrideMetadata.isSymbolicLink()
      || (process.platform !== "win32" && (overrideMetadata.mode & 0o077) !== 0)) {
    fail("Cursor candidate local override is not an owner-only regular file");
  }
  const override = await readJson(provider.localOverridePath, "Cursor candidate local override");
  exactKeys(override, ["schema_version", "CODESTORY_CLI"], "Cursor candidate local override");
  if (override.schema_version !== 1 || override.CODESTORY_CLI !== provider.candidateCli) {
    fail("Cursor candidate local override drifted");
  }
  return true;
}

export async function installCursorQualificationProvider({
  cursorHome,
  candidatePluginRoot,
  candidateCli,
  candidateCliSha256,
}) {
  const discovered = await discoverCursorQualificationProviders(cursorHome);
  const pluginRoot = realpathSync(candidatePluginRoot);
  const cli = realpathSync(candidateCli);
  requiredDigest(candidateCliSha256, "Cursor candidate CLI digest");
  if (await digestFile(cli) !== candidateCliSha256) fail("Cursor candidate CLI digest does not match its bytes");
  const candidatePluginSha256 = await directoryContractSha256(pluginRoot);
  for (const providerRoot of [discovered.cacheRoot, discovered.cloneRoot]) {
    await rm(providerRoot, { recursive: true, force: true });
    await cp(pluginRoot, providerRoot, { recursive: true, errorOnExist: true });
  }
  await writeFile(join(discovered.cacheRoot, ".cache-complete"), "", { mode: 0o600 });
  await mkdir(discovered.dataDir, { recursive: true, mode: 0o700 });
  requireContainedDirectory(discovered.cursorHome, discovered.dataDir, "Cursor plugin data directory");
  const localOverridePath = join(discovered.dataDir, "local-overrides.json");
  await writeFile(localOverridePath, `${JSON.stringify({
    schema_version: 1,
    CODESTORY_CLI: cli,
  })}\n`, { mode: 0o600 });
  await chmod(localOverridePath, 0o600);
  const installed = {
    ...discovered,
    candidatePluginSha256,
    candidateCli: cli,
    candidateCliSha256,
    localOverridePath,
  };
  await verifyCursorQualificationProvider(installed);
  return installed;
}

async function spawnBounded(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: [options.stdin == null ? "ignore" : "pipe", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    let bytes = 0;
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, options.timeoutMs ?? PROCESS_TIMEOUT_MS);
    child.stdout.on("data", (chunk) => {
      bytes += chunk.length;
      if (bytes > 4 * 1024 * 1024) child.kill("SIGKILL");
      else stdout.push(chunk);
    });
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", reject);
    child.on("close", (code) => {
      clearTimeout(timeout);
      resolvePromise({
        code,
        timedOut,
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      });
    });
    if (options.stdin != null) child.stdin.end(options.stdin);
  });
}

async function successfulProcess(command, args, options, label) {
  const result = await spawnBounded(command, args, options);
  if (result.timedOut) fail(`${label} timed out`);
  if (result.code !== 0) fail(`${label} failed: ${result.stderr.trim() || result.stdout.trim()}`);
  return result.stdout;
}

function archiveEntryIsSafe(entry) {
  const normalized = entry.replaceAll("\\", "/").replace(/\/$/u, "");
  return normalized.length > 0 && !normalized.startsWith("/") && !/^[A-Za-z]:\//u.test(normalized)
    && normalized.split("/").every((part) => part && part !== "." && part !== "..");
}

async function extractAuthenticatedArchive(archivePath, destination) {
  const listing = await spawnBounded("tar", ["-tf", archivePath]);
  if (listing.code !== 0) fail(`candidate archive listing failed: ${listing.stderr.trim()}`);
  const entries = listing.stdout.split(/\r?\n/u).filter(Boolean);
  if (entries.length === 0 || entries.length > 4096 || entries.some((entry) => !archiveEntryIsSafe(entry))) {
    fail("candidate archive contains an invalid entry set");
  }
  await mkdir(destination, { recursive: true, mode: 0o700 });
  const extracted = await spawnBounded("tar", ["-xf", archivePath, "-C", destination]);
  if (extracted.code !== 0) fail(`candidate archive extraction failed: ${extracted.stderr.trim()}`);
  return walkFiles(destination);
}

async function gitValue(sourceRoot, args, label) {
  return (await successfulProcess("git", ["-C", sourceRoot, ...args], { timeoutMs: 30_000 }, label)).trim();
}

export async function verifyStagedCandidateInstallation(staged) {
  const {
    sourceHead, sourceTree, packageVersion, pluginArchiveSha256, candidateCliSha256,
    candidateTokenSha256, target, authenticatedCliPath, pluginRoot, pluginData,
    attestationPath, qualificationDir, qualificationNonce, managedCli, managedManifestPath,
  } = staged;
  const attestation = await readJson(attestationPath, "generated candidate install attestation");
  exactKeys(attestation, ["schema_version", "installation_source", "installation", "plugin", "candidate"], "generated candidate install attestation");
  exactKeys(attestation.installation, ["plugin_root", "plugin_data"], "generated candidate installation paths");
  exactKeys(attestation.plugin, ["id", "version", "source_commit", "source_tree", "package_sha256"], "generated candidate installed plugin");
  exactKeys(attestation.candidate, ["cli_sha256", "plugin_archive_sha256", "producer"], "generated candidate install identity");
  exactKeys(attestation.candidate.producer, ["kind"], "generated candidate producer");
  if (attestation.schema_version !== 2 || attestation.installation_source !== "source_candidate"
      || realpathSync(attestation.installation.plugin_root) !== realpathSync(pluginRoot)
      || realpathSync(attestation.installation.plugin_data) !== realpathSync(pluginData)
      || attestation.plugin.id !== "codestory" || attestation.plugin.version !== packageVersion
      || attestation.plugin.source_commit !== sourceHead || attestation.plugin.source_tree !== sourceTree
      || attestation.candidate.cli_sha256 !== candidateCliSha256
      || attestation.candidate.plugin_archive_sha256 !== pluginArchiveSha256
      || attestation.candidate.producer.kind !== "source_candidate") {
    fail("generated candidate attestation drifted from staged source and CLI identity");
  }
  requiredDigest(attestation.plugin.package_sha256, "generated candidate plugin package digest");
  if (await directoryContractSha256(pluginRoot) !== attestation.plugin.package_sha256) {
    fail("generated candidate plugin bytes drifted from their attestation");
  }
  const managedManifest = await readJson(managedManifestPath, "generated managed CLI manifest");
  if (managedManifest.version !== packageVersion || managedManifest.path !== basename(managedCli)
      || managedManifest.build_source !== "candidate_archive" || managedManifest.repo_ref !== sourceHead
      || managedManifest.archive_sha256 !== candidateTokenSha256
      || managedManifest.archive_url !== `candidate-archive:${candidateTokenSha256}`
      || managedManifest.target !== target
      || managedManifest.stdio_initialize_verified !== true
      || managedManifest.sha256 !== candidateCliSha256
      || await digestFile(authenticatedCliPath) !== candidateCliSha256
      || await digestFile(managedCli) !== candidateCliSha256) {
    fail("generated managed CLI installation drifted from authenticated source-candidate bytes");
  }
  const marker = await readJson(join(qualificationDir, "candidate-managed-install.json"), "generated candidate qualification marker");
  exactKeys(marker, ["schema_version", "purpose", "archive_sha256", "qualification_nonce_sha256"], "generated candidate qualification marker");
  if (!SHA256.test(qualificationNonce) || marker.schema_version !== 1
      || marker.purpose !== "codestory-candidate-managed-install" || marker.archive_sha256 !== candidateTokenSha256
      || marker.qualification_nonce_sha256 !== digestBytes(Buffer.from(qualificationNonce))) {
    fail("generated candidate qualification marker drifted from its source candidate and nonce");
  }
  return attestation;
}

function nativeAssetTarget() {
  if (process.platform === "darwin" && process.arch === "arm64") return "macos-arm64";
  if (process.platform === "linux" && process.arch === "x64") return "linux-x64";
  if (process.platform === "win32" && process.arch === "x64") return "windows-x64";
  fail(`source candidate qualification does not support ${process.platform}-${process.arch}`);
}

function nativeArchiveName(version, target) {
  const extension = target === "windows-x64" ? "zip" : "tar.gz";
  return `codestory-cli-v${version}-${target}.${extension}`;
}

async function probeSourceCandidateCli({ cli, packageVersion, protocolRevision, discoveryContracts }) {
  const requests = MCP_PROTOCOL_REVISIONS.map((revision) => ({
    jsonrpc: "2.0",
    id: `source-candidate-authentication-${revision}`,
    method: "initialize",
    params: {
      protocolVersion: revision,
      capabilities: {},
      clientInfo: { name: "codestory-routing-qualification", version: "1" },
    },
  }));
  const stdout = await successfulProcess(cli, ["serve", "--stdio", "--multi-project", "--refresh", "none"], {
    stdin: `${requests.map((request) => JSON.stringify(request)).join("\n")}\n`,
    timeoutMs: 30_000,
  }, "source candidate live initialize");
  let responses;
  try {
    responses = new Map(stdout.split(/\r?\n/u).filter(Boolean).map((line) => {
      const frame = JSON.parse(line);
      return [frame.id, frame];
    }));
  } catch (error) {
    fail(`source candidate live initialize returned invalid JSON: ${error.message}`);
  }
  for (const revision of MCP_PROTOCOL_REVISIONS) {
    const response = responses.get(`source-candidate-authentication-${revision}`);
    if (!plainObject(response?.result) || response.error) fail(`source candidate live ${revision} initialize did not return a result`);
    const result = response.result;
    if (result.serverInfo?.version !== packageVersion || result.version !== packageVersion) {
      fail("source candidate live version does not match the archived plugin and CLI pin");
    }
    if (result._meta?.codestory_publication?.schema_version !== 3) {
      fail("source candidate live schema must be 3");
    }
    if (result.protocolVersion !== revision
        || result._meta?.codestory_protocol?.negotiated !== revision
        || result._meta?.codestory_protocol?.preferred !== protocolRevision) {
      fail(`source candidate live ${revision} protocol does not match the archived catalog`);
    }
    if (result._meta?.codestory_protocol?.discovery_contract_sha256 !== discoveryContracts[revision]) {
      fail(`source candidate live ${revision} discovery identity does not match the archived catalog`);
    }
  }
}

async function buildCandidateCliFromSource({ sourceRoot, targetDir }) {
  await successfulProcess("cargo", [
    "build", "--locked", "--release", "-p", "codestory-cli", "--target-dir", targetDir,
  ], { cwd: sourceRoot, timeoutMs: PROCESS_TIMEOUT_MS }, "source candidate locked release build");
}

export async function authenticateSourceCandidateInstallation({
  candidateSourceRoot,
  qualificationNonce,
  stageRoot,
  buildCandidateCli = buildCandidateCliFromSource,
}) {
  if (!SHA256.test(String(qualificationNonce))) fail("qualification nonce must be a 64-hex value");
  const sourceRoot = realpathSync(candidateSourceRoot);
  const sourceHead = await gitValue(sourceRoot, ["rev-parse", "HEAD"], "candidate source HEAD");
  const sourceTree = await gitValue(sourceRoot, ["rev-parse", "HEAD^{tree}"], "candidate source tree");
  const sourceStatus = await gitValue(sourceRoot, ["status", "--porcelain=v1", "--untracked-files=all"], "candidate source cleanliness");
  if (!COMMIT.test(sourceHead) || /^0{40}$/u.test(sourceHead)
      || !COMMIT.test(sourceTree) || /^0{40}$/u.test(sourceTree) || sourceStatus) {
    fail("candidate source root is not a clean HEAD and tree");
  }
  await rm(stageRoot, { recursive: true, force: true });
  await mkdir(stageRoot, { recursive: true, mode: 0o700 });
  const targetDir = join(sourceRoot, "target", "codestory-mission-candidate");
  if (existsSync(targetDir)) {
    const targetMetadata = await lstat(targetDir);
    if (!targetMetadata.isDirectory() || targetMetadata.isSymbolicLink()
        || realpathSync(targetDir) !== resolve(targetDir)) {
      fail("source candidate build target must be its exact owned directory");
    }
  }
  const candidateCliPath = join(targetDir, "release", process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
  await rm(candidateCliPath, { force: true });
  await buildCandidateCli({ sourceRoot, targetDir, outputPath: candidateCliPath });
  let candidateCliMetadata;
  try {
    candidateCliMetadata = await lstat(candidateCliPath);
  } catch (error) {
    fail(`source candidate locked release build did not produce its owned CLI: ${error.message}`);
  }
  if (!candidateCliMetadata.isFile() || candidateCliMetadata.isSymbolicLink()) {
    fail("source candidate locked release build output must be a regular non-symlink file");
  }
  if (inside(targetDir, candidateCliPath, "source candidate locked release build output") !== resolve(candidateCliPath)) {
    fail("source candidate locked release build output must remain at its exact owned path");
  }
  const authenticatedCli = await ingestImmutable(
    candidateCliPath, join(stageRoot, "authenticated-inputs", "codestory-cli"),
    MAX_CANDIDATE_CLI_BYTES, "candidate CLI",
  );
  const candidateCliSha256 = authenticatedCli.sha256;

  const scratchRoot = join(stageRoot, "scratch");
  await mkdir(scratchRoot, { recursive: true, mode: 0o700 });
  const scratchArchive = join(scratchRoot, "plugin-source.tar");
  await successfulProcess("git", ["-C", sourceRoot, "archive", "--format=tar", `--output=${scratchArchive}`, sourceHead, "plugins/codestory"], { timeoutMs: 30_000 }, "candidate plugin source staging");
  const pluginArchive = await ingestImmutable(
    scratchArchive, join(stageRoot, "authenticated-inputs", "plugin-source.tar"),
    MAX_PLUGIN_ARCHIVE_BYTES, "candidate plugin source archive",
  );
  await rm(scratchRoot, { recursive: true, force: true });
  const afterHead = await gitValue(sourceRoot, ["rev-parse", "HEAD"], "candidate source post-staging HEAD");
  const afterTree = await gitValue(sourceRoot, ["rev-parse", "HEAD^{tree}"], "candidate source post-staging tree");
  const afterStatus = await gitValue(sourceRoot, ["status", "--porcelain=v1", "--untracked-files=all"], "candidate source post-staging cleanliness");
  if (afterHead !== sourceHead || afterTree !== sourceTree || afterStatus) {
    fail("candidate source root changed while its plugin bytes were staged");
  }

  const sourceStage = join(stageRoot, "source");
  await extractAuthenticatedArchive(pluginArchive.path, sourceStage);
  const pluginRoot = realpathSync(join(sourceStage, "plugins", "codestory"));

  const pluginManifest = await readJson(join(pluginRoot, "plugin.json"), "candidate plugin manifest");
  const cliVersion = await readJson(join(pluginRoot, "cli-version.json"), "candidate CLI version pin");
  const catalog = await readJson(join(pluginRoot, "generated-mcp-catalog.json"), "candidate generated catalog");
  const packageVersion = pluginManifest.version;
  const protocolRevision = catalog.wireContract?.preferredMcpProtocolVersion;
  const discoveryContracts = catalog.wireContract?.discoveryContracts;
  const discoveryContractSha256 = discoveryContracts?.[protocolRevision];
  if (pluginManifest.name !== "codestory" || typeof packageVersion !== "string" || !packageVersion
      || cliVersion.cli_version !== packageVersion
      || catalog.wireContract?.publicationStampSchemaVersion !== 3
      || catalog.wireContract?.minimumCompatiblePublicationStampSchemaVersion !== 3
      || protocolRevision !== "2025-11-25"
      || !plainObject(discoveryContracts)
      || JSON.stringify(Object.keys(discoveryContracts).sort())
        !== JSON.stringify([...MCP_PROTOCOL_REVISIONS].sort())
      || JSON.stringify([...(catalog.wireContract?.supportedMcpProtocolVersions ?? [])].sort())
        !== JSON.stringify([...MCP_PROTOCOL_REVISIONS].sort())
      || !plainObject(catalog.revisionProfiles)
      || JSON.stringify(Object.keys(catalog.revisionProfiles).sort())
        !== JSON.stringify([...MCP_PROTOCOL_REVISIONS].sort())) {
    fail("candidate archived plugin/catalog identity is not the required v3 preferred profile");
  }
  for (const revision of MCP_PROTOCOL_REVISIONS) {
    requiredDigest(discoveryContracts[revision], `candidate archived catalog ${revision} discovery digest`);
    if (catalog.revisionProfiles[revision]?.discoveryContractSha256 !== discoveryContracts[revision]) {
      fail(`candidate archived catalog ${revision} revision profile discovery digest does not match its wire contract`);
    }
  }

  const pluginData = join(stageRoot, "plugin-data");
  const managedRoot = join(pluginData, "codestory-cli", packageVersion);
  await mkdir(managedRoot, { recursive: true, mode: 0o700 });
  const managedCli = join(managedRoot, process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
  await copyFile(authenticatedCli.path, managedCli);
  await chmod(managedCli, 0o700);
  await probeSourceCandidateCli({
    cli: managedCli,
    packageVersion,
    protocolRevision,
    discoveryContracts,
  });
  const finalHead = await gitValue(sourceRoot, ["rev-parse", "HEAD"], "candidate source final HEAD");
  const finalTree = await gitValue(sourceRoot, ["rev-parse", "HEAD^{tree}"], "candidate source final tree");
  const finalStatus = await gitValue(sourceRoot, ["status", "--porcelain=v1", "--untracked-files=all"], "candidate source final cleanliness");
  if (finalHead !== sourceHead || finalTree !== sourceTree || finalStatus) {
    fail("candidate source root changed while its owned CLI was built and authenticated");
  }
  const target = nativeAssetTarget();
  const candidateTokenSha256 = digestBytes(Buffer.from([
    "codestory-routing-source-candidate-v1", sourceHead, sourceTree,
    pluginArchive.sha256, candidateCliSha256,
  ].join("\0")));
  const managedManifestPath = join(managedRoot, "manifest.json");
  await writeFile(managedManifestPath, `${JSON.stringify({
    path: basename(managedCli), sha256: candidateCliSha256,
    version: packageVersion, build_source: "candidate_archive",
    repo_ref: sourceHead, archive: nativeArchiveName(packageVersion, target),
    archive_url: `candidate-archive:${candidateTokenSha256}`, archive_sha256: candidateTokenSha256,
    archive_bytes: authenticatedCli.bytes, target,
    stdio_initialize_verified: true,
  }, null, 2)}\n`, { mode: 0o600 });

  const qualificationDir = join(stageRoot, "qualification");
  await mkdir(qualificationDir, { recursive: true, mode: 0o700 });
  await writeFile(join(qualificationDir, "candidate-managed-install.json"), `${JSON.stringify({
    schema_version: 1, purpose: "codestory-candidate-managed-install",
    archive_sha256: candidateTokenSha256,
    qualification_nonce_sha256: digestBytes(Buffer.from(qualificationNonce)),
  }, null, 2)}\n`, { mode: 0o600 });

  const pluginPackageSha256 = await directoryContractSha256(pluginRoot);
  const attestationPath = join(stageRoot, "candidate-install-attestation.json");
  await writeFile(attestationPath, `${JSON.stringify({
    schema_version: 2,
    installation_source: "source_candidate",
    installation: { plugin_root: pluginRoot, plugin_data: pluginData },
    plugin: {
      id: "codestory", version: packageVersion,
      source_commit: sourceHead, source_tree: sourceTree,
      package_sha256: pluginPackageSha256,
    },
    candidate: {
      cli_sha256: candidateCliSha256, plugin_archive_sha256: pluginArchive.sha256,
      producer: { kind: "source_candidate" },
    },
  }, null, 2)}\n`, { mode: 0o600 });
  const staged = {
    sourceHead, sourceTree, packageVersion, pluginArchiveSha256: pluginArchive.sha256,
    candidateCliSha256, candidateTokenSha256, target, authenticatedCliPath: authenticatedCli.path,
    pluginRoot, pluginData, attestationPath, qualificationDir, qualificationNonce,
    managedCli, managedManifestPath,
  };
  await verifyStagedCandidateInstallation(staged);

  const installedRoot = join(stageRoot, "identity");
  await mkdir(join(installedRoot, "archives"), { recursive: true });
  await mkdir(join(installedRoot, "scripts"), { recursive: true });
  await mkdir(join(installedRoot, "managed"), { recursive: true });
  const installedArchive = join(installedRoot, "archives", basename(pluginArchive.path));
  const installedLauncher = join(installedRoot, "scripts", "codestory-mcp.cjs");
  const installedCli = join(installedRoot, "managed", basename(managedCli));
  await copyFile(pluginArchive.path, installedArchive);
  await copyFile(join(pluginRoot, "scripts", "codestory-mcp.cjs"), installedLauncher);
  await copyFile(managedCli, installedCli);
  await chmod(installedCli, 0o700);
  const identity = {
    installation: { root: realpathSync(installedRoot) },
    package: {
      name: "codestory",
      version: packageVersion,
      archive_relative_path: `archives/${basename(installedArchive)}`,
      sha256: pluginArchive.sha256,
    },
    launcher: { relative_path: "scripts/codestory-mcp.cjs", sha256: await digestFile(installedLauncher) },
    cli: {
      relative_path: `managed/${basename(installedCli)}`,
      version: packageVersion,
      sha256: await digestFile(installedCli),
      source: "managed",
    },
    publication: { schema_version: 3 },
    protocol: {
      revision: protocolRevision,
      discovery_contract_sha256: discoveryContractSha256,
      discovery_contracts: discoveryContracts,
    },
  };
  const installedReceipt = join(installedRoot, "installed-receipt.json");
  await writeFile(installedReceipt, `${JSON.stringify({ schema_version: 1, identity }, null, 2)}\n`, "utf8");
  const expectedIdentity = {
    ...identity,
    receipt: { relative_path: "installed-receipt.json", sha256: await digestFile(installedReceipt) },
    static_roster: Object.fromEntries(await Promise.all(STATIC_ROSTER_PATHS.map(async (path) => [
      path,
      await digestFile(join(pluginRoot, path)),
    ]))),
  };
  return {
    pluginRoot,
    pluginData,
    installedRoot,
    installedReceipt,
    expectedIdentity,
    publicIdentity: {
      plugin_archive_sha256: pluginArchive.sha256,
      attestation_sha256: await digestFile(attestationPath),
      plugin_package_sha256: pluginPackageSha256,
      cli_sha256: identity.cli.sha256,
      source_commit: sourceHead,
      source_tree: sourceTree,
      schema_version: 3,
      protocol_revision: protocolRevision,
      discovery_contract_sha256: discoveryContractSha256,
      discovery_contracts: discoveryContracts,
    },
    launcherEnv: {
      CODESTORY_PLUGIN_DATA: realpathSync(pluginData),
      PLUGIN_DATA: realpathSync(pluginData),
      CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256: candidateTokenSha256,
      CODESTORY_EMBED_QUALIFICATION_DIR: qualificationDir,
      CODESTORY_EMBED_QUALIFICATION_NONCE: qualificationNonce,
    },
    staged,
  };
}

export function buildCodexPluginInstallCommand({ executable, codexHome, pluginRoot }) {
  if (!codexHome || !pluginRoot) fail("Codex plugin installation requires isolated home and staged plugin");
  const marketplaceRoot = join(codexHome, "qualification-marketplace");
  return {
    marketplaceRoot,
    marketplaceManifest: join(marketplaceRoot, ".agents", "plugins", "marketplace.json"),
    marketplacePlugin: join(marketplaceRoot, "plugins", "codestory"),
    command: executable,
    marketplaceArgs: ["plugin", "marketplace", "add", marketplaceRoot, "--json"],
    pluginArgs: ["plugin", "add", `codestory@${CODEX_MARKETPLACE}`, "--json"],
    env: { CODEX_HOME: codexHome },
  };
}

async function installCodexQualificationPlugin({ executable, codexHome, pluginRoot, env }) {
  const contract = buildCodexPluginInstallCommand({ executable, codexHome, pluginRoot });
  await mkdir(dirname(contract.marketplacePlugin), { recursive: true, mode: 0o700 });
  await mkdir(dirname(contract.marketplaceManifest), { recursive: true, mode: 0o700 });
  await cp(pluginRoot, contract.marketplacePlugin, { recursive: true, errorOnExist: true });
  await writeFile(contract.marketplaceManifest, `${JSON.stringify({
    name: CODEX_MARKETPLACE,
    interface: { displayName: "CodeStory routing qualification" },
    plugins: [{
      name: "codestory",
      source: { source: "local", path: "./plugins/codestory" },
      policy: { installation: "AVAILABLE", authentication: "ON_INSTALL" },
      category: "Developer Tools",
    }],
  }, null, 2)}\n`, { mode: 0o600 });
  const marketplaceOutput = await successfulProcess(contract.command, contract.marketplaceArgs, {
    env: { ...env, ...contract.env }, timeoutMs: 60_000,
  }, "isolated Codex marketplace installation");
  const marketplaceListOutput = await successfulProcess(
    contract.command, ["plugin", "marketplace", "list", "--json"],
    { env: { ...env, ...contract.env }, timeoutMs: 60_000 },
    "isolated Codex marketplace listing",
  );
  const output = await successfulProcess(contract.command, contract.pluginArgs, {
    env: { ...env, ...contract.env }, timeoutMs: 60_000,
  }, "isolated Codex plugin installation");
  const pluginListOutput = await successfulProcess(
    contract.command, ["plugin", "list", "--json"],
    { env: { ...env, ...contract.env }, timeoutMs: 60_000 },
    "isolated Codex plugin listing",
  );
  let installed;
  try {
    const marketplace = JSON.parse(marketplaceOutput);
    const marketplaceList = JSON.parse(marketplaceListOutput);
    installed = JSON.parse(output);
    const pluginList = JSON.parse(pluginListOutput);
    const listedMarketplace = marketplaceList?.marketplaces?.filter(({ name }) => name === CODEX_MARKETPLACE) ?? [];
    const listedPlugin = pluginList?.installed?.filter(({ pluginId }) => pluginId === `codestory@${CODEX_MARKETPLACE}`) ?? [];
    if (marketplace?.marketplaceName !== CODEX_MARKETPLACE || marketplace?.alreadyAdded !== false
        || typeof marketplace?.installedRoot !== "string" || listedMarketplace.length !== 1
        || realpathSync(marketplace.installedRoot) !== realpathSync(listedMarketplace[0].root)) {
      fail("isolated Codex marketplace installation returned the wrong identity");
    }
    if (listedPlugin.length !== 1 || pluginList?.available?.some(({ pluginId }) => pluginId === `codestory@${CODEX_MARKETPLACE}`)) {
      fail("isolated Codex plugin listing returned the wrong identity");
    }
  } catch (error) {
    if (error instanceof QualificationError) throw error;
    fail(`isolated Codex plugin installation returned invalid JSON: ${error.message}`);
  }
  if (installed?.pluginId !== `codestory@${CODEX_MARKETPLACE}` || installed?.name !== "codestory"
      || installed?.marketplaceName !== CODEX_MARKETPLACE || installed?.authPolicy !== "ON_INSTALL"
      || typeof installed?.installedPath !== "string" || !isAbsolute(installed.installedPath)) {
    fail("isolated Codex plugin installation returned the wrong identity");
  }
  const installedPath = inside(codexHome, installed.installedPath, "installed Codex qualification plugin");
  if (await directoryContractSha256(installedPath) !== await directoryContractSha256(pluginRoot)) {
    fail("installed Codex qualification plugin bytes drifted from the staged candidate");
  }
  return { ...installed, installedPath };
}

export async function cursorHostEnvironment(cursorHome, stateRoot, env) {
  const paths = {
    CURSOR_CONFIG_DIR: join(stateRoot, "config"),
    CURSOR_DATA_DIR: join(stateRoot, "data"),
    CURSOR_AGENT_STORE_DIR: join(stateRoot, "agent-store"),
    NODE_COMPILE_CACHE: join(stateRoot, "node-compile-cache"),
  };
  for (const path of Object.values(paths)) await mkdir(path, { recursive: true, mode: 0o700 });
  const inherited = { ...env };
  delete inherited.CODESTORY_CACHE_ROOT;
  delete inherited.CODESTORY_STDIO_CACHE_ROOT;
  return { ...inherited, HOME: cursorHome, ...paths };
}

async function prepareCursorQualificationProvider({
  outRoot,
  authenticated,
  executable,
  model,
  projectRoot,
  env,
}) {
  const root = join(resolve(outRoot), "cursor-provider");
  await rm(root, { recursive: true, force: true });
  const cursorHome = join(root, "home");
  await mkdir(cursorHome, { recursive: true, mode: 0o700 });
  if (process.platform === "darwin") {
    const sourceKeychains = join(process.env.HOME || "", "Library", "Keychains");
    if (isAbsolute(sourceKeychains) && existsSync(sourceKeychains)) {
      const library = join(cursorHome, "Library");
      await mkdir(library, { recursive: true, mode: 0o700 });
      await symlink(realpathSync(sourceKeychains), join(library, "Keychains"));
    }
  }
  const bootstrapEnv = await cursorHostEnvironment(cursorHome, join(root, "bootstrap"), env);
  const bootstrap = buildRoutingHostCommand({
    host: "cursor",
    executable,
    projectRoot,
    pluginRoot: authenticated.pluginRoot,
    model,
    prompt: "Reply with exactly READY and do not call any tool.",
  });
  const session = await runSession(bootstrap, bootstrapEnv);
  if (session.spawned !== true || session.code !== 0 || session.signal !== null || !session.stdout.trim()) {
    fail("Cursor Composer provider materialization did not complete");
  }
  return installCursorQualificationProvider({
    cursorHome,
    candidatePluginRoot: authenticated.pluginRoot,
    candidateCli: authenticated.staged.managedCli,
    candidateCliSha256: authenticated.staged.candidateCliSha256,
  });
}

export async function materializeRoutingFixture(sourceRoot, destination) {
  await rm(destination, { recursive: true, force: true });
  await cp(sourceRoot, destination, { recursive: true, errorOnExist: true });
  const catalog = [];
  const count = 96;
  for (let index = 0; index < count; index += 1) {
    const suffix = String(index).padStart(4, "0");
    catalog.push(`/// Deterministic routing evidence ${suffix}: ${"bounded documented source ".repeat(2)}`);
    catalog.push(`pub fn documented_route_${suffix}() -> usize { ${index} }`);
  }
  await writeFile(join(destination, "src", "catalog.rs"), `${catalog.join("\n")}\n`, "utf8");
  const libPath = join(destination, "src", "lib.rs");
  const lib = await readFile(libPath, "utf8");
  if (!lib.includes("pub mod catalog;")) await writeFile(libPath, `pub mod catalog;\n${lib}`, "utf8");
  return realpathSync(destination);
}

export function validateRoutingPreflight(scenarioId, body, { exitCode = 0 } = {}) {
  const dispositions = {
    typed_proof_contract_proven: "contract_proven",
    typed_proof_contract_refuted: "contract_refuted",
    typed_proof_unknown: "unknown",
    typed_proof_unavailable: "unavailable",
    proof_observational: "unknown",
    hidden_proof_tool_discovery: "contract_proven",
  };
  if (scenarioId === "malformed_proof_contract") {
    if (exitCode === 0) fail("malformed proof preflight must produce a semantic tool error");
    return true;
  }
  if (Object.hasOwn(dispositions, scenarioId)) {
    if (exitCode !== 0 || body?.kind !== "complete" || body?.disposition?.kind !== dispositions[scenarioId]) {
      fail(`${scenarioId} preflight did not materialize its required proof disposition`);
    }
    return true;
  }
  if (scenarioId === "packet_single_continuation") {
    if (exitCode !== 0 || body?.kind !== "complete" || body?.status !== "continuation_available"
        || !plainObject(body.continuation) || !Array.isArray(body.continuation.gap_ids)
        || body.continuation.gap_ids.length === 0) {
      fail("packet continuation preflight did not materialize a real continuation");
    }
    return true;
  }
  if (["broad_packet", "packet_gap_to_focused_source", "packet_named_fallback_to_source"].includes(scenarioId)) {
    if (exitCode !== 0 || body?.kind !== "complete") fail(`${scenarioId} preflight did not materialize a complete packet`);
    if (scenarioId === "packet_gap_to_focused_source" && !body.gaps?.length) {
      fail("packet gap preflight did not materialize an evidence gap");
    }
    if (scenarioId === "packet_named_fallback_to_source"
        && (!body.gaps?.length || body.evidence?.some((entry) => entry?.path === "src/fallback.rs"))) {
      fail("named packet fallback preflight did not leave the exact fallback unresolved");
    }
  }
  return true;
}

async function prepareRoutingFixture({ cli, projectRoot, env, includeRetrieval }) {
  await successfulProcess(cli, ["index", "--project", projectRoot, "--refresh", "full", "--format", "json"], {
    cwd: projectRoot, env, timeoutMs: PROCESS_TIMEOUT_MS,
  }, "routing fixture core preparation");
  if (!includeRetrieval) return;
  const args = ["retrieval", "index", "--project", projectRoot, "--profile", "agent", "--refresh", "full", "--format", "json"];
  for (let attempt = 1; attempt <= 6; attempt += 1) {
    const result = await spawnBounded(cli, args, { cwd: projectRoot, env, timeoutMs: PROCESS_TIMEOUT_MS });
    if (result.code === 0 && !result.timedOut) return;
    const transientOwner = result.stdout.includes("embedding_server_incompatible_active_owner");
    if (!transientOwner || attempt === 6) {
      if (result.timedOut) fail("routing fixture retrieval preparation timed out");
      fail(`routing fixture retrieval preparation failed: ${result.stderr.trim() || result.stdout.trim()}`);
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 5_000));
  }
}

async function requireMissingRetrievalProjection({ cli, projectRoot, env, label }) {
  const stdout = await successfulProcess(
    cli, ["retrieval", "status", "--project", projectRoot, "--profile", "agent", "--format", "json"],
    { cwd: projectRoot, env, timeoutMs: 60_000 }, label,
  );
  const status = parseJsonOutput(stdout, label);
  if (status.retrieval_mode !== "unavailable" || status.degraded_reason !== "retrieval_manifest_missing") {
    fail(`${label} observed semantic retrieval activation`);
  }
}

function parseJsonOutput(stdout, label) {
  try {
    return JSON.parse(stdout.trim());
  } catch (error) {
    fail(`${label} returned invalid JSON: ${error.message}`);
  }
}

async function preflightRoutingScenario({ cli, entry, projectRoot, env }) {
  const proof = entry.request.proof_contract;
  if (proof) {
    const result = await spawnBounded(cli, ["prove-call-path", "--project", projectRoot, "--spec", "-"], {
      cwd: projectRoot, env, stdin: JSON.stringify(proof), timeoutMs: PROCESS_TIMEOUT_MS,
    });
    const body = result.stdout.trim() ? parseJsonOutput(result.stdout, `${entry.scenario_id} proof preflight`) : null;
    validateRoutingPreflight(entry.scenario_id, body, { exitCode: result.code });
    return;
  }
  if (["broad_packet", "packet_single_continuation", "packet_gap_to_focused_source", "packet_named_fallback_to_source"].includes(entry.scenario_id)) {
    const question = ROUTING_PACKET_QUESTIONS[entry.scenario_id];
    const result = await spawnBounded(cli, ["packet", "--project", projectRoot, "--question", question, "--format", "json"], {
      cwd: projectRoot, env, timeoutMs: PROCESS_TIMEOUT_MS,
    });
    const body = result.stdout.trim() ? parseJsonOutput(result.stdout, `${entry.scenario_id} packet preflight`) : null;
    validateRoutingPreflight(entry.scenario_id, body, { exitCode: result.code });
  }
}

async function runSession(command, baseEnv, timeoutMs = PROCESS_TIMEOUT_MS) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command.command, command.args, {
      cwd: command.cwd,
      env: { ...baseEnv, ...command.env },
      stdio: [command.stdin === null ? "ignore" : "pipe", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let overflow = false;
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);
    child.stdout.on("data", (chunk) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > MAX_TRANSCRIPT_BYTES) {
        overflow = true;
        child.kill("SIGKILL");
      } else stdout.push(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes <= MAX_TRANSCRIPT_BYTES) stderr.push(chunk);
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      clearTimeout(timeout);
      if (overflow) return reject(new QualificationError("host transcript exceeded its closed byte bound"));
      if (timedOut) return reject(new QualificationError("installed host session timed out"));
      resolvePromise({
        spawned: true,
        code,
        signal,
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      });
    });
    if (command.stdin !== null) child.stdin.end(command.stdin);
  });
}

async function main(argv) {
  const options = parseRoutingQualificationOptions(argv);
  const required = [
    "out", "candidate_source_root",
    "qualification_nonce", "codex_auth", "cursor_model",
  ];
  for (const key of required) if (!options[key]) fail(`--${key.replaceAll("_", "-")} is required`);
  validateRoutingRequestCorpus();
  const authenticated = await authenticateSourceCandidateInstallation({
    candidateSourceRoot: options.candidate_source_root,
    qualificationNonce: options.qualification_nonce,
    stageRoot: join(resolve(options.out), "authenticated-installation"),
  });
  await validateStaticHostParity(authenticated.pluginRoot, authenticated.expectedIdentity);
  const fixtureSource = resolve(options.fixture_project
    ?? join(repoRoot, "scripts", "fixtures", "codestory-agent-routing-project"));
  const rows = [];
  let cursorProvider = null;
  for (const host of ["codex", "cursor"]) {
    for (const { id } of ROUTING_SCENARIOS) {
      const sessionRoot = join(resolve(options.out), "sessions", host, id);
      await mkdir(sessionRoot, { recursive: true, mode: 0o700 });
      const projectRoot = await materializeRoutingFixture(
        fixtureSource, join(sessionRoot, "project"),
      );
      const entry = materializeRoutingRequests(projectRoot).find((candidate) => candidate.scenario_id === id);
      let sessionEnv = {
        ...process.env,
        ...authenticated.launcherEnv,
        CODESTORY_CACHE_ROOT: join(sessionRoot, "cache"),
        CODESTORY_STDIO_CACHE_ROOT: join(sessionRoot, "stdio-cache"),
      };
      await mkdir(sessionEnv.CODESTORY_CACHE_ROOT, { recursive: true, mode: 0o700 });
      await mkdir(sessionEnv.CODESTORY_STDIO_CACHE_ROOT, { recursive: true, mode: 0o700 });
      const includeRetrieval = [
        "exact_symbol_search", "ambiguous_symbol_then_context", "selected_target_context",
        "broad_packet", "packet_single_continuation", "packet_gap_to_focused_source",
        "packet_named_fallback_to_source",
      ].includes(id);
      let codexHome = null;
      let installedPluginRoot = null;
      if (host === "codex") {
        codexHome = join(sessionRoot, "codex-home");
        await mkdir(codexHome, { recursive: true, mode: 0o700 });
        await copyFile(resolve(options.codex_auth), join(codexHome, "auth.json"));
        const installedPlugin = await installCodexQualificationPlugin({
          executable: options.codex_command ?? "codex", codexHome,
          pluginRoot: authenticated.pluginRoot, env: sessionEnv,
        });
        installedPluginRoot = installedPlugin.installedPath;
      } else {
        if (!cursorProvider) {
          cursorProvider = await prepareCursorQualificationProvider({
            outRoot: options.out,
            authenticated,
            executable: options.cursor_command ?? "cursor-agent",
            model: options.cursor_model,
            projectRoot,
            env: sessionEnv,
          });
        }
        await verifyCursorQualificationProvider(cursorProvider);
        sessionEnv = await cursorHostEnvironment(
          cursorProvider.cursorHome,
          join(sessionRoot, "cursor-host"),
          sessionEnv,
        );
        installedPluginRoot = cursorProvider.cacheRoot;
      }
      await prepareRoutingFixture({
        cli: authenticated.staged.managedCli, projectRoot, env: sessionEnv, includeRetrieval,
      });
      if (id === "proof_observational") {
        await requireMissingRetrievalProjection({
          cli: authenticated.staged.managedCli, projectRoot, env: sessionEnv,
          label: "proof observational preflight retrieval status",
        });
      }
      await preflightRoutingScenario({ cli: authenticated.staged.managedCli, entry, projectRoot, env: sessionEnv });
      const hostRequest = host === "cursor" ? {
        ...entry.request,
        text: `${entry.request.text}\nThe installed CodeStory guidance for this session is linked at ${join(
          installedPluginRoot, "skills", "codestory-grounding", "SKILL.md",
        )}. Read that exact file if you need its details; do not locate or probe the installed plugin package.`,
      } : entry.request;
      const command = buildRoutingHostCommand({
        host,
        executable: host === "codex" ? (options.codex_command ?? "codex") : (options.cursor_command ?? "cursor-agent"),
        projectRoot,
        pluginRoot: host === "cursor" ? cursorProvider.cacheRoot : authenticated.pluginRoot,
        installedPluginRoot,
        codexHome,
        model: host === "codex" ? (options.codex_model ?? DEFAULT_CODEX_MODEL) : options.cursor_model,
        prompt: hostRequest.text,
      });
      const session = await runSession(command, sessionEnv);
      const capture = await writeRoutingSessionCapture(resolve(options.out), host, entry.scenario_id, session);
      if (session.spawned !== true || session.code !== 0 || session.signal !== null || !session.stdout.trim()) {
        fail(`${host}:${entry.scenario_id} did not complete a real installed host session; capture ${capture.metadataPath}`);
      }
      let report;
      try {
        report = validateInstalledSession({
          host,
          scenarioId: entry.scenario_id,
          request: hostRequest,
          installedRoot: authenticated.installedRoot,
          installedReceipt: authenticated.installedReceipt,
          expectedIdentity: authenticated.expectedIdentity,
          installedPluginRoot,
          transcript: session.stdout,
        });
      } catch (error) {
        fail(`${error.message}; capture ${capture.metadataPath}`);
      }
      if (host === "cursor") await verifyCursorQualificationProvider(cursorProvider);
      if (id === "proof_observational") {
        await requireMissingRetrievalProjection({
          cli: authenticated.staged.managedCli, projectRoot, env: sessionEnv,
          label: "proof observational post-session retrieval status",
        });
      }
      rows.push({ host, scenario_id: entry.scenario_id, transcript: session.stdout, report });
    }
  }
  await writeRoutingQualificationArtifacts(resolve(options.out), rows, {
    ...authenticated.publicIdentity,
    host_configuration: {
      codex: { model: options.codex_model ?? DEFAULT_CODEX_MODEL, config: PINNED_CODEX_CONFIG },
      cursor: { model: options.cursor_model, mode: "ask", output_format: "stream-json" },
    },
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
