#!/usr/bin/env node

const {
  rememberActiveState,
  writeHookOutput,
} = require('./codestory-runtime.cjs');

const SESSION_CONTEXT = [
  'CODESTORY GROUNDING AVAILABLE',
  '',
  'For repository work, follow the loaded CodeStory rule or codestory-grounding skill. Resolve the target repository root; pass its exact absolute path as project. The session cwd is only a starting hint.',
  'Strict routing: named file-local -> direct read; discovery -> search only; selected target -> context; broad -> packet; supplied complete typed contract -> prove_call_path.',
  'Evidence boundary: cite a direct read only after success. If fallback leaves packet gaps unresolved, keep packet authority and Unknown; never upgrade it to source authority.',
  'Call status only for diagnostics. On preparing, retry that same tool after its reported delay; no shell wait or user setup.',
  'If the intended CodeStory tool is hidden and tool_search is available, search only for that tool by name, then call it directly.',
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
