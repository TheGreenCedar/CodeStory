import assert from 'node:assert/strict';
import test from 'node:test';
import { execute } from '../codestory-release.mjs';
import { recordBody } from '../lib/release-coordinator-contract.mjs';
import { githubFixture, C, D, F, M, P } from './fixtures/release-github.mjs';

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
});

test('generated source and canonical lineage failures cannot publish a freeze branch', async t => {
  for (const [name, key, pattern] of [['extra file', 'wrongFreezePath', /only the generated/u], ['lineage owner', 'lineageFailure', /lineage owner rejected/u]]) {
    await t.test(name, async () => {
      const f = await calibrated(); f.data[key] = true;
      assert.match((await run(f)).blocker, pattern);
      assert.equal(f.data.branches.size, 0);
      assert.equal(f.data.next, C);
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
