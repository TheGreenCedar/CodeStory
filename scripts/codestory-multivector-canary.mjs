#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { sha256, fragmentId } from './lib/etr1-evidence.mjs';
import { sourcePacket } from './lib/packet-reader-evidence.mjs';

const [directory, assets, python, contract] = process.argv.slice(2);
assert.ok(directory && assets && python && contract);
fs.mkdirSync(directory, { mode: 0o700 });
const publish = (file, value) => fs.writeFileSync(file, JSON.stringify(value) + '\n', { flag: 'wx' });
function command(executable, args, cwd = directory) {
  const result = spawnSync(executable, args, { cwd, encoding: 'utf8', timeout: 5 * 60 * 1000 });
  assert.equal(result.status, 0, result.stderr); return result.stdout.trim();
}
const prep = { repositories: [], fragments: [], wordings: [] }, annotations = { cases: [] };
for (const [ordinal, count] of [40, 3].entries()) {
  const repoRoot = path.join(directory, `repo-${ordinal}`);
  fs.mkdirSync(repoRoot);
  const lines = Array.from({ length: count }, (_, i) => `def transform_${i}(value): return value + ${i}\n`);
  const bytes = Buffer.from(lines.join('')), file = 'sample.py', project = `synthetic-${ordinal}`;
  fs.writeFileSync(path.join(repoRoot, file), bytes, { flag: 'wx' });
  command('git', ['init', '-q'], repoRoot);
  command('git', ['add', 'sample.py'], repoRoot);
  command('git', ['-c', 'user.name=canary', '-c', 'user.email=canary@invalid', 'commit', '-qm', 'synthetic fixture'], repoRoot);
  const publication = { project_id: project, core_generation_id: 'synthetic-core', retrieval_generation: 'synthetic-retrieval' };
  const sources = new Map([[file, bytes]]), fragments = [];
  let offset = 0;
  for (const [index, source] of lines.entries()) {
    const f = { project_id: project, path: file, content_digest: sha256(bytes),
      byte_range: { start: offset, end: offset + Buffer.byteLength(source) },
      line_range: { start: index + 1, end: index + 1 }, source };
    f.fragment_id = fragmentId(f);
    f.serialized_row_bytes = Buffer.byteLength(JSON.stringify(sourcePacket([f], publication, sources).support[0]));
    fragments.push(f); offset = f.byte_range.end;
  }
  prep.fragments.push(...fragments);
  prep.repositories.push({ repository_id: project, project_id: project,
    local_root: repoRoot, commit: command('git', ['rev-parse', 'HEAD'], repoRoot), publication,
    fragment_ids: fragments.map(f => f.fragment_id),
    base_serialized_bytes: Buffer.byteLength(JSON.stringify(sourcePacket([], publication, sources))) });
  const question = 'Which function adds two to its argument?';
  prep.wordings.push({ case_id: project, phrasing_id: 'original', repository_id: project,
    group: 'synthetic', question, question_sha256: sha256(question),
    seed_fragment_ids: fragments.slice(0, ordinal ? 1 : 3).map(f => f.fragment_id) });
  const f = fragments[2];
  annotations.cases.push({ case_id: project, acceptable_sets: [{ set_id: 'one', required_source_atoms: [
    { atom_id: 'adds-two', source_range: { path: file, content_digest: f.content_digest,
      byte_range: f.byte_range, line_range: f.line_range } }], required_relation_atoms: [] }] });
}
const prepPath = path.join(directory, 'preparation.json'), run = path.join(directory, 'run');
publish(prepPath, prep);
const controller = path.join(path.dirname(fileURLToPath(import.meta.url)), 'codestory-multivector.mjs');
command(process.execPath, [controller, 'prepare', prepPath, run, assets, python, contract, '--synthetic']);
command(process.execPath, [controller, 'run', run]);
command(process.execPath, [controller, 'validate', run]);
// The real evaluator must refuse absent validation without opening an annotation file.
const validation = path.join(run, 'validation.json'), held = path.join(run, 'validation.held');
fs.renameSync(validation, held);
const refused = spawnSync(process.execPath, [controller, 'evaluate', run, '/annotation-access-forbidden'], { encoding: 'utf8' });
assert.notEqual(refused.status, 0);
assert.ok(refused.stderr.includes('validation.json') && !refused.stderr.includes('annotation-access-forbidden'));
fs.renameSync(held, validation);
const annotationPath = path.join(directory, 'annotations.json');
publish(annotationPath, annotations);
command(process.execPath, [controller, 'evaluate', run, annotationPath]);
const result = JSON.parse(fs.readFileSync(path.join(run, 'result.json')));
assert.equal(result.rows[0].legal.length, 27);
assert.equal(result.rows[1].legal.length, 3);
publish(path.join(directory, 'canary.json'), { status: 'passed', synthetic_only: true,
  validation_sha256: sha256(fs.readFileSync(validation)),
  evaluation_sha256: sha256(fs.readFileSync(path.join(run, 'evaluation.json'))),
  source: JSON.parse(fs.readFileSync(path.join(run, 'input.json'))).source });
console.log(JSON.stringify({ status: 'passed', directory }));
