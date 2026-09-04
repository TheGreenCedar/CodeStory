import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { authenticateFragment, encodedCandidateInput, evaluateArm, exactPublicBytes, fragmentId,
  LIMITS, maximizeCoveredAtoms, scoreOrder, selectSuccessors, sha256 } from "../lib/etr1-evidence.mjs";
import { validateArm, validateDocumentVectorRecord, validateEngine,
  validatePreAnnotationBoundary, parseEvents, validateDocumentCompletions } from "../codestory-etr1-validate.mjs";
import { decision, evaluateEtr1, gateOne, gateTwo } from "../codestory-etr1-evaluate.mjs";
import { fileBinding, readExecutionBinding, validateExecution, executionEnvironment,
  validateCanaryGate } from "../lib/etr1-execution.mjs";

function unit(index) {
  const vector = Array(LIMITS.vectorDimension).fill(0);
  vector[index] = 1;
  return vector;
}

function fixture(expectedName = "control") {
  const bytes = Buffer.from("seed\nsuccessor\n"), digest = sha256(bytes), project_id = "project";
  const make = (path, start, end, line, source) => {
    const fragment = { project_id, path, content_digest: digest, byte_range: { start, end },
      line_range: { start: line, end: line }, source, serialized_row_bytes: 80 };
    fragment.fragment_id = fragmentId(fragment);
    return fragment;
  };
  const seed = make("src/lib.rs", 0, 5, 1, "seed\n");
  const successor = make("src/lib.rs", 5, bytes.length, 2, "successor\n");
  const repository = { project_id, fragment_ids: [seed.fragment_id, successor.fragment_id],
    base_serialized_bytes: 100,
    score_order_sha256: sha256(JSON.stringify([seed.fragment_id, successor.fragment_id])) };
  const question = "find successor", wording = { question, seed_fragment_ids: [seed.fragment_id] };
  const input = expectedName === "control" ? question : `${question}\n\n${seed.source}`;
  const batch = { global_batch_ordinal: 0, arm: expectedName, query_ordinals: [0],
    input_sha256: [sha256(input)], wall_ns: 1, completed_tokens: 2,
    qualification_native_completion_sequence: 1, qualification_server_event_sequence: 10,
    qualification_request_id_sha256: sha256("request-1") };
  const arm = { name: expectedName, search_count: 1,
    query_receipts: [{ query_ordinal: 0, seed_fragment_id: seed.fragment_id,
      original_input_sha256: sha256(input), encoded_input_sha256: sha256(input), encoded_input: input,
      removed_trailing_source_lines: 0, model_limit_rejections: 0, global_batch_ordinal: 0,
      score_order_sha256: repository.score_order_sha256, query_vector: unit(0), scores: [0, 1],
      excluded_before: [seed.fragment_id], retained_successors: [successor.fragment_id] }],
    batch_receipts: [batch], successors: [successor.fragment_id],
    descriptor_pool: [seed.fragment_id, successor.fragment_id],
    hydrated_pool: [seed.fragment_id, successor.fragment_id],
    legally_selectable_pool: [seed.fragment_id, successor.fragment_id],
    source_authentication: { fragment_source_bytes: bytes.length, filesystem_bytes_read: bytes.length,
      authenticated_fragment_ids: [seed.fragment_id, successor.fragment_id],
      file_digests: { "src/lib.rs": digest } }, token_total: 2,
    timing: { round_zero_bm25_ns: 1, seed_source_authentication_ns: 1, query_encoding_ns: 1,
      vector_search_ns: 1, descriptor_mapping_ns: 1, remaining_source_authentication_ns: 1,
      prepared_state_ns: 6, unaccounted_ns: 0 } };
  return { arm, expectedName, wording, repository,
    fragments: new Map([[seed.fragment_id, seed], [successor.fragment_id, successor]]),
    documentVectors: new Map([[seed.fragment_id, unit(1)], [successor.fragment_id, unit(0)]]),
    sourceFiles: new Map([["src/lib.rs", bytes]]), batches: new Map([[0, batch]]), seed, successor };
}

test("fragment identity and source authentication bind every coordinate", () => {
  const { seed, sourceFiles } = fixture();
  authenticateFragment(seed, sourceFiles.get(seed.path));
  for (const mutation of [
    { ...seed, project_id: "other" }, { ...seed, path: "src/other.rs" },
    { ...seed, byte_range: { start: 1, end: 5 } }, { ...seed, source: "fake\n" },
  ]) assert.throws(() => authenticateFragment(mutation, sourceFiles.get(seed.path)));
});

test("cumulative exclusions page deterministically through 128 unique successors", () => {
  const seeds = new Set(Array.from({ length: 16 }, (_, index) => `seed-${index}`));
  const order = [...seeds, ...Array.from({ length: 140 }, (_, index) => `successor-${String(index).padStart(3, "0")}`)]
    .map((id, index) => ({ id, score: 1 - index / 1000 }));
  const prior = new Set();
  for (let query = 0; query < 16; query++)
    selectSuccessors(order, seeds, prior).forEach((id) => prior.add(id));
  assert.equal(prior.size, 128);
  assert.deepEqual([...prior].slice(0, 8), Array.from({ length: 8 }, (_, index) =>
    `successor-${String(index).padStart(3, "0")}`));
  const tied = scoreOrder(["z", "a", "m"], [0.5, 0.5, 0.4]);
  assert.deepEqual(tied.map(({ id }) => id), ["a", "z", "m"]);
});

test("candidate input shortening preserves UTF-8 and complete trailing lines", () => {
  const source = "first α\nsecond β\nthird γ\n";
  assert.equal(encodedCandidateInput("question", source, 1), "question\n\nfirst α\nsecond β\n");
  assert.throws(() => encodedCandidateInput("question", source, 3));
});

test("validator uses native completion identity for automatic token events", () => {
  const event = (nativeSequence) => JSON.stringify({ schema_version: 1, sequence: 0,
    action: "completed_tokens", status: "completed", server_event_sequence: 3 + nativeSequence,
    clock: {}, details: { completed_tokens: "5", native_completion_sequence: String(nativeSequence),
      request_id: `request-${nativeSequence}` } });
  const events = parseEvents(Buffer.from(`${event(1)}\n${event(2)}\n`));
  assert.deepEqual(events.map(({ sequence }) => sequence), [0, 0]);
  assert.throws(() => parseEvents(Buffer.from(`${event(1)}\n${event(1)}\n`)));
});

test("validator reconstructs both arm query contracts and refuses hostile mutations", () => {
  for (const name of ["control", "candidate"]) validateArm(fixture(name));
  const mutations = [
    (value) => { value.wording.question = "changed"; },
    (value) => { value.arm.query_receipts[0].encoded_input = "find successor\nseed\n"; },
    (value) => { value.arm.query_receipts[0].scores.pop(); },
    (value) => { value.arm.successors.push(value.successor.fragment_id); },
    (value) => { value.arm.query_receipts[0].excluded_before = []; },
    (value) => { value.arm.query_receipts[0].encoded_input = "find successor"; value.arm.query_receipts[0].removed_trailing_source_lines = 1; },
  ];
  for (const mutate of mutations) {
    const value = fixture("candidate");
    mutate(value);
    assert.throws(() => validateArm(value));
  }
});

test("request wall timing includes unaccounted work and rejects impossible phase totals", () => {
  const value = fixture();
  value.arm.timing.prepared_state_ns = 10;
  value.arm.timing.unaccounted_ns = 4;
  validateArm(value);
  value.arm.timing.unaccounted_ns = 0;
  assert.throws(() => validateArm(value), /timing/u);
});

test("vector, model, and pre-annotation boundaries refuse substitutions", () => {
  const { seed } = fixture();
  validateDocumentVectorRecord({ id: seed.fragment_id, purpose: "document",
    text_sha256: sha256(seed.source), vector: unit(0) }, seed);
  assert.throws(() => validateDocumentVectorRecord({ id: seed.fragment_id, purpose: "symbol",
    text_sha256: sha256(seed.source), vector: unit(0) }, seed));
  const engine = { model_digest: "666db8df27c88570cdc07adca28646260038b8ca65354911d57b936ebf56efaa",
    materialized_model_sha256: "666db8df27c88570cdc07adca28646260038b8ca65354911d57b936ebf56efaa",
    policy: "accelerated", accelerator_execution_verified: true, worker_alive: true,
    embedded_model: true, load_error: null };
  validateEngine(engine);
  const omittedError = { ...engine }; delete omittedError.load_error;
  validateEngine(omittedError);
  assert.throws(() => validateEngine({ ...engine, load_error: "failed" }));
  assert.throws(() => validateEngine({ ...engine, model_digest: "f".repeat(64) }));
  const run = { annotation_access: "not_accessed", graph_invocations: 0, bge_invocations: 0,
    symbol_document_invocations: 0, host_query_invocations: 0, production_packet_invocations: 0 };
  validatePreAnnotationBoundary(run, { annotation_access: "not_accessed" });
  for (const mutate of [(value) => { value.annotation_access = "accessed"; },
    (value) => { value.graph_invocations = 1; }]) {
    const changed = structuredClone(run); mutate(changed);
    assert.throws(() => validatePreAnnotationBoundary(changed, { annotation_access: "not_accessed" }));
  }
});

test("execution uses only its recorded secret-free process environment", () => {
  const env = executionEnvironment({ PATH: "/bin", HOME: "/users/test", OMP_NUM_THREADS: "4",
    CODESTORY_CACHE_ROOT: "/cache", API_TOKEN: "private", NODE_OPTIONS: "--require bad.cjs",
    UNRELATED: "hidden" });
  assert.deepEqual(env, { ...executionEnvironment({}), CODESTORY_CACHE_ROOT: "/cache", HOME: "/users/test",
    OMP_NUM_THREADS: "4", PATH: "/bin" });
});

test("document completions reconcile every fixed diagnostic batch before annotations", () => {
  const event = (i) => JSON.stringify({ schema_version: 1, sequence: 0,
    action: "completed_tokens", status: "completed", server_event_sequence: i + 10, clock: {},
    details: { native_completion_sequence: String(i), completed_tokens: "25", request_id: `doc-${i}` } });
  const eventsBytes = Buffer.from(`${event(1)}\n${event(2)}\n`);
  const stderrBytes = Buffer.from("encoded 16/17 records; batch_ms=1\nencoded 17/17 records; batch_ms=2\n");
  const input = { eventsBytes, stderrBytes, recordCount: 17 };
  assert.deepEqual(validateDocumentCompletions(input), { batches: 2, records: 17, completed_tokens: 50 });
  for (const changed of [
    { eventsBytes: Buffer.from(`${event(1)}\n`) },
    { eventsBytes: Buffer.alloc(0) },
    { stderrBytes: Buffer.from("encoded 16/17 records; batch_ms=1\n") },
    { stderrBytes: Buffer.from("encoded 15/17 records; batch_ms=1\nencoded 17/17 records; batch_ms=2\n") },
    { recordCount: 18 },
  ]) assert.throws(() => validateDocumentCompletions({ ...input, ...changed }));
});

test("corpus launch requires a completed identity-matched canary before model access", async () => {
  await assert.rejects(() => validateCanaryGate(undefined, {}), /passing canary receipt required/u);
  const directory = await mkdtemp(path.join(os.tmpdir(), "etr1-canary-gate-"));
  const file = path.join(directory, "receipt.json");
  await writeFile(file, JSON.stringify({ contract: "codestory.etr1-synthetic-canary/v2",
    authority: "synthetic_canary_only", experiment_status: "invalid" }));
  const binding = await fileBinding(file);
  await assert.rejects(() => validateCanaryGate(binding, {}), /canary did not pass/u);
  await writeFile(file, "{}");
  await assert.rejects(() => validateCanaryGate(binding, {}), /execution artifact/u);
});

test("exact optimizer selects successors without compulsory discovery seeds", () => {
  const rowBytes = new Map([["seed", 15_000], ["successor", 100]]);
  const result = maximizeCoveredAtoms([["successor"]], rowBytes, 100);
  assert.deepEqual(result.selected, ["successor"]);
  assert.equal(result.covered, 1);
});

test("twelve independently discovered successors fit as twelve rows", () => {
  const ids = Array.from({ length: 12 }, (_, index) => `successor-${index}`),
    rowBytes = new Map(ids.map((id) => [id, 100]));
  const result = maximizeCoveredAtoms(ids.map((id) => [id]), rowBytes, 100);
  assert.equal(result.covered, 12);
  assert.equal(result.rows, 12);
  assert.equal(result.public_bytes, 100 + 1200 + 11);
});

test("shared rows are charged once and exact row and byte boundaries hold", () => {
  const costs = new Map([["shared", 100], ["a", 50], ["b", 50]]);
  const shared = maximizeCoveredAtoms([["shared", "a"], ["shared", "b"]], costs, 100);
  assert.equal(shared.public_bytes, exactPublicBytes(100, ["a", "b", "shared"], costs));
  const sixteen = Array.from({ length: 16 }, (_, index) => `r${index}`),
    exactCosts = new Map(sixteen.map((id) => [id, 1000]));
  assert.equal(maximizeCoveredAtoms([sixteen], exactCosts, 369).covered, 1);
  assert.equal(maximizeCoveredAtoms([sixteen], exactCosts, 370).covered, 0);
  const seventeen = [...sixteen, "r16"], seventeenCosts = new Map([...exactCosts, ["r16", 1]]);
  assert.equal(maximizeCoveredAtoms([seventeen], seventeenCosts, 1).covered, 0);
});

test("acceptable alternatives stay separate and relation-only evidence earns no credit", () => {
  const { seed, successor } = fixture(), fragments = [seed, successor];
  const range = (fragment) => ({ path: fragment.path, content_digest: fragment.content_digest,
    byte_range: fragment.byte_range, line_range: fragment.line_range });
  const annotation = { acceptable_sets: [
    { set_id: "seed-route", required_source_atoms: [{ atom_id: "seed", source_range: range(seed) }],
      required_relation_atoms: [{ atom_id: "ignored-relation" }] },
    { set_id: "successor-route", required_source_atoms: [{ atom_id: "successor",
      source_range: range(successor) }], required_relation_atoms: [] },
  ] };
  const result = evaluateArm(annotation, fragments, [successor.fragment_id], 100);
  assert.equal(result.best_set_id, "successor-route");
  assert.equal(result.recall, 1);
  assert.deepEqual(result.reachable_atoms, ["successor"]);
});

test("an atom spanning two fragments receives no partial credit", () => {
  const { seed, successor } = fixture(), annotation = { acceptable_sets: [{ set_id: "whole",
    required_source_atoms: [{ atom_id: "both", source_range: { path: seed.path,
      content_digest: seed.content_digest, byte_range: { start: 0, end: successor.byte_range.end },
      line_range: { start: 1, end: 2 } } }], required_relation_atoms: [] }] };
  const partial = evaluateArm(annotation, [seed, successor], [seed.fragment_id], 100);
  assert.equal(partial.recall, 0);
  assert.equal(partial.complete_source_set, false);
});

test("the frozen decision table never authorizes production integration", () => {
  const sufficient = { mean_recall: 0.9, complete_set_rate: 0.8,
    groups: { a: { mean_recall: 0.8 }, b: { mean_recall: 0.9 } } };
  const weak = { mean_recall: 0.5, complete_set_rate: 0.4,
    groups: { a: { mean_recall: 0.5 }, b: { mean_recall: 0.5 } } };
  assert.equal(gateOne(sufficient).pass, true);
  assert.equal(gateOne(weak).pass, false);
  assert.equal(decision(false, false, false).decision, "no_frontier_selected");
  assert.equal(decision(false, true, false).decision, "no_frontier_selected");
  assert.equal(decision(true, true, false).decision,
    "unconditioned_frontier_selected");
  assert.equal(decision(false, true, true).decision,
    "conditioned_frontier_selected");
  const cases = [{ control_incomplete_for_gain: true, candidate_gained_atom: true }];
  assert.equal(gateTwo(cases, weak, sufficient, true).pass, true);
});

test("annotations cannot be opened before a valid validator receipt", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "etr1-boundary-"));
  const validation = path.join(directory, "validation.json"), bytes = Buffer.from(JSON.stringify({
    contract: "codestory.etr1-validation/v1", experiment_status: "invalid",
    decision: "not_evaluated", annotation_access: "not_accessed",
  }));
  await writeFile(validation, bytes);
  await assert.rejects(() => evaluateEtr1({ validationPath: validation,
    validationSha256: sha256(bytes), annotationsPath: path.join(directory, "must-not-open.json"),
    annotationsSha256: "0".repeat(64), oraclePath: path.join(directory, "oracle.json"),
    oracleSha256: "0".repeat(64), sourceRoot: directory }), /validator did not authorize/u);
});

test("independently frozen execution bindings reject rehashed substitute outputs", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "etr1-execution-"));
  const output = path.join(directory, "vectors.json");
  await writeFile(output, JSON.stringify({ vector: unit(0) }));
  const frozen = await fileBinding(output);
  await readExecutionBinding(frozen);
  await writeFile(output, JSON.stringify({ vector: unit(1) }));
  const substitute = await fileBinding(output);
  assert.notEqual(substitute.sha256, frozen.sha256);
  await assert.rejects(() => readExecutionBinding(frozen), /execution artifact digest changed/u);
  await assert.rejects(() => validateExecution(undefined, { role: "documents" }),
    /independent execution receipt required/u);
});
