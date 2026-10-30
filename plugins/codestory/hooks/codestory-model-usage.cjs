'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { TextDecoder } = require('node:util');

const HEAD_BYTES = 64 * 1024;
const TAIL_BYTES = 1024 * 1024;
const USAGE_BYTES = 8 * 1024;
const MAX_ROWS = 2048;
const decoder = new TextDecoder('utf-8', { fatal: true });
const counters = ['input_tokens', 'cached_input_tokens', 'cache_write_input_tokens',
  'output_tokens', 'reasoning_output_tokens', 'total_tokens'];
const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,192}$/u.test(value);
const absolute = value => typeof value === 'string' && value.length <= 4096
  && path.isAbsolute(value) && !value.includes('\0') && !value.split('/').includes('..');
const identity = (a, b) => a.dev === b.dev && a.ino === b.ino && a.mode === b.mode;
const snapshot = (a, b) => identity(a, b) && a.size === b.size
  && a.mtimeNs === b.mtimeNs && a.ctimeNs === b.ctimeNs;
const stat = name => fs.lstatSync(name, { bigint: true });

function timestamp(value, now) {
  if (typeof value !== 'string' || value.length > 40) throw new Error('timestamp');
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed) || parsed > now) throw new Error('timestamp');
  return parsed;
}

function owner(record, sessionId, now) {
  const p = record?.payload;
  if (record?.type !== 'session_meta' || p?.cli_version !== '0.164.0'
      || p.thread_source !== 'user' || p.source !== 'exec'
      || p.id !== sessionId || p.session_id !== sessionId) throw new Error('owner');
  return timestamp(record.timestamp, now);
}

function recordKind(line) {
  // This is the observed 0.164.0 record layout, not a recursive field search.
  const match = /^\{"timestamp":"[^"\n]{1,40}",(?:"ordinal":(?:0|[1-9][0-9]*),)?"type":"([a-z_]{1,48})","payload":/u.exec(line);
  if (!match) throw new Error('record layout');
  return match[1];
}

function lastUsage(head, tail, offset, sessionId, now) {
  const end = head.indexOf(10);
  if (end < 0) throw new Error('header bound');
  const first = decoder.decode(head.subarray(0, end));
  const initial = JSON.parse(first);
  if (recordKind(first) !== initial?.type || JSON.stringify(initial) !== first) throw new Error('header format');
  const created = owner(initial, sessionId, now);
  if (tail[tail.length - 1] !== 10) throw new Error('partial record');
  if (offset > 0) {
    const boundary = tail.indexOf(10);
    if (boundary < 0) throw new Error('tail bound');
    tail = tail.subarray(boundary + 1);
  }
  const lines = decoder.decode(tail).split('\n');
  lines.pop();
  if (lines.length > MAX_ROWS) throw new Error('row bound');
  const seen = new Map();
  let latest = null;
  let previous = created;
  for (const [index, line] of lines.entries()) {
    const kind = recordKind(line);
    if (kind === 'token_usage_record' && Buffer.byteLength(line) > USAGE_BYTES) throw new Error('usage bound');
    if (kind === 'session_meta' && Buffer.byteLength(line) > HEAD_BYTES) throw new Error('header bound');
    const record = offset === 0 && index === 0 ? initial : JSON.parse(line);
    if (record?.type !== kind) throw new Error('record discriminator');
    if (kind !== 'session_meta' && kind !== 'token_usage_record') continue;
    if (JSON.stringify(record) !== line) throw new Error('metadata format');
    if (kind === 'session_meta') {
      if (owner(record, sessionId, now) !== created) throw new Error('owner conflict');
      continue;
    }
    const p = record.payload;
    if (!p || p.thread_id !== sessionId || p.session_id !== sessionId
        || !id(p.turn_id) || p.root_turn_id !== p.turn_id || !id(p.response_id)) throw new Error('scope');
    const u = p.usage;
    if (!u || Object.keys(u).length !== counters.length
        || counters.some(key => !Number.isSafeInteger(u[key]) || u[key] < 0)
        || !Number.isSafeInteger(u.input_tokens + u.output_tokens)
        || u.total_tokens !== u.input_tokens + u.output_tokens
        || u.cached_input_tokens > u.input_tokens || u.cache_write_input_tokens > u.input_tokens
        || u.reasoning_output_tokens > u.output_tokens) throw new Error('counters');
    const time = timestamp(record.timestamp, now);
    if (time < previous) throw new Error('order');
    const fingerprint = JSON.stringify([record.timestamp, p.turn_id, ...counters.map(key => u[key])]);
    if (seen.has(p.response_id)) {
      if (seen.get(p.response_id) !== fingerprint) throw new Error('response conflict');
    } else {
      if (latest && time === previous) throw new Error('ambiguous order');
      seen.set(p.response_id, fingerprint);
    }
    latest = u;
    previous = time;
  }
  return latest;
}

function readWindow(fd, count, offset) {
  const buffer = Buffer.alloc(count);
  if (fs.readSync(fd, buffer, 0, count, offset) !== count) throw new Error('short read');
  return buffer;
}

function observe(event, home) {
  let rootFd;
  let fileFd;
  try {
    const c = fs.constants;
    if (!['darwin', 'linux'].includes(process.platform)
        || !c.O_NOFOLLOW || !c.O_NONBLOCK || !c.O_DIRECTORY
        || !absolute(home) || !absolute(event?.transcript_path) || !id(event?.session_id)) return null;
    const sessions = path.join(home, 'sessions');
    const relative = path.relative(sessions, event.transcript_path);
    const parts = relative.split(path.sep);
    if (!relative || path.isAbsolute(relative) || parts.some(part => !part || part === '..' || part === '.')
        || parts.length > 32 || path.extname(event.transcript_path) !== '.jsonl') return null;

    // The host's native home is trusted. Check every descendant without following
    // static symlinks; Node cannot make ancestor traversal atomic against same-user ABA swaps.
    const directories = [];
    let current = home;
    for (const part of [null, 'sessions', ...parts.slice(0, -1)]) {
      if (part !== null) current = path.join(current, part);
      const before = stat(current);
      if (!before.isDirectory() || before.isSymbolicLink()) return null;
      directories.push({ name: current, before });
    }
    const canonicalHome = fs.realpathSync(home);
    if (fs.realpathSync(event.transcript_path) !== path.join(canonicalHome, 'sessions', ...parts)) return null;
    const flags = c.O_RDONLY | c.O_NOFOLLOW | c.O_NONBLOCK;
    rootFd = fs.openSync(home, flags | c.O_DIRECTORY);
    if (!identity(directories[0].before, fs.fstatSync(rootFd, { bigint: true }))) return null;
    const before = stat(event.transcript_path);
    if (!before.isFile() || before.isSymbolicLink() || before.size <= 0n
        || before.size > BigInt(Number.MAX_SAFE_INTEGER)
        || typeof before.mtimeNs !== 'bigint' || typeof before.ctimeNs !== 'bigint') return null;
    fileFd = fs.openSync(event.transcript_path, flags);
    const opened = fs.fstatSync(fileFd, { bigint: true });
    if (!opened.isFile() || !snapshot(before, opened)) return null;
    const size = Number(opened.size);
    const head = readWindow(fileFd, Math.min(size, HEAD_BYTES), 0);
    const offset = Math.max(0, size - TAIL_BYTES);
    const tail = offset === 0 && size <= HEAD_BYTES ? head : readWindow(fileFd, size - offset, offset);
    const usage = lastUsage(head, tail, offset, event.session_id, Date.now());
    if (!snapshot(opened, fs.fstatSync(fileFd, { bigint: true }))
        || !snapshot(opened, stat(event.transcript_path))
        || !identity(directories[0].before, fs.fstatSync(rootFd, { bigint: true }))
        || directories.some(directory => !identity(directory.before, stat(directory.name)))
        || fs.realpathSync(home) !== canonicalHome) return null;
    if (!usage) return null;
    const context = `Last observed response: input_tokens=${usage.input_tokens}; output_tokens=${usage.output_tokens}; total_tokens=${usage.total_tokens}. Cached input already included. Scope: parent transcript; may lag; helper totals UNKNOWN.`;
    return Buffer.byteLength(context) <= 384 ? context : null;
  } catch {
    return null;
  } finally {
    for (const fd of [fileFd, rootFd]) {
      if (fd !== undefined) { try { fs.closeSync(fd); } catch { /* Optional observation. */ } }
    }
  }
}

module.exports = { observe };
