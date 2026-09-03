#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs as parseNodeArgs } from "node:util";

import {
  BUILDER_ABLATION_ARMS,
  BUILDER_ABLATION_TASK_IDS,
  evidenceCompilerBuilderAcceptance,
  evidenceCompilerExperimentValidity,
  marginalValue,
} from "./lib/evidence-compiler-ablation.mjs";
import { canaryBlockers } from "./lib/builder-operation-canary.mjs";

function parseArgs(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      "run-dir": { type: "string" },
      adjudication: { type: "string" },
      attempt: { type: "string" },
      "causal-classification": { type: "string" },
    },
  });
  if (!values["run-dir"] || !values.adjudication || !values.attempt) {
    throw new Error("usage: codestory-evidence-compiler-ablation-evaluate.mjs --run-dir <dir> --adjudication <json> --attempt <initial|general_revision> [--causal-classification <new|equivalent>]");
  }
  if (!["initial", "general_revision"].includes(values.attempt)) {
    throw new Error("--attempt must be initial or general_revision");
  }
  if (
    values["causal-classification"] != null &&
    !["new", "equivalent"].includes(values["causal-classification"])
  ) {
    throw new Error("--causal-classification must be new or equivalent");
  }
  return {
    runDir: path.resolve(values["run-dir"]),
    adjudication: path.resolve(values.adjudication),
    attempt: values.attempt,
    causalClassification: values["causal-classification"] ?? null,
  };
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function parseJsonl(text) {
  return text.split(/\r?\n/u).filter(Boolean).map((line, index) => {
    try {
      return JSON.parse(line);
    } catch (error) {
      throw new Error(`invalid runs.jsonl row ${index + 1}: ${error.message}`);
    }
  });
}

function formatRatio(value) {
  if (value == null) return "missing";
  if (!Number.isFinite(value)) return "infinite";
  return `${(value * 100).toFixed(2)}%`;
}

function markdownReceipt(receipt) {
  if (receipt.experiment_status === "invalid") {
    return `# Evidence compiler experiment invalid\n\nPacket decision: **not_evaluated**\n\n${receipt.experiment_validity.reasons.map((reason) => `- ${reason}`).join("\n")}\n\nNo quality aggregate or layer decision was computed. Earlier receipts and their stop decisions are unchanged.\n`;
  }
  const acceptance = receipt.packet_acceptance;
  const graph = receipt.default_layer_decisions.graph_relations;
  const semantic = receipt.default_layer_decisions.dense_semantic_candidates;
  return `# Evidence compiler builder ablation

Next action: **${receipt.packet_decision}**

- Packet versus explicit primitives task-pass difference: ${acceptance.mean_task_pass_difference?.toFixed(4) ?? "missing"}
- Exploratory repository-context-action ratio: ${formatRatio(acceptance.exploratory_repository_context_action_ratio)}
- Whole-task wall ratio: ${formatRatio(acceptance.whole_task_wall_ratio)}
- Input-context ratio: ${formatRatio(acceptance.input_context_ratio)}
- Independent critical-claim adjudication complete: ${acceptance.adjudication_complete}
- Graph relations default: ${graph}
- Dense packet candidates default: ${semantic}

${acceptance.reasons.length ? `## Blocking reasons\n\n${acceptance.reasons.map((reason) => `- ${reason}`).join("\n")}\n` : ""}
This is builder-visible development evidence. It cannot be used as sealed release evidence.
`;
}

function packetNextAction(packetPassed, attempt, causalClassification) {
  if (packetPassed) {
    if (causalClassification != null) {
      throw new Error("a passing packet gate cannot carry a failure causal classification");
    }
    return "advance";
  }
  if (attempt === "general_revision") return "stop";
  if (causalClassification === "new") return "revise_once";
  if (causalClassification === "equivalent") return "stop";
  return "failed_needs_causal_classification";
}

async function evaluate(opts) {
  const attempt = opts.attempt ?? "initial";
  const causalClassification = opts.causalClassification ?? null;
  if (!["initial", "general_revision"].includes(attempt)) {
    throw new Error("attempt must be initial or general_revision");
  }
  const runsPath = path.join(opts.runDir, "runs.jsonl");
  if (!existsSync(runsPath)) throw new Error(`missing ${runsPath}`);
  const runBytes = await readFile(runsPath);
  const rows = parseJsonl(runBytes.toString("utf8"));
  const adjudication = JSON.parse(await readFile(opts.adjudication, "utf8"));
  const runsSha256 = sha256(runBytes);
  if (adjudication.source_runs_sha256 !== runsSha256) {
    throw new Error("adjudication source_runs_sha256 does not match runs.jsonl");
  }
  if (existsSync(path.join(opts.runDir, "builder-ablation.json"))) {
    throw new Error("refusing to replace an existing builder receipt; preserve its decision");
  }
  const summaryPath = path.join(opts.runDir, "summary.json");
  const summary = existsSync(summaryPath)
    ? JSON.parse(await readFile(summaryPath, "utf8"))
    : null;
  const validity = evidenceCompilerExperimentValidity(rows);
  validity.reasons.push(...canaryBlockers(
    summary?.builder_ablation?.operation_canary,
    rows.find((row) => row.codestory_prelude_cli_sha256)?.codestory_prelude_cli_sha256,
  ));
  validity.valid = validity.reasons.length === 0;
  const packetAcceptance = validity.valid ? evidenceCompilerBuilderAcceptance(rows, adjudication) : null;
  const graphRelations = validity.valid ? marginalValue(
    rows,
    "exact_plus_relations",
    "exact_identity_source",
    adjudication,
  ) : null;
  const denseSemanticCandidates = validity.valid ? marginalValue(
    rows,
    "packet_semantic_on",
    "packet_semantic_off",
    adjudication,
  ) : null;
  const packetDecision = validity.valid ? packetNextAction(
    packetAcceptance.pass,
    attempt,
    causalClassification,
  ) : "not_evaluated";
  const receipt = {
    generated_at: new Date().toISOString(),
    evidence_status: "builder_visible_development_only",
    release_authority: false,
    historical_corpus_burned: true,
    experiment_attempt: attempt,
    causal_classification: causalClassification,
    packet_decision: packetDecision,
    experiment_status: validity.valid ? "valid" : "invalid",
    experiment_validity: validity,
    source_runs_sha256: runsSha256,
    adjudication_sha256: sha256(await readFile(opts.adjudication)),
    source_commit: summary?.source_commit ?? summary?.shard?.attestation?.source_commit ?? null,
    source_tree: summary?.source_tree ?? summary?.shard?.attestation?.source_tree ?? null,
    arms: BUILDER_ABLATION_ARMS,
    task_ids: BUILDER_ABLATION_TASK_IDS,
    packet_acceptance: packetAcceptance,
    marginal_value: {
      graph_relations: graphRelations,
      dense_semantic_candidates: denseSemanticCandidates,
    },
    default_layer_decisions: {
      graph_relations: !validity.valid ? "not_evaluated" : graphRelations.positive ? "keep_default" : "disable_default",
      dense_semantic_candidates: !validity.valid ? "not_evaluated" : denseSemanticCandidates.positive
        ? "keep_default"
        : "disable_default",
    },
  };
  await writeFile(
    path.join(opts.runDir, "builder-ablation.json"),
    `${JSON.stringify(receipt, null, 2)}\n`,
    "utf8",
  );
  await writeFile(
    path.join(opts.runDir, "builder-ablation.md"),
    markdownReceipt(receipt),
    "utf8",
  );
  console.log(`wrote ${path.join(opts.runDir, "builder-ablation.json")}`);
  return receipt;
}

export { evaluate, markdownReceipt, packetNextAction, parseArgs, parseJsonl };

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  evaluate(parseArgs(process.argv.slice(2))).then((receipt) => {
    if (receipt.packet_decision !== "advance") process.exitCode = 1;
  }).catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
