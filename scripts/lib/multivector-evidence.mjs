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
