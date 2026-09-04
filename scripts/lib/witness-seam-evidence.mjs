import assert from "node:assert/strict";
import { createHash } from "node:crypto";

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const mean = (values) => values.reduce((sum, value) => sum + value, 0) / values.length;

function merged(ranges) {
  const result = [];
  for (const [start, end] of ranges.toSorted((a, b) => a[0] - b[0] || a[1] - b[1])) {
    const last = result.at(-1);
    if (last && start <= last[1]) last[1] = Math.max(last[1], end);
    else result.push([start, end]);
  }
  return result;
}

function overlap(range, intervals) {
  return merged(intervals).reduce((sum, [start, end]) =>
    sum + Math.max(0, Math.min(range[1], end) - Math.max(range[0], start)), 0);
}

function lines(bytes) {
  const offsets = [0];
  for (let i = 0; i < bytes.length; i++) if (bytes[i] === 10) offsets.push(i + 1);
  if (offsets.at(-1) !== bytes.length) offsets.push(bytes.length);
  return offsets;
}

export function authenticateRange(range, sources) {
  assert.ok(range.path && !range.path.startsWith("/") && !range.path.includes("\\")
    && !range.path.split("/").some((part) => !part || part === "." || part === ".."), "invalid source path");
  const bytes = sources.get(range.path);
  assert.ok(Buffer.isBuffer(bytes), `missing source ${range.path}`);
  assert.equal(sha256(bytes), range.content_digest, "source digest mismatch");
  const { start, end } = range.byte_range;
  assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(end)
    && start >= 0 && end > start && end <= bytes.length, "invalid byte range");
  for (const offset of [start, end]) assert.ok(offset === bytes.length || (bytes[offset] & 0xc0) !== 0x80, "split UTF-8 range");
  const offsets = lines(bytes);
  assert.equal(offsets.filter((offset) => offset <= start).length, range.line_range.start, "start line mismatch");
  assert.equal(offsets.filter((offset) => offset < end).length, range.line_range.end, "end line mismatch");
  return [start, end];
}

function witnessedBytes(row, sources, allowTruncatedLine) {
  const bytes = sources.get(row.path);
  assert.ok(bytes, "packet source file missing");
  const offsets = lines(bytes);
  const presented = [...row.snippet.matchAll(/^[ >]\s*(\d+) \| (.*)$/gm)];
  assert.ok(presented.length, "source row has no authenticated lines");
  assert.equal(Number(presented[0][1]), row.start_line);
  assert.equal(Number(presented.at(-1)[1]), row.end_line);
  const ranges = [];
  for (const [ordinal, match] of presented.entries()) {
    const line = Number(match[1]);
    assert.equal(line, row.start_line + ordinal, "non-contiguous source presentation");
    assert.ok(line > 0 && line < offsets.length, "source line out of bounds");
    const full = bytes.subarray(offsets[line - 1], offsets[line]);
    const text = Buffer.from(full.toString("utf8").replace(/[\r\n]+$/, ""));
    const shown = Buffer.from(match[2]);
    assert.ok(shown.length <= text.length && shown.equals(text.subarray(0, shown.length)), "fabricated source text");
    if (shown.length < text.length) {
      assert.ok(allowTruncatedLine && ordinal === presented.length - 1
        && row.snippet.includes("\n// ... source truncated by packet row cap\n"), "unmarked partial source line");
    }
    ranges.push([offsets[line - 1], offsets[line - 1] + (shown.length === text.length ? full.length : shown.length)]);
  }
  return ranges;
}

/** Evidence annotations are external inputs. Nothing here runs in the product. */
export function scoreWitnessArm(arm, annotation, sources, { headerControl = false } = {}) {
  assert.ok(annotation.acceptable_sets.length > 0, "no acceptable evidence sets");
  const allowed = new Map();
  for (const set of annotation.acceptable_sets) {
    assert.ok(set.required_source_atoms.length > 0, "empty acceptable source set");
    for (const atom of set.required_source_atoms) authenticateRange(atom.source_range, sources);
    for (const range of set.allowed_support_ranges) {
      const intervals = allowed.get(range.path) ?? [];
      intervals.push(authenticateRange(range, sources));
      allowed.set(range.path, intervals);
    }
  }
  const witnesses = new Map();
  let exposed = 0, relevant = 0;
  assert.ok(arm.support.length <= 16, "public row limit exceeded");
  assert.ok(Buffer.byteLength(JSON.stringify(arm.support)) <= 16 * 1024, "public support budget exceeded");
  for (const row of arm.support) {
    assert.ok(["source_range", "symbol_location"].includes(row.kind), "Phase 1A cannot emit relation or claim rows");
    if (row.kind !== "source_range") continue;
    const ranges = witnessedBytes(row, sources, headerControl);
    const existing = witnesses.get(row.path) ?? [];
    witnesses.set(row.path, existing.concat(ranges));
    for (const range of ranges) {
      exposed += range[1] - range[0];
      relevant += overlap(range, allowed.get(row.path) ?? []);
    }
  }
  const alternatives = annotation.acceptable_sets.map((set) => {
    const covered = set.required_source_atoms.filter(({ source_range: range }) =>
      overlap([range.byte_range.start, range.byte_range.end], witnesses.get(range.path) ?? [])
        === range.byte_range.end - range.byte_range.start).length;
    return { set_id: set.set_id, recall: covered / set.required_source_atoms.length,
      complete_source_set: covered === set.required_source_atoms.length };
  });
  const best = alternatives.toSorted((a, b) => b.recall - a.recall || a.set_id.localeCompare(b.set_id))[0];
  return { ...best, exposed_source_bytes: exposed, relevant_source_bytes: relevant,
    relevant_byte_precision: exposed ? relevant / exposed : 0,
    irrelevant_byte_ratio: exposed ? 1 - relevant / exposed : 1 };
}

export function verifyPairedInputs(receipt, manifest) {
  assert.equal(receipt.contract, "codestory.witness-seam-receipt/v1");
  assert.equal(receipt.case_id, manifest.case_id);
  assert.equal(receipt.phrasing_id, manifest.phrasing_id);
  const control = receipt.control.input, addressed = receipt.addressed.input;
  for (const input of [control, addressed]) {
    assert.deepEqual(input.publication, manifest.publication);
    assert.deepEqual(input.admissions, manifest.descriptors.map((value) => value.admission));
    assert.ok(input.admissions.length <= 16);
    assert.deepEqual(input.relations, []);
    assert.deepEqual(input.ambiguities, []);
    const sources = new Map(input.sources.map((source) => [source.stable_identity, source]));
    const gaps = new Map(input.admission_gaps.map((gap) => [gap.stable_identity, gap]));
    assert.equal(sources.size, input.sources.length, "duplicate hydrated identity");
    assert.equal(gaps.size, input.admission_gaps.length, "duplicate source gap");
    assert.equal(sources.size + gaps.size, input.admissions.length, "every admission needs source or a gap");
    const orderedSources = [];
    input.admissions.forEach((admission, ordinal) => {
      assert.equal(admission.packet_ordinal, ordinal);
      assert.equal(admission.reserved_source_bytes, 512);
      const source = sources.get(admission.stable_identity), gap = gaps.get(admission.stable_identity);
      assert.ok(Boolean(source) !== Boolean(gap), "source and gap must be exclusive");
      const anchor = manifest.descriptors[ordinal].anchor;
      const unaddressed = !anchor || anchor.kind === "path_only";
      if (source) {
        assert.ok(!unaddressed, "unaddressed candidate fabricated source");
        assert.ok(Buffer.byteLength(source.source) <= 512);
        orderedSources.push(source);
      } else {
        assert.equal(gap.kind, unaddressed ? "source_unavailable" : "source_budget_exceeded");
      }
    });
    assert.deepEqual(input.sources, orderedSources, "hydration changed candidate ordering");
  }
  assert.equal(receipt.core_pointer.active.generation_id, manifest.publication.core_generation_id);
  for (const source of control.sources) {
    const other = addressed.sources.find((value) => value.stable_identity === source.stable_identity);
    if (other) {
      assert.equal(other.path, source.path);
      assert.equal(other.parser_completeness, source.parser_completeness);
    }
  }
}

export function phase1AGate(records, expectedCases) {
  const caseIds = [...new Set(records.map((row) => row.case_id))].sort();
  assert.deepEqual(caseIds, [...expectedCases].sort(), "missing or unexpected evidence case");
  const cases = caseIds.map((case_id) => {
    const rows = records.filter((row) => row.case_id === case_id);
    assert.deepEqual(rows.map((row) => row.phrasing_id).sort(), ["original", "paraphrase_1", "paraphrase_2"]);
    return { case_id,
      control_recall: mean(rows.map((row) => row.control.recall)),
      addressed_recall: mean(rows.map((row) => row.addressed.recall)),
      control_irrelevant: mean(rows.map((row) => row.control.irrelevant_byte_ratio)),
      addressed_irrelevant: mean(rows.map((row) => row.addressed.irrelevant_byte_ratio)) };
  });
  const controlRecall = mean(cases.map((row) => row.control_recall));
  const recall = mean(cases.map((row) => row.addressed_recall));
  const controlIrrelevant = mean(cases.map((row) => row.control_irrelevant));
  const irrelevant = mean(cases.map((row) => row.addressed_irrelevant));
  const gates = {
    required_source_recall: recall >= 0.75,
    recall_improvement: controlRecall >= 0.75 || recall - controlRecall >= 0.20,
    irrelevant_bytes_reduction: irrelevant <= controlIrrelevant * 0.8,
  };
  return { contract: "codestory.witness-seam-evaluation/v1", authority: "visible_development_only",
    phase1a: Object.values(gates).every(Boolean) ? "pass" : "fail", gates, cases,
    mean_control_recall: controlRecall, mean_addressed_recall: recall,
    mean_control_irrelevant_byte_ratio: controlIrrelevant, mean_addressed_irrelevant_byte_ratio: irrelevant,
    aggregation_unit: "case_mean_across_three_phrasings", packet_decision: "not_evaluated" };
}
