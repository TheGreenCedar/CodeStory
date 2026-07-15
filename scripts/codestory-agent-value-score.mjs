#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_OUTPUT_DIR = path.join("target", "agent-value-score");

const PACKET_QUALITY_WEIGHTS = {
  expected_file_recall: 0.3,
  expected_claim_recall: 0.3,
  citation_coverage: 0.25,
  follow_up: 0.15,
};

const PACKET_QUALITY_DELTA_FIELDS = [
  "median_expected_file_recall",
  "median_expected_claim_recall",
  "median_citation_coverage",
  "median_packet_citation_recall",
  "median_follow_up_commands_count",
  "quality_pass_runs",
];

function parseArgs(argv) {
  const opts = {
    cwd: process.cwd(),
    promptSummaries: [],
    symbolSummary: null,
    packetSummary: null,
    abDoc: "autoresearch.md",
    includeAbDoc: true,
    explicitPromptSummaries: false,
    explicitSymbolSummary: false,
    explicitPacketSummary: false,
    outputDir: DEFAULT_OUTPUT_DIR,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--cwd") {
      opts.cwd = requireValue(argv, ++index, arg);
    } else if (arg === "--prompt-summary") {
      opts.promptSummaries.push(requireValue(argv, ++index, arg));
      opts.explicitPromptSummaries = true;
    } else if (arg === "--symbol-summary") {
      opts.symbolSummary = requireValue(argv, ++index, arg);
      opts.explicitSymbolSummary = true;
    } else if (arg === "--packet-summary") {
      opts.packetSummary = requireValue(argv, ++index, arg);
      opts.explicitPacketSummary = true;
    } else if (arg === "--ab-doc") {
      opts.abDoc = requireValue(argv, ++index, arg);
      opts.includeAbDoc = true;
    } else if (arg === "--no-ab-doc") {
      opts.abDoc = null;
      opts.includeAbDoc = false;
    } else if (arg === "--output-dir") {
      opts.outputDir = requireValue(argv, ++index, arg);
    } else if (arg === "--from-existing") {
      throw new Error("--from-existing is unsupported; provide fresh coherent mandatory-retrieval artifact paths");
    } else if (arg === "--allow-mixed-run-inputs") {
      throw new Error("--allow-mixed-run-inputs is unsupported; mandatory-retrieval scoring requires one coherent artifact run");
    } else if (arg === "--help" || arg === "-h") {
      opts.help = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }

  opts.cwd = path.resolve(opts.cwd);
  opts.outputDir = path.resolve(opts.cwd, opts.outputDir);
  opts.promptSummaries = opts.promptSummaries.map((entry) => resolveUnder(opts.cwd, entry));
  opts.symbolSummary = opts.symbolSummary ? resolveUnder(opts.cwd, opts.symbolSummary) : null;
  opts.packetSummary = opts.packetSummary ? resolveUnder(opts.cwd, opts.packetSummary) : null;
  opts.abDoc = opts.abDoc && opts.includeAbDoc ? resolveUnder(opts.cwd, opts.abDoc) : null;
  return opts;
}

function requireValue(argv, index, flag) {
  const value = argv[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function resolveUnder(cwd, entry) {
  return path.isAbsolute(entry) ? entry : path.resolve(cwd, entry);
}

function discoverInputs(opts) {
  const retrievalRoot = path.join(opts.cwd, "target", "retrieval-golden");
  const packetRoot = path.join(opts.cwd, "target", "agent-benchmark");
  const hasExplicitInput =
    opts.explicitPromptSummaries || opts.explicitSymbolSummary || opts.explicitPacketSummary;
  const sixLanePromptSummaries = [
    newestSummary(retrievalRoot, /rootandruntime.*six-lane-bundle/u),
    newestSummary(retrievalRoot, /prompt-guard.*six-lane-bundle/u),
  ].filter(Boolean);
  const defaultPromptSummaries = sixLanePromptSummaries;
  const sixLaneSymbol = newestSummary(retrievalRoot, /symbol-slice.*six-lane-bundle/u);
  return {
    promptSummaries:
      hasExplicitInput
        ? opts.promptSummaries
        : defaultPromptSummaries,
    symbolSummary: hasExplicitInput ? opts.symbolSummary : sixLaneSymbol,
    packetSummary:
      hasExplicitInput
        ? opts.packetSummary
        : newestNestedFile(
            packetRoot,
            /packet-runtime-local-real.*six-lane/u,
            "packet-runtime-summary.json",
          ),
    abDoc: opts.abDoc && opts.includeAbDoc && fs.existsSync(opts.abDoc) ? opts.abDoc : null,
  };
}

function newestSummary(root, dirPattern) {
  if (!fs.existsSync(root)) {
    return null;
  }
  const candidates = fs
    .readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && dirPattern.test(entry.name))
    .map((entry) => path.join(root, entry.name, "summary.json"))
    .filter((entryPath) => fs.existsSync(entryPath))
    .map((entryPath) => ({ path: entryPath, mtimeMs: fs.statSync(entryPath).mtimeMs }))
    .sort((left, right) => right.mtimeMs - left.mtimeMs || left.path.localeCompare(right.path));
  return candidates[0]?.path ?? null;
}

function newestNestedFile(root, dirPattern, fileName) {
  if (!fs.existsSync(root)) {
    return null;
  }
  const candidates = fs
    .readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && dirPattern.test(entry.name))
    .map((entry) => path.join(root, entry.name, fileName))
    .filter((entryPath) => fs.existsSync(entryPath))
    .map((entryPath) => ({ path: entryPath, mtimeMs: fs.statSync(entryPath).mtimeMs }))
    .sort((left, right) => right.mtimeMs - left.mtimeMs || left.path.localeCompare(right.path));
  return candidates[0]?.path ?? null;
}

function computeAgentValueScore(inputPaths, opts = {}) {
  const runCoherency = assertRunCoherency(inputPaths);
  const promptSummaryEntries = inputPaths.promptSummaries.map((entryPath) => ({
    path: entryPath,
    summary: readJson(entryPath),
  }));
  const promptSummaries = promptSummaryEntries.map((entry) => entry.summary);
  const symbolSummary = inputPaths.symbolSummary ? readJson(inputPaths.symbolSummary) : null;
  const packetSummary = inputPaths.packetSummary ? readJson(inputPaths.packetSummary) : null;
  const abRatios = inputPaths.abDoc ? parseAgentAbRatios(fs.readFileSync(inputPaths.abDoc, "utf8")) : [];
  const retrievalDiagnostics = aggregateRetrievalDiagnostics([
    ...promptSummaryEntries.map((entry) => ({ path: entry.path, summary: entry.summary })),
    ...(inputPaths.symbolSummary && symbolSummary
      ? [{ path: inputPaths.symbolSummary, summary: symbolSummary }]
      : []),
  ]);

  const promptRecalls = promptSummaries
    .map((summary) => finiteNumber(summary.summary?.prompt_file_recall))
    .filter(Number.isFinite);
  const promptScore = mean(promptRecalls);

  const symbolScore = finiteNumber(symbolSummary?.summary?.symbol_path_hit_at_k);
  const packetRows = Array.isArray(packetSummary?.summary) ? packetSummary.summary : [];
  const packetSufficiencyRate = packetRows.length
    ? packetRows.filter((row) => rowEffectiveSufficiency(row)).length / packetRows.length
    : NaN;
  const packetFileRecall = mean(
    packetRows.map((row) => finiteNumber(row.median_expected_file_recall)).filter(Number.isFinite),
  );
  const packetClaimRecall = mean(
    packetRows.map((row) => finiteNumber(row.median_expected_claim_recall)).filter(Number.isFinite),
  );
  const packetCitationCoverage = mean(
    packetRows.map((row) => finiteNumber(row.median_citation_coverage)).filter(Number.isFinite),
  );
  const packetFollowUpMean = mean(
    packetRows.map((row) => finiteNumber(row.median_follow_up_commands_count)).filter(Number.isFinite),
  );
  const packetFollowUpScore = followUpBurdenScore(packetFollowUpMean);
  const packetScore = aggregatePacketQualityScore(packetRows);
  const baselinePacketSummaryPath = inputPaths.packetSummary
    ? discoverPreviousPacketSummary(inputPaths.packetSummary, opts.cwd ?? process.cwd())
    : null;
  const baselinePacketSummary = baselinePacketSummaryPath ? readJson(baselinePacketSummaryPath) : null;
  const baselinePacketRows = Array.isArray(baselinePacketSummary?.summary)
    ? baselinePacketSummary.summary
    : [];
  const packetQualityDeltas = packetRows.length
    ? buildPacketQualityDeltas(packetRows, baselinePacketRows, {
        currentPath: inputPaths.packetSummary,
        baselinePath: baselinePacketSummaryPath,
      })
    : null;

  const packetWallValues = packetRows
    .map((row) => finiteNumber(row.median_wall_ms))
    .filter(Number.isFinite)
    .sort((left, right) => left - right);
  const packetP95WallMs = percentile(packetWallValues, 0.95);
  const latencyScore = Number.isFinite(packetP95WallMs)
    ? 1 / (1 + packetP95WallMs / 20_000)
    : NaN;

  const abOverheadRatioMean = mean(abRatios);
  const abSavingsScore = Number.isFinite(abOverheadRatioMean)
    ? 1 - clamp01(abOverheadRatioMean)
    : NaN;

  const agentValueScore = weightedMean([
    [promptScore, 0.3],
    [symbolScore, 0.15],
    [packetScore, 0.25],
    [latencyScore, 0.2],
    [abSavingsScore, 0.1],
  ]);
  const agentValueGap = 1 - agentValueScore;
  const triage = buildTriage([
    { component: "prompt_file_recall", score: promptScore, weight: 0.3 },
    { component: "exact_symbol_path_hit", score: symbolScore, weight: 0.15 },
    { component: "packet_quality", score: packetScore, weight: 0.25 },
    { component: "packet_latency", score: latencyScore, weight: 0.2 },
    { component: "recorded_live_ab_savings", score: abSavingsScore, weight: 0.1 },
  ]);
  const contributors = buildContributors({
    inputPaths,
    cwd: opts.cwd ?? process.cwd(),
    promptSummaryEntries,
    promptRecalls,
    symbolScore,
    packetRows,
    packetScore,
    latencyScore,
    abRatios,
    abSavingsScore,
  });

  return {
    generated_at: new Date().toISOString(),
    metric:
      "agent_value_gap = 1 - weighted agent_value_score; combines prompt recall, exact-symbol hit rate, packet claim/file/citation/follow-up rubric, packet latency, and recorded live A/B overhead savings",
    score_weights: {
      prompt_file_recall: 0.3,
      exact_symbol_path_hit: 0.15,
      packet_quality: 0.25,
      packet_latency: 0.2,
      recorded_live_ab_savings: 0.1,
      packet_quality_rubric: PACKET_QUALITY_WEIGHTS,
    },
    run_coherency: runCoherency,
    inputs: relativizeInputs(inputPaths, opts.cwd ?? process.cwd()),
    metrics: {
      agent_value_score: round(agentValueScore, 9),
      agent_value_gap: round(agentValueGap, 9),
      prompt_file_recall_mean: round(promptScore, 9),
      prompt_summary_count: promptRecalls.length,
      exact_symbol_path_hit_at_k: round(symbolScore, 9),
      packet_score: round(packetScore, 9),
      packet_sufficiency_rate: round(packetSufficiencyRate, 9),
      packet_expected_file_recall_mean: round(packetFileRecall, 9),
      packet_expected_claim_recall_mean: round(packetClaimRecall, 9),
      packet_citation_coverage_mean: round(packetCitationCoverage, 9),
      packet_follow_up_mean: round(packetFollowUpMean, 9),
      packet_follow_up_score: round(packetFollowUpScore, 9),
      packet_p95_wall_ms: round(packetP95WallMs, 3),
      packet_latency_score: round(latencyScore, 9),
      live_ab_overhead_ratio_mean: round(abOverheadRatioMean, 9),
      live_ab_savings_score: round(abSavingsScore, 9),
      live_ab_ratio_count: abRatios.length,
      retrieval_expected_files_present_missing_from_search_count:
        retrievalDiagnostics.expected_files_present_missing_from_search_count,
      retrieval_expected_symbols_present_missing_from_search_count:
        retrievalDiagnostics.expected_symbols_present_missing_from_search_count,
      retrieval_query_trace_with_candidates_count:
        retrievalDiagnostics.query_trace_with_candidates_count,
      retrieval_query_trace_no_candidates_count:
        retrievalDiagnostics.query_trace_no_candidates_count,
      retrieval_prepare_count: retrievalDiagnostics.retrieval_prepare_count,
      retrieval_prepare_failed_count: retrievalDiagnostics.retrieval_prepare_failed_count,
      retrieval_prepare_max_latency_ms: retrievalDiagnostics.retrieval_prepare_max_latency_ms,
      retrieval_candidate_count: retrievalDiagnostics.candidate_count,
      retrieval_resolved_hit_count: retrievalDiagnostics.resolved_hit_count,
      retrieval_unresolved_candidate_count:
        retrievalDiagnostics.unresolved_candidate_count,
      retrieval_candidate_resolution_rate:
        retrievalDiagnostics.candidate_count > 0
          ? round(retrievalDiagnostics.resolved_hit_count / retrievalDiagnostics.candidate_count, 9)
          : null,
    },
    packet_quality_deltas: packetQualityDeltas,
    triage,
    contributors: {
      ...contributors,
      retrieval_diagnostics: retrievalDiagnostics,
    },
  };
}

function aggregateRetrievalDiagnostics(summaryEntries) {
  const seen = new Set();
  const aggregate = {
    summary_count: 0,
    expected_files_present_missing_from_search_count: 0,
    expected_symbols_present_missing_from_search_count: 0,
    query_trace_with_candidates_count: 0,
    query_trace_no_candidates_count: 0,
    retrieval_prepare_count: 0,
    retrieval_prepare_failed_count: 0,
    retrieval_prepare_max_latency_ms: 0,
    candidate_count: 0,
    resolved_hit_count: 0,
    unresolved_candidate_count: 0,
    resolution_counts: [],
  };
  const resolutionCounts = new Map();
  for (const entry of summaryEntries) {
    if (!entry?.path || seen.has(entry.path)) {
      continue;
    }
    seen.add(entry.path);
    const payload = entry.summary;
    const summary = payload?.summary ?? {};
    aggregate.summary_count += 1;
    aggregate.expected_files_present_missing_from_search_count += numericCount(
      summary.retrieval_expected_files_present_missing_from_search_count,
    );
    aggregate.expected_symbols_present_missing_from_search_count += numericCount(
      summary.retrieval_expected_symbols_present_missing_from_search_count,
    );
    aggregate.query_trace_with_candidates_count += numericCount(
      summary.retrieval_query_trace_with_candidates_count,
    );
    aggregate.query_trace_no_candidates_count += numericCount(
      summary.retrieval_query_trace_no_candidates_count,
    );
    aggregate.retrieval_prepare_count += numericCount(summary.retrieval_prepare_count);
    aggregate.retrieval_prepare_failed_count += numericCount(summary.retrieval_prepare_failed_count);
    aggregate.retrieval_prepare_max_latency_ms = Math.max(
      aggregate.retrieval_prepare_max_latency_ms,
      finiteNumber(summary.retrieval_prepare_max_latency_ms) || 0,
    );

    const summaryHasShadowCounts =
      Number.isFinite(Number(summary.retrieval_shadow_candidate_count)) ||
      Number.isFinite(Number(summary.retrieval_shadow_resolved_hit_count)) ||
      Number.isFinite(Number(summary.retrieval_shadow_unresolved_candidate_count));
    if (summaryHasShadowCounts) {
      aggregate.candidate_count += numericCount(summary.retrieval_shadow_candidate_count);
      aggregate.resolved_hit_count += numericCount(summary.retrieval_shadow_resolved_hit_count);
      aggregate.unresolved_candidate_count += numericCount(
        summary.retrieval_shadow_unresolved_candidate_count,
      );
      mergeResolutionCounts(resolutionCounts, summary.retrieval_shadow_resolution_counts);
    } else {
      aggregateShadowRows(aggregate, resolutionCounts, payload?.rows ?? []);
    }
  }
  aggregate.resolution_counts = [...resolutionCounts.entries()]
    .map(([resolution, count]) => ({ resolution, count }))
    .sort((left, right) => left.resolution.localeCompare(right.resolution));
  return aggregate;
}

function aggregateShadowRows(aggregate, resolutionCounts, rows) {
  if (!Array.isArray(rows)) {
    return;
  }
  for (const row of rows) {
    const shadow = row?.retrieval_shadow;
    if (!shadow || typeof shadow !== "object") {
      continue;
    }
    aggregate.candidate_count += numericCount(shadow.candidate_count);
    aggregate.resolved_hit_count += numericCount(shadow.resolved_hit_count);
    aggregate.unresolved_candidate_count += numericCount(shadow.unresolved_candidate_count);
    if (Array.isArray(shadow.candidate_resolution_counts)) {
      mergeResolutionCounts(resolutionCounts, shadow.candidate_resolution_counts);
    } else {
      for (const candidate of shadow.candidates ?? []) {
        const resolution = String(candidate?.resolution ?? "").trim();
        if (resolution) {
          resolutionCounts.set(resolution, (resolutionCounts.get(resolution) ?? 0) + 1);
        }
      }
    }
  }
}

function mergeResolutionCounts(counts, entries) {
  if (!Array.isArray(entries)) {
    return;
  }
  for (const entry of entries) {
    const resolution = String(entry?.resolution ?? "").trim();
    const count = numericCount(entry?.count);
    if (resolution && count > 0) {
      counts.set(resolution, (counts.get(resolution) ?? 0) + count);
    }
  }
}

function numericCount(value) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : 0;
}

function buildTriage(components) {
  const componentGaps = componentGapRows(components);
  const largest = componentGaps[0] ?? null;
  return {
    largest_gap_component: largest?.component ?? null,
    recommended_next_lane: largest ? laneForComponent(largest.component) : null,
    component_gaps: componentGaps,
  };
}

function componentGapRows(components) {
  const finite = components.filter((entry) => Number.isFinite(entry.score) && entry.weight > 0);
  const weightTotal = finite.reduce((sum, entry) => sum + entry.weight, 0);
  return finite
    .map((entry) => {
      const gap = 1 - clamp01(entry.score);
      const effectiveWeight = entry.weight / weightTotal;
      return {
        component: entry.component,
        score: round(entry.score, 9),
        gap: round(gap, 9),
        effective_weight: round(effectiveWeight, 9),
        weighted_gap: round(gap * effectiveWeight, 9),
        recommended_next_lane: laneForComponent(entry.component),
      };
    })
    .sort((left, right) => {
      const weightedGapDelta = Number(right.weighted_gap) - Number(left.weighted_gap);
      return weightedGapDelta || left.component.localeCompare(right.component);
    });
}

function laneForComponent(component) {
  return (
    {
      prompt_file_recall: "prompt-recall",
      exact_symbol_path_hit: "symbol-search",
      packet_quality: "packet-quality",
      packet_latency: "packet-latency",
      recorded_live_ab_savings: "live-ab-overhead",
    }[component] ?? null
  );
}

function buildContributors({
  inputPaths,
  cwd,
  promptSummaryEntries,
  promptRecalls,
  symbolScore,
  packetRows,
  packetScore,
  latencyScore,
  abRatios,
  abSavingsScore,
}) {
  const relative = (entry) => (entry ? path.relative(cwd, entry).replaceAll(path.sep, "/") : null);
  const inputs = [
    ...promptSummaryEntries.map((entry) => {
      const score = finiteNumber(entry.summary?.summary?.prompt_file_recall);
      return contributorInputRow({
        kind: "prompt_summary",
        path: relative(entry.path),
        component: "prompt_file_recall",
        score,
      });
    }),
    contributorInputRow({
      kind: "symbol_summary",
      path: relative(inputPaths.symbolSummary),
      component: "exact_symbol_path_hit",
      score: symbolScore,
    }),
    contributorInputRow({
      kind: "packet_summary",
      path: relative(inputPaths.packetSummary),
      component: "packet_quality",
      score: packetScore,
    }),
    contributorInputRow({
      kind: "packet_summary",
      path: relative(inputPaths.packetSummary),
      component: "packet_latency",
      score: latencyScore,
    }),
    contributorInputRow({
      kind: "ab_doc",
      path: relative(inputPaths.abDoc),
      component: "recorded_live_ab_savings",
      score: abSavingsScore,
      count: abRatios.length,
    }),
  ].filter((entry) => entry.path && entry.score !== null);

  return {
    inputs,
    packet_rows: packetRows.map(packetContributorRow).filter(Boolean),
    counts: {
      prompt_summary_rows: promptRecalls.length,
      packet_rows: packetRows.length,
      live_ab_ratios: abRatios.length,
    },
  };
}

function contributorInputRow({ kind, path: inputPath, component, score, count = null }) {
  return {
    kind,
    path: inputPath,
    component,
    score: round(score, 9),
    gap: round(1 - clamp01(score), 9),
    recommended_next_lane: laneForComponent(component),
    count,
  };
}

function packetContributorRow(row) {
  const packetQualityScore = packetRowQualityScore(row);
  const packetLatencyScore = packetRowLatencyScore(row);
  const triage = buildTriage([
    { component: "packet_quality", score: packetQualityScore, weight: 0.25 },
    { component: "packet_latency", score: packetLatencyScore, weight: 0.2 },
  ]);
  if (!triage.component_gaps.length) {
    return null;
  }
  return {
    repo: row.repo ?? null,
    task_id: row.task_id ?? null,
    mode: row.mode ?? null,
    quality_pass_runs: Number.isFinite(Number(row.quality_pass_runs)) ? Number(row.quality_pass_runs) : null,
    sufficiency_rate: round(rowSufficiencyRate(row), 9),
    median_expected_file_recall: round(finiteNumber(row.median_expected_file_recall), 9),
    median_expected_claim_recall: round(finiteNumber(row.median_expected_claim_recall), 9),
    median_citation_coverage: round(finiteNumber(row.median_citation_coverage), 9),
    packet_quality_score: round(packetQualityScore, 9),
    packet_latency_score: round(packetLatencyScore, 9),
    median_wall_ms: round(finiteNumber(row.median_wall_ms), 3),
    median_retrieval_total_ms: round(finiteNumber(row.median_packet_retrieval_total_ms), 3),
    median_freshness_ms: round(finiteNumber(row.median_packet_freshness_ms), 3),
    median_unaccounted_ms: round(finiteNumber(row.median_packet_unaccounted_ms), 3),
    top_latency_step: row.packet_top_latency_step_kind ?? null,
    median_top_step_ms: round(finiteNumber(row.median_packet_top_step_ms), 3),
    sla_missed_runs: Number.isFinite(Number(row.packet_sla_missed_runs)) ? Number(row.packet_sla_missed_runs) : null,
    largest_gap_component: triage.largest_gap_component,
    recommended_next_lane: triage.recommended_next_lane,
  };
}

function packetRowQualityScore(row) {
  const base = weightedMean([
    [finiteNumber(row.median_expected_file_recall), PACKET_QUALITY_WEIGHTS.expected_file_recall],
    [finiteNumber(row.median_expected_claim_recall), PACKET_QUALITY_WEIGHTS.expected_claim_recall],
    [finiteNumber(row.median_citation_coverage), PACKET_QUALITY_WEIGHTS.citation_coverage],
    [followUpBurdenScore(row.median_follow_up_commands_count), PACKET_QUALITY_WEIGHTS.follow_up],
  ]);
  if (!Number.isFinite(base)) {
    return NaN;
  }
  const mismatchRuns = Number(row.sufficient_quality_mismatch_runs ?? 0);
  if (mismatchRuns > 0) {
    return base * 0.5;
  }
  return base;
}

function aggregatePacketQualityScore(packetRows) {
  return mean(packetRows.map((row) => packetRowQualityScore(row)).filter(Number.isFinite));
}

function rowSufficiencyRate(row) {
  const counts = row.sufficiency_status_counts ?? {};
  const sufficient = Number(counts.sufficient ?? 0);
  const total = Object.values(counts).reduce((sum, value) => sum + Number(value), 0);
  return total > 0 ? sufficient / total : 0;
}

function rowEffectiveSufficiency(row) {
  if (rowSufficiencyRate(row) <= 0) {
    return false;
  }
  return Number(row.sufficient_quality_mismatch_runs ?? 0) <= 0;
}

function followUpBurdenScore(followUpMean) {
  const value = finiteNumber(followUpMean);
  return Number.isFinite(value) ? 1 - clamp01(value / 4) : NaN;
}

function extractRunStamp(filePath) {
  if (!filePath) {
    return null;
  }
  const normalized = String(filePath).replace(/\\/g, "/");
  const patterns = [
    /packet-runtime-local-real-six-lane-([^/]+)/u,
    /six-lane(?:-bundle)?-([^/]+)/u,
    /packet-runtime-post-([0-9a-f]{7,40})/iu,
    /(?:^|\/)packet-runtime-([^/]+)/u,
  ];
  for (const pattern of patterns) {
    const match = normalized.match(pattern);
    if (match?.[1]) {
      return match[1].toLowerCase();
    }
  }
  return null;
}

function metadataRunStamp(filePath) {
  if (!filePath || !fs.existsSync(filePath)) {
    return null;
  }
  let payload;
  try {
    payload = readJson(filePath);
  } catch {
    return null;
  }
  const stamp =
    payload?.benchmark_run_id ??
    payload?.run_id ??
    payload?.metadata?.benchmark_run_id ??
    payload?.metadata?.run_id ??
    null;
  return normalizeRunStamp(stamp);
}

function artifactRunStamp(filePath) {
  return metadataRunStamp(filePath) ?? extractRunStamp(filePath);
}

function normalizeRunStamp(value) {
  const stamp = String(value ?? "").trim();
  return stamp ? stamp.toLowerCase() : null;
}

function extractArtifactLabel(filePath) {
  if (!filePath) {
    return null;
  }
  const stamp = extractRunStamp(filePath);
  if (stamp) {
    return stamp;
  }
  return path.basename(path.dirname(filePath));
}

function assertRunCoherency(inputPaths) {
  const entries = [
    ...inputPaths.promptSummaries.map((entryPath) => ({ kind: "prompt_summary", path: entryPath })),
    ...(inputPaths.symbolSummary
      ? [{ kind: "symbol_summary", path: inputPaths.symbolSummary }]
      : []),
    ...(inputPaths.packetSummary ? [{ kind: "packet_summary", path: inputPaths.packetSummary }] : []),
  ];
  const stamped = entries.map((entry) => ({
    ...entry,
    stamp: artifactRunStamp(entry.path),
  }));
  const presentStamps = stamped.map((entry) => entry.stamp).filter(Boolean);
  const uniqueStamps = [...new Set(presentStamps)];
  const unstamped = stamped.filter((entry) => !entry.stamp);
  if (uniqueStamps.length > 1) {
    const detail = stamped
      .filter((entry) => entry.stamp)
      .map((entry) => `${entry.kind}=${entry.stamp}`)
      .join(", ");
    throw new Error(
      `mixed run stamps in scoring inputs (${detail}); provide one coherent fresh artifact run`,
    );
  }
  if (presentStamps.length > 0 && unstamped.length > 0) {
    const detail = unstamped.map((entry) => entry.kind).join(", ");
    throw new Error(
      `missing run stamp on ${detail}; provide one coherent fresh artifact run`,
    );
  }
  return {
    enforced: presentStamps.length > 0,
    run_stamp: uniqueStamps[0] ?? null,
    input_stamps: Object.fromEntries(
      stamped.filter((entry) => entry.stamp).map((entry) => [entry.kind, entry.stamp]),
    ),
  };
}

function packetTaskKey(row) {
  return `${row.repo}\t${row.task_id}\t${row.mode}`;
}

function pickPacketQualityMetrics(row) {
  return Object.fromEntries(
    PACKET_QUALITY_DELTA_FIELDS.map((field) => [field, round(finiteNumber(row[field]), 9)]).filter(
      ([, value]) => value != null,
    ),
  );
}

function buildPacketQualityDeltas(currentRows, baselineRows, opts = {}) {
  const baselineByKey = new Map(baselineRows.map((row) => [packetTaskKey(row), row]));
  const tasks = currentRows.map((row) => {
    const baseline = baselineByKey.get(packetTaskKey(row));
    const currentMetrics = pickPacketQualityMetrics(row);
    currentMetrics.sufficiency_rate = round(rowSufficiencyRate(row), 9);
    if (!baseline) {
      return {
        repo: row.repo ?? null,
        task_id: row.task_id ?? null,
        mode: row.mode ?? null,
        baseline: null,
        current: currentMetrics,
        deltas: null,
      };
    }
    const baselineMetrics = pickPacketQualityMetrics(baseline);
    baselineMetrics.sufficiency_rate = round(rowSufficiencyRate(baseline), 9);
    const deltas = {};
    for (const field of [...PACKET_QUALITY_DELTA_FIELDS, "sufficiency_rate"]) {
      const currentValue = finiteNumber(
        field === "sufficiency_rate" ? rowSufficiencyRate(row) : row[field],
      );
      const baselineValue = finiteNumber(
        field === "sufficiency_rate" ? rowSufficiencyRate(baseline) : baseline[field],
      );
      if (Number.isFinite(currentValue) && Number.isFinite(baselineValue)) {
        deltas[field] = {
          baseline: round(baselineValue, 9),
          current: round(currentValue, 9),
          delta: round(currentValue - baselineValue, 9),
        };
      }
    }
    return {
      repo: row.repo ?? null,
      task_id: row.task_id ?? null,
      mode: row.mode ?? null,
      baseline: baselineMetrics,
      current: currentMetrics,
      deltas,
    };
  });
  return {
    baseline_summary: opts.baselinePath ?? null,
    baseline_label: extractArtifactLabel(opts.baselinePath),
    current_summary: opts.currentPath ?? null,
    current_label: extractArtifactLabel(opts.currentPath),
    tasks,
  };
}

function discoverPreviousPacketSummary(packetSummaryPath, cwd) {
  const packetRoot = path.join(cwd, "target", "agent-benchmark");
  const resolvedCurrent = path.resolve(packetSummaryPath);
  if (!fs.existsSync(packetRoot)) {
    return null;
  }
  const candidates = fs
    .readdirSync(packetRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /packet-runtime/u.test(entry.name))
    .map((entry) => path.join(packetRoot, entry.name, "packet-runtime-summary.json"))
    .filter(
      (entryPath) => fs.existsSync(entryPath) && path.resolve(entryPath) !== resolvedCurrent,
    )
    .map((entryPath) => ({ path: entryPath, mtimeMs: fs.statSync(entryPath).mtimeMs }))
    .sort((left, right) => right.mtimeMs - left.mtimeMs || left.path.localeCompare(right.path));
  return candidates[0]?.path ?? null;
}

function packetRowLatencyScore(row) {
  const wallMs = finiteNumber(row.median_wall_ms);
  return Number.isFinite(wallMs) ? 1 / (1 + wallMs / 20_000) : NaN;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function parseAgentAbRatios(markdown) {
  return [...markdown.matchAll(/agent_ab_overhead_ratio=([0-9]+(?:\.[0-9]+)?)/gu)]
    .map((match) => Number(match[1]))
    .filter(Number.isFinite);
}

function finiteNumber(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : NaN;
}

function mean(values) {
  const finite = values.filter(Number.isFinite);
  if (!finite.length) {
    return NaN;
  }
  return finite.reduce((sum, value) => sum + value, 0) / finite.length;
}

function weightedMean(entries) {
  const finite = entries.filter(([value, weight]) => Number.isFinite(value) && weight > 0);
  if (!finite.length) {
    return NaN;
  }
  const weightTotal = finite.reduce((sum, [, weight]) => sum + weight, 0);
  return finite.reduce((sum, [value, weight]) => sum + value * weight, 0) / weightTotal;
}

function percentile(sortedValues, percentileValue) {
  if (!sortedValues.length) {
    return NaN;
  }
  const index = Math.ceil(sortedValues.length * percentileValue) - 1;
  return sortedValues[Math.min(sortedValues.length - 1, Math.max(0, index))];
}

function clamp01(value) {
  return Math.min(1, Math.max(0, value));
}

function round(value, digits = 6) {
  if (!Number.isFinite(Number(value))) {
    return null;
  }
  const scale = 10 ** digits;
  return Math.round(Number(value) * scale) / scale;
}

function relativizeInputs(inputPaths, cwd) {
  const relative = (entry) => (entry ? path.relative(cwd, entry).replaceAll(path.sep, "/") : null);
  return {
    prompt_summaries: inputPaths.promptSummaries.map(relative),
    symbol_summary: relative(inputPaths.symbolSummary),
    packet_summary: relative(inputPaths.packetSummary),
    ab_doc: relative(inputPaths.abDoc),
  };
}

function writeSummary(summary, outputDir) {
  fs.mkdirSync(outputDir, { recursive: true });
  const jsonPath = path.join(outputDir, "summary.json");
  const mdPath = path.join(outputDir, "summary.md");
  fs.writeFileSync(jsonPath, `${JSON.stringify(summary, null, 2)}\n`);
  fs.writeFileSync(mdPath, renderMarkdown(summary));
  return { jsonPath, mdPath };
}

function renderMarkdown(summary) {
  const metricLines = Object.entries(summary.metrics).map(
    ([name, value]) => `- ${name}: \`${value ?? "n/a"}\``,
  );
  const componentGapLines = (summary.triage?.component_gaps ?? []).map(
    (row) =>
      `- ${row.component}: gap \`${row.gap ?? "n/a"}\`, weighted \`${row.weighted_gap ?? "n/a"}\`, lane \`${row.recommended_next_lane ?? "n/a"}\``,
  );
  const packetContributorLines = (summary.contributors?.packet_rows ?? []).map(
    (row) =>
      `- ${row.repo ?? "unknown"} / ${row.task_id ?? "unknown"} / ${row.mode ?? "unknown"}: lane \`${row.recommended_next_lane ?? "n/a"}\`, quality \`${row.packet_quality_score ?? "n/a"}\`, latency \`${row.packet_latency_score ?? "n/a"}\`, top step \`${row.top_latency_step ?? "n/a"}\` \`${row.median_top_step_ms ?? "n/a"}ms\``,
  );
  const retrievalDiagnostics = summary.contributors?.retrieval_diagnostics ?? {};
  const retrievalResolutionLines = (retrievalDiagnostics.resolution_counts ?? []).map(
    (entry) => `- ${entry.resolution}: \`${entry.count}\``,
  );
  const deltaLines = (summary.packet_quality_deltas?.tasks ?? [])
    .filter((row) => row.deltas)
    .map((row) => {
      const fileDelta = row.deltas.median_expected_file_recall?.delta ?? "n/a";
      const claimDelta = row.deltas.median_expected_claim_recall?.delta ?? "n/a";
      const citationDelta = row.deltas.median_citation_coverage?.delta ?? "n/a";
      return `- ${row.repo ?? "unknown"} / ${row.task_id ?? "unknown"} / ${row.mode ?? "unknown"}: file \`${fileDelta}\`, claim \`${claimDelta}\`, citation \`${citationDelta}\``;
    });
  return [
    "# CodeStory Agent Value Score",
    "",
    summary.metric,
    "",
    "This is a cheap steering metric from existing local artifacts. It is not promotion evidence by itself.",
    "",
    "## Inputs",
    "",
    ...summary.inputs.prompt_summaries.map((entry) => `- prompt: \`${entry}\``),
    `- symbols: \`${summary.inputs.symbol_summary ?? "missing"}\``,
    `- packet: \`${summary.inputs.packet_summary ?? "missing"}\``,
    `- A/B notes: \`${summary.inputs.ab_doc ?? "missing"}\``,
    "",
    "## Metrics",
    "",
    ...metricLines,
    "",
    "## Triage",
    "",
    `- largest gap component: \`${summary.triage?.largest_gap_component ?? "n/a"}\``,
    `- recommended next lane: \`${summary.triage?.recommended_next_lane ?? "n/a"}\``,
    ...componentGapLines,
    "",
    "## Packet Contributors",
    "",
    ...(packetContributorLines.length ? packetContributorLines : ["- n/a"]),
    "",
    "## Retrieval Diagnostics",
    "",
    `- summaries: \`${retrievalDiagnostics.summary_count ?? 0}\``,
    `- expected files present in retrievals but missing from search: \`${retrievalDiagnostics.expected_files_present_missing_from_search_count ?? 0}\``,
    `- expected symbols present in retrievals but missing from search: \`${retrievalDiagnostics.expected_symbols_present_missing_from_search_count ?? 0}\``,
    `- retrieval prepare rows: \`${retrievalDiagnostics.retrieval_prepare_count ?? 0}\``,
    `- retrieval prepare failures: \`${retrievalDiagnostics.retrieval_prepare_failed_count ?? 0}\``,
    `- retrieval prepare max latency ms: \`${retrievalDiagnostics.retrieval_prepare_max_latency_ms ?? 0}\``,
    `- retrieval candidates: \`${retrievalDiagnostics.candidate_count ?? 0}\``,
    `- resolved hits: \`${retrievalDiagnostics.resolved_hit_count ?? 0}\``,
    `- unresolved candidates: \`${retrievalDiagnostics.unresolved_candidate_count ?? 0}\``,
    ...(retrievalResolutionLines.length ? retrievalResolutionLines : ["- resolution counts: `n/a`"]),
    "",
    "## Packet Quality Deltas",
    "",
    `- baseline: \`${summary.packet_quality_deltas?.baseline_label ?? "n/a"}\``,
    `- current: \`${summary.packet_quality_deltas?.current_label ?? "n/a"}\``,
    ...(deltaLines.length ? deltaLines : ["- n/a"]),
    "",
  ].join("\n");
}

function emitMetrics(summary, artifactPaths) {
  console.log(`summary: ${artifactPaths.mdPath}`);
  console.log(`ARTIFACT agent_value_summary=${artifactPaths.jsonPath}`);
  for (const [name, value] of metricEntries(summary)) {
    console.log(`METRIC ${name}=${value}`);
  }
}

function metricEntries(summary) {
  return Object.entries(summary.metrics).filter(
    ([, value]) => typeof value === "number" && Number.isFinite(value),
  );
}

function printHelp() {
  console.log(`Usage:
  node scripts/codestory-agent-value-score.mjs --prompt-summary <path> --symbol-summary <path> --packet-summary <path> [--ab-doc <path>|--no-ab-doc]

Reads one coherent local benchmark artifact set and emits METRIC agent_value_gap=<n> plus supporting metrics.`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) {
    printHelp();
    return;
  }
  const inputs = discoverInputs(opts);
  if (!inputs.promptSummaries.length && !inputs.symbolSummary && !inputs.packetSummary) {
    throw new Error("no existing agent-value inputs found");
  }
  const summary = computeAgentValueScore(inputs, {
    cwd: opts.cwd,
  });
  if (!Number.isFinite(Number(summary.metrics.agent_value_gap))) {
    throw new Error("agent_value_gap could not be computed from available inputs");
  }
  const artifacts = writeSummary(summary, opts.outputDir);
  emitMetrics(summary, artifacts);
}

export {
  assertRunCoherency,
  artifactRunStamp,
  buildPacketQualityDeltas,
  computeAgentValueScore,
  discoverInputs,
  discoverPreviousPacketSummary,
  extractRunStamp,
  metricEntries,
  parseAgentAbRatios,
  parseArgs,
  packetRowQualityScore,
  percentile,
  rowEffectiveSufficiency,
  rowSufficiencyRate,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    console.error(error?.stack ?? String(error));
    process.exit(1);
  });
}
