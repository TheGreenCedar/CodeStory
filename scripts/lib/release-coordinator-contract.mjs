import { createHash } from 'node:crypto';

export const COORDINATOR_SCHEMA = 'codestory.release-coordinator/v1';
export const MARKER = `<!-- ${COORDINATOR_SCHEMA} -->`;
export const SOURCE_PROOF_WORKFLOW = '.github/workflows/source-proof.yml';
export const PACKAGED_WORKFLOW = '.github/workflows/packaged-platform-pr.yml';
export const RELEASE_WORKFLOW = '.github/workflows/release.yml';
export const AUTO_RELEASE_WORKFLOW = '.github/workflows/auto-release.yml';
export const CONSTANT_SET = 'crates/codestory-llama-sys/per-user-embedding-server-constant-set.json';
export const BROAD_WORKFLOWS = Object.freeze([SOURCE_PROOF_WORKFLOW, PACKAGED_WORKFLOW, RELEASE_WORKFLOW, AUTO_RELEASE_WORKFLOW]);
export const RUNNING = new Set(['queued', 'waiting', 'requested', 'pending', 'in_progress']);
export const SHA = /^[a-f0-9]{40}$/u;
export const DIGEST = /^sha256:[a-f0-9]{64}$/u;
export const requireThat = (condition, message) => { if (!condition) throw new Error(message); };
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export const sameHead = (left, right) => left?.commit === right?.commit && left?.tree === right?.tree;
export const sourceHead = record => ({ commit: record.heads.C, tree: record.heads.C_tree });
export const frozenHead = record => record.heads.F ? ({ commit: record.heads.F, tree: record.heads.F_tree }) : null;

export function parseRecord(body) {
  if (typeof body !== 'string' || !body.includes(MARKER)) return null;
  const match = body.match(/```json\n([\s\S]*?)\n```/u);
  if (!match) return null;
  const value = JSON.parse(match[1]);
  return value.schema === COORDINATOR_SCHEMA ? value : null;
}

export function recordBody(record) {
  return `${MARKER}\n\`\`\`json\n${JSON.stringify(record, null, 2)}\n\`\`\`\n`;
}

export function workflowFor(phase) {
  if (['source_stabilization', 'frozen_candidate_acceptance'].includes(phase)) return SOURCE_PROOF_WORKFLOW;
  if (['calibration', 'qualification'].includes(phase)) return PACKAGED_WORKFLOW;
  if (phase === 'pre_publish') return RELEASE_WORKFLOW;
  throw new Error(`phase ${phase} has no dispatchable workflow`);
}

export function validateRun(run, { repository, head, workflow, successful = false }) {
  requireThat(Number.isSafeInteger(run?.id) && run.id > 0
    && Number.isSafeInteger(run.run_attempt) && run.run_attempt > 0
    && run.head_sha === head.commit && run.path === workflow
    && run.head_repository?.full_name === repository
    && run.event === (workflow === AUTO_RELEASE_WORKFLOW ? 'push' : 'workflow_dispatch'),
  'release run does not match its repository, head, workflow and event');
  if (successful) requireThat(run.status === 'completed' && run.conclusion === 'success', 'latest release run attempt did not succeed');
  return run;
}

export function artifactRow(artifact, run, head, lane) {
  requireThat(artifact?.expired === false && Number.isSafeInteger(artifact.id)
    && artifact.id > 0 && DIGEST.test(artifact.digest)
    && artifact.workflow_run?.id === run.id && artifact.workflow_run?.head_sha === head.commit,
  'release artifact provenance is missing or changed');
  // Artifacts with stable names may survive a failed rerun. Never attribute them to it.
  requireThat(Number.isFinite(Date.parse(artifact.created_at))
    && Number.isFinite(Date.parse(run.run_started_at))
    && Date.parse(artifact.created_at) >= Date.parse(run.run_started_at),
  'release artifact belongs to an earlier run attempt');
  return { lane, run_id: run.id, attempt: run.run_attempt, artifact: artifact.name,
    artifact_id: artifact.id, digest: artifact.digest, identity: `${lane}@${head.commit}`,
    commit: head.commit, tree: head.tree, conclusion: 'success' };
}

export function requireJob(jobs, run, name) {
  const matches = jobs.filter(job => job.name === name);
  requireThat(matches.length === 1 && matches[0].status === 'completed'
    && matches[0].conclusion === 'success' && matches[0].run_id === run.id
    && matches[0].run_attempt === run.run_attempt && matches[0].head_sha === run.head_sha,
  `required exact-attempt job did not succeed: ${name}`);
  return matches[0];
}
