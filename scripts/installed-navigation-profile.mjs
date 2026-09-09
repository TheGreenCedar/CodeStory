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
const HOST_ENVIRONMENT = ['HOME', 'USERPROFILE', 'TMPDIR', 'CODEX_HOME', 'XDG_CACHE_HOME', 'XDG_CONFIG_HOME',
  'CODESTORY_PLUGIN_DATA', 'CODESTORY_PLUGIN_RELEASE_DIR', 'CODESTORY_CACHE_ROOT', 'CODESTORY_STDIO_CACHE_ROOT',
  'CODESTORY_EMBED_ALLOW_CPU', 'CODESTORY_EMBED_QUALIFICATION_DIR', 'CODESTORY_EMBED_QUALIFICATION_NONCE'];
const canonical = value => Array.isArray(value) ? value.map(canonical)
  : value && typeof value === 'object' ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;

export function installedHostConfig(server, pluginRoot, arm, env) {
  requireThat(server?.command === 'node' && Array.isArray(server.args) && server.args.length === 1
    && server.args[0] === './scripts/codestory-mcp.cjs' && server.cwd === '.'
    && Object.keys(server.env ?? {}).length === 0 && !(server.env_vars?.length), 'unsupported installed launcher transport');
  const allowed = new Set(['command', 'args', 'cwd', 'env', 'tool_timeout_sec', 'startup_timeout_sec']);
  requireThat(Object.keys(server).every(key => allowed.has(key)), 'unrecognized installed launcher setting');
  const lines = [`[plugins."codestory@${arm.marketplace_name}".mcp_servers.codestory]`, 'enabled = false', '',
    '[mcp_servers.codestory]', `command = ${JSON.stringify(server.command)}`,
    `args = ${JSON.stringify([path.join(pluginRoot, 'scripts/codestory-mcp.cjs')])}`, `cwd = ${JSON.stringify(pluginRoot)}`];
  for (const key of ['tool_timeout_sec', 'startup_timeout_sec']) if (server[key] !== undefined) {
    requireThat(Number.isFinite(server[key]) && server[key] > 0, 'invalid installed launcher timeout');
    lines.push(`${key} = ${server[key]}`);
  }
  lines.push(`env_vars = ${JSON.stringify(HOST_ENVIRONMENT.filter(key => env[key] !== undefined))}`);
  return `\n${lines.join('\n')}\n`;
}

export function validateChildEnvironment(actual, expected) {
  for (const [key, value] of Object.entries(expected)) requireThat(actual[key] === value, `actual MCP child environment mismatch: ${key}`);
}

export function hostConfigurationUpdates(before, after, project) {
  if (before.trim() === after.trim()) return [];
  const trust = `[projects.${JSON.stringify(project)}]\ntrust_level = "trusted"`;
  requireThat(after.trim() === [before.trim(), trust].filter(Boolean).join('\n\n'), 'actual host changed configuration outside its selected-project trust entry');
  return ['selected_project_trust'];
}

export function toolPayload(result) {
  requireThat(result && result.isError !== true, 'actual-host MCP tool failed');
  const value = result.structuredContent ?? result.structured_content
    ?? JSON.parse(result.content?.find(item => item.type === 'text')?.text ?? 'null');
  requireThat(value && value.code !== 'codestory_unavailable' && value.state !== 'unavailable', 'actual-host CodeStory unavailable');
  return value;
}

export function validateHostCatalog(inventory, expected, arm) {
  const servers = inventory.data?.filter(server => server.name === 'codestory' || server.serverInfo?.name === 'codestory') ?? [];
  requireThat(servers.length === 1 && servers[0].runtimeStatus === 'connected' && servers[0].pluginId === null
    && servers[0].serverInfo?.version === arm.version, 'expected one explicitly registered installed CodeStory server');
  const actual = servers[0].tools;
  requireThat(actual && Object.keys(actual).length === expected.length
    && expected.every(tool => {
      // Codex's MCP Tool decoder drops the old non-standard top-level safety field.
      // Standard annotations and v3 namespaced metadata must still match exactly.
      const projected = { ...tool };
      if (arm.schema_version === 2) delete projected.safety;
      return JSON.stringify(canonical(actual[tool.name])) === JSON.stringify(canonical(projected));
    }), 'actual host tool catalog mismatch');
}

export function auditParticipantRuntime(events, arm) {
  const audit = { calls: 0, identity_stamps: 0, failures: [] };
  for (const event of events) {
    const item = event.item;
    if (event.type !== 'item.completed' || item?.type !== 'mcp_tool_call' || item.server !== 'codestory') continue;
    audit.calls++;
    const result = item.result;
    let payload = result?.structuredContent ?? result?.structured_content;
    if (!payload) { try { payload = JSON.parse(result?.content?.find(part => part.type === 'text')?.text ?? 'null'); } catch { /* Non-JSON tool errors remain in the transcript. */ } }
    if (payload?.code === 'codestory_unavailable' && /^managed_cli_/.test(payload.failure ?? '')) {
      audit.failures.push({ tool: item.tool, code: String(payload.failure).split(':').slice(0, 2).join(':') });
    }
    const publication = result?._meta?.codestory_publication;
    const runtime = publication?.contract_runtime;
    if (runtime) {
      if (publication.schema_version !== arm.schema_version || runtime.cli_sha256 !== arm.runtime_sha256
        || runtime.cli_source !== arm.runtime_source || runtime.cli_version !== arm.version
        || runtime.plugin_cli_version !== arm.version || runtime.plugin_version !== arm.version
        || runtime.pinned_pair_matches !== true || runtime.known_override_skew_channel !== false) {
        audit.failures.push({ tool: item.tool, code: 'runtime_identity_mismatch' });
      } else audit.identity_stamps++;
    }
  }
  return audit;
}

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
  let config = await readFile(path.join(arm.codex_home_template, 'config.toml'), 'utf8');
  validateTemplateConfig(config, row.arm, arm);
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
    const transport = JSON.parse(await readFile(path.join(pluginRoot, '.mcp.json'), 'utf8'));
    requireThat(Object.keys(transport.mcpServers ?? {}).join() === 'codestory', 'unexpected installed MCP server registration');
    config += installedHostConfig(transport.mcpServers.codestory, pluginRoot, arm, env);
  }
  await writeFile(path.join(env.CODEX_HOME, 'config.toml'), config);
  if (installation.auth_file) await cp(installation.auth_file, path.join(env.CODEX_HOME, 'auth.json'));
  const listingRaw = await checkedProcess(helpers.runProcess, codex, ['plugin', 'list', '--json', '--disable', 'remote_plugin'], { env });
  const listing = JSON.parse(listingRaw);
  validateInventory(listing.installed, row.arm, arm);
  const effective = { schema_version: 1, session: row, model: manifest.model, timeout_ms: 600_000,
    sandbox: 'workspace-write', host_features: { remote_plugin: false }, project, source_commit: repo.commit, source_tree: repo.tree,
    package: arm, configuration: config, plugin_listing: listing, environment: Object.fromEntries(HOST_ENVIRONMENT.filter(key => env[key] !== undefined).map(key => [key, env[key]])) };
  await save(path.join(root, 'effective-config.json'), effective);
  return { root, env, project, pluginRoot, task, arm, effective, codex };
}

async function observeChildEnvironment(channel, session, helpers) {
  requireThat(['darwin', 'linux'].includes(process.platform), 'actual MCP environment observation requires macOS or Linux');
  const output = await checkedProcess(helpers.runProcess, 'ps', ['-axo', 'pid=,ppid=,command='], { env: session.env });
  const rows = output.split('\n').map(line => line.trim().match(/^(\d+)\s+(\d+)\s+(.*)$/)).filter(Boolean)
    .map(match => ({ pid: Number(match[1]), parent: Number(match[2]), command: match[3] }));
  const parents = new Map(rows.map(row => [row.pid, row.parent]));
  const descendant = pid => { const seen = new Set(); while (parents.has(pid) && !seen.has(pid)) { seen.add(pid); pid = parents.get(pid); if (pid === channel.pid) return true; } return false; };
  const children = rows.filter(row => descendant(row.pid) && row.command.includes(path.join(session.pluginRoot, 'scripts/codestory-mcp.cjs')));
  requireThat(children.length === 1, 'expected one actual installed MCP child');
  const expected = Object.fromEntries(HOST_ENVIRONMENT.filter(key => session.env[key] !== undefined).map(key => [key, session.env[key]]));
  const actual = {};
  if (process.platform === 'linux') {
    const entries = (await readFile(`/proc/${children[0].pid}/environ`, 'utf8')).split('\0');
    for (const key of Object.keys(expected)) actual[key] = entries.find(entry => entry.startsWith(`${key}=`))?.slice(key.length + 1);
  } else {
    // Keep the raw process environment in memory only; persist just these isolated controls.
    const raw = await checkedProcess(helpers.runProcess, 'ps', ['eww', '-p', String(children[0].pid), '-o', 'command='], { env: session.env });
    for (const key of Object.keys(expected)) actual[key] = raw.match(new RegExp(`(?:^| )${key}=(.*?)(?= [A-Za-z_][A-Za-z0-9_]*=|$)`))?.[1];
  }
  validateChildEnvironment(actual, expected);
  return { pid: children[0].pid, environment_sha256: sha(JSON.stringify(canonical(actual))), verified_names: Object.keys(expected) };
}

async function withInstalledHost(session, helpers, name, action) {
  const { root, env, pluginRoot, arm, codex } = session;
  const transcript = [];
  const channel = helpers.createSequencedStdioSession(codex, ['app-server', '--disable', 'remote_plugin', '--stdio'],
    { env, cwd: session.project, protocol: 'app-server', timeoutMs: 90_000, maxOutputBytes: 16 * 1024 * 1024,
      onNotification: notification => transcript.push({ notification }) });
  let id = 0;
  const request = async (method, params) => {
    const sent = { id: ++id, method, params };
    const received = await channel.request(sent); transcript.push({ sent, received });
    requireThat(!received.error, `actual host ${method} failed: ${JSON.stringify(received.error)}`);
    return received.result;
  };
  try {
    await request('initialize', { clientInfo: { name: 'installed-navigation-canary', version: '1' }, capabilities: { experimentalApi: true } });
    channel.send({ method: 'initialized' });
    const started = await request('thread/start', { cwd: session.project, model: 'gpt-5.6-terra', sandbox: 'workspace-write', ephemeral: true, config: { model_reasoning_effort: 'low' } });
    const threadId = started.thread?.id;
    requireThat(typeof threadId === 'string', 'actual host did not create an ephemeral inspection context');
    const inventory = await request('mcpServerStatus/list', { threadId });
    const catalog = JSON.parse(await readFile(path.join(pluginRoot, 'generated-mcp-catalog.json'), 'utf8'));
    requireThat(sha(JSON.stringify(catalog.tools)) === arm.tools_sha256, 'installed catalog receipt mismatch');
    validateHostCatalog(inventory, catalog.tools, arm);
    const child = await observeChildEnvironment(channel, session, helpers);
    const call = (tool, args) => request('mcpServer/tool/call', { threadId, server: 'codestory', tool, arguments: args });
    const result = await action(call);
    const config = await readFile(path.join(env.CODEX_HOME, 'config.toml'), 'utf8');
    const hostUpdates = hostConfigurationUpdates(session.effective.configuration, config, session.project);
    session.effective.configuration = config;
    await save(path.join(root, 'effective-config.json'), session.effective);
    return { ...result, host_registration: 'explicit_installed_launcher', configuration_sha256: sha(config),
      host_owned_configuration_updates: hostUpdates, child, model_turns: 0 };
  } finally {
    await channel.stop();
    await save(path.join(root, `${name}-transcript.json`), transcript);
    await writeFile(path.join(root, `${name}-stderr.txt`), channel.stderr());
  }
}

async function operationCanary(session, helpers) {
  const { root, env, pluginRoot, arm } = session;
  if (!pluginRoot) return { status: 'pass', surface: 'native', operations: ['git clone', 'git checkout', 'plugin inventory'] };
  const project = path.join(root, 'operation-canary');
  await mkdir(project);
  await writeFile(path.join(project, 'index.js'), 'export function navigationCanary() { return "BEFORE_REFRESH"; }\n');
  await checkedProcess(helpers.runProcess, 'git', ['-c', 'core.hooksPath=/dev/null', 'init', project], { env });
  return withInstalledHost(session, helpers, 'canary', async hostCall => {
    const call = async (name, args) => {
      const deadline = performance.now() + 75_000;
      while (true) {
        const result = await hostCall(name, { project, ...args });
        const state = toolPayload(result);
        if (!['preparing', 'updating'].includes(state?.state)) { validateRuntime(result, arm); return { ...result, structuredContent: state }; }
        requireThat(performance.now() < deadline, 'managed preparation exceeded canary budget');
        await delay(Math.min(5000, Math.max(100, state.retry_after_ms ?? 1000)));
      }
    };
    await call('search', { query: 'navigationCanary' });
    const readiness = toolPayload(await hostCall('status', { project }));
    requireThat(readiness.live_ready === true && readiness.retrieval_mode === 'full', 'actual host search did not prove full retrieval');
    const before = await call('snippet', { path: 'index.js', start_line: 1, end_line: 1 });
    validateSourceRead(before, project, 'BEFORE_REFRESH');
    await writeFile(path.join(project, 'index.js'), 'export function navigationCanary() { return "AFTER_REFRESH"; }\n');
    await call('files', {});
    const after = await call('snippet', { path: 'index.js', start_line: 1, end_line: 1 });
    validateSourceRead(after, project, 'AFTER_REFRESH');
    const eventsFile = path.join(env.CODESTORY_EMBED_QUALIFICATION_DIR, `${env.CODESTORY_EMBED_QUALIFICATION_NONCE}.events.jsonl`);
    requireThat(!(await lstat(eventsFile)).isSymbolicLink(), 'private native event receipt is a symlink');
    const eventsBytes = await readFile(eventsFile);
    const events = eventsBytes.toString('utf8').trim().split('\n').map(line => JSON.parse(line));
    const completed = events.filter(event => event.action === 'completed_tokens' && event.status === 'completed')
      .reduce((total, event) => total + Number(event.details?.completed_tokens ?? 0), 0);
    requireThat(Number.isFinite(completed) && completed > 0 && env.CODESTORY_EMBED_ALLOW_CPU === '0', 'missing private native completion with CPU fallback disabled');
    return { status: 'pass', surface: 'installed-plugin', catalog_sha256: arm.tools_sha256, runtime_sha256: arm.runtime_sha256,
      private_native_completed_tokens: completed, native_events_sha256: sha(eventsBytes), cpu_fallback_allowed: false };
  });
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
  const infrastructureFailures = new Map();
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
      if (session.pluginRoot) {
        const host = await withInstalledHost(session, bounded, 'participant-host', async () => ({ status: 'pass', catalog_sha256: session.arm.tools_sha256 }));
        await save(path.join(root, 'participant-host.json'), host);
      }
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
      if (session.pluginRoot) {
        result.host_runtime = auditParticipantRuntime(events, session.arm);
        if (result.host_runtime.failures.length) {
          result.execution_status = result.status; result.status = 'runtime_failed';
          const key = `${row.arm}:${result.host_runtime.failures[0].code}`;
          infrastructureFailures.set(key, (infrastructureFailures.get(key) ?? 0) + 1);
        }
      }
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
    if ([...infrastructureFailures.values()].some(count => count >= 2)) {
      for (const pending of results.slice(index + 1)) { pending.status = 'not_run_runtime_failed'; pending.error = 'two equivalent actual-host runtime failures'; }
      await persist(); return;
    }
    // Failed attempts and unstarted budget rows retain their places in the denominator.
  }
}
