import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse as parseYaml } from 'yaml';
import { loadReleaseClaimGraph } from '../codestory-release-claims.mjs';
import {
  MARKER, CONSTANT_SET, SHA, AUTO_RELEASE_WORKFLOW,
  requireThat, sha256, parseRecord, recordBody, sameHead,
} from './release-coordinator-contract.mjs';

const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const python = () => process.env.CODESTORY_PYTHON || 'python3';

function command(program, args, options = {}) {
  return execFileSync(program, args, { encoding: 'utf8', timeout: 90_000,
    maxBuffer: 64 * 1024 * 1024, stdio: ['pipe', 'pipe', 'pipe'], ...options });
}

export function createDefaultHost({ repository = 'TheGreenCedar/CodeStory', projectRoot = defaultRoot,
  runCommand = command, now = () => new Date() } = {}) {
  requireThat(/^[\w.-]+\/[\w.-]+$/u.test(repository), 'invalid release repository');
  const gh = (args, options) => runCommand('gh', args, options);
  const api = (endpoint, body) => JSON.parse(gh(['api', ...(body ? ['--method', 'POST'] : []),
    endpoint, ...(body ? ['--input', '-'] : [])], body ? { input: JSON.stringify(body) } : undefined));
  const patch = (endpoint, body) => JSON.parse(gh(['api', '--method', 'PATCH', endpoint, '--input', '-'], { input: JSON.stringify(body) }));
  const paged = (endpoint, field) => {
    const pages = JSON.parse(gh(['api', '--paginate', '--slurp', endpoint]));
    return pages.flatMap(page => field ? page[field] : page);
  };
  const base = `repos/${repository}`;
  let cache = new Map();
  const cached = (key, read) => { if (!cache.has(key)) cache.set(key, read()); return cache.get(key); };
  const git = (args, options = {}) => runCommand('git', ['-C', projectRoot, ...args], options).trim();
  const commit = sha => {
    requireThat(SHA.test(sha), 'invalid release commit');
    const row = api(`${base}/git/commits/${sha}`);
    requireThat(row.sha === sha && SHA.test(row.tree?.sha), 'GitHub returned another release commit');
    return { commit: sha, tree: row.tree.sha, parents: row.parents.map(parent => parent.sha) };
  };
  const fileAt = (head, file) => {
    requireThat(SHA.test(head), 'source file needs an exact commit');
    const row = api(`${base}/contents/${file}?ref=${head}`);
    requireThat(row.type === 'file' && row.encoding === 'base64', `missing source file ${file}`);
    return Buffer.from(row.content, 'base64').toString('utf8');
  };
  const withFiles = (documents, action) => {
    const root = mkdtempSync(path.join(os.tmpdir(), 'codestory-release-evidence-'));
    try {
      for (const [name, bytes] of Object.entries(documents)) {
        requireThat(path.basename(name) === name, 'local release evidence must be a root file');
        writeFileSync(path.join(root, name), bytes, { flag: 'wx', mode: 0o600 });
      }
      return action(root);
    } finally { rmSync(root, { recursive: true, force: true }); }
  };
  const host = {
    repository, projectRoot, now,
    graph: loadReleaseClaimGraph(projectRoot),
    begin() { cache = new Map(); },
    get actor() { return cached('actor', () => {
      const login = api('user').login;
      requireThat(typeof login === 'string' && login.length > 0, 'GitHub did not authenticate an operator identity');
      return login;
    }); },
    get heads() {
      return cached('heads', () => Object.fromEntries([['next', 'dev/codestory-next'], ['main', 'main']].map(([label, branch]) => {
        try {
          const ref = api(`${base}/git/ref/heads/${encodeURIComponent(branch)}`);
          return [label, commit(ref.object.sha)];
        } catch (error) {
          if (label === 'next' && /HTTP 404/u.test(String(error.stderr ?? error.message))) return [label, null];
          throw error;
        }
      })));
    },
    commit, fileAt,
    getRun(id) { return cached(`run:${id}`, () => api(`${base}/actions/runs/${id}`)); },
    jobs(run) { return cached(`jobs:${run.id}:${run.run_attempt}`, () => paged(`${base}/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs?per_page=100`, 'jobs')); },
    artifacts(run) { return cached(`artifacts:${run.id}`, () => paged(`${base}/actions/runs/${run.id}/artifacts?per_page=100`, 'artifacts')); },
    statuses(head) { return cached(`statuses:${head}`, () => paged(`${base}/commits/${head}/statuses?per_page=100`)); },
    runsFor(head) { return cached(`runs:${head}`, () => paged(`${base}/actions/runs?head_sha=${head}&per_page=100`, 'workflow_runs')); },
    load(number) {
      let issue = number;
      if (!issue) {
        const issues = JSON.parse(gh(['issue', 'list', '--repo', repository, '--state', 'open', '--search', 'coordinator in:title', '--json', 'number,title']));
        requireThat(issues.length === 1, 'select one release coordinator with --issue');
        issue = issues[0].number;
      }
      const comments = paged(`${base}/issues/${issue}/comments?per_page=100`);
      const matches = comments.map(comment => ({ comment, record: parseRecord(comment.body) })).filter(row => row.record);
      requireThat(matches.length === 1, `expected one coordinator record on issue #${issue}`);
      requireThat(matches[0].record.issue_number === Number(issue), 'coordinator record belongs to another issue');
      return matches[0].record;
    },
    createIssue(title, body) { return api(`${base}/issues`, { title, body }); },
    findCoordinator(version) {
      const title = `Release ${version} coordinator`;
      const rows = JSON.parse(gh(['issue', 'list', '--repo', repository, '--state', 'all', '--search', `"${title}" in:title`, '--limit', '100', '--json', 'number,title,state']));
      const matches = rows.filter(row => row.title === title);
      requireThat(matches.length <= 1, 'release coordinator ownership is ambiguous');
      return matches[0];
    },
    findIssue(marker) {
      const rows = JSON.parse(gh(['issue', 'list', '--repo', repository, '--state', 'all', '--search', `"${marker}" in:body`, '--limit', '100', '--json', 'number,body,state']));
      const matches = rows.filter(row => row.body.includes(marker));
      requireThat(matches.length <= 1, 'release issue ownership is ambiguous');
      return matches[0];
    },
    save(record) {
      const comments = paged(`${base}/issues/${record.issue_number}/comments?per_page=100`).filter(row => row.body?.includes(MARKER));
      requireThat(comments.length <= 1, 'release coordinator has duplicate durable records');
      const body = recordBody(record);
      if (comments.length) patch(`${base}/issues/comments/${comments[0].id}`, { body });
      else api(`${base}/issues/${record.issue_number}/comments`, { body });
    },
    readArtifact(artifact, members) {
      const key = `artifact-bytes:${artifact.id}:${artifact.digest}`;
      return cached(`${key}:${members.join(',')}`, () => {
        requireThat(artifact.size_in_bytes <= 64 * 1024 * 1024, 'release evidence artifact exceeds the download bound');
        const bytes = gh(['api', `${base}/actions/artifacts/${artifact.id}/zip`], { encoding: null });
        requireThat(`sha256:${sha256(bytes)}` === artifact.digest, 'Actions artifact bytes differ from their API digest');
        const root = mkdtempSync(path.join(os.tmpdir(), 'codestory-release-container-'));
        try {
          const archive = path.join(root, 'artifact.zip');
          writeFileSync(archive, bytes, { flag: 'wx', mode: 0o600 });
          return JSON.parse(runCommand(python(), [path.join(projectRoot, 'scripts/lib/read-release-artifact.py'),
            '--archive', archive, '--sha256', artifact.digest, ...members.flatMap(name => ['--member', name])]));
        } finally { rmSync(root, { recursive: true, force: true }); }
      });
    },
    verifyCalibration(documents, run, artifact) {
      return withFiles(documents, root => {
        const inputs = path.join(root, 'inputs');
        mkdirSync(inputs);
        for (const name of ['per-user-embedding-server-measurement-protocol.json', 'per-user-embedding-server-protocol.json', path.basename(CONSTANT_SET)]) {
          writeFileSync(path.join(inputs, name), fileAt(run.head_sha, `crates/codestory-llama-sys/${name}`), { flag: 'wx', mode: 0o600 });
        }
        return JSON.parse(runCommand(python(), ['-c',
          'import sys,json;from pathlib import Path;sys.path.insert(0,sys.argv[1]+"/.github/scripts");from packaged_agent_proof.measurement_protocol import load_server_measurement_contract;from packaged_agent_proof.calibration_verification import verify_calibration_bundle;contract=load_server_measurement_contract(Path(sys.argv[2])/"inputs/per-user-embedding-server-measurement-protocol.json");print(json.dumps(verify_calibration_bundle(Path(sys.argv[2])/"calibration-bundle.json",contract,compare_frozen_constant_set=False,expected_producer_run_id=sys.argv[3],expected_producer_artifact=sys.argv[4])))',
          projectRoot, root, String(run.id), artifact.name]));
      });
    },
    preflight(head, phase, dispatch) {
      requireThat(git(['rev-parse', 'HEAD']) === head.commit
        && git(['status', '--porcelain', '--untracked-files=all']) === '', 'release dispatch requires a clean local checkout of the exact pushed proof head');
      const version = /\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/u.exec(fileAt(head.commit, 'crates/codestory-cli/Cargo.toml'))?.[1];
      requireThat(version === dispatch.version, 'release version differs from the exact source head');
      const constants = JSON.parse(fileAt(head.commit, CONSTANT_SET));
      if (['source_stabilization', 'calibration'].includes(phase)) requireThat(constants.status === 'unfrozen' && constants.freeze_record === null, 'calibration source still carries a frozen receipt; reset it before source stabilization');
      else requireThat(constants.status === 'frozen' && SHA.test(constants.freeze_record?.selection_source_commit), 'frozen proof head has no calibrated constant-set identity');
      const workflow = parseYaml(fileAt(head.commit, dispatch.workflow));
      const declared = workflow.on?.workflow_dispatch?.inputs ?? {};
      for (const [key, value] of Object.entries(dispatch.inputs)) {
        requireThat(declared[key], `workflow does not accept input ${key}`);
        if (declared[key].type === 'choice') requireThat(declared[key].options.includes(value), `workflow does not accept ${key}=${value}`);
      }
      for (const [key, value] of Object.entries(declared)) requireThat(!value.required || value.default !== undefined || dispatch.inputs[key] !== undefined, `workflow requires ${key}`);
      const needed = phase === 'source_stabilization' || phase === 'frozen_candidate_acceptance' ? ['windows-x64-vulkan']
        : phase === 'calibration' ? ['macos-arm64-metal']
          : phase === 'qualification' ? ['macos-arm64-metal', 'windows-x64-vulkan'] : [];
      // The release workflow owns policy-permitted host withholds. Do not turn those
      // into an extra driver gate; require availability only for non-withholdable lanes.
      if (needed.length) host.requireHeartbeats(needed);
      return { checked_at: now().toISOString(), head, version, workflow: dispatch.workflow, required_hosts: needed };
    },
    requireHeartbeats(ids) {
      const policy = host.graph.non_claim_policy;
      const runs = api(`${base}/actions/workflows/${path.basename(policy.reservation.heartbeat.workflow)}/runs?per_page=10`).workflow_runs;
      const cutoff = now().getTime() - policy.reservation.heartbeat.tolerance_minutes * 60_000;
      for (const id of ids) {
        const target = policy.hosts.find(row => row.id === id);
        let proven = false;
        for (const run of runs) {
          if (run.head_repository?.full_name !== repository || Date.parse(run.created_at) < cutoff) continue;
          const job = host.jobs(run).find(row => row.name === target.heartbeat_job_name);
          if (job?.status === 'completed' && job.conclusion === 'success' && Date.parse(job.completed_at) >= cutoff
            && target.runner_labels.every(label => job.labels?.includes(label))) { proven = true; break; }
        }
        requireThat(proven, `protected runner ${id} has no fresh authenticated heartbeat; restore its heartbeat before dispatch`);
      }
    },
    dispatch(request) {
      const result = api(`${base}/actions/workflows/${path.basename(request.workflow)}/dispatches`,
        { ref: request.ref, inputs: request.inputs, return_run_details: true });
      requireThat(Number.isSafeInteger(result.workflow_run_id) && result.workflow_run_id > 0,
        'workflow dispatch returned no run identity; reconcile the durable dispatch intent before retrying');
      return result.workflow_run_id;
    },
    cancel(run, force = false) {
      gh(['api', '--method', 'POST', `${base}/actions/runs/${run.id}/${force ? 'force-cancel' : 'cancel'}`]);
    },
    freeze(record, calibration) {
      const C = { commit: record.heads.C, tree: record.heads.C_tree };
      requireThat(sameHead(host.heads.next, C) && git(['rev-parse', 'HEAD']) === C.commit
        && git(['status', '--porcelain', '--untracked-files=all']) === '', 'freeze requires the clean accepted calibration source');
      const marker = `codestory-generated-freeze:${record.receipt.version}:${C.commit}`;
      const branch = `codex/release-${record.receipt.version}-freeze-${C.commit.slice(0, 12)}`;
      let issue = host.findIssue(marker);
      if (!issue) issue = host.createIssue(`Freeze ${record.receipt.version} runtime constants`,
        `Apply the authenticated constant set from calibration run ${calibration.rows[0].run_id}, attempt ${calibration.rows[0].attempt}, as the sole child change of ${C.commit}.\n\nRefs #${record.issue_number}.\n\n<!-- ${marker} -->`);
      let F;
      const existing = git(['ls-remote', '--heads', 'origin', `refs/heads/${branch}`]).split(/\s+/u)[0];
      if (existing) {
        git(['fetch', 'origin', branch]);
        F = { commit: existing, tree: git(['rev-parse', `${existing}^{tree}`]) };
      } else {
        const root = mkdtempSync(path.join(os.tmpdir(), 'codestory-release-freeze-'));
        const checkout = path.join(root, 'source');
        try {
          git(['-c', 'core.hooksPath=/dev/null', 'worktree', 'add', '--detach', checkout, C.commit]);
          writeFileSync(path.join(checkout, CONSTANT_SET), calibration.documents['per-user-embedding-server-constant-set.json']);
          const localGit = args => runCommand('git', ['-C', checkout, '-c', 'core.hooksPath=/dev/null', ...args]).trim();
          requireThat(localGit(['diff', '--name-only']) === CONSTANT_SET, 'freeze must change only the generated constant set');
          localGit(['add', '--', CONSTANT_SET]);
          localGit(['commit', '-m', `freeze ${record.receipt.version} runtime constants`]);
          F = { commit: localGit(['rev-parse', 'HEAD']), tree: localGit(['rev-parse', 'HEAD^{tree}']) };
          host.verifyFrozen(F, C, calibration, checkout);
          localGit(['push', 'origin', `${F.commit}:refs/heads/${branch}`]);
        } finally {
          try { git(['worktree', 'remove', '--force', checkout]); } finally { rmSync(root, { recursive: true, force: true }); }
        }
      }
      host.verifyFrozen(F, C, calibration);
      const pulls = api(`${base}/pulls?state=all&head=${repository.split('/')[0]}:${branch}&base=dev/codestory-next&per_page=100`);
      requireThat(pulls.length <= 1, 'generated freeze pull request ownership is ambiguous');
      const pr = pulls[0] ?? api(`${base}/pulls`, { title: `Freeze CodeStory ${record.receipt.version} runtime constants`, head: branch,
        base: 'dev/codestory-next', draft: true,
        body: `This generated change freezes the three authenticated Metal calibration runs from ${C.commit}. Only the constant-set file changes. Source stabilization is retained from C; frozen acceptance must not repeat the full workspace proof.\n\nCloses #${issue.number}. Refs #${record.issue_number}.\n\n<!-- ${marker} -->` });
      requireThat(pr.head.sha === F.commit && pr.base.ref === 'dev/codestory-next', 'generated freeze PR head changed');
      return { ...F, branch, issue: issue.number, pull_request: pr.number };
    },
    verifyFrozen(F, C, calibration, checkout) {
      try { git(['cat-file', '-e', `${F.commit}^{commit}`]); }
      catch { git(['fetch', 'origin', F.commit]); }
      requireThat(git(['rev-list', '--parents', '-n', '1', F.commit]) === `${F.commit} ${C.commit}`
        && git(['diff', '--name-only', C.commit, F.commit]) === CONSTANT_SET
        && git(['show', `${F.commit}:${CONSTANT_SET}`]) === calibration.documents['per-user-embedding-server-constant-set.json'].trim(),
      'generated freeze is not the exact constant-only child of the calibrated source');
      let owned;
      if (!checkout) {
        owned = mkdtempSync(path.join(os.tmpdir(), 'codestory-verify-freeze-'));
        checkout = path.join(owned, 'source');
        git(['-c', 'core.hooksPath=/dev/null', 'worktree', 'add', '--detach', checkout, F.commit]);
      }
      try {
        return withFiles(calibration.documents, root => runCommand(python(), [path.join(checkout, '.github/scripts/check-calibration-release-lineage.py'),
          '--repo', checkout, '--expected-sha', F.commit, '--calibration-bundle', path.join(root, 'calibration-bundle.json'),
          '--artifact-constant-set', path.join(root, 'per-user-embedding-server-constant-set.json'),
          '--expected-producer-run-id', String(calibration.rows[0].run_id), '--expected-producer-run-attempt', String(calibration.rows[0].attempt),
          '--expected-producer-artifact', calibration.artifact.name]));
      } finally { if (owned) { try { git(['worktree', 'remove', '--force', checkout]); } finally { rmSync(owned, { recursive: true, force: true }); } } }
    },
    pullRequest(number) { return api(`${base}/pulls/${number}`); },
    requireChecks(number) {
      const rows = JSON.parse(gh(['pr', 'checks', String(number), '--repo', repository, '--json', 'name,bucket']));
      requireThat(rows.length > 0 && rows.every(row => ['pass', 'skipping'].includes(row.bucket)), `PR #${number} has unfinished or failed checks`);
    },
    integrateFrozen(record) {
      const pr = host.pullRequest(record.freeze.pull_request);
      requireThat(pr.head.sha === record.heads.F && pr.base.ref === 'dev/codestory-next'
        && host.heads.next.commit === record.heads.C, 'frozen integration head moved');
      host.requireChecks(pr.number);
      if (pr.draft) gh(['pr', 'ready', String(pr.number), '--repo', repository]);
      patch(`${base}/git/refs/heads/dev%2Fcodestory-next`, { sha: record.heads.F, force: false });
    },
    promotionPr(record) {
      const marker = `codestory-release-promotion:${record.receipt.version}:${record.heads.F}`;
      const pulls = api(`${base}/pulls?state=all&head=${repository.split('/')[0]}:dev/codestory-next&base=main&per_page=100`);
      const found = pulls.filter(pr => pr.body?.includes(marker));
      requireThat(found.length <= 1 && !pulls.some(pr => pr.state === 'open' && !pr.body?.includes(marker)), 'another main promotion owns the branch');
      return found[0] ?? api(`${base}/pulls`, { title: `Release CodeStory ${record.receipt.version}`, head: 'dev/codestory-next', base: 'main', draft: true,
        body: `Promote the qualified ${record.receipt.version} candidate ${record.heads.F} with a tree-preserving merge. Pre-publish proof: run ${record.evidence.pre_publish.row.run_id}, attempt ${record.evidence.pre_publish.row.attempt}. Combined maintainer approval is required before merging.\n\nRefs #${record.issue_number}.\n\n<!-- ${marker} -->` });
    },
    promote(record) {
      const pr = host.pullRequest(record.promotion_pr);
      requireThat(pr.state === 'open' && pr.head.sha === record.heads.F && pr.base.ref === 'main'
        && pr.base.sha === record.approval.main_commit && host.heads.main.commit === record.approval.main_commit
        && host.heads.next.commit === record.heads.F, 'promotion PR no longer names the approved frozen head and main base');
      const comparison = api(`${base}/compare/${record.approval.main_commit}...${record.heads.F}`);
      requireThat(comparison.merge_base_commit?.sha === record.approval.main_commit
        && ['ahead', 'identical'].includes(comparison.status), 'main is not an ancestor of the qualified frozen candidate');
      host.requireChecks(pr.number);
      if (pr.draft) gh(['pr', 'ready', String(pr.number), '--repo', repository]);
      gh(['pr', 'merge', String(pr.number), '--repo', repository, '--merge', '--match-head-commit', record.heads.F]);
      const merged = host.pullRequest(pr.number);
      if (merged.merged) {
        const P = commit(merged.merge_commit_sha);
        requireThat(P.tree === record.heads.F_tree && P.parents.includes(record.heads.F), 'promotion did not preserve the qualified tree');
      }
      return merged;
    },
    release(version) {
      return JSON.parse(gh(['release', 'view', `v${version}`, '--repo', repository, '--json', 'tagName,isDraft,isPrerelease,targetCommitish,publishedAt,url,assets']));
    },
    tag(version) {
      let object = api(`${base}/git/ref/tags/v${version}`).object;
      if (object.type === 'tag') object = api(`${base}/git/tags/${object.sha}`).object;
      requireThat(object.type === 'commit', 'release tag does not resolve to a commit');
      return object.sha;
    },
    reconcileDev(P) {
      requireThat(host.heads.main.commit === P.commit, 'main moved before release reconciliation');
      if (host.heads.next) patch(`${base}/git/refs/heads/dev%2Fcodestory-next`, { sha: P.commit, force: false });
      else api(`${base}/git/refs`, { ref: 'refs/heads/dev/codestory-next', sha: P.commit });
    },
    autoRelease(P) {
      const rows = host.runsFor(P.commit).filter(run => run.path === AUTO_RELEASE_WORKFLOW && run.event === 'push');
      return rows.sort((a, b) => b.id - a.id)[0];
    },
  };
  return host;
}
