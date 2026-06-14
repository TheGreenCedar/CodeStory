#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { benchmarkChildEnv } from "./codestory-benchmark-contract.mjs";

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
../rootandruntime, and this repo. The gap list is seeded from the three manual
subagent tracks: packet-first repo explanation, semantic health, broad-search
drift, grounding recommendation quality, object/config trails, snippet context,
format validation, and current skill guidance. Emits METRIC quality_gap=<count>
and writes a JSON report under target/autoresearch/.`);
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

const startedAt = new Date().toISOString();
const reportRunPath = join(
  artifactDir,
  `codestory-manual-friction-report-${startedAt.replace(/[:.]/g, "-")}.json`,
);

const report = {
  started_at: startedAt,
  repo_root: repoRoot,
  cli_path: cliPath,
  mode: quick ? "quick" : "full",
  refresh_full: refreshFull,
  setup_embeddings: setupEmbeddings,
  repos: [],
  gaps: [],
};
report.skill_text = readFileSync(join(repoRoot, ".agents", "skills", "codestory-grounding", "SKILL.md"), "utf8");

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
    env: benchmarkChildEnv(process.env, { NO_COLOR: "1" }),
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

function explanationAnnotations(value) {
  return (value ?? []).filter((annotation) => {
    const text = String(annotation ?? "").toLowerCase();
    return !text.startsWith("index freshness ");
  });
}

function repoBadTerms(repoName) {
  if (repoName === "Sourcetrail") {
    return ["javaparser", "bin/app/user/projects", "src/external", "testing/", "std::"];
  }
  if (repoName === "rootandruntime") {
    return [
      "clone-libsql-db",
      "readdependencies",
      "scripts/qa",
      "joinurl",
      "mergeadjacenttextnodes",
      "migrate-wordpress",
    ];
  }
  return [
    "repo_text_excerpt",
    "looks_like_repo_text_query",
    "codestory-manual-friction-check",
    "is_repo_explanation_prompt",
    "repobadterms",
  ];
}

function duplicateValues(values) {
  const seen = new Set();
  const duplicates = new Set();
  for (const value of values) {
    const key = String(value ?? "").trim().toLowerCase();
    if (!key) continue;
    if (seen.has(key)) {
      duplicates.add(key);
    }
    seen.add(key);
  }
  return [...duplicates];
}

function checkGroundingRecommendations(repoName, groundJson) {
  const queries = Array.isArray(groundJson?.recommended_queries)
    ? groundJson.recommended_queries
    : [];
  if (queries.length === 0) {
    addGap(repoName, "ground_no_recommendations", 2, "ground output has no recommended queries");
    return;
  }
  const duplicates = duplicateValues(queries.slice(0, 5));
  if (duplicates.length > 0) {
    addGap(repoName, "ground_duplicate_recommendations", 2, "ground recommendations contain duplicate top queries", {
      duplicates,
      recommended_queries: queries,
    });
  }
  const recommendationText = fieldText(queries);
  for (const term of repoBadTerms(repoName)) {
    if (recommendationText.includes(term)) {
      addGap(repoName, "ground_low_value_recommendation", 2, `ground recommended low-value anchor ${term}`, {
        recommended_queries: queries,
      });
    }
  }
}

function checkBroadSearch(repoName, searchJson) {
  const topHits = [
    ...(searchJson?.suggestions ?? []).slice(0, 10),
    ...(searchJson?.indexed_symbol_hits ?? []).slice(0, 10),
    ...(searchJson?.repo_text_hits ?? []).slice(0, 5),
  ];
  if (topHits.length === 0) {
    addGap(repoName, "search_no_architecture_hits", 1, "broad architecture search returned no anchors");
    return;
  }
  const topText = fieldText(topHits);
  for (const term of repoBadTerms(repoName)) {
    if (topText.includes(term)) {
      addGap(repoName, "search_architecture_drift", 1, `broad architecture search drifted into ${term}`, {
        top_hits: topHits.map((hit) => ({
          display_name: hit.display_name,
          file_path: hit.file_path,
          origin: hit.origin,
        })),
      });
    }
  }
}

function checkPacket(repoName, packetJson) {
  const sufficiency = packetJson?.sufficiency ?? {};
  const status = sufficiency.status ?? null;
  if (!["sufficient", "partial"].includes(status)) {
    addGap(repoName, "packet_status_unusable", 1, "packet output is neither sufficient nor partial", {
      status,
    });
  }
  const citations = packetJson?.answer?.citations ?? [];
  const avoidOpening = sufficiency.avoid_opening ?? [];
  const avoidOpeningPaths = sufficiency.avoid_opening_paths ?? null;
  if (!Array.isArray(citations) || citations.length === 0) {
    addGap(repoName, "packet_missing_citations", 1, "packet answer has no structured citations");
  }
  if (
    status === "sufficient" &&
    Array.isArray(sufficiency.follow_up_commands) &&
    sufficiency.follow_up_commands.length > 0
  ) {
    addGap(repoName, "packet_sufficient_with_followups", 2, "sufficient packet still asks for follow-up commands", {
      follow_up_commands: sufficiency.follow_up_commands,
    });
  }
  if (avoidOpening != null && !Array.isArray(avoidOpening)) {
    addGap(repoName, "packet_avoid_opening_malformed", 2, "packet sufficiency avoid_opening is not a list");
  }
  if (!Array.isArray(avoidOpeningPaths)) {
    addGap(
      repoName,
      "packet_avoid_opening_paths_malformed",
      2,
      "packet sufficiency avoid_opening_paths is not a raw path list",
    );
  }
  const retrievalTrace = packetJson?.answer?.retrieval_trace;
  if (!retrievalTrace || typeof retrievalTrace !== "object") {
    addGap(repoName, "packet_missing_retrieval_trace", 2, "packet answer does not expose retrieval trace telemetry");
  }
  const text = fieldText({
    citations,
    sufficiency,
    sections: packetJson?.answer?.sections ?? [],
    supported_claims: packetJson?.answer?.supported_claims ?? [],
    annotations: explanationAnnotations(retrievalTrace?.annotations),
  });
  for (const term of repoBadTerms(repoName)) {
    if (text.includes(term)) {
      addGap(repoName, "packet_drift", 1, `packet output drifted into ${term}`);
    }
  }
}

function checkRootRuntimePostsTrail(trailJson) {
  const nodes = trailJson?.trail?.trail?.nodes ?? [];
  const edges = trailJson?.trail?.trail?.edges ?? [];
  const labels = fieldText(nodes);
  const notes = fieldText(trailJson?.notes ?? []);
  if (edges.length === 0 && !notes.includes("object/config exports")) {
    addGap(
      "rootandruntime",
      "object_config_trail_empty",
      1,
      "trail Posts still returns an empty object/config graph without fallback guidance",
      { nodes: nodes.length, edges: edges.length },
    );
  }
  if (edges.length > 0 && !labels.includes("fields") && !labels.includes("hooks") && !labels.includes("access")) {
    addGap(
      "rootandruntime",
      "object_config_trail_missing_config_members",
      2,
      "trail Posts has edges but does not expose recognizable config members",
      { labels: nodes.map((node) => node.label) },
    );
  }
}

function checkSkillGuidance(repoName) {
  if (repoName !== "codestory") {
    return;
  }
  const skillText = String(report.skill_text ?? "").toLowerCase();
  if (!skillText.includes("start broad repo") || !skillText.includes("`packet`")) {
    addGap(repoName, "skill_missing_packet_flow", 1, "skill does not route broad repo explanations through packet");
  }
  if (!skillText.includes("sufficiency.follow_up_commands")) {
    addGap(repoName, "skill_missing_followup_contract", 2, "skill does not name the packet follow-up contract");
  }
  if (!skillText.includes("do not pass broad natural-language questions directly to `context`")) {
    addGap(repoName, "skill_missing_context_boundary", 2, "skill does not preserve packet-before-context boundary");
  }
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
    if (ground.json) {
      checkGroundingRecommendations(repoName, ground.json);
    }

    const broadSearch = runJson(repoName, [
      "search",
      "--project",
      repoPath,
      "--query",
      "How does this repo fit together?",
      "--why",
      "--format",
      "json",
    ]);
    repoReport.commands.push(compactTranscript(broadSearch.result));
    if (broadSearch.json) {
      checkBroadSearch(repoName, broadSearch.json);
    }

    const packet = runJson(repoName, [
      "packet",
      "--project",
      repoPath,
      "--question",
      "How does this repo fit together?",
      "--budget",
      "compact",
      "--refresh",
      "none",
      "--format",
      "json",
    ], 240_000);
    repoReport.commands.push(compactTranscript(packet.result));
    if (packet.json) {
      checkPacket(repoName, packet.json);
    }

    if (repoName === "rootandruntime") {
      const postsTrail = runJson(repoName, [
        "trail",
        "--project",
        repoPath,
        "--query",
        "Posts",
        "--format",
        "json",
      ]);
      repoReport.commands.push(compactTranscript(postsTrail.result));
      if (postsTrail.json) {
        checkRootRuntimePostsTrail(postsTrail.json);
      }
    }
    checkSkillGuidance(repoName);
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
writeFileSync(reportRunPath, `${JSON.stringify(report, null, 2)}\n`);

console.log(`CodeStory manual-friction check: quality_gap=${qualityGap}`);
for (const gap of report.gaps) {
  console.log(`GAP P${gap.severity} ${gap.repo} ${gap.code}: ${gap.message}`);
}
console.log(`METRIC quality_gap=${qualityGap}`);
console.log(`METRIC repos_checked=${report.repos.length}`);
console.log(`ARTIFACT manual_friction_report=${reportPath}`);
console.log(`ARTIFACT manual_friction_report_run=${reportRunPath}`);

if (failOnGap && qualityGap > 0) {
  process.exit(1);
}
