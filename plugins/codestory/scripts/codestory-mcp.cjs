#!/usr/bin/env node

const { spawn } = require('child_process');
const { spawnSync } = require('child_process');
const { createHash } = require('crypto');
const fs = require('fs');
const https = require('https');
const os = require('os');
const path = require('path');

const pluginRoot = path.dirname(__dirname);
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
const fallbackBinaryNames = process.platform === 'win32'
  ? ['codestory-cli.exe', 'codestory-cli.cmd', 'codestory-cli']
  : ['codestory-cli'];

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
  return `codestory-cli ready --goal agent --repair --project ${JSON.stringify(projectRoot)} --format json`;
}

function recordSidecarRepair(state, patch = {}) {
  const policy = readSidecarPolicy();
  if (policy.state !== 'enabled') return;
  writeSidecarPolicy('enabled', {
    last_repair: {
      ...(policy.lastRepair || {}),
      ...patch,
      state,
      updated_at: new Date().toISOString(),
    },
  });
}

function scheduleSidecarRepair(resolved, policy) {
  if (policy.state !== 'enabled') return;
  const projectRoot = process.cwd();
  const args = ['ready', '--goal', 'agent', '--repair', '--project', projectRoot, '--format', 'json'];
  recordSidecarRepair('scheduled', {
    project_root: projectRoot,
    command: repairCommandForProject(projectRoot),
    started_at: new Date().toISOString(),
  });
  try {
    const repair = spawn(resolved.path, args, {
      stdio: 'ignore',
      shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
      windowsHide: true,
      env: { ...process.env, CODESTORY_PLUGIN_SIDECAR_REPAIR: '1' },
    });
    repair.on('exit', (code, signal) => {
      recordSidecarRepair(code === 0 ? 'completed' : 'failed', {
        exit_code: code,
        signal: signal || null,
        finished_at: new Date().toISOString(),
      });
    });
    repair.on('error', (error) => {
      recordSidecarRepair('spawn_failed', {
        error: error.message,
        finished_at: new Date().toISOString(),
      });
    });
    repair.unref();
  } catch (error) {
    recordSidecarRepair('spawn_failed', {
      error: error.message,
      finished_at: new Date().toISOString(),
    });
  }
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

function downloadFile(url, destination) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        downloadFile(response.headers.location, destination).then(resolve, reject);
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
    request.setTimeout(15000, () => request.destroy(new Error(`download timed out: ${url}`)));
    request.on('error', reject);
  });
}

async function fetchReleaseFile(version, name, destination) {
  if (process.env.CODESTORY_PLUGIN_RELEASE_DIR) {
    copyLocalReleaseFile(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name, destination);
    return `file://${path.join(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name)}`;
  }
  const baseUrl = process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL ||
    `https://github.com/TheGreenCedar/CodeStory/releases/download/v${version}`;
  const url = `${baseUrl.replace(/\/$/u, '')}/${name}`;
  await downloadFile(url, destination);
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

  warnings.push('managed_cli_unavailable_using_path_fallback');
  return {
    source: 'path_fallback',
    path: 'codestory-cli',
    sha256: null,
    version,
    cliVersion: null,
    repoRef: null,
    buildSource: 'path_fallback',
    archiveSha256: null,
    archiveUrl: null,
    provisionedAt: null,
    warnings,
  };
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
  handleSidecarPolicyCommand(process.argv);
  const resolved = await resolveCli();
  rememberLaunch(resolved);
  const sidecarPolicy = readSidecarPolicy();
  scheduleSidecarRepair(resolved, sidecarPolicy);
  const sidecarStatus = readSidecarPolicy();

  const child = spawn(resolved.path, ['serve', '--stdio', '--refresh', 'none'], {
    stdio: 'inherit',
    shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
    windowsHide: true,
    env: {
      ...process.env,
      CODESTORY_PLUGIN_VERSION: resolved.version || '',
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
      CODESTORY_PLUGIN_SIDECAR_NEXT_REPAIR_COMMAND: repairCommandForProject(process.cwd()),
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_STATE: sidecarStatus.lastRepair?.state || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_AT: sidecarStatus.lastRepair?.updated_at || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_PROJECT: sidecarStatus.lastRepair?.project_root || '',
      CODESTORY_PLUGIN_SIDECAR_LAST_REPAIR_COMMAND: sidecarStatus.lastRepair?.command || '',
    },
  });

  child.on('exit', (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    process.exit(code || 0);
  });

  child.on('error', (error) => {
    process.stderr.write(`codestory mcp launch failed: ${error.message}\n`);
    process.exit(1);
  });
}

main().catch((error) => {
  process.stderr.write(`codestory mcp launch failed: ${error.message}\n`);
  process.exit(1);
});
