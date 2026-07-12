#!/usr/bin/env node

const { spawn } = require('child_process');
const { spawnSync } = require('child_process');
const { createHash, randomBytes } = require('crypto');
const fs = require('fs');
const http = require('http');
const https = require('https');
const os = require('os');
const path = require('path');
const { dirtyMarkerPathForProject } = require('../hooks/codestory-runtime.cjs');

const pluginRoot = path.dirname(__dirname);
const launchCwd = process.cwd();
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
const fallbackBinaryNames = process.platform === 'win32'
  ? ['codestory-cli.exe', 'codestory-cli.cmd', 'codestory-cli']
  : ['codestory-cli'];
const activeStateFile = '.codestory-active';
const activeThreadStatePrefix = '.codestory-active-thread-';
const sharedAgentRunId = 'shared-agent';
const releaseDownloadTimeoutMs = 60000;
const releaseDownloadAttempts = 3;
const releaseDownloadRetryDelaysMs = [1000, 3000];
const managedCliLockStaleMs = 10 * 60 * 1000;
const managedCliLockMaxAgeMs = 30 * 60 * 1000;
const managedCliLockWaitMs = 5 * 60 * 1000;
const managedCliPendingOwnerCleanupLimit = 64;
const managedCliQuarantineRetention = 2;

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function fileSha256(file) {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function findFile(root, names) {
  const entries = fs.readdirSync(root, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(root, entry.name);
    if (entry.isFile() && names.includes(entry.name)) return fullPath;
    if (entry.isDirectory()) {
      const found = findFile(fullPath, names);
      if (found) return found;
    }
  }
  return null;
}

function pluginVersion() {
  const manifest = readJson(path.join(pluginRoot, '.codex-plugin', 'plugin.json'));
  return manifest && typeof manifest.version === 'string' ? manifest.version : null;
}

function pluginCacheVersion() {
  const parent = path.basename(path.dirname(pluginRoot)).toLowerCase();
  return parent === 'codestory' ? path.basename(pluginRoot) : null;
}

function inferredCodexPluginDataDir(root = pluginRoot) {
  const parts = path.resolve(root).split(/[\\/]+/u);
  for (let index = 0; index <= parts.length - 6; index += 1) {
    if (
      parts[index].toLowerCase() !== '.codex' ||
      parts[index + 1] !== 'plugins' ||
      parts[index + 2] !== 'cache' ||
      parts[index + 4] !== 'codestory'
    ) {
      continue;
    }
    const codexRoot = parts.slice(0, index + 1).join(path.sep);
    const dataDir = path.join(codexRoot, 'plugins', 'data', `codestory-${parts[index + 3]}`);
    if (usablePluginDataDir(dataDir)) return dataDir;
  }
  return null;
}

function usablePluginDataDir(dataDir) {
  try {
    if (fs.existsSync(dataDir)) return fs.statSync(dataDir).isDirectory();
    const dataRoot = path.dirname(dataDir);
    if (fs.existsSync(dataRoot)) return fs.statSync(dataRoot).isDirectory();
    fs.accessSync(path.dirname(dataRoot), fs.constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

function pluginDataDir() {
  return process.env.PLUGIN_DATA
    || process.env.COPILOT_PLUGIN_DATA
    || process.env.CODESTORY_PLUGIN_DATA
    || inferredCodexPluginDataDir();
}

function sidecarPolicyPath(dataDir = pluginDataDir()) {
  return dataDir ? path.join(dataDir, 'sidecar-setup-policy.json') : null;
}

function activeStatePath(dataDir = pluginDataDir()) {
  return dataDir ? path.join(dataDir, activeStateFile) : null;
}

function activeThreadStatePath(threadId, dataDir = pluginDataDir()) {
  const normalized = String(threadId || '').trim();
  if (!dataDir || !normalized) return null;
  const key = createHash('sha256').update(normalized).digest('hex').slice(0, 16);
  return path.join(dataDir, `${activeThreadStatePrefix}${key}.json`);
}

function normalizeProjectRoot(projectRoot) {
  const resolved = path.resolve(projectRoot);
  try {
    return fs.realpathSync(resolved);
  } catch {
    return resolved;
  }
}

function existingProjectRoot(projectRoot) {
  if (!projectRoot || typeof projectRoot !== 'string' || !projectRoot.trim()) return null;
  const normalized = normalizeProjectRoot(projectRoot);
  try {
    return fs.statSync(normalized).isDirectory() ? normalized : null;
  } catch {
    return null;
  }
}

function activeProjectStateMaxAgeMs() {
  const parsed = Number.parseInt(process.env.CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS || '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 60 * 60 * 1000;
}

function activeProjectStateTimestamp(active, statePath) {
  const parsed = Date.parse(active?.updatedAt || active?.updated_at || '');
  if (Number.isFinite(parsed)) return parsed;
  try {
    return fs.statSync(statePath).mtimeMs;
  } catch {
    return null;
  }
}

function activeProjectStateMatchesHost(active) {
  const currentThread = String(process.env.CODEX_THREAD_ID || '').trim();
  const activeThread = String(active?.codexThreadId || '').trim();
  if (!activeThread) return true;
  if (!currentThread) return false;
  return activeThread === currentThread;
}

function activeProjectStateMatchesCurrentThread(active) {
  const currentThread = String(process.env.CODEX_THREAD_ID || '').trim();
  if (!currentThread) return true;
  return String(active?.codexThreadId || '').trim() === currentThread;
}

function activeProjectStateFresh(active, statePath, nowMs = Date.now(), options = {}) {
  const timestamp = activeProjectStateTimestamp(active, statePath);
  return timestamp !== null
    && nowMs - timestamp <= activeProjectStateMaxAgeMs()
    && (!options.requireThreadMatch || activeProjectStateMatchesHost(active));
}

function activeProjectStateSummary(statePath, nowMs = Date.now()) {
  const active = statePath ? readJson(statePath) : null;
  const timestamp = activeProjectStateTimestamp(active, statePath);
  return {
    path: statePath,
    cwd: active?.cwd || null,
    updated_at: active?.updatedAt || active?.updated_at || null,
    age_ms: timestamp === null ? null : Math.max(0, Math.round(nowMs - timestamp)),
    codex_thread_id: active?.codexThreadId || null,
  };
}

function projectResolutionDiagnostics(projectResolution, nowMs = Date.now()) {
  const currentThread = String(process.env.CODEX_THREAD_ID || '').trim();
  const threadStatePath = activeThreadStatePath(currentThread);
  return {
    project_root_resolution_source: projectResolution.source || null,
    project_root_resolution_state_path: projectResolution.statePath || null,
    project_root_available_after_launch: Boolean(projectResolution.projectRoot),
    active_state: activeProjectStateSummary(activeStatePath(), nowMs),
    thread_state: activeProjectStateSummary(threadStatePath, nowMs),
    codex_thread_id: currentThread || null,
    launch_cwd: launchCwd,
    runtime_cwd: process.cwd(),
  };
}

function resolveProjectRoot(options = {}) {
  const explicit = existingProjectRoot(options.projectRoot || process.env.CODESTORY_PROJECT_ROOT);
  if (explicit) {
    return { projectRoot: explicit, source: options.projectRoot ? 'argument' : 'env' };
  }

  const cwd = existingProjectRoot(options.cwd || launchCwd);
  if (cwd && !samePathText(cwd, pluginRoot)) {
    return { projectRoot: cwd, source: 'process_cwd' };
  }

  const currentThread = String(process.env.CODEX_THREAD_ID || '').trim();
  const threadStatePath = activeThreadStatePath(currentThread);
  const threadActive = threadStatePath ? readJson(threadStatePath) : null;
  if (threadActive && !activeProjectStateFresh(threadActive, threadStatePath, Date.now(), { requireThreadMatch: true })) {
    return {
      projectRoot: null,
      source: 'plugin_active_thread_state_stale',
      statePath: threadStatePath,
      reason: 'project_root_unavailable',
    };
  }
  const threadRoot = existingProjectRoot(threadActive?.cwd);
  if (threadRoot && !samePathText(threadRoot, pluginRoot)) {
    return { projectRoot: threadRoot, source: 'plugin_active_thread_state', statePath: threadStatePath };
  }

  const statePath = activeStatePath();
  const active = statePath ? readJson(statePath) : null;
  if (active && !activeProjectStateFresh(active, statePath)) {
    return {
      projectRoot: null,
      source: 'plugin_active_state_stale',
      statePath,
      reason: 'project_root_unavailable',
    };
  }
  if (active && !activeProjectStateMatchesCurrentThread(active)) {
    return {
      projectRoot: null,
      source: 'plugin_active_state_thread_mismatch',
      statePath,
      reason: 'project_root_unavailable',
    };
  }
  const activeRoot = existingProjectRoot(active?.cwd);
  if (activeRoot && !samePathText(activeRoot, pluginRoot)) {
    return { projectRoot: activeRoot, source: 'plugin_active_state', statePath };
  }

  return {
    projectRoot: null,
    source: statePath ? 'plugin_active_state_missing' : 'plugin_data_missing',
    statePath,
    reason: 'project_root_unavailable',
  };
}

function sidecarPolicyCommand(action, policyFile = sidecarPolicyPath()) {
  const policyArg = policyFile ? ` --policy-file ${JSON.stringify(policyFile)}` : '';
  return `node ${JSON.stringify(__filename)} sidecar-policy ${action}${policyArg}`;
}

function normalizeSidecarPolicyState(value) {
  const state = String(value || '').trim().toLowerCase();
  if (state === 'enabled' || state === 'disabled' || state === 'ask' || state === 'unknown') {
    return state === 'unknown' ? 'ask' : state;
  }
  return 'ask';
}

function sidecarPolicyStateForAction(action) {
  if (action === 'enable' || action === 'repair') return 'enabled';
  if (action === 'disable') return 'disabled';
  return 'ask';
}

function readSidecarPolicy(file = sidecarPolicyPath()) {
  const policy = file ? readJson(file) : null;
  const state = normalizeSidecarPolicyState(process.env.CODESTORY_PLUGIN_SIDECAR_POLICY || policy?.state);
  return {
    state,
    path: file,
    updatedAt: policy?.updated_at || null,
    lastRepair: policy?.last_repair || null,
  };
}

function firstSemverToken(value) {
  const match = String(value || '').match(/\b\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?\b/u);
  return match ? match[0] : null;
}

function sidecarLastRepairStaleReason(lastRepair, activeVersion = pluginVersion(), activePath = null) {
  const command = lastRepair?.command;
  if (!command) return null;
  const version = firstSemverToken(command);
  if (version && activeVersion && version !== activeVersion) {
    return `last_repair_cli_version_mismatch:${version}!=${activeVersion}`;
  }
  if (activePath && command.includes('codestory-cli') && !command.includes(activePath)) {
    return 'last_repair_cli_path_mismatch';
  }
  return null;
}

function normalizedSidecarLastRepair(lastRepair, activeVersion = pluginVersion(), activePath = null) {
  if (!lastRepair) return null;
  const staleReason = sidecarLastRepairStaleReason(lastRepair, activeVersion, activePath);
  return {
    ...lastRepair,
    current: Boolean(lastRepair.command) && !staleReason,
    stale_reason: staleReason,
  };
}

function writeSidecarPolicy(state, patch = {}, file = sidecarPolicyPath()) {
  if (!file) return null;
  const current = readJson(file) || {};
  const next = {
    ...current,
    ...patch,
    state: normalizeSidecarPolicyState(state),
    updated_at: new Date().toISOString(),
  };
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(next, null, 2));
  return next;
}

function handleSidecarPolicyCommand(argv) {
  if (argv[2] !== 'sidecar-policy') return;
  const action = argv[3] || 'status';
  const policyFileFlag = argv.indexOf('--policy-file');
  const policyFile = policyFileFlag >= 0 ? argv[policyFileFlag + 1] : sidecarPolicyPath();
  if (!['enable', 'disable', 'ask', 'repair', 'status'].includes(action)) {
    process.stderr.write('usage: codestory-mcp.cjs sidecar-policy [enable|disable|ask|repair|status] [--policy-file PATH]\n');
    process.exit(2);
  }
  if (policyFileFlag >= 0 && !policyFile) {
    process.stderr.write('sidecar-policy --policy-file requires a path\n');
    process.exit(2);
  }
  if (action !== 'status') {
    if (!policyFile) {
      process.stderr.write('sidecar-policy needs PLUGIN_DATA, COPILOT_PLUGIN_DATA, CODESTORY_PLUGIN_DATA, or --policy-file to remember the setting\n');
      process.exit(2);
    }
    writeSidecarPolicy(sidecarPolicyStateForAction(action), {}, policyFile);
  }
  process.stdout.write(`${JSON.stringify(readSidecarPolicy(policyFile), null, 2)}\n`);
  process.exit(0);
}

function repairCommandForProject(projectRoot = launchCwd) {
  return `codestory-cli ready --goal agent --repair --project ${JSON.stringify(projectRoot)} --format json --run-id ${sharedAgentRunId}`;
}

function resolvedRepairCommandForProject(resolved, projectRoot = launchCwd) {
  return `${JSON.stringify(resolved.path)} ready --goal agent --repair --project ${JSON.stringify(projectRoot)} --format json --run-id ${sharedAgentRunId}`;
}

function optionValue(argv, name) {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : null;
}

function dirtyMarkerEnv(projectRoot = launchCwd) {
  const markerPath = dirtyMarkerPathForProject(projectRoot, pluginDataDir());
  const normalizedRoot = (() => {
    try {
      return fs.realpathSync(path.resolve(projectRoot));
    } catch {
      return path.resolve(projectRoot);
    }
  })();
  return {
    path: markerPath || '',
    projectRoot: markerPath ? normalizedRoot : '',
  };
}

function runtimeTruthStatus(plugin, options = {}) {
  const readinessGoals = new Set(options.readinessGoals || []);
  const readinessLanes = options.readinessLanes && typeof options.readinessLanes === 'object'
    ? options.readinessLanes
    : null;
  const readinessRefs = {};
  if (readinessGoals.has('local_navigation')) {
    readinessRefs.local_graph = 'readiness[goal=local_navigation]';
  }
  if (readinessGoals.has('agent_packet_search')) {
    readinessRefs.agent_packet_search = 'readiness[goal=agent_packet_search]';
  }
  if (readinessGoals.has('project_root')) {
    readinessRefs.project_root = 'readiness[goal=project_root]';
  }
  if (options.localRefresh) readinessRefs.local_refresh = 'local_refresh';
  if (readinessLanes?.local_default) readinessRefs.local_default = 'readiness_lanes.local_default';
  if (readinessLanes?.agent_packet_search) {
    readinessRefs.agent_packet_search = 'readiness_lanes.agent_packet_search';
  }
  const sidecarStatusRef = readinessLanes?.agent_packet_search
    ? 'readiness_lanes.agent_packet_search'
    : readinessGoals.has('agent_packet_search')
      ? 'readiness[goal=agent_packet_search]'
      : null;
  return {
    runtime_source: plugin.cli_source || 'unavailable',
    plugin_root: plugin.plugin_root || null,
    managed_cli_path: plugin.managed_binary_path || null,
    launcher_source: plugin.cli_source || 'unavailable',
    sidecar_policy: options.sidecarPolicy || 'unavailable',
    sidecar_status_ref: sidecarStatusRef,
    readiness_refs: readinessRefs,
    readiness_broker_ref: options.hasReadinessBroker ? 'readiness_broker' : null,
  };
}

function resolveManifest(manifestPath) {
  const manifest = readJson(manifestPath);
  if (!manifest) return null;
  const executable = manifest.executable_path || manifest.executablePath || manifest.path;
  if (!executable) return null;
  const cliPath = path.resolve(path.dirname(manifestPath), executable);
  if (!fs.existsSync(cliPath)) return null;
  const sha256 = fileSha256(cliPath);
  const expected = manifest.sha256 || manifest.executable_sha256 || manifest.executableSha256;
  if (expected && expected.toLowerCase() !== sha256) {
    return { warning: `managed_cli_checksum_mismatch:${manifestPath}` };
  }
  return {
    path: cliPath,
    sha256,
    manifestPath,
    cliVersion: manifest.version || manifest.cli_version || null,
    repoRef: manifest.repo_ref || null,
    buildSource: manifest.build_source || manifest.source || null,
    archiveSha256: manifest.archive_sha256 || null,
    archiveUrl: manifest.archive_url || null,
    provisionedAt: manifest.provisioned_at || null,
  };
}

function assetTarget() {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === 'win32' && arch === 'x64') return 'windows-x64';
  if (platform === 'win32' && arch === 'arm64') return 'windows-arm64';
  if (platform === 'linux' && arch === 'x64') return 'linux-x64';
  if (platform === 'linux' && arch === 'arm64') return 'linux-arm64';
  if (platform === 'darwin' && arch === 'x64') return 'macos-x64';
  if (platform === 'darwin' && arch === 'arm64') return 'macos-arm64';
  return null;
}

function archiveName(version, target = assetTarget()) {
  if (!target) return null;
  const extension = target.startsWith('windows-') ? 'zip' : 'tar.gz';
  return `codestory-cli-v${version}-${target}.${extension}`;
}

function expectedArchiveHash(sumsText, name) {
  for (const line of sumsText.split(/\r?\n/u)) {
    const match = line.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/u);
    if (match && match[2].trim() === name) return match[1].toLowerCase();
  }
  throw new Error(`SHA256SUMS.txt did not contain ${name}`);
}

function copyLocalReleaseFile(releaseDir, name, destination) {
  fs.copyFileSync(path.join(releaseDir, name), destination);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function downloadFileOnce(url, destination, options = {}) {
  const timeoutMs = options.timeoutMs || releaseDownloadTimeoutMs;
  const redirectsRemaining = options.redirectsRemaining ?? 5;
  const parsedUrl = new URL(url);
  const loopbackHttp = parsedUrl.protocol === 'http:' &&
    ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(parsedUrl.hostname);
  if (!options.get && parsedUrl.protocol !== 'https:' && !loopbackHttp) {
    return Promise.reject(new Error('download transport must be HTTPS'));
  }
  const get = options.get || (loopbackHttp ? http.get : https.get);
  return new Promise((resolve, reject) => {
    const request = get(url, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        if (!response.headers.location || redirectsRemaining <= 0) {
          reject(new Error(`download redirect failed: ${url}`));
          return;
        }
        const nextUrl = new URL(response.headers.location, url).toString();
        downloadFileOnce(nextUrl, destination, { ...options, redirectsRemaining: redirectsRemaining - 1 })
          .then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed ${response.statusCode}: ${url}`));
        return;
      }
      const output = fs.createWriteStream(destination);
      response.pipe(output);
      output.on('finish', () => output.close(resolve));
      output.on('error', reject);
    });
    request.setTimeout(timeoutMs, () => request.destroy(new Error(`download timed out after ${timeoutMs}ms: ${url}`)));
    request.on('error', reject);
  });
}

async function downloadFile(url, destination, options = {}) {
  const attempts = options.attempts || releaseDownloadAttempts;
  const startedAt = Date.now();
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await downloadFileOnce(url, destination, options);
      return;
    } catch (error) {
      lastError = error;
      fs.rmSync(destination, { force: true });
      if (attempt < attempts) {
        const delayMs = options.retryDelayMs
          ? options.retryDelayMs(attempt)
          : releaseDownloadRetryDelaysMs[attempt - 1] ||
            releaseDownloadRetryDelaysMs[releaseDownloadRetryDelaysMs.length - 1];
        if (delayMs > 0) await sleep(delayMs);
      }
    }
  }
  const elapsedMs = Date.now() - startedAt;
  throw new Error(`download failed after ${attempts} attempts over ${elapsedMs}ms: ${lastError?.message || 'unknown error'}`);
}

function releaseAssetFetchFailure(name, startedAt, attempts, error) {
  const elapsedMs = Date.now() - startedAt;
  return `managed_cli_asset_fetch_failed:${name}:elapsed_ms=${elapsedMs}:attempts=${attempts}:retry=restart_reload_status:last_error=${error.message}`;
}

function releaseFileUrl(version, name) {
  if (process.env.CODESTORY_PLUGIN_RELEASE_DIR) {
    return `file://${path.join(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name)}`;
  }
  const baseUrl = process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL ||
    `https://github.com/TheGreenCedar/CodeStory/releases/download/v${version}`;
  return `${baseUrl.replace(/\/$/u, '')}/${name}`;
}

function redactedReleaseFileUrl(version, name) {
  const url = new URL(releaseFileUrl(version, name));
  url.username = '';
  url.password = '';
  url.search = '';
  url.hash = '';
  return url.toString();
}

async function fetchReleaseFile(version, name, destination) {
  const startedAt = Date.now();
  if (process.env.CODESTORY_PLUGIN_RELEASE_DIR) {
    try {
      copyLocalReleaseFile(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name, destination);
    } catch (error) {
      throw new Error(releaseAssetFetchFailure(name, startedAt, 1, error));
    }
    return redactedReleaseFileUrl(version, name);
  }
  const url = releaseFileUrl(version, name);
  try {
    await downloadFile(url, destination);
  } catch (error) {
    throw new Error(releaseAssetFetchFailure(name, startedAt, releaseDownloadAttempts, error));
  }
  return redactedReleaseFileUrl(version, name);
}

function extractArchive(archivePath, destination) {
  fs.mkdirSync(destination, { recursive: true });
  const result = spawnSync('tar', ['-xf', archivePath, '-C', destination], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) {
    throw new Error(`tar extract failed: ${result.stderr || result.error?.message || result.status}`);
  }
}

function processIsAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  if (pid === process.pid) return true;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

function processStartIdentity(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  try {
    if (process.platform === 'linux') {
      const stat = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
      const fields = stat.slice(stat.lastIndexOf(') ') + 2).trim().split(/\s+/u);
      const bootId = fs.readFileSync('/proc/sys/kernel/random/boot_id', 'utf8').trim();
      return `linux:${bootId}:${fields[19]}`;
    }
    if (process.platform === 'darwin') {
      const result = spawnSync('/bin/ps', ['-o', 'lstart=', '-p', String(pid)], {
        encoding: 'utf8',
        windowsHide: true,
      });
      const started = result.status === 0 ? result.stdout.trim().replace(/\s+/gu, ' ') : '';
      return started ? `darwin:${started}` : null;
    }
    if (process.platform === 'win32') {
      const powershell = path.join(
        process.env.SystemRoot || process.env.WINDIR || 'C:\\Windows',
        'System32',
        'WindowsPowerShell',
        'v1.0',
        'powershell.exe',
      );
      const result = spawnSync(powershell, [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        `(Get-Process -Id ${pid} -ErrorAction Stop).StartTime.ToUniversalTime().Ticks`,
      ], { encoding: 'utf8', windowsHide: true });
      const started = result.status === 0 ? result.stdout.trim() : '';
      return /^\d+$/u.test(started) ? `win32:${started}` : null;
    }
  } catch {
    return null;
  }
  return null;
}

function sleepSync(milliseconds) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

function managedCliRoot(dataDir, create = false) {
  const root = path.join(dataDir, 'codestory-cli');
  if (fs.existsSync(root)) {
    const metadata = fs.lstatSync(root);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error(`managed_cli_root_not_direct:${root}`);
    }
  } else if (create) {
    fs.mkdirSync(root, { recursive: true });
  }
  return root;
}

function managedCliLockOwnerIsStale(
  owner,
  checkProcessIdentity = true,
  processStartIdentityFor = processStartIdentity,
) {
  if (!owner || !Number.isInteger(owner.pid)) return null;
  const alive = processIsAlive(owner.pid);
  const observedIdentity = alive && checkProcessIdentity ? processStartIdentityFor(owner.pid) : null;
  return !alive || Boolean(
    owner.process_start_identity &&
    observedIdentity &&
    owner.process_start_identity !== observedIdentity
  );
}

function removeManagedCliLockArtifact(artifactPath) {
  const stalePath = `${artifactPath}.stale-${process.pid}-${randomBytes(6).toString('hex')}`;
  try {
    fs.renameSync(artifactPath, stalePath);
    fs.rmSync(stalePath, { recursive: true, force: true });
    return true;
  } catch {
    return false;
  }
}

function reclaimStaleManagedCliPendingOwners(
  root,
  checkProcessIdentity = true,
  processStartIdentityFor = processStartIdentity,
) {
  let entries;
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return 0;
  }
  let removed = 0;
  let inspected = 0;
  for (const entry of entries) {
    if (!entry.isFile() || entry.isSymbolicLink()) {
      continue;
    }
    const match = entry.name.match(/^\.retention-lock\.owner-(\d+)-([0-9a-f]{32})$/u);
    if (!match) continue;
    if (inspected >= managedCliPendingOwnerCleanupLimit) break;
    inspected += 1;
    const artifactPath = path.join(root, entry.name);
    let descriptor;
    try {
      const before = fs.lstatSync(artifactPath);
      if (!before.isFile() || before.isSymbolicLink()) continue;
      descriptor = fs.openSync(
        artifactPath,
        fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0),
      );
      const metadata = fs.fstatSync(descriptor);
      if (
        !metadata.isFile() || metadata.dev !== before.dev || metadata.ino !== before.ino ||
        metadata.size !== before.size || metadata.mtimeMs !== before.mtimeMs
      ) {
        continue;
      }
      let owner = null;
      try {
        owner = JSON.parse(fs.readFileSync(descriptor, 'utf8'));
      } catch {
        // A young partial/malformed artifact remains protected by the age fallback.
      }
      const pid = Number(match[1]);
      const completeOwner = owner &&
        owner.pid === pid &&
        owner.token === match[2] &&
        typeof owner.purpose === 'string' && owner.purpose.length > 0 &&
        typeof owner.process_start_identity === 'string' && owner.process_start_identity.length > 0 &&
        typeof owner.started_at === 'string' && Number.isFinite(Date.parse(owner.started_at));
      const ageMs = Date.now() - metadata.mtimeMs;
      // Fresh live claims cannot be stale yet; defer expensive identity probes until the
      // existing ten-minute stale threshold makes PID reuse relevant.
      const stale = completeOwner
        ? managedCliLockOwnerIsStale(
          owner,
          checkProcessIdentity && ageMs > managedCliLockStaleMs,
          processStartIdentityFor,
        )
        : ageMs > managedCliLockStaleMs;
      if (!stale) continue;
      const current = fs.lstatSync(artifactPath);
      if (
        current.isSymbolicLink() || !current.isFile() ||
        current.dev !== metadata.dev || current.ino !== metadata.ino ||
        current.size !== metadata.size || current.mtimeMs !== metadata.mtimeMs
      ) {
        continue;
      }
      fs.unlinkSync(artifactPath);
      removed += 1;
    } catch {
      // Another contender may publish or remove the artifact concurrently.
    } finally {
      if (descriptor !== undefined) fs.closeSync(descriptor);
    }
  }
  return removed;
}

function reclaimStaleManagedCliInitialization(lockPath, checkProcessIdentity = true) {
  const initializationPath = `${lockPath}.initializing`;
  const owner = readJson(initializationPath);
  let stale = managedCliLockOwnerIsStale(owner, checkProcessIdentity);
  if (stale === null) {
    try {
      stale = Date.now() - fs.statSync(initializationPath).mtimeMs > managedCliLockStaleMs;
    } catch {
      return false;
    }
  }
  return stale ? removeManagedCliLockArtifact(initializationPath) : false;
}

function reclaimStaleManagedCliLock(lockPath, checkProcessIdentity = true) {
  const ownerPath = path.join(lockPath, 'owner.json');
  const owner = readJson(ownerPath);
  const initializationOwner = owner ? null : readJson(`${lockPath}.initializing`);
  let stale = managedCliLockOwnerIsStale(owner || initializationOwner, checkProcessIdentity);
  if (stale === null) {
    try {
      stale = Date.now() - fs.statSync(lockPath).mtimeMs > managedCliLockStaleMs;
    } catch {
      return false;
    }
  }
  if (!stale) return false;
  const removed = removeManagedCliLockArtifact(lockPath);
  if (removed) reclaimStaleManagedCliInitialization(lockPath, checkProcessIdentity);
  return removed;
}

function releaseManagedCliInitialization(lockPath, owner) {
  const initializationPath = `${lockPath}.initializing`;
  const current = readJson(initializationPath);
  if (!current || current.pid !== owner.pid || current.token !== owner.token) return;
  fs.rmSync(initializationPath, { force: true });
}

function acquireManagedCliLock(root, purpose, waitMs = 0, options = {}) {
  const lockPath = path.join(root, '.retention-lock');
  const ownerPath = path.join(lockPath, 'owner.json');
  const initializationPath = `${lockPath}.initializing`;
  const token = randomBytes(16).toString('hex');
  const processStartIdentityFor = options.processStartIdentity || processStartIdentity;
  const selfIdentity = processStartIdentityFor(process.pid);
  if (!selfIdentity) throw new Error('managed_cli_process_identity_unavailable');
  const owner = {
    pid: process.pid,
    purpose,
    token,
    process_start_identity: selfIdentity,
    started_at: new Date().toISOString(),
  };
  const pendingOwnerPath = `${lockPath}.owner-${process.pid}-${token}`;
  const deadline = Date.now() + waitMs;
  let waited = false;
  let reclaimed = false;
  let nextIdentityCheckAt = 0;
  reclaimStaleManagedCliPendingOwners(root);
  fs.writeFileSync(pendingOwnerPath, JSON.stringify(owner), { flag: 'wx', mode: 0o600 });
  try {
    while (true) {
      let createdLock = false;
      let ownsInitialization = false;
      let publishedOwner = false;
      try {
        fs.linkSync(pendingOwnerPath, initializationPath);
        ownsInitialization = true;
        try {
          fs.mkdirSync(lockPath);
          createdLock = true;
          fs.linkSync(initializationPath, ownerPath);
          publishedOwner = true;
        } catch (error) {
          if (createdLock) fs.rmSync(lockPath, { recursive: true, force: true });
          throw error;
        }
        try {
          releaseManagedCliInitialization(lockPath, owner);
        } catch {
          // The owner-bearing directory is authoritative; release retries this alias.
        }
        reclaimStaleManagedCliPendingOwners(root);
        return { lockPath, token, waited, reclaimed };
      } catch (error) {
        if (ownsInitialization && !publishedOwner) {
          releaseManagedCliInitialization(lockPath, owner);
        }
        if (error.code !== 'EEXIST') throw error;
        waited = true;
        const checkProcessIdentity = Date.now() >= nextIdentityCheckAt;
        if (checkProcessIdentity) nextIdentityCheckAt = Date.now() + 2000;
        if (checkProcessIdentity) reclaimStaleManagedCliPendingOwners(root);
        if (
          reclaimStaleManagedCliLock(lockPath, checkProcessIdentity) ||
          reclaimStaleManagedCliInitialization(lockPath, checkProcessIdentity)
        ) {
          reclaimed = true;
          continue;
        }
        if (Date.now() >= deadline) return null;
        sleepSync(50);
      }
    }
  } finally {
    fs.rmSync(pendingOwnerPath, { force: true });
  }
}

function managedCliFailureCode(error) {
  return String(error?.message || error || 'unknown_failure').match(/^[a-z0-9_]+/iu)?.[0] || 'unknown_failure';
}

function verifyPublishedManagedCli(
  versionDir,
  version,
  expectedTarget = assetTarget(),
  probeVersion = probeResolvedCli,
) {
  let bytes = 0;
  try {
    bytes = managedPathSize(versionDir);
  } catch (error) {
    return { verified: false, reason: `version_unreadable:${error.code || managedCliFailureCode(error)}` };
  }
  const verified = verifyManagedCliVersion({
    version,
    versionDir,
    bytes,
    scanError: null,
    provisioning: false,
  }, probeVersion);
  if (!verified.verified) return verified;
  const manifest = readJson(path.join(versionDir, 'manifest.json'));
  const expectedAsset = archiveName(version, expectedTarget);
  if (
    manifest.build_source !== 'github_release' ||
    manifest.repo_ref !== `v${version}` ||
    manifest.archive !== expectedAsset ||
    manifest.target !== expectedTarget ||
    !/^[0-9a-f]{64}$/iu.test(String(manifest.archive_sha256 || '')) ||
    manifest.archive_url !== redactedReleaseFileUrl(version, expectedAsset)
  ) {
    return { verified: false, reason: 'manifest_release_metadata_invalid' };
  }
  const resolved = resolveManifest(path.join(versionDir, 'manifest.json'));
  if (!resolved?.path) return { verified: false, reason: 'manifest_resolution_failed' };
  return { verified: true, reason: null, resolved: { ...resolved, cliVersion: version } };
}

function trimManagedCliQuarantines(root, version, options = {}) {
  const rmSync = options.rmSync || fs.rmSync;
  const candidates = fs.readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.name.startsWith(`.quarantine-${version}-`));
  if (candidates.some((entry) => !entry.isDirectory() || entry.isSymbolicLink())) {
    throw new Error('managed_cli_quarantine_not_direct');
  }
  const quarantines = candidates
    .map((entry) => path.join(root, entry.name))
    .sort()
    .reverse();
  for (const stale of quarantines.slice(managedCliQuarantineRetention)) {
    try {
      rmSync(stale, { recursive: true, force: false });
    } catch (error) {
      throw new Error(`managed_cli_quarantine_retention_failed:${error.code || managedCliFailureCode(error)}`);
    }
  }
}

function quarantineManagedCliVersion(root, versionDir, version, reason, options = {}) {
  const renameSync = options.renameSync || fs.renameSync;
  const metadata = fs.lstatSync(versionDir);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error('managed_cli_quarantine_not_direct');
  }
  const quarantine = path.join(
    root,
    `.quarantine-${version}-${Date.now()}-${randomBytes(6).toString('hex')}`,
  );
  try {
    renameSync(versionDir, quarantine);
  } catch (error) {
    throw new Error(`managed_cli_quarantine_failed:${error.code || managedCliFailureCode(error)}`);
  }
  trimManagedCliQuarantines(root, version, options);
  return { reason, quarantine };
}

function releaseManagedCliLock(lock) {
  if (!lock) return;
  const owner = readJson(path.join(lock.lockPath, 'owner.json'));
  if (!owner || owner.token !== lock.token || owner.pid !== process.pid) return;
  releaseManagedCliInitialization(lock.lockPath, owner);
  try {
    fs.rmSync(lock.lockPath, { recursive: true, force: true });
  } catch (error) {
    try {
      fs.writeFileSync(
        path.join(lock.lockPath, 'owner.json'),
        JSON.stringify({ ...owner, pid: -1, released_at: new Date().toISOString() }),
      );
    } catch {
      // The next process can still reclaim a malformed lock after the stale timeout.
    }
    throw error;
  }
}

async function provisionManagedCli(dataDir, version, warnings = []) {
  if (!dataDir || !version || process.env.CODESTORY_PLUGIN_DISABLE_PROVISION === '1') return null;
  const target = assetTarget();
  const asset = archiveName(version, target);
  if (!target || !asset) throw new Error(`unsupported_release_target:${process.platform}-${process.arch}`);

  const root = managedCliRoot(dataDir, true);
  const versionDir = path.join(root, version);
  const lock = acquireManagedCliLock(root, `provision:${version}`, managedCliLockWaitMs);
  if (!lock) throw new Error('managed_cli_publish_locked');
  if (lock.waited) warnings.push('managed_cli_publication:waiter');
  if (lock.reclaimed) warnings.push('managed_cli_publication:reclaimed_lock');
  let tempRoot = null;
  let stagingDir = null;
  try {
    trimManagedCliQuarantines(root, version);
    if (fs.existsSync(versionDir)) {
      const existing = verifyPublishedManagedCli(versionDir, version, target);
      if (existing.verified) return existing.resolved;
      quarantineManagedCliVersion(root, versionDir, version, existing.reason);
      warnings.push(`managed_cli_publication:quarantine:${existing.reason}`);
      warnings.push(`managed_cli_publication:reprovision:${existing.reason}`);
    }
    warnings.push('managed_cli_publication:publisher');
    tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'codestory-plugin-cli-'));
    const sumsPath = path.join(tempRoot, 'SHA256SUMS.txt');
    const archivePath = path.join(tempRoot, asset);
    const extractDir = path.join(tempRoot, 'extract');
    await fetchReleaseFile(version, 'SHA256SUMS.txt', sumsPath);
    const archiveUrl = await fetchReleaseFile(version, asset, archivePath);
    const expected = expectedArchiveHash(fs.readFileSync(sumsPath, 'utf8'), asset);
    const actual = fileSha256(archivePath);
    if (actual !== expected) {
      throw new Error(`archive_checksum_mismatch:${asset}`);
    }
    extractArchive(archivePath, extractDir);
    const extracted = findFile(extractDir, fallbackBinaryNames);
    if (!extracted) throw new Error(`archive_missing_cli:${asset}`);

    stagingDir = fs.mkdtempSync(path.join(root, `.provisioning-${version}-${process.pid}-`));
    const binDir = path.join(stagingDir, 'bin');
    const manifestPath = path.join(stagingDir, 'manifest.json');
    fs.mkdirSync(binDir, { recursive: true });
    const destination = path.join(binDir, path.basename(extracted));
    fs.copyFileSync(extracted, destination);
    if (process.platform !== 'win32') fs.chmodSync(destination, 0o755);
    const binarySha256 = fileSha256(destination);
    fs.writeFileSync(manifestPath, JSON.stringify({
      path: path.relative(stagingDir, destination).replace(/\\/gu, '/'),
      sha256: binarySha256,
      version,
      build_source: 'github_release',
      repo_ref: `v${version}`,
      archive: asset,
      archive_url: archiveUrl,
      archive_sha256: actual,
      target,
      provisioned_at: new Date().toISOString(),
    }, null, 2));
    const staged = verifyPublishedManagedCli(
      stagingDir,
      version,
      target,
      (resolved) => probeResolvedCli({ ...resolved, provisioningProbe: true }),
    );
    if (!staged.verified) {
      throw new Error(`managed_cli_staging_verification_failed:${staged.reason}`);
    }
    if (fs.existsSync(versionDir)) throw new Error('managed_cli_publish_target_reappeared');
    fs.renameSync(stagingDir, versionDir);
    stagingDir = null;
    return resolveManifest(path.join(versionDir, 'manifest.json'));
  } finally {
    if (stagingDir) fs.rmSync(stagingDir, { recursive: true, force: true });
    if (tempRoot) fs.rmSync(tempRoot, { recursive: true, force: true });
    releaseManagedCliLock(lock);
  }
}

async function resolveManagedCli(dataDir, version, warnings) {
  if (!dataDir || !version) return null;
  try {
    managedCliRoot(dataDir);
  } catch (error) {
    warnings.push(`managed_cli_root_invalid:${error.message}`);
    return null;
  }
  const versionDir = path.join(dataDir, 'codestory-cli', version);
  if (fs.existsSync(versionDir)) {
    const existing = verifyPublishedManagedCli(versionDir, version);
    if (existing.verified) return existing.resolved;
  }
  try {
    return await provisionManagedCli(dataDir, version, warnings);
  } catch (error) {
    const code = managedCliFailureCode(error);
    warnings.push(`managed_cli_publication:terminal_failure:${code}`);
    warnings.push(`managed_cli_provision_failed:${code}`);
  }
  return null;
}

async function resolveCli() {
  const version = pluginVersion();
  const warnings = [];
  if (process.env.CODESTORY_CLI) {
    const cliPath = path.isAbsolute(process.env.CODESTORY_CLI)
      ? process.env.CODESTORY_CLI
      : path.resolve(launchCwd, process.env.CODESTORY_CLI);
    return {
      source: 'local_dev_override',
      path: cliPath,
      sha256: fs.existsSync(cliPath) ? fileSha256(cliPath) : null,
      version,
      cliVersion: null,
      repoRef: null,
      buildSource: 'local_dev_override',
      archiveSha256: null,
      archiveUrl: null,
      provisionedAt: null,
      warnings,
    };
  }

  const managed = await resolveManagedCli(pluginDataDir(), version, warnings);
  if (managed && managed.warning) warnings.push(managed.warning);
  if (managed && managed.path) {
    return { source: 'managed', version, warnings, ...managed };
  }

  const managedProvisionFailure = warnings.find((warning) => warning.startsWith('managed_cli_provision_failed:')) || null;
  warnings.push('managed_cli_unavailable');
  return {
    source: 'managed_unavailable',
    path: null,
    sha256: null,
    version,
    cliVersion: null,
    repoRef: null,
    buildSource: 'managed_unavailable',
    archiveSha256: null,
    archiveUrl: null,
    provisionedAt: null,
    managedProvisionFailure,
    warnings,
  };
}

function normalizeVersion(value) {
  const match = String(value || '').match(/\b[vV]?(\d+\.\d+\.\d+)\b/u);
  return match ? match[1] : null;
}

function probeResolvedCli(resolved) {
  if (!resolved.path) {
    return {
      status: null,
      error: `${resolved.source || 'unavailable'}_cli_unavailable`,
      version: null,
      stdout: '',
      stderr: '',
    };
  }
  const result = spawnSync(resolved.path, ['--version'], {
    encoding: 'utf8',
    env: resolved.provisioningProbe
      ? { ...process.env, CODESTORY_PLUGIN_PROVISIONING_PROBE: '1' }
      : process.env,
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    timeout: 3000,
    windowsHide: true,
  });
  const output = `${result.stdout || ''}\n${result.stderr || ''}`;
  return {
    status: result.status,
    error: result.error ? result.error.message : null,
    version: normalizeVersion(output),
    stdout: result.stdout || '',
    stderr: result.stderr || '',
  };
}

function failOpenReasonForProbe(resolved, probe) {
  if (resolved.source === 'managed_unavailable') {
    return resolved.managedProvisionFailure || 'managed_cli_unavailable';
  }
  if (probe.error || probe.status !== 0) {
    return `${resolved.source}_cli_unspawnable`;
  }
  return null;
}

function compareManagedCliVersions(left, right) {
  const leftParts = left.split('.').map(Number);
  const rightParts = right.split('.').map(Number);
  for (let index = 0; index < 3; index += 1) {
    const difference = leftParts[index] - rightParts[index];
    if (difference !== 0) return difference;
  }
  return 0;
}

function managedPathSize(pathname) {
  const metadata = fs.lstatSync(pathname);
  if (metadata.isSymbolicLink()) {
    throw new Error(`managed_cli_retention_link:${pathname}`);
  }
  if (!metadata.isDirectory()) return metadata.size;
  let bytes = 0;
  for (const entry of fs.readdirSync(pathname, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) {
      throw new Error(`managed_cli_retention_link:${path.join(pathname, entry.name)}`);
    }
    bytes += managedPathSize(path.join(pathname, entry.name));
  }
  return bytes;
}

function managedProvisioningState(versionDir) {
  const sentinel = path.join(versionDir, '.provisioning');
  if (!fs.existsSync(sentinel)) return { active: false, recovered: false };
  let pid = null;
  let staleByAge = false;
  try {
    pid = Number.parseInt(fs.readFileSync(sentinel, 'utf8').trim(), 10);
    staleByAge = Date.now() - fs.statSync(sentinel).mtimeMs > managedCliLockStaleMs;
  } catch {
    return { active: true, recovered: false };
  }
  const stale = Number.isInteger(pid) && pid > 0
    ? !processIsAlive(pid) || staleByAge
    : staleByAge;
  if (!stale) return { active: true, recovered: false };
  try {
    fs.unlinkSync(sentinel);
    return { active: false, recovered: true };
  } catch {
    return { active: true, recovered: false };
  }
}

function managedCliVersionEntries(dataDir) {
  const root = path.join(dataDir, 'codestory-cli');
  if (!fs.existsSync(root)) return { root, entries: [], staging: [], errors: [] };
  const entries = [];
  const staging = [];
  const errors = [];
  let children;
  try {
    const rootMetadata = fs.lstatSync(root);
    if (rootMetadata.isSymbolicLink() || !rootMetadata.isDirectory()) {
      return { root, entries, staging, errors: [`managed_cli_root_not_direct:${root}`] };
    }
    children = fs.readdirSync(root, { withFileTypes: true });
  } catch (error) {
    return { root, entries, staging, errors: [`scan:${error.code || error.message}`] };
  }
  for (const child of children) {
    if (child.name.startsWith('.provisioning-') || child.name.startsWith('.replaced-')) {
      const stagingPath = path.join(root, child.name);
      try {
        if (!child.isDirectory() || child.isSymbolicLink()) {
          errors.push(`managed_cli_staging_not_direct:${stagingPath}`);
          continue;
        }
        const match = /^\.(?:provisioning|replaced)-(\d+\.\d+\.\d+)-(\d+)-/u.exec(child.name);
        const pid = match ? Number.parseInt(match[2], 10) : null;
        const ageMs = Date.now() - fs.statSync(stagingPath).mtimeMs;
        const stale = pid ? !processIsAlive(pid) || ageMs > managedCliLockMaxAgeMs : ageMs > managedCliLockStaleMs;
        staging.push({
          version: match?.[1] || child.name,
          versionDir: stagingPath,
          bytes: managedPathSize(stagingPath),
          stale,
          reason: child.name.startsWith('.replaced-') ? 'publish_backup' : 'provisioning',
        });
      } catch (error) {
        errors.push(`scan_staging:${error.code || error.message}`);
      }
      continue;
    }
    const version = normalizeVersion(child.name);
    if (!version || version !== child.name) continue;
    const versionDir = path.join(root, child.name);
    if (!child.isDirectory() || child.isSymbolicLink()) {
      entries.push({
        version,
        versionDir,
        bytes: 0,
        scanError: 'link_or_non_directory',
        provisioning: false,
      });
      continue;
    }
    try {
      const provisioning = managedProvisioningState(versionDir);
      entries.push({
        version,
        versionDir,
        bytes: managedPathSize(versionDir),
        scanError: null,
        provisioning: provisioning.active,
      });
    } catch (error) {
      entries.push({
        version,
        versionDir,
        bytes: 0,
        scanError: error.message,
        provisioning: false,
      });
    }
  }
  entries.sort((left, right) => compareManagedCliVersions(right.version, left.version));
  return { root, entries, staging, errors };
}

function verifyManagedCliVersion(entry, probeVersion = probeResolvedCli) {
  if (entry.scanError || entry.provisioning) {
    return { verified: false, reason: entry.scanError || 'provisioning' };
  }
  const manifestPath = path.join(entry.versionDir, 'manifest.json');
  const manifest = readJson(manifestPath);
  if (!manifest || manifest.version !== entry.version) {
    return { verified: false, reason: 'manifest_version_mismatch' };
  }
  const executable = manifest.executable_path || manifest.executablePath || manifest.path;
  const expectedSha256 = manifest.sha256 || manifest.executable_sha256 || manifest.executableSha256;
  if (!executable || !/^[0-9a-f]{64}$/iu.test(String(expectedSha256 || ''))) {
    return { verified: false, reason: 'manifest_incomplete' };
  }
  const executablePath = path.resolve(entry.versionDir, executable);
  if (!pathInside(executablePath, entry.versionDir) || !fs.existsSync(executablePath)) {
    return { verified: false, reason: 'manifest_path_unsafe' };
  }
  let realVersionDir;
  let realExecutable;
  try {
    realVersionDir = fs.realpathSync(entry.versionDir);
    realExecutable = fs.realpathSync(executablePath);
  } catch (error) {
    return { verified: false, reason: `manifest_path_unreadable:${error.code || error.message}` };
  }
  if (!pathInside(realExecutable, realVersionDir)) {
    return { verified: false, reason: 'manifest_path_escape' };
  }
  let actualSha256;
  try {
    actualSha256 = fileSha256(realExecutable);
  } catch (error) {
    return { verified: false, reason: `checksum_unreadable:${error.code || error.message}` };
  }
  if (actualSha256 !== String(expectedSha256).toLowerCase()) {
    return { verified: false, reason: 'checksum_mismatch' };
  }
  const resolved = {
    source: 'managed',
    path: realExecutable,
    sha256: actualSha256,
    version: entry.version,
    cliVersion: entry.version,
    manifestPath,
    warnings: [],
  };
  const probe = probeVersion(resolved);
  if (probe.error || probe.status !== 0 || probe.version !== entry.version) {
    return { verified: false, reason: 'version_probe_mismatch' };
  }
  return { verified: true, reason: null, executablePath: realExecutable, resolved };
}

function reportUnverifiedManagedCliInventory(report, entries, reason) {
  for (const entry of entries) {
    report.reclaimable.push({
      version: entry.version,
      path: entry.versionDir,
      bytes: entry.bytes,
      reason,
    });
    report.reclaimable_bytes += entry.bytes;
  }
}

function managedCliRetentionReportUnlocked(resolved, probe, options = {}) {
  const dataDir = options.dataDir || pluginDataDir();
  const dryRun = options.dryRun ?? process.env.CODESTORY_PLUGIN_CLI_RETENTION_DRY_RUN === '1';
  const report = {
    policy: 'active_plus_one_verified_adjacent',
    dry_run: dryRun,
    active_version: probe.version || resolved.version || null,
    retained: [],
    removed: [],
    reclaimable: [],
    retained_bytes: 0,
    removed_bytes: 0,
    reclaimable_bytes: 0,
    warnings: [],
  };
  if (!dataDir || resolved.source !== 'managed') {
    report.warnings.push('managed_cli_retention_not_applicable');
    return report;
  }
  const inventory = managedCliVersionEntries(dataDir);
  report.warnings.push(...inventory.errors);
  if (inventory.errors.length > 0) return report;
  for (const entry of inventory.staging) {
    if (!dryRun && entry.stale) {
      try {
        fs.rmSync(entry.versionDir, { recursive: true, force: false });
        report.removed.push({
          version: entry.version,
          path: entry.versionDir,
          bytes: entry.bytes,
          reason: `abandoned_${entry.reason}`,
        });
        report.removed_bytes += entry.bytes;
        continue;
      } catch (error) {
        report.warnings.push(`managed_cli_staging_remove_failed:${error.code || error.message}`);
      }
    }
    report.reclaimable.push({
      version: entry.version,
      path: entry.versionDir,
      bytes: entry.bytes,
      reason: entry.stale ? `abandoned_${entry.reason}` : entry.reason,
    });
    report.reclaimable_bytes += entry.bytes;
  }
  if (probe.error || probe.status !== 0) {
    report.warnings.push('managed_cli_retention_active_unverified:version_probe_failed');
    reportUnverifiedManagedCliInventory(report, inventory.entries, 'active_unverified');
    return report;
  }
  if (probe.version !== resolved.version) {
    report.warnings.push('managed_cli_retention_active_version_mismatch');
    reportUnverifiedManagedCliInventory(report, inventory.entries, 'active_version_mismatch');
    return report;
  }

  const active = inventory.entries.find((entry) => entry.version === resolved.version);
  if (!active) {
    report.warnings.push('managed_cli_retention_active_directory_missing');
    reportUnverifiedManagedCliInventory(report, inventory.entries, 'active_directory_missing');
    return report;
  }
  const activeVerification = verifyManagedCliVersion(active, options.probeVersion || probeResolvedCli);
  if (!activeVerification.verified
      || !samePathText(activeVerification.executablePath, resolved.path)) {
    report.warnings.push(`managed_cli_retention_active_unverified:${activeVerification.reason || 'path_mismatch'}`);
    reportUnverifiedManagedCliInventory(report, inventory.entries, 'active_unverified');
    return report;
  }

  const newer = inventory.entries.filter((entry) => compareManagedCliVersions(entry.version, active.version) > 0);
  const older = inventory.entries.filter((entry) => compareManagedCliVersions(entry.version, active.version) < 0);
  let adjacent = null;
  for (const entry of [...newer, ...older]) {
    const verification = verifyManagedCliVersion(entry, options.probeVersion || probeResolvedCli);
    if (verification.verified) {
      adjacent = entry;
      break;
    }
  }

  const retainedVersions = new Set([active.version]);
  if (adjacent) retainedVersions.add(adjacent.version);
  for (const entry of inventory.entries) {
    if (retainedVersions.has(entry.version)) {
      const reason = entry.version === active.version
        ? 'active'
        : compareManagedCliVersions(entry.version, active.version) > 0
          ? 'newer_pending_activation'
          : 'rollback';
      report.retained.push({ version: entry.version, path: entry.versionDir, bytes: entry.bytes, reason });
      report.retained_bytes += entry.bytes;
      continue;
    }
    if (entry.scanError || entry.provisioning) {
      report.reclaimable.push({
        version: entry.version,
        path: entry.versionDir,
        bytes: entry.bytes,
        reason: entry.scanError || 'provisioning',
      });
      report.reclaimable_bytes += entry.bytes;
      continue;
    }
    if (dryRun) {
      report.reclaimable.push({
        version: entry.version,
        path: entry.versionDir,
        bytes: entry.bytes,
        reason: 'outside_retention_window',
      });
      report.reclaimable_bytes += entry.bytes;
      continue;
    }
    const removal = removeManagedCliVersion(entry, {
      platform: options.platform || process.platform,
      unlinkSync: options.unlinkSync || fs.unlinkSync,
      rmSync: options.rmSync || fs.rmSync,
    });
    if (removal.removed) {
      report.removed.push({
        version: entry.version,
        path: entry.versionDir,
        bytes: entry.bytes,
        reason: 'outside_retention_window',
      });
      report.removed_bytes += entry.bytes;
    } else {
      let remainingBytes = entry.bytes;
      try {
        remainingBytes = fs.existsSync(entry.versionDir) ? managedPathSize(entry.versionDir) : 0;
      } catch {
        // Keep the pre-delete size when a partial failure also prevents measurement.
      }
      report.reclaimable.push({
        version: entry.version,
        path: entry.versionDir,
        bytes: remainingBytes,
        reason: removal.reason,
      });
      report.reclaimable_bytes += remainingBytes;
    }
  }
  return report;
}

function managedCliRetentionReport(resolved, probe, options = {}) {
  const dataDir = options.dataDir || pluginDataDir();
  if (!dataDir || resolved.source !== 'managed') {
    return managedCliRetentionReportUnlocked(resolved, probe, options);
  }
  let root;
  try {
    root = managedCliRoot(dataDir, true);
  } catch (error) {
    const report = managedCliRetentionReportUnlocked(resolved, probe, {
      ...options,
      dataDir,
      dryRun: true,
    });
    report.warnings.push(`managed_cli_retention_root_failed:${error.code || error.message}`);
    return report;
  }
  let lock;
  try {
    lock = acquireManagedCliLock(root, 'retention');
  } catch (error) {
    const report = managedCliRetentionReportUnlocked(resolved, probe, {
      ...options,
      dataDir,
      dryRun: true,
    });
    report.warnings.push(`managed_cli_retention_lock_failed:${error.code || error.message}`);
    return report;
  }
  if (!lock) {
    const report = managedCliRetentionReportUnlocked(resolved, probe, {
      ...options,
      dataDir,
      dryRun: true,
    });
    report.warnings.push('managed_cli_retention_locked');
    return report;
  }
  try {
    return managedCliRetentionReportUnlocked(resolved, probe, { ...options, dataDir });
  } finally {
    try {
      releaseManagedCliLock(lock);
    } catch {
      // The PID/token lock is reclaimed after an interrupted owner exits.
    }
  }
}

function removeManagedCliVersion(entry, options) {
  if (options.platform === 'win32') {
    const knownExecutables = [
      path.join(entry.versionDir, 'bin', binaryName),
      path.join(entry.versionDir, binaryName),
    ].filter((candidate) => fs.existsSync(candidate) && pathInside(candidate, entry.versionDir));
    for (const executable of knownExecutables) {
      try {
        options.unlinkSync(executable);
      } catch (error) {
        if (['EPERM', 'EBUSY', 'EACCES'].includes(error.code)) {
          return { removed: false, reason: `locked:${error.code}` };
        }
        return { removed: false, reason: `unlink_failed:${error.code || error.message}` };
      }
    }
  }
  try {
    options.rmSync(entry.versionDir, { recursive: true, force: false });
    return { removed: true, reason: null };
  } catch (error) {
    return { removed: false, reason: `remove_failed:${error.code || error.message}` };
  }
}

function localWaitFreshCommand(projectRoot = launchCwd) {
  return `<managed codestory-cli> ready --goal local --wait-fresh --project ${JSON.stringify(projectRoot)} --format json`;
}

function singleRepairNextCalls(commands) {
  const needsHost = commands.some((command) => command.startsWith('Restart/reload') || command.startsWith('Refresh or reinstall'));
  if (needsHost) {
    return [
      { method: 'host/restart', instruction: commands[0] },
      { method: 'resources/read', uri: 'codestory://status' },
    ];
  }
  const sidecarRepair = commands.find((command) => command.includes('ready --goal agent --repair'));
  if (sidecarRepair) {
    return [
      {
        method: 'resources/read',
        uri: 'codestory://status',
        instruction: 'Diagnostic MCP cannot run sidecar repair until a compatible stdio runtime starts; restore or reload the runtime before retrying grounding.',
        debug_command: sidecarRepair,
      },
    ];
  }
  return [
    {
      method: 'resources/read',
      uri: 'codestory://status',
      instruction: 'Diagnostic MCP cannot run full repair until a compatible stdio runtime starts; restore the runtime before retrying grounding.',
      debug_commands: commands,
    },
  ];
}

function localWaitFreshTimeoutMs() {
  const parsed = Number.parseInt(process.env.CODESTORY_PLUGIN_LOCAL_REPAIR_TIMEOUT_MS || '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 30000;
}

function runLocalNavigationWaitFresh(resolved, projectRoot) {
  const args = ['ready', '--goal', 'local', '--wait-fresh'];
  args.push('--project', projectRoot, '--format', 'json');
  const result = spawnSync(resolved.path, args, {
    cwd: projectRoot,
    encoding: 'utf8',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    timeout: localWaitFreshTimeoutMs(),
    windowsHide: true,
  });
  return { args, result };
}

function runAgentReadiness(resolved, projectRoot) {
  const args = ['ready', '--goal', 'agent'];
  args.push('--project', projectRoot, '--format', 'json');
  const result = spawnSync(resolved.path, args, {
    cwd: projectRoot,
    encoding: 'utf8',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    timeout: localWaitFreshTimeoutMs(),
    windowsHide: true,
  });
  return { args, result };
}

function localReadySetup(prefix, args, result) {
  return {
    [`${prefix}_args`]: args,
    [`${prefix}_status`]: result.status,
    [`${prefix}_error`]: result.error ? result.error.message : null,
    [`${prefix}_stdout`]: result.stdout || '',
    [`${prefix}_stderr`]: result.stderr || '',
  };
}

function parseLocalReadyResult(result) {
  let parsed;
  try {
    parsed = JSON.parse(result.stdout || '{}');
  } catch (error) {
    return {
      ok: false,
      invalidJson: error.message,
      verdict: null,
    };
  }
  const verdict = Array.isArray(parsed.verdicts)
    ? parsed.verdicts.find((item) => item && item.goal === 'local_navigation') || parsed.verdicts[0]
    : null;
  return {
    ok: Boolean(verdict && verdict.status === 'ready'),
    invalidJson: null,
    verdict,
    localRefresh: parsed.local_refresh || null,
  };
}

function parseAgentReadinessResult(result) {
  let parsed;
  try {
    parsed = JSON.parse(result.stdout || '{}');
  } catch (error) {
    return {
      ok: false,
      invalidJson: error.message,
      parsed: null,
      verdict: null,
    };
  }
  const verdict = Array.isArray(parsed.verdicts)
    ? parsed.verdicts.find((item) => item && item.goal === 'agent_packet_search') || null
    : null;
  const lane = parsed.readiness_lanes?.agent_packet_search || null;
  return {
    ok: Boolean((verdict && verdict.status === 'ready') || (lane && lane.status === 'ready')),
    invalidJson: null,
    parsed,
    verdict,
  };
}

function localRefreshFailure(state, reason, readinessStatus = 'repair_setup') {
  return {
    state,
    blocks_local_surfaces: true,
    readiness_status: readinessStatus,
    reason,
    changed_file_count: 0,
    new_file_count: 0,
    removed_file_count: 0,
    fatal_error_count: 0,
  };
}

function localNavigationFailure(projectRoot, reason, status, summary, setup, verdict = null, localRefresh = null) {
  const repair = localWaitFreshCommand(projectRoot);
  return {
    ready: false,
    reason,
    status,
    summary,
    localRefresh,
    minimumNext: Array.isArray(verdict?.minimum_next) && verdict.minimum_next.length > 0
      ? verdict.minimum_next
      : [repair],
    fullRepair: Array.isArray(verdict?.full_repair) && verdict.full_repair.length > 0
      ? verdict.full_repair
      : [repair, `codestory-cli doctor --project ${JSON.stringify(projectRoot)}`],
    setup: {
      ...setup,
      readiness_verdict: verdict,
    },
  };
}

function probeLocalNavigation(resolved, projectRoot = process.cwd()) {
  const probe = runLocalNavigationWaitFresh(resolved, projectRoot);
  const setup = {
    ...localReadySetup('local_wait_fresh', probe.args, probe.result),
  };
  if (probe.result.error || probe.result.status !== 0) {
    const timeout = probe.result.error && probe.result.error.code === 'ETIMEDOUT';
    return localNavigationFailure(
      projectRoot,
      timeout ? 'local_navigation_wait_fresh_timeout' : 'local_navigation_wait_fresh_failed',
      'repair_setup',
      timeout
        ? 'CodeStory MCP timed out while refreshing local navigation before starting stdio.'
        : 'CodeStory MCP could not refresh local navigation before starting stdio.',
      setup,
      null,
      localRefreshFailure('failed', timeout ? 'wait_fresh_timeout' : 'wait_fresh_failed'),
    );
  }
  const parsedProbe = parseLocalReadyResult(probe.result);
  if (parsedProbe.invalidJson) {
    return localNavigationFailure(
      projectRoot,
      'local_navigation_wait_fresh_invalid_json',
      'repair_setup',
      `CodeStory MCP local wait-fresh returned invalid JSON: ${parsedProbe.invalidJson}`,
      setup,
      null,
      localRefreshFailure('failed', `invalid_json:${parsedProbe.invalidJson}`),
    );
  }
  if (parsedProbe.ok) {
    return {
      ready: true,
      setup,
      verdict: parsedProbe.verdict || null,
      localRefresh: parsedProbe.localRefresh || null,
    };
  }
  const verdict = parsedProbe.verdict || null;
  const status = typeof verdict?.status === 'string' ? verdict.status : 'repair_setup';
  const state = typeof parsedProbe.localRefresh?.state === 'string' ? parsedProbe.localRefresh.state : status;
  return localNavigationFailure(
    projectRoot,
    `local_navigation_wait_fresh_${state}`,
    status,
    verdict?.summary || 'CodeStory local navigation is not ready after wait-fresh.',
    setup,
    verdict,
    parsedProbe.localRefresh || null,
  );
}

function probeAgentReadiness(resolved, projectRoot = process.cwd()) {
  const probe = runAgentReadiness(resolved, projectRoot);
  const setup = {
    ...localReadySetup('agent_readiness', probe.args, probe.result),
  };
  if (probe.result.error || probe.result.status !== 0) {
    return {
      ready: false,
      setup,
      parsed: null,
      verdict: null,
      reason: probe.result.error
        ? probe.result.error.message
        : `agent_readiness_failed:${probe.result.status}`,
    };
  }
  const parsedProbe = parseAgentReadinessResult(probe.result);
  return {
    ready: parsedProbe.ok,
    setup,
    parsed: parsedProbe.parsed,
    verdict: parsedProbe.verdict,
    reason: parsedProbe.invalidJson ? `agent_readiness_invalid_json:${parsedProbe.invalidJson}` : null,
  };
}

function pluginRuntimeForResolved(resolved) {
  return {
    plugin_version: resolved.version,
    plugin_root: pluginRoot,
    plugin_cache_version: pluginCacheVersion(),
    plugin_data: pluginDataDir(),
    launch_cwd: launchCwd,
    runtime_cwd: process.cwd(),
    cli_source: resolved.source,
    cli_path: resolved.path,
    cli_sha256: resolved.sha256,
    build_source: resolved.buildSource,
    repo_ref: resolved.repoRef,
    local_dev_override: resolved.source === 'local_dev_override',
    managed_binary_path: resolved.source === 'managed' ? resolved.path : null,
    managed_binary_sha256: resolved.source === 'managed' ? resolved.sha256 : null,
    managed_manifest_path: resolved.manifestPath || null,
    managed_cli_retention: resolved.managedCliRetention || null,
    warnings: resolved.warnings.filter(Boolean),
  };
}

function fallbackDiagnostic(resolved, probe, reason, options = {}) {
  const projectRoot = Object.hasOwn(options, 'projectRoot') ? options.projectRoot : launchCwd;
  const managedProvisionFailed = String(reason || '').startsWith('managed_cli_provision_failed:');
  const managedProvisionNext = [
    'Restart/reload the Codex host/app and read codestory://status; managed CLI provisioning will retry release asset downloads.',
    'Refresh or reinstall the CodeStory plugin after GitHub release assets are reachable, then restart/reload the Codex host/app and read codestory://status.',
  ];
  const minimumNext = options.minimumNext || (managedProvisionFailed ? managedProvisionNext : [
    'Refresh or reinstall the CodeStory plugin, then restart/reload the Codex host/app and read codestory://status in a fresh thread.',
  ]);
  const fullRepair = options.fullRepair || minimumNext;
  const recommendedNext = options.recommendedNext || fullRepair;
  const sidecarPolicy = readSidecarPolicy();
  const lastRepair = normalizedSidecarLastRepair(sidecarPolicy.lastRepair, resolved.version, resolved.path);
  const plugin = pluginRuntimeForResolved({ ...resolved, warnings: [...resolved.warnings, reason] });
  const repair = {
    goal: options.goal || 'local_navigation',
    status: options.status || 'repair_setup',
    summary: options.summary || 'CodeStory plugin MCP could not start a compatible codestory-cli stdio runtime.',
    repair_reason: reason,
    local_refresh: options.localRefresh || null,
    minimum_next: minimumNext,
    full_repair: fullRepair,
    setup: {
      active_path: resolved.path,
      active_version: probe.version,
      expected_version: resolved.version,
      probe_error: probe.error,
      probe_status: probe.status,
      probe_stdout: probe.stdout,
      probe_stderr: probe.stderr,
      ...(options.setup || {}),
    },
  };
  const localSurfaces = [
    'ground',
    'files',
    'symbol',
    'definition',
    'callers',
    'callees',
    'trail',
    'trace',
    'references',
    'snippet',
    'affected',
    'symbols',
    'get_node',
    'neighbors',
    'shortest_path',
    'query_subgraph',
  ];
  const sidecarSurfaces = ['packet', 'search', 'context'];
  const blockedSurface = () => ({
    allowed: false,
    readiness_goal: repair.goal,
    failed_layer: 'runtime_setup',
    repair_reason: reason,
  });
  const allowedSurfaces = Object.fromEntries([
    ...localSurfaces.map((surface) => [surface, blockedSurface()]),
    ...sidecarSurfaces.map((surface) => [surface, blockedSurface()]),
  ]);
  allowedSurfaces.repair_all = {
    ...blockedSurface('repair_all', 'agent_packet_search'),
    blocked_reason: 'diagnostic_fail_open',
  };
  const sidecarSetup = {
    state: sidecarPolicy.state || 'unavailable',
    auto_repair: false,
    status_triggered_repair: false,
    explicit_repair_enabled: sidecarPolicy.state === 'enabled',
    repair_mode: 'diagnostic_fail_open',
    prompt_required: sidecarPolicy.state === 'ask',
    prompt: sidecarPolicy.state === 'ask'
      ? 'CodeStory packet/search needs retrieval sidecars. MCP repair may start or download retrieval sidecars for this project. Enable MCP sidecar repair for this plugin install?'
      : null,
    last_repair: lastRepair,
    active_repair: null,
    abandoned_repair: null,
  };
  allowedSurfaces.sidecar_setup = {
    allowed: true,
    readiness_goal: repair.goal,
    status: repair.status,
    repair_reason: reason,
    blocked_reason: 'diagnostic_fail_open_repair_unavailable',
    summary: 'sidecar_setup status and policy actions are available in diagnostic fail-open mode; repair requires the real stdio runtime.',
    allowed_actions: ['status', 'enable', 'disable', 'ask'],
    canonical_arguments: { action: 'status' },
    minimum_next: minimumNext,
    full_repair: fullRepair,
  };
  const readinessBroker = {
    schema_version: 2,
    identity: null,
    install_id: null,
    project_id: null,
    canonical_root_hash: null,
    workspace_root: projectRoot,
    cli_version: probe.version,
    updated_at_epoch_ms: Date.now(),
    snapshot_path: null,
    persistence_status: 'unavailable',
    persistence_error: 'diagnostic_fail_open',
    operations: [],
    resources: {},
    reconciliation: {
      status: 'diagnostic_fail_open',
      cleanup_performed: false,
      stale_status_paths_removed: [],
      stale_lock_paths_removed: [],
      abandoned_repairs: [],
      local_refresh_cleanups: [],
      active_repair: null,
      unresolved_orphan_reason: reason,
    },
    gpu_proof: {
      requested: false,
      requested_provider: null,
      requested_device: null,
      policy: null,
      observed_state: null,
      observation_source: null,
      detected_provider: null,
      detected_gpu: null,
      cpu_allowed: false,
      proof_status: 'diagnostic_fail_open',
      meaningful_accelerator_work_proven: false,
      embed_smoke_ok: null,
      embed_smoke_ms: null,
      degraded_reason: reason,
    },
  };
  return {
    server_version: null,
    cli_version: probe.version,
    server_executable: null,
    server_executable_sha256: null,
    source_checkout_version: sourceCheckoutVersion(projectRoot),
    sidecar_contract_version: null,
    plugin_runtime: plugin,
    runtime_truth: runtimeTruthStatus(plugin, {
      sidecarPolicy: sidecarPolicy.state || 'unavailable',
      localRefresh: options.localRefresh || null,
      readinessGoals: [repair.goal],
      hasReadinessBroker: true,
    }),
    runtime_boundary: {
      restart_required_for_runtime_change: true,
      message: 'A running MCP server keeps using the CLI process it was launched with; plugin refresh, managed runtime provisioning, or CODESTORY_CLI changes require a host reload/restart and fresh codestory://status readback.',
    },
    warnings: plugin.warnings,
    project_root: projectRoot,
    project_root_source: options.projectRootSource || null,
    retrieval_mode: 'unavailable',
    degraded_reason: reason,
    local_refresh: options.localRefresh || null,
    readiness: [repair],
    sidecar_setup: sidecarSetup,
    readiness_broker: readinessBroker,
    allowed_surfaces: allowedSurfaces,
    recommended_next_calls: singleRepairNextCalls(recommendedNext),
  };
}

function projectRootUnavailableDiagnostic(resolved, probe, projectResolution) {
  const projectRoot = projectResolution.projectRoot || null;
  const reason = projectRoot ? 'project_root_recovered_after_launch' : projectResolution.reason;
  return fallbackDiagnostic(resolved, probe, reason, {
    projectRoot,
    projectRootSource: projectResolution.source,
    goal: 'project_root',
    status: 'repair_setup',
    summary: projectRoot
      ? 'CodeStory plugin MCP found a target project root after diagnostic startup and will hand off to the real stdio runtime on the next request.'
      : 'CodeStory plugin MCP could not determine the target project root before starting stdio.',
    minimumNext: projectRoot
      ? ['Retry the CodeStory MCP request; this diagnostic wrapper will hand off to codestory-cli serve --stdio.']
      : ['Start or reload the Codex host from the target repo, then read codestory://status.'],
    fullRepair: projectRoot
      ? [
          'Retry the CodeStory MCP request; this diagnostic wrapper will hand off to codestory-cli serve --stdio.',
          'If the client cached the earlier tool list, restart/reload the Codex host and read codestory://status.',
        ]
      : [
          'Start or resume the Codex session in the target repo so CodeStory can infer the active project root.',
          'Restart/reload the Codex host from the target repo, then read codestory://status.',
        ],
    setup: projectResolutionDiagnostics(projectResolution),
  });
}

async function bootstrapStatus(projectRoot = launchCwd) {
  projectRoot = normalizeProjectRoot(projectRoot);
  const resolved = await resolveCli();
  const probe = probeResolvedCli(resolved);
  const failOpenReason = failOpenReasonForProbe(resolved, probe);
  if (failOpenReason) {
    resolved.managedCliRetention = managedCliRetentionReport(resolved, probe, { dryRun: true });
    rememberLaunch(resolved);
    return {
      ready: false,
      ...fallbackDiagnostic(resolved, probe, failOpenReason, { projectRoot }),
    };
  }
  resolved.managedCliRetention = managedCliRetentionReport(resolved, probe);
  rememberLaunch(resolved);

  const localReadiness = probeLocalNavigation(resolved, projectRoot);
  if (!localReadiness.ready) {
    return {
      ready: false,
      ...fallbackDiagnostic(resolved, probe, localReadiness.reason, {
        projectRoot,
        status: localReadiness.status,
        summary: localReadiness.summary,
        minimumNext: localReadiness.minimumNext,
        fullRepair: localReadiness.fullRepair,
        localRefresh: localReadiness.localRefresh,
        setup: localReadiness.setup,
      }),
    };
  }

  const sidecarPolicy = readSidecarPolicy();
  const plugin = pluginRuntimeForResolved(resolved);
  const agentReadiness = probeAgentReadiness(resolved, projectRoot);
  const localRefresh = localReadiness.localRefresh || {
    state: 'fresh',
    blocks_local_surfaces: false,
    readiness_status: 'ready',
  };
  const repair = {
    goal: 'local_navigation',
    status: 'ready',
    summary: localReadiness.verdict?.summary || 'CodeStory local navigation is ready.',
    repair_reason: null,
    local_refresh: localRefresh,
    minimum_next: [{ method: 'resources/read', uri: 'codestory://status' }],
    full_repair: [],
    setup: {
      ...localReadiness.setup,
      readiness_verdict: localReadiness.verdict,
      ...agentReadiness.setup,
      agent_readiness_verdict: agentReadiness.verdict,
      agent_readiness_reason: agentReadiness.reason,
    },
  };
  const readiness = [repair];
  if (agentReadiness.verdict) readiness.push(agentReadiness.verdict);
  return {
    ready: true,
    project_root: projectRoot,
    server_version: resolved.version,
    cli_version: probe.version,
    plugin_runtime: plugin,
    project_root_source: 'argument',
    runtime_truth: runtimeTruthStatus(plugin, {
      sidecarPolicy: sidecarPolicy.state || 'unavailable',
      localRefresh,
      readinessGoals: readiness.map((verdict) => verdict.goal),
      readinessLanes: agentReadiness.parsed?.readiness_lanes || null,
      hasReadinessBroker: Boolean(agentReadiness.parsed?.readiness_broker),
    }),
    local_refresh: localRefresh,
    readiness,
    readiness_lanes: agentReadiness.parsed?.readiness_lanes || null,
    readiness_broker: agentReadiness.parsed?.readiness_broker || null,
    recommended_next_calls: [{ method: 'resources/read', uri: 'codestory://status' }],
  };
}

async function handleBootstrapStatusCommand(argv) {
  if (argv[2] !== 'bootstrap-status') return false;
  const resolution = resolveProjectRoot({ projectRoot: optionValue(argv, '--project') });
  try {
    if (!resolution.projectRoot) {
      const resolved = await resolveCli();
      const probe = probeResolvedCli(resolved);
      const failOpenReason = failOpenReasonForProbe(resolved, probe);
      resolved.managedCliRetention = managedCliRetentionReport(resolved, probe, {
        dryRun: Boolean(failOpenReason),
      });
      rememberLaunch(resolved);
      process.stdout.write(`${JSON.stringify({
        ready: false,
        ...projectRootUnavailableDiagnostic(resolved, probe, resolution),
      })}\n`);
      process.exit(0);
    }
    const projectRoot = resolution.projectRoot;
    process.stdout.write(`${JSON.stringify(await bootstrapStatus(projectRoot))}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      ready: false,
      degraded_reason: `launcher_error:${error.message}`,
      project_root: resolution.projectRoot,
      project_root_source: resolution.source,
    })}\n`);
  }
  process.exit(0);
}

function samePathText(left, right) {
  const normalize = (value) => String(value || '').replace(/[\\/]+$/u, '').toLowerCase();
  return normalize(left) === normalize(right);
}

function pathInside(child, parent) {
  const relative = path.relative(path.resolve(parent), path.resolve(child));
  return relative === '' || (relative && !relative.startsWith('..') && !path.isAbsolute(relative));
}

function ensureRuntimeDirectory(candidate) {
  if (!candidate) return null;
  try {
    fs.mkdirSync(candidate, { recursive: true });
    return fs.statSync(candidate).isDirectory() ? fs.realpathSync(candidate) : null;
  } catch {
    return null;
  }
}

function launcherRuntimeCwd() {
  const dataDir = pluginDataDir();
  for (const candidate of [
    dataDir ? path.join(dataDir, 'runtime-cwd') : null,
    dataDir,
    os.tmpdir(),
  ]) {
    const runtimeCwd = ensureRuntimeDirectory(candidate);
    if (runtimeCwd && !pathInside(runtimeCwd, pluginRoot)) return runtimeCwd;
  }
  return launchCwd;
}

function releasePluginCacheCwd() {
  const current = path.resolve(process.cwd());
  if (!pathInside(current, pluginRoot)) return current;
  const runtimeCwd = launcherRuntimeCwd();
  if (!runtimeCwd || pathInside(runtimeCwd, pluginRoot)) return current;
  try {
    process.chdir(runtimeCwd);
    return runtimeCwd;
  } catch {
    return current;
  }
}

function sourceCheckoutVersion(projectRoot) {
  if (!projectRoot) return null;
  try {
    return cargoPackageVersion(fs.readFileSync(path.join(projectRoot, 'crates', 'codestory-cli', 'Cargo.toml'), 'utf8'));
  } catch {
    return null;
  }
}

function cargoPackageVersion(manifest) {
  let inPackage = false;
  for (const line of manifest.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (/^\[[^\]]+\]$/u.test(trimmed)) {
      inPackage = trimmed === '[package]';
      continue;
    }
    if (!inPackage) continue;
    const match = trimmed.match(/^version\s*=\s*"([^"]+)"/u);
    if (match) return match[1];
  }
  return null;
}

function diagnosticText(status) {
  const setup = status.readiness[0].setup;
  const next = status.recommended_next_calls?.[0] || status.readiness[0].minimum_next?.[0] || null;
  return [
    'CodeStory MCP runtime is not ready.',
    `reason: ${status.degraded_reason}`,
    `plugin_version: ${status.plugin_runtime.plugin_version || '<unknown>'}`,
    `plugin_root: ${status.plugin_runtime.plugin_root}`,
    `cli_source: ${status.plugin_runtime.cli_source}`,
    `cli_path: ${setup.active_path}`,
    `cli_version: ${setup.active_version || '<unknown>'}`,
    `source_checkout_version: ${status.source_checkout_version || '<none>'}`,
    `next: ${next ? JSON.stringify(next) : 'read codestory://status'}`,
  ].join('\n');
}

function jsonrpcResult(id, result) {
  return { jsonrpc: '2.0', id, result };
}

function jsonrpcError(id, code, message) {
  return { jsonrpc: '2.0', id, error: { code, message } };
}

function resourceContents(uri, value) {
  return {
    contents: [{
      uri,
      mimeType: 'application/json',
      text: JSON.stringify(value),
    }],
  };
}

function failOpenSidecarSetupResult(request, status = null) {
  const action = request.params?.arguments?.action || 'status';
  if (action === 'repair') {
    return {
      isError: true,
      content: [{ type: 'text', text: 'sidecar_setup repair is unavailable while CodeStory is in diagnostic fail-open mode; reload the host or restore the stdio runtime first.' }],
      structuredContent: {
        code: 'repair_unavailable_diagnostic_fail_open',
        action,
        recommended_next_calls: [{
          method: 'resources/read',
          uri: 'codestory://status',
          instruction: 'Read status again after restoring the compatible stdio runtime.',
        }],
      },
    };
  }
  if (!['status', 'enable', 'disable', 'ask'].includes(action)) {
    return {
      isError: true,
      content: [{ type: 'text', text: 'sidecar_setup.action must be status, enable, disable, or ask while the runtime is in diagnostic mode.' }],
      structuredContent: { code: 'invalid_sidecar_setup_action', action },
    };
  }
  if (action !== 'status') {
    writeSidecarPolicy(sidecarPolicyStateForAction(action));
  }
  const policy = readSidecarPolicy();
  const lastRepair = normalizedSidecarLastRepair(
    policy.lastRepair,
    status?.plugin_runtime?.plugin_version,
    status?.plugin_runtime?.cli_path,
  );
  const statusPolicy = { ...policy, lastRepair };
  return {
    content: [{ type: 'text', text: JSON.stringify(statusPolicy) }],
    structuredContent: {
      state: policy.state,
      path: policy.path,
      updated_at: policy.updatedAt,
      last_repair: lastRepair,
    },
  };
}

function runFailOpenMcp(status, options = {}) {
  const currentStatus = () => (typeof status === 'function' ? status() : status);
  let handoff = null;
  const maybeHandoff = () => {
    if (handoff || typeof options.startRuntime !== 'function') {
      return handoff;
    }
    const liveStatus = currentStatus();
    if (!liveStatus.project_root || liveStatus.degraded_reason !== 'project_root_recovered_after_launch') {
      return null;
    }
    handoff = options.startRuntime(liveStatus);
    handoff.stdout?.pipe(process.stdout);
    handoff.stderr?.pipe(process.stderr);
    handoff.on('exit', (code, signal) => {
      if (signal) process.kill(process.pid, signal);
      process.exit(code || 0);
    });
    handoff.on('error', (error) => {
      process.stdout.write(`${JSON.stringify(jsonrpcError(null, -32000, `CodeStory stdio handoff failed: ${error.message}`))}\n`);
    });
    return handoff;
  };
  const tools = [{
    name: 'sidecar_setup',
    description: 'Read or change plugin-local sidecar setup policy while CodeStory is in diagnostic fail-open mode. Repair requires the real stdio runtime.',
    inputSchema: {
      type: 'object',
      properties: {
        action: { type: 'string', enum: ['status', 'enable', 'disable', 'ask'], default: 'status' },
      },
      additionalProperties: false,
    },
    outputSchema: { type: 'object', additionalProperties: true },
    safety: {
      readOnly: false,
      sideEffects: true,
      localOnly: true,
      openWorld: false,
      mutation: 'local_plugin_configuration',
    },
    annotations: {
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
    },
  }];
  const resources = [
    { uri: 'codestory://status', name: 'CodeStory runtime status', mimeType: 'application/json' },
    { uri: 'codestory://agent-guide', name: 'CodeStory agent guide', mimeType: 'application/json' },
  ];
  const guide = () => {
    const liveStatus = currentStatus();
    return {
      status: 'repair_setup',
      message: 'Read codestory://status, follow recommended_next_calls, restore a compatible stdio runtime, then retry grounding.',
      recommended_next_calls: liveStatus.recommended_next_calls,
    };
  };
  let buffer = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    buffer += chunk;
    const lines = buffer.split(/\r?\n/u);
    buffer = lines.pop() || '';
    for (const line of lines) {
      if (!line.trim()) continue;
      let request;
      try {
        request = JSON.parse(line);
      } catch {
        process.stdout.write(`${JSON.stringify(jsonrpcError(null, -32700, 'Parse error'))}\n`);
        continue;
      }
      const delegated = maybeHandoff();
      if (delegated) {
        delegated.stdin.write(`${line}\n`);
        continue;
      }
      if (request.id === undefined) continue;
      let response;
      if (request.method === 'initialize') {
        const liveStatus = currentStatus();
        response = jsonrpcResult(request.id, {
          protocolVersion: request.params?.protocolVersion || '2024-11-05',
          capabilities: { tools: {}, resources: {} },
          serverInfo: { name: 'codestory', version: resolvedVersionForStatus(liveStatus) },
        });
      } else if (request.method === 'tools/list') {
        response = jsonrpcResult(request.id, { tools });
      } else if (request.method === 'resources/list') {
        response = jsonrpcResult(request.id, { resources });
      } else if (request.method === 'resources/read') {
        const uri = request.params?.uri;
        if (uri === 'codestory://status') {
          response = jsonrpcResult(request.id, resourceContents(uri, currentStatus()));
        } else if (uri === 'codestory://agent-guide') {
          response = jsonrpcResult(request.id, resourceContents(uri, guide()));
        } else {
          response = jsonrpcError(request.id, -32602, `unknown resource: ${uri || '<missing>'}`);
        }
      } else if (request.method === 'tools/call') {
        if (request.params?.name === 'sidecar_setup') {
          response = jsonrpcResult(request.id, failOpenSidecarSetupResult(request, currentStatus()));
        } else {
          response = jsonrpcError(request.id, -32602, 'CodeStory grounding tools are unavailable in diagnostic fail-open mode; read codestory://status and restore a compatible stdio runtime.');
        }
      } else {
        response = jsonrpcError(request.id, -32601, `method not found: ${request.method || '<missing>'}`);
      }
      process.stdout.write(`${JSON.stringify(response)}\n`);
    }
  });
}

function resolvedVersionForStatus(status) {
  return status.plugin_runtime.plugin_version || 'unknown';
}

function rememberLaunch(resolved, runtimeCwd = process.cwd()) {
  const dataDir = pluginDataDir();
  if (!dataDir) return;
  try {
    fs.mkdirSync(dataDir, { recursive: true });
    fs.writeFileSync(path.join(dataDir, '.codestory-mcp-runtime.json'), JSON.stringify({
      source: resolved.source,
      path: resolved.path,
      sha256: resolved.sha256,
      pluginRoot,
      launchCwd,
      runtimeCwd,
      pluginCacheVersion: pluginCacheVersion(),
      pluginVersion: resolved.version,
      manifestPath: resolved.manifestPath || null,
      cliVersion: resolved.cliVersion || null,
      repoRef: resolved.repoRef || null,
      buildSource: resolved.buildSource || null,
      archiveSha256: resolved.archiveSha256 || null,
      archiveUrl: resolved.archiveUrl || null,
      provisionedAt: resolved.provisionedAt || null,
      managedCliRetention: resolved.managedCliRetention || null,
      updatedAt: new Date().toISOString(),
    }, null, 2));
  } catch {
    // Best effort only. Launch metadata must not block MCP startup.
  }
}

function stdioRuntimeEnv(resolved, runtimeCwd) {
  const sidecarStatus = readSidecarPolicy();
  return {
    ...process.env,
    CODESTORY_PLUGIN_VERSION: resolved.version || '',
    CODESTORY_PLUGIN_ROOT: pluginRoot,
    CODESTORY_PLUGIN_LAUNCH_CWD: launchCwd,
    CODESTORY_PLUGIN_RUNTIME_CWD: runtimeCwd,
    CODESTORY_PLUGIN_CACHE_VERSION: pluginCacheVersion() || '',
    CODESTORY_PLUGIN_CLI_VERSION: resolved.cliVersion || resolved.version || '',
    CODESTORY_PLUGIN_CLI_SOURCE: resolved.source,
    CODESTORY_PLUGIN_CLI_PATH: resolved.path,
    CODESTORY_PLUGIN_CLI_SHA256: resolved.sha256 || '',
    CODESTORY_PLUGIN_CLI_MANIFEST_PATH: resolved.manifestPath || '',
    CODESTORY_PLUGIN_CLI_BUILD_SOURCE: resolved.buildSource || '',
    CODESTORY_PLUGIN_CLI_REPO_REF: resolved.repoRef || '',
    CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256: resolved.archiveSha256 || '',
    CODESTORY_PLUGIN_CLI_ARCHIVE_URL: resolved.archiveUrl || '',
    CODESTORY_PLUGIN_CLI_PROVISIONED_AT: resolved.provisionedAt || '',
    CODESTORY_PLUGIN_CLI_RETENTION: JSON.stringify(resolved.managedCliRetention || null),
    CODESTORY_PLUGIN_CLI_WARNINGS: resolved.warnings.join(';'),
    CODESTORY_PLUGIN_MULTI_PROJECT: '1',
    CODESTORY_PLUGIN_DATA: pluginDataDir() || '',
    CODESTORY_PLUGIN_SIDECAR_POLICY_STATE: sidecarStatus.state,
    CODESTORY_PLUGIN_SIDECAR_POLICY_PATH: sidecarStatus.path || '',
    CODESTORY_PLUGIN_SIDECAR_POLICY_UPDATED_AT: sidecarStatus.updatedAt || '',
    CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND: sidecarPolicyCommand('enable'),
    CODESTORY_PLUGIN_SIDECAR_DISABLE_COMMAND: sidecarPolicyCommand('disable'),
    CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_STATE: sidecarStatus.lastRepair?.state || '',
    CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_AT: sidecarStatus.lastRepair?.updated_at || '',
    CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_PROJECT: sidecarStatus.lastRepair?.project_root || '',
    CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_COMMAND: sidecarStatus.lastRepair?.command || '',
  };
}

function spawnStdioRuntime(resolved, runtimeCwd, stdio) {
  return spawn(resolved.path, ['serve', '--stdio', '--multi-project', '--refresh', 'none'], {
    cwd: runtimeCwd,
    stdio,
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    windowsHide: true,
    env: stdioRuntimeEnv(resolved, runtimeCwd),
  });
}

async function main() {
  if (await handleBootstrapStatusCommand(process.argv)) return;
  handleSidecarPolicyCommand(process.argv);
  const runtimeCwd = releasePluginCacheCwd();
  const resolved = await resolveCli();
  const probe = probeResolvedCli(resolved);
  const failOpenReason = failOpenReasonForProbe(resolved, probe);
  if (failOpenReason) {
    resolved.managedCliRetention = managedCliRetentionReport(resolved, probe, { dryRun: true });
    rememberLaunch(resolved, runtimeCwd);
    runFailOpenMcp(fallbackDiagnostic(resolved, probe, failOpenReason, {
      projectRoot: null,
      projectRootSource: 'request_argument',
    }));
    return;
  }
  resolved.managedCliRetention = managedCliRetentionReport(resolved, probe);
  rememberLaunch(resolved, runtimeCwd);
  const child = spawnStdioRuntime(resolved, runtimeCwd, 'inherit');

  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    if (!code) {
      process.exit(0);
      return;
    }
    runFailOpenMcp(fallbackDiagnostic(resolved, {
      ...probe,
      status: code,
      error: `codestory-cli serve --stdio exited with status ${code}`,
      stderr: probe.stderr || '',
    }, 'runtime_stdio_child_exit', {
      projectRoot: null,
      projectRootSource: 'request_argument',
      summary: 'CodeStory plugin MCP launched codestory-cli, but the stdio runtime exited before it could serve requests.',
      minimumNext: [
        'Call the status tool with an explicit project for the active runtime diagnostic.',
        'Restart/reload the Codex host/app after updating or repairing the CodeStory plugin runtime.',
      ],
    }));
  });

  child.on('error', (error) => {
    runFailOpenMcp(fallbackDiagnostic(resolved, {
      status: null,
      error: error.message,
      version: null,
      stdout: '',
      stderr: '',
    }, `${resolved.source}_cli_unspawnable`, {
      projectRoot: null,
      projectRootSource: 'request_argument',
    }));
  });
}

function runLauncherError(error) {
  releasePluginCacheCwd();
  const resolved = {
    source: 'launcher',
    path: 'codestory-cli',
    sha256: null,
    version: pluginVersion(),
    cliVersion: null,
    repoRef: null,
    buildSource: 'launcher',
    archiveSha256: null,
    archiveUrl: null,
    provisionedAt: null,
    warnings: [],
  };
  runFailOpenMcp(fallbackDiagnostic(resolved, {
    status: null,
    error: error.message,
    version: null,
    stdout: '',
    stderr: '',
  }, 'launcher_error'));
}

if (require.main === module) {
  main().catch(runLauncherError);
} else {
  module.exports = {
    _test: {
      compareManagedCliVersions,
      downloadFile,
      acquireManagedCliLock,
      reclaimStaleManagedCliPendingOwners,
      processStartIdentity,
      provisionManagedCli,
      quarantineManagedCliVersion,
      releaseManagedCliLock,
      resolveManagedCli,
      managedCliRetentionReport,
      managedCliVersionEntries,
      removeManagedCliVersion,
      verifyPublishedManagedCli,
      verifyManagedCliVersion,
    },
  };
}
