#!/usr/bin/env node
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadTasks, materializeRepos } from "./codestory-agent-ab-benchmark.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const defaultRepoCacheRoot = path.join(repoRoot, "target", "agent-benchmark", "repos");

function usage() {
  console.log(`Usage:
  node scripts/fetch-holdout-repos.mjs [--repo-cache-dir path] [--timeout-ms ms]

Clones or updates pinned OSS repos for the holdout-retrieval suite into
target/agent-benchmark/repos (gitignored). Same materialization path as
codestory-agent-ab-benchmark.mjs --materialize-repos.
`);
}

function parseArgs(argv) {
  const opts = {
    repoCacheDir: defaultRepoCacheRoot,
    timeoutMs: 1_800_000,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
    if (arg === "--repo-cache-dir") {
      opts.repoCacheDir = path.resolve(argv[++i]);
      continue;
    }
    if (arg === "--timeout-ms") {
      opts.timeoutMs = Number.parseInt(argv[++i], 10);
      continue;
    }
    throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(opts.timeoutMs) || opts.timeoutMs < 1000) {
    throw new Error("--timeout-ms must be an integer >= 1000");
  }
  return opts;
}

async function main() {
  const cliOpts = parseArgs(process.argv.slice(2));
  const opts = {
    taskSuite: "holdout-retrieval",
    taskManifest: null,
    taskIds: null,
    materializeRepos: true,
    repoCacheDir: cliOpts.repoCacheDir,
    timeoutMs: cliOpts.timeoutMs,
  };
  const tasks = await loadTasks(opts);
  await materializeRepos(tasks, opts);
  const repos = [...new Set(tasks.map((task) => task.repo))].sort();
  console.log(
    `materialized holdout-retrieval repos (${repos.join(", ")}) under ${opts.repoCacheDir}`,
  );
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
