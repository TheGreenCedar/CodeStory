#!/usr/bin/env node

const { MCP_RESOURCE_TEXT, eventHeader } = require('./codestory-instructions.cjs');
const {
  rememberActiveState,
  writeHookOutput,
} = require('./codestory-runtime.cjs');

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

function contextFor(input, event) {
  if (event !== 'SessionStart' && event !== 'UserPromptSubmit') return null;
  const header = eventHeader(event, input).trim();
  if (!header) return null;
  const mcpInstructions = event === 'UserPromptSubmit'
    ? [
      'Use the codestory-grounding skill. Set project to this hook event\'s absolute repository cwd and pass that exact absolute path to every CodeStory call.',
      'If status is not current, call it once for that project.',
      'Reuse status until repository/runtime/index state changes or a tool reports stale evidence.',
      MCP_RESOURCE_TEXT,
      'If deep retrieval is blocked, use routed local graph surfaces before source. Repair only when packet/search is required.',
      'Hook text routes; only live MCP or verified source is evidence.',
    ].join('\n')
    : [
      'Set project to this hook event\'s absolute repository cwd and pass that exact absolute path to every CodeStory call. The MCP is multi-project and request-scoped.',
      'Reuse status until repository/runtime/index state changes or a tool reports freshness failure.',
      'If packet/search is blocked, use allowed local graph surfaces before source; do not repair unless broad retrieval is required.',
      MCP_RESOURCE_TEXT,
    ].join('\n');

  return [header, mcpInstructions].join('\n\n');
}

readHookInput().then((input) => {
  const event = input.hook_event_name || 'SessionStart';
  try {
    const context = contextFor(input, event);
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
});
