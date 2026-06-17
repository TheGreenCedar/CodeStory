import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";

const RETRIEVAL_ENV_KEYS = [
  "CODESTORY_RETRIEVAL",
  "CODESTORY_RETRIEVAL_SHADOW",
  "CODESTORY_RETRIEVAL_V2",
  "CODESTORY_RETRIEVAL_V2_SHADOW",
  "CODESTORY_QDRANT_ENABLED",
  "CODESTORY_ZOEKT_ENABLED",
  "CODESTORY_RETRIEVAL_REAL_EMBEDDINGS",
  "CODESTORY_RETRIEVAL_COMPOSE_PROFILE",
  "CODESTORY_EMBED_BACKEND",
  "CODESTORY_EMBED_LLAMACPP_URL",
  "CODESTORY_EVAL_PROBES",
];

const DEFAULT_LLAMACPP_EMBED_URL = "http://127.0.0.1:8080/v1/embeddings";
const BENCHMARK_CONTRACT_VERSION = 1;

function retrievalEnv(env = process.env) {
  return Object.fromEntries(RETRIEVAL_ENV_KEYS.map((key) => [key, env[key] ?? null]));
}

function unsupportedSidecarDisabledRequest(env = process.env) {
  return env.CODESTORY_RETRIEVAL === "0";
}

function unsupportedSidecarContractRequests(env = process.env) {
  const blockers = [];
  if (unsupportedSidecarDisabledRequest(env)) {
    blockers.push("CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory");
  }
  if (env.CODESTORY_RETRIEVAL_V2 != null || env.CODESTORY_RETRIEVAL_V2_SHADOW != null) {
    blockers.push("deprecated CODESTORY_RETRIEVAL_V2 aliases are unsupported; sidecar retrieval is mandatory");
  }
  const shadow = String(env.CODESTORY_RETRIEVAL_SHADOW ?? "").trim().toLowerCase();
  if (shadow && !["0", "false", "no", "off"].includes(shadow)) {
    blockers.push("CODESTORY_RETRIEVAL_SHADOW is unsupported in product benchmarks; sidecar retrieval is primary");
  }
  if (env.CODESTORY_QDRANT_ENABLED === "0" || env.CODESTORY_QDRANT_ENABLED === "false") {
    blockers.push("CODESTORY_QDRANT_ENABLED=0 is unsupported; Qdrant sidecar is mandatory");
  }
  if (env.CODESTORY_ZOEKT_ENABLED === "0" || env.CODESTORY_ZOEKT_ENABLED === "false") {
    blockers.push("CODESTORY_ZOEKT_ENABLED=0 is unsupported; Zoekt sidecar is mandatory");
  }
  if (
    env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS === "0" ||
    env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS === "false"
  ) {
    blockers.push(
      "CODESTORY_RETRIEVAL_REAL_EMBEDDINGS=0 is unsupported; llama.cpp embedding sidecar is mandatory",
    );
  }
  const composeProfile = String(env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE ?? "").trim().toLowerCase();
  if (composeProfile && composeProfile !== "real") {
    blockers.push(
      `CODESTORY_RETRIEVAL_COMPOSE_PROFILE=${composeProfile} is unsupported; profile real is mandatory`,
    );
  }
  const embeddingBackend = String(env.CODESTORY_EMBED_BACKEND ?? "").trim().toLowerCase();
  if (embeddingBackend && !["llamacpp", "llama_cpp"].includes(embeddingBackend)) {
    blockers.push(
      `CODESTORY_EMBED_BACKEND=${embeddingBackend} is unsupported; llama.cpp embedding sidecar is mandatory`,
    );
  }
  return blockers;
}

function assertSidecarMandatoryEnv(env = process.env) {
  const blockers = unsupportedSidecarContractRequests(env);
  if (blockers.length) {
    throw new Error(blockers.join("; "));
  }
}

function retrievalContractSummary(env = process.env) {
  const raw = env.CODESTORY_RETRIEVAL ?? null;
  if (unsupportedSidecarDisabledRequest(env)) {
    return {
      retrieval_contract: "unsupported_sidecar_disabled",
      sidecar_primary: false,
      unsupported_sidecar_disabled_request: true,
      code_story_retrieval: raw,
    };
  }
  return {
    retrieval_contract: raw === "1" ? "sidecar_primary_forced" : "sidecar_primary_default",
    sidecar_primary: true,
    unsupported_sidecar_disabled_request: false,
    code_story_retrieval: raw,
    embedding_backend: env.CODESTORY_EMBED_BACKEND ?? null,
    compose_profile: env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE ?? null,
  };
}

function benchmarkChildEnv(baseEnv = process.env, additions = {}) {
  const env = { ...baseEnv, ...additions };
  if (env.CODESTORY_RETRIEVAL == null || env.CODESTORY_RETRIEVAL === "") {
    env.CODESTORY_RETRIEVAL = "1";
  }
  if (
    env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS == null ||
    env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS === ""
  ) {
    env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS = "1";
  }
  if (env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE == null || env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE === "") {
    env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE = "real";
  }
  if (env.CODESTORY_EMBED_BACKEND == null || env.CODESTORY_EMBED_BACKEND === "") {
    env.CODESTORY_EMBED_BACKEND = "llamacpp";
  }
  if (env.CODESTORY_EMBED_LLAMACPP_URL == null || env.CODESTORY_EMBED_LLAMACPP_URL === "") {
    env.CODESTORY_EMBED_LLAMACPP_URL = DEFAULT_LLAMACPP_EMBED_URL;
  }
  assertSidecarMandatoryEnv(env);
  return env;
}

function shouldPrepareRetrievalIndex(env = process.env) {
  assertSidecarMandatoryEnv(env);
  return true;
}

function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(stableJson).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256Text(value) {
  return createHash("sha256").update(value).digest("hex");
}

function fileSha256(filePath) {
  if (!filePath || !existsSync(filePath)) {
    return null;
  }
  return sha256Text(readFileSync(filePath));
}

function taskManifestHash(task) {
  if (!task) {
    return null;
  }
  if (task.manifest_path && existsSync(task.manifest_path)) {
    return fileSha256(task.manifest_path);
  }
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
    retrieval_contract: retrievalContractSummary(benchmarkChildEnv(env)),
    retrieval_env: retrievalEnv(benchmarkChildEnv(env)),
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
    return {
      compatible: false,
      mismatches: ["previous row is missing benchmark_contract.compatibility_fingerprint"],
    };
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
    mismatches.push("retrieval sidecar contract/env differs");
  }
  if (stableJson(current.packet_threshold_config) !== stableJson(previous.packet_threshold_config)) {
    mismatches.push("packet threshold config differs");
  }
  return { compatible: false, mismatches };
}

export {
  BENCHMARK_CONTRACT_VERSION,
  RETRIEVAL_ENV_KEYS,
  assertSidecarMandatoryEnv,
  benchmarkContractCompatibility,
  benchmarkChildEnv,
  benchmarkRunContract,
  retrievalContractSummary,
  retrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedSidecarDisabledRequest,
  unsupportedSidecarContractRequests,
};
