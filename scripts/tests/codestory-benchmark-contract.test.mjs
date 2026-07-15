import test from "node:test";
import assert from "node:assert/strict";

import {
  assertRetrievalEngineEnv,
  benchmarkContractCompatibility,
  benchmarkChildEnv,
  benchmarkRunContract,
  retrievalContractSummary,
  retrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedRetrievalContractRequests,
} from "../codestory-benchmark-contract.mjs";

test("benchmark child env uses one fixed in-process retrieval contract", () => {
  const env = benchmarkChildEnv({ CODESTORY_EMBED_ALLOW_CPU: "1" });

  assert.equal(env.CODESTORY_RETRIEVAL, "1");
  assert.equal(shouldPrepareRetrievalIndex(env), true);
  assert.deepEqual(retrievalContractSummary(env), {
    retrieval_contract: "in_process_v1",
    retrieval_enabled: true,
    embedding_engine: "process_shared",
    execution_policy: "cpu_explicit",
  });
  assert.deepEqual(retrievalEnv(env), {
    CODESTORY_RETRIEVAL: "1",
    CODESTORY_EMBED_ALLOW_CPU: "1",
    CODESTORY_EVAL_PROBES: null,
  });
});

test("benchmark contract rejects disabled retrieval and invalid CPU policy", () => {
  const env = {
    CODESTORY_RETRIEVAL: "0",
    CODESTORY_EMBED_ALLOW_CPU: "sometimes",
  };

  assert.equal(unsupportedRetrievalContractRequests(env).length, 2);
  assert.throws(() => assertRetrievalEngineEnv(env), /full retrieval is mandatory/u);
  assert.throws(() => benchmarkChildEnv(env), /must be 0 or 1/u);
});

test("benchmark reuse contract accepts identical fingerprints and rejects drift", () => {
  const task = { id: "task-a", manifest_path: null, prompt: "Explain flow" };
  const current = benchmarkRunContract({
    opts: { runner: "codex", model: "new-model", sandbox: "workspace-write" },
    task,
    env: { CODESTORY_EMBED_ALLOW_CPU: "1" },
  });
  const identical = benchmarkRunContract({
    opts: { runner: "codex", model: "new-model", sandbox: "workspace-write" },
    task,
    env: { CODESTORY_EMBED_ALLOW_CPU: "1" },
  });
  const previous = benchmarkRunContract({
    opts: { runner: "codex", model: "old-model", sandbox: "workspace-write" },
    task,
    env: { CODESTORY_EMBED_ALLOW_CPU: "1" },
  });

  assert.deepEqual(benchmarkContractCompatibility(current, identical), { compatible: true, mismatches: [] });
  assert.ok(benchmarkContractCompatibility(current, previous).mismatches.some((line) => line.startsWith("model:")));
  assert.match(benchmarkContractCompatibility(current, {}).mismatches[0], /missing benchmark_contract/u);
});
