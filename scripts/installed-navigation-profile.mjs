import { createHash, randomBytes } from 'node:crypto';
import { cp, mkdir, readFile, writeFile, stat, realpath, readdir, lstat } from 'node:fs/promises';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { parseArgs } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { directoryDigest } from '../.github/scripts/install-codestory-marketplace-proof.mjs';

const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const save = (file, value) => writeFile(file, `${JSON.stringify(value, null, 2)}\n`);
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };
const safeId = value => typeof value === 'string' && /^[a-zA-Z0-9_-]+$/.test(value);

export function validateManifest(manifest) {
  requireThat(manifest.schema_version === 1, 'unsupported navigation manifest');
  requireThat(manifest.model?.name === 'gpt-5.6-terra' && manifest.model?.reasoning_effort === 'low', 'navigation model must be Terra low');
  requireThat(Array.isArray(manifest.repositories) && Array.isArray(manifest.tasks) && Array.isArray(manifest.sessions), 'manifest needs repositories, tasks and sessions');
  const repos = new Map(manifest.repositories.map(repo => [repo.id, repo]));
  const tasks = new Map(manifest.tasks.map(task => [task.id, task]));
  requireThat(repos.size === manifest.repositories.length && tasks.size === manifest.tasks.length, 'duplicate repository or task');
  for (const repo of repos.values()) requireThat(/^[a-f0-9]{40}$/.test(repo.commit) && /^[a-f0-9]{40}$/.test(repo.tree) && path.isAbsolute(repo.seed_clone), 'repository needs pinned commit, tree and seed clone');
  for (const task of tasks.values()) requireThat(repos.has(task.repository_id) && typeof task.prompt === 'string' && task.prompt.length > 0 && ['read_only', 'change'].includes(task.effect_mode), 'invalid navigation task');
  requireThat(Array.isArray(manifest.arms) && manifest.arms.every(safeId) && new Set(manifest.arms).size === manifest.arms.length && manifest.arms.includes('native'), 'unique arms including native required');
  const seen = new Set();
  const ids = new Set();
  manifest.sessions.forEach((row, index) => {
    requireThat(row.sequence === index + 1 && safeId(row.session_id) && !ids.has(row.session_id), 'invalid sequence or duplicate session id');
    ids.add(row.session_id);
    requireThat(tasks.has(row.task_id) && manifest.arms.includes(row.arm) && Number.isInteger(row.repeat) && row.repeat > 0 && row.repeat <= manifest.repeats, 'invalid session');
    const key = `${row.task_id}:${row.arm}:${row.repeat}`;
    requireThat(!seen.has(key), 'duplicate task/arm/repeat'); seen.add(key);
  });
  requireThat(seen.size === tasks.size * manifest.arms.length * manifest.repeats, 'incomplete navigation schedule');
  if (manifest.profile === 'codestory-0176-navigation-maintenance') {
    requireThat(repos.size === 3 && tasks.size === 6 && manifest.repeats === 2 && manifest.sessions.length === 36, 'maintenance panel requires 3 repositories, 6 tasks, 36 sessions');
    for (const category of ['discovery', 'relationship', 'edit_refresh']) requireThat([...tasks.values()].filter(task => task.category === category).length === 2, 'maintenance category allocation changed');
  }
  return { repos, tasks };
}

export function validateTemplateConfig(config, armName, arm) {
  let section = null;
  for (const raw of config.split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    requireThat(armName !== 'native', 'native template contains host configuration');
    if (line.startsWith('[')) {
      if (line === `[marketplaces.${arm.marketplace_name}]`) section = 'marketplace';
      else if (line === `[plugins."codestory@${arm.marketplace_name}"]`) section = 'plugin';
      else throw new Error(`unexpected installation configuration section: ${line}`);
    } else {
      requireThat((section === 'plugin' && line === 'enabled = true')
        || (section === 'marketplace' && /^(source_type|source|ref) = "[^"\n]*"$/.test(line)), 'installation configuration contains a policy override');
    }
  }
}

export function validateInventory(installed, armName, arm) {
  requireThat(Array.isArray(installed), 'missing plugin inventory');
  if (armName === 'native') return requireThat(installed.length === 0, 'native arm contains plugins');
  requireThat(installed.length === 1, 'expected one installed CodeStory plugin');
  const plugin = installed[0];
  requireThat(plugin.name === 'codestory' && plugin.pluginId === `codestory@${arm.marketplace_name}`
    && plugin.marketplaceName === arm.marketplace_name && plugin.version === arm.version
    && plugin.installed === true && plugin.enabled === true, 'Codex did not expose the expected enabled plugin');
}

export function validateRuntime(result, arm) {
  const publication = result?._meta?.codestory_publication;
  const runtime = publication?.contract_runtime;
  requireThat(publication?.schema_version === arm.schema_version && publication?.served_from === 'complete_publication'
    && typeof publication?.core_publication?.generation_id === 'string'
    && runtime?.cli_sha256 === arm.runtime_sha256 && runtime?.cli_source === arm.runtime_source
    && runtime?.cli_version === arm.version && runtime?.plugin_cli_version === arm.version
    && runtime?.plugin_version === arm.version && runtime?.pinned_pair_matches === true
    && runtime?.known_override_skew_channel === false, 'typed installed runtime/publication identity mismatch');
}

export function validateSourceRead(result, project, marker) {
  const ranges = result?.structuredContent?.ranges;
  requireThat(Array.isArray(ranges) && ranges.length === 1, 'source-read canary needs exactly one range');
  const range = ranges[0];
  requireThat(range.path === path.join(project, 'index.js') && range.start_line === 1 && range.end_line === 1
    && range.snippet_truncated === false && typeof range.snippet === 'string'
    && range.snippet.includes(`export function navigationCanary() { return "${marker}"; }`)
    && !range.snippet.includes(marker === 'BEFORE_REFRESH' ? 'AFTER_REFRESH' : 'BEFORE_REFRESH'), 'source-read canary returned missing, mixed or stale source');
}

export function navigationCommand(codex, project, output) {
  return { command: codex, args: ['exec', '--disable', 'remote_plugin', '--model', 'gpt-5.6-terra', '--config', 'model_reasoning_effort="low"', '--sandbox', 'workspace-write', '--cd', project, '--json', '--output-last-message', output, '-'] };
}

export function navigationEnvironment(parent, root, nonce) {
  const env = Object.fromEntries(Object.entries(parent).filter(([key]) => !/^(CODEX_|CODESTORY_|CLAUDE_|COPILOT_|PLUGIN_|OPENAI_API_KEY$)/.test(key)));
  return { ...env, HOME: path.join(root, 'home'), USERPROFILE: path.join(root, 'home'),
    CODEX_HOME: path.join(root, 'codex'), TMPDIR: path.join(root, 'tmp'),
    XDG_CACHE_HOME: path.join(root, 'home/.cache'), XDG_CONFIG_HOME: path.join(root, 'home/.config'),
    CODESTORY_CACHE_ROOT: path.join(root, 'cache'), CODESTORY_STDIO_CACHE_ROOT: path.join(root, 'cache'),
    CODESTORY_EMBED_ALLOW_CPU: '0',
    CODESTORY_EMBED_QUALIFICATION_DIR: path.join(root, 'qualification'),
    CODESTORY_EMBED_QUALIFICATION_NONCE: nonce };
}

async function checkedProcess(runProcess, command, args, options) {
  const result = await runProcess(command, args, { timeoutMs: 90_000, ...options });
  requireThat(result.status === 'pass', `${path.basename(command)} failed: ${result.stderr}`);
  return result.stdout.trim();
}

async function prepareSession(row, manifest, installation, output, codex, helpers) {
  const task = manifest.tasks.find(task => task.id === row.task_id);
  const repo = manifest.repositories.find(repo => repo.id === task.repository_id);
  const root = path.join(output, row.session_id);
  await mkdir(root); // Existing attempts are never overwritten or silently resumed.
  const env = navigationEnvironment(process.env, root, randomBytes(32).toString('hex'));
  for (const folder of ['home', 'codex', 'tmp', 'cache', 'qualification']) await mkdir(path.join(root, folder), { mode: 0o700 });
  const project = path.join(root, 'repository');
  const git = args => checkedProcess(helpers.runProcess, 'git', ['-c', 'core.hooksPath=/dev/null', ...args], { env });
  requireThat(await git(['-C', repo.seed_clone, 'rev-parse', repo.commit]) === repo.commit, 'seed commit mismatch');
  await git(['clone', '--no-hardlinks', '--no-checkout', repo.seed_clone, project]);
  await git(['-C', project, 'checkout', '--detach', repo.commit]);
  requireThat(await git(['-C', project, 'rev-parse', 'HEAD^{tree}']) === repo.tree, 'checkout tree mismatch');
  const arm = installation.arms[row.arm];
  requireThat(arm && path.isAbsolute(arm.codex_home_template), 'missing isolated installation template');
  // Copy only installation-owned material; reject host instructions before copying.
  for (const entry of await readdir(arm.codex_home_template, { withFileTypes: true })) {
    requireThat(['config.toml', 'plugins', 'tmp', '.tmp'].includes(entry.name) && !entry.isSymbolicLink(), `unexpected template entry: ${entry.name}`);
  }
  const config = await readFile(path.join(arm.codex_home_template, 'config.toml'), 'utf8');
  validateTemplateConfig(config, row.arm, arm);
  await writeFile(path.join(env.CODEX_HOME, 'config.toml'), config);
  let pluginRoot = null;
  if (row.arm !== 'native') {
    requireThat(safeId(arm.marketplace_name) && /^\d+\.\d+\.\d+$/.test(arm.version), 'invalid installed identity');
    const expectedPath = `plugins/cache/${arm.marketplace_name}/codestory/${arm.version}`;
    requireThat(arm.plugin_relative_path === expectedPath, 'unexpected installed package path');
    const sourcePlugin = path.join(arm.codex_home_template, expectedPath);
    requireThat(!(await lstat(sourcePlugin)).isSymbolicLink(), 'template plugin is a symlink');
    requireThat((await realpath(sourcePlugin)).startsWith(`${await realpath(arm.codex_home_template)}${path.sep}`), 'template plugin escaped isolated home');
    requireThat(directoryDigest(sourcePlugin) === arm.package_sha256, 'installed package drift');
    pluginRoot = path.join(env.CODEX_HOME, expectedPath);
    await cp(sourcePlugin, pluginRoot, { recursive: true, dereference: false });
    requireThat(directoryDigest(pluginRoot) === arm.package_sha256, 'copied installed package drift');
    env.CODESTORY_PLUGIN_DATA = path.join(env.CODEX_HOME, 'plugins/data', `codestory-${arm.marketplace_name}`);
    await mkdir(env.CODESTORY_PLUGIN_DATA, { recursive: true, mode: 0o700 });
    requireThat(/^[a-f0-9]{40}$/.test(arm.source_commit) && /^[a-f0-9]{64}$/.test(arm.runtime_sha256)
      && [2, 3].includes(arm.schema_version) && ['managed', 'local_dev_override'].includes(arm.runtime_source), 'missing source/runtime identity');
    if (arm.release_directory) env.CODESTORY_PLUGIN_RELEASE_DIR = arm.release_directory;
  }
  if (installation.auth_file) await cp(installation.auth_file, path.join(env.CODEX_HOME, 'auth.json'));
  const listingRaw = await checkedProcess(helpers.runProcess, codex, ['plugin', 'list', '--json', '--disable', 'remote_plugin'], { env });
  const listing = JSON.parse(listingRaw);
  validateInventory(listing.installed, row.arm, arm);
  const effective = { schema_version: 1, session: row, model: manifest.model, timeout_ms: 600_000,
    sandbox: 'workspace-write', host_features: { remote_plugin: false }, project, source_commit: repo.commit, source_tree: repo.tree,
    package: arm, configuration: config, plugin_listing: listing, environment: Object.fromEntries(Object.entries(env).filter(([key]) => /^(HOME|USERPROFILE|TMPDIR|CODEX_HOME|CODESTORY_)/.test(key))) };
  await save(path.join(root, 'effective-config.json'), effective);
  return { root, env, project, pluginRoot, task, arm, effective };
}

async function operationCanary(session, helpers) {
  const { root, env, pluginRoot, arm } = session;
  if (!pluginRoot) return { status: 'pass', surface: 'native', operations: ['git clone', 'git checkout', 'plugin inventory'] };
  const project = path.join(root, 'operation-canary');
  await mkdir(project);
  await writeFile(path.join(project, 'index.js'), 'export function navigationCanary() { return "BEFORE_REFRESH"; }\n');
  await checkedProcess(helpers.runProcess, 'git', ['-c', 'core.hooksPath=/dev/null', 'init', project], { env });
  const transcript = [];
  const channel = helpers.createSequencedStdioSession(process.execPath, [path.join(pluginRoot, 'scripts/codestory-mcp.cjs')], { env, timeoutMs: 90_000, onNotification: notification => transcript.push({ notification }) });
  let id = 0;
  const request = async (method, params) => {
    const sent = { jsonrpc: '2.0', id: ++id, method, params };
    const received = await channel.request(sent); transcript.push({ sent, received });
    requireThat(!received.error && !received.result?.isError, `canary ${method} failed: ${JSON.stringify(received)}`);
    return received.result;
  };
  try {
    const init = await request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'installed-navigation-canary', version: '1' } });
    requireThat(init.serverInfo?.version === arm.version, 'installed runtime version mismatch');
    channel.send({ jsonrpc: '2.0', method: 'notifications/initialized' });
    const catalog = await request('tools/list', {});
    requireThat(Array.isArray(catalog.tools) && catalog.tools.every(tool => tool.inputSchema?.type === 'object'), 'invalid tool schemas');
    requireThat(sha(JSON.stringify(catalog.tools)) === arm.tools_sha256, 'installed tool catalog mismatch');
    const call = async (name, args) => {
      const deadline = performance.now() + 75_000;
      while (true) {
        const result = await request('tools/call', { name, arguments: { project, ...args } });
        const state = result.structuredContent;
        if (!['preparing', 'updating'].includes(state?.state)) { validateRuntime(result, arm); return result; }
        requireThat(performance.now() < deadline, 'managed preparation exceeded canary budget');
        await delay(Math.min(5000, Math.max(100, state.retry_after_ms ?? 1000)));
      }
    };
    await call('files', {});
    const before = await call('snippet', { path: 'index.js', start_line: 1, end_line: 1 });
    validateSourceRead(before, project, 'BEFORE_REFRESH');
    await writeFile(path.join(project, 'index.js'), 'export function navigationCanary() { return "AFTER_REFRESH"; }\n');
    await call('files', {});
    const after = await call('snippet', { path: 'index.js', start_line: 1, end_line: 1 });
    validateSourceRead(after, project, 'AFTER_REFRESH');
    await channel.close();
    return { status: 'pass', surface: 'installed-plugin', catalog_sha256: arm.tools_sha256, runtime_sha256: arm.runtime_sha256 };
  } finally {
    await channel.stop();
    await save(path.join(root, 'canary-transcript.json'), transcript);
    await writeFile(path.join(root, 'canary-stderr.txt'), channel.stderr());
  }
}

export async function runInstalledNavigation(argv, helpers) {
  const { values } = parseArgs({ args: argv, options: {
    'installed-navigation': { type: 'boolean' }, manifest: { type: 'string' }, installations: { type: 'string' },
    out: { type: 'string' }, codex: { type: 'string', default: 'codex' },
    'preflight-only': { type: 'boolean', default: false },
  }, strict: true });
  requireThat(values.manifest && values.installations && values.out, '--manifest, --installations and --out required');
  const manifestBytes = await readFile(values.manifest);
  const manifest = JSON.parse(manifestBytes);
  validateManifest(manifest);
  const installationBytes = await readFile(values.installations);
  const installation = JSON.parse(installationBytes);
  requireThat(installation.manifest_sha256 === sha(manifestBytes), 'installation receipt targets another manifest');
  requireThat(installation.budget?.max_sessions >= manifest.sessions.length && installation.budget?.max_wall_ms >= manifest.sessions.length * 600_000, 'frozen budget must cover all ten-minute sessions');
  const output = path.resolve(values.out);
  await mkdir(output); // No overwrite/resume: failures stay in their original attempt.
  const runStart = performance.now();
  const remainingMs = () => Math.min(installation.budget.max_wall_ms - (performance.now() - runStart), installation.budget.deadline_utc ? Date.parse(installation.budget.deadline_utc) - Date.now() : Infinity);
  const allowance = requested => {
    const available = Math.floor(Math.min(requested, remainingMs()));
    requireThat(available > 0, 'navigation elapsed budget exhausted');
    return available;
  };
  const bounded = { ...helpers,
    runProcess: async (command, args, options = {}) => {
      const result = await helpers.runProcess(command, args, { ...options, timeoutMs: allowance(options.timeoutMs ?? 90_000) });
      return result;
    },
    createSequencedStdioSession: (command, args, options) => {
      const channel = helpers.createSequencedStdioSession(command, args, { ...options, timeoutMs: allowance(options.timeoutMs) });
      return { ...channel, request: async request => { allowance(1); const result = await channel.request(request); return result; } };
    },
  };
  const prepared = [];
  const results = manifest.sessions.map(row => ({ ...row, status: 'not_run', model_attempted: false, whole_task_wall_ms: 0, telemetry_complete: false }));
  const persist = () => save(path.join(output, 'summary.json'), { schema_version: 1, profile: manifest.profile,
    manifest_sha256: sha(manifestBytes), installations_sha256: sha(installationBytes),
    expected_sessions: manifest.sessions.length, recorded_sessions: results.filter(row => row.status !== 'not_run').length, model_attempts: results.filter(row => row.model_attempted).length,
    results, preflight: prepared, acceptance: 'pending_independent_evaluator', elapsed_ms: performance.now() - runStart });
  await persist();
  try {
    // Every arm must pass deterministic checks before any model starts.
    for (const armName of manifest.arms) {
      allowance(1);
      const example = manifest.sessions.find(row => row.arm === armName);
      const session = await prepareSession({ ...example, session_id: `preflight-${armName}` }, manifest, installation, output, values.codex, bounded);
      const canary = await operationCanary(session, bounded);
      allowance(1);
      prepared.push({ arm: armName, canary, effective_config_sha256: sha(await readFile(path.join(session.root, 'effective-config.json'))) });
      await save(path.join(output, 'preflight.json'), prepared);
    }
  } catch (error) {
    prepared.push({ status: 'fail', error: error.message });
    for (const row of results) { row.status = 'not_run_preflight_failed'; row.error = error.message; }
    await save(path.join(output, 'preflight.json'), prepared); await persist(); throw error;
  }
  if (values['preflight-only']) { await persist(); return; }
  for (const [index, row] of manifest.sessions.entries()) {
    const started = performance.now();
    const root = path.join(output, row.session_id);
    let phase = 'preparation';
    const result = { ...row, status: 'preparation_failed', model_attempted: false, telemetry_complete: false,
      usage: { input_tokens: null, output_tokens: null, total_tokens: null }, grading: 'pending_independent_evaluator' };
    let stdout = ''; let stderr = '';
    try {
      allowance(1);
      const session = await prepareSession(row, manifest, installation, output, values.codex, bounded);
      const invocation = navigationCommand(values.codex, session.project, path.join(root, 'answer.md'));
      result.preparation_ms = performance.now() - started;
      await writeFile(path.join(root, 'prompt.txt'), session.task.prompt);
      allowance(1); // No model process may start after preparation consumes the budget.
      phase = 'model'; result.model_attempted = true;
      const run = await bounded.runProcess(invocation.command, invocation.args, { cwd: session.project, env: session.env,
        stdin: session.task.prompt, timeoutMs: 600_000, killProcessTree: true, maxOutputBytes: 64 * 1024 * 1024 });
      stdout = run.stdout; stderr = run.stderr;
      const events = []; const malformed = [];
      for (const line of stdout.split('\n').filter(Boolean)) { try { events.push(JSON.parse(line)); } catch { malformed.push(line); } }
      const usage = helpers.extractUsage(events);
      Object.assign(result, { status: run.status, exit_code: run.exitCode, timed_out: run.timedOut, usage,
        telemetry_complete: malformed.length === 0 && ['input_tokens', 'output_tokens', 'total_tokens'].every(key => Number.isFinite(usage[key]) && usage[key] >= 0)
          && events.some(event => event.type === 'turn.completed'),
        malformed_lines: malformed.length, analysis: helpers.analyzeTranscript(events, session.project) });
    } catch (error) {
      result.status = remainingMs() <= 0 ? 'budget_exhausted' : `${phase}_failed`;
      result.error = error.message; stderr += `${error.message}\n`;
    } finally {
      result.whole_task_wall_ms = performance.now() - started;
      result.transcript_sha256 = sha(stdout);
      await mkdir(root, { recursive: true });
      await writeFile(path.join(root, 'transcript.jsonl'), stdout);
      await writeFile(path.join(root, 'stderr.txt'), stderr);
      await save(path.join(root, 'result.json'), result);
      results[index] = result; await persist();
    }
    // Failed attempts and unstarted budget rows retain their places in the denominator.
  }
}
