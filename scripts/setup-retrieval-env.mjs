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
  --fetch-embed-model       Prewarm the machine-wide pinned embedding model cache
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
  if (process.env.CODESTORY_CACHE_ROOT?.trim()) {
    return path.resolve(process.env.CODESTORY_CACHE_ROOT.trim());
  }
  if (process.platform === "win32" && process.env.LOCALAPPDATA) {
    return path.join(process.env.LOCALAPPDATA, "codestory", "codestory", "cache");
  }
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Caches", "dev.codestory.codestory");
  }
  if (process.env.XDG_CACHE_HOME && path.isAbsolute(process.env.XDG_CACHE_HOME)) {
    return path.join(process.env.XDG_CACHE_HOME, "codestory");
  }
  return path.join(os.homedir(), ".cache", "codestory");
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

function cargoRunArgs(...args) {
  return ["run", "--locked", ...args];
}

function printPrereqReport(opts) {
  const composeFile = path.join(repoRoot, "docker", "retrieval-compose.yml");
  const cacheRoot = codestoryCacheRoot();
  const checks = [
    ["node", commandExists("node"), "required"],
    [
      "cargo",
      commandExists("cargo"),
      opts.skipBuild ? "optional" : "required",
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
  console.log("  - Docker Compose: Qdrant + llama.cpp embed service");
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
    console.log("\nLexical search needs no service; CodeStory stores project-local SQLite FTS shards.");
  }

  return failed;
}

const EMBED_MODELS_MANIFEST = path.join(
  repoRoot,
  "crates",
  "codestory-retrieval",
  "assets",
  "embedding-models.json",
);
function pinnedEmbedModel() {
  const models = JSON.parse(fs.readFileSync(EMBED_MODELS_MANIFEST, "utf8")).models ?? [];
  if (models.length !== 1) {
    throw new Error(`${EMBED_MODELS_MANIFEST} must contain exactly one product model`);
  }
  return models[0];
}

function embedModelDir() {
  const model = pinnedEmbedModel();
  return path.join(codestoryCacheRoot(), "managed-embeddings", "models", "sha256", model.sha256);
}

const LLAMA_BACKENDS_MANIFEST = path.join(
  repoRoot,
  "crates",
  "codestory-retrieval",
  "assets",
  "llama-sidecar-backends.json",
);
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
    (osName === "macos" && arch === "aarch64"
      ? "metal"
      : osName === "windows" && arch === "x86_64"
        ? "vulkan"
        : "");
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

function printLlamaServerPlan(backend) {
  console.log("\nManaged llama-server:");
  console.log(`  backend: ${backend.id}`);
  console.log(`  artifact: ${backend.artifact}`);
  console.log(`  artifact_bytes: ${backend.artifact_bytes}`);
  console.log(`  url: ${backend.url}`);
  console.log(`  sha256: ${backend.sha256}`);
  console.log(`  executable_archive_path: ${backend.executable_archive_path}`);
  console.log(`  executable_sha256: ${backend.executable_sha256}`);
  console.log(`  target: ${managedLlamaServerPath(backend)}`);
}

function managedAssetPrewarmArgs(opts) {
  const args = ["retrieval", "prewarm-assets"];
  if (opts.fetchEmbedModel) args.push("--model");
  if (opts.fetchLlamaServer) args.push("--native-backend");
  if (opts.fetchLlamaServer && opts.llamaBackend) {
    args.push("--llama-backend", opts.llamaBackend);
  }
  return args;
}

function runManagedAssetPrewarm(opts) {
  const args = managedAssetPrewarmArgs(opts);
  if (opts.skipBuild) {
    const cli = cliPath(opts.release);
    if (!fs.existsSync(cli)) {
      throw new Error(`CLI not found at ${cli}; drop --skip-build to build it.`);
    }
    runChecked("Prewarm managed retrieval assets", cli, args);
    return;
  }
  runChecked(
    "Prewarm managed retrieval assets",
    "cargo",
    cargoRunArgs(...(opts.release ? ["--release"] : []), "-p", "codestory-cli", "--", ...args),
  );
}

function runSelfTest() {
  const model = pinnedEmbedModel();
  if (
    model.filename !== "bge-base-en-v1.5.Q8_0.gguf" ||
    !Number.isSafeInteger(model.artifact_bytes) ||
    !/^[0-9a-f]{64}$/.test(model.sha256) ||
    !Array.isArray(model.urls) ||
    model.urls.length === 0
  ) {
    throw new Error("managed embedding model metadata is incomplete");
  }
  const backends = readLlamaBackends();
  const macMetal = backends.find((backend) => backend.id === "macos-aarch64-metal");
  const winVulkan = backends.find((backend) => backend.id === "windows-x86_64-vulkan");
  const winLegacy = backends.find(
    (backend) => backend.id === "windows-x86_64-vulkan-b9058-legacy",
  );
  if (!macMetal) {
    throw new Error("missing macos-aarch64-metal backend");
  }
  if (!winVulkan) {
    throw new Error("missing windows-x86_64-vulkan backend");
  }
  if (!winLegacy || !winLegacy.sha256 || !winLegacy.executable_sha256) {
    throw new Error("missing checksum-backed legacy Windows Vulkan managed-cache fallback");
  }
  if (
    backends.some(
      (backend) =>
        backend.url && (!Number.isSafeInteger(backend.artifact_bytes) || backend.artifact_bytes <= 0),
    )
  ) {
    throw new Error("downloadable llama-server backends must declare artifact_bytes");
  }
  if (backends.some((backend) => backend.os === "macos" && backend.arch !== "aarch64")) {
    throw new Error("macOS Intel llama-server backend must not be present");
  }
  if (!macMetal.managed_cache_rel_dir.includes("/llama/b9902/")) {
    throw new Error(`unexpected managed cache version path: ${macMetal.managed_cache_rel_dir}`);
  }
  if (macMetal.executable_archive_path !== "llama-b9902/llama-server") {
    throw new Error(`unexpected executable archive path: ${macMetal.executable_archive_path}`);
  }
  if (!/^[0-9a-f]{64}$/.test(macMetal.executable_sha256)) {
    throw new Error(`unexpected executable sha256: ${macMetal.executable_sha256}`);
  }
  if (winVulkan.executable_archive_path !== "llama-server.exe") {
    throw new Error(`unexpected Windows executable archive path: ${winVulkan.executable_archive_path}`);
  }
  if (!winVulkan.managed_cache_rel_dir.includes("/llama/b9902/")) {
    throw new Error(`unexpected Windows managed cache version path: ${winVulkan.managed_cache_rel_dir}`);
  }
  if (!/^[0-9a-f]{64}$/.test(winVulkan.executable_sha256)) {
    throw new Error(`unexpected Windows executable sha256: ${winVulkan.executable_sha256}`);
  }
  const selected = selectedLlamaBackend({ llamaBackend: "macos-aarch64-metal" });
  if (managedLlamaServerPath(selected).split(path.sep).join("/").endsWith("llama-server") !== true) {
    throw new Error("managed llama-server target path should end in llama-server");
  }
  const prewarmArgs = managedAssetPrewarmArgs({
    fetchEmbedModel: true,
    fetchLlamaServer: true,
    llamaBackend: "macos-aarch64-metal",
  });
  if (
    JSON.stringify(prewarmArgs) !==
    JSON.stringify([
      "retrieval",
      "prewarm-assets",
      "--model",
      "--native-backend",
      "--llama-backend",
      "macos-aarch64-metal",
    ])
  ) {
    throw new Error(`managed asset prewarm command drifted: ${prewarmArgs.join(" ")}`);
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

  if (opts.fetchEmbedModel || opts.fetchLlamaServer) {
    runManagedAssetPrewarm(opts);
  }
  if (opts.fetchOnly) {
    console.log("\nFetch-only setup complete.");
    return;
  }

  const bootstrapArgs = cargoRunArgs(
    "-p",
    "codestory-cli",
    "--",
    "retrieval",
    "bootstrap",
    "--project",
    opts.project,
    "--wait-secs",
    String(opts.waitSecs),
  );
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
      runChecked(
        "Retrieval status",
        "cargo",
        cargoRunArgs(
          "-p",
          "codestory-cli",
          ...(opts.release ? ["--release"] : []),
          "--",
          "retrieval",
          "status",
          "--project",
          opts.project,
        ),
      );
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
  console.log(
    "Next: cargo run --locked -p codestory-cli -- retrieval index --project <repo-root> --refresh auto",
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});
