import assert from 'node:assert/strict';
import { exactPublicBytes } from './etr1-evidence.mjs';

function ordered(ids, scores) {
  assert.equal(new Set(ids).size, ids.length, 'duplicate scored identity');
  assert.equal(ids.length, scores.length, 'missing score');
  assert.ok(scores.every(Number.isFinite), 'nonfinite score');
  return ids.map((id, i) => ({ id, score: scores[i] })).sort((a, b) =>
    b.score - a.score || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

export function frontier(ids, scores, seeds) {
  assert.ok(seeds.length <= 16 && new Set(seeds).size === seeds.length, 'seed count or identity');
  const all = new Set(ids), excluded = new Set(seeds);
  assert.ok(seeds.every(id => all.has(id)), 'unknown seed');
  return [...seeds, ...ordered(ids, scores).filter(x => !excluded.has(x.id))
    .slice(0, seeds.length * 8).map(x => x.id)];
}

export function greedyRows(legal, ids, scores, rowBytes, baseBytes) {
  assert.equal(new Set(legal).size, legal.length);
  const available = new Set(legal), selected = [];
  for (const { id } of ordered(ids, scores)) {
    if (!available.has(id)) continue;
    assert.ok(Number.isSafeInteger(rowBytes.get(id)) && rowBytes.get(id) > 0);
    if (exactPublicBytes(baseBytes, [...selected, id], rowBytes) <= 16384) selected.push(id);
    if (selected.length === 16) break;
  }
  return selected;
}

export function validateRows(input, rows) {
  assert.equal(rows.length, input.wordings.length, 'incomplete run');
  for (const [i, row] of rows.entries()) {
    const wording = input.wordings[i];
    for (const key of ['case_id', 'phrasing_id', 'repository_id', 'question_sha256'])
      assert.equal(row[key], wording[key], `row ${i} ${key} changed`);
    const repo = input.repositories.find(r => r.repository_id === wording.repository_id);
    assert.ok(repo, 'unknown repository');
    assert.deepEqual(row.seeds, wording.seed_fragment_ids, 'seed manifest changed');
    assert.deepEqual(row.legal, frontier(repo.fragment_ids, row.scores, row.seeds), 'frontier changed');
    const t = row.timing;
    assert.ok(Object.values(t).every(x => Number.isFinite(x) && x >= 0), 'invalid interval');
    assert.ok(Math.abs(t.whole_ms - t.query_ms - t.scoring_ms - t.assembly_ms - t.unaccounted_ms) < 0.01,
      'timing intervals do not reconcile');
  }
}

export function validateExecution(result, end, vectorReceipt, documentCount, queryCount) {
  assert.equal(result.status, 'outputs_frozen');
  assert.equal(end.experiment_status, 'outputs_frozen');
  for (const key of ['preparation_ms', 'vector_serialization_ms', 'elapsed_before_result_serialization_ms'])
    assert.ok(Number.isFinite(result[key]) && result[key] >= 0, `invalid ${key}`);
  assert.ok(Number.isFinite(end.wall_ms) && end.wall_ms > 0);
  const accounted = result.preparation_ms + result.vector_serialization_ms
    + result.rows.reduce((sum, row) => sum + row.timing.whole_ms, 0);
  assert.ok(result.elapsed_before_result_serialization_ms + .01 >= accounted, 'inner intervals exceed enclosing wall');
  assert.ok(end.wall_ms >= result.elapsed_before_result_serialization_ms, 'inner wall exceeds process wall');
  assert.equal(vectorReceipt.status, 'validated');
  assert.equal(vectorReceipt.vectors_sha256, result.vectors_sha256);
  assert.equal(vectorReceipt.documents_reencoded, documentCount);
  assert.equal(vectorReceipt.queries_reencoded, queryCount);
  assert.ok(Number.isFinite(vectorReceipt.maximum_score_error) && vectorReceipt.maximum_score_error >= 0
    && vectorReceipt.maximum_score_error < 1e-4, 'invalid exact score verification');
  return { request_loop_unaccounted_ms: result.elapsed_before_result_serialization_ms - accounted,
    process_overhead_and_result_serialization_ms: end.wall_ms - result.elapsed_before_result_serialization_ms };
}
