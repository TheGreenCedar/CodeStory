#!/usr/bin/env node
/**
 * Thin wrapper around `cargo retrieval-setup` (see .cargo/config.toml).
 *
 * Primary documented path: `cargo retrieval-setup` from repo root.
 * This script adds prerequisite reporting and optional holdout repo clones.
 *
 * Prerequisites: Node 18+, cargo, Docker Desktop (unless --skip-compose).
 * SCIP language indexers are documented only — not installed by this script.
 */
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");

function usage() {
  console.log(`Usage:
  node scripts/setup-retrieval-env.mjs [options]

Options:
  --check-only, --dry-run   Verify prerequisites and print planned steps (no changes)
  --skip-build              Do not run cargo build -p codestory-cli
  --skip-compose            Pass --skip-compose to "retrieval bootstrap"
  --skip-status             Skip final "retrieval status"
  --with-holdout-clone      Clone holdout-retrieval OSS repos (network; large)
  --fetch-embed-model       Download bge-base-en-v1.5.Q8_0.gguf into target/retrieval-models
  --fetch-llama-server      Download the managed native llama-server for this host
  --llama-backend <id>      Select a llama-server backend from llama-sidecar-backends.json
  --fetch-only              With --fetch-embed-model/--fetch-llama-server, fetch/verify and exit
  --release                 Build and use release CLI (default: debug for speed)
  --project <path>          Project root for status (default: repo root)
  --wait-secs <n>           Bootstrap wait timeout (default: 90)
  --self-test               Run script self-tests (no network)

Examples:
  node scripts/setup-retrieval-env.mjs --check-only
  node scripts/setup-retrieval-env.mjs
  node scripts/setup-retrieval-env.mjs --with-holdout-clone
`);
}

function parseArgs(argv) {
  const opts = {
    checkOnly: false,
    skipBuild: false,
    skipCompose: false,
    skipStatus: false,
    withHoldoutClone: false,
    fetchEmbedModel: false,
    fetchLlamaServer: false,
    llamaBackend: null,
    fetchOnly: false,
    release: false,
    project: repoRoot,
    waitSecs: 90,
    selfTest: false,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
    if (arg === "--check-only" || arg === "--dry-run") {
      opts.checkOnly = true;
      continue;
    }
    if (arg === "--skip-build") {
      opts.skipBuild = true;
      continue;
    }
    if (arg === "--skip-compose") {
      opts.skipCompose = true;
      continue;
    }
    if (arg === "--skip-status") {
      opts.skipStatus = true;
      continue;
    }
    if (arg === "--with-holdout-clone") {
      opts.withHoldoutClone = true;
      continue;
    }
    if (arg === "--fetch-embed-model") {
      opts.fetchEmbedModel = true;
      continue;
    }
    if (arg === "--fetch-llama-server") {
      opts.fetchLlamaServer = true;
      continue;
    }
    if (arg === "--llama-backend") {
      opts.llamaBackend = argv[++i];
      continue;
    }
    if (arg === "--fetch-only") {
      opts.fetchOnly = true;
      continue;
    }
    if (arg === "--release") {
      opts.release = true;
      continue;
    }
    if (arg === "--project") {
      opts.project = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--wait-secs") {
      opts.waitSecs = Number.parseInt(argv[++i], 10);
      continue;
    }
    if (arg === "--self-test") {
      opts.selfTest = true;
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(opts.waitSecs) || opts.waitSecs < 0) {
    throw new Error("--wait-secs must be a non-negative integer");
  }
  if (opts.fetchOnly && !opts.fetchEmbedModel && !opts.fetchLlamaServer) {
    throw new Error("--fetch-only requires --fetch-embed-model or --fetch-llama-server");
  }
  if (opts.llamaBackend && !opts.fetchLlamaServer && !opts.selfTest) {
    throw new Error("--llama-backend requires --fetch-llama-server");
  }
  return opts;
}

function commandExists(name) {
  const lookup = process.platform === "win32" ? "where" : "which";
  const result = spawnSync(commandName(lookup), [commandName(name)], {
    encoding: "utf8",
    shell: false,
  });
  return result.status === 0;
}

function commandName(name) {
  if (process.platform === "win32" && !name.toLowerCase().endsWith(".exe")) {
    return `${name}.exe`;
  }
  return name;
}

function codestoryCacheRoot() {
  if (process.platform === "win32" && process.env.LOCALAPPDATA) {
    return path.join(process.env.LOCALAPPDATA, "codestory", "cache");
  }
  return path.join(os.homedir(), ".cache", "codestory", "cache");
}

function cliPath(release) {
  const base = path.join(repoRoot, "target", release ? "release" : "debug");
  const name = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  return path.join(base, name);
}

function runChecked(label, file, args, env = process.env) {
  console.log(`\n==> ${label}`);
  console.log(`    ${file} ${args.join(" ")}`);
  const result = spawnSync(commandName(file), args, {
    cwd: repoRoot,
    env,
    encoding: "utf8",
    shell: false,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`${label} failed (exit ${result.status ?? "unknown"})`);
  }
}

function printPrereqReport(opts) {
  const composeFile = path.join(repoRoot, "docker", "retrieval-compose.yml");
  const cacheRoot = codestoryCacheRoot();
  const checks = [
    ["node", commandExists("node"), "required"],
    [
      "cargo",
      commandExists("cargo"),
      opts.skipBuild || opts.fetchOnly ? "optional" : "required",
    ],
    [
      "docker",
      commandExists("docker"),
      opts.skipCompose || opts.fetchOnly ? "optional" : "required for live Qdrant",
    ],
    [
      "tar",
      commandExists("tar"),
      opts.fetchLlamaServer && !opts.checkOnly
        ? "required for llama-server archive extraction"
        : "optional",
    ],
    [
      `compose file (${composeFile})`,
      fs.existsSync(composeFile),
      opts.fetchOnly
        ? "optional"
        : "required unless CODESTORY_RETRIEVAL_COMPOSE_FILE points elsewhere",
    ],
  ];

  console.log("CodeStory retrieval sidecar environment setup");
  console.log("Primary path: cargo retrieval-setup");
  console.log(`Repository: ${repoRoot}`);
  console.log(`Cache root:   ${cacheRoot}`);
  console.log("\nPrerequisites:");
  let failed = false;
  for (const [name, ok, note] of checks) {
    const mark = ok ? "OK" : "MISSING";
    console.log(`  [${mark}] ${name} — ${note}`);
    if (!ok && note.startsWith("required")) {
      failed = true;
    }
  }

  console.log("\nAutomated:");
  console.log("  - Docker Compose: Qdrant + Zoekt webserver + llama.cpp embed service");
  console.log("  - codestory retrieval bootstrap (cache dirs, sidecar state, health wait)");
  console.log("  - codestory retrieval status --project <path>");
  if (opts.withHoldoutClone) {
    console.log("  - node scripts/fetch-holdout-repos.mjs");
  }

  console.log("\nManual (not automated):");
  console.log("  - SCIP indexers per language (rust-analyzer scip, scip-typescript, etc.)");
  console.log("  - retrieval index --project <repo> after sidecars are healthy");

  if (!opts.skipCompose && !commandExists("docker")) {
    console.log("\nDocker install (Windows):");
    console.log("  https://docs.docker.com/desktop/setup/install/windows-install/");
    console.log("\nManual Qdrant without compose:");
    console.log(
      `  docker run -d --name codestory-qdrant -p 127.0.0.1:6333:6333 -p 127.0.0.1:6334:6334 ` +
        `-v "${path.join(cacheRoot, "qdrant")}:/qdrant/storage" qdrant/qdrant:v1.12.5@sha256:05fecce7dce45d1254e0468bc037e8210e187fd56fa847688b012293d5f08aae`,
    );
    console.log("\nZoekt without compose:");
    console.log("  run sourcegraph/zoekt-webserver on 127.0.0.1:6070 with the CodeStory shard directory mounted");
  }

  return failed;
}

const BGE_GGUF = "bge-base-en-v1.5.Q8_0.gguf";
const BGE_GGUF_SHA256 = "ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c";
const BGE_GGUF_BYTES = 117_974_304;
const BGE_URLS = [
  "https://huggingface.co/BAAI/bge-base-en-v1.5-GGUF/resolve/main/bge-base-en-v1.5.Q8_0.gguf",
  "https://huggingface.co/CompendiumLabs/bge-base-en-v1.5-gguf/resolve/main/bge-base-en-v1.5-q8_0.gguf",
];

function embedModelDir() {
  if (process.env.CODESTORY_EMBED_MODEL_DIR) {
    return path.resolve(process.env.CODESTORY_EMBED_MODEL_DIR);
  }
  return path.join(repoRoot, "target", "retrieval-models");
}

function sha256File(file) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    const stream = fs.createReadStream(file);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolve(hash.digest("hex")));
  });
}

async function verifyEmbedModel(file) {
  const stat = fs.statSync(file);
  if (!stat.isFile()) {
    throw new Error(`Embed model path is not a file: ${file}`);
  }
  if (stat.size !== BGE_GGUF_BYTES) {
    throw new Error(
      `Embed model size mismatch for ${file}: got ${stat.size} bytes, expected ${BGE_GGUF_BYTES}`,
    );
  }
  const actual = await sha256File(file);
  if (actual !== BGE_GGUF_SHA256) {
    throw new Error(
      `Embed model SHA-256 mismatch for ${file}: got ${actual}, expected ${BGE_GGUF_SHA256}`,
    );
  }
  return actual;
}

async function fetchEmbedModel() {
  const dir = embedModelDir();
  fs.mkdirSync(dir, { recursive: true });
  const dest = path.join(dir, BGE_GGUF);
  if (fs.existsSync(dest) && fs.statSync(dest).size > 1_000_000) {
    let checksum;
    try {
      checksum = await verifyEmbedModel(dest);
    } catch (error) {
      throw new Error(
        `${error instanceof Error ? error.message : error}. Remove ${dest} and rerun --fetch-embed-model.`,
      );
    }
    console.log(`Embed model already present and verified: ${dest} sha256=${checksum}`);
    return dest;
  }
  let lastError = null;
  for (const url of BGE_URLS) {
    console.log(`Downloading ${BGE_GGUF} from ${url} to ${dest} ...`);
    const response = await fetch(url);
    if (!response.ok) {
      lastError = `HTTP ${response.status} from ${url}`;
      continue;
    }
    const buffer = Buffer.from(await response.arrayBuffer());
    const tempDest = `${dest}.tmp-${process.pid}`;
    try {
      fs.writeFileSync(tempDest, buffer);
      const checksum = await verifyEmbedModel(tempDest);
      fs.renameSync(tempDest, dest);
      console.log(`Wrote ${dest} (${buffer.length} bytes, sha256=${checksum})`);
      return dest;
    } catch (error) {
      fs.rmSync(tempDest, { force: true });
      lastError = `${error instanceof Error ? error.message : error} from ${url}`;
    }
  }
  throw new Error(`Failed to download embed model: ${lastError ?? "no URLs configured"}`);
}

const LLAMA_BACKENDS_MANIFEST = path.join(
  repoRoot,
  "crates",
  "codestory-retrieval",
  "assets",
  "llama-sidecar-backends.json",
);
const LLAMA_INSTALL_MANIFEST = "install-manifest.json";

function readLlamaBackends() {
  return JSON.parse(fs.readFileSync(LLAMA_BACKENDS_MANIFEST, "utf8")).backends ?? [];
}

function hostOs() {
  if (process.platform === "darwin") {
    return "macos";
  }
  if (process.platform === "win32") {
    return "windows";
  }
  return process.platform;
}

function hostArch() {
  if (process.arch === "arm64") {
    return "aarch64";
  }
  if (process.arch === "x64") {
    return "x86_64";
  }
  return process.arch;
}

function selectedLlamaBackend(opts) {
  const backends = readLlamaBackends();
  if (opts.llamaBackend) {
    const backend = backends.find((candidate) => candidate.id === opts.llamaBackend);
    if (!backend) {
      throw new Error(`Unknown llama backend ${opts.llamaBackend} in ${LLAMA_BACKENDS_MANIFEST}`);
    }
    return backend;
  }
  const osName = hostOs();
  const arch = hostArch();
  const provider =
    process.env.CODESTORY_EMBED_DEVICE_PROVIDER?.trim().toLowerCase() ||
    (osName === "macos" && arch === "aarch64" ? "metal" : "");
  const backend = backends.find(
    (candidate) =>
      candidate.os === osName && candidate.arch === arch && candidate.provider === provider,
  );
  if (!backend) {
    throw new Error(`No managed llama-server backend for ${osName}/${arch}/${provider || "default"}`);
  }
  return backend;
}

function managedLlamaServerPath(backend) {
  return path.join(codestoryCacheRoot(), backend.managed_cache_rel_dir, backend.executable_rel_path);
}

async function verifySha256(file, expected, label) {
  const actual = await sha256File(file);
  if (actual !== expected.toLowerCase()) {
    throw new Error(`${label} SHA-256 mismatch for ${file}: got ${actual}, expected ${expected}`);
  }
  return actual;
}

function safeArchiveMemberPath(member) {
  if (!member || member.trim() === "" || member.includes("\\") || /^[A-Za-z]:/.test(member)) {
    throw new Error(`Managed llama-server archive path is not portable: ${member}`);
  }
  const normalized = path.posix.normalize(member);
  if (
    normalized === "." ||
    normalized.startsWith("../") ||
    normalized.includes("/../") ||
    path.posix.isAbsolute(normalized)
  ) {
    throw new Error(`Managed llama-server archive path must be relative and contained: ${member}`);
  }
  return normalized;
}

async function validateExtractedExecutable(file, backend) {
  const stat = fs.lstatSync(file);
  if (stat.isSymbolicLink() || !stat.isFile()) {
    throw new Error(`Managed llama-server archive member is not a regular file: ${file}`);
  }
  const executableSha = await sha256File(file);
  if (executableSha !== backend.executable_sha256.toLowerCase()) {
    throw new Error(
      `llama-server executable SHA-256 mismatch for ${file}: got ${executableSha}, expected ${backend.executable_sha256}`,
    );
  }
  return executableSha;
}

function printLlamaServerPlan(backend) {
  console.log("\nManaged llama-server:");
  console.log(`  backend: ${backend.id}`);
  console.log(`  artifact: ${backend.artifact}`);
  console.log(`  url: ${backend.url}`);
  console.log(`  sha256: ${backend.sha256}`);
  console.log(`  executable_archive_path: ${backend.executable_archive_path}`);
  console.log(`  executable_sha256: ${backend.executable_sha256}`);
  console.log(`  target: ${managedLlamaServerPath(backend)}`);
}

async function fetchLlamaServer(opts) {
  const backend = selectedLlamaBackend(opts);
  const dest = managedLlamaServerPath(backend);
  printLlamaServerPlan(backend);
  if (opts.checkOnly) {
    return dest;
  }
  if (!commandExists("tar")) {
    throw new Error("tar is required to extract the managed llama-server archive");
  }
  const targetDir = path.dirname(dest);
  fs.mkdirSync(targetDir, { recursive: true });
  const existingManifest = path.join(targetDir, LLAMA_INSTALL_MANIFEST);
  if (fs.existsSync(dest) && fs.existsSync(existingManifest)) {
    const manifest = JSON.parse(fs.readFileSync(existingManifest, "utf8"));
    const executableSha = await sha256File(dest);
    if (
      manifest.artifact === backend.artifact &&
      manifest.artifact_sha256?.toLowerCase() === backend.sha256.toLowerCase() &&
      manifest.executable_rel_path === backend.executable_rel_path &&
      manifest.executable_sha256?.toLowerCase() === backend.executable_sha256.toLowerCase() &&
      executableSha === backend.executable_sha256.toLowerCase()
    ) {
      console.log(`llama-server already present and verified: ${dest} sha256=${executableSha}`);
      return dest;
    }
    console.log(`Managed llama-server install manifest is stale for ${dest}; redownloading.`);
  }

  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-llama-server-"));
  const archive = path.join(tempRoot, backend.artifact);
  const extractDir = path.join(tempRoot, "extract");
  fs.mkdirSync(extractDir);
  try {
    console.log(`Downloading ${backend.artifact} from ${backend.url} to ${archive} ...`);
    const response = await fetch(backend.url);
    if (!response.ok) {
      throw new Error(`HTTP ${response.status} from ${backend.url}`);
    }
    fs.writeFileSync(archive, Buffer.from(await response.arrayBuffer()));
    await verifySha256(archive, backend.sha256, "llama-server artifact");
    const member = safeArchiveMemberPath(backend.executable_archive_path);
    const result = spawnSync(commandName("tar"), ["-xzf", archive, "-C", extractDir, member], {
      encoding: "utf8",
      shell: false,
    });
    if (result.status !== 0) {
      throw new Error(`tar failed extracting ${archive}: ${result.stderr || result.stdout}`);
    }
    const extracted = path.join(extractDir, ...member.split("/"));
    await validateExtractedExecutable(extracted, backend);
    fs.copyFileSync(extracted, dest);
    fs.chmodSync(dest, 0o755);
    const executableSha = await sha256File(dest);
    if (executableSha !== backend.executable_sha256.toLowerCase()) {
      throw new Error(
        `llama-server executable SHA-256 mismatch for ${dest}: got ${executableSha}, expected ${backend.executable_sha256}`,
      );
    }
    fs.writeFileSync(
      existingManifest,
      `${JSON.stringify(
        {
          backend: backend.id,
          artifact: backend.artifact,
          artifact_sha256: backend.sha256,
          executable_rel_path: backend.executable_rel_path,
          executable_sha256: backend.executable_sha256,
          source_url: backend.url,
        },
        null,
        2,
      )}\n`,
    );
    console.log(`Wrote ${dest} (sha256=${executableSha})`);
    return dest;
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

async function runSelfTest() {
  const backends = readLlamaBackends();
  const macMetal = backends.find((backend) => backend.id === "macos-aarch64-metal");
  if (!macMetal) {
    throw new Error("missing macos-aarch64-metal backend");
  }
  if (backends.some((backend) => backend.os === "macos" && backend.arch !== "aarch64")) {
    throw new Error("macOS Intel llama-server backend must not be present");
  }
  if (!macMetal.managed_cache_rel_dir.includes("/llama/b9902/")) {
    throw new Error(`unexpected managed cache version path: ${macMetal.managed_cache_rel_dir}`);
  }
  if (safeArchiveMemberPath(macMetal.executable_archive_path) !== "llama-b9902/llama-server") {
    throw new Error(`unexpected executable archive path: ${macMetal.executable_archive_path}`);
  }
  if (!/^[0-9a-f]{64}$/.test(macMetal.executable_sha256)) {
    throw new Error(`unexpected executable sha256: ${macMetal.executable_sha256}`);
  }
  const selected = selectedLlamaBackend({ llamaBackend: "macos-aarch64-metal" });
  if (managedLlamaServerPath(selected).split(path.sep).join("/").endsWith("llama-server") !== true) {
    throw new Error("managed llama-server target path should end in llama-server");
  }
  console.log("setup-retrieval-env self-test passed");
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.selfTest) {
    await runSelfTest();
    return;
  }
  const failed = printPrereqReport(opts);
  if (opts.fetchLlamaServer && opts.checkOnly) {
    printLlamaServerPlan(selectedLlamaBackend(opts));
  }
  if (opts.checkOnly) {
    process.exit(failed ? 1 : 0);
  }
  if (failed && !opts.skipCompose) {
    throw new Error("Fix missing prerequisites (or use --skip-compose / --skip-build where applicable).");
  }

  if (opts.fetchEmbedModel) {
    await fetchEmbedModel();
  }
  if (opts.fetchLlamaServer) {
    await fetchLlamaServer(opts);
  }
  if (opts.fetchOnly) {
    console.log("\nFetch-only setup complete.");
    return;
  }

  const bootstrapArgs = [
    "run",
    "-p",
    "codestory-cli",
    "--",
    "retrieval",
    "bootstrap",
    "--project",
    opts.project,
    "--wait-secs",
    String(opts.waitSecs),
  ];
  if (opts.skipCompose) {
    bootstrapArgs.push("--skip-compose");
  }
  if (opts.release) {
    bootstrapArgs.splice(1, 0, "--release");
  }

  if (!opts.skipBuild) {
    runChecked("Bootstrap retrieval sidecars", "cargo", bootstrapArgs);
  } else {
    const cli = cliPath(opts.release);
    if (!fs.existsSync(cli)) {
      throw new Error(`CLI not found at ${cli}; drop --skip-build or run cargo retrieval-setup.`);
    }
    const directArgs = bootstrapArgs.slice(bootstrapArgs.indexOf("--") + 1);
    runChecked("Bootstrap retrieval sidecars", cli, directArgs);
  }

  if (!opts.skipStatus) {
    if (opts.skipBuild) {
      const cli = cliPath(opts.release);
      runChecked("Retrieval status", cli, ["retrieval", "status", "--project", opts.project]);
    } else {
      runChecked("Retrieval status", "cargo", [
        "run",
        "-p",
        "codestory-cli",
        ...(opts.release ? ["--release"] : []),
        "--",
        "retrieval",
        "status",
        "--project",
        opts.project,
      ]);
    }
  }

  if (opts.withHoldoutClone) {
    runChecked(
      "Fetch holdout-retrieval repos",
      process.execPath,
      [path.join(repoRoot, "scripts", "fetch-holdout-repos.mjs")],
    );
  }

  console.log("\nSetup complete.");
  console.log("Next: cargo run -p codestory-cli -- retrieval index --project <repo-root> --refresh auto");
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
