#!/usr/bin/env node

const {
  rememberActiveState,
  writeHookOutput,
  resolveCli,
  pluginDataDir,
} = require('./codestory-runtime.cjs');

const path = require('path');
const fs = require('fs');
const { spawnSync } = require('child_process');
const { TextDecoder } = require('util');

const MAX_SKILL_BYTES = 4096;
const MAX_CONTEXT_BYTES = 8192;

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

function canonicalSkillContext() {
  const skillPath = path.join(__dirname, '..', 'skills', 'codestory-grounding', 'SKILL.md');
  let fd;
  try {
    const metadata = fs.lstatSync(skillPath);
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > MAX_SKILL_BYTES) return null;
    fd = fs.openSync(skillPath, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0)
      | (fs.constants.O_NONBLOCK || 0));
    const opened = fs.fstatSync(fd);
    if (!opened.isFile() || opened.size > MAX_SKILL_BYTES) return null;
    const bytes = Buffer.alloc(MAX_SKILL_BYTES + 1);
    let size = 0;
    while (size < bytes.length) {
      const count = fs.readSync(fd, bytes, size, bytes.length - size, null);
      if (count === 0) break;
      size += count;
    }
    if (size > MAX_SKILL_BYTES) return null;
    const body = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes.subarray(0, size));
    if (!body.trim()) return null;
    const context = `CODESTORY GROUNDING AVAILABLE\n\nCanonical skill from ${JSON.stringify(skillPath)} (resolve relative references from this file):\n\n${body}`;
    return Buffer.byteLength(context, 'utf8') <= MAX_CONTEXT_BYTES ? context : null;
  } catch { return null; }
  finally { if (fd !== undefined) { try { fs.closeSync(fd); } catch {} } }
}

function startupContext(input, context) {
  if (input.hook_event_name !== 'SessionStart' || input.source !== 'startup'
      || typeof input.cwd !== 'string' || !path.isAbsolute(input.cwd)) return context;
  const initial = canonicalSkillContext() || context;
  try {
    const cli = resolveCli([], pluginDataDir());
    if (!cli) return initial;
    const result = spawnSync(cli, ['internal-session-context', '--project', input.cwd], {
      encoding: 'utf8', shell: false, windowsHide: true, timeout: 2000, maxBuffer: 16 * 1024,
    });
    if (result.error || result.status !== 0) return initial;
    const output = JSON.parse(result.stdout);
    if (output?.schema_version !== 1 || output?.status !== 'ready'
        || typeof output.context !== 'string' || !output.context.trim()
        || Buffer.byteLength(output.context, 'utf8') > 3584) return initial;
    const combined = `${initial}\n\n${output.context}`;
    return Buffer.byteLength(combined, 'utf8') <= MAX_CONTEXT_BYTES ? combined : initial;
  } catch { return initial; }
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
