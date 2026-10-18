#!/usr/bin/env node

const {
  rememberActiveState,
  writeHookOutput,
  resolveCli,
  pluginDataDir,
} = require('./codestory-runtime.cjs');

const path = require('path');
const { spawnSync } = require('child_process');

const SESSION_CONTEXT = [
  'CODESTORY GROUNDING AVAILABLE',
  '',
  'For repository work, read and follow the loaded codestory-grounding skill. It is the sole source of truth for CodeStory tool routing and evidence boundaries; this hook adds no parallel instructions.',
].join('\n');

function readHookInput() {
  return new Promise((resolve) => {
    let input = '';
    process.stdin.on('data', (chunk) => { input += chunk; });
    process.stdin.on('end', () => {
      try {
        resolve(input.trim() ? JSON.parse(input.replace(/^\uFEFF/, '')) : {});
      } catch {
        resolve({});
      }
    });
  });
}

function contextFor(event) {
  return event === 'SessionStart' || event === 'sessionStart' ? SESSION_CONTEXT : null;
}

function startupContext(input, context) {
  if (input.hook_event_name !== 'SessionStart' || input.source !== 'startup'
      || typeof input.cwd !== 'string' || !path.isAbsolute(input.cwd)) return context;
  try {
    const cli = resolveCli([], pluginDataDir());
    if (!cli) return context;
    const result = spawnSync(cli, ['internal-session-context', '--project', input.cwd], {
      encoding: 'utf8', shell: false, windowsHide: true, timeout: 2000, maxBuffer: 16 * 1024,
    });
    if (result.error || result.status !== 0) return context;
    const output = JSON.parse(result.stdout);
    if (output?.schema_version !== 1 || output?.status !== 'ready'
        || typeof output.context !== 'string' || !output.context.trim()
        || Buffer.byteLength(output.context, 'utf8') > 3584) return context;
    const combined = `${context}\n\n${output.context}`;
    return Buffer.byteLength(combined, 'utf8') <= 4096 ? combined : context;
  } catch { return context; }
}

async function main() {
  const input = await readHookInput();
  const event = input.hook_event_name || 'SessionStart';
  try {
    const context = contextFor(event);
    if (!context) {
      writeHookOutput(event, null);
      return;
    }
    rememberActiveState({
      event,
      cwd: input.cwd || process.cwd(),
      source: input.source || input.trigger || null,
      codexThreadId: process.env.CODEX_THREAD_ID || null,
      hook: {
        instructions_emitted: {},
        bridge_removed: true,
      },
    });
    writeHookOutput(event, startupContext(input, context));
  } catch {
    // Best effort only. A hook failure must not block the agent session.
  }
}

main();
