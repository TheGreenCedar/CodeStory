#!/usr/bin/env node
// Unregistered experimental adapter. Locator text is data, never shell input.
const path = require('path');
const { spawnSync } = require('child_process');
const { resolveCli, pluginDataDir, writeHookOutput } = require('./codestory-runtime.cjs');
const EVENT_BYTES = 64 * 1024;

function literalTokens(command) {
  if (/[\r\n]/u.test(command)) return null;
  // Recognize only whole whitespace-delimited literals. Refuse escapes,
  // composition and expansion rather than partially interpreting shell syntax.
  const token = /[ \t]*(?:"([^"\\$`]*)"|'([^'\\]*)'|([^ \t"'\\$`;&|<>(){}\[\]*?~!#]+))(?=[ \t]|$)/gy;
  const values = [];
  let offset = 0;
  while (offset < command.length) {
    token.lastIndex = offset;
    const match = token.exec(command);
    if (!match) return /^[ \t]*$/u.test(command.slice(offset)) ? values : null;
    values.push(match[1] ?? match[2] ?? match[3]);
    offset = token.lastIndex;
  }
  return values;
}

async function main() {
  let bytes = 0;
  const chunks = [];
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > EVENT_BYTES) return {};
    chunks.push(chunk);
  }
  const input = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)));
  if (input?.hook_event_name !== 'PostToolUse' || input.tool_name !== 'Bash'
      || typeof input.cwd !== 'string' || !path.isAbsolute(input.cwd)
      || typeof input.tool_input?.command !== 'string' || typeof input.tool_response !== 'string') return {};
  const locations = [];
  let limited = false;
  for (const line of input.tool_response.split(/\r?\n/u)) {
    const bare = /^([1-9]\d*):(.*)$/u.exec(line);
    const named = bare ? null : /^(.+?):([1-9]\d*):(.*)$/u.exec(line);
    if (!bare && !named) continue;
    const number = Number(bare ? bare[1] : named[2]);
    if (!Number.isSafeInteger(number) || number > 0xffffffff) continue;
    if (locations.length === 32) { limited = true; break; }
    locations.push({ path: bare ? null : named[1], line: number, text: bare ? bare[2] : named[3] });
  }
  if (!locations.length) return {};
  const inputPaths = [];
  if (locations.some((location) => location.path === null)) {
    const tokens = literalTokens(input.tool_input.command);
    if (!tokens) return {};
    // Keep every literal, even option-looking names: only runtime admission
    // establishes which tokens are files. Never discard a possible second path.
    for (const value of tokens) {
      if (value && !inputPaths.includes(value)) inputPaths.push(value);
      if (inputPaths.length > 32) return {};
    }
  }
  const cli = resolveCli([], pluginDataDir());
  if (!cli) return {};
  const result = spawnSync(cli, ['internal-navigation-context', '--project', input.cwd], {
    input: JSON.stringify({ locations, input_paths: inputPaths, candidate_limit_reached: limited }),
    encoding: 'utf8', shell: false, windowsHide: true, timeout: 2000, maxBuffer: 16 * 1024,
  });
  if (result.error || result.status !== 0) return {};
  const output = JSON.parse(result.stdout);
  if (output?.schema_version !== 1 || output.status !== 'ready' || typeof output.context !== 'string'
      || !output.context.trim() || Buffer.byteLength(output.context, 'utf8') > 1024) return {};
  return output.context;
}

main().then((context) => {
  if (typeof context === 'string') writeHookOutput('PostToolUse', context);
  else process.stdout.write('{}');
}).catch(() => process.stdout.write('{}'));
