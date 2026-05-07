#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const repoRoot = resolve(process.cwd());
const artifactDir = join(repoRoot, "target", "autoresearch");
const reportPath = join(artifactDir, "codestory-manual-friction-report.json");
const cliPath = process.platform === "win32"
  ? join(repoRoot, "target", "release", "codestory-cli.exe")
  : join(repoRoot, "target", "release", "codestory-cli");

const args = new Set(process.argv.slice(2));
if (args.has("--help") || args.has("-h")) {
  console.log(`Usage: node scripts/codestory-manual-friction-check.mjs [--quick] [--no-refresh] [--setup-embeddings] [--fail-on-gap]

Runs the CodeStory skill-first manual-friction harness across ../Sourcetrail,
../rootandruntime, and this repo. Emits METRIC quality_gap=<count> and writes a
JSON report under target/autoresearch/.`);
  process.exit(0);
}
const quick = args.has("--quick");
const refreshFull = !quick && !args.has("--no-refresh");
const failOnGap = args.has("--fail-on-gap");
const setupEmbeddings =
  args.has("--setup-embeddings") ||
  process.env.CODESTORY_MANUAL_FRICTION_SETUP_EMBEDDINGS === "1";

const repos = [
  resolve(repoRoot, "..", "Sourcetrail"),
  resolve(repoRoot, "..", "rootandruntime"),
  repoRoot,
];

mkdirSync(artifactDir, { recursive: true });

const report = {
  started_at: new Date().toISOString(),
  repo_root: repoRoot,
  cli_path: cliPath,
  mode: quick ? "quick" : "full",
  refresh_full: refreshFull,
  setup_embeddings: setupEmbeddings,
  repos: [],
  gaps: [],
};

function addGap(repo, code, severity, message, details = {}) {
  report.gaps.push({
    repo,
    code,
    severity,
    message,
    details,
  });
}

function runCli(commandArgs, options = {}) {
  const result = spawnSync(cliPath, commandArgs, {
    cwd: repoRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      NO_COLOR: "1",
    },
    timeout: options.timeoutMs ?? 180_000,
  });
  return {
    args: commandArgs,
    status: result.status,
    signal: result.signal,
    error: result.error ? String(result.error.message ?? result.error) : null,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function parseJson(stdout) {
  try {
    return JSON.parse(stdout);
  } catch {
    const start = stdout.indexOf("{");
    const end = stdout.lastIndexOf("}");
    if (start >= 0 && end > start) {
      return JSON.parse(stdout.slice(start, end + 1));
    }
    throw new Error("stdout did not contain a JSON object");
  }
}

function compactTranscript(result) {
  return {
    args: result.args,
    status: result.status,
    signal: result.signal,
    error: result.error,
    stdout_tail: result.stdout.slice(-4000),
    stderr_tail: result.stderr.slice(-4000),
  };
}

function fieldText(value) {
  return JSON.stringify(value ?? "").toLowerCase();
}

function repoBadTerms(repoName) {
  if (repoName === "Sourcetrail") {
    return ["javaparser", "bin/app/user/projects"];
  }
  if (repoName === "rootandruntime") {
    return ["clone-libsql-db", "readdependencies"];
  }
  return [];
}

function semanticStateFromDoctor(repoName, doctor) {
  const retrieval = doctor.retrieval ?? {};
  const stats = doctor.stats ?? {};
  if (retrieval.semantic_ready !== true) {
    addGap(
      repoName,
      "semantic_not_ready",
      1,
      "doctor reports semantic retrieval is not ready",
      { fallback_reason: retrieval.fallback_reason, fallback_message: retrieval.fallback_message },
    );
  }
  if (
    retrieval.semantic_ready === true &&
    Number.isFinite(retrieval.semantic_doc_count) &&
    Number.isFinite(stats.file_count) &&
    stats.file_count > 0 &&
    retrieval.semantic_doc_count < stats.file_count
  ) {
    addGap(
      repoName,
      "semantic_partial",
      1,
      "doctor reports fewer semantic docs than indexed files",
      { semantic_doc_count: retrieval.semantic_doc_count, file_count: stats.file_count },
    );
  }
  const warningChecks = (doctor.checks ?? []).filter((check) => check.status === "warn");
  for (const check of warningChecks) {
    if (String(check.name).startsWith("semantic")) {
      if (String(check.message).startsWith("semantic partial:")) {
        continue;
      }
      addGap(repoName, "semantic_warning", 2, check.message, { check });
    }
  }
}

function firstItem(json, path) {
  let cursor = json;
  for (const segment of path) {
    if (cursor == null) return undefined;
    cursor = cursor[segment];
  }
  return Array.isArray(cursor) ? cursor[0] : cursor;
}

function runJson(repoName, commandArgs, timeoutMs) {
  const result = runCli(commandArgs, { timeoutMs });
  if (result.status !== 0) {
    addGap(repoName, "command_failed", 1, `${commandArgs[0]} failed`, compactTranscript(result));
    return { result, json: null };
  }
  try {
    return { result, json: parseJson(result.stdout) };
  } catch (error) {
    addGap(repoName, "json_parse_failed", 1, `${commandArgs[0]} did not emit parseable JSON`, {
      error: String(error.message ?? error),
      transcript: compactTranscript(result),
    });
    return { result, json: null };
  }
}

if (!existsSync(cliPath)) {
  addGap(
    "all",
    "cli_missing",
    0,
    "release codestory-cli is missing; run cargo build --release -p codestory-cli first",
    { cli_path: cliPath },
  );
} else {
  if (setupEmbeddings) {
    const setup = runCli(["setup", "embeddings", "--project", repoRoot, "--format", "json"], {
      timeoutMs: 600_000,
    });
    report.setup_embeddings_transcript = compactTranscript(setup);
    if (setup.status !== 0) {
      addGap("all", "embedding_setup_failed", 0, "managed embedding setup failed", compactTranscript(setup));
    }
  }

  for (const repoPath of repos) {
    const repoName = basename(repoPath);
    const repoReport = {
      name: repoName,
      path: repoPath,
      commands: [],
    };
    report.repos.push(repoReport);

    if (!existsSync(repoPath)) {
      addGap(repoName, "repo_missing", 0, "manual-test repo does not exist", { path: repoPath });
      continue;
    }

    if (refreshFull) {
      const indexed = runCli([
        "index",
        "--project",
        repoPath,
        "--refresh",
        "full",
        "--format",
        "json",
      ], { timeoutMs: 900_000 });
      repoReport.commands.push(compactTranscript(indexed));
      if (indexed.status !== 0) {
        addGap(repoName, "full_index_failed", 0, "full index failed", compactTranscript(indexed));
        continue;
      }
    }

    const doctor = runJson(repoName, [
      "doctor",
      "--project",
      repoPath,
      "--format",
      "json",
    ]);
    repoReport.commands.push(compactTranscript(doctor.result));
    if (doctor.json) {
      semanticStateFromDoctor(repoName, doctor.json);
    }

    const ground = runJson(repoName, [
      "ground",
      "--project",
      repoPath,
      "--format",
      "json",
    ]);
    repoReport.commands.push(compactTranscript(ground.result));

    const ask = runJson(repoName, [
      "ask",
      "--project",
      repoPath,
      "--investigate",
      "--format",
      "json",
      "How does this repo fit together?",
    ], 240_000);
    repoReport.commands.push(compactTranscript(ask.result));
    if (ask.json) {
      const answerText = fieldText(ask.json);
      if (answerText.includes("low confidence")) {
        addGap(repoName, "ask_low_confidence", 2, "broad explanation ask still reports low confidence");
      }
      for (const term of repoBadTerms(repoName)) {
        if (answerText.includes(term)) {
          addGap(repoName, "ask_drift", 1, `broad explanation drifted into ${term}`);
        }
      }
      if (!answerText.includes("db-first retrieval packet") && !answerText.includes("mode=db_first_no_local_agent")) {
        addGap(repoName, "ask_mode_unlabeled", 2, "ask output does not clearly label DB-first/no-local-agent mode");
      }
    }
  }

  const codeStoryName = basename(repoRoot);
  const symbol = runJson(codeStoryName, [
    "symbol",
    "--project",
    repoRoot,
    "--query",
    "Runtime",
    "--format",
    "json",
  ]);
  const querySymbol = runJson(codeStoryName, [
    "query",
    "--project",
    repoRoot,
    "symbol('Runtime') | limit(1)",
    "--format",
    "json",
  ]);
  if (symbol.json && querySymbol.json) {
    const symbolId = symbol.json.resolution?.resolved?.node_id;
    const queryId = querySymbol.json.items?.[0]?.node_id;
    if (symbolId && queryId && symbolId !== queryId) {
      addGap(codeStoryName, "symbol_query_mismatch", 1, "symbol --query and query symbol() resolve different targets", {
        symbol_id: symbolId,
        query_id: queryId,
        symbol_name: symbol.json.resolution?.resolved?.display_name,
        query_name: querySymbol.json.items?.[0]?.display_name,
      });
    }
  }

  const trail = runJson(codeStoryName, [
    "trail",
    "--project",
    repoRoot,
    "--query",
    "WorkspaceIndexer::run",
    "--format",
    "json",
  ]);
  const queryTrail = runJson(codeStoryName, [
    "query",
    "--project",
    repoRoot,
    "trail(symbol: 'WorkspaceIndexer::run') | limit(120)",
    "--format",
    "json",
  ]);
  if (trail.json && queryTrail.json) {
    const trailNodes = trail.json.trail?.trail?.nodes ?? [];
    const queryItems = queryTrail.json.items ?? [];
    if (trailNodes.length < queryItems.length) {
      addGap(codeStoryName, "trail_query_width_mismatch", 1, "trail --query returns fewer default nodes than query trail()", {
        trail_nodes: trailNodes.length,
        query_items: queryItems.length,
      });
    }
  }

  const snippet = runJson(codeStoryName, [
    "snippet",
    "--project",
    repoRoot,
    "--query",
    "build_llm_symbol_doc_text",
    "--context",
    "60",
    "--format",
    "json",
  ]);
  if (snippet.json) {
    const payload = firstItem(snippet.json, ["snippet"]) ?? snippet.json.snippet;
    if (!payload?.requested_context) {
      addGap(codeStoryName, "snippet_context_unlabeled", 2, "snippet JSON does not report requested_context");
    }
    if (payload?.snippet_truncated === true && !payload?.max_snippet_bytes) {
      addGap(codeStoryName, "snippet_truncation_unbounded", 2, "truncated snippet does not report max_snippet_bytes");
    }
  }
}

const qualityGap = report.gaps.filter((gap) => gap.severity <= 2).length;
report.finished_at = new Date().toISOString();
report.quality_gap = qualityGap;

writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);

console.log(`CodeStory manual-friction check: quality_gap=${qualityGap}`);
for (const gap of report.gaps) {
  console.log(`GAP P${gap.severity} ${gap.repo} ${gap.code}: ${gap.message}`);
}
console.log(`METRIC quality_gap=${qualityGap}`);
console.log(`METRIC repos_checked=${report.repos.length}`);
console.log(`ARTIFACT manual_friction_report=${reportPath}`);

if (failOnGap && qualityGap > 0) {
  process.exit(1);
}
