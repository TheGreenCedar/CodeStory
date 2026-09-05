import test from 'node:test';
import assert from 'node:assert/strict';
import { frontier, greedyRows, validateRows } from '../lib/multivector-evidence.mjs';

test('frontier has deterministic ties, natural underfill and independent successors', () => {
  const ids = Array.from({ length: 160 }, (_, i) => String(i).padStart(3, '0'));
  const scores = ids.map(() => 1);
  const seeds = ids.slice(144);
  const pool = frontier(ids, scores, seeds);
  assert.equal(pool.length, 144);
  assert.deepEqual(pool.slice(16), ids.slice(0, 128));
  assert.deepEqual(frontier(['b', 'a'], [1, 1], ['b']), ['b', 'a']);
  assert.deepEqual(frontier(ids, scores, []), []);
  assert.deepEqual(greedyRows(pool, ids, scores, new Map(ids.map(id => [id, 500])), 274), ids.slice(0, 16));
});

test('exact row and metadata budgets; no compulsory seeds or oversize row substitution', () => {
  const ids = ['a', 'b', 'c'];
  assert.deepEqual(greedyRows(ids, ids, [3, 2, 1], new Map([['a', 17000], ['b', 8000], ['c', 8000]]), 384), ['b']);
  assert.deepEqual(greedyRows(['a', 'b'], ['a', 'b'], [2, 1], new Map([['a', 8000], ['b', 8000]]), 383), ['a', 'b']);
});

test('complete row validation refuses the whole malformed execution class', () => {
  const input = { repositories: [{ repository_id: 'r', fragment_ids: ['a', 'b', 'c'] }],
    wordings: [{ case_id: 'x', phrasing_id: 'p', repository_id: 'r', question_sha256: 'q', seed_fragment_ids: ['b'] }] };
  const row = { case_id: 'x', phrasing_id: 'p', repository_id: 'r', question_sha256: 'q',
    scores: [2, 3, 1], seeds: ['b'], legal: ['b', 'a', 'c'],
    timing: { whole_ms: 10, query_ms: 2, scoring_ms: 3, assembly_ms: 4, unaccounted_ms: 1 } };
  validateRows(input, [row]);
  for (const mutate of [r => r.scores.pop(), r => r.scores[0] = NaN,
    r => r.seeds.push('a'), r => r.legal.reverse(), r => r.legal.push('a'),
    r => r.question_sha256 = 'changed', r => r.timing.unaccounted_ms = -1,
    r => r.timing.whole_ms = 1]) {
    const changed = structuredClone(row); mutate(changed);
    assert.throws(() => validateRows(input, [changed]));
  }
  assert.throws(() => validateRows(input, []));
  assert.throws(() => validateRows(input, [row, row]));
});
