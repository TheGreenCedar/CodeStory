#!/usr/bin/env node

const { createHash } = require('crypto');
const { eventHeader } = require('./codestory-instructions.cjs');
const {
  readHookState,
  rememberActiveState,
  writeHookOutput,
  writeHookState,
} = require('./codestory-runtime.cjs');

const EVENT_CAPS = {
  UserPromptSubmit: 1200,
  SessionStart: 1000,
  GoalLoopHeartbeat: 600,
};

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

function normalizeText(text) {
  return String(text || '').replace(/\s+/g, ' ').trim();
}

function hashText(text) {
  return createHash('sha256').update(text).digest('hex').slice(0, 16);
}

function taxonomy(input, event) {
  if (event === 'UserPromptSubmit') return 'user_prompt';
  const source = normalizeText(input.source || input.trigger || '').toLowerCase();
  if (/goal|heartbeat/u.test(`${event} ${source}`.toLowerCase())) return 'goal_heartbeat';
  if (/compact/u.test(source)) return 'compact';
  if (/resume/u.test(source)) return 'resume';
  if (/clear/u.test(source)) return 'clear';
  return 'startup';
}

function capFor(event) {
  return EVENT_CAPS[event] || 1000;
}

function truncate(text, cap) {
  const value = String(text || '').trim();
  return value.length <= cap ? value : `${value.slice(0, cap)}\n\n... CodeStory hook output truncated.`;
}

function shouldEmit(key, reset = false) {
  const state = readHookState();
  const emitted = reset ? {} : (state.emitted || {});
  if (emitted[key]) return false;
  writeHookState({ ...state, emitted: { ...emitted, [key]: true } });
  return true;
}

function contextFor(input, event) {
  const kind = taxonomy(input, event);
  const cwd = input.cwd || process.cwd();
  const prompt = normalizeText(input.prompt || '');
  const key = [
    kind,
    cwd,
    event === 'UserPromptSubmit' ? hashText(prompt) : '',
  ].filter(Boolean).join(':');
  if (!shouldEmit(key, kind === 'startup' || kind === 'clear')) return null;
  if (kind === 'goal_heartbeat') return null;

  const header = eventHeader(event, input).trim();
  const mcpInstructions = [
    'CodeStory MCP startup path:',
    '1. Use live CodeStory MCP before manual source reads for repository work.',
    `2. If mcp__codestory tools are visible, call status with project=${JSON.stringify(input.cwd || process.cwd())}, then pass that same project to every CodeStory tool call.`,
    '3. If mcp__codestory is not visible and tool_search is available, query "codestory mcp ground status packet search", then use the loaded CodeStory MCP tools.',
    '4. The MCP is multi-project and request-scoped. Never infer workspace from another thread or a global active-state file.',
    '5. Do not treat hook text as grounding evidence; only live MCP results or verified source reads count.',
  ].join('\n');

  return truncate([header, mcpInstructions].filter(Boolean).join('\n\n'), capFor(event));
}

readHookInput().then((input) => {
  const event = input.hook_event_name || 'SessionStart';
  try {
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
    writeHookOutput(event, contextFor(input, event));
  } catch {
    // Best effort only. A hook failure must not block the agent session.
  }
});
