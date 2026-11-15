#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const {
  inferredCodexPluginDataDir,
  inferredCursorPluginDataDir,
  writeDirtyMarker,
  resolveCli,
} = require('./codestory-runtime.cjs');

const MAX_CLI_OUTPUT_BYTES = 4 * 1024 * 1024;

function argValue(args, name) {
  const index = args.indexOf(name);
  return index === -1 ? null : args[index + 1] || null;
}

function usage() {
  return [
    'Usage: codestory-dirty-hook.cjs <install|uninstall|status|mark> --project <repo> [--plugin-data <dir>] [--cli <codestory-cli>]',
    '',
    'Install, uninstall, and status delegate to the verified CodeStory CLI. mark writes the fail-open dirty marker.',
  ].join('\n');
}

function typedResult(status, projectRoot, message) {
  return {
    schema_version: 1,
    status,
    project_root: path.resolve(projectRoot || process.cwd()),
    hooks: [],
    ...(message ? { message } : {}),
  };
}

function delegateHookAction(action, projectRoot, dataDir, args) {
  if (!dataDir) return typedResult('plugin_data_required', projectRoot, 'plugin data path is required');
  const cli = resolveCli(args, dataDir);
  if (!cli) {
    return typedResult(
      'cli_unavailable',
      projectRoot,
      'pass --cli, set CODESTORY_PLUGIN_CLI_PATH/CODESTORY_CLI, or start the installed plugin once to publish a verified runtime receipt',
    );
  }
  const result = spawnSync(cli, [
    'internal-dirty-hook',
    action,
    '--project',
    path.resolve(projectRoot),
    '--plugin-data',
    path.resolve(dataDir),
    '--node',
    process.execPath,
    '--script',
    fs.realpathSync(__filename),
  ], {
    encoding: 'utf8',
    maxBuffer: MAX_CLI_OUTPUT_BYTES,
    shell: false,
    windowsHide: true,
  });
  if (result.error) {
    throw new Error(`dirty_hook_cli_failed:${result.error.code || result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`dirty_hook_cli_failed:exit_${result.status}`);
  }
  try {
    const parsed = JSON.parse(result.stdout);
    if (parsed?.schema_version !== 1 || typeof parsed?.status !== 'string' || !Array.isArray(parsed?.hooks)) {
      throw new Error('invalid result shape');
    }
    return parsed;
  } catch (error) {
    throw new Error(`dirty_hook_cli_invalid_output:${error.message}`);
  }
}

function main() {
  const args = process.argv.slice(2);
  const action = args[0];
  const project = argValue(args, '--project') || process.cwd();
  const pluginDataDir = argValue(args, '--plugin-data')
    || process.env.PLUGIN_DATA
    || process.env.COPILOT_PLUGIN_DATA
    || process.env.CODESTORY_PLUGIN_DATA
    || inferredCodexPluginDataDir()
    || inferredCursorPluginDataDir();
  if (!['install', 'uninstall', 'status', 'mark'].includes(action)) {
    console.error(usage());
    process.exit(2);
  }

  const result = action === 'mark'
    ? writeDirtyMarker(project, {
        pluginDataDir,
        dirty: true,
        source: argValue(args, '--source') || 'codestory-git-hook',
      }) || { status: 'plugin_data_required' }
    : delegateHookAction(action, project, pluginDataDir, args);

  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exit(1);
}
