import assert from "node:assert/strict";
import { createHash } from "node:crypto";

export const LIMITS = Object.freeze({ rows: 16, bytes: 16 * 1024, seeds: 16,
  successorsPerQuery: 8, successors: 128, pool: 144, vectorDimension: 768 });

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

// Match Rust split_inclusive('\n'): CR and Unicode separators remain source,
// never additional boundaries or reasons to discard text.
export const sourceLines = (source) => source.match(/[^\n]*\n|[^\n]+$/gu) ?? [];

export function fragmentId(fragment) {
  const framed = [];
  for (const value of [fragment.project_id, fragment.path, fragment.content_digest]) {
    const bytes = Buffer.from(value);
    const length = Buffer.alloc(8);
    length.writeBigUInt64LE(BigInt(bytes.length));
    framed.push(length, bytes);
  }
  const bounds = Buffer.alloc(16);
  bounds.writeBigUInt64LE(BigInt(fragment.byte_range.start), 0);
  bounds.writeBigUInt64LE(BigInt(fragment.byte_range.end), 8);
  return sha256(Buffer.concat([Buffer.from("codestory.frozen-fragment/v1\0"), ...framed, bounds]));
}

export function validateVector(vector, label = "vector") {
  assert.equal(vector.length, LIMITS.vectorDimension, `${label} dimension changed`);
  assert.ok(vector.every(Number.isFinite), `${label} contains a nonfinite value`);
  const norm = vector.reduce((sum, value) => sum + value * value, 0);
  assert.ok(Math.abs(norm - 1) < 0.001, `${label} is not normalized`);
}

export function f32Dot(left, right) {
  assert.equal(left.length, right.length, "dot-product dimensions differ");
  let sum = 0;
  for (let index = 0; index < left.length; index++)
    sum = Math.fround(sum + Math.fround(Math.fround(left[index]) * Math.fround(right[index])));
  return sum;
}

export function scoreOrder(fragmentIds, scores) {
  assert.equal(fragmentIds.length, scores.length, "score vector length differs from repository order");
  return fragmentIds.map((id, index) => ({ id, score: scores[index] }))
    .toSorted((left, right) => right.score - left.score || left.id.localeCompare(right.id));
}

export function selectSuccessors(order, seeds, prior, limit = LIMITS.successorsPerQuery) {
  const excluded = new Set([...seeds, ...prior]), seen = new Set(), selected = [];
  for (const { id } of order) {
    if (!excluded.has(id) && !seen.has(id)) {
      seen.add(id);
      selected.push(id);
      if (selected.length === limit) break;
    }
  }
  return selected;
}

export function encodedCandidateInput(question, source, removedTrailingLines) {
  assert.ok(Number.isSafeInteger(removedTrailingLines) && removedTrailingLines >= 0,
    "invalid removed-line count");
  const lines = sourceLines(source);
  assert.ok(lines.length > removedTrailingLines, "all seed lines were removed");
  const retained = lines.slice(0, lines.length - removedTrailingLines).join("");
  assert.ok(retained.trim(), "retained seed source is empty");
  return `${question}\n\n${retained}`;
}

function lineOffsets(bytes) {
  const offsets = [0];
  for (let index = 0; index < bytes.length; index++) if (bytes[index] === 10) offsets.push(index + 1);
  if (offsets.at(-1) !== bytes.length) offsets.push(bytes.length);
  return offsets;
}

export function authenticateFragment(fragment, bytes) {
  assert.equal(fragment.fragment_id, fragmentId(fragment), "fragment identity changed");
  assert.equal(sha256(bytes), fragment.content_digest, "fragment file digest changed");
  const { start, end } = fragment.byte_range;
  assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(end)
    && start >= 0 && end > start && end <= bytes.length, "fragment range invalid");
  for (const offset of [start, end])
    assert.ok(offset === bytes.length || (bytes[offset] & 0xc0) !== 0x80, "fragment range splits UTF-8");
  assert.equal(bytes.subarray(start, end).toString("utf8"), fragment.source, "fragment source changed");
  const offsets = lineOffsets(bytes);
  assert.equal(offsets.filter((offset) => offset <= start).length, fragment.line_range.start,
    "fragment start line changed");
  assert.equal(offsets.filter((offset) => offset < end).length, fragment.line_range.end,
    "fragment end line changed");
}

export function requiredFragments(atom, fragments) {
  const matching = fragments.map((fragment, index) => ({ fragment, index }))
    .filter(({ fragment }) => fragment.path === atom.path && fragment.content_digest === atom.content_digest)
    .toSorted((left, right) => left.fragment.byte_range.start - right.fragment.byte_range.start);
  for (let index = 1; index < matching.length; index++)
    assert.ok(matching[index - 1].fragment.byte_range.end <= matching[index].fragment.byte_range.start,
      "frozen fragments overlap");
  const required = [];
  let cursor = atom.byte_range.start;
  for (const { fragment, index } of matching) {
    const range = fragment.byte_range;
    if (range.end <= atom.byte_range.start || range.start >= atom.byte_range.end) continue;
    if (range.start > cursor) return null;
    required.push(index);
    cursor = Math.max(cursor, range.end);
  }
  return cursor >= atom.byte_range.end ? required : null;
}

export function exactPublicBytes(baseBytes, selected, rowBytes) {
  return baseBytes + selected.reduce((sum, id) => sum + rowBytes.get(id), 0)
    + Math.max(0, selected.length - 1);
}

function compareIdentity(left, right) {
  if (typeof left === "number" && typeof right === "number") return left - right;
  assert.equal(typeof left, typeof right, "optimizer identity types differ");
  return left < right ? -1 : left > right ? 1 : 0;
}

/** Exact union-of-whole-atom optimizer. Requirements are arrays of selectable IDs or null. */
export function maximizeCoveredAtoms(requirements, rowBytes, baseBytes,
  limits = { rows: LIMITS.rows, bytes: LIMITS.bytes }) {
  assert.ok(Number.isSafeInteger(baseBytes) && baseBytes >= 0 && baseBytes <= limits.bytes,
    "invalid fixed packet bytes");
  const ids = [...new Set(requirements.filter(Boolean).flat())].sort(compareIdentity);
  ids.forEach((id) => assert.ok(Number.isSafeInteger(rowBytes.get(id)) && rowBytes.get(id) > 0,
    `missing row cost for ${id}`));
  for (const requirement of requirements) if (requirement)
    assert.ok(requirement.length > 0 && new Set(requirement).size === requirement.length,
      "atom requirement is empty or duplicated");
  const bitFor = new Map(ids.map((id, index) => [id, 1n << BigInt(index)]));
  const masks = requirements.map((requirement) => requirement == null ? null
    : requirement.reduce((mask, id) => mask | bitFor.get(id), 0n));
  const states = new Map([[0n, { selected: [], bytes: baseBytes }]]);
  for (const requirement of new Set(masks.filter((mask) => mask != null))) {
    for (const [mask, state] of [...states]) {
      const union = mask | requirement;
      if (states.has(union)) continue;
      const selected = ids.filter((id) => (union & bitFor.get(id)) !== 0n);
      if (selected.length > limits.rows) continue;
      const bytes = exactPublicBytes(baseBytes, selected, rowBytes);
      if (bytes <= limits.bytes) states.set(union, { selected, bytes });
    }
  }
  let best = { mask: 0n, covered: 0, selected: [], rows: 0, public_bytes: baseBytes };
  for (const [mask, state] of states) {
    const covered = masks.filter((required) => required != null && (required & mask) === required).length;
    const candidate = { mask, covered, selected: state.selected, rows: state.selected.length,
      public_bytes: state.bytes };
    if (candidate.covered > best.covered
      || (candidate.covered === best.covered && (candidate.rows < best.rows
        || (candidate.rows === best.rows && (candidate.public_bytes < best.public_bytes
          || (candidate.public_bytes === best.public_bytes && candidate.mask < best.mask)))))) best = candidate;
  }
  return { covered: best.covered, selected: best.selected, rows: best.rows,
    public_bytes: best.public_bytes, feasible_states: states.size };
}

export function evaluateAlternative(set, repositoryFragments, legalIds, baseBytes) {
  const legal = new Set(legalIds), rowBytes = new Map();
  repositoryFragments.forEach((fragment, index) => rowBytes.set(index, fragment.serialized_row_bytes));
  const atoms = set.required_source_atoms;
  const sourceRequirements = atoms.map(({ source_range }) => requiredFragments(source_range, repositoryFragments));
  const requirements = sourceRequirements.map((required) => required != null
    && required.every((index) => legal.has(repositoryFragments[index].fragment_id)) ? required : null);
  const optimum = maximizeCoveredAtoms(requirements, rowBytes, baseBytes);
  const reachable_atoms = atoms.filter((_, index) => {
    const required = requirements[index];
    if (required == null) return false;
    return required.length <= LIMITS.rows
      && exactPublicBytes(baseBytes, required, rowBytes) <= LIMITS.bytes;
  }).map(({ atom_id }) => atom_id);
  return { set_id: set.set_id, required_atoms: atoms.length, requirements, reachable_atoms, optimum,
    recall: atoms.length ? optimum.covered / atoms.length : 0,
    complete_source_set: optimum.covered === atoms.length };
}

export function evaluateArm(annotation, repositoryFragments, legalIds, baseBytes) {
  assert.ok(annotation.acceptable_sets.length > 0, "case has no acceptable source set");
  const alternatives = annotation.acceptable_sets.map((set) =>
    evaluateAlternative(set, repositoryFragments, legalIds, baseBytes));
  const best = alternatives.toSorted((left, right) => right.recall - left.recall
    || left.set_id.localeCompare(right.set_id))[0];
  const reachable_atoms = [...new Set(alternatives.flatMap((value) => value.reachable_atoms))].sort();
  return { best_set_id: best.set_id, recall: best.recall,
    complete_source_set: best.complete_source_set, rows: best.optimum.rows,
    public_bytes: best.optimum.public_bytes, selected_fragment_indexes: best.optimum.selected,
    reachable_atoms, alternatives };
}

export const mean = (values) => values.reduce((sum, value) => sum + value, 0) / values.length;

export function percentile(values, probability) {
  assert.ok(values.length > 0 && probability > 0 && probability <= 1, "invalid percentile input");
  const ordered = values.toSorted((left, right) => left - right);
  return ordered[Math.max(0, Math.ceil(probability * ordered.length) - 1)];
}
