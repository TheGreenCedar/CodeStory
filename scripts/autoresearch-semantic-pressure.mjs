#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";

const root = process.cwd();
const args = new Set(process.argv.slice(2));
const mode = optionValue("--mode") ?? process.env.CODESTORY_SEMANTIC_PRESSURE_MODE ?? "latest-log";

if (args.has("--help") || args.has("-h")) {
  console.log(`Usage: node scripts/autoresearch-semantic-pressure.mjs [--mode latest-log|full-sidecar|contract-drift-test|foreground-limit-test]

Modes:
  latest-log             Fast setup/lint mode. Reads docs/testing/codestory-e2e-stats-log.md.
  full-sidecar           Runs the release build and ignored full-sidecar repo E2E stats test.
  contract-drift-test    Runs the live incremental contract-drift regression test.
  foreground-limit-test  Runs the live foreground semantic-doc limit regression test.

The primary metric is lower-is-better:
  latest-log/full-sidecar:  semantic_pressure_score = semantic_phase_seconds + index_seconds
  contract-drift-test:      semantic_pressure_score = contract_drift_embedded_docs
  foreground-limit-test:    semantic_pressure_score = foreground_limit_embedded_docs
`);
  process.exit(0);
}

if (mode === "latest-log") {
  const stats = readLatestLoggedStats();
  emitMetrics(stats, { liveMeasurement: 0, source: "latest-log" });
  emitArtifact("stats_log", "docs/testing/codestory-e2e-stats-log.md");
} else if (mode === "full-sidecar") {
  run("cargo", ["build", "--release", "-p", "codestory-cli"]);
  const output = run("cargo", [
    "test",
    "-p",
    "codestory-cli",
    "--test",
    "codestory_repo_e2e_stats",
    "--",
    "--ignored",
    "--nocapture",
  ]);
  const stats = parseStatsJson(output.stdout);
  emitMetrics(stats, { liveMeasurement: 1, source: "full-sidecar" });
} else if (mode === "contract-drift-test") {
  const output = run("cargo", [
    "test",
    "-p",
    "codestory-runtime",
    "incremental_refresh_repairs_touched_file_semantic_docs_when_embedding_contract_changes",
    "--",
    "--nocapture",
  ]);
  const metrics = parseMetricLines(output.stdout);
  const beforeDocs = requiredParsedMetric(metrics, "contract_drift_before_docs");
  const embeddedDocs = requiredParsedMetric(metrics, "contract_drift_embedded_docs");
  const staleDocs = requiredParsedMetric(metrics, "contract_drift_stale_docs");
  console.log("semantic_pressure_source=contract-drift-test");
  emitMetric("semantic_pressure_score", embeddedDocs);
  emitMetric("contract_drift_before_docs", beforeDocs);
  emitMetric("contract_drift_embedded_docs", embeddedDocs);
  emitMetric("contract_drift_stale_docs", staleDocs);
  emitMetric("contract_drift_embed_ratio", beforeDocs > 0 ? embeddedDocs / beforeDocs : 0);
  emitMetric("live_measurement", 1);
} else if (mode === "foreground-limit-test") {
  const output = run("cargo", [
    "test",
    "-p",
    "codestory-runtime",
    "semantic_foreground_limit_defers_unembedded_docs",
    "--",
    "--nocapture",
  ]);
  const metrics = parseMetricLines(output.stdout);
  const pendingDocs = requiredParsedMetric(metrics, "foreground_limit_pending_docs");
  const embeddedDocs = requiredParsedMetric(metrics, "foreground_limit_embedded_docs");
  const staleDocs = requiredParsedMetric(metrics, "foreground_limit_stale_docs");
  const storedDocs = requiredParsedMetric(metrics, "foreground_limit_stored_docs");
  console.log("semantic_pressure_source=foreground-limit-test");
  emitMetric("semantic_pressure_score", embeddedDocs);
  emitMetric("foreground_limit_pending_docs", pendingDocs);
  emitMetric("foreground_limit_embedded_docs", embeddedDocs);
  emitMetric("foreground_limit_stale_docs", staleDocs);
  emitMetric("foreground_limit_stored_docs", storedDocs);
  emitMetric("foreground_limit_deferred_docs", Math.max(0, pendingDocs - embeddedDocs));
  emitMetric("live_measurement", 1);
} else {
  fail(
    `unsupported --mode ${JSON.stringify(
      mode,
    )}; expected latest-log, full-sidecar, contract-drift-test, or foreground-limit-test`,
  );
}

function optionValue(name) {
  const values = process.argv.slice(2);
  for (let index = 0; index < values.length; index += 1) {
    if (values[index] === name) {
      return values[index + 1];
    }
    if (values[index]?.startsWith(`${name}=`)) {
      return values[index].slice(name.length + 1);
    }
  }
  return null;
}

function readLatestLoggedStats() {
  const logPath = join(root, "docs", "testing", "codestory-e2e-stats-log.md");
  if (!existsSync(logPath)) {
    fail(`missing stats log at ${logPath}`);
  }
  const lines = readFileSync(logPath, "utf8").split(/\r?\n/);
  const phaseRows = lines
    .filter((line) => line.startsWith("| 20") && !line.includes("| n/a |"))
    .map(parseMarkdownRow)
    .filter((cells) => cells.length >= 9 && Number.isFinite(Number(cells[3])) && Number.isFinite(Number(cells[5])));
  const row = phaseRows.at(-1);
  if (!row) {
    fail("could not find a numeric phase-metrics row in docs/testing/codestory-e2e-stats-log.md");
  }
  return {
    date: row[0],
    commit: row[1],
    scenario: row[2],
    index_seconds: markdownNumber(row[3]),
    graph_phase_seconds: markdownNumber(row[4]),
    semantic_phase_seconds: markdownNumber(row[5]),
    semantic_docs_reused: markdownNumber(row[6]),
    semantic_docs_embedded: markdownNumber(row[7]),
    semantic_docs_stale: markdownNumber(row[8]),
    retrieval_index_seconds: extractScenarioNumber(row[2], "retrieval_index_seconds"),
    proof_tier: row[2].includes("full_sidecar") || row[2].includes("retrieval_mode full") ? "full_sidecar" : "logged",
  };
}

function parseMarkdownRow(line) {
  return line
    .split("|")
    .slice(1, -1)
    .map((cell) => cell.trim().replace(/^`|`$/g, ""));
}

function markdownNumber(value) {
  return Number(String(value ?? "").replaceAll(",", ""));
}

function extractScenarioNumber(scenario, label) {
  const pattern = new RegExp(`${label}\\s+([0-9]+(?:\\.[0-9]+)?)`);
  const match = pattern.exec(scenario);
  return match ? Number(match[1]) : 0;
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    cwd: root,
    encoding: "utf8",
    shell: process.platform === "win32",
    maxBuffer: 1024 * 1024 * 64,
  });
  if (result.stdout) {
    process.stdout.write(result.stdout);
  }
  if (result.stderr) {
    process.stderr.write(result.stderr);
  }
  if (result.status !== 0) {
    fail(`${command} ${commandArgs.join(" ")} exited with ${result.status}`);
  }
  return result;
}

function parseStatsJson(stdout) {
  const start = stdout.indexOf("{\n  \"project_root\"");
  if (start < 0) {
    fail("could not find RepoE2eStats JSON in full-sidecar output");
  }
  for (let end = stdout.length; end > start; end = stdout.lastIndexOf("}", end - 1)) {
    const candidate = stdout.slice(start, end + 1);
    try {
      return JSON.parse(candidate);
    } catch {
      // Keep searching for the matching final brace.
    }
  }
  fail("could not parse RepoE2eStats JSON from full-sidecar output");
}

function parseMetricLines(stdout) {
  const metrics = new Map();
  for (const line of stdout.split(/\r?\n/)) {
    const match = /^METRIC\s+([A-Za-z0-9_.:-]+)=(-?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)$/i.exec(
      line.trim(),
    );
    if (match) {
      metrics.set(match[1], Number(match[2]));
    }
  }
  return metrics;
}

function requiredParsedMetric(metrics, name) {
  const value = metrics.get(name);
  if (!Number.isFinite(value)) {
    fail(`benchmark output did not include METRIC ${name}=<number>`);
  }
  return value;
}

function emitMetrics(stats, { liveMeasurement, source }) {
  const indexSeconds = requiredNumber(stats, "index_seconds");
  const semanticSeconds = requiredNumber(stats, "semantic_phase_seconds");
  const graphSeconds = optionalNumber(stats, "graph_phase_seconds");
  const retrievalIndexSeconds = optionalNumber(stats, "retrieval_index_seconds");
  const semanticDocsEmbedded = optionalNumber(stats, "semantic_docs_embedded");
  const semanticDocsReused = optionalNumber(stats, "semantic_docs_reused");
  const semanticDocsStale = optionalNumber(stats, "semantic_docs_stale");
  const semanticPressureScore = semanticSeconds + indexSeconds;
  const reuseRatio =
    semanticDocsEmbedded + semanticDocsReused > 0
      ? semanticDocsReused / (semanticDocsEmbedded + semanticDocsReused)
      : 0;

  console.log(`semantic_pressure_source=${source}`);
  console.log(`semantic_pressure_date=${stats.date ?? ""}`);
  console.log(`semantic_pressure_commit=${stats.commit ?? ""}`);
  console.log(`semantic_pressure_proof_tier=${stats.proof_tier ?? ""}`);
  emitMetric("semantic_pressure_score", semanticPressureScore);
  emitMetric("index_seconds", indexSeconds);
  emitMetric("semantic_phase_seconds", semanticSeconds);
  emitMetric("graph_phase_seconds", graphSeconds);
  emitMetric("retrieval_index_seconds", retrievalIndexSeconds);
  emitMetric("semantic_docs_embedded", semanticDocsEmbedded);
  emitMetric("semantic_docs_reused", semanticDocsReused);
  emitMetric("semantic_docs_stale", semanticDocsStale);
  emitMetric("semantic_reuse_ratio", reuseRatio);
  emitMetric("live_measurement", liveMeasurement);
}

function requiredNumber(stats, key) {
  const value = Number(stats[key]);
  if (!Number.isFinite(value)) {
    fail(`stats field ${key} is missing or not numeric`);
  }
  return value;
}

function optionalNumber(stats, key) {
  const value = Number(stats[key] ?? 0);
  return Number.isFinite(value) ? value : 0;
}

function emitMetric(name, value) {
  console.log(`METRIC ${name}=${round(value)}`);
}

function emitArtifact(name, path) {
  console.log(`ARTIFACT ${name}=${resolve(root, path)}`);
}

function round(value) {
  return Number(value).toFixed(6).replace(/\.?0+$/, "");
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
