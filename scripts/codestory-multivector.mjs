#!/usr/bin/env node
/** Development-only controller. Preparation and execution never read annotations. */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { sha256, evaluateArm, mean, percentile } from './lib/etr1-evidence.mjs';
import { sourcePacket } from './lib/packet-reader-evidence.mjs';
import { greedyRows, validateRows, validateExecution } from './lib/multivector-evidence.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const PREPARATION = '30b84d4d848f96bd4fe799f2e0f28b9114971da0e47bf98ebe54fe36242199fd';
const ANNOTATIONS = '52b0cc223292bc70f1e4fa3f52b67bf42a91e4d4b9ed997aa12c648c068e9ade';
const REVISION = '4bcdf5ed93f791259eb130b577a240f753d68dd8';
const ASSETS = '56981561a46f8e8e37d74269703186447d065ba87178197c03f5c2290b8b643b';
const CONTRACT = '8d237ba300f9c9970f834c3bb6eb1866248e48f4d655259c139a3a4b37826c3e';
const read = file => JSON.parse(fs.readFileSync(file));
const hash = file => sha256(fs.readFileSync(file));
const publish = (file, value) => fs.writeFileSync(file, JSON.stringify(value) + '\n', { flag: 'wx', mode: 0o600 });
const git = (cwd, ...args) => {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr); return result.stdout.trim();
};
function identity() {
  assert.equal(git(root, 'status', '--porcelain'), '', 'runner source must be clean');
  return { commit: git(root, 'rev-parse', 'HEAD'), tree: git(root, 'rev-parse', 'HEAD^{tree}'),
    files: Object.fromEntries(['scripts/codestory-multivector.mjs', 'scripts/codestory-multivector.py',
      'scripts/lib/multivector-evidence.mjs', 'scripts/lib/etr1-evidence.mjs',
      'scripts/lib/packet-reader-evidence.mjs'].map(file => [file, hash(path.join(root, file))])) };
}
function authenticate(data) {
  assert.equal(data.contract_sha256, CONTRACT);
  assert.equal(hash(data.asset_path), ASSETS);
  assert.deepEqual(read(data.asset_path), data.assets);
  if (data.authority !== 'synthetic_canary_only') {
    assert.equal(data.authority, 'burned_development_only');
    assert.equal(hash(data.preparation_path), PREPARATION);
    const retained = read(data.preparation_path);
    assert.deepEqual(data.repositories, retained.repositories);
    assert.deepEqual(data.fragments, retained.fragments);
    assert.deepEqual(data.wordings, retained.wordings.map(w => Object.fromEntries(['case_id', 'phrasing_id', 'repository_id',
      'group', 'question', 'question_sha256', 'seed_fragment_ids'].map(k => [k, w[k]]))));
  }
  assert.equal(new Set(data.fragments.map(f => f.fragment_id)).size, data.fragments.length);
  const sourceMaps = new Map();
  for (const repo of data.repositories) {
    assert.equal(git(repo.local_root, 'rev-parse', 'HEAD'), repo.commit, 'repository commit drift');
    const repoRoot = fs.realpathSync(repo.local_root), sources = new Map();
    const fragments = data.fragments.filter(f => f.project_id === repo.project_id);
    assert.deepEqual([...repo.fragment_ids].sort(), fragments.map(f => f.fragment_id).sort());
    for (const f of fragments) {
      assert.ok(!path.isAbsolute(f.path) && !f.path.includes('\\') && !f.path.includes('\0')
        && f.path.split('/').every(p => p && p !== '.' && p !== '..'));
      const target = fs.realpathSync(path.join(repoRoot, f.path));
      assert.ok(target.startsWith(repoRoot + path.sep), 'source escapes repository');
      if (!sources.has(f.path)) sources.set(f.path, fs.readFileSync(target));
      sourcePacket([f], repo.publication, sources);
    }
    assert.equal(Buffer.byteLength(JSON.stringify(sourcePacket([], repo.publication, sources))), repo.base_serialized_bytes);
    sourceMaps.set(repo.repository_id, sources);
  }
  assert.equal(new Set(data.wordings.map(w => `${w.case_id}/${w.phrasing_id}`)).size, data.wordings.length);
  for (const w of data.wordings) {
    assert.equal(sha256(w.question), w.question_sha256);
    const r = data.repositories.find(r => r.repository_id === w.repository_id);
    assert.ok(r && w.seed_fragment_ids.length <= 16);
    assert.equal(new Set(w.seed_fragment_ids).size, w.seed_fragment_ids.length);
    assert.ok(w.seed_fragment_ids.every(id => r.fragment_ids.includes(id)));
  }
  return sourceMaps;
}

function prepare(preparationPath, directory, assetPath, python, contract, canaryPath) {
  const synthetic = canaryPath === '--synthetic';
  const preparation = read(preparationPath), preparationHash = hash(preparationPath);
  if (!synthetic) { assert.equal(preparationHash, PREPARATION); assert.equal(preparation.wordings.length, 72); }
  const assets = read(assetPath);
  assert.equal(hash(assetPath), ASSETS); assert.equal(hash(contract), CONTRACT);
  assert.equal(assets.revision, REVISION);
  const modelRoot = path.join(path.dirname(assetPath), 'model');
  for (const e of assets.entries) assert.equal(hash(path.join(modelRoot, e.path)), e.sha256);
  const environment = { PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR ?? '/tmp',
    LANG: 'en_US.UTF-8', PYTHONNOUSERSITE: '1', PYTHONDONTWRITEBYTECODE: '1' };
  const packageProbe = spawnSync(python, ['-c', 'import json,importlib.metadata,sysconfig;print(json.dumps({"packages":{d.metadata["Name"]:d.version for d in importlib.metadata.distributions()},"site":sysconfig.get_paths()["purelib"]}))'],
    { encoding: 'utf8', env: environment });
  assert.equal(packageProbe.status, 0, packageProbe.stderr);
  const { packages, site } = JSON.parse(packageProbe.stdout), moduleCode = [];
  function inventory(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      const target = path.join(directory, entry.name);
      if (entry.isDirectory()) inventory(target);
      else if (entry.isFile() && /\.(?:py|so|dylib|pth)$/.test(entry.name)) moduleCode.push({ path: target, sha256: hash(target) });
    }
  }
  inventory(site);
  for (const [name, version] of Object.entries({ pylate: '1.3.4', transformers: '4.56.2', torch: '2.8.0' }))
    assert.equal(Object.entries(packages).find(([key]) => key.toLowerCase() === name)?.[1], version);
  let canary = null;
  if (!synthetic) {
    assert.ok(canaryPath, 'exact-source canary receipt required');
    const receipt = read(canaryPath);
    assert.equal(receipt.status, 'passed'); assert.equal(receipt.synthetic_only, true);
    assert.deepEqual(receipt.source, identity());
    assert.deepEqual(receipt.packages, packages);
    assert.equal(receipt.module_code_sha256, sha256(JSON.stringify(moduleCode)));
    assert.equal(receipt.assets_sha256, hash(assetPath));
    for (const [file, field] of [['run/validation.json', 'validation_sha256'], ['run/evaluation.json', 'evaluation_sha256']])
      assert.equal(hash(path.join(path.dirname(canaryPath), file)), receipt[field]);
    canary = { path: path.resolve(canaryPath), sha256: hash(canaryPath) };
  }
  const data = { authority: synthetic ? 'synthetic_canary_only' : 'burned_development_only',
    preparation_path: path.resolve(preparationPath), asset_path: path.resolve(assetPath),
    preparation_sha256: preparationHash, contract_sha256: hash(contract), source: identity(),
    environment, packages, module_code: moduleCode, canary,
    assets, model_root: modelRoot, python: path.resolve(python), python_sha256: hash(python),
    repositories: preparation.repositories, fragments: preparation.fragments,
    wordings: preparation.wordings.map(w => Object.fromEntries(['case_id', 'phrasing_id', 'repository_id',
      'group', 'question', 'question_sha256', 'seed_fragment_ids'].map(k => [k, w[k]]))) };
  authenticate(data);
  fs.mkdirSync(directory, { mode: 0o700 });
  publish(path.join(directory, 'input.json'), data);
}

function run(directory) {
  const inputPath = path.join(directory, 'input.json'), data = read(inputPath);
  assert.deepEqual(identity(), data.source);
  assert.equal(hash(data.python), data.python_sha256);
  for (const entry of data.module_code) assert.equal(hash(entry.path), entry.sha256);
  authenticate(data);
  if (data.authority !== 'synthetic_canary_only') {
    assert.ok(data.canary); assert.equal(hash(data.canary.path), data.canary.sha256);
    const canary = read(data.canary.path);
    assert.equal(canary.status, 'passed'); assert.deepEqual(canary.source, data.source);
    assert.equal(canary.module_code_sha256, sha256(JSON.stringify(data.module_code)));
  }
  const argv = [path.join(root, 'scripts/codestory-multivector.py'), 'run', inputPath];
  const envelope = { input_sha256: hash(inputPath), source: identity(), executable: data.python,
    executable_sha256: hash(data.python), cwd: directory, environment: data.environment,
    module_code_sha256: sha256(JSON.stringify(data.module_code)), argv, started_at: new Date().toISOString() };
  publish(path.join(directory, 'execution-start.json'), envelope);
  const stdout = fs.openSync(path.join(directory, 'stdout.jsonl'), 'wx', 0o600);
  const stderr = fs.openSync(path.join(directory, 'stderr.log'), 'wx', 0o600);
  const start = performance.now();
  const result = spawnSync(data.python, argv, { cwd: directory, env: data.environment, stdio: ['ignore', stdout, stderr],
    timeout: 25 * 60 * 1000, killSignal: 'SIGKILL' });
  fs.closeSync(stdout); fs.closeSync(stderr);
  const completed = { start_sha256: hash(path.join(directory, 'execution-start.json')),
    exit_status: result.status, signal: result.signal, error: result.error?.message ?? null,
    wall_ms: performance.now() - start, stdout_sha256: hash(path.join(directory, 'stdout.jsonl')),
    stderr_sha256: hash(path.join(directory, 'stderr.log')),
    result_sha256: fs.existsSync(path.join(directory, 'result.json')) ? hash(path.join(directory, 'result.json')) : null,
    experiment_status: result.status === 0 ? 'outputs_frozen' : 'invalid', decision: 'not_evaluated' };
  publish(path.join(directory, 'execution-end.json'), completed);
  assert.equal(result.status, 0, 'execution invalid; inspect stderr, no quality analysis permitted');
}

function validate(directory) {
  const inputPath = path.join(directory, 'input.json'), data = read(inputPath);
  assert.deepEqual(identity(), data.source);
  const sources = authenticate(data);
  const start = read(path.join(directory, 'execution-start.json')), end = read(path.join(directory, 'execution-end.json'));
  assert.equal(start.input_sha256, hash(inputPath)); assert.deepEqual(start.source, data.source);
  assert.equal(start.executable, data.python); assert.equal(hash(data.python), start.executable_sha256);
  assert.equal(start.cwd, directory); assert.deepEqual(start.environment, data.environment);
  assert.equal(start.module_code_sha256, sha256(JSON.stringify(data.module_code)));
  for (const entry of data.module_code) assert.equal(hash(entry.path), entry.sha256);
  assert.deepEqual(start.argv, [path.join(root, 'scripts/codestory-multivector.py'), 'run', inputPath]);
  assert.equal(end.start_sha256, hash(path.join(directory, 'execution-start.json')));
  assert.equal(end.exit_status, 0); assert.equal(end.signal, null); assert.equal(end.error, null);
  for (const [file, field] of [['stdout.jsonl', 'stdout_sha256'], ['stderr.log', 'stderr_sha256'], ['result.json', 'result_sha256']])
    assert.equal(hash(path.join(directory, file)), end[field]);
  const result = read(path.join(directory, 'result.json'));
  assert.equal(result.input_sha256, hash(inputPath));
  assert.equal(result.vectors_sha256, hash(path.join(directory, 'vectors.npz')));
  assert.equal(result.parameter_device, 'mps:0'); assert.equal(result.fallback, false);
  assert.deepEqual(result.packages, data.packages);
  if (data.authority !== 'synthetic_canary_only') {
    assert.ok(data.canary); assert.equal(hash(data.canary.path), data.canary.sha256);
    assert.deepEqual(read(data.canary.path).source, data.source);
  }
  validateRows(data, result.rows);
  const fragments = new Map(data.fragments.map(f => [f.fragment_id, f]));
  for (const row of result.rows) {
    assert.equal(row.source_bytes, row.legal.reduce((n, id) => n + Buffer.byteLength(fragments.get(id).source), 0));
    const repo = data.repositories.find(r => r.repository_id === row.repository_id);
    row.baseline = greedyRows(row.legal, repo.fragment_ids, row.scores,
      new Map(data.fragments.map(f => [f.fragment_id, f.serialized_row_bytes])), repo.base_serialized_bytes);
    sourcePacket(row.baseline.map(id => fragments.get(id)), repo.publication, sources.get(row.repository_id));
  }
  const check = spawnSync(data.python, [path.join(root, 'scripts/codestory-multivector.py'), 'verify', inputPath],
    { cwd: directory, env: data.environment, encoding: 'utf8', timeout: 15 * 60 * 1000, maxBuffer: 4 * 1024 * 1024 });
  assert.equal(check.status, 0, check.stderr);
  const vectorReceipt = read(path.join(directory, 'vector-validation.json'));
  assert.equal(vectorReceipt.result_sha256, end.result_sha256);
  assert.equal(vectorReceipt.input_sha256, hash(inputPath));
  const timing = validateExecution(result, end, vectorReceipt, data.fragments.length, data.wordings.length);
  publish(path.join(directory, 'validation.json'), { status: 'validated', authority: data.authority,
    input_sha256: hash(inputPath), result_sha256: end.result_sha256,
    execution_sha256: hash(path.join(directory, 'execution-end.json')),
    vector_validation_sha256: hash(path.join(directory, 'vector-validation.json')),
    source: identity(), timing, baselines: result.rows.map(r => r.baseline) });
}

function evaluate(directory, annotationPath) {
  // Authenticate frozen validation BEFORE opening the annotation file.
  const validation = read(path.join(directory, 'validation.json'));
  assert.equal(validation.status, 'validated'); assert.deepEqual(validation.source, identity());
  for (const [file, key] of [['input.json', 'input_sha256'], ['result.json', 'result_sha256'],
    ['execution-end.json', 'execution_sha256'], ['vector-validation.json', 'vector_validation_sha256']])
    assert.equal(hash(path.join(directory, file)), validation[key]);
  const input = read(path.join(directory, 'input.json')), result = read(path.join(directory, 'result.json'));
  if (input.authority !== 'synthetic_canary_only') assert.equal(hash(annotationPath), ANNOTATIONS);
  const annotations = read(annotationPath);
  const rows = result.rows.map((row, index) => {
    const wording = input.wordings[index], repo = input.repositories.find(r => r.repository_id === row.repository_id);
    const annotation = annotations.cases.find(c => c.case_id === row.case_id);
    assert.ok(annotation, 'missing annotation');
    const fragments = repo.fragment_ids.map(id => input.fragments.find(f => f.fragment_id === id));
    return { case_id: row.case_id, phrasing_id: row.phrasing_id, group: wording.group,
      frontier: evaluateArm(annotation, fragments, row.legal, repo.base_serialized_bytes),
      baseline: evaluateArm(annotation, fragments, validation.baselines[index], repo.base_serialized_bytes) };
  });
  const cases = [...new Set(rows.map(r => r.case_id))].map(id => {
    const selected = rows.filter(r => r.case_id === id);
    if (input.authority !== 'synthetic_canary_only') assert.equal(selected.length, 3);
    return { case_id: id, group: selected[0].group, ...Object.fromEntries(['frontier', 'baseline'].map(arm =>
      [arm, { recall: mean(selected.map(r => r[arm].recall)), complete: mean(selected.map(r => +r[arm].complete_source_set)) }])) };
  });
  const summary = Object.fromEntries(['frontier', 'baseline'].map(arm => [arm, {
    recall: mean(cases.map(c => c[arm].recall)), complete: mean(cases.map(c => c[arm].complete)),
    groups: Object.fromEntries([...new Set(cases.map(c => c.group))].map(group => [group,
      mean(cases.filter(c => c.group === group).map(c => c[arm].recall))])) }]));
  const quality = summary.frontier.recall >= .85 && summary.frontier.complete >= .75
    && Object.values(summary.frontier.groups).every(value => value >= .70);
  const p95 = percentile(result.rows.map(r => r.timing.whole_ms), .95);
  publish(path.join(directory, 'evaluation.json'), { authority: input.authority, validation_sha256: hash(path.join(directory, 'validation.json')),
    annotations_sha256: hash(annotationPath), summary, cases, rows, prepared_request_p95_ms: p95,
    quality_pass: quality, latency_pass: p95 <= 1250,
    decision: quality && p95 <= 1250 ? 'frontier_only_qualified' : 'mechanism_failed', working_packet: false });
  console.log(JSON.stringify({ summary, p95_ms: p95, quality_pass: quality, working_packet: false }));
}

const [command, ...args] = process.argv.slice(2);
try {
  if (command === 'prepare') prepare(...args);
  else if (command === 'run') run(...args);
  else if (command === 'validate') validate(...args);
  else if (command === 'evaluate') evaluate(...args);
  else if (command === 'cancel') publish(path.join(args[0], 'cancel.json'), { input_sha256: hash(path.join(args[0], 'input.json')) });
  else throw new Error('expected prepare <preparation> <new-directory> <assets> <python> <contract> [--synthetic], run/validate <directory>, or evaluate <directory> <annotations>');
} catch (error) {
  console.error(JSON.stringify({ experiment_status: 'invalid', decision: 'not_evaluated', error: error.message }));
  process.exitCode = 1;
}
