#!/usr/bin/env node

const { spawnSync } = require('child_process');
const { eventHeader, getCodeStoryInstructions } = require('./codestory-instructions.cjs');
const {
  classifyMcpRuntime,
  mcpDetectionText,
  readActiveState,
  rememberActiveState,
  writeHookOutput,
} = require('./codestory-runtime.cjs');

const MAX_OUTPUT_CHARS = 4000;
const RUNTIME_TIMEOUT_MS = 3500;
const SOURCE_FALLBACK = 'CodeStory is unavailable for this session. Use bounded source reads in the target repo; inspect only task-named files and nearby tests.';

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

function firstRuntimeFailure(mcp) {
  if (!mcp.mcp_config_installed) {
    return `MCP: no codestory server configured at ${mcp.mcp_config_path}`;
  }
  if (!mcp.mcp_process_launchable) {
    return 'MCP: configured codestory server is not launchable from the plugin root';
  }
  if (!mcp.mcp_resources_exposed) {
    return `MCP: ${mcp.mcp_resource_status}`;
  }
  if (!mcp.managed_cli_present) {
    return 'managed runtime: no managed CLI manifest or runtime state was found';
  }
  return 'runtime: no usable grounding surface was found';
}

function shortDegradedNotice(mcp, reason, state = {}) {
  const hook = state.hook || {};
  return [
    'CodeStory degraded mode: no MCP or managed runtime surface is usable.',
    `First failing layer: ${firstRuntimeFailure(mcp)}.`,
    reason ? `Reason: ${truncate(reason)}` : null,
    hook.preflight_failed ? SOURCE_FALLBACK : 'Run one bounded managed/local-dev preflight if available; otherwise use bounded source reads.',
  ].filter(Boolean).join('\n');
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
      next: 'If CodeStory is unavailable, use bounded source reads in the target repo instead of repeated repair attempts.',
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
      next: 'If CodeStory is unavailable, use bounded source reads in the target repo instead of repeated repair attempts.',
    };
  }

  return null;
}

function runCodeStory(input, event, state = {}) {
  if (process.env.CODESTORY_HOOK_DISABLE_RUNTIME === '1') {
    return {
      kind: 'disabled',
      output: 'Runtime grounding disabled for this hook invocation. The agent must use codestory://status before source reads.',
    };
  }

  const command = hookCommand(input, event);
  if (!command) return null;
  const mcp = classifyMcpRuntime();
  if (mcp.mcp_config_installed && mcp.mcp_process_launchable) {
    return {
      kind: 'mcp detection',
      output: [
        mcpDetectionText(mcp),
        mcp.mcp_resources_exposed
          ? 'Use codestory://status as the active runtime truth before CLI fallback.'
          : 'CodeStory MCP is configured and launchable, but MCP resources are not visible to this hook/model context. Reload the host/plugin and read codestory://status; do not add CodeStory to PATH.',
      ].join('\n'),
    };
  }
  if (!process.env.CODESTORY_CLI && !mcp.managed_cli_present) {
    return {
      degraded: true,
      kind: 'degraded mode',
      output: shortDegradedNotice(mcp, null, state),
    };
  }

  const cli = process.env.CODESTORY_CLI || mcp.managed_cli_path;

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

  const failedState = {
    ...state,
    hook: {
      ...(state.hook || {}),
      preflight_failed: true,
    },
  };

  return {
    degraded: mcp.degraded_no_surface,
    preflightFailed: true,
    kind: command.kind,
    output: mcp.degraded_no_surface
      ? shortDegradedNotice(mcp, reason, failedState)
      : [
        mcpDetectionText(mcp),
        `CodeStory hook attempted ${command.kind} but did not receive usable output.`,
        reason ? `Reason: ${reason}` : null,
        command.next,
      ].filter(Boolean).join('\n'),
  };
}

function buildContext(input, event, state = {}) {
  const runtime = runCodeStory(input, event, state);
  const runtimeBlock = runtime && runtime.output
    ? [
      `CODESTORY HOOK ${runtime.kind.toUpperCase()}`,
      runtime.output,
      runtime.next ? `Next: ${runtime.next}` : null,
    ].filter(Boolean).join('\n\n')
    : null;

  if (runtime && runtime.kind === 'mcp detection') {
    return [eventHeader(event, input).trim(), runtimeBlock].filter(Boolean).join('\n\n');
  }

  if (runtime && runtime.degraded) {
    return [eventHeader(event, input).trim(), runtimeBlock].filter(Boolean).join('\n\n');
  }

  const emitted = state.hook && state.hook.instructions_emitted;
  const alreadyEmitted = emitted && emitted[event];
  const parts = [alreadyEmitted ? eventHeader(event, input).trim() : getCodeStoryInstructions(event, input)];

  if (runtimeBlock) {
    parts.push(runtimeBlock);
  }

  return parts.join('\n\n');
}

function freshInstructionBoundary(event, input = {}) {
  const source = String(input.source || input.trigger || '').toLowerCase();
  return event === 'SessionStart' && (!source || source === 'startup');
}

readHookInput().then((input) => {
  const event = input.hook_event_name || 'SessionStart';
  try {
    const state = readActiveState() || {};
    const activeState = freshInstructionBoundary(event, input)
      ? {
        ...state,
        hook: {
          ...(state.hook || {}),
          instructions_emitted: {},
        },
      }
      : state;
    const context = buildContext(input, event, activeState);
    const priorInstructions = (activeState.hook && activeState.hook.instructions_emitted) || {};
    const emittedFullInstructions = context.includes('CODESTORY BACKGROUND GROUNDING RULES') ||
      context.includes('CODESTORY BACKGROUND GROUNDING ACTIVE');
    const instructions = emittedFullInstructions
      ? { ...priorInstructions, [event]: true }
      : priorInstructions;
    rememberActiveState({
      event,
      cwd: input.cwd || process.cwd(),
      source: input.source || input.trigger || null,
      hook: {
        instructions_emitted: instructions,
        preflight_failed: state.hook && state.hook.preflight_failed,
      },
    });
    const nextState = readActiveState() || {};
    if (/CodeStory is unavailable for this session/u.test(context)) {
      rememberActiveState({ hook: { ...(nextState.hook || {}), preflight_failed: true } });
    }
    writeHookOutput(event, context);
  } catch (e) {
    // Best effort only. A hook failure must not block the agent session.
  }
});
