#!/usr/bin/env node

const { spawnSync } = require('child_process');
const { createHash } = require('crypto');
const { eventHeader, getCodeStoryInstructions } = require('./codestory-instructions.cjs');
const {
  bootstrapManagedRuntime,
  classifyMcpRuntime,
  dirtyMarkerPathForProject,
  mcpDetectionText,
  readActiveState,
  readHookState,
  rememberActiveState,
  writeDirtyMarker,
  writeHookState,
  writeHookOutput,
} = require('./codestory-runtime.cjs');

const MAX_OUTPUT_CHARS = 4000;
const EVENT_CAPS = {
  startup: 4000,
  clear: 4000,
  child_worktree_start: 4000,
  user_prompt: 4000,
  resume: 2200,
  compact: 2200,
  handoff: 2200,
  goal_heartbeat: 1400,
  session: 3000,
};
const RUNTIME_TIMEOUT_MS = 3500;
const SOURCE_FALLBACK = 'CodeStory is unavailable for this session. Use bounded source reads in the target repo; inspect only task-named files and nearby tests.';
const BOOTSTRAP_TAXONOMIES = new Set(['startup', 'clear', 'child_worktree_start', 'session']);

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

function truncate(text, maxChars = MAX_OUTPUT_CHARS) {
  const value = String(text || '').trim();
  if (value.length <= maxChars) return value;
  return `${value.slice(0, maxChars)}\n\n... CodeStory hook output truncated by hook budget.`;
}

function normalizeText(text) {
  return String(text || '').replace(/\s+/g, ' ').trim();
}

function hashText(text) {
  return createHash('sha256').update(text).digest('hex').slice(0, 16);
}

function compactKey(text, maxChars = 160) {
  return normalizeText(text).slice(0, maxChars);
}

function hookTaxonomy(input, event) {
  const source = compactKey(input.source || input.trigger || input.reason || '').toLowerCase();
  const combined = `${String(event || '').toLowerCase()} ${source}`;
  if (event === 'UserPromptSubmit') return 'user_prompt';
  if (/goal|heartbeat/u.test(combined)) return 'goal_heartbeat';
  if (/compact/u.test(combined)) return 'compact';
  if (/resume/u.test(combined)) return 'resume';
  if (/clear/u.test(combined)) return 'clear';
  if (/handoff/u.test(combined)) return 'handoff';
  if (/child|worktree/u.test(combined)) return 'child_worktree_start';
  if (/start|startup/u.test(combined)) return 'startup';
  return 'session';
}

function hookPolicy(input, event) {
  const taxonomy = hookTaxonomy(input, event);
  const project = input.cwd || process.cwd();
  const promptKey = `prompt:${hashText(normalizeText(input.prompt || ''))}`;
  const dedupeBase = `${taxonomy}:${project}`;
  return {
    taxonomy,
    project,
    cap: EVENT_CAPS[taxonomy] || EVENT_CAPS.session,
    dedupeKey: taxonomy === 'user_prompt' ? `${dedupeBase}:${promptKey}` : dedupeBase,
    resetDedupe: taxonomy === 'startup' || taxonomy === 'clear',
    runtimeOnly: ['resume', 'compact', 'handoff', 'goal_heartbeat'].includes(taxonomy),
    heartbeat: taxonomy === 'goal_heartbeat',
  };
}

function gitDirtyState(project) {
  if (!project) return null;
  const result = spawnSync('git', ['-C', project, 'status', '--porcelain'], {
    encoding: 'utf8',
    timeout: 1000,
    maxBuffer: 20_000,
    windowsHide: true,
  });
  if (result.status !== 0 || result.error) return null;
  const paths = result.stdout
    .split(/\r?\n/u)
    .map((line) => line.slice(3).trim())
    .filter(Boolean)
    .slice(0, 20);
  return {
    dirty: paths.length > 0,
    pathSample: paths,
  };
}

function writeProjectDirtyMarker(policy) {
  if (!dirtyMarkerPathForProject(policy.project)) return;
  const state = gitDirtyState(policy.project);
  if (!state) return;
  writeDirtyMarker(policy.project, {
    dirty: state.dirty,
    pathSample: state.pathSample,
    source: `codestory-hook:${policy.taxonomy}`,
  });
}

function runtimeFingerprint(mcp) {
  return [
    mcp.mcp_resource_status,
    mcp.mcp_model_visible_blocked ? 'model-hidden' : 'model-visible-or-unlaunchable',
    mcp.managed_cli_present ? 'managed' : 'no-managed',
    mcp.degraded_no_surface ? 'degraded' : 'surface',
  ].join('|');
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
  if (mcp.mcp_model_visible_blocked) {
    return [
      'CodeStory setup blocked: MCP is configured and launchable, but resources are not model-visible.',
      `First failing layer: ${firstRuntimeFailure(mcp)}.`,
      reason ? `Reason: ${truncate(reason, 600)}` : null,
      'Request CodeStory MCP through host deferred discovery/tool_search when available; otherwise use the hook MCP bridge when present. Reload only after plugin install or config changes. Do not use ambient CodeStory CLI discovery as grounding.',
    ].filter(Boolean).join('\n');
  }
  const hook = state.hook || {};
  return [
    'CodeStory degraded mode: no MCP or managed runtime surface is usable.',
    `First failing layer: ${firstRuntimeFailure(mcp)}.`,
    reason ? `Reason: ${truncate(reason, 600)}` : null,
    hook.preflight_failed ? SOURCE_FALLBACK : 'Run one bounded managed/local-dev preflight if available; otherwise use bounded source reads.',
  ].filter(Boolean).join('\n');
}

function runtimeStatusBlock(policy, mcp) {
  return [
    'CODESTORY HOOK RUNTIME TRUTH',
    `event_taxonomy: ${policy.taxonomy}`,
    `output_cap_chars: ${policy.cap}`,
    `dedupe_key: ${policy.dedupeKey}`,
    mcpDetectionText(mcp),
    mcp.mcp_model_visible_blocked
      ? 'Next: request CodeStory MCP through host deferred discovery/tool_search when available; otherwise use the hook MCP bridge when present. Reload only after plugin install or config changes. Do not use ambient CodeStory CLI discovery as grounding.'
      : mcp.mcp_resources_exposed
      ? 'Next: read codestory://status before source reads; use packet/search/context only when status allows them with retrieval_mode=full.'
      : 'Next: no sidecar-backed packet/search surface is proven available; use bounded source reads instead of repeated repair attempts.',
  ].join('\n');
}

function shouldBootstrapManagedRuntime(policy, mcp) {
  if (process.env.CODESTORY_CLI) return false;
  if (!BOOTSTRAP_TAXONOMIES.has(policy.taxonomy)) return false;
  return Boolean(policy.project && mcp.mcp_process_launchable && mcp.mcp_resources_exposed && !mcp.managed_cli_present);
}

function bootstrapText(bootstrap) {
  if (!bootstrap || !bootstrap.attempted) return null;
  const status = bootstrap.parsed || {};
  const plugin = status.plugin_runtime || {};
  const localRefresh = status.local_refresh || status.readiness?.[0]?.local_refresh || {};
  const reason = status.degraded_reason || status.readiness?.[0]?.repair_reason || bootstrap.error || bootstrap.stderr;
  return [
    `managed_bootstrap: ${bootstrap.ready ? 'ready' : 'blocked'}`,
    `managed_bootstrap_cli_source: ${plugin.cli_source || '<unknown>'}`,
    plugin.managed_binary_path ? `managed_bootstrap_cli_path: ${plugin.managed_binary_path}` : null,
    `managed_bootstrap_local_refresh: ${localRefresh.state || status.readiness?.[0]?.status || '<unknown>'}`,
    reason ? `managed_bootstrap_reason: ${truncate(reason, 500)}` : null,
  ].filter(Boolean).join('\n');
}

function firstFreshness(value) {
  if (!value || typeof value !== 'object') return null;
  for (const [key, nested] of Object.entries(value)) {
    if ((key === 'freshness' || key === 'index_freshness') && nested && typeof nested === 'object') {
      return nested;
    }
    const found = firstFreshness(nested);
    if (found) return found;
  }
  return null;
}

function freshnessSummary(value) {
  const freshness = firstFreshness(value);
  if (!freshness) return 'not_reported';
  const status = freshness.status || 'unknown';
  const changed = freshness.changed_file_count ?? freshness.changed_files ?? 'unknown';
  const added = freshness.new_file_count ?? freshness.new_files ?? 'unknown';
  const removed = freshness.removed_file_count ?? freshness.removed_files ?? 'unknown';
  return `${status} changed=${changed} new=${added} removed=${removed}`;
}

function readinessEvidence(parsed) {
  const verdicts = Array.isArray(parsed.verdicts) ? parsed.verdicts : [];
  const statuses = verdicts
    .map((verdict) => `${verdict?.goal || 'unknown'}=${verdict?.status || 'unknown'}`)
    .join(' ') || 'none';
  const evidence = {
    statuses,
    freshness: freshnessSummary(parsed),
  };
  return {
    text: [
      `agent_readiness_evidence: ${evidence.statuses}`,
      `freshness_evidence: ${evidence.freshness}`,
    ].join('\n'),
    fingerprint: hashText(JSON.stringify(evidence)),
  };
}

function bridgeAllowedSurfaces(bootstrap, readiness) {
  const surfaces = [];
  if (bootstrap?.ready) surfaces.push('local_navigation');
  if (readiness?.ready) surfaces.push('agent_packet_search');
  return surfaces.length > 0 ? surfaces.join(',') : 'status_only';
}

function bridgeBootstrapForPolicy(policy, mcp, bootstrap) {
  if (bootstrap?.attempted) return bootstrap;
  if (!policy.project || !mcp.mcp_process_launchable) {
    return { attempted: false, reason: 'mcp_launcher_unavailable' };
  }
  if (!mcp.managed_cli_present && !BOOTSTRAP_TAXONOMIES.has(policy.taxonomy)) {
    return { attempted: false, reason: 'managed_runtime_not_present' };
  }
  return bootstrapManagedRuntime({ projectRoot: policy.project });
}

function managedHookCli(mcp) {
  return process.env.CODESTORY_CLI || mcp.managed_cli_path || null;
}

function bridgeStatusText(policy, mcp, bootstrap, readiness, commandReason) {
  const status = bootstrap?.parsed || {};
  const plugin = status.plugin_runtime || {};
  const localRefresh = status.local_refresh || status.readiness?.[0]?.local_refresh || {};
  const reason = status.degraded_reason
    || status.readiness?.[0]?.repair_reason
    || bootstrap?.reason
    || bootstrap?.error
    || bootstrap?.stderr;
  return [
    'CODESTORY HOOK MCP BRIDGE',
    'bridge_context_label: hook-bridged context, not live MCP tools',
    'bridge_resource_uri: codestory://status',
    `event_taxonomy: ${policy.taxonomy}`,
    `output_cap_chars: ${policy.cap}`,
    `dedupe_key: ${policy.dedupeKey}`,
    mcpDetectionText(mcp),
    `hook_bridge_status: ${bootstrap?.ready ? 'ready' : 'blocked'}`,
    `hook_bridge_cli_source: ${plugin.cli_source || (process.env.CODESTORY_CLI ? 'local_dev_override' : mcp.managed_cli_source) || '<unknown>'}`,
    plugin.managed_binary_path ? `hook_bridge_managed_cli_path: ${plugin.managed_binary_path}` : null,
    `hook_bridge_local_refresh: ${localRefresh.state || status.readiness?.[0]?.status || '<unknown>'}`,
    `hook_bridge_allowed_surfaces: ${bridgeAllowedSurfaces(bootstrap, readiness)}`,
    readiness ? readiness.evidence : null,
    commandReason ? `hook_bridge_context: ${commandReason}` : null,
    reason ? `hook_bridge_reason: ${truncate(reason, 500)}` : null,
    'mcp_resources_exposed: mcp_resources_not_model_visible',
    'mcp_tools_visible: no',
    'hook_bridge_next: request CodeStory MCP through host deferred discovery/tool_search when available; otherwise use this bounded bridge status and report that live MCP tools are hidden.',
  ].filter(Boolean).join('\n');
}

function runManagedHookCommand(cli, command, cwd) {
  const result = spawnSync(cli, command.args, {
    cwd,
    encoding: 'utf8',
    timeout: RUNTIME_TIMEOUT_MS,
    maxBuffer: MAX_OUTPUT_CHARS * 4,
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(cli),
    windowsHide: true,
  });
  if (result.status === 0 && result.stdout.trim()) {
    return {
      ok: true,
      output: truncate(result.stdout),
      fingerprint: `${command.kind}:${hashText(result.stdout)}`,
    };
  }
  const reason = result.error
    ? result.error.message
    : truncate(result.stderr || `${command.kind} exited with status ${result.status} without usable output`);
  return {
    ok: false,
    reason,
    fingerprint: `${command.kind}:blocked:${hashText(reason)}`,
  };
}

function hookMcpBridge(input, policy, mcp, command, bootstrap) {
  const bridgeBootstrap = bridgeBootstrapForPolicy(policy, mcp, bootstrap);
  const bridgedMcp = bridgeBootstrap.attempted ? classifyMcpRuntime() : mcp;
  const cli = managedHookCli(bridgedMcp);
  const cwd = input.cwd || process.cwd();
  let readiness = null;
  let commandResult = null;
  let commandReason = command ? null : 'status_only';

  if (command && !cli) {
    commandReason = 'skipped_no_managed_cli';
  } else if (command?.kind === 'request packet') {
    readiness = runReadinessProbe(cli, policy.project, cwd);
    if (readiness.ready) {
      commandResult = runManagedHookCommand(cli, command, cwd);
      commandReason = commandResult.ok ? null : `skipped_${commandResult.reason}`;
    } else {
      commandReason = `skipped_${readiness.reason}`;
    }
  } else if (command?.kind === 'session ground') {
    if (bridgeBootstrap.ready) {
      commandResult = runManagedHookCommand(cli, command, cwd);
      commandReason = commandResult.ok ? null : `skipped_${commandResult.reason}`;
    } else {
      commandReason = 'skipped_local_navigation_not_ready';
    }
  }

  const bridge = bridgeStatusText(policy, bridgedMcp, bridgeBootstrap, readiness, commandReason);
  const commandBlock = commandResult?.ok
    ? [
      'CODESTORY HOOK MCP BRIDGE CONTEXT',
      `hook_bridge_command: ${command.kind}`,
      commandResult.output,
    ].join('\n')
    : null;
  return {
    kind: commandResult?.ok ? command.kind : 'mcp bridge',
    output: truncate([bridge, commandBlock].filter(Boolean).join('\n'), policy.cap),
    next: command?.next,
    fingerprint: [
      runtimeFingerprint(bridgedMcp),
      bridgeBootstrap.ready ? 'bridge-ready' : `bridge-blocked:${bridgeBootstrap.reason || 'status'}`,
      readiness?.fingerprint,
      commandResult?.fingerprint,
      commandReason,
    ].filter(Boolean).join('|'),
  };
}

function rememberEmission(policy, contentKey) {
  const state = readHookState();
  const emitted = state.emitted || {};
  const previous = policy.resetDedupe ? undefined : emitted[policy.dedupeKey];
  if (previous === contentKey) {
    return false;
  }
  state.emitted = {
    ...(policy.resetDedupe ? {} : emitted),
    [policy.dedupeKey]: contentKey,
  };
  writeHookState(state);
  return true;
}

function rememberHeartbeat(policy, contentKey) {
  const state = readHookState();
  const previous = state.heartbeatKey;
  writeHookState({
    ...state,
    heartbeatKey: contentKey,
  });
  return previous ? previous !== contentKey : false;
}

function hookCommand(input, event, policy) {
  const project = policy.project;
  if (!project) return null;

  if (policy.taxonomy === 'user_prompt' && String(input.prompt || '').trim()) {
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

  if (['startup', 'clear', 'child_worktree_start'].includes(policy.taxonomy)) {
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

function runReadinessProbe(cli, project, cwd) {
  const result = spawnSync(cli, [
    'ready',
    '--goal', 'agent',
    '--project', project,
    '--format', 'json',
  ], {
    cwd,
    encoding: 'utf8',
    timeout: RUNTIME_TIMEOUT_MS,
    maxBuffer: MAX_OUTPUT_CHARS * 4,
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(cli),
    windowsHide: true,
  });
  if (result.status !== 0 || !result.stdout.trim()) {
    const reason = result.error
      ? result.error.message
      : truncate(result.stderr || `agent readiness probe exited with status ${result.status}`);
    return {
      ready: false,
      reason,
      evidence: [
        'agent_readiness_evidence: unavailable',
        'freshness_evidence: unavailable',
      ].join('\n'),
      fingerprint: `readiness-unavailable:${hashText(reason)}`,
    };
  }
  try {
    const parsed = JSON.parse(result.stdout);
    const verdicts = Array.isArray(parsed.verdicts) ? parsed.verdicts : [];
    const evidence = readinessEvidence(parsed);
    const ready = verdicts.some((verdict) => verdict?.goal === 'agent_packet_search' && verdict?.status === 'ready');
    return {
      ready,
      reason: ready ? null : 'agent_packet_search readiness is not ready',
      evidence: evidence.text,
      fingerprint: `readiness:${evidence.fingerprint}`,
    };
  } catch {
    return {
      ready: false,
      reason: 'agent readiness probe returned invalid JSON',
      evidence: [
        'agent_readiness_evidence: invalid_json',
        'freshness_evidence: unavailable',
      ].join('\n'),
      fingerprint: 'readiness-invalid-json',
    };
  }
}

function heartbeatReadinessProbe(mcp, project, cwd) {
  const cli = process.env.CODESTORY_CLI || mcp.managed_cli_path;
  if (!cli) {
    return {
      evidence: [
        'agent_readiness_evidence: unavailable',
        'freshness_evidence: unavailable',
      ].join('\n'),
      fingerprint: 'readiness-unavailable:no-cli',
    };
  }
  return runReadinessProbe(cli, project, cwd);
}

function runCodeStory(input, event, policy, state = {}) {
  if (process.env.CODESTORY_HOOK_DISABLE_RUNTIME === '1') {
    return {
      kind: 'disabled',
      output: 'Runtime grounding disabled for this hook invocation. The agent must use codestory://status before source reads.',
    };
  }

  let mcp = classifyMcpRuntime();
  const bootstrap = shouldBootstrapManagedRuntime(policy, mcp)
    ? bootstrapManagedRuntime({ projectRoot: policy.project })
    : null;
  if (bootstrap?.attempted) {
    mcp = classifyMcpRuntime();
  }
  const bootstrapBlock = bootstrapText(bootstrap);
  if (policy.runtimeOnly) {
    if (mcp.mcp_config_installed && mcp.mcp_process_launchable && mcp.mcp_model_visible_blocked) {
      return hookMcpBridge(input, policy, mcp, null, bootstrap);
    }
    const heartbeatProbe = policy.heartbeat && !mcp.mcp_model_visible_blocked
      ? heartbeatReadinessProbe(mcp, policy.project, input.cwd || process.cwd())
      : null;
    const output = state.hook?.preflight_failed && mcp.degraded_no_surface
      ? [
        `event_taxonomy: ${policy.taxonomy}`,
        `output_cap_chars: ${policy.cap}`,
        `dedupe_key: ${policy.dedupeKey}`,
        shortDegradedNotice(mcp, null, state),
      ].join('\n')
      : runtimeStatusBlock(policy, mcp);
    return {
      kind: 'runtime truth',
      output: truncate([output, heartbeatProbe?.evidence].filter(Boolean).join('\n'), policy.cap),
      fingerprint: [runtimeFingerprint(mcp), heartbeatProbe?.fingerprint].filter(Boolean).join('|'),
    };
  }

  const command = hookCommand(input, event, policy);
  if (!command) return null;
  if (mcp.mcp_config_installed && mcp.mcp_process_launchable) {
    if (mcp.mcp_model_visible_blocked) {
      return hookMcpBridge(input, policy, mcp, command, bootstrap);
    }
    return {
      kind: 'mcp detection',
      output: truncate([
        `event_taxonomy: ${policy.taxonomy}`,
        `output_cap_chars: ${policy.cap}`,
        `dedupe_key: ${policy.dedupeKey}`,
        bootstrapBlock,
        mcpDetectionText(mcp),
        mcp.mcp_resources_exposed
          ? 'Use codestory://status as the active runtime truth. Run packet/search/context only when status allows that surface with retrieval_mode=full.'
          : 'CodeStory MCP is configured and launchable, but MCP resources are not visible to this hook/model context. Request CodeStory MCP through host deferred discovery/tool_search when available; otherwise use hook-bridged status and report that live MCP tools are hidden.',
      ].join('\n'), policy.cap),
      fingerprint: `${runtimeFingerprint(mcp)}|${bootstrapBlock || 'no-bootstrap'}`,
    };
  }

  if (!process.env.CODESTORY_CLI && !mcp.managed_cli_present) {
    return {
      degraded: true,
      kind: 'degraded mode',
      output: truncate([
        `event_taxonomy: ${policy.taxonomy}`,
        `output_cap_chars: ${policy.cap}`,
        `dedupe_key: ${policy.dedupeKey}`,
        shortDegradedNotice(mcp, null, state),
      ].join('\n'), policy.cap),
      fingerprint: `${runtimeFingerprint(mcp)}|no-cli`,
    };
  }

  const cli = process.env.CODESTORY_CLI || mcp.managed_cli_path;
  const readiness = policy.taxonomy === 'user_prompt'
    ? runReadinessProbe(cli, policy.project, input.cwd || process.cwd())
    : { ready: true, reason: null };
  if (policy.taxonomy === 'user_prompt' && !readiness.ready) {
    const failedState = {
      ...state,
      hook: {
        ...(state.hook || {}),
        preflight_failed: Boolean(readiness.reason),
      },
    };
    return {
      degraded: true,
      kind: 'runtime truth',
      output: truncate([
        `event_taxonomy: ${policy.taxonomy}`,
        `output_cap_chars: ${policy.cap}`,
        `dedupe_key: ${policy.dedupeKey}`,
        mcp.degraded_no_surface
          ? shortDegradedNotice(mcp, readiness.reason, failedState)
          : runtimeStatusBlock(policy, mcp),
        'Packet skipped: sidecar-backed packet/search readiness is not proven full for this hook invocation.',
      ].join('\n'), policy.cap),
      fingerprint: `${runtimeFingerprint(mcp)}|packet-not-ready`,
    };
  }

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
      output: truncate(result.stdout, policy.cap),
      next: command.next,
      fingerprint: `${runtimeFingerprint(mcp)}|${command.kind}`,
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
    output: truncate(mcp.degraded_no_surface
      ? [
        `event_taxonomy: ${policy.taxonomy}`,
        `output_cap_chars: ${policy.cap}`,
        `dedupe_key: ${policy.dedupeKey}`,
        shortDegradedNotice(mcp, reason, failedState),
      ].join('\n')
      : [
        mcpDetectionText(mcp),
        `CodeStory hook attempted ${command.kind} but did not receive usable output.`,
        reason ? `Reason: ${reason}` : null,
        command.next,
      ].filter(Boolean).join('\n'), policy.cap),
    fingerprint: `${runtimeFingerprint(mcp)}|${reason}`,
  };
}

function buildContext(input, event, state = {}) {
  const policy = hookPolicy(input, event);
  writeProjectDirtyMarker(policy);
  const runtime = runCodeStory(input, event, policy, state);
  if (runtime && policy.heartbeat && !rememberHeartbeat(policy, runtime.fingerprint || runtime.output)) {
    return null;
  }
  const runtimeBlock = runtime && runtime.output
    ? [
      `CODESTORY HOOK ${runtime.kind.toUpperCase()}`,
      runtime.output,
      runtime.next ? `Next: ${runtime.next}` : null,
    ].filter(Boolean).join('\n\n')
    : null;
  const contentKey = runtime?.fingerprint || runtimeBlock || policy.taxonomy;
  if (runtimeBlock && !rememberEmission(policy, contentKey)) {
    return null;
  }

  if (runtime && runtimeBlock && (runtime.kind === 'mcp detection' || runtimeBlock.includes('CODESTORY HOOK MCP BRIDGE'))) {
    return truncate([eventHeader(event, input).trim(), runtimeBlock].filter(Boolean).join('\n\n'), policy.cap);
  }

  if (policy.runtimeOnly || (runtime && runtime.degraded)) {
    return truncate([eventHeader(event, input).trim(), runtimeBlock].filter(Boolean).join('\n\n'), policy.cap);
  }

  const emitted = state.hook && state.hook.instructions_emitted;
  const alreadyEmitted = emitted && emitted[event];
  const parts = [alreadyEmitted ? eventHeader(event, input).trim() : getCodeStoryInstructions(event, input)];

  if (runtimeBlock) {
    parts.push(runtimeBlock);
  }

  return truncate(parts.join('\n\n'), policy.cap);
}

function freshInstructionBoundary(event, input = {}) {
  const source = String(input.source || input.trigger || '').toLowerCase();
  return event === 'SessionStart' && (!source || source === 'startup' || source === 'clear');
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
    const emittedFullInstructions = context && (
      context.includes('CODESTORY BACKGROUND GROUNDING RULES') ||
      context.includes('CODESTORY BACKGROUND GROUNDING ACTIVE')
    );
    const instructions = emittedFullInstructions
      ? { ...priorInstructions, [event]: true }
      : priorInstructions;
    rememberActiveState({
      event,
      cwd: input.cwd || process.cwd(),
      source: input.source || input.trigger || null,
      codexThreadId: process.env.CODEX_THREAD_ID || null,
      hook: {
        instructions_emitted: instructions,
        preflight_failed: state.hook && state.hook.preflight_failed,
      },
    });
    const nextState = readActiveState() || {};
    if (context && /CodeStory is unavailable for this session/u.test(context)) {
      rememberActiveState({ hook: { ...(nextState.hook || {}), preflight_failed: true } });
    }
    writeHookOutput(event, context);
  } catch (e) {
    // Best effort only. A hook failure must not block the agent session.
  }
});
