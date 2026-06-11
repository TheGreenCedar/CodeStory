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
  --fetch-embed-model       Download bge-base-en-v1.5.Q8_0.gguf into target/retrieval-models
  --release                 Build and use release CLI (default: debug for speed)
  --project <path>          Project root for status (default: repo root)
  --wait-secs <n>           Bootstrap wait timeout (default: 90)

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
    release: false,
    project: repoRoot,
    waitSecs: 90,
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
    throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(opts.waitSecs) || opts.waitSecs < 0) {
    throw new Error("--wait-secs must be a non-negative integer");
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
    ["cargo", commandExists("cargo"), opts.skipBuild ? "optional (--skip-build)" : "required"],
    [
      "docker",
      commandExists("docker"),
      opts.skipCompose ? "optional (--skip-compose)" : "required for live Qdrant",
    ],
    [
      `compose file (${composeFile})`,
      fs.existsSync(composeFile),
      "required unless CODESTORY_RETRIEVAL_COMPOSE_FILE points elsewhere",
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
        `-v "${path.join(cacheRoot, "qdrant")}:/qdrant/storage" qdrant/qdrant:v1.12.5`,
    );
    console.log("\nZoekt without compose:");
    console.log("  run sourcegraph/zoekt-webserver on 127.0.0.1:6070 with the CodeStory shard directory mounted");
  }

  return failed;
}

const BGE_GGUF = "bge-base-en-v1.5.Q8_0.gguf";
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

async function fetchEmbedModel() {
  const dir = embedModelDir();
  fs.mkdirSync(dir, { recursive: true });
  const dest = path.join(dir, BGE_GGUF);
  if (fs.existsSync(dest) && fs.statSync(dest).size > 1_000_000) {
    console.log(`Embed model already present: ${dest}`);
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
    fs.writeFileSync(dest, buffer);
    console.log(`Wrote ${dest} (${buffer.length} bytes)`);
    return dest;
  }
  throw new Error(`Failed to download embed model: ${lastError ?? "no URLs configured"}`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const failed = printPrereqReport(opts);
  if (opts.checkOnly) {
    process.exit(failed ? 1 : 0);
  }
  if (failed && !opts.skipCompose) {
    throw new Error("Fix missing prerequisites (or use --skip-compose / --skip-build where applicable).");
  }

  if (opts.fetchEmbedModel) {
    await fetchEmbedModel();
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
