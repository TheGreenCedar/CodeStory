import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import os from 'node:os';
import { createDefaultHost } from '../../lib/release-coordinator-github.mjs';
import { receiptDigest } from '../../../.github/scripts/release-freeze-barrier.mjs';
import { CONSTANT_SET, sha256, parseRecord } from '../../lib/release-coordinator-contract.mjs';
import { loadReleaseClaimGraph, releaseClaimGraphDigest, canonicalReleaseClaimValue } from '../../codestory-release-claims.mjs';
import { deriveReleaseCells } from '../../codestory-release-closeout.mjs';

export const ROOT = fileURLToPath(new URL('../../../', import.meta.url));
export const REPOSITORY = 'TheGreenCedar/CodeStory';
export const C = 'c'.repeat(40), D = 'd'.repeat(40), F = 'f'.repeat(40), M = '0'.repeat(40), P = 'e'.repeat(40);
export const TREES = { [C]: '1'.repeat(40), [D]: '3'.repeat(40), [F]: '2'.repeat(40), [M]: '9'.repeat(40), [P]: '2'.repeat(40) };
const python = process.env.CODESTORY_PYTHON || 'python3';

export function zipDocuments(documents) {
  return execFileSync(python, ['-c', 'import json,sys,zipfile,io;docs=json.load(sys.stdin);out=io.BytesIO();z=zipfile.ZipFile(out,"w",zipfile.ZIP_DEFLATED);[z.writestr(k,v) for k,v in docs.items()];z.close();sys.stdout.buffer.write(out.getvalue())'],
    { input: JSON.stringify(documents), maxBuffer: 64 * 1024 * 1024 });
}

export function githubFixture() {
  const graph = loadReleaseClaimGraph(ROOT);
  const json = value => `${JSON.stringify(canonicalReleaseClaimValue(value), null, 2)}\n`;
  const data = { next: C, main: M, localHead: C, comments: [], issues: [], runs: [],
    jobs: new Map(), artifacts: new Map(), containers: new Map(), statuses: new Map(), calls: [],
    clock: new Date('2026-09-09T04:00:00.000Z'), nextId: 121, artifactId: 1000,
    dirty: '', cancelHonored: true, heartbeat: true, constantsFrozen: false,
    pulls: [], branches: new Map(), worktreeHeads: new Map(), sourceFiles: new Map(), lineageChecks: 0 };
  for (const name of ['per-user-embedding-server-measurement-protocol.json', 'per-user-embedding-server-protocol.json', path.basename(CONSTANT_SET)]) {
    const file = `crates/codestory-llama-sys/${name}`;
    let bytes = readFileSync(path.join(ROOT, file), 'utf8');
    if (file === CONSTANT_SET) { const parsed = JSON.parse(bytes); parsed.status = 'unfrozen'; parsed.freeze_record = null; bytes = json(parsed); }
    data.sourceFiles.set(file, bytes);
  }
  const current = () => data.clock.toISOString();
  function job(run, name, steps = []) {
    return { id: 8000 + data.calls.length, name, status: 'completed', conclusion: 'success',
      run_id: run.id, run_attempt: run.run_attempt, head_sha: run.head_sha,
      completed_at: current(), labels: ['self-hosted', 'Windows', 'X64', 'codestory-vulkan', 'macOS', 'ARM64', 'codestory-metal'],
      steps: steps.map(name => ({ name, status: 'completed', conclusion: 'success', started_at: current(), completed_at: current() })) };
  }
  function artifact(run, name, documents) {
    const bytes = zipDocuments(documents);
    const item = { id: ++data.artifactId, name, expired: false, digest: `sha256:${sha256(bytes)}`,
      size_in_bytes: bytes.length, created_at: current(), workflow_run: { id: run.id, head_sha: run.head_sha } };
    data.containers.set(item.id, bytes);
    data.artifacts.set(run.id, [...(data.artifacts.get(run.id) ?? []), item]);
    return item;
  }
  function completeSource(id, phase = 'source_stabilization') {
    const run = data.runs.find(row => row.id === id);
    Object.assign(run, { status: 'completed', conclusion: 'success' });
    const frozen = phase === 'frozen_candidate';
    const actions = frozen ? ['frozen-candidate-acceptance', 'qualification', 'release']
      : ['source-stabilization', 'calibration', 'generated-constant-freeze', 'frozen-candidate-acceptance', 'qualification', 'release'];
    const receipt = { schema: 3, authority: 'github_actions', phase, repository: REPOSITORY,
      branch: 'dev/codestory-next', commit: run.head_sha, tree: TREES[run.head_sha], worktree_clean: true, remote_head: run.head_sha,
      release_pr: { number: 0, bind: 'next_head', base: 'dev/codestory-next', base_commit: run.head_sha,
        head: 'dev/codestory-next', head_commit: run.head_sha }, integrated_support_prs: [],
      known_future_source_changes: frozen ? [] : [CONSTANT_SET], planned_proof_actions: actions,
      proof_triggering_labels: [], proof_triggering_actions: actions, reusable_evidence: [], invalidated_evidence: [],
      running_workflows: [], cancelled_superseded_runs: [], next_permitted_mutation: frozen ? null : CONSTANT_SET,
      acceptance_run: { id: run.id, attempt: run.run_attempt, workflow: run.path, event: run.event } };
    receipt.digest = receiptDigest(receipt);
    data.jobs.set(run.id, [
      job(run, 'freeze-hostile-mutations', ['Execute exact-head hostile mutation matrix']),
      job(run, 'freeze-windows-native-probe', ['Run exact-head Windows native probe']),
      job(run, 'freeze-acceptance', ['Publish executable release freeze']),
      ...(!frozen ? ['full-source-gate', 'retrieval-generalization', 'windows-native-contracts'].map(name => job(run, name)) : []),
    ]);
    artifact(run, `release-freeze-receipt-attempt-${run.run_attempt}`, { 'release-freeze-receipt.json': JSON.stringify(receipt) });
    if (!frozen) artifact(run, `release-cell-prepublish-source-attempt-${run.run_attempt}`, { 'source.json': '{}' });
    data.statuses.set(run.head_sha, [{ id: 1, state: 'success', context: `codestory/release-freeze/${receipt.digest}`,
      description: `tree=${TREES[run.head_sha]}`, target_url: `https://github.com/${REPOSITORY}/actions/runs/${run.id}`,
      creator: { login: 'github-actions[bot]', type: 'Bot' } }]);
    return receipt;
  }
  function completeCalibration(id = 123) {
    const run = data.runs.find(row => row.id === id);
    Object.assign(run, { status: 'completed', conclusion: 'success' });
    const root = mkdtempSync(path.join(os.tmpdir(), 'codestory-calibration-test-'));
    try {
      const inputs = path.join(root, 'inputs'); mkdirSync(inputs);
      for (const [file, bytes] of data.sourceFiles) writeFileSync(path.join(inputs, path.basename(file)), bytes);
      const documents = JSON.parse(execFileSync(python, ['-c',
        'import sys,json;from pathlib import Path;sys.path.insert(0,sys.argv[1]+"/.github/scripts");from packaged_agent_proof.measurement_protocol import load_server_measurement_contract;from packaged_agent_proof.calibration_self_test import build_calibration_self_test_bundle;from packaged_agent_proof.contract_primitives import write_json,sha256;root=Path(sys.argv[2]);contract=load_server_measurement_contract(root/"inputs/per-user-embedding-server-measurement-protocol.json");source={"commit":sys.argv[3],"tree":sys.argv[4],"tracked_dirty":False};bundle_path,frozen,bundle=build_calibration_self_test_bundle(root,contract,source=source);frozen["constant_set"]["freeze_record"]["selected_at"]="github-actions-run:123:1";constant=root/"per-user-embedding-server-constant-set.json";write_json(constant,frozen["constant_set"]);manifest={"bundle":{"sha256":sha256(bundle_path)},"frozen_constant_set":{"sha256":sha256(constant)},"selection_source":source,"source_head_sha":source["commit"],"source_tree":source["tree"],"producer_run_id":"123","producer_run_attempt":"1","run_count":3,"matrix_cell_count":1};write_json(root/"manifest.json",manifest);print(json.dumps({name:(root/name).read_text() for name in ["calibration-bundle.json","per-user-embedding-server-constant-set.json","manifest.json"]}))',
        ROOT, root, C, TREES[C]], { encoding: 'utf8' }));
      data.calibrationDocuments = documents;
      data.frozenBytes = documents['per-user-embedding-server-constant-set.json'];
      data.jobs.set(run.id, [job(run, 'calibration-assemble')]);
      artifact(run, `embedding-calibration-bundle-${C}`, documents);
    } finally { rmSync(root, { recursive: true, force: true }); }
  }
  function completeQualification(id = 125) {
    const selected = data.runs.find(row => row.id === id);
    Object.assign(selected, { status: 'completed', conclusion: 'success' });
    const jobs = [job(selected, 'closeout')];
    for (const { asset_target: target } of graph.workflow_policy.package_matrix) {
      jobs.push(job(selected, `packaged-proof / Build ${target}`));
      artifact(selected, `codestory-candidate-archive-record-${target}`, { 'record.json': '{}' });
      artifact(selected, `codestory-qualification-driver-${target}`, { 'driver.json': '{}' });
    }
    for (const [target, label, backend, name] of [['macos-arm64', 'metal', 'MTL', 'Packaged Apple Silicon Metal engine'], ['windows-x64', 'vulkan', 'Vulkan', 'Packaged Windows Vulkan engine']]) {
      jobs.push(job(selected, `proof / ${name}`));
      artifact(selected, `${target}-${label}-proof-0.17.6-attempt-1`, { 'qualification.json': json({
        status: 'pass', tier: 'protected_hardware', source: { commit: F, tree: TREES[F], tracked_dirty: false },
        host: { target, backend, policy: 'accelerated', unplanned_suspend: false }, timing: { constants_frozen_before_run: true },
      }) });
    }
    data.jobs.set(selected.id, jobs);
  }
  function completeCloseout(id = 126, phase = 'pre_publish') {
    const selected = data.runs.find(row => row.id === id);
    Object.assign(selected, { status: 'completed', conclusion: 'success' });
    const head = selected.head_sha;
    const provenance = json({ schema: 'codestory.release-actions-provenance/v1', test_fixture: true });
    const cells = deriveReleaseCells(graph, phase).map(cell => ({ id: cell.id, status: 'pass',
      ...(cell.id.startsWith('package_identity:') ? { archive: { name: `codestory-cli-v0.17.6-${cell.id.split(':')[1]}.tar.gz`, bytes: 100, sha256: 'a'.repeat(64) } } : {}),
    }));
    const delivery = graph.workflow_policy.catalog_delivery.states[0];
    const shared = { phase, decision: 'accept', graph_sha256: releaseClaimGraphDigest(graph), version: '0.17.6',
      identity: { repository: REPOSITORY, commit: head, source_tree: TREES[head] }, input_errors: [],
      producer_provenance_sha256: sha256(provenance), withheld_claims: [],
      ...(phase === 'post_publish' ? { catalog_delivery: { state: delivery.id, installer: delivery.installer } } : {}),
    };
    const ledger = { ...shared, schema: graph.closeout.ledger_schema, cells };
    const summary = { ...shared, schema: graph.closeout.summary_schema, counts: { required: cells.length, passed: cells.length, failed: 0, missing: 0, withheld: 0 } };
    const suffix = phase === 'post_publish' ? '-attempt-1' : '';
    artifact(selected, `release-closeout-${phase.replace('_', '-')}-0.17.6-${head}${suffix}`, {
      [`${phase}/ledger.json`]: json(ledger), [`${phase}/summary.json`]: json(summary), [`${phase}/producer-provenance.json`]: provenance,
    });
    data.jobs.set(selected.id, [job(selected, `native-release / Authenticate ${phase.replace('_', '-')} release cells`)]);
    return ledger;
  }
  function runCommand(program, args, options = {}) {
    data.calls.push({ program, args, body: options.input ? JSON.parse(options.input) : undefined });
    if (program !== 'gh' && program !== 'git') {
      if (args[0].endsWith('read-release-artifact.py')) return execFileSync(program, args, { encoding: 'utf8', ...options });
      if (args[0] === '-c' && args[1].includes('calibration_verification')) return execFileSync(program, args, { encoding: 'utf8', ...options });
      if (args[0].endsWith('check-calibration-release-lineage.py')) {
        data.lineageChecks++;
        if (data.lineageFailure) throw new Error('canonical lineage owner rejected generated bytes');
        return '{"status":"passed"}';
      }
      throw new Error(`unhandled hermetic process ${program} ${args.join(' ')}`);
    }
    if (program === 'git') {
      const scope = args[1]; let command = args.slice(2);
      while (command[0] === '-c') command = command.slice(2);
      if (command[0] === 'rev-parse' && command[1] === 'HEAD') return data.worktreeHeads.get(scope) ?? data.localHead;
      if (command[0] === 'rev-parse' && command[1].endsWith('^{tree}')) return TREES[data.worktreeHeads.get(scope) ?? F];
      if (command[0] === 'status') return data.dirty;
      if (command[0] === 'ls-remote') { const ref = command.at(-1).replace('refs/heads/', ''); return data.branches.has(ref) ? `${data.branches.get(ref)}\trefs/heads/${ref}` : ''; }
      if (command[0] === 'fetch' || command[0] === 'cat-file') return '';
      if (command[0] === 'rev-list') return `${F} ${C}`;
      if (command[0] === 'diff') return data.wrongFreezePath ? 'README.md' : CONSTANT_SET;
      if (command[0] === 'show') return data.frozenBytes;
      if (command[0] === 'add') return '';
      if (command[0] === 'commit') { data.worktreeHeads.set(scope, F); return F; }
      if (command[0] === 'push') { data.branches.set(command.at(-1).split('refs/heads/')[1], F); return ''; }
      if (command[0] === 'worktree' && command[1] === 'add') {
        const checkout = command.at(-2); mkdirSync(path.join(checkout, path.dirname(CONSTANT_SET)), { recursive: true });
        data.worktreeHeads.set(checkout, command.at(-1)); return '';
      }
      if (command[0] === 'worktree' && command[1] === 'remove') { data.worktreeHeads.delete(command.at(-1)); return ''; }
      throw new Error(`unhandled hermetic git ${command.join(' ')}`);
    }
    if (args[0] === 'issue' && args[1] === 'list') {
      const raw = args[args.indexOf('--search') + 1];
      const search = raw.replaceAll('"', '').replace(/ in:(body|title)$/u, '');
      return JSON.stringify(data.issues.filter(issue => raw.endsWith('in:title') ? issue.title === search : issue.body.includes(search)));
    }
    if (args[0] === 'pr' && args[1] === 'checks') return JSON.stringify([{ name: 'focused contracts', bucket: data.checksFailed ? 'fail' : 'pass' }]);
    if (args[0] === 'pr' && args[1] === 'ready') { data.pulls.find(pr => pr.number === Number(args[2])).draft = false; return ''; }
    if (args[0] === 'pr' && args[1] === 'merge') {
      const pr = data.pulls.find(row => row.number === Number(args[2]));
      pr.merged = true; pr.state = 'closed'; pr.merge_commit_sha = P; data.main = P;
      if (data.deleteDevOnMerge) data.next = null;
      data.runs.push({ id: ++data.nextId, run_attempt: 1, status: 'queued', conclusion: null,
        path: '.github/workflows/auto-release.yml', head_sha: P, head_repository: { full_name: REPOSITORY },
        actor: { login: 'TheGreenCedar' }, event: 'push', created_at: current(), run_started_at: current() });
      return '';
    }
    if (args[0] === 'release' && args[1] === 'view') return JSON.stringify({ tagName: 'v0.17.6', isDraft: false, isPrerelease: false,
      publishedAt: current(), url: `https://github.com/${REPOSITORY}/releases/tag/v0.17.6`,
      assets: [...graph.workflow_policy.package_matrix.map(row => ({ name: `codestory-cli-v0.17.6-${row.asset_target}.${row.extension}` })), { name: 'SHA256SUMS.txt' }] });
    if (args[0] !== 'api') throw new Error(`unhandled hermetic gh ${args.join(' ')}`);
    const endpoint = args.find(value => value.startsWith('repos/')) ?? 'user';
    const method = args.includes('--method') ? args[args.indexOf('--method') + 1] : 'GET';
    const body = options.input ? JSON.parse(options.input) : null;
    const output = value => JSON.stringify(args.includes('--slurp') ? [value] : value);
    if (endpoint === 'user') return output({ login: 'TheGreenCedar' });
    if (endpoint.includes('/git/ref/heads/')) {
      const branch = decodeURIComponent(endpoint.split('/git/ref/heads/')[1]);
      if (branch !== 'main' && data.next === null) { const error = new Error('HTTP 404'); error.stderr = 'HTTP 404'; throw error; }
      return output({ object: { sha: branch === 'main' ? data.main : data.next } });
    }
    if (endpoint.includes('/git/ref/tags/')) return output({ object: { type: 'commit', sha: P } });
    if (endpoint.includes('/compare/')) return output({ status: 'ahead', merge_base_commit: { sha: data.main } });
    if (endpoint.includes('/git/refs')) { data.next = body.sha; return output({ object: { sha: body.sha } }); }
    if (endpoint.includes('/git/commits/')) {
      const sha = endpoint.split('/').at(-1);
      return output({ sha, tree: { sha: TREES[sha] }, parents: (sha === P ? [M, F] : sha === F ? [C] : [M]).map(sha => ({ sha })) });
    }
    if (endpoint.includes('/contents/')) {
      const file = endpoint.split('/contents/')[1].split('?')[0];
      let contents;
      if (file === 'crates/codestory-cli/Cargo.toml') contents = '[package]\nname="codestory-cli"\nversion="0.17.6"\n';
      else if (file === CONSTANT_SET && endpoint.endsWith(`?ref=${F}`) && data.frozenBytes) contents = data.frozenBytes;
      else if (file === CONSTANT_SET && data.constantsFrozen) contents = JSON.stringify({ status: 'frozen', freeze_record: {} });
      else if (data.sourceFiles.has(file)) contents = data.sourceFiles.get(file);
      else contents = readFileSync(path.join(ROOT, file), 'utf8');
      return output({ type: 'file', encoding: 'base64', content: Buffer.from(contents).toString('base64') });
    }
    if (endpoint.endsWith('/issues') && method === 'POST') {
      const issue = { number: 999 + data.issues.length, ...body }; data.issues.push(issue); return output(issue);
    }
    if (endpoint.endsWith('/pulls') && method === 'POST') {
      const pr = { number: 77 + data.pulls.length, state: 'open', merged: false, title: body.title, body: body.body, draft: body.draft,
        head: { sha: F, ref: body.head }, base: { ref: body.base, sha: body.base === 'main' ? M : C } };
      data.pulls.push(pr); return output(pr);
    }
    if (/\/pulls\/\d+$/u.test(endpoint)) return output(data.pulls.find(pr => pr.number === Number(endpoint.split('/').at(-1))));
    if (endpoint.includes('/pulls?')) {
      const query = new URL(`https://api.github.com/${endpoint}`).searchParams;
      return output(data.pulls.filter(pr => pr.base.ref === query.get('base') && pr.head.ref === query.get('head').split(':')[1]));
    }
    if (/\/issues\/\d+\/comments/u.test(endpoint)) {
      if (method === 'POST') { const comment = { id: data.comments.length + 1, ...body }; data.comments.push(comment); return output(comment); }
      return output(data.comments);
    }
    if (/\/issues\/comments\/\d+/u.test(endpoint)) {
      const comment = data.comments.find(row => row.id === Number(endpoint.split('/').at(-1)));
      Object.assign(comment, body); return output(comment);
    }
    if (endpoint.includes('/workflows/protected-runner-heartbeat.yml/runs')) return output({ workflow_runs: data.heartbeat ? [{ id: 900, run_attempt: 1, head_sha: C, head_repository: { full_name: REPOSITORY }, created_at: current() }] : [] });
    if (endpoint.endsWith('/dispatches')) {
      if (data.dispatchFailure) throw new Error('uncertain network response');
      const run = { id: ++data.nextId, run_attempt: 1, status: 'queued', conclusion: null,
        path: `.github/workflows/${endpoint.split('/').at(-2)}`, head_sha: body.inputs.expected_head_sha,
        head_repository: { full_name: REPOSITORY }, actor: { login: 'TheGreenCedar' }, event: 'workflow_dispatch',
        created_at: current(), run_started_at: current() };
      data.runs.push(run);
      return output({ workflow_run_id: run.id, run_url: `https://api.github.com/${endpoint}/${run.id}`, html_url: `https://github.com/${REPOSITORY}/actions/runs/${run.id}` });
    }
    if (endpoint.includes('/actions/runs?')) return output({ workflow_runs: data.runs.filter(run => run.head_sha === new URL(`https://api.github.com/${endpoint}`).searchParams.get('head_sha')) });
    const selected = /\/actions\/runs\/(\d+)/u.exec(endpoint);
    if (selected) {
      const id = Number(selected[1]);
      const run = data.runs.find(row => row.id === id);
      if (endpoint.includes('/jobs')) {
        if (id === 900) return output({ jobs: ['macos-arm64-metal', 'windows-x64-vulkan'].map(name => job({ id: 900, run_attempt: 1, head_sha: C }, `Heartbeat ${name}`)) });
        return output({ jobs: data.jobs.get(id) ?? [] });
      }
      if (endpoint.includes('/artifacts')) return output({ artifacts: data.artifacts.get(id) ?? [] });
      if (endpoint.endsWith('/cancel') || endpoint.endsWith('/force-cancel')) {
        if (data.cancelHonored || endpoint.endsWith('/force-cancel')) Object.assign(run, { status: 'completed', conclusion: 'cancelled' });
        return '';
      }
      return output(run);
    }
    if (endpoint.includes('/actions/artifacts/')) return data.containers.get(Number(endpoint.split('/').at(-2)));
    if (endpoint.includes('/statuses')) return output(data.statuses.get(endpoint.split('/commits/')[1].split('/')[0]) ?? []);
    throw new Error(`unhandled hermetic API ${method} ${endpoint}`);
  }
  return { data, artifact, completeSource, completeCalibration, completeQualification, completeCloseout, job,
    record: () => parseRecord(data.comments[0].body),
    host: () => createDefaultHost({ repository: REPOSITORY, projectRoot: ROOT, runCommand, now: () => data.clock }),
  };
}
