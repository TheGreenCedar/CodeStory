#!/usr/bin/env node

const { spawn } = require('child_process');
const { createHash } = require('crypto');
const fs = require('fs');
const path = require('path');

const pluginRoot = path.dirname(__dirname);
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';

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

function pluginVersion() {
  const manifest = readJson(path.join(pluginRoot, '.codex-plugin', 'plugin.json'));
  return manifest && typeof manifest.version === 'string' ? manifest.version : null;
}

function pluginDataDir() {
  return process.env.PLUGIN_DATA || process.env.COPILOT_PLUGIN_DATA || null;
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
  return { path: cliPath, sha256, manifestPath };
}

function resolveManagedCli(dataDir, version) {
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
  return null;
}

function resolveCli() {
  const version = pluginVersion();
  const warnings = [];
  if (process.env.CODESTORY_CLI) {
    return {
      source: 'local_dev_override',
      path: process.env.CODESTORY_CLI,
      sha256: fs.existsSync(process.env.CODESTORY_CLI) ? fileSha256(process.env.CODESTORY_CLI) : null,
      version,
      warnings,
    };
  }

  const managed = resolveManagedCli(pluginDataDir(), version);
  if (managed && managed.warning) warnings.push(managed.warning);
  if (managed && managed.path) {
    return { source: 'managed', version, warnings, ...managed };
  }

  warnings.push('managed_cli_unavailable_using_path_fallback');
  return { source: 'path_fallback', path: 'codestory-cli', sha256: null, version, warnings };
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
      updatedAt: new Date().toISOString(),
    }, null, 2));
  } catch {
    // Best effort only. Launch metadata must not block MCP startup.
  }
}

const resolved = resolveCli();
rememberLaunch(resolved);

const child = spawn(resolved.path, ['serve', '--stdio', '--refresh', 'none'], {
  stdio: 'inherit',
  shell: process.platform === 'win32' && /\.(cmd|bat)$/i.test(resolved.path),
  windowsHide: true,
  env: {
    ...process.env,
    CODESTORY_PLUGIN_VERSION: resolved.version || '',
    CODESTORY_PLUGIN_CLI_SOURCE: resolved.source,
    CODESTORY_PLUGIN_CLI_PATH: resolved.path,
    CODESTORY_PLUGIN_CLI_SHA256: resolved.sha256 || '',
    CODESTORY_PLUGIN_CLI_MANIFEST_PATH: resolved.manifestPath || '',
    CODESTORY_PLUGIN_CLI_WARNINGS: resolved.warnings.join(';'),
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
