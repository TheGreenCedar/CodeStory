const fs = require('fs');
const path = require('path');
const { createHash } = require('crypto');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);

const STATE_FILE = '.codestory-active';
const HOOK_STATE_FILE = '.codestory-hook-output-state.json';
const MCP_RUNTIME_FILE = '.codestory-mcp-runtime.json';
const DIRTY_MARKER_SCHEMA_VERSION = 1;
const DIRTY_MARKER_SAMPLE_LIMIT = 20;
const DIRTY_HOOK_NAMES = ['post-checkout', 'post-merge', 'post-rewrite'];
const DIRTY_HOOK_START = '# >>> codestory dirty marker >>>';
const DIRTY_HOOK_END = '# <<< codestory dirty marker <<<';

function pluginDataDir() {
  if (isCodex) return process.env.PLUGIN_DATA;
  if (isCopilot) return process.env.COPILOT_PLUGIN_DATA;
  return null;
}

function stateFilePath() {
  const stateDir = pluginDataDir();
  return stateDir ? path.join(stateDir, STATE_FILE) : null;
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function pluginRoot() {
  return path.dirname(__dirname);
}

function normalizeProjectRoot(projectRoot) {
  const resolved = path.resolve(projectRoot || process.cwd());
  try {
    return fs.realpathSync(resolved);
  } catch {
    return resolved;
  }
}

function dirtyMarkerPathForProject(projectRoot, dataDir = pluginDataDir()) {
  if (!dataDir || !projectRoot) return null;
  const normalizedRoot = normalizeProjectRoot(projectRoot);
  const key = createHash('sha256').update(normalizedRoot).digest('hex').slice(0, 32);
  return path.join(dataDir, 'dirty-markers', `${key}.json`);
}

function pluginVersion(root = pluginRoot()) {
  const manifest = readJson(path.join(root, '.codex-plugin', 'plugin.json'));
  return manifest && typeof manifest.version === 'string' ? manifest.version : null;
}

function configuredMcp(root = pluginRoot()) {
  const configPath = path.join(root, '.mcp.json');
  const config = readJson(configPath);
  const server = config?.mcpServers?.codestory;
  if (!server) {
    return { installed: false, configPath, command: null, args: [] };
  }
  return {
    installed: true,
    configPath,
    command: server.command || null,
    args: Array.isArray(server.args) ? server.args : [],
  };
}

function mcpScriptExists(root, args) {
  const scriptArg = args.find((arg) => typeof arg === 'string' && /codestory-mcp\.cjs$/u.test(arg));
  if (!scriptArg) return false;
  return fs.existsSync(path.resolve(root, scriptArg));
}

function managedCliInfo(dataDir = pluginDataDir(), version = pluginVersion()) {
  if (!dataDir) return { present: false, path: null, source: null };
  const runtime = readJson(path.join(dataDir, MCP_RUNTIME_FILE));
  if (runtime?.source === 'managed' && runtime.path) {
    return { present: fs.existsSync(runtime.path), path: runtime.path, source: 'runtime_state' };
  }

  const manifestPath = version
    ? path.join(dataDir, 'codestory-cli', version, 'manifest.json')
    : path.join(dataDir, 'codestory-cli', 'manifest.json');
  const manifest = readJson(manifestPath);
  const manifestCli = manifest?.path
    ? path.resolve(path.dirname(manifestPath), manifest.path)
    : null;
  if (manifestCli) {
    return { present: fs.existsSync(manifestCli), path: manifestCli, source: 'manifest' };
  }
  return { present: false, path: null, source: null };
}

function classifyMcpRuntime(options = {}) {
  const root = options.pluginRoot || pluginRoot();
  const dataDir = options.pluginDataDir === undefined ? pluginDataDir() : options.pluginDataDir;
  const mcp = configuredMcp(root);
  const runtimePath = dataDir ? path.join(dataDir, MCP_RUNTIME_FILE) : null;
  const runtime = runtimePath ? readJson(runtimePath) : null;
  const managed = managedCliInfo(dataDir, pluginVersion(root));
  const launchable = mcp.installed && mcp.command === 'node' && mcpScriptExists(root, mcp.args);
  const resourcesExposed = process.env.CODESTORY_MCP_RESOURCES_EXPOSED === '1' ||
    Boolean(runtime?.source && runtime?.path && fs.existsSync(runtime.path));
  const resourceStatus = resourcesExposed
    ? 'mcp_resources_exposed'
    : launchable
      ? 'mcp_resources_not_model_visible'
      : 'mcp_resources_unavailable';

  return {
    mcp_config_installed: mcp.installed,
    mcp_config_path: mcp.configPath,
    mcp_command: mcp.command,
    mcp_args: mcp.args,
    mcp_process_launchable: launchable,
    mcp_resources_exposed: resourcesExposed,
    mcp_resource_status: resourceStatus,
    mcp_runtime_state_path: runtimePath,
    mcp_runtime_state_present: Boolean(runtime),
    managed_cli_present: managed.present,
    managed_cli_path: managed.path,
    managed_cli_source: managed.source,
    degraded_no_surface: !resourcesExposed && !managed.present,
  };
}

function mcpDetectionText(status) {
  return [
    'CODESTORY MCP RUNTIME DETECTION',
    `mcp_config_installed: ${status.mcp_config_installed ? 'yes' : 'no'}${status.mcp_config_installed ? ` (${status.mcp_config_path})` : ''}`,
    `mcp_process_launchable: ${status.mcp_process_launchable ? 'yes' : 'no'}`,
    `mcp_resources_exposed: ${status.mcp_resource_status}`,
    `managed_cli_present: ${status.managed_cli_present ? 'yes' : 'no'}${status.managed_cli_path ? ` (${status.managed_cli_path})` : ''}`,
    `degraded_no_surface: ${status.degraded_no_surface ? 'yes' : 'no'}`,
  ].join('\n');
}

function readActiveState() {
  const file = stateFilePath();
  return file ? readJson(file) : null;
}

function rememberActiveState(state) {
  const file = stateFilePath();
  if (!file) return;

  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const previous = readActiveState() || {};
    fs.writeFileSync(file, JSON.stringify({
      ...previous,
      ...state,
      hook: {
        ...(previous.hook || {}),
        ...(state.hook || {}),
      },
      updatedAt: new Date().toISOString(),
    }));
  } catch (e) {
    // Best effort only. Hook state must not block the host session.
  }
}

function readHookState() {
  const stateDir = pluginDataDir();
  if (!stateDir) return {};
  return readJson(path.join(stateDir, HOOK_STATE_FILE)) || {};
}

function writeHookState(state) {
  const stateDir = pluginDataDir();
  if (!stateDir) return;

  try {
    fs.mkdirSync(stateDir, { recursive: true });
    fs.writeFileSync(path.join(stateDir, HOOK_STATE_FILE), JSON.stringify({
      ...state,
      updatedAt: new Date().toISOString(),
    }));
  } catch (e) {
    // Best effort only. Hook state must not block the host session.
  }
}

function writeDirtyMarker(projectRoot, options = {}) {
  const markerPath = dirtyMarkerPathForProject(projectRoot, options.pluginDataDir);
  if (!markerPath) return null;
  const normalizedRoot = normalizeProjectRoot(projectRoot);
  const pathSample = Array.isArray(options.pathSample)
    ? options.pathSample
      .filter((item) => typeof item === 'string' && item.trim())
      .slice(0, DIRTY_MARKER_SAMPLE_LIMIT)
    : [];
  const marker = {
    schema_version: DIRTY_MARKER_SCHEMA_VERSION,
    project_root: normalizedRoot,
    dirty: Boolean(options.dirty),
    updated_at: new Date().toISOString(),
    source: String(options.source || 'codestory-hook'),
  };
  if (pathSample.length > 0) {
    marker.path_sample = pathSample;
  }

  try {
    fs.mkdirSync(path.dirname(markerPath), { recursive: true });
    const existing = readJson(markerPath);
    const existingSample = Array.isArray(existing?.path_sample) ? existing.path_sample : [];
    if (
      existing?.schema_version === marker.schema_version
      && existing?.project_root === marker.project_root
      && existing?.dirty === marker.dirty
      && existing?.source === marker.source
      && JSON.stringify(existingSample) === JSON.stringify(pathSample)
    ) {
      return { path: markerPath, marker: existing, unchanged: true };
    }
    fs.writeFileSync(markerPath, JSON.stringify(marker, null, 2));
    return { path: markerPath, marker };
  } catch {
    return null;
  }
}

function shellQuote(value) {
  return `'${String(value).replace(/\\/g, '/').replace(/'/g, `'\\''`)}'`;
}

function gitDirForProject(projectRoot) {
  const dotGit = path.join(normalizeProjectRoot(projectRoot), '.git');
  try {
    const stat = fs.statSync(dotGit);
    if (stat.isDirectory()) return dotGit;
    if (stat.isFile()) {
      const text = fs.readFileSync(dotGit, 'utf8').trim();
      const match = text.match(/^gitdir:\s*(.+)$/iu);
      if (!match) return null;
      return path.resolve(path.dirname(dotGit), match[1]);
    }
  } catch {
    return null;
  }
  return null;
}

function hookManagerPaths(projectRoot, options = {}) {
  const dataDir = options.pluginDataDir || pluginDataDir();
  const gitDir = gitDirForProject(projectRoot);
  const scriptPath = path.join(__dirname, 'codestory-dirty-hook.cjs');
  return {
    dataDir,
    gitDir,
    hooksDir: gitDir ? path.join(gitDir, 'hooks') : null,
    nodePath: process.execPath,
    projectRoot: normalizeProjectRoot(projectRoot),
    scriptPath,
  };
}

function dirtyHookBlock(paths, hookName) {
  const command = [
    shellQuote(paths.nodePath),
    shellQuote(paths.scriptPath),
    'mark',
    '--project',
    shellQuote(paths.projectRoot),
    '--plugin-data',
    shellQuote(paths.dataDir),
    '--source',
    shellQuote(`git-hook:${hookName}`),
    '|| true',
  ].join(' ');
  return `${DIRTY_HOOK_START}\n${command}\n${DIRTY_HOOK_END}`;
}

function splitDirtyHookBlock(text) {
  const start = text.indexOf(DIRTY_HOOK_START);
  const end = text.indexOf(DIRTY_HOOK_END);
  if (start === -1 || end === -1 || end < start) {
    return { before: text, block: null, after: '' };
  }
  const afterStart = end + DIRTY_HOOK_END.length;
  return {
    before: text.slice(0, start).replace(/[ \t]*\r?\n?$/u, ''),
    block: text.slice(start, afterStart),
    after: text.slice(afterStart).replace(/^\r?\n/u, ''),
  };
}

function dirtyHookState(hookPath, expectedBlock) {
  if (!fs.existsSync(hookPath)) {
    return { state: 'not_installed', path: hookPath };
  }
  const text = fs.readFileSync(hookPath, 'utf8');
  const parts = splitDirtyHookBlock(text);
  if (!parts.block) {
    return { state: 'foreign_hook_present', path: hookPath };
  }
  return {
    state: parts.block === expectedBlock ? 'installed' : 'uninstall_required',
    path: hookPath,
  };
}

function dirtyHookSummary(results) {
  const states = results.map((result) => result.state);
  if (states.every((state) => state === 'installed')) return 'installed';
  if (states.every((state) => state === 'not_installed')) return 'not_installed';
  if (states.some((state) => state === 'uninstall_required')) return 'uninstall_required';
  if (states.some((state) => state === 'installed')) return 'partially_installed';
  if (states.some((state) => state === 'foreign_hook_present')) return 'foreign_hook_present';
  return 'unknown';
}

function dirtyHookStatus(projectRoot, options = {}) {
  const paths = hookManagerPaths(projectRoot, options);
  if (!paths.dataDir || !paths.hooksDir) {
    return {
      status: !paths.dataDir ? 'plugin_data_required' : 'not_a_git_repository',
      project_root: paths.projectRoot,
      hooks: [],
    };
  }
  const hooks = DIRTY_HOOK_NAMES.map((hookName) => {
    return {
      hook: hookName,
      ...dirtyHookState(path.join(paths.hooksDir, hookName), dirtyHookBlock(paths, hookName)),
    };
  });
  return {
    status: dirtyHookSummary(hooks),
    project_root: paths.projectRoot,
    plugin_data: paths.dataDir,
    hooks,
  };
}

function installDirtyHooks(projectRoot, options = {}) {
  const paths = hookManagerPaths(projectRoot, options);
  if (!paths.dataDir) throw new Error('plugin data path is required');
  if (!paths.hooksDir) throw new Error('project is not a git repository');
  fs.mkdirSync(paths.hooksDir, { recursive: true });
  const hooks = DIRTY_HOOK_NAMES.map((hookName) => {
    const hookPath = path.join(paths.hooksDir, hookName);
    const expectedBlock = dirtyHookBlock(paths, hookName);
    const state = dirtyHookState(hookPath, expectedBlock);
    if (state.state === 'installed') return { hook: hookName, ...state, changed: false };
    if (state.state === 'uninstall_required') {
      return { hook: hookName, ...state, changed: false };
    }
    const existing = fs.existsSync(hookPath) ? fs.readFileSync(hookPath, 'utf8').trimEnd() : '#!/bin/sh';
    const next = `${existing}\n\n${expectedBlock}\n`;
    fs.writeFileSync(hookPath, next, { mode: 0o755 });
    return { hook: hookName, ...dirtyHookState(hookPath, expectedBlock), changed: true };
  });
  return {
    status: dirtyHookSummary(hooks),
    project_root: paths.projectRoot,
    plugin_data: paths.dataDir,
    hooks,
  };
}

function uninstallDirtyHooks(projectRoot, options = {}) {
  const paths = hookManagerPaths(projectRoot, options);
  if (!paths.hooksDir) throw new Error('project is not a git repository');
  const hooks = DIRTY_HOOK_NAMES.map((hookName) => {
    const hookPath = path.join(paths.hooksDir, hookName);
    if (!fs.existsSync(hookPath)) {
      return { hook: hookName, state: 'not_installed', path: hookPath, changed: false };
    }
    const text = fs.readFileSync(hookPath, 'utf8');
    const parts = splitDirtyHookBlock(text);
    if (!parts.block) {
      return { hook: hookName, state: 'foreign_hook_present', path: hookPath, changed: false };
    }
    const next = [parts.before, parts.after].filter(Boolean).join('\n').trimEnd();
    if (next && next.trim() !== '#!/bin/sh') {
      fs.writeFileSync(hookPath, `${next}\n`, { mode: 0o755 });
    } else {
      fs.rmSync(hookPath, { force: true });
    }
    return { hook: hookName, ...dirtyHookState(hookPath, dirtyHookBlock(paths, hookName)), changed: true };
  });
  return {
    status: dirtyHookSummary(hooks),
    project_root: paths.projectRoot,
    plugin_data: paths.dataDir,
    hooks,
  };
}

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify({ additionalContext: context }));
    return;
  }

  if (isCodex) {
    const output = {
      systemMessage: 'CODESTORY:BACKGROUND',
    };
    if (context) {
      output.hookSpecificOutput = {
        hookEventName: event,
        additionalContext: context,
      };
    }
    process.stdout.write(JSON.stringify(output));
    return;
  }

  process.stdout.write(context);
}

module.exports = {
  classifyMcpRuntime,
  dirtyMarkerPathForProject,
  dirtyHookStatus,
  installDirtyHooks,
  mcpDetectionText,
  readActiveState,
  readHookState,
  rememberActiveState,
  uninstallDirtyHooks,
  writeDirtyMarker,
  writeHookState,
  writeHookOutput,
};
