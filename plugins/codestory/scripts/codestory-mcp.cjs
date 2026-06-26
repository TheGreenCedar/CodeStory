#!/usr/bin/env node

const { spawn } = require('child_process');
const { spawnSync } = require('child_process');
const { createHash } = require('crypto');
const fs = require('fs');
const https = require('https');
const os = require('os');
const path = require('path');
const { dirtyMarkerPathForProject } = require('../hooks/codestory-runtime.cjs');

const pluginRoot = path.dirname(__dirname);
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
const fallbackBinaryNames = process.platform === 'win32'
  ? ['codestory-cli.exe', 'codestory-cli.cmd', 'codestory-cli']
  : ['codestory-cli'];
const sharedAgentRunId = 'shared-agent';
const releaseDownloadTimeoutMs = 60000;
const releaseDownloadAttempts = 3;
const releaseDownloadRetryDelaysMs = [1000, 3000];

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

function pluginDataDir() {
  return process.env.PLUGIN_DATA || process.env.COPILOT_PLUGIN_DATA || null;
}

function sidecarPolicyPath(dataDir = pluginDataDir()) {
  return dataDir ? path.join(dataDir, 'sidecar-setup-policy.json') : null;
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
  if (!['enable', 'disable', 'ask', 'status'].includes(action)) {
    process.stderr.write('usage: codestory-mcp.cjs sidecar-policy [enable|disable|ask|status] [--policy-file PATH]\n');
    process.exit(2);
  }
  if (policyFileFlag >= 0 && !policyFile) {
    process.stderr.write('sidecar-policy --policy-file requires a path\n');
    process.exit(2);
  }
  if (action !== 'status') {
    if (!policyFile) {
      process.stderr.write('sidecar-policy needs PLUGIN_DATA or --policy-file to remember the setting\n');
      process.exit(2);
    }
    writeSidecarPolicy(action === 'enable' ? 'enabled' : action === 'disable' ? 'disabled' : 'ask', {}, policyFile);
  }
  process.stdout.write(`${JSON.stringify(readSidecarPolicy(policyFile), null, 2)}\n`);
  process.exit(0);
}

function repairCommandForProject(projectRoot = process.cwd()) {
  return `codestory-cli ready --goal agent --repair --project ${JSON.stringify(projectRoot)} --format json --run-id ${sharedAgentRunId}`;
}

function resolvedRepairCommandForProject(resolved, projectRoot = process.cwd()) {
  return `${JSON.stringify(resolved.path)} ready --goal agent --repair --project ${JSON.stringify(projectRoot)} --format json --run-id ${sharedAgentRunId}`;
}

function optionValue(argv, name) {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : null;
}

function dirtyMarkerEnv(projectRoot = process.cwd()) {
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

function runtimeTruthStatus(plugin, repair, options = {}) {
  return {
    runtime_source: plugin.cli_source || 'unavailable',
    plugin_root: plugin.plugin_root || null,
    managed_cli_path: plugin.managed_binary_path || null,
    launcher_source: plugin.cli_source || 'unavailable',
    sidecar_policy: options.sidecarPolicy || 'unavailable',
    sidecar_status: {
      profile: 'agent',
      run_id: 'unavailable',
      mode: 'unavailable',
      degraded_reason: options.degradedReason || repair.repair_reason || null,
    },
    readiness_lanes: {
      local_graph: {
        status: repair.status || 'unavailable',
        refresh_state: options.localRefresh?.state || 'unavailable',
        blocks_local_surfaces: options.localRefresh?.blocks_local_surfaces ?? null,
      },
      local_default: {
        status: 'unavailable',
        profile: 'local',
        sidecar_mode: 'unavailable',
        degraded_reason: 'unavailable',
      },
      agent_packet_search: {
        status: 'unavailable',
        profile: 'agent',
        run_id: 'unavailable',
        sidecar_mode: 'unavailable',
        degraded_reason: options.degradedReason || repair.repair_reason || null,
      },
    },
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
  const get = options.get || https.get;
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

async function fetchReleaseFile(version, name, destination) {
  const startedAt = Date.now();
  if (process.env.CODESTORY_PLUGIN_RELEASE_DIR) {
    try {
      copyLocalReleaseFile(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name, destination);
    } catch (error) {
      throw new Error(releaseAssetFetchFailure(name, startedAt, 1, error));
    }
    return `file://${path.join(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name)}`;
  }
  const baseUrl = process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL ||
    `https://github.com/TheGreenCedar/CodeStory/releases/download/v${version}`;
  const url = `${baseUrl.replace(/\/$/u, '')}/${name}`;
  try {
    await downloadFile(url, destination);
  } catch (error) {
    throw new Error(releaseAssetFetchFailure(name, startedAt, releaseDownloadAttempts, error));
  }
  return url;
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

async function provisionManagedCli(dataDir, version) {
  if (!dataDir || !version || process.env.CODESTORY_PLUGIN_DISABLE_PROVISION === '1') return null;
  const target = assetTarget();
  const asset = archiveName(version, target);
  if (!target || !asset) throw new Error(`unsupported_release_target:${process.platform}-${process.arch}`);

  const versionDir = path.join(dataDir, 'codestory-cli', version);
  const binDir = path.join(versionDir, 'bin');
  const manifestPath = path.join(versionDir, 'manifest.json');
  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'codestory-plugin-cli-'));
  const sumsPath = path.join(tempRoot, 'SHA256SUMS.txt');
  const archivePath = path.join(tempRoot, asset);
  const extractDir = path.join(tempRoot, 'extract');
  try {
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

    fs.mkdirSync(binDir, { recursive: true });
    const destination = path.join(binDir, path.basename(extracted));
    fs.copyFileSync(extracted, destination);
    if (process.platform !== 'win32') fs.chmodSync(destination, 0o755);
    const binarySha256 = fileSha256(destination);
    fs.writeFileSync(manifestPath, JSON.stringify({
      path: path.relative(versionDir, destination).replace(/\\/gu, '/'),
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
    return resolveManifest(manifestPath);
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}

async function resolveManagedCli(dataDir, version, warnings) {
  if (!dataDir || !version) return null;
  for (const manifestPath of [
    path.join(dataDir, 'codestory-cli', version, 'manifest.json'),
    path.join(dataDir, 'codestory-cli', 'manifest.json'),
  ]) {
    const resolved = resolveManifest(manifestPath);
    if (resolved) return resolved;
  }

  for (const cliPath of [
    path.join(dataDir, 'codestory-cli', version, binaryName),
    path.join(dataDir, 'codestory-cli', version, 'bin', binaryName),
  ]) {
    if (fs.existsSync(cliPath)) {
      return { path: cliPath, sha256: fileSha256(cliPath), manifestPath: null };
    }
  }
  try {
    return await provisionManagedCli(dataDir, version);
  } catch (error) {
    warnings.push(`managed_cli_provision_failed:${error.message}`);
  }
  return null;
}

async function resolveCli() {
  const version = pluginVersion();
  const warnings = [];
  if (process.env.CODESTORY_CLI) {
    return {
      source: 'local_dev_override',
      path: process.env.CODESTORY_CLI,
      sha256: fs.existsSync(process.env.CODESTORY_CLI) ? fileSha256(process.env.CODESTORY_CLI) : null,
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
  const pathCandidates = pathCliCandidates();
  warnings.push(pathCandidates.length > 0
    ? 'managed_cli_unavailable_using_path_fallback'
    : 'managed_cli_unavailable_no_path_fallback');
  const pathFallback = pathCandidates[0] || 'codestory-cli';
  return {
    source: 'path_fallback',
    path: pathFallback,
    sha256: null,
    version,
    cliVersion: null,
    repoRef: null,
    buildSource: 'path_fallback',
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

function semverParts(version) {
  const normalized = normalizeVersion(version);
  if (!normalized) return null;
  return normalized.split('.').map((part) => Number.parseInt(part, 10));
}

function compareSemver(left, right) {
  const leftParts = semverParts(left);
  const rightParts = semverParts(right);
  if (!leftParts || !rightParts) return null;
  for (let index = 0; index < 3; index += 1) {
    if (leftParts[index] !== rightParts[index]) {
      return leftParts[index] < rightParts[index] ? -1 : 1;
    }
  }
  return 0;
}

function probeResolvedCli(resolved) {
  const result = spawnSync(resolved.path, ['--version'], {
    encoding: 'utf8',
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
  if (resolved.managedProvisionFailure && resolved.source === 'path_fallback' && pathCliCandidates().length === 0) {
    return resolved.managedProvisionFailure;
  }
  if (probe.error || probe.status !== 0) {
    return `${resolved.source}_cli_unspawnable`;
  }
  if (resolved.source !== 'path_fallback') return null;
  const comparison = compareSemver(probe.version, resolved.version);
  if (!probe.version || comparison === null) return 'path_fallback_cli_unavailable';
  if (comparison < 0) return 'path_fallback_cli_stale';
  return null;
}

function localWaitFreshCommand(projectRoot = process.cwd()) {
  return `codestory-cli ready --goal local --wait-fresh --project ${JSON.stringify(projectRoot)} --format json`;
}

function localWaitFreshTimeoutMs() {
  const parsed = Number.parseInt(process.env.CODESTORY_PLUGIN_LOCAL_REPAIR_TIMEOUT_MS || '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 120000;
}

function sidecarRepairTimeoutMs() {
  const parsed = Number.parseInt(process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR_TIMEOUT_MS || '', 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 120000;
}

function runLocalNavigationWaitFresh(resolved, projectRoot) {
  const args = ['ready', '--goal', 'local', '--wait-fresh'];
  args.push('--project', projectRoot, '--format', 'json');
  const result = spawnSync(resolved.path, args, {
    encoding: 'utf8',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    timeout: localWaitFreshTimeoutMs(),
    windowsHide: true,
  });
  return { args, result };
}

function runSidecarStartupRepair(resolved, sidecarStatus, projectRoot = process.cwd()) {
  if (sidecarStatus.state !== 'enabled') return sidecarStatus;
  if (resolved.source !== 'managed' && resolved.source !== 'local_dev_override') return sidecarStatus;
  const args = ['ready', '--goal', 'agent', '--repair'];
  args.push('--project', projectRoot, '--format', 'json', '--run-id', sharedAgentRunId);
  const result = spawnSync(resolved.path, args, {
    encoding: 'utf8',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    timeout: sidecarRepairTimeoutMs(),
    windowsHide: true,
    env: {
      ...process.env,
      CODESTORY_PLUGIN_SIDECAR_REPAIR: '1',
      CODESTORY_PLUGIN_SIDECAR_POLICY_STATE: sidecarStatus.state,
    },
  });
  const lastRepair = {
    state: result.error?.code === 'ETIMEDOUT' ? 'timeout' : result.status === 0 ? 'completed' : 'failed',
    updated_at: new Date().toISOString(),
    project_root: projectRoot,
    command: resolvedRepairCommandForProject(resolved, projectRoot),
  };
  if (sidecarStatus.path) {
    writeSidecarPolicy(sidecarStatus.state, { last_repair: lastRepair }, sidecarStatus.path);
    return readSidecarPolicy(sidecarStatus.path);
  }
  return {
    ...sidecarStatus,
    lastRepair,
  };
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

function pluginRuntimeForResolved(resolved) {
  return {
    plugin_version: resolved.version,
    plugin_root: pluginRoot,
    plugin_cache_version: pluginCacheVersion(),
    plugin_data: pluginDataDir(),
    cli_source: resolved.source,
    cli_path: resolved.path,
    cli_sha256: resolved.sha256,
    build_source: resolved.buildSource,
    repo_ref: resolved.repoRef,
    local_dev_override: resolved.source === 'local_dev_override',
    path_fallback: resolved.source === 'path_fallback',
    managed_binary_path: resolved.source === 'managed' ? resolved.path : null,
    managed_binary_sha256: resolved.source === 'managed' ? resolved.sha256 : null,
    managed_manifest_path: resolved.manifestPath || null,
    warnings: resolved.warnings.filter(Boolean),
  };
}

function fallbackDiagnostic(resolved, probe, reason, options = {}) {
  const projectRoot = options.projectRoot || process.cwd();
  const pathCandidates = pathCliCandidates().map((candidate) => ({
    path: candidate,
    version: cliVersion(candidate),
    active: samePathText(candidate, resolved.path),
  }));
  const managedProvisionFailed = String(reason || '').startsWith('managed_cli_provision_failed:');
  const managedProvisionNext = [
    'Restart/reload the Codex host/app and read codestory://status; managed CLI provisioning will retry release asset downloads.',
    'Refresh or reinstall the CodeStory plugin after GitHub release assets are reachable, then restart/reload the Codex host/app and read codestory://status.',
  ];
  const minimumNext = options.minimumNext || (managedProvisionFailed && pathCandidates.length === 0 ? managedProvisionNext : [
    'Refresh or reinstall the CodeStory plugin, then restart/reload the Codex host/app and read codestory://status in a fresh thread.',
    process.platform === 'win32' ? 'where.exe codestory-cli' : 'command -v codestory-cli',
    'codestory-cli --version',
  ]);
  const fullRepair = options.fullRepair || minimumNext;
  const recommendedNext = options.recommendedNext || fullRepair;
  const sidecarPolicy = readSidecarPolicy();
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
  const blockedSurface = (surface, goal) => ({
    allowed: false,
    readiness_goal: goal,
    status: repair.status,
    repair_reason: reason,
    minimum_next: minimumNext,
    full_repair: fullRepair,
  });
  const allowedSurfaces = Object.fromEntries([
    ...localSurfaces.map((surface) => [surface, blockedSurface(surface, 'local_navigation')]),
    ...sidecarSurfaces.map((surface) => [surface, blockedSurface(surface, 'agent_packet_search')]),
  ]);
  return {
    server_version: null,
    cli_version: probe.version,
    server_executable: null,
    server_executable_sha256: null,
    source_checkout_version: sourceCheckoutVersion(projectRoot),
    path_candidates: pathCandidates,
    sidecar_contract_version: null,
    plugin_runtime: plugin,
    runtime_truth: runtimeTruthStatus(plugin, repair, {
      sidecarPolicy: sidecarPolicy.state || 'unavailable',
      localRefresh: options.localRefresh || null,
      degradedReason: reason,
    }),
    runtime_boundary: {
      restart_required_for_runtime_change: true,
      message: 'A running MCP server keeps using the CLI process it was launched with; plugin refresh, managed runtime provisioning, CODESTORY_CLI, or PATH changes require a host reload/restart and fresh codestory://status readback.',
    },
    warnings: plugin.warnings,
    project_root: projectRoot,
    retrieval_mode: 'unavailable',
    degraded_reason: reason,
    local_refresh: options.localRefresh || null,
    readiness: [repair],
    allowed_surfaces: allowedSurfaces,
    recommended_next_calls: [
      { method: 'resources/read', uri: 'codestory://status' },
      ...recommendedNext.map((command) => command.startsWith('Refresh or reinstall') || command.startsWith('Restart/reload')
        ? { method: 'host/restart', instruction: command }
        : { method: 'cli', command }),
    ],
  };
}

async function bootstrapStatus(projectRoot = process.cwd()) {
  const resolved = await resolveCli();
  rememberLaunch(resolved);
  const probe = probeResolvedCli(resolved);
  const failOpenReason = failOpenReasonForProbe(resolved, probe);
  if (failOpenReason) {
    return {
      ready: false,
      ...fallbackDiagnostic(resolved, probe, failOpenReason, { projectRoot }),
    };
  }

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
    },
  };
  return {
    ready: true,
    project_root: projectRoot,
    server_version: resolved.version,
    cli_version: probe.version,
    plugin_runtime: plugin,
    runtime_truth: runtimeTruthStatus(plugin, repair, {
      sidecarPolicy: sidecarPolicy.state || 'unavailable',
      localRefresh,
    }),
    local_refresh: localRefresh,
    readiness: [repair],
    recommended_next_calls: [{ method: 'resources/read', uri: 'codestory://status' }],
  };
}

async function handleBootstrapStatusCommand(argv) {
  if (argv[2] !== 'bootstrap-status') return false;
  const projectRoot = optionValue(argv, '--project') || process.cwd();
  try {
    process.stdout.write(`${JSON.stringify(await bootstrapStatus(projectRoot))}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify({
      ready: false,
      degraded_reason: `launcher_error:${error.message}`,
      project_root: projectRoot,
    })}\n`);
  }
  process.exit(0);
}

function pathCliCandidates() {
  const pathValue = process.env.PATH || '';
  const candidates = [];
  for (const directory of pathValue.split(path.delimiter).filter(Boolean)) {
    for (const name of cliBinaryNames()) {
      const candidate = path.join(directory, name);
      if (fs.existsSync(candidate) && fs.statSync(candidate).isFile()) {
        candidates.push(candidate);
      }
    }
  }
  return dedupePaths(candidates);
}

function cliBinaryNames() {
  return process.platform === 'win32'
    ? ['codestory-cli.exe', 'codestory-cli.cmd', 'codestory-cli.bat', 'codestory-cli']
    : ['codestory-cli'];
}

function cliVersion(candidate) {
  const result = spawnSync(candidate, ['--version'], {
    encoding: 'utf8',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(candidate),
    timeout: 3000,
    windowsHide: true,
  });
  if (result.status !== 0 || result.error) return null;
  return normalizeVersion(`${result.stdout || ''}\n${result.stderr || ''}`);
}

function samePathText(left, right) {
  const normalize = (value) => String(value || '').replace(/[\\/]+$/u, '').toLowerCase();
  return normalize(left) === normalize(right);
}

function dedupePaths(paths) {
  const deduped = [];
  for (const candidate of paths) {
    if (!deduped.some((seen) => samePathText(seen, candidate))) {
      deduped.push(candidate);
    }
  }
  return deduped;
}

function sourceCheckoutVersion(projectRoot) {
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
  return [
    'CodeStory MCP runtime is not ready.',
    `reason: ${status.degraded_reason}`,
    `plugin_version: ${status.plugin_runtime.plugin_version || '<unknown>'}`,
    `plugin_root: ${status.plugin_runtime.plugin_root}`,
    `cli_source: ${status.plugin_runtime.cli_source}`,
    `cli_path: ${setup.active_path}`,
    `cli_version: ${setup.active_version || '<unknown>'}`,
    `source_checkout_version: ${status.source_checkout_version || '<none>'}`,
    `path_candidates: ${(status.path_candidates || []).map((candidate) => `${candidate.path}@${candidate.version || '<unknown>'}`).join(', ') || '<none>'}`,
    `next: ${status.readiness[0].minimum_next[0]}`,
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

function failOpenToolResult(status) {
  const readiness = status.readiness[0];
  return {
    isError: true,
    content: [{ type: 'text', text: diagnosticText(status) }],
    structuredContent: {
      code: 'codestory_mcp_runtime_unavailable',
      status: readiness.status,
      repair_reason: status.degraded_reason,
      local_refresh: status.local_refresh || null,
      plugin_runtime: status.plugin_runtime,
      setup: readiness.setup,
      minimum_next: readiness.minimum_next,
      full_repair: readiness.full_repair,
      recommended_next_calls: status.recommended_next_calls,
    },
  };
}

function runFailOpenMcp(status) {
  const tools = ['ground', 'files', 'packet', 'search', 'context'].map((name) => ({
    name,
    description: 'CodeStory diagnostic fail-open surface; runtime repair is required before this tool can return repository grounding.',
    inputSchema: { type: 'object', additionalProperties: true },
    outputSchema: { type: 'object', additionalProperties: true },
  }));
  const resources = [
    { uri: 'codestory://status', name: 'CodeStory runtime status', mimeType: 'application/json' },
    { uri: 'codestory://agent-guide', name: 'CodeStory agent guide', mimeType: 'application/json' },
  ];
  const guide = {
    status: 'repair_setup',
    message: 'Read codestory://status, follow recommended_next_calls, restart/reload the host, then retry grounding.',
    recommended_next_calls: status.recommended_next_calls,
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
      if (request.id === undefined) continue;
      let response;
      if (request.method === 'initialize') {
        response = jsonrpcResult(request.id, {
          protocolVersion: request.params?.protocolVersion || '2024-11-05',
          capabilities: { tools: {}, resources: {} },
          serverInfo: { name: 'codestory', version: resolvedVersionForStatus(status) },
        });
      } else if (request.method === 'tools/list') {
        response = jsonrpcResult(request.id, { tools });
      } else if (request.method === 'resources/list') {
        response = jsonrpcResult(request.id, { resources });
      } else if (request.method === 'resources/read') {
        const uri = request.params?.uri;
        if (uri === 'codestory://status') {
          response = jsonrpcResult(request.id, resourceContents(uri, status));
        } else if (uri === 'codestory://agent-guide') {
          response = jsonrpcResult(request.id, resourceContents(uri, guide));
        } else {
          response = jsonrpcError(request.id, -32602, `unknown resource: ${uri || '<missing>'}`);
        }
      } else if (request.method === 'tools/call') {
        response = jsonrpcResult(request.id, failOpenToolResult(status));
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

function rememberLaunch(resolved) {
  const dataDir = pluginDataDir();
  if (!dataDir) return;
  try {
    fs.mkdirSync(dataDir, { recursive: true });
    fs.writeFileSync(path.join(dataDir, '.codestory-mcp-runtime.json'), JSON.stringify({
      source: resolved.source,
      path: resolved.path,
      sha256: resolved.sha256,
      pluginRoot,
      pluginCacheVersion: pluginCacheVersion(),
      pluginVersion: resolved.version,
      manifestPath: resolved.manifestPath || null,
      cliVersion: resolved.cliVersion || null,
      repoRef: resolved.repoRef || null,
      buildSource: resolved.buildSource || null,
      archiveSha256: resolved.archiveSha256 || null,
      archiveUrl: resolved.archiveUrl || null,
      provisionedAt: resolved.provisionedAt || null,
      updatedAt: new Date().toISOString(),
    }, null, 2));
  } catch {
    // Best effort only. Launch metadata must not block MCP startup.
  }
}

async function main() {
  if (await handleBootstrapStatusCommand(process.argv)) return;
  handleSidecarPolicyCommand(process.argv);
  const resolved = await resolveCli();
  rememberLaunch(resolved);
  const probe = probeResolvedCli(resolved);
  const failOpenReason = failOpenReasonForProbe(resolved, probe);
  if (failOpenReason) {
    runFailOpenMcp(fallbackDiagnostic(resolved, probe, failOpenReason));
    return;
  }
  const localReadiness = probeLocalNavigation(resolved);
  if (!localReadiness.ready) {
    runFailOpenMcp(fallbackDiagnostic(resolved, probe, localReadiness.reason, {
      status: localReadiness.status,
      summary: localReadiness.summary,
      minimumNext: localReadiness.minimumNext,
      fullRepair: [
        ...localReadiness.fullRepair,
        'Restart/reload the Codex host/app after repairing the local CodeStory index, then read codestory://status in a fresh thread.',
      ],
      localRefresh: localReadiness.localRefresh,
      setup: localReadiness.setup,
    }));
    return;
  }
  const sidecarStatus = runSidecarStartupRepair(resolved, readSidecarPolicy(), process.cwd());
  const dirtyMarker = dirtyMarkerEnv(process.cwd());

  const child = spawn(resolved.path, ['serve', '--stdio', '--refresh', 'none'], {
    stdio: 'inherit',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    windowsHide: true,
    env: {
      ...process.env,
      CODESTORY_PLUGIN_VERSION: resolved.version || '',
      CODESTORY_PLUGIN_ROOT: pluginRoot,
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
      CODESTORY_PLUGIN_CLI_WARNINGS: resolved.warnings.join(';'),
      CODESTORY_PLUGIN_SIDECAR_POLICY_STATE: sidecarStatus.state,
      CODESTORY_PLUGIN_SIDECAR_POLICY_PATH: sidecarStatus.path || '',
      CODESTORY_PLUGIN_SIDECAR_POLICY_UPDATED_AT: sidecarStatus.updatedAt || '',
      CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND: sidecarPolicyCommand('enable'),
      CODESTORY_PLUGIN_SIDECAR_DISABLE_COMMAND: sidecarPolicyCommand('disable'),
      CODESTORY_PLUGIN_SIDECAR_NEXT_REPAIR_COMMAND: resolvedRepairCommandForProject(resolved, process.cwd()),
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_STATE: sidecarStatus.lastRepair?.state || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_AT: sidecarStatus.lastRepair?.updated_at || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_PROJECT: sidecarStatus.lastRepair?.project_root || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_COMMAND: sidecarStatus.lastRepair?.command || '',
      CODESTORY_PLUGIN_DIRTY_MARKER_PATH: dirtyMarker.path,
      CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT: dirtyMarker.projectRoot,
    },
  });

  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    process.exit(code || 0);
  });

  child.on('error', (error) => {
    runFailOpenMcp(fallbackDiagnostic(resolved, {
      status: null,
      error: error.message,
      version: null,
      stdout: '',
      stderr: '',
    }, `${resolved.source}_cli_unspawnable`));
  });
}

function runLauncherError(error) {
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
      downloadFile,
    },
  };
}
