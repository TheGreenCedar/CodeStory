const fs = require('fs');
const path = require('path');
const { createHash } = require('crypto');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);

const STATE_FILE = '.codestory-active';
const THREAD_STATE_PREFIX = '.codestory-active-thread-';
const DIRTY_MARKER_SCHEMA_VERSION = 1;
const DIRTY_MARKER_SAMPLE_LIMIT = 20;

function pluginDataDir() {
  if (process.env.PLUGIN_DATA) return process.env.PLUGIN_DATA;
  if (process.env.COPILOT_PLUGIN_DATA) return process.env.COPILOT_PLUGIN_DATA;
  if (process.env.CODESTORY_PLUGIN_DATA) return process.env.CODESTORY_PLUGIN_DATA;
  return null;
}

function stateFilePath() {
  const stateDir = pluginDataDir();
  return stateDir ? path.join(stateDir, STATE_FILE) : null;
}

function threadStateFilePath(threadId) {
  const stateDir = pluginDataDir();
  const normalized = String(threadId || '').trim();
  if (!stateDir || !normalized) return null;
  const key = createHash('sha256').update(normalized).digest('hex').slice(0, 16);
  return path.join(stateDir, `${THREAD_STATE_PREFIX}${key}.json`);
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function normalizeProjectRoot(projectRoot) {
  const resolved = path.resolve(projectRoot || process.cwd());
  try {
    return fs.realpathSync(resolved);
  } catch {
    return resolved;
  }
}

function dirtyMarkerPathForProject(projectRoot, dataDir = pluginDataDir()) {
  if (!dataDir || !projectRoot) return null;
  const normalizedRoot = normalizeProjectRoot(projectRoot);
  const key = createHash('sha256').update(normalizedRoot).digest('hex').slice(0, 32);
  return path.join(dataDir, 'dirty-markers', `${key}.json`);
}

function readActiveState() {
  const file = stateFilePath();
  return file ? readJson(file) : null;
}

function rememberActiveState(state) {
  const file = stateFilePath();
  if (!file) return;

  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const previous = readActiveState() || {};
    const nextState = {
      ...previous,
      ...state,
      hook: {
        ...(previous.hook || {}),
        ...(state.hook || {}),
      },
      updatedAt: new Date().toISOString(),
    };
    fs.writeFileSync(file, JSON.stringify(nextState));
    const threadFile = threadStateFilePath(nextState.codexThreadId);
    if (threadFile) {
      fs.writeFileSync(threadFile, JSON.stringify(nextState));
    }
  } catch (e) {
    // Best effort only. Hook state must not block the host session.
  }
}

function writeDirtyMarker(projectRoot, options = {}) {
  const markerPath = dirtyMarkerPathForProject(projectRoot, options.pluginDataDir);
  if (!markerPath) return null;
  const normalizedRoot = normalizeProjectRoot(projectRoot);
  const pathSample = Array.isArray(options.pathSample)
    ? options.pathSample
      .filter((item) => typeof item === 'string' && item.trim())
      .slice(0, DIRTY_MARKER_SAMPLE_LIMIT)
    : [];
  const marker = {
    schema_version: DIRTY_MARKER_SCHEMA_VERSION,
    project_root: normalizedRoot,
    dirty: Boolean(options.dirty),
    updated_at: new Date().toISOString(),
    source: String(options.source || 'codestory-hook'),
  };
  if (pathSample.length > 0) {
    marker.path_sample = pathSample;
  }

  try {
    fs.mkdirSync(path.dirname(markerPath), { recursive: true });
    const existing = readJson(markerPath);
    const existingSample = Array.isArray(existing?.path_sample) ? existing.path_sample : [];
    if (
      existing?.schema_version === marker.schema_version
      && existing?.project_root === marker.project_root
      && existing?.dirty === marker.dirty
      && existing?.source === marker.source
      && JSON.stringify(existingSample) === JSON.stringify(pathSample)
    ) {
      return { path: markerPath, marker: existing, unchanged: true };
    }
    fs.writeFileSync(markerPath, JSON.stringify(marker, null, 2));
    return { path: markerPath, marker };
  } catch {
    return null;
  }
}

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify({ additionalContext: context }));
    return;
  }

  if (isCodex) {
    const output = {
      systemMessage: 'CODESTORY:BACKGROUND',
    };
    if (context) {
      output.hookSpecificOutput = {
        hookEventName: event,
        additionalContext: context,
      };
    }
    process.stdout.write(JSON.stringify(output));
    return;
  }

  process.stdout.write(context);
}

module.exports = {
  dirtyMarkerPathForProject,
  readActiveState,
  rememberActiveState,
  writeDirtyMarker,
  writeHookOutput,
};
