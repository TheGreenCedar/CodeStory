import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";

const RETRIEVAL_ENV_KEYS = [
  "CODESTORY_RETRIEVAL",
  "CODESTORY_EMBED_ALLOW_CPU",
  "CODESTORY_EVAL_PROBES",
];

const BENCHMARK_CONTRACT_VERSION = 2;

function retrievalEnv(env = process.env) {
  return Object.fromEntries(RETRIEVAL_ENV_KEYS.map((key) => [key, env[key] ?? null]));
}

function unsupportedRetrievalDisabledRequest(env = process.env) {
  return env.CODESTORY_RETRIEVAL === "0";
}

function unsupportedRetrievalContractRequests(env = process.env) {
  const blockers = [];
  if (unsupportedRetrievalDisabledRequest(env)) {
    blockers.push("CODESTORY_RETRIEVAL=0 is unsupported; full retrieval is mandatory");
  }
  const cpuPolicy = String(env.CODESTORY_EMBED_ALLOW_CPU ?? "").trim();
  if (cpuPolicy && cpuPolicy !== "0" && cpuPolicy !== "1") {
    blockers.push("CODESTORY_EMBED_ALLOW_CPU must be 0 or 1");
  }
  return blockers;
}

function assertRetrievalEngineEnv(env = process.env) {
  const blockers = unsupportedRetrievalContractRequests(env);
  if (blockers.length) throw new Error(blockers.join("; "));
}

function retrievalContractSummary(env = process.env) {
  return {
    retrieval_contract: "in_process_v1",
    retrieval_enabled: !unsupportedRetrievalDisabledRequest(env),
    embedding_engine: "process_shared",
    execution_policy: env.CODESTORY_EMBED_ALLOW_CPU === "1" ? "cpu_explicit" : "accelerated",
  };
}

function benchmarkChildEnv(baseEnv = process.env, additions = {}) {
  const env = { ...baseEnv, ...additions };
  env.CODESTORY_RETRIEVAL ||= "1";
  assertRetrievalEngineEnv(env);
  return env;
}

function shouldPrepareRetrievalIndex(env = process.env) {
  assertRetrievalEngineEnv(env);
  return true;
}

function stableJson(value) {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256Text(value) {
  return createHash("sha256").update(value).digest("hex");
}

function fileSha256(filePath) {
  return filePath && existsSync(filePath) ? sha256Text(readFileSync(filePath)) : null;
}

function taskManifestHash(task) {
  if (!task) return null;
  if (task.manifest_path && existsSync(task.manifest_path)) return fileSha256(task.manifest_path);
  return sha256Text(stableJson(task.task_manifest_snapshot ?? task));
}

function benchmarkRunContract({
  opts = {},
  task = null,
  env = process.env,
  harnessPath = null,
  scorerPath = null,
  cliIdentity = null,
} = {}) {
  const childEnv = benchmarkChildEnv(env);
  const contract = {
    contract_version: BENCHMARK_CONTRACT_VERSION,
    task_id: task?.id ?? null,
    task_manifest_hash: taskManifestHash(task),
    scorer_hash: fileSha256(scorerPath),
    harness_hash: fileSha256(harnessPath),
    runner: opts.runner ?? null,
    model: opts.model ?? null,
    sandbox: opts.sandbox ?? null,
    codestory_cli: cliIdentity ?? opts.codestoryCli ?? null,
    retrieval_contract: retrievalContractSummary(childEnv),
    retrieval_env: retrievalEnv(childEnv),
    packet_threshold_config: {
      task_suite: opts.taskSuite ?? null,
      max_source_reads_after_packet: opts.maxSourceReadsAfterPacket ?? null,
      diagnostic_extra_probes_from_manifest: Boolean(opts.diagnosticExtraProbesFromManifest),
      packet_gate_improved_from: opts.packetGateImprovedFrom ?? null,
    },
  };
  return {
    ...contract,
    compatibility_fingerprint: sha256Text(stableJson(contract)),
    promotion_eligible: true,
  };
}

function benchmarkContractCompatibility(current, previous) {
  if (!previous?.compatibility_fingerprint) {
    return { compatible: false, mismatches: ["previous row is missing benchmark_contract.compatibility_fingerprint"] };
  }
  if (current.compatibility_fingerprint === previous.compatibility_fingerprint) {
    return { compatible: true, mismatches: [] };
  }
  const keys = [
    "contract_version",
    "task_id",
    "task_manifest_hash",
    "scorer_hash",
    "harness_hash",
    "runner",
    "model",
    "sandbox",
    "codestory_cli",
  ];
  const mismatches = keys
    .filter((key) => stableJson(current[key] ?? null) !== stableJson(previous[key] ?? null))
    .map((key) => `${key}: current=${stableJson(current[key] ?? null)} previous=${stableJson(previous[key] ?? null)}`);
  if (
    stableJson(current.retrieval_contract) !== stableJson(previous.retrieval_contract)
    || stableJson(current.retrieval_env) !== stableJson(previous.retrieval_env)
  ) {
    mismatches.push("retrieval engine contract/env differs");
  }
  if (stableJson(current.packet_threshold_config) !== stableJson(previous.packet_threshold_config)) {
    mismatches.push("packet threshold config differs");
  }
  return { compatible: false, mismatches };
}

export {
  BENCHMARK_CONTRACT_VERSION,
  RETRIEVAL_ENV_KEYS,
  assertRetrievalEngineEnv,
  benchmarkContractCompatibility,
  benchmarkChildEnv,
  benchmarkRunContract,
  retrievalContractSummary,
  retrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedRetrievalContractRequests,
  unsupportedRetrievalDisabledRequest,
};
