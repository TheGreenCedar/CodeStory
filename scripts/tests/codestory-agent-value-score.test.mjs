import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  artifactRunStamp,
  buildPacketQualityDeltas,
  computeAgentValueScore,
  discoverInputs,
  extractRunStamp,
  metricEntries,
  parseAgentAbRatios,
  parseArgs,
  packetRowQualityScore,
  percentile,
  rowEffectiveSufficiency,
  rowSufficiencyRate,
} from "../codestory-agent-value-score.mjs";

test("parses explicit inputs and rejects from-existing scoring", () => {
  const opts = parseArgs([
    "--cwd",
    "C:/repo",
    "--prompt-summary",
    "target/retrieval-golden/prompt/summary.json",
    "--symbol-summary",
    "target/retrieval-golden/symbol/summary.json",
    "--packet-summary",
    "target/agent-benchmark/packet/packet-runtime-summary.json",
    "--ab-doc",
    "autoresearch.md",
  ]);

  assert.equal(opts.fromExisting, undefined);
  assert.equal(opts.includeAbDoc, true);
  assert.equal(opts.explicitPromptSummaries, true);
  assert.equal(opts.explicitSymbolSummary, true);
  assert.equal(opts.explicitPacketSummary, true);
  assert.equal(opts.promptSummaries.length, 1);
  assert.match(opts.promptSummaries[0], /target[\\/]retrieval-golden[\\/]prompt[\\/]summary\.json$/u);
  assert.match(opts.symbolSummary, /target[\\/]retrieval-golden[\\/]symbol[\\/]summary\.json$/u);
  assert.match(opts.packetSummary, /target[\\/]agent-benchmark[\\/]packet[\\/]packet-runtime-summary\.json$/u);
  assert.match(opts.abDoc, /autoresearch\.md$/u);
  assert.equal(parseArgs(["--no-ab-doc"]).abDoc, null);
  assert.equal(parseArgs(["--no-ab-doc"]).includeAbDoc, false);
  assert.throws(() => parseArgs(["--from-existing"]), /from-existing is unsupported/);
  assert.throws(() => parseArgs(["--allow-mixed-run-inputs"]), /allow-mixed-run-inputs is unsupported/);
});

test("parses recorded live A/B ratios from markdown", () => {
  assert.deepEqual(
    parseAgentAbRatios([
      "Codex `agent_ab_overhead_ratio=0.183466`.",
      "Sourcetrail `agent_ab_overhead_ratio=0.10904`.",
      "No metric here.",
    ].join("\n")),
    [0.183466, 0.10904],
  );
});

test("percentile uses nearest-rank behavior for packet p95", () => {
  assert.equal(percentile([10, 20, 30, 40], 0.95), 40);
  assert.equal(percentile([10], 0.95), 10);
});

test("computes stable agent value gap from local artifact summaries", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-"));
  const promptA = writeJson(root, "prompt-a.json", {
    summary: { prompt_file_recall: 0.5 },
  });
  const promptB = writeJson(root, "prompt-b.json", {
    summary: { prompt_file_recall: 1 },
  });
  const symbol = writeJson(root, "symbol.json", {
    summary: { symbol_path_hit_at_k: 1 },
  });
  const packet = writeJson(root, "packet.json", {
    summary: [
      {
        repo: "codex",
        task_id: "event-output",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 1,
        median_expected_claim_recall: 1,
        median_citation_coverage: 0.8,
        median_follow_up_commands_count: 0,
        median_wall_ms: 10_000,
      },
      {
        repo: "vscode",
        task_id: "extension-flow",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 1,
        median_expected_claim_recall: 0.8,
        median_citation_coverage: 1,
        median_follow_up_commands_count: 2,
        median_wall_ms: 30_000,
      },
    ],
  });
  const abDoc = path.join(root, "autoresearch.md");
  fs.writeFileSync(
    abDoc,
    "agent_ab_overhead_ratio=0.2\nagent_ab_overhead_ratio=0.4\n",
  );

  const summary = computeAgentValueScore(
    {
      promptSummaries: [promptA, promptB],
      symbolSummary: symbol,
      packetSummary: packet,
      abDoc,
    },
    { cwd: root },
  );

  assert.equal(summary.metrics.prompt_file_recall_mean, 0.75);
  assert.equal(summary.metrics.exact_symbol_path_hit_at_k, 1);
  assert.equal(summary.metrics.packet_p95_wall_ms, 30000);
  assert.equal(summary.metrics.live_ab_overhead_ratio_mean, 0.3);
  assert.equal(summary.metrics.live_ab_ratio_count, 2);
  assert.equal(summary.metrics.packet_sufficiency_rate, 1);
  assert.equal(summary.metrics.packet_expected_file_recall_mean, 1);
  assert.equal(summary.metrics.packet_expected_claim_recall_mean, 0.9);
  assert.equal(summary.metrics.packet_citation_coverage_mean, 0.9);
  assert.equal(summary.metrics.agent_value_score, 0.751875);
  assert.equal(summary.metrics.agent_value_gap, 0.248125);
});

test("counts packet sufficiency without quality_pass_runs", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-sufficiency-"));
  const packet = writeJson(root, "packet.json", {
    summary: [
      {
        repo: "vscode",
        task_id: "extension-flow",
        mode: "cold-cli",
        quality_pass_runs: 0,
        sufficiency_status_counts: { sufficient: 1, partial: 1 },
        median_expected_file_recall: 0.6,
        median_expected_claim_recall: 0.5,
        median_citation_coverage: 0.7,
        median_follow_up_commands_count: 0,
      },
    ],
  });

  const summary = computeAgentValueScore(
    {
      promptSummaries: [],
      symbolSummary: null,
      packetSummary: packet,
      abDoc: null,
    },
    { cwd: root },
  );

  const row = {
    quality_pass_runs: 0,
    sufficiency_status_counts: { sufficient: 1, partial: 1 },
    median_expected_file_recall: 0.6,
    median_expected_claim_recall: 0.5,
    median_citation_coverage: 0.7,
    median_follow_up_commands_count: 0,
  };

  assert.equal(summary.metrics.packet_sufficiency_rate, 1);
  assert.equal(rowSufficiencyRate(row), 0.5);
  assert.ok(Math.abs(packetRowQualityScore(row) - 0.655) < 1e-9);
});

test("penalizes sufficient_quality_mismatch in sufficiency rate and packet score", () => {
  const row = {
    sufficiency_status_counts: { sufficient: 1 },
    sufficient_quality_mismatch_runs: 1,
    median_expected_file_recall: 0.8,
    median_expected_claim_recall: 0.8,
    median_citation_coverage: 0.8,
    median_follow_up_commands_count: 0,
  };
  assert.equal(rowEffectiveSufficiency(row), false);
  assert.ok(packetRowQualityScore(row) < packetRowQualityScore({ ...row, sufficient_quality_mismatch_runs: 0 }));
});

test("rejects mixed run stamps", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-coherency-"));
  const prompt = writeJson(
    root,
    "target/retrieval-golden/rootandruntime-prompt-six-lane-bundle-2026-05-25/summary.json",
    { summary: { prompt_file_recall: 1 } },
  );
  const symbol = writeJson(
    root,
    "target/retrieval-golden/symbol-slice-six-lane-bundle-2026-05-26/summary.json",
    { summary: { symbol_path_hit_at_k: 1 } },
  );
  const packet = writeJson(
    root,
    "target/agent-benchmark/packet-runtime-local-real-six-lane-2026-05-26/packet-runtime-summary.json",
    { summary: [] },
  );

  assert.throws(
    () =>
      computeAgentValueScore(
        {
          promptSummaries: [prompt],
          symbolSummary: symbol,
          packetSummary: packet,
          abDoc: null,
        },
        { cwd: root },
      ),
    /mixed run stamps/i,
  );
  assert.equal(extractRunStamp(prompt), "2026-05-25");
  assert.equal(extractRunStamp(symbol), "2026-05-26");
  assert.equal(extractRunStamp(packet), "2026-05-26");
});

test("uses benchmark_run_id metadata for coherent mandatory-retrieval artifact scoring", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-retrieval-coherency-"));
  const prompt = writeJson(root, "prompt/summary.json", {
    benchmark_run_id: "segment43-retrieval",
    summary: { prompt_file_recall: 1 },
  });
  const symbol = writeJson(root, "symbol/summary.json", {
    benchmark_run_id: "segment43-retrieval",
    summary: { symbol_path_hit_at_k: 1 },
  });
  const packet = writeJson(root, "packet/packet-runtime-summary.json", {
    benchmark_run_id: "segment43-retrieval",
    summary: [],
  });

  const summary = computeAgentValueScore(
    {
      promptSummaries: [prompt],
      symbolSummary: symbol,
      packetSummary: packet,
      abDoc: null,
    },
    { cwd: root },
  );

  assert.equal(artifactRunStamp(prompt), "segment43-retrieval");
  assert.equal(summary.run_coherency.enforced, true);
  assert.equal(summary.run_coherency.run_stamp, "segment43-retrieval");

  const mismatchedPacket = writeJson(root, "packet-mismatch/packet-runtime-summary.json", {
    benchmark_run_id: "segment43-other",
    summary: [],
  });
  assert.throws(
    () =>
      computeAgentValueScore(
        {
          promptSummaries: [prompt],
          symbolSummary: symbol,
          packetSummary: mismatchedPacket,
          abDoc: null,
        },
        { cwd: root },
      ),
    /mixed run stamps/i,
  );
});

test("builds per-task commit-to-commit packet quality deltas", () => {
  const baselineRows = [
    {
      repo: "codex",
      task_id: "event-output",
      mode: "cold-cli",
      quality_pass_runs: 0,
      sufficiency_status_counts: { partial: 1 },
      median_expected_file_recall: 0.625,
      median_expected_claim_recall: 0.5,
      median_citation_coverage: 0.625,
      median_follow_up_commands_count: 2,
    },
  ];
  const currentRows = [
    {
      repo: "codex",
      task_id: "event-output",
      mode: "cold-cli",
      quality_pass_runs: 1,
      sufficiency_status_counts: { sufficient: 1 },
      median_expected_file_recall: 0.75,
      median_expected_claim_recall: 0.75,
      median_citation_coverage: 0.75,
      median_follow_up_commands_count: 0,
    },
  ];

  const deltas = buildPacketQualityDeltas(currentRows, baselineRows, {
    baselinePath: "target/agent-benchmark/packet-runtime-post-aaaaaaa/packet-runtime-summary.json",
    currentPath: "target/agent-benchmark/packet-runtime-post-bbbbbbb/packet-runtime-summary.json",
  });

  assert.equal(deltas.baseline_label, "aaaaaaa");
  assert.equal(deltas.current_label, "bbbbbbb");
  assert.equal(deltas.tasks.length, 1);
  assert.equal(deltas.tasks[0].deltas.median_expected_file_recall.delta, 0.125);
  assert.equal(deltas.tasks[0].deltas.quality_pass_runs.delta, 1);
  assert.equal(deltas.tasks[0].deltas.sufficiency_rate.delta, 1);
});

test("summarizes largest gap component and recommended next lane", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-lane-"));
  const prompt = writeJson(root, "prompt.json", {
    summary: { prompt_file_recall: 0.95 },
  });
  const symbol = writeJson(root, "symbol.json", {
    summary: { symbol_path_hit_at_k: 0.9 },
  });
  const packet = writeJson(root, "packet.json", {
    summary: [
      {
        repo: "codex",
        task_id: "event-output",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 0.8,
        median_expected_claim_recall: 0.7,
        median_citation_coverage: 0.7,
        median_follow_up_commands_count: 1,
        median_wall_ms: 90_000,
        median_packet_retrieval_total_ms: 70_000,
        median_packet_freshness_ms: 10_000,
        median_packet_unaccounted_ms: 10_000,
        packet_top_latency_step_kind: "search",
        median_packet_top_step_ms: 55_000,
        packet_sla_missed_runs: 1,
      },
      {
        repo: "vscode",
        task_id: "extension-flow",
        mode: "warm-stdio",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 1,
        median_expected_claim_recall: 1,
        median_citation_coverage: 1,
        median_follow_up_commands_count: 0,
        median_wall_ms: 15_000,
      },
    ],
  });

  const summary = computeAgentValueScore(
    {
      promptSummaries: [prompt],
      symbolSummary: symbol,
      packetSummary: packet,
      abDoc: null,
    },
    { cwd: root },
  );

  assert.equal(summary.triage.largest_gap_component, "packet_latency");
  assert.equal(summary.triage.recommended_next_lane, "packet-latency");
  assert.ok(summary.triage.component_gaps.some((row) => row.component === "packet_latency"));
  assert.equal(summary.contributors.inputs[0].kind, "prompt_summary");
  assert.equal(summary.contributors.packet_rows.length, 2);
  assert.equal(summary.contributors.packet_rows[0].top_latency_step, "search");
  assert.equal(summary.contributors.packet_rows[0].median_top_step_ms, 55_000);
  assert.equal(summary.contributors.packet_rows[0].sla_missed_runs, 1);
  assert.deepEqual(
    summary.contributors.packet_rows.map((row) => [
      row.repo,
      row.task_id,
      row.mode,
      row.largest_gap_component,
      row.recommended_next_lane,
    ]),
    [
      ["codex", "event-output", "cold-cli", "packet_latency", "packet-latency"],
      ["vscode", "extension-flow", "warm-stdio", "packet_latency", "packet-latency"],
    ],
  );
});

test("discovers newest default artifact inputs by known prefixes", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-discover-"));
  writeJson(root, "target/retrieval-golden/rootandruntime-prompt-old/summary.json", {
    summary: { prompt_file_recall: 0.2 },
  });
  const rootPrompt = writeJson(
    root,
    "target/retrieval-golden/rootandruntime-prompt-six-lane-bundle-new/summary.json",
    { summary: { prompt_file_recall: 0.4 } },
  );
  const guardPrompt = writeJson(root, "target/retrieval-golden/prompt-guard-six-lane-bundle-new/summary.json", {
    summary: { prompt_file_recall: 1 },
  });
  const symbol = writeJson(root, "target/retrieval-golden/symbol-slice-six-lane-bundle-new/summary.json", {
    summary: { symbol_path_hit_at_k: 1 },
  });
  const packet = writeJson(
    root,
    "target/agent-benchmark/packet-runtime-local-real-six-lane-new/packet-runtime-summary.json",
    { summary: [] },
  );
  fs.writeFileSync(path.join(root, "autoresearch.md"), "agent_ab_overhead_ratio=0.25\n");

  const inputs = discoverInputs({
    cwd: root,
    promptSummaries: [],
    symbolSummary: null,
    packetSummary: null,
    abDoc: path.join(root, "autoresearch.md"),
  });

  assert.deepEqual(inputs.promptSummaries.sort(), [guardPrompt, rootPrompt].sort());
  assert.equal(inputs.symbolSummary, symbol);
  assert.equal(inputs.packetSummary, packet);
});

test("explicit score inputs do not backfill mixed-vintage default artifacts", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-explicit-"));
  const prompt = writeJson(root, "target/retrieval-golden/scout/summary.json", {
    benchmark_run_id: "fresh-scout",
    summary: { prompt_file_recall: 0.5, symbol_path_hit_at_k: 1 },
  });
  writeJson(root, "target/agent-benchmark/packet-runtime-local-real-six-lane-old/packet-runtime-summary.json", {
    benchmark_run_id: "old-packet",
    summary: [],
  });

  const opts = parseArgs([
    "--cwd",
    root,
    "--prompt-summary",
    prompt,
    "--symbol-summary",
    prompt,
    "--no-ab-doc",
  ]);
  const inputs = discoverInputs(opts);

  assert.deepEqual(inputs.promptSummaries, [prompt]);
  assert.equal(inputs.symbolSummary, prompt);
  assert.equal(inputs.packetSummary, null);
  assert.equal(inputs.abDoc, null);
});

test("aggregates retrieval diagnostics once for shared prompt and symbol scout summary", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-agent-value-retrieval-loss-"));
  const searchSummary = writeJson(root, "target/retrieval-golden/scout/summary.json", {
    benchmark_run_id: "segment58-loss-scout",
    summary: {
      prompt_file_recall: 0.5,
      symbol_path_hit_at_k: 1,
      retrieval_expected_files_present_missing_from_search_count: 2,
      retrieval_expected_symbols_present_missing_from_search_count: 1,
      retrieval_query_trace_with_candidates_count: 1,
      retrieval_query_trace_no_candidates_count: 0,
      retrieval_prepare_count: 1,
      retrieval_prepare_failed_count: 1,
      retrieval_prepare_max_latency_ms: 90_070.026,
      retrieval_shadow_candidate_count: 5,
      retrieval_shadow_resolved_hit_count: 3,
      retrieval_shadow_unresolved_candidate_count: 2,
      retrieval_shadow_resolution_counts: [
        { resolution: "node_unresolved", count: 2 },
        { resolution: "resolved", count: 3 },
      ],
    },
    rows: [
      {
        retrieval_shadow: {
          candidate_count: 99,
          resolved_hit_count: 99,
          unresolved_candidate_count: 99,
        },
      },
    ],
  });

  const summary = computeAgentValueScore(
    {
      promptSummaries: [searchSummary],
      symbolSummary: searchSummary,
      packetSummary: null,
      abDoc: null,
    },
    { cwd: root },
  );

  assert.equal(summary.metrics.retrieval_expected_files_present_missing_from_search_count, 2);
  assert.equal(summary.metrics.retrieval_expected_symbols_present_missing_from_search_count, 1);
  assert.equal(summary.metrics.retrieval_query_trace_with_candidates_count, 1);
  assert.equal(summary.metrics.retrieval_prepare_count, 1);
  assert.equal(summary.metrics.retrieval_prepare_failed_count, 1);
  assert.equal(summary.metrics.retrieval_prepare_max_latency_ms, 90_070.026);
  assert.equal(summary.metrics.retrieval_candidate_count, 5);
  assert.equal(summary.metrics.retrieval_resolved_hit_count, 3);
  assert.equal(summary.metrics.retrieval_unresolved_candidate_count, 2);
  assert.equal(summary.metrics.retrieval_candidate_resolution_rate, 0.6);
  assert.deepEqual(summary.contributors.retrieval_diagnostics.resolution_counts, [
    { resolution: "node_unresolved", count: 2 },
    { resolution: "resolved", count: 3 },
  ]);
  assert.ok(
    metricEntries(summary).some(
      ([name, value]) => name === "retrieval_unresolved_candidate_count" && value === 2,
    ),
  );
});

test("metric emission excludes null component scores", () => {
  const entries = metricEntries({
    metrics: {
      agent_value_gap: 0.5,
      packet_score: null,
      live_ab_overhead_ratio_mean: null,
      live_ab_ratio_count: 0,
    },
  });

  assert.deepEqual(entries, [
    ["agent_value_gap", 0.5],
    ["live_ab_ratio_count", 0],
  ]);
});

test("packet composition scoring favors citation-backed recall over answer-text-only mentions", async () => {
  const { packetComposition, packetCompositionFileScore, PACKET_COMPOSITION_WEIGHTS } = await import(
    "../codestory-agent-ab-benchmark.mjs"
  );

  const expectedFiles = [
    "src/lib/project/Project.cpp",
    "src/lib/data/storage/StorageAccessProxy.cpp",
  ];
  const citedOnly = packetComposition(
    {
      answer: {
        citations: expectedFiles.map((file_path, index) => ({
          display_name: `Anchor${index}`,
          file_path,
          line: 1,
        })),
      },
      sufficiency: { avoid_opening: [] },
    },
    { expected_files: expectedFiles },
  );
  const textOnly = packetComposition(
    {
      answer: {
        summary: "Mentions src/lib/data/storage/StorageAccessProxy.cpp only in prose.",
        citations: [{ display_name: "Anchor", file_path: "src/lib/project/Project.cpp", line: 1 }],
      },
      sufficiency: { avoid_opening: [] },
    },
    { expected_files: expectedFiles },
  );

  assert.equal(citedOnly.composition_score, 1);
  assert.ok(textOnly.composition_score < citedOnly.composition_score);
  assert.equal(textOnly.answer_text_file_count, 1);
  assert.equal(PACKET_COMPOSITION_WEIGHTS.cited, 1);
  assert.equal(packetCompositionFileScore({ cited: true, avoid_opening: false, answer_text_mentioned: false, citation_backed_found: true }), 1);
});

test("local-real compact-budget packet rows keep distinct repo task coverage", () => {
  const packet = {
    summary: [
      {
        repo: "vscode",
        task_id: "vscode-workbench-extension-host",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 0.8,
        median_expected_claim_recall: 0.8,
        median_citation_coverage: 0.8,
        median_follow_up_commands_count: 0,
        median_wall_ms: 25_000,
      },
      {
        repo: "codex",
        task_id: "codex-exec-json-flow",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { sufficient: 1 },
        median_expected_file_recall: 0.85,
        median_expected_claim_recall: 0.85,
        median_citation_coverage: 0.75,
        median_follow_up_commands_count: 1,
        median_wall_ms: 12_000,
      },
      {
        repo: "sourcetrail",
        task_id: "sourcetrail-indexing-to-storage",
        mode: "cold-cli",
        quality_pass_runs: 1,
        sufficiency_status_counts: { partial: 1 },
        median_expected_file_recall: 0.7,
        median_expected_claim_recall: 0.7,
        median_citation_coverage: 0.65,
        median_follow_up_commands_count: 2,
        median_wall_ms: 18_000,
      },
    ],
  };

  for (const row of packet.summary) {
    assert.ok(packetRowQualityScore(row) > 0.5, `${row.repo}/${row.task_id} should stay above quality floor`);
    assert.ok(row.median_wall_ms <= 30_000, `${row.repo}/${row.task_id} should stay within compact budget SLA`);
  }
});

function writeJson(root, relativePath, payload) {
  const filePath = path.join(root, relativePath);
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(payload, null, 2)}\n`);
  return filePath;
}
