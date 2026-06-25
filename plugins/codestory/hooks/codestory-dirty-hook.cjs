#!/usr/bin/env node

const {
  dirtyHookStatus,
  installDirtyHooks,
  uninstallDirtyHooks,
  writeDirtyMarker,
} = require('./codestory-runtime.cjs');

function argValue(args, name) {
  const index = args.indexOf(name);
  return index === -1 ? null : args[index + 1] || null;
}

function usage() {
  return [
    'Usage: codestory-dirty-hook.cjs <install|uninstall|status|mark> --project <repo> [--plugin-data <dir>]',
    '',
    'Installs or removes CodeStory-managed Git hook blocks that write the dirty marker.',
  ].join('\n');
}

function main() {
  const args = process.argv.slice(2);
  const action = args[0];
  const project = argValue(args, '--project') || process.cwd();
  const pluginDataDir = argValue(args, '--plugin-data') || process.env.PLUGIN_DATA || process.env.COPILOT_PLUGIN_DATA;
  const options = { pluginDataDir };

  if (!['install', 'uninstall', 'status', 'mark'].includes(action)) {
    console.error(usage());
    process.exit(2);
  }

  let result;
  if (action === 'install') {
    result = installDirtyHooks(project, options);
  } else if (action === 'uninstall') {
    result = uninstallDirtyHooks(project, options);
  } else if (action === 'status') {
    result = dirtyHookStatus(project, options);
  } else {
    result = writeDirtyMarker(project, {
      ...options,
      dirty: true,
      source: argValue(args, '--source') || 'codestory-git-hook',
    }) || { status: 'plugin_data_required' };
  }

  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exit(1);
}
