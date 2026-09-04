import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, realpath, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { authenticateFragment, encodedCandidateInput, f32Dot, fragmentId, LIMITS,
  scoreOrder, selectSuccessors, sha256, validateVector } from "./lib/etr1-evidence.mjs";

const FIXED = Object.freeze({ parent: "c9c935d87129a79f326b650bbf23d73191df8b4f",
  fragment_diagnostic: "ca185ed13c635bbb4b64cc6760c5025799700359ebcc4dd3bcc53e34f8cf9194",
  fragment_build: "2201780e1a752db4bfcceb047bf5cd0b5a854733c4050330ef87575b960f3baf",
  lexical_membership_freeze: "e6867b5c79706160021ec5edf60792273345ca97cae8377e69934d5e2c9992ee",
  questions: "8e7219a59c973c02f8ea93120bb680da46a75b8272153986c76e55bfb73ca3b6",
  annotations: "52b0cc223292bc70f1e4fa3f52b67bf42a91e4d4b9ed997aa12c648c068e9ade",
  model_contract: "cb0e3c00290f1eb21ecdcd873521d03331069b1efa766fcd1e493e6d4299b4b7",
  model: "666db8df27c88570cdc07adca28646260038b8ca65354911d57b936ebf56efaa",
  tokenizer: "7465b93c945b7a266481e6785aa13e505c625562c1c046c4b762bb4da4d46082",
  lexical_policy_source: "43b2478d75abd3d5689d05e08c072e4148fd21ab29bcc55533d30a494edf986b" });

async function boundBytes(binding, expectedPath) {
  assert.ok(binding && path.isAbsolute(binding.path), "bound path is not absolute");
  if (expectedPath) assert.equal(await realpath(binding.path), await realpath(expectedPath), "bound path differs");
  const metadata = await stat(binding.path);
  assert.ok(metadata.isFile(), "bound path is not a regular file");
  const bytes = await readFile(binding.path);
  assert.equal(bytes.length, binding.bytes, "bound length changed");
  assert.equal(sha256(bytes), binding.sha256, "bound digest changed");
  return bytes;
}

async function boundJson(binding, expectedPath) {
  return JSON.parse(await boundBytes(binding, expectedPath));
}

function renderSnippet(fragment) {
  let result = "```text\n";
  const lines = fragment.source.match(/.*(?:\n|$)/gu).filter(Boolean);
  lines.forEach((line, offset) => {
    result += ` ${String(fragment.line_range.start + offset).padStart(5)} | ${line.replace(/[\r\n]+$/u, "")}\n`;
  });
  return `${result}\`\`\``;
}

function serializedRowBytes(fragment) {
  return Buffer.byteLength(JSON.stringify({ kind: "source_range", path: fragment.path,
    start_line: fragment.line_range.start, end_line: fragment.line_range.end,
    snippet: renderSnippet(fragment), content_digest: fragment.content_digest,
    byte_range: fragment.byte_range }));
}

export function validateEngine(engine) {
  assert.equal(engine.model_digest, FIXED.model, "engine model changed");
  assert.equal(engine.materialized_model_sha256, FIXED.model, "materialized model changed");
  assert.equal(engine.policy, "accelerated", "engine policy changed");
  assert.equal(engine.accelerator_execution_verified, true, "accelerator execution unverified");
  assert.equal(engine.worker_alive, true, "embedding worker is not alive");
  assert.equal(engine.embedded_model, true, "embedding model was not embedded");
  assert.equal(engine.load_error, null, "embedding engine recorded a load error");
}

export function validatePreAnnotationBoundary(run, preparation) {
  assert.equal(run.annotation_access, "not_accessed", "run accessed annotations before validation");
  assert.equal(preparation.annotation_access, "not_accessed",
    "preparation accessed annotations before validation");
  for (const key of ["graph_invocations", "bge_invocations", "symbol_document_invocations",
    "host_query_invocations", "production_packet_invocations"])
    assert.equal(run[key], 0, `${key} is forbidden`);
}

export function validateDocumentVectorRecord(record, fragment) {
  assert.equal(record.id, fragment.fragment_id, "document vector order changed");
  assert.equal(record.purpose, "document", "symbol or query document substituted");
  assert.equal(record.text_sha256, sha256(fragment.source), "document vector text changed");
  validateVector(record.vector, "document vector");
}

function validateSourceAuthentication(arm, fragments, repository, sourceFiles) {
  assert.deepEqual(arm.source_authentication.authenticated_fragment_ids, arm.hydrated_pool,
    "source authentication differs from H");
  let fragmentBytes = 0;
  const paths = new Set();
  for (const id of arm.hydrated_pool) {
    const fragment = fragments.get(id);
    assert.ok(fragment, "hydrated fragment is absent");
    assert.equal(fragment.project_id, repository.project_id, "hydrated fragment belongs to another project");
    authenticateFragment(fragment, sourceFiles.get(fragment.path));
    fragmentBytes += Buffer.byteLength(fragment.source);
    paths.add(fragment.path);
  }
  assert.equal(arm.source_authentication.fragment_source_bytes, fragmentBytes,
    "authenticated fragment byte total changed");
  const filesystemBytes = [...paths].reduce((sum, relative) => sum + sourceFiles.get(relative).length, 0);
  assert.equal(arm.source_authentication.filesystem_bytes_read, filesystemBytes,
    "filesystem byte total changed");
  assert.deepEqual(Object.keys(arm.source_authentication.file_digests).sort(), [...paths].sort(),
    "authenticated file set changed");
  for (const relative of paths)
    assert.equal(arm.source_authentication.file_digests[relative], sha256(sourceFiles.get(relative)),
      "authenticated file digest changed");
}

export function validateArm({ arm, expectedName, wording, repository, fragments, documentVectors,
  sourceFiles, batches }) {
  assert.equal(arm.name, expectedName);
  const seeds = wording.seed_fragment_ids;
  assert.equal(arm.search_count, seeds.length, "logical search count changed");
  assert.equal(arm.query_receipts.length, seeds.length, "query receipt count changed");
  assert.ok(arm.successors.length <= LIMITS.successors, "successor ceiling exceeded");
  assert.equal(new Set(arm.successors).size, arm.successors.length, "successors are duplicated");
  assert.deepEqual(arm.descriptor_pool, [...seeds, ...arm.successors], "D equation changed");
  assert.ok(arm.descriptor_pool.length <= LIMITS.pool, "descriptor ceiling exceeded");
  assert.deepEqual(arm.hydrated_pool, arm.descriptor_pool, "H differs from fully authenticated D");
  assert.deepEqual(arm.legally_selectable_pool, [...new Set(arm.hydrated_pool)].filter((id) => {
    const fragment = fragments.get(id);
    return repository.base_serialized_bytes + fragment.serialized_row_bytes <= LIMITS.bytes;
  }), "L equation changed");
  const seedSet = new Set(seeds), prior = new Set();
  for (let ordinal = 0; ordinal < arm.query_receipts.length; ordinal++) {
    const query = arm.query_receipts[ordinal], seed = fragments.get(seeds[ordinal]);
    assert.equal(query.query_ordinal, ordinal, "query ordinal changed");
    assert.equal(query.seed_fragment_id, seeds[ordinal], "query seed changed");
    const original = expectedName === "control" ? wording.question : `${wording.question}\n\n${seed.source}`;
    assert.equal(query.original_input_sha256, sha256(original), "original query digest changed");
    const encoded = expectedName === "control" ? wording.question
      : encodedCandidateInput(wording.question, seed.source, query.removed_trailing_source_lines);
    assert.equal(query.encoded_input, encoded, "encoded query construction changed");
    assert.equal(query.encoded_input_sha256, sha256(encoded), "encoded query digest changed");
    if (expectedName === "control") {
      assert.equal(query.removed_trailing_source_lines, 0, "control query was shortened");
      assert.equal(query.model_limit_rejections, 0, "control query exceeded the model limit");
    } else {
      assert.ok(query.model_limit_rejections >= query.removed_trailing_source_lines,
        "candidate shortening lacks a typed model-limit rejection");
    }
    validateVector(query.query_vector, "query vector");
    assert.equal(query.score_order_sha256, repository.score_order_sha256, "score-order binding changed");
    assert.equal(query.scores.length, repository.fragment_ids.length, "complete score vector missing");
    query.scores.forEach((score, index) => {
      assert.ok(Number.isFinite(score), "semantic score is nonfinite");
      const recomputed = f32Dot(query.query_vector, documentVectors.get(repository.fragment_ids[index]));
      assert.ok(Math.abs(score - recomputed) <= 2e-6, "semantic score differs from normalized dot product");
    });
    const expectedExclusions = [...new Set([...seedSet, ...prior])].sort();
    assert.deepEqual(query.excluded_before, expectedExclusions, "cumulative exclusions changed");
    const selected = selectSuccessors(scoreOrder(repository.fragment_ids, query.scores), seedSet, prior);
    assert.deepEqual(query.retained_successors, selected, "top-eight successor selection changed");
    selected.forEach((id) => prior.add(id));
    const batch = batches.get(query.global_batch_ordinal);
    assert.ok(batch && batch.arm === expectedName, "query references the wrong successful batch");
    const position = batch.query_ordinals.indexOf(ordinal);
    assert.ok(position >= 0, "query missing from its successful batch");
    assert.equal(batch.input_sha256[position], query.encoded_input_sha256,
      "successful batch input digest changed");
  }
  assert.deepEqual(arm.successors, [...prior], "successor pool equation changed");
  assert.ok(arm.batch_receipts.every((batch) => batch.query_ordinals.length >= 1
    && batch.query_ordinals.length <= 8 && batch.wall_ns > 0 && batch.completed_tokens > 0),
  "batch contract invalid");
  const batchOrdinals = arm.batch_receipts.flatMap((batch) => batch.query_ordinals).toSorted((a, b) => a - b);
  assert.deepEqual(batchOrdinals, Array.from({ length: seeds.length }, (_, index) => index),
    "successful batches do not partition the queries");
  assert.equal(arm.token_total,
    arm.batch_receipts.reduce((sum, batch) => sum + batch.completed_tokens, 0), "arm token total changed");
  const timingKeys = ["round_zero_bm25_ns", "seed_source_authentication_ns", "query_encoding_ns",
    "vector_search_ns", "descriptor_mapping_ns", "remaining_source_authentication_ns"];
  timingKeys.forEach((key) => assert.ok(Number.isSafeInteger(arm.timing[key]) && arm.timing[key] >= 0,
    `invalid timing ${key}`));
  assert.equal(arm.timing.prepared_state_ns,
    timingKeys.reduce((sum, key) => sum + arm.timing[key], 0), "prepared timing does not reconcile");
  validateSourceAuthentication(arm, fragments, repository, sourceFiles);
}

export function parseEvents(bytes) {
  assert.equal(bytes.at(-1), 10, "qualification event log is unterminated");
  let previous = null;
  return bytes.toString("utf8").trimEnd().split("\n").map(JSON.parse).filter((event) => {
    assert.equal(event.schema_version, 1, "qualification event schema changed");
    assert.ok(previous === null || event.sequence > previous, "qualification event sequence changed");
    previous = event.sequence;
    return event.action === "completed_tokens";
  });
}

async function loadSourceFiles(repository, repositoryFragments) {
  const root = await realpath(repository.local_root), result = new Map();
  for (const relative of new Set(repositoryFragments.map((fragment) => fragment.path))) {
    assert.ok(relative && !path.isAbsolute(relative) && !relative.includes("\\")
      && !relative.split("/").some((part) => !part || part === "." || part === ".."),
    "source path escapes project");
    const absolute = await realpath(path.join(root, relative));
    assert.ok(absolute.startsWith(`${root}${path.sep}`), "source path escapes project");
    result.set(relative, await readFile(absolute));
  }
  return result;
}

export async function validateEtr1({ runBinding, runPath, sourceRoot }) {
  const run = await boundJson(runBinding, runPath);
  assert.equal(run.contract, "codestory.etr1-run/v1");
  assert.equal(run.authority, "visible_development_frontier_only");
  assert.equal(run.experiment_status, "awaiting_validation");
  assert.equal(run.decision, "not_evaluated");
  assert.equal(run.parent_head, FIXED.parent);
  assert.equal(run.annotation_access, "not_accessed");
  assert.equal(run.vector_artifact_loaded_before_timing, true);
  const sourceCommit = execFileSync("git", ["-C", sourceRoot, "rev-parse", "HEAD^{commit}"],
    { encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_TERMINAL_PROMPT: "0" } }).trim();
  const sourceTree = execFileSync("git", ["-C", sourceRoot, "rev-parse", "HEAD^{tree}"],
    { encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_TERMINAL_PROMPT: "0" } }).trim();
  assert.equal(run.build.source_commit, sourceCommit, "run source commit differs from validator checkout");
  assert.equal(run.build.source_tree, sourceTree, "run source tree differs from validator checkout");
  assert.equal(run.build.source_dirty, false, "run binary was built dirty");
  assert.equal(sha256(await readFile(run.build.binary_path)), run.build.binary_sha256, "run binary changed");
  const preparation = await boundJson(run.preparation);
  assert.equal(preparation.contract, "codestory.etr1-preparation/v1");
  validatePreAnnotationBoundary(run, preparation);
  assert.equal(preparation.annotations.sha256, FIXED.annotations);
  assert.equal(preparation.model_sha256, FIXED.model);
  assert.equal(preparation.tokenizer_sha256, FIXED.tokenizer);
  assert.equal(preparation.build.source_commit, sourceCommit);
  assert.equal(preparation.build.source_tree, sourceTree);
  assert.equal(run.method_sha256, preparation.method.sha256);
  for (const name of ["fragment_diagnostic", "fragment_build", "lexical_membership_freeze",
    "questions", "model_contract", "lexical_policy_source"])
    assert.equal(preparation.fixed_inputs[name].sha256, FIXED[name], `fixed input changed: ${name}`);
  await boundBytes(preparation.method);
  await boundBytes(preparation.embedding_input);
  for (const binding of Object.values(preparation.fixed_inputs)) await boundBytes(binding);
  assert.equal(preparation.fragments.length, 10_369, "fragment count changed");
  assert.equal(preparation.wordings.length, 72, "wording count changed");
  const fragments = new Map(), fragmentsByRepository = new Map();
  for (const fragment of preparation.fragments) {
    assert.equal(fragment.fragment_id, fragmentId(fragment), "prepared fragment identity changed");
    assert.equal(fragment.serialized_row_bytes, serializedRowBytes(fragment), "public row cost changed");
    assert.ok(!fragments.has(fragment.fragment_id), "duplicate prepared fragment");
    fragments.set(fragment.fragment_id, fragment);
    const list = fragmentsByRepository.get(fragment.project_id) ?? [];
    list.push(fragment);
    fragmentsByRepository.set(fragment.project_id, list);
  }
  const repositories = new Map(preparation.repositories.map((repository) => [repository.repository_id, repository]));
  for (const repository of repositories.values()) {
    assert.equal(sha256(JSON.stringify(repository.fragment_ids)), repository.score_order_sha256,
      "repository score order changed");
    assert.equal(new Set(repository.fragment_ids).size, repository.fragment_ids.length,
      "repository fragment order contains duplicates");
  }
  const vectorArtifact = await boundJson(run.fragment_vectors);
  assert.equal(vectorArtifact.contract, "codestory.embedding-diagnostic-output/v1");
  assert.equal(vectorArtifact.input_sha256, preparation.embedding_input.sha256);
  assert.equal(vectorArtifact.source_commit, sourceCommit);
  assert.equal(vectorArtifact.source_tree, sourceTree);
  validateEngine(vectorArtifact.initial_engine);
  validateEngine(vectorArtifact.final_engine);
  assert.equal(vectorArtifact.initial_engine.server_instance_id, vectorArtifact.final_engine.server_instance_id);
  assert.equal(vectorArtifact.records.length, preparation.fragments.length, "document vector count changed");
  const documentVectors = new Map();
  vectorArtifact.records.forEach((record, index) => {
    const fragment = preparation.fragments[index];
    validateDocumentVectorRecord(record, fragment);
    assert.ok(!documentVectors.has(record.id), "duplicate document vector");
    documentVectors.set(record.id, record.vector);
  });
  validateEngine(run.initial_engine);
  validateEngine(run.final_engine);
  assert.equal(run.initial_engine.server_instance_id, run.final_engine.server_instance_id,
    "query engine changed during ETR-1");
  const eventBytes = await boundBytes(run.qualification_events), events = parseEvents(eventBytes);
  const batches = new Map(), rows = [];
  assert.equal(run.rows.length, preparation.wordings.length, "run row count changed");
  for (let rowIndex = 0; rowIndex < run.rows.length; rowIndex++) {
    const row = await boundJson(run.rows[rowIndex]), wording = preparation.wordings[rowIndex];
    assert.equal(row.contract, "codestory.etr1-wording/v1");
    for (const key of ["case_id", "phrasing_id", "repository_id", "group", "question_sha256"])
      assert.equal(row[key], wording[key], `row ${key} changed`);
    assert.deepEqual(row.seed_fragment_ids, wording.seed_fragment_ids, "arm seed manifest changed");
    const repository = repositories.get(wording.repository_id);
    assert.ok(repository, "row repository missing");
    const repositoryFragments = repository.fragment_ids.map((id) => fragments.get(id));
    const sourceFiles = await loadSourceFiles(repository, repositoryFragments);
    for (const fragment of repositoryFragments) authenticateFragment(fragment, sourceFiles.get(fragment.path));
    for (const arm of [row.control, row.candidate]) for (const batch of arm.batch_receipts) {
      assert.ok(!batches.has(batch.global_batch_ordinal), "global batch ordinal duplicated");
      batches.set(batch.global_batch_ordinal, batch);
    }
    validateArm({ arm: row.control, expectedName: "control", wording, repository, fragments,
      documentVectors, sourceFiles, batches });
    validateArm({ arm: row.candidate, expectedName: "candidate", wording, repository, fragments,
      documentVectors, sourceFiles, batches });
    rows.push(row);
  }
  assert.deepEqual([...batches.keys()].sort((a, b) => a - b),
    Array.from({ length: batches.size }, (_, index) => index), "global batch sequence has gaps");
  assert.equal(events.length, batches.size, "qualification event count differs from successful batches");
  for (const [ordinal, batch] of [...batches].sort((left, right) => left[0] - right[0])) {
    const event = events[ordinal];
    assert.equal(event.sequence, batch.qualification_event_sequence, "batch event sequence changed");
    assert.equal(Number(event.details.completed_tokens), batch.completed_tokens, "batch token count changed");
    assert.ok(event.details.request_id, "batch request identity missing");
    assert.ok(Number(event.details.native_completion_sequence) > 0, "native completion identity missing");
  }
  assert.equal(run.qualification_completed_token_total,
    [...batches.values()].reduce((sum, batch) => sum + batch.completed_tokens, 0),
  "qualification token total changed");
  return { run, preparation, rows, source_commit: sourceCommit, source_tree: sourceTree,
    batch_count: batches.size, query_count: rows.reduce((sum, row) =>
      sum + row.control.search_count + row.candidate.search_count, 0) };
}

async function main() {
  const { values } = parseArgs({ options: {
    run: { type: "string" }, "run-sha256": { type: "string" }, output: { type: "string" },
  } });
  for (const name of ["run", "run-sha256", "output"]) assert.ok(values[name], `missing --${name}`);
  assert.ok(path.isAbsolute(values.run) && path.isAbsolute(values.output), "paths must be absolute");
  const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  let receipt;
  try {
    const validated = await validateEtr1({ runBinding: { path: values.run,
      sha256: values["run-sha256"], bytes: (await stat(values.run)).size }, runPath: values.run, sourceRoot });
    receipt = { contract: "codestory.etr1-validation/v1", experiment_status: "valid",
      decision: "not_evaluated", annotation_access: "not_accessed",
      run: { path: values.run, sha256: values["run-sha256"], bytes: (await stat(values.run)).size },
      source_commit: validated.source_commit, source_tree: validated.source_tree,
      binary_sha256: validated.run.build.binary_sha256,
      preparation_sha256: validated.run.preparation.sha256,
      fragment_vectors_sha256: validated.run.fragment_vectors.sha256,
      row_count: validated.rows.length, batch_count: validated.batch_count,
      query_count: validated.query_count, source_address_validity: 1 };
  } catch (error) {
    receipt = { contract: "codestory.etr1-validation/v1", experiment_status: "invalid",
      decision: "not_evaluated", annotation_access: "not_accessed",
      run: { path: values.run, sha256: values["run-sha256"] }, error: error.message };
  }
  const bytes = `${JSON.stringify(receipt, null, 2)}\n`;
  await writeFile(values.output, bytes, { flag: "wx", mode: 0o600 });
  console.log(`${sha256(bytes)}  ${values.output}`);
  if (receipt.experiment_status !== "valid") process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
