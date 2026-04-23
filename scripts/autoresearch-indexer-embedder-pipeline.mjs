import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const root = path.resolve(process.env.CODESTORY_PIPELINE_ROOT ?? process.cwd());
const mode = process.env.CODESTORY_PIPELINE_OBSERVATION_MODE ?? "live";
const budget = parsePositiveInt(process.env.CODESTORY_PIPELINE_EXPERIMENT_BUDGET, 100);
const liveBudget = parsePositiveInt(process.env.CODESTORY_PIPELINE_LIVE_BUDGET, mode === "live" ? 1 : budget);
const requiredQueryCount = parsePositiveInt(process.env.CODESTORY_PIPELINE_REQUIRED_QUERY_COUNT, 150);
const buildReleaseBeforeLive = parseBool(process.env.CODESTORY_PIPELINE_BUILD_RELEASE, mode === "live");
const outputRoot = path.join(root, "target", "autoresearch", "indexer-embedder");
const stamp = new Date().toISOString().replaceAll(/[-:]/g, "").replace(/\..+/, "");
const runDir = path.join(outputRoot, stamp);
const benchmarkScript = path.join(root, "scripts", "embedding-gpu-fair-benchmark.mjs");
const scoreScale = 1_000_000;
const scoreWeights = {
  quality: 0.6,
  speed: 0.3,
  footprint: 0.1,
};

fs.mkdirSync(runDir, { recursive: true });

function parsePositiveInt(value, fallback) {
  if (value === undefined || value === "") {
    return fallback;
  }
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`expected positive integer, got ${value}`);
  }
  return parsed;
}

function parseBool(value, fallback) {
  if (value === undefined || value === "") {
    return fallback;
  }
  return /^(1|true|yes|on)$/i.test(String(value));
}

function number(value, fallback = 0) {
  return Number.isFinite(Number(value)) ? Number(value) : fallback;
}

const precisionBytes = {
  float32: 4,
  float16: 2,
  int8: 1,
  uint8: 1,
  binary: 1 / 8,
  ubinary: 1 / 8,
};

function clamp01(value) {
  return Math.min(1, Math.max(0, value));
}

function embeddingDimension(row) {
  const expectedDim = number(row.expectedDim ?? row.expected_dim, NaN);
  if (Number.isFinite(expectedDim) && expectedDim > 0) {
    return expectedDim;
  }
  const truncateDim = number(row.truncateDim ?? row.truncate_dim, NaN);
  if (Number.isFinite(truncateDim) && truncateDim > 0) {
    return truncateDim;
  }
  if (row.profile === "qwen3-embedding-0.6b") {
    return 1024;
  }
  if (
    row.profile === "bge-base-en-v1.5" ||
    row.profile === "nomic-embed-text-v1.5" ||
    row.profile === "nomic-embed-text-v2-moe" ||
    row.profile === "embeddinggemma-300m"
  ) {
    return 768;
  }
  return 384;
}

function persistedVectorBytesPerDoc(row) {
  const explicit = number(row.persisted_vector_bytes_per_doc ?? row.vector_bytes_per_doc, NaN);
  if (Number.isFinite(explicit) && explicit > 0) {
    return explicit;
  }
  const dimension = embeddingDimension(row);
  const precision = row.vectorEncoding ?? row.vector_encoding ?? "float32";
  if (precision === "int8") {
    const versionedHeaderBytes = 9;
    return versionedHeaderBytes + dimension * precisionBytes.int8;
  }
  return embeddingDimension(row) * precisionBytes.float32;
}

function prefilterVectorBytesPerDoc(row) {
  const explicit = number(row.prefilter_vector_bytes_per_doc, NaN);
  if (Number.isFinite(explicit) && explicit > 0) {
    return explicit;
  }
  const precision = row.vectorEncoding ?? row.vector_encoding ?? "float32";
  const bytes = precisionBytes[precision] ?? precisionBytes.float32;
  return embeddingDimension(row) * bytes;
}

function queryCount(row) {
  if (Array.isArray(row?.queries)) {
    return row.queries.length;
  }
  return number(row?.query_count, 0);
}

function isDecisionGrade(row) {
  return Boolean(
    row &&
      !row.error &&
      !row.skipped &&
      queryCount(row) >= requiredQueryCount &&
      row.provider_verified === true &&
      row.score &&
      Number.isFinite(Number(row.score.mrr_at_10)) &&
      Number.isFinite(Number(row.score.hit_at_10)),
  );
}

function qualityComponent(row) {
  const score = row.score ?? {};
  const mrr = number(score.mrr_at_10);
  const hit10 = number(score.hit_at_10);
  const hit1 = number(score.hit_at_1);
  const persistentHit10 = number(score.persistent_hit_at_10, hit10);
  return 0.42 * mrr + 0.24 * hit10 + 0.18 * hit1 + 0.16 * persistentHit10;
}

function speedComponent(row) {
  const docsPerSecond = clamp01(number(row.docs_per_second) / 900);
  const indexSeconds = number(row.index_seconds);
  const indexPace = indexSeconds > 0 && !row.cache_replay_from ? clamp01(45 / indexSeconds) : 0;
  const searchP95 = number(row.search_query_ms_p95);
  const searchPace = searchP95 > 0 ? clamp01(250 / searchP95) : 0;
  return 0.55 * docsPerSecond + 0.35 * indexPace + 0.1 * searchPace;
}

function footprintComponent(row) {
  const modelSizeMb = number(row.model_size_mb, 600);
  const vectorBytesPerDoc = persistedVectorBytesPerDoc(row);
  const model = 1 - clamp01(modelSizeMb / 600);
  const vectors = 1 - clamp01(vectorBytesPerDoc / 4096);
  return 0.65 * model + 0.35 * vectors;
}

function scorePipeline(row) {
  if (!isDecisionGrade(row)) {
    return {
      pipeline_score: 0,
      quality_component: 0,
      speed_component: 0,
      footprint_component: 0,
    };
  }
  const quality = qualityComponent(row);
  const speed = speedComponent(row);
  const footprint = footprintComponent(row);
  return {
    pipeline_score:
      scoreScale *
      (scoreWeights.quality * quality + scoreWeights.speed * speed + scoreWeights.footprint * footprint),
    quality_component: quality,
    speed_component: speed,
    footprint_component: footprint,
  };
}

function casePriority(caseId) {
  const id = caseId.toLowerCase();
  let priority = 0;
  if (id.includes("runtime-default")) priority += 180;
  if (id.includes("frontier-b512-r4")) priority += 120;
  if (id.includes("q5")) priority += 95;
  if (id.includes("fast-profile") || id.includes("fast-bge-small")) priority += 90;
  if (id.includes("pure-semantic") || id.includes("semantic9") || id.includes("slim")) {
    priority += 80;
  }
  if (id.includes("crossed-scope-all-no-alias")) priority += 78;
  if (id.includes("default") || id.includes("baseline")) priority += 70;
  if (id.includes("weight") || id.includes("vec-")) priority += 60;
  if (id.includes("bge-small") || id.includes("bge-base")) priority += 55;
  if (id.includes("nomic") || id.includes("qwen") || id.includes("gemma")) priority += 30;
  if (id.includes("q4") || id.includes("binary") || id.includes("dim-64")) priority -= 20;
  return priority;
}

function allExistingRows() {
  const roots = [
    path.join(root, "target", "embedding-research"),
    path.join(root, "target", "autoresearch", "indexer-embedder"),
  ];
  const rows = [];
  for (const artifactRoot of roots) {
    if (!fs.existsSync(artifactRoot)) {
      continue;
    }
    for (const entry of fs.readdirSync(artifactRoot, { withFileTypes: true })) {
      if (!entry.isDirectory()) {
        continue;
      }
      const candidates = [
        {
          resultsPath: path.join(artifactRoot, entry.name, "results.json"),
          artifactRoot: path.join(artifactRoot, entry.name),
          observedAt: entry.name,
        },
        {
          resultsPath: path.join(artifactRoot, entry.name, "live", "results.json"),
          artifactRoot: path.join(artifactRoot, entry.name, "live"),
          observedAt: `${entry.name}/live`,
        },
      ];
      for (const candidate of candidates) {
        if (!fs.existsSync(candidate.resultsPath)) {
          continue;
        }
        try {
          const parsed = JSON.parse(fs.readFileSync(candidate.resultsPath, "utf8"));
          if (!Array.isArray(parsed)) {
            continue;
          }
          for (const row of parsed) {
            rows.push({
              ...row,
              artifact_root: candidate.artifactRoot,
              observed_at: candidate.observedAt,
            });
          }
        } catch (error) {
          rows.push({
            case_id: `${candidate.observedAt}:results-json-parse-failed`,
            error: error.message,
            artifact_root: candidate.artifactRoot,
            observed_at: candidate.observedAt,
          });
        }
      }
    }
  }
  return rows;
}

function latestDistinctRows(rows) {
  const byCase = new Map();
  for (const row of rows) {
    if (!row.case_id) {
      continue;
    }
    const previous = byCase.get(row.case_id);
    if (!previous || String(row.observed_at) > String(previous.observed_at)) {
      byCase.set(row.case_id, row);
    }
  }
  return [...byCase.values()];
}

function annotateRows(rows) {
  return rows.map((row) => ({
    ...row,
    ...scorePipeline(row),
    case_priority: casePriority(row.case_id ?? ""),
  }));
}

function selectArtifactRows() {
  const rows = annotateRows(latestDistinctRows(allExistingRows()));
  return rows
    .sort(
      (left, right) =>
        Number(isDecisionGrade(right)) - Number(isDecisionGrade(left)) ||
        right.case_priority - left.case_priority ||
        right.pipeline_score - left.pipeline_score ||
        String(right.observed_at).localeCompare(String(left.observed_at)),
    )
    .slice(0, budget);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    env: { ...process.env, ...options.env },
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 64,
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed with ${result.status}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  }
  return result.stdout;
}

function listCases() {
  const stdout = run(process.execPath, [benchmarkScript], {
    env: {
      CODESTORY_EMBED_RESEARCH_STAGE: "all",
      CODESTORY_EMBED_RESEARCH_LIST: "1",
      CODESTORY_EMBED_RESEARCH_OUT_DIR: path.join(runDir, "manifest-only"),
    },
  });
  return stdout
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

function selectLiveCases() {
  const requested = (process.env.CODESTORY_PIPELINE_CASES ?? "")
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  if (requested.length > 0) {
    return requested.slice(0, liveBudget);
  }
  return listCases()
    .sort((left, right) => casePriority(right) - casePriority(left))
    .slice(0, liveBudget);
}

function runLiveRows() {
  if (buildReleaseBeforeLive) {
    run("cargo", ["build", "--release", "-p", "codestory-cli"]);
  }
  const cases = selectLiveCases();
  fs.writeFileSync(path.join(runDir, "live-cases.txt"), `${cases.join("\n")}\n`);
  const liveDir = path.join(runDir, "live");
  const queryBuckets = process.env.CODESTORY_PIPELINE_QUERY_BUCKETS ?? "";
  const queryLimit = process.env.CODESTORY_PIPELINE_QUERY_LIMIT ?? "";
  run(process.execPath, [benchmarkScript], {
    env: {
      CODESTORY_EMBED_RESEARCH_STAGE: "all",
      CODESTORY_EMBED_RESEARCH_CASES: cases.join(","),
      CODESTORY_EMBED_RESEARCH_QUERY_BUCKETS: queryBuckets,
      CODESTORY_EMBED_RESEARCH_QUERY_LIMIT: queryLimit,
      CODESTORY_EMBED_RESEARCH_OUT_DIR: liveDir,
    },
  });
  const resultsPath = path.join(liveDir, "results.json");
  const rows = fs.existsSync(resultsPath) ? JSON.parse(fs.readFileSync(resultsPath, "utf8")) : [];
  return annotateRows(
    rows.map((row) => ({
      ...row,
      artifact_root: liveDir,
      observed_at: stamp,
    })),
  );
}

function summarize(rows) {
  const decisionGrade = rows.filter(isDecisionGrade);
  const best = [...decisionGrade].sort((a, b) => b.pipeline_score - a.pipeline_score)[0];
  if (!best) {
    throw new Error("no decision-grade rows available for pipeline_score");
  }
  const skipped = rows.filter((row) => row.skipped).length;
  const failed = rows.filter((row) => row.error).length;
  const summary = {
    generated_at: new Date().toISOString(),
    mode,
    required_query_count: requiredQueryCount,
    metric:
      "pipeline_score = 1000000 * (quality_component * 0.60 + speed_component * 0.30 + footprint_component * 0.10); correctness/provider failures or partial query suites score 0",
    priority_order:
      "weighted priority: quality contributes 60%, speed contributes 30%, and memory footprint contributes 10%.",
    footprint_basis:
      "footprint_component uses model size and persisted vector bytes; quantized prefilter bytes are reported separately and do not count as persisted memory savings.",
    rows_observed: rows.length,
    decision_grade_rows: decisionGrade.length,
    skipped_rows: skipped,
    failed_rows: failed,
    best: compactRow(best),
    top10: [...decisionGrade]
      .sort((a, b) => b.pipeline_score - a.pipeline_score)
      .slice(0, 10)
      .map(compactRow),
  };
  return summary;
}

function compactRow(row) {
  return {
    case_id: row.case_id,
    artifact_root: row.artifact_root,
    query_count: queryCount(row),
    pipeline_score: round(row.pipeline_score, 6),
    quality_component: round(row.quality_component, 9),
    speed_component: round(row.speed_component, 9),
    footprint_component: round(row.footprint_component, 9),
    mrr_at_10: round(row.score?.mrr_at_10, 9),
    hit_at_10: round(row.score?.hit_at_10, 9),
    hit_at_1: round(row.score?.hit_at_1, 9),
    persistent_hit_at_10: round(row.score?.persistent_hit_at_10, 9),
    docs_per_second: round(row.docs_per_second, 6),
    index_seconds: round(row.index_seconds, 6),
    cache_replay_from: row.cache_replay_from ?? "",
    search_query_ms_p95: round(row.search_query_ms_p95, 6),
    model_size_mb: round(row.model_size_mb, 6),
    vector_bytes_per_doc: round(persistedVectorBytesPerDoc(row), 6),
    prefilter_vector_bytes_per_doc: round(prefilterVectorBytesPerDoc(row), 6),
    provider_evidence: row.provider_evidence,
  };
}

function round(value, digits = 6) {
  if (!Number.isFinite(Number(value))) {
    return null;
  }
  const scale = 10 ** digits;
  return Math.round(Number(value) * scale) / scale;
}

function writeSummary(summary, rows) {
  const rowsPath = path.join(runDir, "scored-rows.json");
  const summaryPath = path.join(runDir, "summary.json");
  const summaryMdPath = path.join(runDir, "summary.md");
  fs.writeFileSync(rowsPath, JSON.stringify(rows.map(compactRow), null, 2));
  fs.writeFileSync(summaryPath, JSON.stringify(summary, null, 2));
  fs.writeFileSync(
    summaryMdPath,
    [
      "# Indexer + Embedder Autoresearch Summary",
      "",
      `Mode: \`${summary.mode}\``,
      `Required query count: \`${summary.required_query_count}\``,
      `Rows observed: \`${summary.rows_observed}\``,
      `Decision-grade rows: \`${summary.decision_grade_rows}\``,
      `Skipped rows: \`${summary.skipped_rows}\``,
      `Failed rows: \`${summary.failed_rows}\``,
      "",
      "## Best Row",
      "",
      `- Case: \`${summary.best.case_id}\``,
      `- Pipeline score: \`${summary.best.pipeline_score}\``,
      `- Quality: \`${summary.best.quality_component}\`, speed: \`${summary.best.speed_component}\`, footprint: \`${summary.best.footprint_component}\``,
      `- MRR@10: \`${summary.best.mrr_at_10}\`, Hit@10: \`${summary.best.hit_at_10}\`, Hit@1: \`${summary.best.hit_at_1}\`, Persistent Hit@10: \`${summary.best.persistent_hit_at_10}\``,
      `- Docs/sec: \`${summary.best.docs_per_second}\`, index seconds: \`${summary.best.index_seconds}\``,
      "",
      "## Top 10",
      "",
      "| Rank | Case | Pipeline score | MRR@10 | Hit@10 | Docs/sec |",
      "| ---: | --- | ---: | ---: | ---: | ---: |",
      ...summary.top10.map(
        (row, index) =>
          `| ${index + 1} | \`${row.case_id}\` | ${row.pipeline_score} | ${row.mrr_at_10} | ${row.hit_at_10} | ${row.docs_per_second} |`,
      ),
      "",
    ].join("\n"),
  );
  fs.copyFileSync(summaryPath, path.join(outputRoot, "latest-summary.json"));
  fs.copyFileSync(summaryMdPath, path.join(outputRoot, "latest-summary.md"));
}

function emitMetrics(summary) {
  const best = summary.best;
  console.log(`summary: ${path.join(runDir, "summary.md")}`);
  console.log(`METRIC pipeline_score=${best.pipeline_score}`);
  console.log(`METRIC quality_component=${best.quality_component}`);
  console.log(`METRIC speed_component=${best.speed_component}`);
  console.log(`METRIC footprint_component=${best.footprint_component}`);
  console.log(`METRIC experiments_observed=${summary.rows_observed}`);
  console.log(`METRIC decision_grade_rows=${summary.decision_grade_rows}`);
  console.log(`METRIC best_mrr_at_10=${best.mrr_at_10}`);
  console.log(`METRIC best_hit_at_10=${best.hit_at_10}`);
  console.log(`METRIC best_docs_per_second=${best.docs_per_second}`);
  console.log(`METRIC best_index_seconds=${best.index_seconds}`);
}

const rows = mode === "live" ? runLiveRows() : selectArtifactRows();
const summary = summarize(rows);
writeSummary(summary, rows);
emitMetrics(summary);
