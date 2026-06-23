#!/usr/bin/env node

const { spawnSync } = require('child_process');
const { getCodeStoryInstructions } = require('./codestory-instructions.cjs');
const { rememberActiveState, writeHookOutput } = require('./codestory-runtime.cjs');

const MAX_OUTPUT_CHARS = 12000;
const RUNTIME_TIMEOUT_MS = 3500;

function readHookInput() {
  return new Promise((resolve) => {
    let input = '';
    process.stdin.on('data', (chunk) => { input += chunk; });
    process.stdin.on('end', () => {
      if (!input.trim()) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(input.replace(/^\uFEFF/, '')));
      } catch (e) {
        resolve({});
      }
    });
  });
}

function truncate(text) {
  const value = String(text || '').trim();
  if (value.length <= MAX_OUTPUT_CHARS) return value;
  return `${value.slice(0, MAX_OUTPUT_CHARS)}\n\n... CodeStory hook output truncated by hook budget.`;
}

function hookCommand(input, event) {
  const project = input.cwd || process.cwd();
  if (!project) return null;

  if (event === 'UserPromptSubmit' && String(input.prompt || '').trim()) {
    return {
      kind: 'request packet',
      args: [
        'packet',
        '--project', project,
        '--question', String(input.prompt),
        '--budget', 'tiny',
        '--refresh', 'none',
        '--latency-budget-ms', '1500',
      ],
      next: 'If the packet is partial or unavailable, read codestory://status and run the packet/search/context follow-up that status allows before opening source files.',
    };
  }

  if (event === 'SessionStart') {
    return {
      kind: 'session ground',
      args: [
        'ground',
        '--project', project,
        '--budget', 'strict',
        '--refresh', 'none',
      ],
      next: 'If the ground snapshot is unavailable, read codestory://status and run ready --goal local --repair for the target repo before source reads.',
    };
  }

  return null;
}

function runCodeStory(input, event) {
  if (process.env.CODESTORY_HOOK_DISABLE_RUNTIME === '1') {
    return {
      kind: 'disabled',
      output: 'Runtime grounding disabled for this hook invocation. The agent must use codestory://status before source reads.',
    };
  }

  const command = hookCommand(input, event);
  if (!command) return null;
  const cli = process.env.CODESTORY_CLI || 'codestory-cli';

  const result = spawnSync(cli, command.args, {
    cwd: input.cwd || process.cwd(),
    encoding: 'utf8',
    timeout: RUNTIME_TIMEOUT_MS,
    maxBuffer: MAX_OUTPUT_CHARS * 4,
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(cli),
    windowsHide: true,
  });

  if (result.status === 0 && result.stdout.trim()) {
    return {
      kind: command.kind,
      output: truncate(result.stdout),
      next: command.next,
    };
  }

  const reason = result.error
    ? result.error.message
    : truncate(result.stderr || `codestory-cli exited with status ${result.status}`);

  return {
    kind: command.kind,
    output: [
      `CodeStory hook attempted ${command.kind} but did not receive usable output.`,
      reason ? `Reason: ${reason}` : null,
      command.next,
    ].filter(Boolean).join('\n'),
  };
}

function buildContext(input, event) {
  const runtime = runCodeStory(input, event);
  const parts = [getCodeStoryInstructions(event, input)];

  if (runtime && runtime.output) {
    parts.push([
      `CODESTORY HOOK ${runtime.kind.toUpperCase()}`,
      runtime.output,
      runtime.next ? `Next: ${runtime.next}` : null,
    ].filter(Boolean).join('\n\n'));
  }

  return parts.join('\n\n');
}

readHookInput().then((input) => {
  const event = input.hook_event_name || 'SessionStart';
  try {
    rememberActiveState({
      event,
      cwd: input.cwd || process.cwd(),
      source: input.source || input.trigger || null,
    });
    writeHookOutput(event, buildContext(input, event));
  } catch (e) {
    // Best effort only. A hook failure must not block the agent session.
  }
});
