import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { authenticateRange, phase1AGate, scoreWitnessArm, sha256, verifyPairedInputs } from "../lib/witness-seam-evidence.mjs";

function fixture() {
  const bytes = Buffer.from("// preamble\nfn café() { work(); }\nfn alternate() {}\n");
  const source = bytes.toString();
  const range = (text, line) => ({ path: "src/a.rs", byte_range: { start: bytes.indexOf(text), end: bytes.indexOf(text) + Buffer.byteLength(text) },
    line_range: { start: line, end: line }, content_digest: sha256(bytes) });
  const primary = range("fn café() { work(); }\n", 2);
  const alternative = range("fn alternate() {}\n", 3);
  return { source, bytes, sources: new Map([["src/a.rs", bytes]]), primary,
    annotation: { acceptable_sets: [primary, alternative].map((atom, index) => ({ set_id: String(index),
      required_source_atoms: [{ atom_id: "witness", source_range: atom }], allowed_support_ranges: [primary, alternative] })) },
    row: { kind: "source_range", path: "src/a.rs", start_line: 2, end_line: 2, snippet: "```text\n>    2 | fn café() { work(); }\n```" } };
}

test("evidence coverage admits independently supported alternatives and counts actual bytes", () => {
  const f = fixture();
  const primary = scoreWitnessArm({ support: [f.row] }, f.annotation, f.sources);
  assert.equal(primary.recall, 1);
  assert.equal(primary.relevant_byte_precision, 1);
  const other = { ...f.row, start_line: 3, end_line: 3, snippet: "```text\n>    3 | fn alternate() {}\n```" };
  assert.equal(scoreWitnessArm({ support: [other] }, f.annotation, f.sources).recall, 1);
  const header = { ...f.row, start_line: 1, end_line: 1, snippet: "```text\n>    1 | // preamble\n```" };
  assert.equal(scoreWitnessArm({ support: [header] }, f.annotation, f.sources).irrelevant_byte_ratio, 1);
  assert.equal(scoreWitnessArm({ support: [header] }, f.annotation, f.sources).recall, 0);
});

test("source authentication rejects fabricated, stale, aliased, and partial addresses", () => {
  const f = fixture();
  for (const range of [
    { ...f.primary, content_digest: "0".repeat(64) },
    { ...f.primary, path: "a.rs" },
    { ...f.primary, path: "src/../a.rs" },
    { ...f.primary, line_range: { start: 1, end: 1 } },
    { ...f.primary, byte_range: { start: f.bytes.indexOf("é") + 1, end: f.primary.byte_range.end } },
  ]) assert.throws(() => authenticateRange(range, f.sources));
  assert.throws(() => scoreWitnessArm({ support: [{ ...f.row, snippet: f.row.snippet.replace("work", "fake") }] }, f.annotation, f.sources));
  const truncated = { ...f.row, snippet: "```text\n>    2 | fn café() {\n// ... source truncated by packet row cap\n```" };
  assert.throws(() => scoreWitnessArm({ support: [truncated] }, f.annotation, f.sources));
  const control = scoreWitnessArm({ support: [truncated] }, f.annotation, f.sources, { headerControl: true });
  assert.equal(control.recall, 0, "a partial line never covers an entire atom");
  assert.equal(control.exposed_source_bytes, Buffer.byteLength("fn café() {"));
  assert.throws(() => scoreWitnessArm({ support: [{ kind: "typed_graph_edge" }] }, f.annotation, f.sources));
});

test("Phase 1A aggregates phrasings inside cases and rejects selective subsets", () => {
  const rows = ["a", "b"].flatMap((case_id) => ["original", "paraphrase_1", "paraphrase_2"].map((phrasing_id) => ({
    case_id, phrasing_id, control: { recall: 0.2, irrelevant_byte_ratio: 0.8 },
    addressed: { recall: case_id === "a" ? 1 : 0.5, irrelevant_byte_ratio: 0.5 },
  })));
  const result = phase1AGate(rows, ["a", "b"]);
  assert.equal(result.mean_addressed_recall, 0.75);
  assert.equal(result.phase1a, "pass");
  assert.equal(result.packet_decision, "not_evaluated");
  assert.throws(() => phase1AGate(rows.slice(1), ["a", "b"]));
  assert.throws(() => phase1AGate(rows.filter((row) => row.case_id === "a"), ["a", "b"]));
});

test("the paired-input contract binds cardinality, charge, order, and publication", () => {
  const admissions = Array.from({ length: 16 }, (_, packet_ordinal) => ({ packet_ordinal, reserved_source_bytes: 512, stable_identity: `node:${packet_ordinal}` }));
  const publication = { core_generation_id: "core" };
  const input = { publication, admissions, sources: admissions.map((item) => ({ stable_identity: item.stable_identity, source: "bounded" })),
    relations: [], ambiguities: [], admission_gaps: [] };
  const manifest = { case_id: "a", phrasing_id: "original", publication, descriptors: admissions.map((admission) => ({ admission, anchor: { kind: "indexed_node" } })) };
  const receipt = { contract: "codestory.witness-seam-receipt/v1", case_id: "a", phrasing_id: "original",
    core_pointer: { active: { generation_id: "core" } }, control: { input }, addressed: { input: structuredClone(input) } };
  verifyPairedInputs(receipt, manifest);
  for (const mutate of [
    (copy) => copy.addressed.input.admissions.pop(),
    (copy) => copy.addressed.input.admissions.reverse(),
    (copy) => copy.addressed.input.admissions[0].reserved_source_bytes++,
    (copy) => copy.addressed.input.publication.core_generation_id = "other",
    (copy) => copy.addressed.input.sources[0].source = "x".repeat(513),
  ]) { const copy = structuredClone(receipt); mutate(copy); assert.throws(() => verifyPairedInputs(copy, manifest)); }
});

test("paired retrieval exhaustion and missing precision remain observations, not invalid experiments", () => {
  for (const count of [0, 1, 15, 16]) {
    const admissions = Array.from({ length: count }, (_, packet_ordinal) => ({
      packet_ordinal, reserved_source_bytes: 512, stable_identity: `node:${packet_ordinal}`,
    }));
    const publication = { core_generation_id: "core" };
    const sources = admissions.slice(1).map(({ stable_identity }) => ({ stable_identity, source: "bounded" }));
    const admission_gaps = count ? [{ kind: "source_unavailable", stable_identity: "node:0", exact_selector_ordinal: null }] : [];
    const input = { publication, admissions, sources, relations: [], ambiguities: [], admission_gaps };
    const manifest = { case_id: "a", phrasing_id: "original", publication, descriptors: admissions.map((admission, i) => ({
      admission, anchor: i ? { kind: "indexed_node" } : null,
    })) };
    const receipt = { contract: "codestory.witness-seam-receipt/v1", case_id: "a", phrasing_id: "original",
      core_pointer: { active: { generation_id: "core" } }, control: { input }, addressed: { input: structuredClone(input) } };
    verifyPairedInputs(receipt, manifest);
    if (count) {
      const missing = structuredClone(receipt);
      missing.addressed.input.admission_gaps = [];
      assert.throws(() => verifyPairedInputs(missing, manifest), "every missing source requires a typed gap");
      const invented = structuredClone(receipt);
      invented.addressed.input.sources.unshift({ stable_identity: "node:0", source: "header" });
      assert.throws(() => verifyPairedInputs(invented, manifest), "unaddressed candidates never gain source");
    }
  }
});

test("invalid artifacts cannot create a quality aggregate or overwrite a decision", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "witness-evaluation-"));
  const output = path.join(root, "evaluation.json");
  const script = new URL("../codestory-witness-seam-evaluate.mjs", import.meta.url);
  const argv = [script.pathname, "--output", output];
  for (const kind of ["questions", "annotations", "runs"])
    argv.push(`--${kind}`, path.join(root, `${kind}.json`), `--${kind}-sha256`, "0".repeat(64));
  const failed = spawnSync(process.execPath, argv, { encoding: "utf8" });
  assert.equal(failed.status, 1);
  const bytes = await readFile(output, "utf8");
  const report = JSON.parse(bytes);
  assert.equal(report.experiment_status, "invalid");
  assert.equal(report.phase1a, "blocked");
  assert.equal(report.packet_decision, "not_evaluated");
  assert.equal(report.mean_addressed_recall, undefined);
  assert.equal(spawnSync(process.execPath, argv).status, 1);
  assert.equal(await readFile(output, "utf8"), bytes);
});
