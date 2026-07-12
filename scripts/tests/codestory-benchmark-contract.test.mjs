import test from "node:test";
import assert from "node:assert/strict";

import {
  assertSidecarMandatoryEnv,
  benchmarkContractCompatibility,
  benchmarkChildEnv,
  benchmarkRunContract,
  retrievalContractSummary,
  retrievalEnv,
  shouldPrepareRetrievalIndex,
  unsupportedSidecarContractRequests,
  unsupportedSidecarDisabledRequest,
} from "../codestory-benchmark-contract.mjs";

test("benchmark child env makes sidecar-primary explicit by default", () => {
  const env = benchmarkChildEnv({});

  assert.equal(env.CODESTORY_RETRIEVAL, "1");
  assert.equal(env.CODESTORY_RETRIEVAL_REAL_EMBEDDINGS, "1");
  assert.equal(env.CODESTORY_RETRIEVAL_COMPOSE_PROFILE, "real");
  assert.equal(env.CODESTORY_EMBED_BACKEND, "llamacpp");
  assert.equal(env.CODESTORY_EMBED_LLAMACPP_URL, "http://127.0.0.1:8080/v1/embeddings");
  assert.equal(env.CODESTORY_EVAL_PROBES, undefined);
  assert.equal(shouldPrepareRetrievalIndex(env), true);
  assert.deepEqual(retrievalContractSummary(env), {
    retrieval_contract: "sidecar_primary_forced",
    sidecar_primary: true,
    unsupported_sidecar_disabled_request: false,
    code_story_retrieval: "1",
    embedding_backend: "llamacpp",
    compose_profile: "real",
  });
});

test("benchmark child env preserves explicit eval probe diagnostics without injecting them", () => {
  const env = benchmarkChildEnv({ CODESTORY_EVAL_PROBES: "1" });

  assert.equal(env.CODESTORY_RETRIEVAL, "1");
  assert.equal(env.CODESTORY_EVAL_PROBES, "1");
});

test("explicit sidecar disable is rejected by the benchmark contract", () => {
  const env = { CODESTORY_RETRIEVAL: "0", CODESTORY_EVAL_PROBES: "0" };

  assert.throws(
    () => benchmarkChildEnv(env),
    /CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory/,
  );
  assert.throws(
    () => shouldPrepareRetrievalIndex(env),
    /CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory/,
  );
  assert.throws(
    () => assertSidecarMandatoryEnv(env),
    /CODESTORY_RETRIEVAL=0 is unsupported; sidecar retrieval is mandatory/,
  );
  assert.equal(unsupportedSidecarDisabledRequest(env), true);
  assert.deepEqual(retrievalContractSummary(env), {
    retrieval_contract: "unsupported_sidecar_disabled",
    sidecar_primary: false,
    unsupported_sidecar_disabled_request: true,
    code_story_retrieval: "0",
  });
});

test("benchmark contract rejects diagnostic sidecar downgrades", () => {
  const env = {
    CODESTORY_RETRIEVAL: "1",
    CODESTORY_RETRIEVAL_SHADOW: "1",
    CODESTORY_QDRANT_ENABLED: "0",
    CODESTORY_RETRIEVAL_REAL_EMBEDDINGS: "0",
    CODESTORY_RETRIEVAL_COMPOSE_PROFILE: "stub",
    CODESTORY_EMBED_BACKEND: "hash",
  };

  const blockers = unsupportedSidecarContractRequests(env);
  assert.equal(blockers.length, 5);
  assert.throws(() => benchmarkChildEnv(env), /Qdrant sidecar is mandatory/);
});

test("retrieval env captures sidecar variables", () => {
  const env = retrievalEnv({
    CODESTORY_RETRIEVAL: "1",
  });

  assert.equal(env.CODESTORY_RETRIEVAL, "1");
  assert.equal(env.CODESTORY_RETRIEVAL_SHADOW, null);
});

test("benchmark reuse contract accepts identical fingerprints", () => {
  const opts = { runner: "codex", model: "gpt-test", sandbox: "workspace-write" };
  const task = { id: "task-a", manifest_path: null, prompt: "Explain flow" };
  const env = benchmarkChildEnv({});
  const current = benchmarkRunContract({ opts, task, env });
  const previous = benchmarkRunContract({ opts, task, env });

  assert.equal(current.compatibility_fingerprint, previous.compatibility_fingerprint);
  assert.deepEqual(benchmarkContractCompatibility(current, previous), {
    compatible: true,
    mismatches: [],
  });
});

test("benchmark reuse contract rejects model drift and missing fingerprints", () => {
  const env = benchmarkChildEnv({});
  const task = { id: "task-a", manifest_path: null, prompt: "Explain flow" };
  const current = benchmarkRunContract({
    opts: { runner: "codex", model: "new-model", sandbox: "workspace-write" },
    task,
    env,
  });
  const previous = benchmarkRunContract({
    opts: { runner: "codex", model: "old-model", sandbox: "workspace-write" },
    task,
    env,
  });

  const drift = benchmarkContractCompatibility(current, previous);
  assert.equal(drift.compatible, false);
  assert.ok(drift.mismatches.some((line) => line.startsWith("model:")));

  const missing = benchmarkContractCompatibility(current, {});
  assert.equal(missing.compatible, false);
  assert.match(missing.mismatches[0], /missing benchmark_contract/);
});
