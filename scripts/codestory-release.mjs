#!/usr/bin/env node
import { pathToFileURL } from 'node:url';
import { initReceipt, invalidateReceipt, recordGroup, validatePhase } from '../.github/scripts/release-driver-receipt.mjs';
import { createDefaultHost } from './lib/release-coordinator-github.mjs';
import { sourceEvidence, calibrationEvidence, qualificationEvidence, closeoutEvidence } from './lib/release-coordinator-evidence.mjs';
import {
  COORDINATOR_SCHEMA, SOURCE_PROOF_WORKFLOW, PACKAGED_WORKFLOW, RELEASE_WORKFLOW,
  AUTO_RELEASE_WORKFLOW, BROAD_WORKFLOWS, RUNNING, SHA, requireThat, sameHead,
  sourceHead, frozenHead, workflowFor, validateRun,
} from './lib/release-coordinator-contract.mjs';

export { COORDINATOR_SCHEMA, SOURCE_PROOF_WORKFLOW, PACKAGED_WORKFLOW, RELEASE_WORKFLOW, BROAD_WORKFLOWS, createDefaultHost };

function parseArgs(argv) {
  const options = { command: argv[0], lane: 'native' };
  requireThat(['start', 'status', 'advance', 'resume'].includes(options.command), 'usage: codestory-release.mjs <start|status|advance|resume>');
  for (let index = 1; index < argv.length; index++) {
    const flag = argv[index];
    if (['--rehearse', '--record-approval', '--retry-failed'].includes(flag)) { options[flag.slice(2)] = true; continue; }
    requireThat(['--version', '--lane', '--issue', '--approver', '--repo'].includes(flag) && argv[index + 1] && !argv[index + 1].startsWith('--'), `invalid release argument ${flag}`);
    options[flag.slice(2)] = argv[++index];
  }
  if (options.issue) requireThat(/^[1-9][0-9]*$/u.test(options.issue), 'invalid coordinator issue');
  requireThat(['native', 'plugin'].includes(options.lane), 'invalid release lane');
  return options;
}

function put(record, group, value) {
  const next = recordGroup(record.receipt, group, value);
  if (record.receipt.groups[group]?.status !== 'active'
    || JSON.stringify(record.receipt.groups[group].value) !== JSON.stringify(next.groups[group].value)) record.receipt = next;
}

function baseGroups(record) {
  put(record, 'calibration-source', sourceHead(record));
  const openFreeze = record.freeze && !record.freeze.integrated;
  put(record, 'pull-requests', { release_pr: openFreeze ? record.freeze.pull_request : null,
    bind: openFreeze ? 'release_pr' : 'next_head', integrated_support_prs: record.support_prs ?? [] });
  if (!record.receipt.groups.evidence || record.receipt.groups.evidence.status !== 'active') put(record, 'evidence', { reusable: [], invalidated: [] });
}

function result(record, host, extra = {}) {
  const target = record.published_head ?? frozenHead(record) ?? sourceHead(record);
  return { phase: record.phase, sha: target.commit, tree: target.tree, record,
    elapsed_ms: host.now().getTime() - Date.parse(record.started_at), active_runs: [], blocker: null,
    next_action: `advance ${record.phase}`, rehearse: record.rehearse, ...extra };
}

function save(record, host) {
  put(record, 'next-action', { action: record.next_action ?? `advance ${record.phase}`, owner: 'codestory-release' });
  host.save(record);
}

function initialize(record) {
  requireThat(record.schema === COORDINATOR_SCHEMA && SHA.test(record.heads?.C) && SHA.test(record.heads?.C_tree), 'invalid release coordinator source identity');
  if (record.driver_version !== 2) {
    const priorProof = Object.keys(record.receipt.groups).some(group => !['calibration-source', 'pull-requests', 'evidence', 'next-action'].includes(group));
    requireThat(!priorProof, 'legacy coordinator contains unverified proof; preserve that record and start an authenticated release record');
    record.driver_version = 2;
    record.dispatches = [];
    record.approval = null;
  }
  record.dispatches ??= [];
  record.evidence = {};
  baseGroups(record);
}

function stepHead(record, phase) {
  return ['source_stabilization', 'calibration'].includes(phase) ? sourceHead(record) : frozenHead(record);
}

function liveStep(host, step) {
  const run = host.getRun(step.id);
  validateRun(run, { repository: host.repository, head: step.head, workflow: step.workflow });
  return run;
}

function recoverIntents(record, host) {
  for (const step of record.dispatches.filter(row => !row.id && !row.rejected)) {
    const matches = host.runsFor(step.head.commit).filter(run => run.path === step.workflow
      && run.event === 'workflow_dispatch' && run.head_repository?.full_name === host.repository
      && run.actor?.login === step.actor && Date.parse(run.created_at) >= Date.parse(step.started_at));
    requireThat(matches.length === 1, `unconfirmed ${step.phase} dispatch: ${matches.length} matching runs; no replacement will be dispatched`);
    step.id = matches[0].id;
    liveStep(host, step);
    save(record, host);
  }
}

function cancelObsolete(record, host, next) {
  const active = [];
  for (const step of record.dispatches.filter(row => row.id && row.head.commit !== next.commit)) {
    const run = liveStep(host, step);
    if (!RUNNING.has(run.status)) continue;
    if (!step.cancel_requested_at) {
      host.cancel(run);
      step.cancel_requested_at = host.now().toISOString();
    } else if (!step.force_cancel_requested_at && host.now().getTime() - Date.parse(step.cancel_requested_at) >= 30_000) {
      host.cancel(run, true);
      step.force_cancel_requested_at = host.now().toISOString();
    }
    active.push({ id: run.id, workflow: run.path, headSha: run.head_sha, status: run.status });
  }
  return active;
}

function reconcileSource(record, host) {
  const next = host.heads.next;
  const C = sourceHead(record), F = frozenHead(record);
  const promotion = record.promotion_pr ? host.pullRequest(record.promotion_pr) : null;
  if (promotion?.merged) {
    const P = host.commit(promotion.merge_commit_sha);
    requireThat(F && P.parents.includes(F.commit) && P.tree === F.tree && host.heads.main.commit === P.commit,
      'main promotion is not the exact tree-preserving frozen candidate');
    record.published_head = P;
    return null;
  }
  requireThat(next, 'dev/codestory-next is missing before promotion');
  if (sameHead(next, C) || sameHead(next, F)) return null;
  const active = cancelObsolete(record, host, next);
  record.approval = null;
  if (active.length) return { active_runs: active, blocker: 'obsolete owned proof is still active', next_action: 'wait for cancellation, then resume; force cancellation is attempted after 30 seconds' };
  record.receipt = invalidateReceipt(record.receipt, { event: 'evidence', groups: Object.keys(record.receipt.groups),
    reason: 'source head moved outside the generated freeze/promotion lineage', replacingSha: next.commit });
  record.heads = { C: next.commit, C_tree: next.tree };
  delete record.freeze; delete record.promotion_pr; delete record.published_head;
  record.phase = 'source_stabilization';
  baseGroups(record);
  put(record, 'evidence', { reusable: [], invalidated: [{ identity: `source@${C.commit}`, reason: 'source head moved', replacing_sha: next.commit }] });
  return null;
}

function observeStep(record, host, phase, read, options) {
  const head = stepHead(record, phase);
  const steps = record.dispatches.filter(step => step.phase === phase && sameHead(step.head, head) && step.id);
  const step = steps.at(-1);
  if (!step) return { missing: true };
  const run = liveStep(host, step);
  if (RUNNING.has(run.status)) return { waiting: true, run };
  if (run.status !== 'completed' || run.conclusion !== 'success') {
    invalidatePhaseEvidence(record, `run ${run.id} attempt ${run.run_attempt} finished ${run.conclusion}`);
    if (options['retry-failed'] && options.command !== 'status') return { missing: true, prior_failure: { run_id: run.id, attempt: run.run_attempt, conclusion: run.conclusion } };
    throw new Error(`${phase} run ${run.id}, attempt ${run.run_attempt}, finished ${run.conclusion}; resolve its cause before --retry-failed`);
  }
  return { evidence: read(run, head) };
}

function invalidatePhaseEvidence(record, reason) {
  const phases = ['source_stabilization', 'calibration', 'frozen_candidate_acceptance', 'qualification', 'pre_publish', 'publication'];
  const groups = [
    ['source-stabilization', 'source-proof'], ['calibration'], ['frozen-candidate-acceptance'],
    ['qualification'], ['pre-publish-ledger', 'package', 'hardware', 'installed-candidate'],
    ['publication', 'post-publish-ledger', 'catalog-delivery'],
  ];
  const index = phases.indexOf(record.phase);
  if (index < 0) return;
  const affected = groups.slice(index).flat().filter(group => record.receipt.groups[group]?.status === 'active');
  if (affected.length) record.receipt = invalidateReceipt(record.receipt, { event: 'evidence', groups: affected,
    reason, replacingSha: (frozenHead(record) ?? sourceHead(record)).commit });
}

function archiveBindings(ledger) {
  return ledger.cells.filter(cell => cell.id.startsWith('package_identity:'))
    .map(cell => ({ id: cell.id, archive: cell.archive })).sort((left, right) => left.id.localeCompare(right.id));
}

function observe(record, host, options) {
  recoverIntents(record, host);
  const drift = reconcileSource(record, host);
  if (drift) return result(record, host, drift);
  const completed = {};
  const readers = [
    ['source_stabilization', (run, head) => sourceEvidence(host, run, head, 'source_stabilization')],
    ['calibration', (run, head) => calibrationEvidence(host, run, head)],
  ];
  if (record.freeze && (sameHead(host.heads.next, frozenHead(record)) || record.published_head)) readers.push(
    ['frozen_candidate_acceptance', (run, head) => sourceEvidence(host, run, head, 'frozen_candidate')],
    ['qualification', (run, head) => qualificationEvidence(host, run, head, record.receipt.version)],
    ['pre_publish', (run, head) => closeoutEvidence(host, run, head, record.receipt.version, 'pre_publish')],
  );
  for (const [phase, read] of readers) {
    record.phase = phase;
    const observed = observeStep(record, host, phase, read, options);
    if (observed.missing) return result(record, host, { prior_failure: observed.prior_failure, completed });
    if (observed.waiting) return result(record, host, { completed, active_runs: [{ id: observed.run.id, workflow: observed.run.path, headSha: observed.run.head_sha, status: observed.run.status }], next_action: 'wait for the in-flight permitted workflow' });
    completed[phase] = observed.evidence;
    if (phase === 'source_stabilization') {
      put(record, 'source-stabilization', observed.evidence.row);
      put(record, 'source-proof', observed.evidence.row);
    } else if (phase === 'calibration') put(record, 'calibration', observed.evidence.rows);
    else if (phase === 'frozen_candidate_acceptance') {
      host.verifyFrozen(frozenHead(record), sourceHead(record), completed.calibration);
      put(record, 'frozen-candidate', frozenHead(record));
      put(record, 'frozen-candidate-acceptance', observed.evidence.row);
    } else if (phase === 'qualification') put(record, 'qualification', observed.evidence.rows);
    else {
      put(record, 'pre-publish-ledger', observed.evidence.row);
      for (const [group, prefix] of [['package', 'package_identity'], ['hardware', 'accelerator_execution'], ['installed-candidate', 'candidate_installed_behavior']]) {
        put(record, group, { ...observed.evidence.row, lane: group,
          cells: observed.evidence.ledger.cells.filter(cell => cell.id.startsWith(prefix)).map(cell => ({ id: cell.id, status: cell.status })) });
      }
      record.evidence.pre_publish = { row: observed.evidence.row, withheld_claims: observed.evidence.ledger.withheld_claims };
    }
  }
  if (!record.freeze) record.phase = 'freeze';
  else if (!sameHead(host.heads.next, frozenHead(record)) && !record.published_head) record.phase = 'integrate_frozen';
  else if (record.published_head) record.phase = 'publication';
  else if (record.rehearse) record.phase = 'rehearsal_complete';
  else if (!record.approval) record.phase = 'awaiting_approval';
  else if (!record.published_head) record.phase = 'promotion';
  else record.phase = 'publication';
  if (record.phase === 'publication') {
    const P = record.published_head;
    const run = host.autoRelease(P);
    if (!run) return result(record, host, { completed, next_action: 'wait for the automatic main release; no manual publication dispatch is permitted' });
    validateRun(run, { repository: host.repository, head: P, workflow: AUTO_RELEASE_WORKFLOW });
    if (RUNNING.has(run.status)) return result(record, host, { completed, active_runs: [{ id: run.id, workflow: run.path, headSha: run.head_sha, status: run.status }], next_action: 'wait for automatic publication and post-publish proof' });
    const post = closeoutEvidence(host, run, P, record.receipt.version, 'post_publish');
    const release = host.release(record.receipt.version);
    const assets = host.graph.workflow_policy.package_matrix.map(row => `codestory-cli-v${record.receipt.version}-${row.asset_target}.${row.extension}`);
    requireThat(!release.isDraft && !release.isPrerelease && release.publishedAt
      && release.tagName === `v${record.receipt.version}` && host.tag(record.receipt.version) === P.commit
      && assets.every(name => release.assets.some(asset => asset.name === name))
      && release.assets.some(asset => asset.name === 'SHA256SUMS.txt'), 'published release assets or tag do not match the qualified promotion');
    put(record, 'promotion', { pull_request: record.promotion_pr, approver: record.approval.approver, approved_at: record.approval.at });
    put(record, 'publication', { commit: P.commit, tree: P.tree, tag: release.tagName, release_url: release.url, release_run: post.row });
    put(record, 'post-publish-ledger', post.row);
    put(record, 'catalog-delivery', { state: post.ledger.catalog_delivery.state, installer_identity: post.ledger.catalog_delivery.installer });
    record.evidence.post_publish = { row: post.row, withheld_claims: post.ledger.withheld_claims };
    record.phase = sameHead(host.heads.next, P) ? 'complete' : 'reconcile_dev';
    if (record.phase === 'complete') validatePhase(record.receipt, 'closeout');
  }
  return result(record, host, { completed });
}

function dispatchRequest(record, completed) {
  const phase = record.phase;
  const head = stepHead(record, phase);
  const version = record.receipt.version;
  const workflow = workflowFor(phase);
  let ref = 'dev/codestory-next', inputs = { expected_head_sha: head.commit };
  if (phase === 'source_stabilization' || phase === 'frozen_candidate_acceptance') {
    inputs = { ...inputs, version, acceptance_only: 'true',
      acceptance_phase: phase === 'source_stabilization' ? phase : 'frozen_candidate',
      freeze_receipt_digest: '', support_prs_json: JSON.stringify(record.support_prs ?? []),
      reusable_evidence_json: '[]', invalidated_evidence_json: '[]' };
  } else if (phase === 'calibration' || phase === 'qualification') {
    inputs = { ...inputs, mode: phase, scope: phase === 'calibration' ? 'none' : 'full',
      freeze_receipt_digest: completed[phase === 'calibration' ? 'source_stabilization' : 'frozen_candidate_acceptance'].freeze_digest };
    if (phase === 'qualification') {
      inputs.calibration_bundle_artifact = completed.calibration.artifact.name;
      inputs.calibration_bundle_run_id = String(completed.calibration.rows[0].run_id);
      inputs.qualify_linux_vulkan = 'false';
    }
  } else inputs.version = version;
  return { phase, head, workflow, ref, inputs, version };
}

async function advance(record, host, state, options) {
  requireThat(!state.blocker && state.active_runs.length === 0, state.blocker ?? 'release proof is already in flight');
  requireThat(record.lane === 'native', 'plugin-only release has no dispatchable pre-publish workflow; native proof cannot authorize that lane');
  const completed = state.completed;
  if (options['record-approval']) {
    requireThat(record.phase === 'awaiting_approval' && !record.rehearse && options.approver === host.actor,
      'combined approval requires the qualified candidate and the authenticated maintainer login');
    validatePhase(record.receipt, 'frozen');
    requireThat(record.promotion_pr, 'create the concrete promotion PR before recording approval');
    record.approval = { approver: host.actor, at: host.now().toISOString(), commit: record.heads.F,
      tree: record.heads.F_tree, main_commit: host.heads.main.commit, qualification_run: completed.qualification.rows[0].run_id,
      pre_publish_run: completed.pre_publish.row.run_id, pre_publish_digest: completed.pre_publish.row.digest,
      archives: archiveBindings(completed.pre_publish.ledger) };
    record.phase = 'promotion';
    return result(record, host, { next_action: 'advance to merge the approved promotion PR' });
  }
  if (record.phase === 'freeze') {
    record.freeze = host.freeze(record, completed.calibration);
    record.heads.F = record.freeze.commit; record.heads.F_tree = record.freeze.tree;
    put(record, 'frozen-candidate', frozenHead(record));
    record.phase = 'integrate_frozen';
    return result(record, host, { next_action: `independently review generated PR #${record.freeze.pull_request} and its checks, then advance the tree-preserving fast-forward` });
  }
  if (record.phase === 'integrate_frozen') {
    host.verifyFrozen(frozenHead(record), sourceHead(record), completed.calibration);
    host.integrateFrozen(record);
    record.freeze.integrated = true;
    record.phase = 'frozen_candidate_acceptance';
    return result(record, host, { next_action: 'verify the clean pushed dev head, then advance frozen acceptance against that stable branch binding' });
  }
  if (record.phase === 'awaiting_approval') {
    validatePhase(record.receipt, 'frozen');
    record.promotion_pr = host.promotionPr(record).number;
    return result(record, host, { next_action: `obtain combined maintainer approval for promotion PR #${record.promotion_pr}; record it with --record-approval --approver ${host.actor}` });
  }
  if (record.phase === 'promotion') {
    requireThat(record.approval?.commit === record.heads.F && record.approval.tree === record.heads.F_tree
      && JSON.stringify(record.approval.archives) === JSON.stringify(archiveBindings(completed.pre_publish.ledger)),
    'maintainer approval no longer binds the qualified candidate');
    validatePhase(record.receipt, 'frozen');
    const pr = host.promote(record);
    requireThat(pr.merged === true, 'promotion has not merged');
    record.phase = 'publication';
    return result(record, host, { next_action: 'wait for automatic main publication and authenticated post-publish closeout' });
  }
  if (record.phase === 'reconcile_dev') {
    host.reconcileDev(record.published_head);
    return result(record, host, { next_action: 'read back both branches and post-publish proof before closing the release' });
  }
  if (['complete', 'rehearsal_complete', 'publication'].includes(record.phase)) return state;
  const request = dispatchRequest(record, completed);
  const probe = host.preflight(request.head, record.phase, request);
  const intent = { ...request, started_at: host.now().toISOString(), actor: host.actor, preflight: probe };
  record.dispatches.push(intent);
  save(record, host); // Persist intent before the external effect. Uncertain replies never become retries.
  intent.id = host.dispatch(request);
  return result(record, host, { dispatched: { id: intent.id, workflow: intent.workflow, headSha: intent.head.commit }, next_action: 'wait for the dispatched exact-head workflow' });
}

export async function execute(argv, host) {
  const options = parseArgs(argv);
  host.begin();
  if (options.command === 'start') {
    requireThat(/^\d+\.\d+\.\d+$/u.test(options.version ?? ''), '--version must be a release version');
    const marker = `codestory-release:${options.version}:${options.lane}`;
    const existing = host.findCoordinator(options.version) ?? host.findIssue(marker);
    requireThat(!existing, `release coordinator already exists: #${existing?.number}`);
    const head = host.heads.next;
    requireThat(head, 'dev/codestory-next is missing');
    const issue = host.createIssue(`Release ${options.version} coordinator`, `Canonical coordinator record for ${options.version}.\n\n<!-- ${marker} -->`);
    const record = { schema: COORDINATOR_SCHEMA, driver_version: 2, issue_number: issue.number,
      lane: options.lane, rehearse: Boolean(options.rehearse), phase: 'source_stabilization',
      started_at: host.now().toISOString(), heads: { C: head.commit, C_tree: head.tree },
      approval: null, dispatches: [], receipt: initReceipt(options.version) };
    baseGroups(record); save(record, host);
    return result(record, host);
  }
  const record = host.load(options.issue);
  initialize(record);
  let state;
  try {
    state = observe(record, host, options);
    if (options.command !== 'status' && !state.blocker && state.active_runs.length === 0) state = await advance(record, host, state, options);
  } catch (error) {
    invalidatePhaseEvidence(record, error.message);
    record.next_action = error.message;
    state = result(record, host, { blocker: error.message, next_action: error.message });
  }
  record.next_action = state.next_action;
  save(record, host);
  return state;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const value = await execute(process.argv.slice(2), createDefaultHost({ repository: options.repo }));
  for (const [key, result] of Object.entries({ phase: value.phase, sha: value.sha, tree: value.tree,
    active_runs: value.active_runs.length, blocker: value.blocker ?? 'none', next: value.next_action,
    elapsed_minutes: Math.round(value.elapsed_ms / 60000), rehearse: value.rehearse })) console.log(`${key}: ${result}`);
  if (value.blocker && options.command !== 'status') process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main().catch(error => {
  console.error(`codestory-release: ${error.message}`); process.exitCode = 1;
});
