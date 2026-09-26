#!/usr/bin/env node

const {
  rememberActiveState,
  writeHookOutput,
} = require('./codestory-runtime.cjs');

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
    writeHookOutput(event, context);
  } catch {
    // Best effort only. A hook failure must not block the agent session.
  }
}

main();
