import assert from 'node:assert/strict';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execute, createDefaultHost } from '../codestory-release.mjs';
import { MARKER, CONSTANT_SET, recordBody } from '../lib/release-coordinator-contract.mjs';
import { githubFixture, ROOT, C, D, F, M, P } from './fixtures/release-github.mjs';

async function started() {
  const f = githubFixture();
  assert.equal((await execute(['start', '--version', '0.17.6'], f.host())).blocker, null);
  return f;
}
const run = (f, command = 'advance', ...extra) => execute([command, '--issue', '999', ...extra], f.host());
const dispatches = f => f.data.calls.filter(call => call.args.some(arg => arg.endsWith('/dispatches')));

test('the live adapter persists the real dispatch identity and resumes without duplication', async () => {
  const f = await started();
  const first = await run(f);
  assert.equal(first.blocker, null);
  assert.equal(first.dispatched.id, 122);
  assert.equal(f.record().dispatches[0].id, 122);
  const body = dispatches(f)[0].body;
  assert.equal(body.return_run_details, true);
  assert.equal(body.inputs.acceptance_phase, 'source_stabilization');
  assert.equal(body.ref, 'dev/codestory-next');
  assert.equal((await run(f, 'resume')).active_runs[0].id, 122);
  assert.equal(dispatches(f).length, 1);
});

test('authenticated source proof advances to calibration with its actual digest', async () => {
  const f = await started(); await run(f);
  const receipt = f.completeSource(122);
  const advanced = await run(f, 'resume');
  assert.equal(advanced.blocker, null);
  assert.equal(advanced.phase, 'calibration');
  assert.equal(advanced.dispatched.id, 123);
  const evidence = f.record().receipt.groups['source-stabilization'].value[0];
  assert.equal(evidence.run_id, 122);
  assert.equal(evidence.attempt, 1);
  assert.match(evidence.digest, /^sha256:[a-f0-9]{64}$/u);
  const body = dispatches(f)[1].body;
  assert.equal(body.inputs.freeze_receipt_digest, receipt.digest);
  assert.equal(body.inputs.mode, 'calibration');
  assert.equal(body.inputs.version, undefined);
  assert.equal(body.inputs.scope, 'none');
  await run(f, 'resume');
  assert.equal(dispatches(f).length, 2);
});

test('wrong-head success never becomes current-head evidence', async () => {
  const f = await started(); await run(f); f.completeSource(122);
  f.data.runs[0].head_sha = D;
  assert.match((await run(f, 'resume')).blocker, /does not match/u);
  assert.equal(f.record().receipt.groups['source-stabilization'], undefined);
  assert.equal(dispatches(f).length, 1);
});

test('drift cancels only owned work and waits before new-head dispatch', async () => {
  const f = await started(); await run(f);
  f.data.runs.push({ ...f.data.runs[0], id: 50, head_sha: M });
  f.data.next = D; f.data.localHead = D;
  assert.match((await run(f, 'resume')).blocker, /obsolete owned proof/u);
  assert.equal(f.data.runs.find(row => row.id === 122).conclusion, 'cancelled');
  assert.equal(f.data.runs.find(row => row.id === 50).status, 'queued');
  assert.equal(dispatches(f).length, 1);
  const second = await run(f, 'resume');
  assert.equal(second.blocker, null);
  assert.equal(second.sha, D);
  assert.equal(dispatches(f)[1].body.inputs.expected_head_sha, D);
});

test('ignored cancellation receives one bounded force request', async () => {
  const f = await started(); await run(f);
  f.data.cancelHonored = false; f.data.next = D; f.data.localHead = D;
  await run(f, 'resume'); await run(f, 'resume');
  const forced = () => f.data.calls.filter(call => call.args.some(arg => arg.endsWith('/force-cancel'))).length;
  assert.equal(forced(), 0);
  f.data.clock = new Date(f.data.clock.getTime() + 31_000);
  await run(f, 'resume');
  assert.equal(forced(), 1);
  assert.equal(dispatches(f).length, 1);
  await run(f, 'resume');
  assert.equal(dispatches(f).length, 2);
});

test('an uncertain dispatch reply retains intent and never silently retries', async () => {
  const f = await started(); f.data.dispatchFailure = true;
  assert.match((await run(f)).blocker, /uncertain network/u);
  assert.equal(f.record().dispatches.length, 1);
  f.data.dispatchFailure = false;
  assert.match((await run(f, 'resume')).blocker, /unconfirmed.*0 matching runs/u);
  assert.equal(dispatches(f).length, 1);
});

test('an existing broad run blocks dispatch without acquiring cancellation ownership', async () => {
  const f = await started();
  f.data.runs.push({ id: 50, head_sha: C, path: '.github/workflows/source-proof.yml',
    head_repository: { full_name: 'TheGreenCedar/CodeStory' }, status: 'queued' });
  assert.match((await run(f)).blocker, /broad proof is already active/u);
  assert.equal(dispatches(f).length, 0);
  assert.equal(f.record().dispatches.length, 0);
});

test('lost dispatch response resumes only the persisted nonce and leaves an unrelated run alone', async () => {
  const f = await started(); f.data.dispatchReplyLost = true;
  assert.match((await run(f)).blocker, /uncertain network/u);
  const owned = f.data.runs[0];
  f.data.runs.push({ ...owned, id: 50, display_title: 'unrelated source proof' });
  f.data.dispatchReplyLost = false;
  assert.equal((await run(f, 'resume')).active_runs[0].id, owned.id);
  assert.equal(dispatches(f).length, 1);
  f.data.next = D; f.data.localHead = D;
  await run(f, 'resume');
  assert.equal(f.data.runs.find(row => row.id === 50).status, 'queued');
  assert.equal(owned.conclusion, 'cancelled');
});

test('unrelated marked comments cannot replace or block the canonical coordinator', async t => {
  for (const [name, body] of [
    ['parseable', recordBody({ schema: 'codestory.release-coordinator/v1', issue_number: 999, phase: 'forged' })],
    ['malformed', `${MARKER}\n\`\`\`json\n{\n\`\`\`\n`],
  ]) await t.test(name, async () => {
    const f = await started();
    f.data.comments.push({ id: 77, user: { login: 'unrelated-user' }, body });
    const state = await run(f, 'status');
    assert.equal(state.blocker, null);
    assert.equal(f.record().phase, 'source_stabilization');
    assert.equal(f.record().durable_comment.id, 1);
    assert.equal(f.data.comments[1].body, body);
  });
});

test('legacy canonical comment gains an authenticated pin while authorized ambiguity fails closed', async () => {
  const f = await started();
  const legacy = f.record(); delete legacy.durable_comment;
  f.data.comments[0].body = recordBody(legacy);
  assert.equal((await run(f, 'status')).blocker, null);
  assert.deepEqual(f.record().durable_comment, { id: 1, author: 'TheGreenCedar' });
  f.data.comments.push({ id: 78, user: { login: 'TheGreenCedar' }, body: recordBody(f.record()) });
  await assert.rejects(run(f, 'status'), /ambiguous authorized comments/u);
});

test('a pinned canonical comment cannot be silently replaced or moved', async t => {
  for (const kind of ['missing', 'different id', 'different author']) await t.test(kind, async () => {
    const f = await started();
    if (kind === 'missing') f.data.comments.splice(0, 1);
    if (kind === 'different id') f.data.comments[0].id = 80;
    if (kind === 'different author') f.data.comments[0].user.login = 'unrelated-user';
    await assert.rejects(run(f, 'status'));
    assert.equal(f.data.comments.length <= 1, true);
  });
});

test('malformed canonical comment fails closed without adopting an unrelated marked record', async () => {
  const f = await started();
  const canonical = f.record();
  f.data.comments[0].body = `${MARKER}\n\`\`\`json\n{\n\`\`\`\n`;
  f.data.comments.push({ id: 79, user: { login: 'unrelated-user' }, body: recordBody(canonical) });
  await assert.rejects(run(f, 'status'));
});

test('unique authenticated dispatch nonce survives an ahead local clock', async () => {
  const f = await started(); f.data.dispatchReplyLost = true;
  assert.match((await run(f)).blocker, /uncertain network/u);
  const owned = f.data.runs[0];
  owned.created_at = new Date(Date.parse(owned.created_at) - 120_000).toISOString();
  f.data.dispatchReplyLost = false;
  const resumed = await run(f, 'resume');
  assert.equal(resumed.blocker, null);
  assert.equal(resumed.active_runs[0].id, owned.id);
  assert.equal(f.record().dispatches[0].id, owned.id);
  assert.equal(dispatches(f).length, 1);
});

test('an exact dispatch nonce from another actor cannot be adopted across clock skew', async () => {
  const f = await started(); f.data.dispatchReplyLost = true;
  await run(f);
  const candidate = f.data.runs[0];
  candidate.created_at = new Date(Date.parse(candidate.created_at) - 120_000).toISOString();
  candidate.actor.login = 'unrelated-user';
  assert.match((await run(f, 'resume')).blocker, /unconfirmed.*0 matching runs/u);
  assert.equal(f.record().dispatches[0].id, undefined);
});

test('recovery persists no run ID until detailed ownership checks succeed', async t => {
  for (const [name, mutate, blocker] of [
    ['actor', row => { row.actor = { login: 'unrelated-user' }; }, /run actor differs/u],
    ['head', row => { row.head_sha = D; }, /run does not match/u],
    ['workflow', row => { row.path = '.github/workflows/release.yml'; }, /run does not match/u],
    ['repository', row => { row.head_repository = { full_name: 'someone/else' }; }, /run does not match/u],
    ['event', row => { row.event = 'push'; }, /run does not match/u],
  ]) await t.test(name, async () => {
    const f = await started(); f.data.dispatchReplyLost = true;
    await run(f);
    f.data.dispatchReplyLost = false;
    const detailedHost = f.host();
    const getRun = detailedHost.getRun;
    detailedHost.getRun = id => {
      const row = structuredClone(getRun(id));
      mutate(row);
      return row;
    };
    assert.match((await execute(['resume', '--issue', '999'], detailedHost)).blocker, blocker);
    assert.equal(f.record().dispatches[0].id, undefined);
    assert.equal(dispatches(f).length, 1);
    assert.equal((await run(f, 'resume')).active_runs[0].id, f.data.runs[0].id);
    assert.equal(dispatches(f).length, 1);
  });
});

test('wrong or duplicate dispatch nonces cannot be adopted as owned work', async t => {
  for (const kind of ['missing', 'different', 'duplicate']) await t.test(kind, async () => {
    const f = await started(); f.data.dispatchReplyLost = true; await run(f);
    if (kind === 'missing') delete f.data.runs[0].display_title;
    if (kind === 'different') f.data.runs[0].display_title = 'codestory-release:another';
    if (kind === 'duplicate') f.data.runs.push({ ...f.data.runs[0], id: 50 });
    f.data.next = D; f.data.localHead = D;
    assert.match((await run(f, 'resume')).blocker, /unconfirmed/u);
    assert.ok(f.data.runs.every(row => row.status === 'queued'));
    assert.equal(f.record().dispatches[0].id, undefined);
  });
});

test('a failed newer attempt cannot reuse earlier successful evidence', async () => {
  const f = await started(); await run(f); f.completeSource(122);
  Object.assign(f.data.runs[0], { run_attempt: 2, conclusion: 'failure' });
  assert.match((await run(f, 'resume')).blocker, /attempt 2.*failure/u);
  assert.equal(dispatches(f).length, 1);
});

test('unproven source and host preconditions block before dispatch', async t => {
  for (const [name, mutate, pattern] of [
    ['frozen input', f => { f.data.constantsFrozen = true; }, /reset it before source stabilization/u],
    ['offline runner', f => { f.data.heartbeat = false; }, /no fresh authenticated heartbeat/u],
    ['dirty checkout', f => { f.data.dirty = ' M source.rs'; }, /clean local checkout/u],
    ['wrong checkout', f => { f.data.localHead = D; }, /clean local checkout/u],
  ]) await t.test(name, async () => {
    const f = await started(); mutate(f);
    assert.match((await run(f)).blocker, pattern);
    assert.equal(dispatches(f).length, 0);
  });
});

test('artifact and acceptance mutation matrix fails before calibration', async t => {
  for (const [name, mutate, pattern] of [
    ['expired', f => { f.data.artifacts.get(122)[0].expired = true; }, /artifact provenance/u],
    ['corrupted container', f => { f.data.containers.set(f.data.artifacts.get(122)[0].id, Buffer.from('wrong')); }, /API digest/u],
    ['earlier artifact', f => { f.data.artifacts.get(122)[0].created_at = '2026-09-08T00:00:00Z'; }, /earlier run attempt/u],
    ['duplicate artifact', f => { f.data.artifacts.get(122).push(f.data.artifacts.get(122)[0]); }, /one retained artifact/u],
    ['missing workspace proof', f => { f.data.jobs.set(122, f.data.jobs.get(122).filter(row => row.name !== 'full-source-gate')); }, /full-source-gate/u],
    ['wrong job attempt', f => { f.data.jobs.get(122)[0].run_attempt = 2; }, /exact-run job/u],
    ['revoked status', f => { f.data.statuses.get(C).push({ ...f.data.statuses.get(C)[0], id: 2, state: 'failure' }); }, /authenticated Actions acceptance/u],
    ['non-Actions actor', f => { f.data.statuses.get(C)[0].creator.login = 'someone'; }, /authenticated Actions acceptance/u],
  ]) await t.test(name, async () => {
    const f = await started(); await run(f); f.completeSource(122); mutate(f);
    assert.match((await run(f, 'resume')).blocker, pattern);
    assert.equal(dispatches(f).length, 1);
  });
});

test('persisted phase text cannot synthesize freeze or bypass approval', async () => {
  const f = await started();
  const record = f.record(); record.phase = 'freeze'; f.data.comments[0].body = recordBody(record);
  assert.equal((await run(f)).phase, 'source_stabilization');
  assert.equal(f.record().heads.F, undefined);
  assert.equal(dispatches(f)[0].body.inputs.expected_head_sha, C);
  await run(f, 'advance', '--record-approval', '--approver', 'TheGreenCedar');
  assert.equal(f.record().approval, null);
});

test('legacy simulated evidence is rejected rather than adopted', async () => {
  const f = await started(); const record = f.record(); delete record.driver_version;
  record.receipt.groups['source-proof'] = { status: 'active', value: [{ digest: 'a'.repeat(64) }] };
  f.data.comments[0].body = recordBody(record);
  await assert.rejects(run(f, 'resume'), /legacy coordinator contains unverified proof/u);
  assert.equal(dispatches(f).length, 0);
});

test('explicit failed-run retry stays on its phase and records a real run', async () => {
  const f = await started(); await run(f);
  Object.assign(f.data.runs[0], { status: 'completed', conclusion: 'failure' });
  const state = await run(f, 'resume', '--retry-failed');
  assert.equal(state.blocker, null);
  assert.equal(state.dispatched.id, 123);
  assert.equal(f.record().dispatches.length, 2);
  assert.equal(dispatches(f)[1].body.inputs.acceptance_phase, 'source_stabilization');
});

async function calibrated() {
  const f = await started(); await run(f); f.completeSource(122); await run(f);
  f.completeCalibration(123);
  return f;
}

async function qualified() {
  const f = await calibrated();
  assert.equal((await run(f)).blocker, null); // Generated branch and draft PR.
  assert.equal(f.record().heads.F, F);
  assert.equal(f.data.next, C);
  assert.equal((await run(f)).blocker, null); // Checked fast-forward, before frozen proof.
  assert.equal(f.data.next, F);
  f.data.localHead = F;
  assert.equal((await run(f)).dispatched.id, 124);
  f.completeSource(124, 'frozen_candidate');
  assert.equal((await run(f)).dispatched.id, 125);
  f.completeQualification(125);
  assert.equal((await run(f)).dispatched.id, 126);
  f.completeCloseout(126);
  const ready = await run(f);
  assert.equal(ready.blocker, null);
  assert.equal(ready.phase, 'awaiting_approval');
  return f;
}

test('retained closeout must preserve mandatory cells, exception eligibility, and exact counts', async t => {
  for (const kind of ['mandatory-withheld', 'all-withheld', 'exception', 'count', 'summary-withheld']) await t.test(kind, async () => {
    const f = await qualified();
    const artifact = f.data.artifacts.get(126)[0];
    const names = ['pre_publish/ledger.json', 'pre_publish/summary.json', 'pre_publish/producer-provenance.json'];
    const docs = f.host().readArtifact(artifact, names);
    const ledger = JSON.parse(docs[names[0]]), summary = JSON.parse(docs[names[1]]);
    if (kind === 'mandatory-withheld') ledger.cells.find(row => row.id.startsWith('package_identity:')).status = 'withheld';
    if (kind === 'all-withheld') for (const row of ledger.cells) row.status = 'withheld';
    if (kind === 'exception') ledger.cells.find(row => row.id.startsWith('package_identity:')).status = 'pass_with_exception';
    if (kind === 'count') summary.counts.passed--;
    if (kind === 'summary-withheld') summary.withheld_hosts = ['linux-x64-vulkan'];
    docs[names[0]] = JSON.stringify(ledger); docs[names[1]] = JSON.stringify(summary);
    f.data.artifacts.set(126, []); f.artifact(f.data.runs.find(row => row.id === 126), artifact.name, docs);
    assert.ok((await run(f)).blocker);
    assert.equal(f.record().approval, null);
  });
});

test('three real calibration records retain their shared Actions producer and distinct raw identities', async () => {
  const f = await calibrated();
  const state = await run(f);
  assert.equal(state.blocker, null);
  const rows = f.record().receipt.groups.calibration.value;
  assert.equal(rows.length, 3);
  assert.deepEqual(rows.map(row => row.run_id), [123, 123, 123]);
  assert.deepEqual(rows.map(row => row.attempt), [1, 1, 1]);
  assert.equal(new Set(rows.map(row => row.identity)).size, 3);
  assert.equal(new Set(rows.map(row => row.member_sha256)).size, 3);
  assert.ok(f.data.lineageChecks >= 1);
  assert.equal(f.data.next, C);
  assert.equal(f.data.calls.some(call => call.program === 'git' && call.args.includes('worktree')), false,
    'freezing must reuse the existing checkout without creating another worktree');
});

test('generated source and canonical lineage failures cannot publish a freeze branch', async t => {
  for (const [name, key, pattern] of [['extra file', 'wrongFreezePath', /exact constant-only child/u], ['lineage owner', 'lineageFailure', /lineage owner rejected/u]]) {
    await t.test(name, async () => {
      const f = await calibrated(); f.data[key] = true;
      assert.match((await run(f)).blocker, pattern);
      assert.equal(f.data.branches.size, 0);
      assert.equal(f.data.next, C);
      assert.equal(f.data.localHead, C);
      assert.equal(f.data.localBranch, 'dev/codestory-next');
      assert.equal(f.data.localBranches.get('dev/codestory-next'), C);
      assert.equal(f.record().heads.F, undefined);
    });
  }
});

test('the real adapter reaches explicit approval using current workflow modes and one source stabilization', async () => {
  const f = await qualified();
  assert.equal(f.record().approval, null);
  assert.equal(f.data.main, M);
  assert.deepEqual(dispatches(f).map(call => call.body.inputs.acceptance_phase).filter(Boolean), ['source_stabilization', 'frozen_candidate']);
  assert.deepEqual(dispatches(f).map(call => call.body.inputs.mode).filter(Boolean), ['calibration', 'qualification']);
  assert.equal(dispatches(f)[2].body.ref, 'dev/codestory-next');
  assert.equal(dispatches(f)[2].body.inputs.pr_number, undefined);
  assert.equal(dispatches(f)[3].body.inputs.calibration_bundle_run_id, '123');
  assert.equal(dispatches(f)[4].body.inputs.publish_release, undefined);
  await run(f, 'resume');
  assert.equal(dispatches(f).length, 5);
  assert.equal(f.data.main, M);
});

test('publication requires approval and complete post-publish evidence, then restores dev', async () => {
  const f = await qualified();
  f.data.deleteDevOnMerge = true;
  assert.equal((await run(f, 'advance', '--record-approval', '--approver', 'TheGreenCedar')).phase, 'promotion');
  assert.equal(f.data.main, M);
  assert.equal((await run(f)).phase, 'publication');
  assert.equal(f.data.main, P);
  assert.equal(f.data.next, null);
  const publishing = await run(f, 'status');
  assert.equal(publishing.phase, 'publication');
  assert.equal(publishing.active_runs[0].id, 127);
  f.completeCloseout(127, 'post_publish');
  const restoring = await run(f);
  assert.equal(restoring.blocker, null);
  assert.equal(restoring.phase, 'reconcile_dev');
  assert.equal(f.data.next, P);
  const complete = await run(f, 'status');
  assert.equal(complete.blocker, null);
  assert.equal(complete.phase, 'complete');
  assert.equal(complete.sha, P);
  assert.equal(dispatches(f).length, 5); // Main publication is never a manual dispatch.
  assert.ok(Buffer.byteLength(f.data.comments[0].body) < 60_000, 'durable issue record must fit without embedding raw proof documents');
});

test('main-base drift after approval blocks before the merge effect', async () => {
  const f = await qualified();
  await run(f, 'advance', '--record-approval', '--approver', 'TheGreenCedar');
  f.data.main = D;
  const blocked = await run(f);
  assert.match(blocked.blocker, /approved frozen head and main base/u);
  assert.equal(f.data.calls.filter(call => call.args[0] === 'pr' && call.args[1] === 'merge').length, 0);
  assert.equal(f.data.main, D);
});

test('missing post-publish artifact authority cannot complete or reconcile the release', async () => {
  const f = await qualified(); f.data.deleteDevOnMerge = true;
  await run(f, 'advance', '--record-approval', '--approver', 'TheGreenCedar'); await run(f);
  f.completeCloseout(127, 'post_publish');
  f.data.artifacts.get(127)[0].expired = true;
  const blocked = await run(f);
  assert.match(blocked.blocker, /artifact provenance/u);
  assert.equal(blocked.phase, 'publication');
  assert.equal(f.record().receipt.groups.publication, undefined);
  assert.equal(f.data.next, null);
});

test('start recognizes a legacy coordinator title without creating a duplicate', async () => {
  const f = githubFixture();
  f.data.issues.push({ number: 888, title: 'Release 0.17.6 coordinator', body: 'Legacy coordinator without a marker.' });
  await assert.rejects(execute(['start', '--version', '0.17.6'], f.host()), /already exists: #888/u);
  assert.equal(f.data.issues.length, 1);
});

test('a rehearsal ends at pre-publish proof without main or catalog claims', async () => {
  const f = await qualified();
  const record = f.record(); record.rehearse = true; f.data.comments[0].body = recordBody(record);
  const done = await run(f, 'resume');
  assert.equal(done.blocker, null);
  assert.equal(done.phase, 'rehearsal_complete');
  assert.equal(f.data.main, M);
  assert.equal(f.record().receipt.groups.publication, undefined);
  assert.equal(f.record().receipt.groups['catalog-delivery'], undefined);
});

test('freeze uses real Git without another checkout and preserves operator state on failure', async t => {
  for (const scenario of ['attached', 'detached', 'commit failure', 'lineage failure', 'push failure',
    'adopt existing', 'concurrent staged files', 'concurrent constant edit']) await t.test(scenario, () => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'codestory-freeze-git-test-'));
    try {
      const repo = path.join(root, 'repo'), remote = path.join(root, 'remote.git');
      mkdirSync(repo);
      const git = args => execFileSync('git', ['-C', repo, ...args], { encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }).trim();
      git(['init', '-b', 'candidate']);
      git(['config', 'user.name', 'Fixture']); git(['config', 'user.email', 'fixture@example.invalid']);
      git(['config', 'commit.gpgsign', 'false']);
      git(['config', 'core.hooksPath', os.devNull]);
      mkdirSync(path.join(repo, path.dirname(CONSTANT_SET)), { recursive: true });
      writeFileSync(path.join(repo, CONSTANT_SET), '{"status":"unfrozen"}\n');
      writeFileSync(path.join(repo, 'other.txt'), 'original\n');
      git(['add', '.']); git(['commit', '-m', 'source']);
      const C = { commit: git(['rev-parse', 'HEAD']), tree: git(['rev-parse', 'HEAD^{tree}']) };
      git(['init', '--bare', remote]); git(['remote', 'add', 'origin', remote]);
      if (scenario === 'detached') git(['switch', '--detach', C.commit]);
      const originalBranch = git(['branch', '--show-current']);
      const originalIndex = git(['ls-files', '--stage']);
      const bytes = JSON.stringify({ status: 'frozen', source: C.commit }) + '\n';
      const calibration = { documents: { 'per-user-embedding-server-constant-set.json': bytes,
        'calibration-bundle.json': '{}' }, rows: [{ run_id: 123, attempt: 1 }], artifact: { name: 'calibration' } };
      let F, commits = 0, pushes = 0;
      const host = createDefaultHost({ projectRoot: ROOT, runCommand(program, args, options = {}) {
        if (program === 'git') {
          assert.equal(args[0], '-C'); assert.equal(args[1], ROOT);
          assert.equal(args.includes('worktree'), false);
          if (args.includes('commit-tree') && scenario === 'commit failure') throw new Error('fixture commit failure');
          if (args.includes('push')) {
            pushes++;
            if (scenario === 'push failure') throw new Error('fixture push failure');
          }
          const result = execFileSync(program, ['-C', repo, ...args.slice(2)], {
            encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'], ...options,
          });
          if (args.includes('commit-tree')) {
            F = result.trim(); commits++;
            if (scenario === 'concurrent staged files') {
              writeFileSync(path.join(repo, 'other.txt'), 'operator edit\n'); git(['add', 'other.txt']);
              writeFileSync(path.join(repo, 'untracked.txt'), 'operator file\n');
            }
          }
          return result;
        }
        if (program === 'gh') {
          assert.ok(args[1].includes('/pulls') || args.includes('--method'));
          if (!options.input) return '[]';
          const body = JSON.parse(options.input);
          return JSON.stringify({ number: 42, head: { sha: F }, base: { ref: body.base } });
        }
        assert.ok(args[0].endsWith('check-calibration-release-lineage.py'));
        assert.equal(args[args.indexOf('--repo') + 1], ROOT);
        assert.equal(git(['rev-parse', 'HEAD']), F);
        assert.equal(readFileSync(path.join(repo, CONSTANT_SET), 'utf8'), bytes);
        if (scenario === 'concurrent constant edit') {
          writeFileSync(path.join(repo, CONSTANT_SET), 'operator constant edit\n');
          throw new Error('fixture lineage failure after operator edit');
        }
        if (scenario === 'lineage failure') throw new Error('fixture lineage failure');
        return '{"status":"passed"}';
      } });
      Object.defineProperty(host, 'heads', { value: { next: C } });
      host.findIssue = () => ({ number: 41 });
      const record = { heads: { C: C.commit, C_tree: C.tree }, receipt: { version: '0.18.0' }, issue_number: 40 };
      if (scenario.includes('failure') || scenario.startsWith('concurrent')) {
        const refusal = scenario === 'concurrent staged files' ? /clean existing checkout/u
          : scenario === 'concurrent constant edit' ? /would be overwritten by checkout/u
            : new RegExp(`fixture ${scenario}`, 'u');
        assert.throws(() => host.freeze(record, calibration), error => {
          assert.match(error.message, refusal);
          if (scenario === 'concurrent constant edit') {
            assert.ok(error instanceof AggregateError);
            assert.match(error.errors[0].message, /fixture lineage failure after operator edit/u);
            assert.match(error.errors[1].message, /would be overwritten by checkout/u);
            assert.match(error.message, /fixture lineage failure after operator edit/u);
          }
          return true;
        });
        assert.equal(pushes, scenario === 'push failure' ? 1 : 0);
      } else {
        assert.equal(host.freeze(record, calibration).commit, F);
        if (scenario === 'adopt existing') {
          assert.equal(host.freeze(record, calibration).commit, F);
          assert.equal(commits, 1); assert.equal(pushes, 1);
        }
      }
      assert.equal(git(['rev-parse', 'refs/heads/candidate']), C.commit);
      if (F) {
        assert.equal(git(['rev-list', '--parents', '-n', '1', F]), `${F} ${C.commit}`);
        assert.equal(git(['diff', '--name-only', C.commit, F]), CONSTANT_SET);
      }
      if (scenario === 'concurrent constant edit') {
        assert.equal(git(['rev-parse', 'HEAD']), F);
        assert.equal(readFileSync(path.join(repo, CONSTANT_SET), 'utf8'), 'operator constant edit\n');
      } else {
        assert.equal(git(['rev-parse', 'HEAD']), C.commit);
        assert.equal(git(['branch', '--show-current']), originalBranch);
        if (scenario === 'concurrent staged files') {
          assert.equal(git(['diff', '--cached', '--name-only']), 'other.txt');
          assert.equal(readFileSync(path.join(repo, 'other.txt'), 'utf8'), 'operator edit\n');
          assert.equal(readFileSync(path.join(repo, 'untracked.txt'), 'utf8'), 'operator file\n');
        } else {
          assert.equal(git(['status', '--porcelain']), '');
          assert.equal(git(['ls-files', '--stage']), originalIndex);
        }
      }
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
});
