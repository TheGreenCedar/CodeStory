import { acceptedFreezeStatus, validateAcceptanceProvenance } from '../../.github/scripts/release-freeze-barrier.mjs';
import { deriveReleaseCells, validateReleaseCloseoutLedger } from '../codestory-release-closeout.mjs';
import { releaseClaimGraphDigest } from '../codestory-release-claims.mjs';
import {
  SOURCE_PROOF_WORKFLOW, PACKAGED_WORKFLOW, RELEASE_WORKFLOW, AUTO_RELEASE_WORKFLOW,
  requireThat, sha256, sameHead, artifactRow, requireJob, validateRun,
} from './release-coordinator-contract.mjs';

function selectedArtifact(host, run, head, name, lane) {
  const matches = host.artifacts(run).filter(artifact => artifact.name === name);
  requireThat(matches.length === 1, `expected one retained artifact: ${name}`);
  const artifact = matches[0];
  return { artifact, row: artifactRow(artifact, run, head, lane) };
}

function sourceMatches(source, head) {
  return sameHead(source, head) && source.tracked_dirty === false;
}

function requireNestedJob(jobs, run, suffix) {
  const matches = jobs.filter(job => job.name === suffix || job.name.endsWith(` / ${suffix}`));
  requireThat(matches.length === 1, `expected one exact-attempt job: ${suffix}`);
  return requireJob(jobs, run, matches[0].name);
}

export function sourceEvidence(host, run, head, phase) {
  validateRun(run, { repository: host.repository, head, workflow: SOURCE_PROOF_WORKFLOW, successful: true });
  const { artifact, row } = selectedArtifact(host, run, head, `release-freeze-receipt-attempt-${run.run_attempt}`, phase);
  const receipt = JSON.parse(host.readArtifact(artifact, ['release-freeze-receipt.json'])['release-freeze-receipt.json']);
  const status = acceptedFreezeStatus(host.statuses(head.commit), { tree: head.tree, digest: receipt.digest });
  const jobs = host.jobs(run);
  validateAcceptanceProvenance({ status, run, jobs, artifact, receipt, repository: host.repository,
    commit: head.commit, tree: head.tree, digest: receipt.digest, phase });
  if (phase === 'source_stabilization') {
    for (const name of ['full-source-gate', 'retrieval-generalization', 'windows-native-contracts']) requireJob(jobs, run, name);
    selectedArtifact(host, run, head, `release-cell-prepublish-source-attempt-${run.run_attempt}`, 'source_behavior');
  }
  return { row, freeze_digest: receipt.digest, receipt };
}

export function calibrationEvidence(host, run, head) {
  validateRun(run, { repository: host.repository, head, workflow: PACKAGED_WORKFLOW, successful: true });
  requireJob(host.jobs(run), run, 'calibration-assemble');
  const { artifact, row } = selectedArtifact(host, run, head, `embedding-calibration-bundle-${head.commit}`, 'calibration');
  const members = ['calibration-bundle.json', 'manifest.json', 'per-user-embedding-server-constant-set.json'];
  const documents = host.readArtifact(artifact, members);
  const bundle = JSON.parse(documents[members[0]]);
  const manifest = JSON.parse(documents[members[1]]);
  const frozen = JSON.parse(documents[members[2]]);
  requireThat(sourceMatches(bundle.source, head) && sourceMatches(manifest.selection_source, head)
    && manifest.source_head_sha === head.commit && manifest.source_tree === head.tree
    && String(manifest.producer_run_id) === String(run.id)
    && String(manifest.producer_run_attempt) === String(run.run_attempt)
    && manifest.run_count === 3 && manifest.matrix_cell_count === 1
    && manifest.bundle?.sha256 === sha256(documents[members[0]])
    && manifest.frozen_constant_set?.sha256 === sha256(documents[members[2]])
    && bundle.producer?.repository === host.repository && bundle.producer.workflow_path === PACKAGED_WORKFLOW
    && String(bundle.producer.run_id) === String(run.id) && String(bundle.producer.run_attempt) === String(run.run_attempt)
    && bundle.producer.artifact_name === artifact.name && bundle.producer.source_head_sha === head.commit,
  'calibration bundle identity differs from its exact producer');
  requireThat(Array.isArray(bundle.runs) && bundle.runs.length === 3
    && new Set(bundle.runs.map(item => item.run_index)).size === 3
    && new Set(bundle.runs.map(item => item.run_id_sha256)).size === 3
    && new Set(bundle.runs.map(item => item.raw_artifact?.sha256)).size === 3
    && bundle.runs.every(item => [1, 2, 3].includes(item.run_index)
      && sourceMatches(item.source, head) && item.matrix_cell_id === 'protected_macos_arm64_metal')
    && frozen.status === 'frozen' && frozen.freeze_record?.selection_source_commit === head.commit
    && frozen.freeze_record.selection_source_tree === head.tree
    && frozen.freeze_record.calibration_bundle_sha256 === manifest.bundle.sha256,
  'calibration must contain three distinct exact-source protected Metal runs');
  // The calibration owner checks measurements, accelerator policy and raw payload digests.
  // It is also used again with full source lineage before any generated commit is pushed.
  host.verifyCalibration(documents, run, artifact);
  return { artifact, documents, manifest, rows: bundle.runs.map(item => ({ ...row,
    lane: `metal-${item.run_index}`, identity: item.run_id_sha256,
    member: item.raw_artifact.name, member_sha256: item.raw_artifact.sha256 })) };
}

export function qualificationEvidence(host, run, head, version) {
  validateRun(run, { repository: host.repository, head, workflow: PACKAGED_WORKFLOW, successful: true });
  const jobs = host.jobs(run);
  requireJob(jobs, run, 'closeout');
  const rows = [];
  for (const { asset_target: target } of host.graph.workflow_policy.package_matrix) {
    requireJob(jobs, run, `packaged-proof / Build ${target}`);
    selectedArtifact(host, run, head, `codestory-candidate-archive-record-${target}`, `package-${target}`);
    // Qualification carries this driver; a normal platform run cannot stand in for it.
    selectedArtifact(host, run, head, `codestory-qualification-driver-${target}`, `qualification-driver-${target}`);
  }
  for (const [target, suffix, backend] of [
    ['macos-arm64', 'Packaged Apple Silicon Metal engine', 'MTL'],
    ['windows-x64', 'Packaged Windows Vulkan engine', 'Vulkan'],
  ]) {
    requireNestedJob(jobs, run, suffix);
    const label = target === 'macos-arm64' ? 'metal' : 'vulkan';
    const { artifact, row } = selectedArtifact(host, run, head, `${target}-${label}-proof-${version}-attempt-${run.run_attempt}`, `qualification-${target}`);
    const q = JSON.parse(host.readArtifact(artifact, ['qualification.json'])['qualification.json']);
    requireThat(q.status === 'pass' && q.tier === 'protected_hardware' && sourceMatches(q.source, head)
      && q.host?.target === target && q.host.policy === 'accelerated'
      && q.host.backend === backend && q.host.unplanned_suspend === false
      && q.timing?.constants_frozen_before_run === true,
    `qualification did not prove accelerated execution on ${target}`);
    rows.push(row);
  }
  return { rows };
}

export function closeoutEvidence(host, run, head, version, phase) {
  validateRun(run, { repository: host.repository, head,
    workflow: phase === 'pre_publish' ? RELEASE_WORKFLOW : AUTO_RELEASE_WORKFLOW, successful: true });
  requireNestedJob(host.jobs(run), run, phase === 'pre_publish'
    ? 'Authenticate pre-publish release cells' : 'Authenticate post-publish release cells');
  const stage = phase.replace('_', '-');
  const suffix = phase === 'post_publish' ? `-attempt-${run.run_attempt}` : '';
  const { artifact, row } = selectedArtifact(host, run, head, `release-closeout-${stage}-${version}-${head.commit}${suffix}`, phase);
  const names = [`${phase}/ledger.json`, `${phase}/summary.json`, `${phase}/producer-provenance.json`];
  const documents = host.readArtifact(artifact, names);
  const ledger = JSON.parse(documents[names[0]]);
  const summary = JSON.parse(documents[names[1]]);
  const graph = host.graph;
  const expected = deriveReleaseCells(graph, phase).map(cell => cell.id).sort();
  requireThat(ledger.schema === graph.closeout.ledger_schema && summary.schema === graph.closeout.summary_schema
    && ledger.phase === phase && summary.phase === phase && ledger.decision === 'accept' && summary.decision === 'accept'
    && ledger.version === version && summary.version === version
    && ledger.identity?.repository === host.repository && ledger.identity.commit === head.commit
    && ledger.identity.source_tree === head.tree && JSON.stringify(summary.identity) === JSON.stringify(ledger.identity)
    && ledger.graph_sha256 === releaseClaimGraphDigest(graph) && summary.graph_sha256 === ledger.graph_sha256
    && ledger.producer_provenance_sha256 === sha256(documents[names[2]])
    && summary.producer_provenance_sha256 === ledger.producer_provenance_sha256
    && Array.isArray(ledger.input_errors) && ledger.input_errors.length === 0
    && Array.isArray(summary.input_errors) && summary.input_errors.length === 0
    && Array.isArray(ledger.cells) && JSON.stringify(ledger.cells.map(cell => cell.id).sort()) === JSON.stringify(expected)
    && ledger.cells.every(cell => ['pass', 'pass_with_exception', 'withheld'].includes(cell.status))
    && summary.counts?.required === expected.length && summary.counts.failed === 0 && summary.counts.missing === 0,
  'release closeout is not an accepted exact-head complete ledger');
  validateReleaseCloseoutLedger({ graph, phase, ledger, summary });
  if (phase === 'post_publish') {
    const delivery = ledger.catalog_delivery;
    requireThat(graph.workflow_policy.catalog_delivery.states.some(state => state.id === delivery?.state && state.installer === delivery.installer)
      && JSON.stringify(delivery) === JSON.stringify(summary.catalog_delivery), 'catalog delivery is not authenticated by the closeout');
  }
  return { row, ledger, summary, artifact };
}
