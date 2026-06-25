const fs = require('fs');
const path = require('path');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);

const STATE_FILE = '.codestory-active';
const MCP_RUNTIME_FILE = '.codestory-mcp-runtime.json';

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
  mcpDetectionText,
  readActiveState,
  rememberActiveState,
  writeHookOutput,
};
