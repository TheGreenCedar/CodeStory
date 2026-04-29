import fs from "node:fs";
import path from "node:path";

const resultsPath = process.env.CODESTORY_AUTORESEARCH_RESULTS;
if (!resultsPath) {
  throw new Error("CODESTORY_AUTORESEARCH_RESULTS is required");
}

const summaryDir =
  process.env.CODESTORY_AUTORESEARCH_SUMMARY_DIR ?? path.dirname(resultsPath);
fs.mkdirSync(summaryDir, { recursive: true });

const scoreScale = 1_000_000;
const weights = {
  quality: 0.7,
  speed: 0.2,
  memory: 0.1,
};

const minMrrAt10 = number(process.env.CODESTORY_PIPELINE_MIN_MRR_AT_10, 0.98);
const minHitAt10 = number(process.env.CODESTORY_PIPELINE_MIN_HIT_AT_10, 0.99);
const requiredQueryCount = parsePositiveInt(
  process.env.CODESTORY_PIPELINE_REQUIRED_QUERY_COUNT,
  20,
);
const requirePromotionEligible = /^(1|true|yes|on)$/i.test(
  process.env.CODESTORY_PIPELINE_REQUIRE_PROMOTION_ELIGIBLE ?? "",
);

const precisionBytes = {
  float32: 4,
  float16: 2,
  int8: 1,
  uint8: 1,
  binary: 1 / 8,
  ubinary: 1 / 8,
};

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

function number(value, fallback = 0) {
  if (value === null || value === undefined || value === "") {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function clamp01(value) {
  return Math.min(1, Math.max(0, value));
}

function queryCount(row) {
  if (Array.isArray(row?.queries)) {
    return row.queries.length;
  }
  return number(row?.query_count, 0);
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
    return 9 + dimension * precisionBytes.int8;
  }
  return dimension * (precisionBytes[precision] ?? precisionBytes.float32);
}

function prefilterVectorBytesPerDoc(row) {
  const explicit = number(row.prefilter_vector_bytes_per_doc, NaN);
  if (Number.isFinite(explicit) && explicit > 0) {
    return explicit;
  }
  const precision = row.vectorEncoding ?? row.vector_encoding ?? "float32";
  return embeddingDimension(row) * (precisionBytes[precision] ?? precisionBytes.float32);
}

function isDecisionGrade(row) {
  return Boolean(
    row &&
      !row.error &&
      !row.skipped &&
      (!requirePromotionEligible || isPromotionEligible(row)) &&
      queryCount(row) >= requiredQueryCount &&
      row.provider_verified === true &&
      row.score &&
      Number.isFinite(Number(row.score.mrr_at_10)) &&
      Number.isFinite(Number(row.score.hit_at_10)),
  );
}

function isPromotionEligible(row) {
  return row?.promotion_eligible === true || row?.query_split === "holdout";
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

function memoryComponent(row) {
  const modelSizeMb = number(row.model_size_mb, 600);
  const vectorBytes = persistedVectorBytesPerDoc(row);
  const cacheSizeMb = number(row.cache_dir_size_mb, NaN);
  const model = 1 - clamp01(modelSizeMb / 600);
  const vectors = 1 - clamp01(vectorBytes / 4096);
  const cache = Number.isFinite(cacheSizeMb) && cacheSizeMb > 0 ? 1 - clamp01(cacheSizeMb / 750) : vectors;
  return 0.55 * model + 0.25 * vectors + 0.2 * cache;
}

function scorePipeline(row) {
  if (!isDecisionGrade(row)) {
    return emptyScore(row);
  }
  const quality = qualityComponent(row);
  const speed = speedComponent(row);
  const memory = memoryComponent(row);
  const mrrAt10 = number(row.score?.mrr_at_10);
  const hitAt10 = number(row.score?.hit_at_10);
  const gatePass =
    mrrAt10 >= minMrrAt10 && hitAt10 >= minHitAt10;
  const gatePenalty = Math.min(1, mrrAt10 / minMrrAt10) * Math.min(1, hitAt10 / minHitAt10);
  const rawScore =
    scoreScale *
    (weights.quality * quality + weights.speed * speed + weights.memory * memory);
  return {
    pipeline_score: rawScore * gatePenalty,
    raw_pipeline_score: rawScore,
    quality_component: quality,
    speed_component: speed,
    memory_component: memory,
    quality_gate_pass: gatePass,
    quality_gate_penalty: gatePenalty,
    promotion_eligible: isPromotionEligible(row),
    promotion_score: isPromotionEligible(row) ? rawScore * gatePenalty : 0,
  };
}

function emptyScore(row) {
  return {
    pipeline_score: 0,
    raw_pipeline_score: 0,
    quality_component: 0,
    speed_component: 0,
    memory_component: 0,
    quality_gate_pass: false,
    quality_gate_penalty: 0,
    promotion_eligible: isPromotionEligible(row),
    promotion_score: 0,
    rejected_reason: row?.error ?? row?.skipped ?? "not decision grade",
  };
}

function compactRow(row) {
  const vectorBytes = persistedVectorBytesPerDoc(row);
  return {
    case_id: row.case_id,
    query_count: queryCount(row),
    pipeline_score: round(row.pipeline_score, 6),
    raw_pipeline_score: round(row.raw_pipeline_score, 6),
    quality_component: round(row.quality_component, 9),
    speed_component: round(row.speed_component, 9),
    memory_component: round(row.memory_component, 9),
    quality_gate_pass: row.quality_gate_pass,
    quality_gate_penalty: round(row.quality_gate_penalty, 9),
    promotion_eligible: row.promotion_eligible === true,
    promotion_score: round(row.promotion_score, 6),
    query_split: row.query_split ?? "",
    promotion_blocker: row.promotion_blocker ?? "",
    mrr_at_10: round(row.score?.mrr_at_10, 9),
    hit_at_10: round(row.score?.hit_at_10, 9),
    hit_at_1: round(row.score?.hit_at_1, 9),
    persistent_hit_at_10: round(row.score?.persistent_hit_at_10, 9),
    docs_per_second: round(row.docs_per_second, 6),
    index_seconds: round(row.index_seconds, 6),
    semantic_seconds: round(row.semantic_seconds, 6),
    cache_refresh_seconds: round(row.cache_refresh_seconds, 6),
    search_query_ms_p95: round(row.search_query_ms_p95, 6),
    cache_dir_size_mb: round(row.cache_dir_size_mb, 6),
    model_size_mb: round(row.model_size_mb, 6),
    vector_bytes_per_doc: round(vectorBytes, 6),
    prefilter_vector_bytes_per_doc: round(prefilterVectorBytesPerDoc(row), 6),
    provider_verified: row.provider_verified === true,
    provider_evidence: row.provider_evidence ?? "",
    error: row.error ?? "",
  };
}

function round(value, digits = 6) {
  if (value === null || value === undefined || value === "") {
    return null;
  }
  if (!Number.isFinite(Number(value))) {
    return null;
  }
  const scale = 10 ** digits;
  return Math.round(Number(value) * scale) / scale;
}

function emitMetric(name, value) {
  if (Number.isFinite(Number(value))) {
    console.log(`METRIC ${name}=${Number(value)}`);
  }
}

const parsed = JSON.parse(fs.readFileSync(resultsPath, "utf8"));
if (!Array.isArray(parsed)) {
  throw new Error(`${resultsPath} did not contain an array`);
}

const scoredRows = parsed
  .map((row) => ({ ...row, ...scorePipeline(row) }))
  .sort(
    (left, right) =>
      right.pipeline_score - left.pipeline_score ||
      right.raw_pipeline_score - left.raw_pipeline_score,
  );

const decisionGradeRows = scoredRows.filter(isDecisionGrade);
const best = scoredRows.find((row) => row.pipeline_score > 0) ?? decisionGradeRows[0];
if (!best) {
  throw new Error("no decision-grade rows available for pipeline_score");
}

const summary = {
  generated_at: new Date().toISOString(),
  results_path: resultsPath,
  required_query_count: requiredQueryCount,
  require_promotion_eligible: requirePromotionEligible,
  metric:
    "pipeline_score = 1000000 * (0.70 * quality_component + 0.20 * speed_component + 0.10 * memory_component) * quality_gate_penalty",
  promotion_rule:
    "Only rows produced with query_split=holdout are promotion eligible; dev rows can guide experiments but must not be promoted.",
  quality_gate: {
    min_mrr_at_10: minMrrAt10,
    min_hit_at_10: minHitAt10,
  },
  rows_observed: scoredRows.length,
  decision_grade_rows: decisionGradeRows.length,
  best: compactRow(best),
  top10: scoredRows.slice(0, 10).map(compactRow),
};

fs.writeFileSync(path.join(summaryDir, "scored-rows.json"), JSON.stringify(scoredRows.map(compactRow), null, 2));
fs.writeFileSync(path.join(summaryDir, "summary.json"), JSON.stringify(summary, null, 2));
fs.writeFileSync(
  path.join(summaryDir, "summary.md"),
  [
    "# CodeStory Index And Embedding Pipeline Score",
    "",
    `Results: \`${resultsPath}\``,
    `Metric: \`${summary.metric}\``,
    `Promotion rule: ${summary.promotion_rule}`,
    `Promotion eligibility required: \`${requirePromotionEligible}\``,
    `Quality gate: MRR@10 >= \`${minMrrAt10}\`, Hit@10 >= \`${minHitAt10}\``,
    `Decision-grade rows: \`${summary.decision_grade_rows}\` of \`${summary.rows_observed}\``,
    "",
    "## Best Row",
    "",
    `- Case: \`${summary.best.case_id}\``,
    `- Pipeline score: \`${summary.best.pipeline_score}\``,
    `- Promotion eligible: \`${summary.best.promotion_eligible}\`, promotion score: \`${summary.best.promotion_score}\``,
    `- Quality: \`${summary.best.quality_component}\`, speed: \`${summary.best.speed_component}\`, memory: \`${summary.best.memory_component}\``,
    `- MRR@10: \`${summary.best.mrr_at_10}\`, Hit@10: \`${summary.best.hit_at_10}\`, Hit@1: \`${summary.best.hit_at_1}\``,
    `- Docs/sec: \`${summary.best.docs_per_second}\`, index seconds: \`${summary.best.index_seconds}\`, semantic seconds: \`${summary.best.semantic_seconds}\``,
    `- Model MB: \`${summary.best.model_size_mb}\`, cache dir MB: \`${summary.best.cache_dir_size_mb}\`, persisted vector bytes/doc: \`${summary.best.vector_bytes_per_doc}\``,
    "",
    "## Top Rows",
    "",
    "| Rank | Case | Split | Score | Promotion | Quality | Speed | Memory | MRR@10 | Hit@10 | Docs/sec |",
    "| ---: | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...summary.top10.map(
      (row, index) =>
        `| ${index + 1} | \`${row.case_id}\` | ${row.query_split || "unknown"} | ${row.pipeline_score} | ${row.promotion_score} | ${row.quality_component} | ${row.speed_component} | ${row.memory_component} | ${row.mrr_at_10} | ${row.hit_at_10} | ${row.docs_per_second} |`,
    ),
    "",
  ].join("\n"),
);

console.log(`summary: ${path.join(summaryDir, "summary.md")}`);
emitMetric("pipeline_score", best.pipeline_score);
emitMetric("raw_pipeline_score", best.raw_pipeline_score);
emitMetric("quality_component", best.quality_component);
emitMetric("speed_component", best.speed_component);
emitMetric("memory_component", best.memory_component);
emitMetric("quality_gate_penalty", best.quality_gate_penalty);
emitMetric("promotion_score", best.promotion_score);
emitMetric("promotion_eligible", best.promotion_eligible ? 1 : 0);
emitMetric("experiments_observed", scoredRows.length);
emitMetric("decision_grade_rows", decisionGradeRows.length);
emitMetric("best_mrr_at_10", best.score?.mrr_at_10);
emitMetric("best_hit_at_10", best.score?.hit_at_10);
emitMetric("best_hit_at_1", best.score?.hit_at_1);
emitMetric("best_docs_per_second", best.docs_per_second);
emitMetric("best_index_seconds", best.index_seconds);
emitMetric("best_semantic_seconds", best.semantic_seconds);
emitMetric("best_cache_dir_size_mb", best.cache_dir_size_mb);
emitMetric("best_model_size_mb", best.model_size_mb);
emitMetric("best_vector_bytes_per_doc", persistedVectorBytesPerDoc(best));
emitMetric("best_prefilter_vector_bytes_per_doc", prefilterVectorBytesPerDoc(best));
emitMetric("quality_gate_pass", best.quality_gate_pass ? 1 : 0);
