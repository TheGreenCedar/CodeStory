#!/usr/bin/env node

const { spawn } = require('child_process');
const { spawnSync } = require('child_process');
const { createHash, randomBytes } = require('crypto');
const fs = require('fs');
const http = require('http');
const https = require('https');
const os = require('os');
const path = require('path');
const { Transform, pipeline } = require('stream');
const { TextDecoder } = require('util');
const zlib = require('zlib');
const {
  sourceBuildTarget,
  validateDevCliReceipt,
} = require('./codestory-dev-cli-contract.cjs');

const pluginRoot = path.dirname(__dirname);
const launchCwd = process.cwd();
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
const releaseDownloadTimeoutMs = 60000;
const releaseDownloadAttempts = 3;
const releaseDownloadRetryDelaysMs = [1000, 3000];
const managedCliLockStaleMs = 10 * 60 * 1000;
const managedCliLockMaxAgeMs = 30 * 60 * 1000;
const releaseAssetRetryBudgetMs =
  releaseDownloadAttempts * releaseDownloadTimeoutMs +
  releaseDownloadRetryDelaysMs.slice(0, releaseDownloadAttempts - 1).reduce((sum, delay) => sum + delay, 0);
const managedCliStagingBudgetMs = 30 * 1000;
const managedCliLockWaitMs = 2 * releaseAssetRetryBudgetMs + managedCliStagingBudgetMs;
const managedCliPendingOwnerCleanupLimit = 64;
const managedCliQuarantineRetention = 2;
const managedCliArchiveMaxBytes = 256 * 1024 * 1024;
const managedCliChecksumMaxBytes = 1024 * 1024;
const managedCliArchiveMaxEntries = 20_000;
const managedCliArchiveMaxEntryBytes = 256 * 1024 * 1024;
const managedCliArchiveMaxOutputBytes = 512 * 1024 * 1024;
const managedCliProbeStdoutMaxBytes = 64 * 1024;
const managedCliProbeStderrMaxBytes = 4 * 1024;
const managedCliProbeTerminationGraceMs = 500;
const managedCliProbeForceKillGraceMs = 1000;
const managedCliMcpProtocolVersion = '2024-11-05';

function isWindowsBatchCli(cliPath, platform = process.platform) {
  return platform === 'win32' && /\.(?:cmd|bat)$/iu.test(String(cliPath || ''));
}

function requireDirectCli(cliPath, platform = process.platform) {
  if (isWindowsBatchCli(cliPath, platform)) {
    throw new Error('codestory_cli_batch_override_rejected:use_codestory_cli_exe');
  }
}

function spawnCodeStoryCli(cliPath, args, options = {}, spawnChild = spawn) {
  requireDirectCli(cliPath);
  return spawnChild(cliPath, args, { ...options, shell: false });
}

function spawnCodeStoryCliSync(cliPath, args, options = {}) {
  requireDirectCli(cliPath);
  return spawnSync(cliPath, args, { ...options, shell: false });
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

const canonicalMcpCatalog = readJson(path.join(pluginRoot, 'generated-mcp-catalog.json'));

function fileSha256(file) {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
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

function candidateQualificationArchiveSha256() {
  const archiveSha256 = process.env.CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256 || '';
  const qualificationDir = process.env.CODESTORY_EMBED_QUALIFICATION_DIR || '';
  const nonce = process.env.CODESTORY_EMBED_QUALIFICATION_NONCE || '';
  if (
    !/^[0-9a-f]{64}$/iu.test(archiveSha256) ||
    !path.isAbsolute(qualificationDir) ||
    !/^[0-9a-f]{64}$/iu.test(nonce)
  ) {
    return null;
  }
  try {
    const directoryStat = fs.lstatSync(qualificationDir);
    if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink()) return null;
    if (process.platform !== 'win32' && (directoryStat.mode & 0o077) !== 0) return null;
    if (fs.realpathSync(qualificationDir) !== path.resolve(qualificationDir)) return null;
    const markerPath = path.join(qualificationDir, 'candidate-managed-install.json');
    const markerStat = fs.lstatSync(markerPath);
    if (!markerStat.isFile() || markerStat.isSymbolicLink()) return null;
    const marker = readJson(markerPath);
    const nonceSha256 = createHash('sha256').update(nonce, 'utf8').digest('hex');
    if (
      marker?.schema_version !== 1 ||
      marker?.purpose !== 'codestory-candidate-managed-install' ||
      marker?.archive_sha256 !== archiveSha256 ||
      marker?.qualification_nonce_sha256 !== nonceSha256 ||
      Object.keys(marker).sort().join(',') !==
        'archive_sha256,purpose,qualification_nonce_sha256,schema_version'
    ) {
      return null;
    }
    return archiveSha256;
  } catch {
    return null;
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

function assetTarget(platform = process.platform, arch = process.arch) {
  if (platform === 'win32' && arch === 'x64') return 'windows-x64';
  if (platform === 'darwin' && arch === 'arm64') return 'macos-arm64';
  return null;
}

function archiveName(version, target = assetTarget()) {
  if (!target) return null;
  const extension = target.startsWith('windows-') ? 'zip' : 'tar.gz';
  return `codestory-cli-v${version}-${target}.${extension}`;
}

function releaseAssetIdentity(version, platform = process.platform, arch = process.arch) {
  const target = assetTarget(platform, arch);
  if (!target) throw new Error(`unsupported_release_target:${platform}-${arch}`);
  return { target, asset: archiveName(version, target) };
}

function managedAssetIdentity(version, options = {}) {
  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const explicitSource = options.explicitSource ?? explicitPackageSourceConfigured();
  if (explicitSource) {
    const target = sourceBuildTarget(platform, arch);
    if (!target) {
      throw new Error(`unsupported_package_target:${platform}-${arch}`);
    }
    return {
      target,
      asset: archiveName(version, target),
      buildSource: 'explicit_package',
    };
  }
  return {
    ...releaseAssetIdentity(version, platform, arch),
    buildSource: 'github_release',
  };
}

function explicitPackageSourceConfigured() {
  return Boolean(
    process.env.CODESTORY_PLUGIN_RELEASE_DIR ||
    process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL
  );
}

function expectedArchiveHash(sumsText, name) {
  for (const line of sumsText.split(/\r?\n/u)) {
    const match = line.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/u);
    if (match && match[2].trim() === name) return match[1].toLowerCase();
  }
  throw new Error(`SHA256SUMS.txt did not contain ${name}`);
}

function releaseFileMaxBytes(name) {
  return name === 'SHA256SUMS.txt' ? managedCliChecksumMaxBytes : managedCliArchiveMaxBytes;
}

function copyLocalReleaseFile(releaseDir, name, destination, maxBytes) {
  const source = path.join(releaseDir, name);
  try {
    if (fs.statSync(source).size > maxBytes) {
      throw new Error(`download_size_limit_exceeded:${name}`);
    }
    fs.copyFileSync(source, destination);
    if (fs.statSync(destination).size > maxBytes) {
      throw new Error(`download_size_limit_exceeded:${name}`);
    }
  } catch (error) {
    fs.rmSync(destination, { force: true });
    throw error;
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function downloadFileOnce(url, destination, options = {}) {
  const timeoutMs = options.timeoutMs || releaseDownloadTimeoutMs;
  const maxBytes = options.maxBytes ?? managedCliArchiveMaxBytes;
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    return Promise.reject(new Error(`download_size_limit_invalid:${maxBytes}`));
  }
  const deadlineMs = options.deadlineMs ?? Date.now() + timeoutMs;
  const redirectsRemaining = options.redirectsRemaining ?? 5;
  const parsedUrl = new URL(url);
  const loopbackHttp = parsedUrl.protocol === 'http:' &&
    ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(parsedUrl.hostname);
  if (!options.get && parsedUrl.protocol !== 'https:' && !loopbackHttp) {
    return Promise.reject(new Error('download transport must be HTTPS'));
  }
  const get = options.get || (loopbackHttp ? http.get : https.get);
  return new Promise((resolve, reject) => {
    let settled = false;
    let output = null;
    let activeRequest = null;
    let activeResponse = null;
    let limiter = null;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadlineTimer);
      if (error) {
        if (limiter) limiter.destroy();
        if (output) output.destroy();
        if (activeResponse) activeResponse.destroy();
        if (activeRequest) activeRequest.destroy();
        reject(error);
      } else {
        resolve();
      }
    };
    const remainingMs = Math.max(0, deadlineMs - Date.now());
    const deadlineTimer = setTimeout(
      () => finish(new Error(`download timed out after ${timeoutMs}ms total: ${url}`)),
      remainingMs,
    );
    const request = get(url, (response) => {
      activeResponse = response;
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        if (!response.headers.location || redirectsRemaining <= 0) {
          finish(new Error(`download redirect failed: ${url}`));
          return;
        }
        const nextUrl = new URL(response.headers.location, url).toString();
        downloadFileOnce(nextUrl, destination, {
          ...options,
          deadlineMs,
          redirectsRemaining: redirectsRemaining - 1,
        }).then(() => finish(null), finish);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        finish(new Error(`download failed ${response.statusCode}: ${url}`));
        return;
      }
      const announced = response.headers['content-length'];
      if (announced !== undefined) {
        const contentLength = Number(announced);
        if (!Number.isSafeInteger(contentLength) || contentLength < 0) {
          response.resume();
          finish(new Error(`download_content_length_invalid:${url}`));
          return;
        }
        if (contentLength > maxBytes) {
          response.resume();
          finish(new Error(`download_size_limit_exceeded:${url}`));
          return;
        }
      }
      let receivedBytes = 0;
      limiter = new Transform({
        transform(chunk, _encoding, callback) {
          receivedBytes += chunk.length;
          if (receivedBytes > maxBytes) {
            callback(new Error(`download_size_limit_exceeded:${url}`));
            return;
          }
          callback(null, chunk);
        },
      });
      output = fs.createWriteStream(destination);
      pipeline(response, limiter, output, (error) => finish(error || null));
    });
    activeRequest = request;
    request.on('error', finish);
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
  const maxBytes = releaseFileMaxBytes(name);
  if (process.env.CODESTORY_PLUGIN_RELEASE_DIR) {
    try {
      copyLocalReleaseFile(process.env.CODESTORY_PLUGIN_RELEASE_DIR, name, destination, maxBytes);
    } catch (error) {
      throw new Error(releaseAssetFetchFailure(name, startedAt, 1, error));
    }
    return redactedReleaseFileUrl(version, name);
  }
  const url = releaseFileUrl(version, name);
  try {
    await downloadFile(url, destination, { maxBytes });
  } catch (error) {
    throw new Error(releaseAssetFetchFailure(name, startedAt, releaseDownloadAttempts, error));
  }
  return redactedReleaseFileUrl(version, name);
}

function safeArchiveDestination(destination, entryName) {
  const normalized = String(entryName || '').replace(/\\/gu, '/');
  if (!normalized || normalized.startsWith('/') || /^[A-Za-z]:\//u.test(normalized)) {
    throw new Error(`archive_path_invalid:${entryName}`);
  }
  const parts = normalized.split('/').filter(Boolean);
  if (parts.some((part) => part === '..')) throw new Error(`archive_path_escape:${entryName}`);
  const resolved = path.resolve(destination, ...parts);
  const root = `${path.resolve(destination)}${path.sep}`;
  if (resolved !== path.resolve(destination) && !resolved.startsWith(root)) {
    throw new Error(`archive_path_escape:${entryName}`);
  }
  return resolved;
}

function crc32(content) {
  let crc = 0xffffffff;
  for (const byte of content) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function tarText(block, start, length) {
  return block.subarray(start, start + length).toString('utf8').replace(/\0.*$/su, '').trim();
}

function tarNumber(block, start, length) {
  const text = tarText(block, start, length);
  if (!text || !/^[0-7]+$/u.test(text)) throw new Error('tar_numeric_field_invalid');
  const value = Number.parseInt(text, 8);
  if (!Number.isSafeInteger(value) || value < 0) throw new Error('tar_numeric_field_invalid');
  return value;
}

function paxPath(payload) {
  let offset = 0;
  let selected = null;
  while (offset < payload.length) {
    const space = payload.indexOf(0x20, offset);
    if (space < 0) throw new Error('tar_pax_length_missing');
    const lengthText = payload.subarray(offset, space).toString('ascii');
    if (!/^\d+$/u.test(lengthText)) throw new Error('tar_pax_length_invalid');
    const length = Number.parseInt(lengthText, 10);
    if (!Number.isSafeInteger(length) || length <= 0 || offset + length > payload.length) {
      throw new Error('tar_pax_length_invalid');
    }
    if (payload[offset + length - 1] !== 0x0a) throw new Error('tar_pax_record_unterminated');
    const record = payload.subarray(space + 1, offset + length - 1).toString('utf8');
    const separator = record.indexOf('=');
    if (separator <= 0) throw new Error('tar_pax_record_invalid');
    if (record.slice(0, separator) === 'path') {
      selected = record.slice(separator + 1);
      if (!selected || selected.includes('\0')) throw new Error('tar_pax_path_invalid');
    }
    offset += length;
  }
  if (offset !== payload.length) throw new Error('tar_pax_trailing_bytes');
  return selected;
}

function extractTarGz(archivePath, destination) {
  const archive = zlib.gunzipSync(fs.readFileSync(archivePath), {
    maxOutputLength: managedCliArchiveMaxOutputBytes,
  });
  let offset = 0;
  let nextPath = null;
  let entries = 0;
  let outputBytes = 0;
  let terminated = false;
  while (offset + 512 <= archive.length) {
    const header = archive.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      if (
        offset + 1024 > archive.length ||
        !archive.subarray(offset + 512, offset + 1024).every((byte) => byte === 0)
      ) {
        throw new Error('tar_terminator_invalid');
      }
      if (!archive.subarray(offset + 1024).every((byte) => byte === 0)) {
        throw new Error('tar_trailing_bytes');
      }
      terminated = true;
      break;
    }
    entries += 1;
    if (entries > managedCliArchiveMaxEntries) throw new Error('archive_entry_limit_exceeded');
    const storedChecksum = tarNumber(header, 148, 8);
    let checksum = 0;
    for (let index = 0; index < 512; index += 1) {
      checksum += index >= 148 && index < 156 ? 0x20 : header[index];
    }
    if (checksum !== storedChecksum) throw new Error('tar_header_checksum_mismatch');
    const size = tarNumber(header, 124, 12);
    if (size > managedCliArchiveMaxEntryBytes) throw new Error('archive_entry_size_limit_exceeded');
    const type = String.fromCharCode(header[156] || 0);
    const prefix = tarText(header, 345, 155);
    const headerName = tarText(header, 0, 100);
    const name = nextPath || (prefix ? `${prefix}/${headerName}` : headerName);
    nextPath = null;
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > archive.length) throw new Error('tar_entry_truncated');
    const payload = archive.subarray(dataStart, dataEnd);
    if (type === 'x') nextPath = paxPath(payload);
    else if (type === 'L') {
      const terminator = payload.indexOf(0);
      if (terminator < 1 || !payload.subarray(terminator).every((byte) => byte === 0)) {
        throw new Error('tar_long_name_unterminated');
      }
      nextPath = payload.subarray(0, terminator).toString('utf8');
    }
    else if (type === '5') fs.mkdirSync(safeArchiveDestination(destination, name), { recursive: true });
    else if (type === '\0' || type === '0') {
      if (!name) throw new Error('tar_entry_name_missing');
      outputBytes += size;
      if (outputBytes > managedCliArchiveMaxOutputBytes) throw new Error('archive_output_limit_exceeded');
      const output = safeArchiveDestination(destination, name);
      fs.mkdirSync(path.dirname(output), { recursive: true });
      fs.writeFileSync(output, payload, { mode: tarNumber(header, 100, 8) || 0o644 });
    } else if (type === 'g') {
      paxPath(payload);
    } else {
      throw new Error(`tar_entry_type_unsupported:${type.charCodeAt(0)}`);
    }
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  if (!terminated || nextPath) throw new Error(nextPath ? 'tar_extended_name_without_entry' : 'tar_terminator_missing');
}

function findZipEndOfCentralDirectory(archive) {
  const minimum = Math.max(0, archive.length - 65557);
  for (let offset = archive.length - 22; offset >= minimum; offset -= 1) {
    if (
      archive.readUInt32LE(offset) === 0x06054b50 &&
      offset + 22 + archive.readUInt16LE(offset + 20) === archive.length
    ) return offset;
  }
  throw new Error('zip_end_of_central_directory_missing');
}

function extractZip(archivePath, destination) {
  const archive = fs.readFileSync(archivePath);
  const eocd = findZipEndOfCentralDirectory(archive);
  if (
    archive.readUInt16LE(eocd + 4) !== 0 || archive.readUInt16LE(eocd + 6) !== 0 ||
    archive.readUInt16LE(eocd + 8) !== archive.readUInt16LE(eocd + 10)
  ) throw new Error('zip_multi_disk_unsupported');
  const entries = archive.readUInt16LE(eocd + 10);
  if (entries === 0xffff || entries > managedCliArchiveMaxEntries) {
    throw new Error('archive_entry_limit_exceeded');
  }
  const centralSize = archive.readUInt32LE(eocd + 12);
  const centralOffset = archive.readUInt32LE(eocd + 16);
  if (
    centralSize === 0xffffffff || centralOffset === 0xffffffff ||
    centralOffset + centralSize !== eocd
  ) throw new Error('zip_central_directory_bounds_invalid');
  let offset = centralOffset;
  let outputBytes = 0;
  const extractedPaths = new Set();
  for (let index = 0; index < entries; index += 1) {
    if (offset + 46 > eocd) throw new Error('zip_central_directory_truncated');
    if (archive.readUInt32LE(offset) !== 0x02014b50) throw new Error('zip_central_directory_invalid');
    const flags = archive.readUInt16LE(offset + 8);
    const method = archive.readUInt16LE(offset + 10);
    const compressedSize = archive.readUInt32LE(offset + 20);
    const uncompressedSize = archive.readUInt32LE(offset + 24);
    const nameLength = archive.readUInt16LE(offset + 28);
    const extraLength = archive.readUInt16LE(offset + 30);
    const commentLength = archive.readUInt16LE(offset + 32);
    const externalAttributes = archive.readUInt32LE(offset + 38);
    const localOffset = archive.readUInt32LE(offset + 42);
    const centralEnd = offset + 46 + nameLength + extraLength + commentLength;
    if (centralEnd > eocd) throw new Error('zip_central_entry_bounds_invalid');
    if (
      compressedSize === 0xffffffff || uncompressedSize === 0xffffffff ||
      localOffset === 0xffffffff || uncompressedSize > managedCliArchiveMaxEntryBytes
    ) throw new Error('archive_entry_size_limit_exceeded');
    const nameBytes = archive.subarray(offset + 46, offset + 46 + nameLength);
    const name = nameBytes.toString('utf8');
    if (!name || name.includes('\0') || name.includes('\ufffd')) throw new Error('zip_entry_name_invalid');
    if ((flags & 0x1) !== 0) throw new Error('zip_encryption_unsupported');
    if (((externalAttributes >>> 16) & 0o170000) === 0o120000) throw new Error('zip_symlink_unsupported');
    if (localOffset + 30 > centralOffset || archive.readUInt32LE(localOffset) !== 0x04034b50) {
      throw new Error('zip_local_header_invalid');
    }
    const localNameLength = archive.readUInt16LE(localOffset + 26);
    const localExtraLength = archive.readUInt16LE(localOffset + 28);
    const localName = archive.subarray(localOffset + 30, localOffset + 30 + localNameLength);
    if (!localName.equals(nameBytes)) throw new Error('zip_local_name_mismatch');
    const localFlags = archive.readUInt16LE(localOffset + 6);
    const localMethod = archive.readUInt16LE(localOffset + 8);
    const localCrc = archive.readUInt32LE(localOffset + 14);
    const localCompressedSize = archive.readUInt32LE(localOffset + 18);
    const localUncompressedSize = archive.readUInt32LE(localOffset + 22);
    if (localFlags !== flags || localMethod !== method) {
      throw new Error('zip_local_metadata_mismatch');
    }
    const usesDataDescriptor = (flags & 0x8) !== 0;
    if (usesDataDescriptor) {
      for (const [local, central] of [
        [localCrc, archive.readUInt32LE(offset + 16)],
        [localCompressedSize, compressedSize],
        [localUncompressedSize, uncompressedSize],
      ]) {
        if (local !== 0 && local !== central) throw new Error('zip_local_metadata_mismatch');
      }
    } else if (
      localCrc !== archive.readUInt32LE(offset + 16) ||
      localCompressedSize !== compressedSize || localUncompressedSize !== uncompressedSize
    ) {
      throw new Error('zip_local_metadata_mismatch');
    }
    const dataStart = localOffset + 30 + localNameLength + localExtraLength;
    if (dataStart + compressedSize > centralOffset) throw new Error('zip_entry_bounds_invalid');
    const compressed = archive.subarray(dataStart, dataStart + compressedSize);
    if (compressed.length !== compressedSize) throw new Error('zip_entry_truncated');
    if (usesDataDescriptor) {
      let descriptorOffset = dataStart + compressedSize;
      if (descriptorOffset + 4 <= centralOffset && archive.readUInt32LE(descriptorOffset) === 0x08074b50) {
        descriptorOffset += 4;
      }
      if (descriptorOffset + 12 > centralOffset) throw new Error('zip_data_descriptor_missing');
      if (
        archive.readUInt32LE(descriptorOffset) !== archive.readUInt32LE(offset + 16) ||
        archive.readUInt32LE(descriptorOffset + 4) !== compressedSize ||
        archive.readUInt32LE(descriptorOffset + 8) !== uncompressedSize
      ) throw new Error('zip_data_descriptor_mismatch');
    }
    const output = safeArchiveDestination(destination, name);
    if (extractedPaths.has(output)) throw new Error('archive_duplicate_path');
    extractedPaths.add(output);
    if (name.endsWith('/')) {
      fs.mkdirSync(output, { recursive: true });
    } else {
      outputBytes += uncompressedSize;
      if (outputBytes > managedCliArchiveMaxOutputBytes) throw new Error('archive_output_limit_exceeded');
      const content = method === 0 ? compressed : method === 8 ? zlib.inflateRawSync(compressed, {
        maxOutputLength: Math.min(managedCliArchiveMaxEntryBytes, uncompressedSize + 1),
      }) : null;
      if (!content) throw new Error(`zip_compression_unsupported:${method}`);
      if (content.length !== uncompressedSize) throw new Error('zip_entry_size_mismatch');
      if (crc32(content) !== archive.readUInt32LE(offset + 16)) throw new Error('zip_entry_crc_mismatch');
      fs.mkdirSync(path.dirname(output), { recursive: true });
      fs.writeFileSync(output, content, { mode: ((externalAttributes >>> 16) & 0o777) || 0o644 });
    }
    offset = centralEnd;
  }
  if (offset !== eocd) throw new Error('zip_central_directory_entry_count_mismatch');
}

function extractArchive(archivePath, destination) {
  const archiveBytes = fs.statSync(archivePath).size;
  if (archiveBytes > managedCliArchiveMaxBytes) throw new Error('archive_input_limit_exceeded');
  fs.mkdirSync(destination, { recursive: true });
  if (archivePath.endsWith('.zip')) extractZip(archivePath, destination);
  else if (archivePath.endsWith('.tar.gz')) extractTarGz(archivePath, destination);
  else throw new Error(`archive_format_unsupported:${path.basename(archivePath)}`);
}

function copyDirectTree(source, destination) {
  const metadata = fs.lstatSync(source);
  if (metadata.isSymbolicLink()) throw new Error(`managed_cli_package_link:${source}`);
  if (metadata.isDirectory()) {
    fs.mkdirSync(destination, { recursive: true });
    for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
      copyDirectTree(path.join(source, entry.name), path.join(destination, entry.name));
    }
    return;
  }
  if (!metadata.isFile()) throw new Error(`managed_cli_package_non_file:${source}`);
  fs.copyFileSync(source, destination, fs.constants.COPYFILE_EXCL);
  if (process.platform !== 'win32') fs.chmodSync(destination, metadata.mode & 0o777);
}

function stageExtractedManagedCli(extractDir, asset, stagingDir) {
  const archiveBase = asset.replace(/\.(?:zip|tar\.gz)$/u, '');
  if (!archiveBase || archiveBase === asset) {
    throw new Error(`managed_cli_archive_name_invalid:${asset}`);
  }
  const entries = fs.readdirSync(extractDir);
  if (entries.length !== 1 || entries[0] !== archiveBase) {
    throw new Error('managed_cli_archive_root_invalid');
  }
  const packageRoot = path.join(extractDir, archiveBase);
  const rootMetadata = fs.lstatSync(packageRoot);
  if (rootMetadata.isSymbolicLink() || !rootMetadata.isDirectory()) {
    throw new Error('managed_cli_archive_root_invalid');
  }
  for (const reserved of ['manifest.json', '.provisioning']) {
    if (fs.existsSync(path.join(packageRoot, reserved))) {
      throw new Error(`managed_cli_archive_reserved_path:${reserved}`);
    }
  }
  const launcher = path.join(packageRoot, binaryName);
  if (!fs.existsSync(launcher)) throw new Error(`archive_missing_cli:${asset}`);
  const launcherMetadata = fs.lstatSync(launcher);
  if (launcherMetadata.isSymbolicLink() || !launcherMetadata.isFile()) {
    throw new Error(`archive_missing_cli:${asset}`);
  }
  copyDirectTree(packageRoot, stagingDir);
  return path.join(stagingDir, binaryName);
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
        env: { ...process.env, LC_ALL: 'C' },
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
  return removeManagedCliInitializationIf(initializationPath, (owner, metadata) => {
    const stale = managedCliLockOwnerIsStale(owner, checkProcessIdentity);
    return stale === null ? Date.now() - metadata.mtimeMs > managedCliLockStaleMs : stale;
  });
}

function sameFileIdentity(left, right) {
  return left.isFile() && right.isFile() && !left.isSymbolicLink() && !right.isSymbolicLink() &&
    left.dev === right.dev && left.ino === right.ino && left.size === right.size &&
    left.mtimeMs === right.mtimeMs;
}

function restoreMovedInitialization(initializationPath, movedPath) {
  try {
    fs.renameSync(movedPath, initializationPath);
  } catch {
    // A new contender already owns the canonical alias. Every initializing owner retains its
    // private hard-linked pending-owner claim, so dropping only this moved alias cannot delete it.
    try {
      fs.unlinkSync(movedPath);
    } catch {
      // Best effort; the unique artifact is never mistaken for the canonical initialization path.
    }
  }
}

function removeManagedCliInitializationIf(initializationPath, shouldRemove, options = {}) {
  let descriptor;
  const movedPath = `${initializationPath}.reclaim-${process.pid}-${randomBytes(8).toString('hex')}`;
  try {
    const before = fs.lstatSync(initializationPath);
    if (!before.isFile() || before.isSymbolicLink()) return false;
    descriptor = fs.openSync(
      initializationPath,
      fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0),
    );
    const opened = fs.fstatSync(descriptor);
    if (!sameFileIdentity(before, opened)) return false;
    let owner = null;
    try {
      owner = JSON.parse(fs.readFileSync(descriptor, 'utf8'));
    } catch {
      // Malformed artifacts are removable only through the caller's age fallback.
    }
    if (!shouldRemove(owner, opened)) return false;
    const current = fs.lstatSync(initializationPath);
    if (!sameFileIdentity(opened, current)) return false;
    fs.renameSync(initializationPath, movedPath);
    if (options.afterRename) options.afterRename({ initializationPath, movedPath });
    const moved = fs.lstatSync(movedPath);
    const movedOwner = readJson(movedPath);
    if (
      !sameFileIdentity(opened, moved) ||
      movedOwner?.pid !== owner?.pid || movedOwner?.token !== owner?.token
    ) {
      restoreMovedInitialization(initializationPath, movedPath);
      return false;
    }
    fs.unlinkSync(movedPath);
    return true;
  } catch {
    if (fs.existsSync(movedPath)) restoreMovedInitialization(initializationPath, movedPath);
    return false;
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
  }
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
  removeManagedCliInitializationIf(
    initializationPath,
    (current) => current?.pid === owner.pid && current?.token === owner.token,
  );
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

async function acquireManagedCliLockAsync(root, purpose, waitMs) {
  const deadline = Date.now() + waitMs;
  let waited = false;
  while (true) {
    const lock = acquireManagedCliLock(root, purpose, 0);
    if (lock) return { ...lock, waited: waited || lock.waited };
    waited = true;
    const remaining = deadline - Date.now();
    if (remaining <= 0) return null;
    await sleep(Math.min(50, remaining));
  }
}

function managedCliFailureCode(error) {
  return String(error?.message || error || 'unknown_failure').match(/^[a-z0-9_]+/iu)?.[0] || 'unknown_failure';
}

function verifyPublishedManagedCli(
  versionDir,
  version,
  expectedTarget,
  probeVersion = probeResolvedCli,
) {
  const target = expectedTarget || releaseAssetIdentity(version).target;
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
  const expectedAsset = archiveName(version, target);
  const candidateArchiveSha256 = candidateQualificationArchiveSha256() || '';
  const publicReleaseMetadataValid =
    !explicitPackageSourceConfigured() &&
    manifest.build_source === 'github_release' &&
    manifest.repo_ref === `v${version}` &&
    manifest.archive_url === redactedReleaseFileUrl(version, expectedAsset);
  const explicitPackageMetadataValid =
    explicitPackageSourceConfigured() &&
    manifest.build_source === 'explicit_package' &&
    manifest.repo_ref === null &&
    manifest.archive_url === `explicit-package:${manifest.archive_sha256}`;
  const candidateMetadataValid =
    /^[0-9a-f]{64}$/iu.test(candidateArchiveSha256) &&
    manifest.build_source === 'candidate_archive' &&
    /^[0-9a-f]{40}$/iu.test(String(manifest.repo_ref || '')) &&
    manifest.archive_sha256 === candidateArchiveSha256 &&
    manifest.archive_url === `candidate-archive:${candidateArchiveSha256}`;
  if (
    (!publicReleaseMetadataValid && !explicitPackageMetadataValid && !candidateMetadataValid) ||
    manifest.archive !== expectedAsset ||
    manifest.target !== expectedTarget ||
    manifest.stdio_initialize_verified !== true ||
    !/^[0-9a-f]{64}$/iu.test(String(manifest.archive_sha256 || ''))
  ) {
    return { verified: false, reason: 'manifest_release_metadata_invalid' };
  }
  const resolved = resolveManifest(path.join(versionDir, 'manifest.json'));
  if (!resolved?.path) return { verified: false, reason: 'manifest_resolution_failed' };
  return { verified: true, reason: null, resolved: { ...resolved, cliVersion: version } };
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function probeManagedCliStdio(cliPath, timeoutMs = 5000, options = {}) {
  return new Promise((resolve, reject) => {
    const spawnChild = options.spawn || spawn;
    const child = spawnCodeStoryCli(cliPath, ['serve', '--stdio', '--multi-project', '--refresh', 'none'], {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      env: { ...process.env, CODESTORY_PLUGIN_PROVISIONING_PROBE: '1' },
    }, spawnChild);
    let completed = false;
    let requestedOutcome = null;
    let stdout = '';
    let stderr = '';
    let forceTimer = null;
    let terminationTimer = null;
    const finish = (error) => {
      if (completed) return;
      completed = true;
      clearTimeout(probeTimer);
      if (forceTimer) clearTimeout(forceTimer);
      if (terminationTimer) clearTimeout(terminationTimer);
      if (error) reject(error); else resolve();
    };
    const terminate = (error) => {
      if (requestedOutcome) return;
      requestedOutcome = { error };
      clearTimeout(probeTimer);
      try {
        child.kill('SIGTERM');
      } catch (killError) {
        finish(new Error(`managed_cli_stdio_initialize_terminate:${killError.message}`));
        return;
      }
      forceTimer = setTimeout(() => {
        try {
          child.kill('SIGKILL');
        } catch (killError) {
          finish(new Error(`managed_cli_stdio_initialize_force_kill:${killError.message}`));
        }
      },
        options.terminationGraceMs ?? managedCliProbeTerminationGraceMs);
      terminationTimer = setTimeout(
        () => finish(new Error('managed_cli_stdio_initialize_termination_timeout')),
        (options.terminationGraceMs ?? managedCliProbeTerminationGraceMs) +
          (options.forceKillGraceMs ?? managedCliProbeForceKillGraceMs),
      );
    };
    const probeTimer = setTimeout(
      () => terminate(new Error(`managed_cli_stdio_initialize_timeout:${timeoutMs}`)),
      timeoutMs,
    );
    child.stderr.on('data', (chunk) => {
      const remaining = managedCliProbeStderrMaxBytes - Buffer.byteLength(stderr, 'utf8');
      if (remaining > 0) {
        stderr += Buffer.from(chunk).subarray(0, remaining).toString('utf8');
      }
    });
    child.stderr.on('error', (error) => terminate(new Error(`managed_cli_stdio_initialize_stderr:${error.message}`)));
    child.stdout.on('data', (chunk) => {
      const bytes = Buffer.from(chunk);
      if (Buffer.byteLength(stdout, 'utf8') + bytes.length > managedCliProbeStdoutMaxBytes) {
        terminate(new Error('managed_cli_stdio_initialize_stdout_limit'));
        return;
      }
      stdout += bytes.toString('utf8');
      const newline = stdout.indexOf('\n');
      if (newline < 0) return;
      let response;
      try {
        response = JSON.parse(stdout.slice(0, newline).trim());
      } catch (error) {
        terminate(new Error(`managed_cli_stdio_initialize_invalid_json:${error.message}`));
        return;
      }
      if (
        response?.jsonrpc !== '2.0' || response?.id !== 'managed-cli-staging' ||
        !isPlainObject(response.result) ||
        response.result.protocolVersion !== managedCliMcpProtocolVersion ||
        !isPlainObject(response.result.capabilities) ||
        !isPlainObject(response.result.serverInfo) ||
        typeof response.result.serverInfo.name !== 'string' || !response.result.serverInfo.name.trim() ||
        typeof response.result.serverInfo.version !== 'string' || !response.result.serverInfo.version.trim()
      ) {
        terminate(new Error('managed_cli_stdio_initialize_incompatible'));
        return;
      }
      terminate(null);
    });
    child.stdout.on('error', (error) => terminate(new Error(`managed_cli_stdio_initialize_stdout:${error.message}`)));
    child.stdin.on('error', (error) => terminate(new Error(`managed_cli_stdio_initialize_stdin:${error.message}`)));
    child.on('error', (error) => finish(new Error(`managed_cli_stdio_initialize_spawn:${error.message}`)));
    child.on('exit', (code, signal) => {
      if (requestedOutcome) {
        finish(requestedOutcome.error);
      } else {
        finish(new Error(
          `managed_cli_stdio_initialize_exit:code=${code}:signal=${signal || 'none'}:stderr=${stderr}`,
        ));
      }
    });
    try {
      child.stdin.end(`${JSON.stringify({
        jsonrpc: '2.0',
        id: 'managed-cli-staging',
        method: 'initialize',
        params: {
          protocolVersion: managedCliMcpProtocolVersion,
          capabilities: {},
          clientInfo: { name: 'codestory-managed-cli-staging', version: '1' },
        },
      })}\n`);
    } catch (error) {
      terminate(new Error(`managed_cli_stdio_initialize_stdin:${error.message}`));
    }
  });
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
  const { target, asset, buildSource } = managedAssetIdentity(version);

  const root = managedCliRoot(dataDir, true);
  const versionDir = path.join(root, version);
  const lock = await acquireManagedCliLockAsync(root, `provision:${version}`, managedCliLockWaitMs);
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

    stagingDir = fs.mkdtempSync(path.join(root, `.provisioning-${version}-${process.pid}-`));
    const manifestPath = path.join(stagingDir, 'manifest.json');
    const destination = stageExtractedManagedCli(extractDir, asset, stagingDir);
    const binarySha256 = fileSha256(destination);
    const manifest = {
      path: path.relative(stagingDir, destination).replace(/\\/gu, '/'),
      sha256: binarySha256,
      version,
      build_source: buildSource,
      repo_ref: buildSource === 'github_release' ? `v${version}` : null,
      archive: asset,
      archive_url: buildSource === 'github_release'
        ? archiveUrl
        : `explicit-package:${actual}`,
      archive_sha256: actual,
      target,
      provisioned_at: new Date().toISOString(),
      stdio_initialize_verified: true,
    };
    const versionProbe = probeResolvedCli({ path: destination, provisioningProbe: true });
    if (versionProbe.error || versionProbe.status !== 0 || versionProbe.version !== version) {
      throw new Error('managed_cli_staging_verification_failed:version_probe_failed');
    }
    await probeManagedCliStdio(destination);
    fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
    const staged = verifyPublishedManagedCli(
      stagingDir,
      version,
      target,
      () => versionProbe,
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

async function resolveManagedCli(dataDir, version, warnings, options = {}) {
  if (!dataDir || !version) return null;
  let target;
  try {
    target = managedAssetIdentity(version).target;
  } catch (error) {
    warnings.push(`managed_cli_unsupported_target:${managedCliFailureCode(error)}`);
    return null;
  }
  try {
    managedCliRoot(dataDir);
  } catch (error) {
    warnings.push(`managed_cli_root_invalid:${error.message}`);
    return null;
  }
  const versionDir = path.join(dataDir, 'codestory-cli', version);
  if (fs.existsSync(versionDir)) {
    const existing = verifyPublishedManagedCli(versionDir, version, target);
    if (existing.verified) return existing.resolved;
  }
  if (options.provision === false) return null;
  try {
    return await provisionManagedCli(dataDir, version, warnings);
  } catch (error) {
    const code = managedCliFailureCode(error);
    warnings.push(`managed_cli_publication:terminal_failure:${code}`);
    warnings.push(`managed_cli_provision_failed:${code}`);
  }
  return null;
}

async function resolveCli(options = {}) {
  const version = pluginVersion();
  const warnings = [];
  const devReceipt = validateDevCliReceipt(pluginRoot, {
    expectedPluginVersion: version,
  });
  if (process.env.CODESTORY_CLI && devReceipt.state !== 'absent') {
    const reason = 'codestory_dev_cli_ambiguous_override';
    warnings.push(reason);
    return {
      source: 'local_dev_receipt_invalid',
      path: null,
      sha256: null,
      version,
      cliVersion: null,
      repoRef: null,
      buildSource: 'local_dev_receipt_invalid',
      sourcePackageSha256: null,
      archiveSha256: null,
      archiveUrl: null,
      provisionedAt: null,
      localDevReceiptFailure: reason,
      warnings,
    };
  }
  if (devReceipt.state === 'verified') {
    warnings.push('codestory_dev_receipt:verified');
    return {
      source: 'local_dev_override',
      path: devReceipt.path,
      sha256: devReceipt.sha256,
      version,
      cliVersion: devReceipt.cliVersion,
      repoRef: devReceipt.sourceCommit,
      buildSource: 'codestory_dev_receipt',
      sourcePackageSha256: devReceipt.sourcePackageSha256,
      archiveSha256: null,
      archiveUrl: null,
      provisionedAt: null,
      manifestPath: devReceipt.receiptPath,
      warnings,
    };
  }
  if (devReceipt.state === 'invalid') {
    const reason = `codestory_dev_receipt_invalid:${devReceipt.reason}`;
    warnings.push(reason);
    return {
      source: 'local_dev_receipt_invalid',
      path: null,
      sha256: null,
      version,
      cliVersion: null,
      repoRef: null,
      buildSource: 'local_dev_receipt_invalid',
      sourcePackageSha256: null,
      archiveSha256: null,
      archiveUrl: null,
      provisionedAt: null,
      manifestPath: devReceipt.receiptPath,
      localDevReceiptFailure: reason,
      warnings,
    };
  }
  if (process.env.CODESTORY_CLI) {
    const cliPath = path.isAbsolute(process.env.CODESTORY_CLI)
      ? process.env.CODESTORY_CLI
      : path.resolve(launchCwd, process.env.CODESTORY_CLI);
    const batchOverride = isWindowsBatchCli(cliPath);
    if (batchOverride) {
      warnings.push('codestory_cli_batch_override_rejected:use_codestory_cli_exe');
    }
    return {
      source: 'local_dev_override',
      path: batchOverride ? null : cliPath,
      sha256: !batchOverride && fs.existsSync(cliPath) ? fileSha256(cliPath) : null,
      version,
      cliVersion: null,
      repoRef: null,
      buildSource: 'local_dev_override',
      sourcePackageSha256: null,
      archiveSha256: null,
      archiveUrl: null,
      provisionedAt: null,
      warnings,
    };
  }

  const managed = await resolveManagedCli(pluginDataDir(), version, warnings, options);
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
    sourcePackageSha256: null,
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
  const result = spawnCodeStoryCliSync(resolved.path, ['--version'], {
    encoding: 'utf8',
    env: resolved.provisioningProbe
      ? { ...process.env, CODESTORY_PLUGIN_PROVISIONING_PROBE: '1' }
      : process.env,
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
  const batchRejection = (resolved.warnings || []).find((warning) =>
    warning.startsWith('codestory_cli_batch_override_rejected:'));
  if (batchRejection) return batchRejection;
  if (resolved.source === 'managed_unavailable') {
    return resolved.managedProvisionFailure || 'managed_cli_unavailable';
  }
  if (resolved.source === 'local_dev_receipt_invalid') {
    return resolved.localDevReceiptFailure || 'codestory_dev_receipt_invalid';
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
  if (isWindowsBatchCli(realExecutable)) {
    return { verified: false, reason: 'manifest_batch_executable_rejected' };
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
      || !sameFilesystemPath(activeVerification.executablePath, resolved.path)) {
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
    source_package_sha256: resolved.sourcePackageSha256 || null,
    local_dev_override: resolved.source === 'local_dev_override',
    managed_binary_path: resolved.source === 'managed' ? resolved.path : null,
    managed_binary_sha256: resolved.source === 'managed' ? resolved.sha256 : null,
    managed_manifest_path: resolved.manifestPath || null,
    managed_cli_retention: resolved.managedCliRetention || null,
    warnings: resolved.warnings.filter(Boolean),
  };
}

function fallbackDiagnostic(resolved, probe, reason, options = {}) {
  const projectRoot = Object.hasOwn(options, 'projectRoot') ? options.projectRoot : null;
  const preparing = reason === 'managed_cli_provisioning';
  const plugin = pluginRuntimeForResolved({ ...resolved, warnings: [...resolved.warnings, reason] });
  const readiness = {
    goal: 'runtime',
    status: preparing ? 'preparing' : 'unavailable',
    summary: options.summary || 'CodeStory plugin MCP could not start a compatible codestory-cli stdio runtime.',
    reason,
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
  const surfaces = [
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
    'packet',
    'search',
    'context',
  ];
  const blockedSurface = () => ({
    allowed: false,
    readiness_goal: readiness.goal,
    failed_layer: 'runtime_setup',
    reason,
  });
  return {
    cli_version: probe.version,
    plugin_runtime: plugin,
    runtime: {
      source: plugin.cli_source || 'unavailable',
      state: readiness.status,
      automatic: true,
    },
    warnings: plugin.warnings,
    project_root: projectRoot,
    project_root_source: options.projectRootSource || null,
    retrieval_mode: 'unavailable',
    degraded_reason: reason,
    readiness: [readiness],
    managed_retrieval: {
      state: readiness.status,
      automatic: true,
    },
    allowed_surfaces: Object.fromEntries(surfaces.map((surface) => [surface, blockedSurface()])),
    recommended_next_calls: preparing
      ? [{ method: 'tools/call', instruction: 'Retry the intended CodeStory tool shortly.', retry_after_ms: 1500 }]
      : projectRoot
        ? [{
            method: 'resources/read',
            uri: projectBoundResourceUri('codestory://status', projectRoot),
          }]
        : [{
            method: 'resources/read',
            uri_template: 'codestory://status{?project}',
          }],
  };
}

function sameFilesystemPath(left, right) {
  if (!String(left || '').trim() || !String(right || '').trim()) return false;
  const leftPath = path.resolve(String(left));
  const rightPath = path.resolve(String(right));
  let leftStat;
  let rightStat;
  try {
    leftStat = fs.statSync(leftPath, { bigint: true });
  } catch (error) {
    if (!['ENOENT', 'ENOTDIR'].includes(error?.code)) return false;
  }
  try {
    rightStat = fs.statSync(rightPath, { bigint: true });
  } catch (error) {
    if (!['ENOENT', 'ENOTDIR'].includes(error?.code)) return false;
  }
  if (leftStat && rightStat) {
    if (leftStat.ino !== 0n || rightStat.ino !== 0n) {
      return leftStat.dev === rightStat.dev && leftStat.ino === rightStat.ino;
    }
    const leftReal = fs.realpathSync(leftPath);
    const rightReal = fs.realpathSync(rightPath);
    const normalizeExisting = (value) => process.platform === 'win32' ? value.toLowerCase() : value;
    return normalizeExisting(leftReal) === normalizeExisting(rightReal);
  }
  if (leftStat || rightStat) return false;
  const normalizeMissing = (value) => process.platform === 'win32' ? value.toLowerCase() : value;
  return normalizeMissing(leftPath) === normalizeMissing(rightPath);
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

function strictUriComponentEncode(value) {
  let encoded = '';
  for (const byte of Buffer.from(String(value), 'utf8')) {
    const unreserved =
      (byte >= 0x30 && byte <= 0x39)
      || (byte >= 0x41 && byte <= 0x5A)
      || (byte >= 0x61 && byte <= 0x7A)
      || [0x2D, 0x2E, 0x5F, 0x7E].includes(byte);
    encoded += unreserved ? String.fromCharCode(byte) : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
  }
  return encoded;
}

function strictUriComponentDecode(value, label) {
  const bytes = [];
  for (let index = 0; index < value.length;) {
    const code = value.charCodeAt(index);
    const unreserved =
      (code >= 0x30 && code <= 0x39)
      || (code >= 0x41 && code <= 0x5A)
      || (code >= 0x61 && code <= 0x7A)
      || [0x2D, 0x2E, 0x5F, 0x7E].includes(code);
    if (unreserved) {
      bytes.push(code);
      index += 1;
      continue;
    }
    const escape = value.slice(index, index + 3);
    if (!/^%[0-9A-F]{2}$/u.test(escape)) {
      throw new Error(`${label} uses a non-canonical URI encoding`);
    }
    bytes.push(Number.parseInt(escape.slice(1), 16));
    index += 3;
  }
  let decoded;
  try {
    decoded = new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(bytes));
  } catch {
    throw new Error(`${label} is not valid UTF-8`);
  }
  if (strictUriComponentEncode(decoded) !== value) {
    throw new Error(`${label} uses a non-canonical URI encoding`);
  }
  return decoded;
}

function cleanPublicProjectPath(value, platform = process.platform) {
  if (platform !== 'win32') return String(value);
  let project = String(value).replaceAll('\\', '/');
  if (project.startsWith('//?/UNC/')) {
    project = `//${project.slice('//?/UNC/'.length)}`;
  } else if (project.startsWith('//?/')) {
    project = project.slice('//?/'.length);
  }
  return project;
}

function projectBoundResourceUri(baseUri, project) {
  return `${baseUri}?project=${strictUriComponentEncode(cleanPublicProjectPath(project))}`;
}

function parseFailOpenResourceRequest(uri, legacyProject) {
  if (uri === 'codestory://agent-guide') {
    if (legacyProject !== undefined) {
      throw new Error('resource_project_unexpected: codestory://agent-guide is static and does not accept a project selector');
    }
    return { kind: 'agent-guide', project: null, uri };
  }
  const queryIndex = typeof uri === 'string' ? uri.indexOf('?') : -1;
  const baseUri = queryIndex >= 0 ? uri.slice(0, queryIndex) : uri;
  const query = queryIndex >= 0 ? uri.slice(queryIndex + 1) : null;
  if (baseUri !== 'codestory://status') {
    throw new Error(`unknown resource: ${uri || '<missing>'}`);
  }
  if (query !== null && (query.includes('?') || query.includes('&'))) {
    throw new Error('resource_project_conflict: project-scoped resource URI must include exactly one `project` query selector');
  }
  if (query !== null && legacyProject !== undefined) {
    throw new Error('resource_project_conflict: pass `project` exactly once, either in the resource URI or the legacy params field');
  }
  let projectValue = legacyProject;
  if (query !== null) {
    if (!query.startsWith('project=') || query.slice('project='.length).includes('=')) {
      throw new Error('resource_project_malformed: expected one non-empty `project` query selector');
    }
    const encodedProject = query.slice('project='.length);
    if (!encodedProject) {
      throw new Error('resource_project_malformed: expected one non-empty `project` query selector');
    }
    projectValue = strictUriComponentDecode(encodedProject, 'resource project');
  }
  const selection = selectExplicitProject(projectValue);
  if (!selection.ok) {
    throw new Error(`${selection.code}: ${selection.message}`);
  }
  const project = cleanPublicProjectPath(selection.project);
  return {
    kind: 'status',
    project,
    projectSource: query === null ? 'request_argument' : 'resource_uri',
    uri: projectBoundResourceUri(baseUri, project),
  };
}

function failOpenToolCatalog() {
  if (!Array.isArray(canonicalMcpCatalog?.tools)) {
    throw new Error('generated_mcp_catalog_missing:run_generate_codestory_skill_syntax');
  }
  return JSON.parse(JSON.stringify(canonicalMcpCatalog.tools));
}

function selectExplicitProject(value) {
  if (typeof value !== 'string' || !value.trim()) {
    return {
      ok: false,
      code: 'project_required',
      message: 'Pass the caller\'s absolute repository root in the `project` argument.',
      project: null,
    };
  }
  if (!path.isAbsolute(value)) {
    return {
      ok: false,
      code: 'project_required',
      message: '`project` must be an absolute repository root.',
      project: null,
    };
  }
  const project = path.resolve(value);
  try {
    if (!fs.statSync(project).isDirectory()) {
      throw Object.assign(new Error('project is not a directory'), { code: 'ENOTDIR' });
    }
    return { ok: true, project: fs.realpathSync(project) };
  } catch (error) {
    return {
      ok: false,
      code: 'project_unavailable',
      message: `Project root is unavailable: ${project} (${error.code || error.message})`,
      project,
    };
  }
}

function failOpenToolResult(tool, status, argumentsValue = {}) {
  const preparing = status.managed_retrieval?.state === 'preparing';
  const readiness = Array.isArray(status.readiness) ? status.readiness[0] : null;
  const degradedReason = status.degraded_reason || readiness?.reason || (preparing ? 'managed_cli_provisioning' : 'runtime_unavailable');
  const primaryFailure = readiness?.setup?.probe_error
    || readiness?.setup?.probe_stderr
    || readiness?.summary
    || status.warnings?.find((warning) => String(warning || '').trim())
    || degradedReason;
  const selection = selectExplicitProject(argumentsValue.project);
  if (!selection.ok) {
    const structuredContent = {
      code: selection.code,
      message: selection.message,
      tool,
      project: selection.project,
      state: selection.code === 'project_required' ? 'no_project' : 'unavailable',
    };
    if (tool === 'status' && selection.code === 'project_required') {
      return {
        content: [{ type: 'text', text: 'state: no_project\nresult: structured\n' }],
        structuredContent,
      };
    }
    return {
      content: [{ type: 'text', text: structuredContent.message }],
      structuredContent,
      isError: true,
    };
  }
  const project = selection.project;
  if (tool === 'status') {
    const diagnosticsUri = projectBoundResourceUri('codestory://status', project);
    const structuredContent = {
      project,
      state: preparing ? 'preparing' : 'unavailable',
      degraded_reason: degradedReason,
      capabilities: { local_navigation: 'unavailable', broad_search: preparing ? 'preparing' : 'unavailable' },
      current_operation: preparing ? {
        operation_id: 'managed-runtime-provisioning',
        state: 'preparing',
        stage: 'dense_preparation',
        attempt: 1,
        retry_after_ms: 1500,
        failure: null,
      } : null,
      failure: preparing ? null : primaryFailure,
      next_action: preparing ? 'retry_intended_tool' : 'use_source_inspection',
      retry_after_ms: preparing ? 1500 : null,
      diagnostics_uri: diagnosticsUri,
    };
    return {
      content: [{ type: 'text', text: `state: ${structuredContent.state}\nresult: structured\n` }],
      structuredContent,
    };
  }
  const diagnosticsUri = projectBoundResourceUri('codestory://status', project);
  const structuredContent = preparing ? {
    code: 'codestory_preparing',
    message: 'CodeStory is preparing. Retry the same tool shortly.',
    tool,
    project,
    state: 'preparing',
    retry_tool: tool,
    retry_after_ms: 1500,
    operation: {
      operation_id: 'managed-runtime-provisioning',
      state: 'preparing',
      stage: 'dense_preparation',
      attempt: 1,
      retry_after_ms: 1500,
      failure: null,
    },
    diagnostics_uri: diagnosticsUri,
  } : {
    code: 'codestory_unavailable',
    message: 'CodeStory is unavailable. Continue with focused source inspection.',
    tool,
    project,
    state: 'unavailable',
    diagnostics_uri: diagnosticsUri,
  };
  return {
    content: [{ type: 'text', text: structuredContent.message }],
    structuredContent,
    isError: true,
  };
}

const shuttingDownHandoffs = new WeakSet();

function shutdownHandoffChild(child, options = {}) {
  if (!child || typeof child !== 'object' || shuttingDownHandoffs.has(child)) return;
  shuttingDownHandoffs.add(child);
  try {
    child.stdin?.end();
  } catch {
    // Continue to the bounded process shutdown below.
  }
  if (typeof child.kill !== 'function') return;
  const isRunning = () => child.exitCode == null && child.signalCode == null;
  const graceMs = options.handoffTerminationGraceMs ?? 500;
  const forceGraceMs = options.handoffForceKillGraceMs ?? 500;
  let forceTimer = null;
  const terminateTimer = setTimeout(() => {
    if (!isRunning()) return;
    try {
      child.kill('SIGTERM');
    } catch {
      return;
    }
    forceTimer = setTimeout(() => {
      if (!isRunning()) return;
      try {
        child.kill('SIGKILL');
      } catch {
        // The child already left or the platform rejected the final signal.
      }
    }, forceGraceMs);
    forceTimer.unref?.();
  }, graceMs);
  terminateTimer.unref?.();
  const clearTimers = () => {
    clearTimeout(terminateTimer);
    if (forceTimer) clearTimeout(forceTimer);
  };
  child.once?.('exit', clearTimers);
  child.once?.('close', clearTimers);
}

function runFailOpenMcp(status, options = {}) {
  const currentStatus = () => (typeof status === 'function' ? status() : status);
  let handoff = null;
  let initializeRequest = null;
  let initializedNotification = null;
  let runtimeReadyNotified = false;
  let stdinEnded = false;
  const delegatedRequestIds = new Set();
  let handoffFailureHandled = false;
  const notifyRuntimeReady = () => {
    if (!initializedNotification || runtimeReadyNotified) return;
    if (
      typeof options.shouldHandoff === 'function' &&
      !options.shouldHandoff(currentStatus())
    ) return;
    runtimeReadyNotified = true;
    process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' })}\n`);
    process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/resources/list_changed' })}\n`);
    process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/prompts/list_changed' })}\n`);
  };
  const maybeHandoff = () => {
    if (handoff || typeof options.startRuntime !== 'function') {
      return handoff;
    }
    const liveStatus = currentStatus();
    const shouldHandoff = typeof options.shouldHandoff === 'function'
      ? options.shouldHandoff(liveStatus)
      : liveStatus.project_root && liveStatus.degraded_reason === 'project_root_recovered_after_launch';
    if (!shouldHandoff) {
      return null;
    }
    handoff = options.startRuntime(liveStatus);
    handoffFailureHandled = false;
    const failHandoff = (reason, details = {}) => {
      if (handoffFailureHandled) return;
      handoffFailureHandled = true;
      const failedHandoff = handoff;
      handoff = null;
      shutdownHandoffChild(failedHandoff, options);
      if (typeof options.onRuntimeFailure !== 'function') {
        process.exit(details.code || 1);
        return;
      }
      for (const id of delegatedRequestIds) {
        process.stdout.write(`${JSON.stringify(jsonrpcError(JSON.parse(id), -32000, reason))}\n`);
      }
      delegatedRequestIds.clear();
      options.onRuntimeFailure({ reason, ...details });
    };
    if (handoff.stdout) {
      let stdout = '';
      let suppressInitialize = Boolean(initializeRequest);
      handoff.stdout.setEncoding('utf8');
      handoff.stdout.on('data', (chunk) => {
        stdout += chunk;
        const lines = stdout.split(/\r?\n/u);
        stdout = lines.pop() || '';
        for (const output of lines) {
          if (!output) continue;
          let parsed = null;
          try {
            parsed = JSON.parse(output);
          } catch {
            // Non-JSON output remains visible instead of hiding a runtime failure.
          }
          if (suppressInitialize && parsed?.id === initializeRequest.id) {
            suppressInitialize = false;
            continue;
          }
          if (parsed?.id !== undefined) delegatedRequestIds.delete(JSON.stringify(parsed.id));
          process.stdout.write(`${output}\n`);
        }
      });
    }
    handoff.stderr?.pipe(process.stderr);
    handoff.on('close', (code, signal) => {
      if (signal || code) {
        failHandoff('CodeStory stdio handoff exited before completing the request.', { code, signal });
        return;
      }
      process.exit(0);
    });
    handoff.on('error', (error) => {
      failHandoff(`CodeStory stdio handoff failed: ${error.message}`, { error });
    });
    if (initializeRequest) {
      handoff.stdin.write(`${JSON.stringify(initializeRequest)}\n`);
      handoff.stdin.write(`${JSON.stringify(initializedNotification || {
        jsonrpc: '2.0',
        method: 'notifications/initialized',
      })}\n`);
    }
    if (stdinEnded) handoff.stdin.end();
    return handoff;
  };
  const tools = failOpenToolCatalog();
  const resources = (canonicalMcpCatalog.resources || []).filter(({ uri }) =>
    uri === 'codestory://agent-guide');
  // Fail-open serves the project-bound status template and static guide. Do
  // not advertise other generated templates or prompts until the native
  // runtime owns their read/get handlers.
  const resourceTemplates = (canonicalMcpCatalog.resourceTemplates || []).filter(({ uriTemplate }) =>
    uriTemplate === 'codestory://status{?project}');
  const prompts = [];
  const guide = () => {
    return {
      message: 'Call the tool that matches the task and pass its absolute repository root. If it reports preparing, retry that same tool after its delay.',
      diagnostics_uri_template: 'codestory://status{?project}',
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
      if (request.method === 'notifications/initialized') {
        initializedNotification = request;
        notifyRuntimeReady();
        continue;
      }
      if (request.method === 'initialize' && request.id !== undefined) {
        initializeRequest = request;
      }
      const delegated = request.method === 'initialize' ? null : maybeHandoff();
      if (delegated) {
        if (request.id !== undefined) delegatedRequestIds.add(JSON.stringify(request.id));
        try {
          delegated.stdin.write(`${line}\n`);
        } catch (error) {
          process.stdout.write(`${JSON.stringify(jsonrpcError(request.id ?? null, -32000, `CodeStory stdio handoff failed: ${error.message}`))}\n`);
        }
        continue;
      }
      if (request.id === undefined) continue;
      let response;
      if (request.method === 'initialize') {
        const liveStatus = currentStatus();
        response = jsonrpcResult(request.id, {
          protocolVersion: request.params?.protocolVersion || '2024-11-05',
          capabilities: {
            tools: { listChanged: true },
            resources: { subscribe: false, listChanged: true },
            prompts: { listChanged: true },
          },
          serverInfo: { name: 'codestory', version: resolvedVersionForStatus(liveStatus) },
        });
      } else if (request.method === 'tools/list') {
        response = jsonrpcResult(request.id, { tools });
      } else if (request.method === 'resources/list') {
        response = jsonrpcResult(request.id, { resources });
      } else if (request.method === 'resources/templates/list') {
        response = jsonrpcResult(request.id, { resourceTemplates });
      } else if (request.method === 'prompts/list') {
        response = jsonrpcResult(request.id, { prompts });
      } else if (request.method === 'resources/read') {
        const uri = request.params?.uri;
        let parsedResource;
        try {
          parsedResource = parseFailOpenResourceRequest(uri, request.params?.project);
        } catch (error) {
          response = jsonrpcError(request.id, -32602, error.message);
        }
        if (parsedResource?.kind === 'status') {
          const project = parsedResource.project;
          const statusValue = { ...currentStatus() };
          statusValue.project_root = project;
          statusValue.project_root_source = parsedResource.projectSource;
          statusValue.diagnostics_uri = parsedResource.uri;
          if (Array.isArray(statusValue.recommended_next_calls)) {
            statusValue.recommended_next_calls = statusValue.recommended_next_calls.map((call) =>
              call?.method === 'resources/read'
                && call?.uri_template === 'codestory://status{?project}'
                ? { method: call.method, uri: parsedResource.uri }
                : call);
          }
          response = jsonrpcResult(
            request.id,
            resourceContents(parsedResource.uri, statusValue),
          );
        } else if (parsedResource?.kind === 'agent-guide') {
          response = jsonrpcResult(request.id, resourceContents(parsedResource.uri, guide()));
        }
      } else if (request.method === 'tools/call') {
        const tool = request.params?.name;
        response = tools.some((candidate) => candidate.name === tool)
          ? jsonrpcResult(request.id, failOpenToolResult(tool, currentStatus(), request.params?.arguments))
          : jsonrpcError(request.id, -32602, `unknown tool: ${tool || '<missing>'}`);
      } else {
        response = jsonrpcError(request.id, -32601, `method not found: ${request.method || '<missing>'}`);
      }
      process.stdout.write(`${JSON.stringify(response)}\n`);
    }
  });
  process.stdin.on('end', () => {
    stdinEnded = true;
    shutdownHandoffChild(handoff, options);
  });
  return { notifyRuntimeReady };
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
      sourcePackageSha256: resolved.sourcePackageSha256 || null,
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
    CODESTORY_PLUGIN_SOURCE_PACKAGE_SHA256: resolved.sourcePackageSha256 || '',
    CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256: resolved.archiveSha256 || '',
    CODESTORY_PLUGIN_CLI_ARCHIVE_URL: resolved.archiveUrl || '',
    CODESTORY_PLUGIN_CLI_PROVISIONED_AT: resolved.provisionedAt || '',
    CODESTORY_PLUGIN_CLI_RETENTION: JSON.stringify(resolved.managedCliRetention || null),
    CODESTORY_PLUGIN_CLI_WARNINGS: resolved.warnings.join(';'),
    CODESTORY_PLUGIN_MULTI_PROJECT: '1',
    CODESTORY_PLUGIN_DATA: pluginDataDir() || '',
  };
}

function spawnStdioRuntime(resolved, runtimeCwd, stdio) {
  return spawnCodeStoryCli(resolved.path, ['serve', '--stdio', '--multi-project', '--refresh', 'none'], {
    cwd: runtimeCwd,
    stdio,
    windowsHide: true,
    env: stdioRuntimeEnv(resolved, runtimeCwd),
  });
}

async function main() {
  const runtimeCwd = releasePluginCacheCwd();
  const installed = await resolveCli({ provision: false });
  if (
    installed.source === 'managed_unavailable' &&
    process.env.CODESTORY_PLUGIN_DISABLE_PROVISION !== '1'
  ) {
    let ready = null;
    let diagnostic = null;
    let status = fallbackDiagnostic(installed, probeResolvedCli(installed), 'managed_cli_provisioning', {
      projectRoot: null,
      projectRootSource: 'request_argument',
      summary: 'CodeStory is preparing. The requested tool will be available shortly.',
    });
    setImmediate(() => {
      resolveCli().then((resolved) => {
        const probe = probeResolvedCli(resolved);
        const reason = failOpenReasonForProbe(resolved, probe);
        resolved.managedCliRetention = managedCliRetentionReport(resolved, probe, { dryRun: Boolean(reason) });
        rememberLaunch(resolved, runtimeCwd);
        if (reason) {
          status = fallbackDiagnostic(resolved, probe, reason, {
            projectRoot: null,
            projectRootSource: 'request_argument',
          });
          return;
        }
        ready = resolved;
        diagnostic?.notifyRuntimeReady();
      }).catch((error) => {
        status = fallbackDiagnostic(installed, probeResolvedCli(installed), `launcher_error:${error.message}`, {
          projectRoot: null,
          projectRootSource: 'request_argument',
        });
      });
    });
    diagnostic = runFailOpenMcp(() => status, {
      shouldHandoff: () => Boolean(ready),
      startRuntime: () => spawnStdioRuntime(ready, runtimeCwd, ['pipe', 'pipe', 'pipe']),
      onRuntimeFailure: (failure) => {
        const failed = ready;
        ready = null;
        const reason = failure.error ? 'managed_cli_handoff_unspawnable' : 'runtime_stdio_child_exit';
        status = fallbackDiagnostic(failed, {
          status: failure.code ?? null,
          error: failure.error?.message || failure.reason,
          version: failed.cliVersion || failed.version,
          stdout: '',
          stderr: '',
        }, reason, {
          projectRoot: null,
          projectRootSource: 'request_argument',
          summary: 'CodeStory managed CLI provisioning completed, but the stdio runtime failed during handoff.',
        });
      },
    });
    return;
  }
  const resolved = installed;
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
  let status = fallbackDiagnostic(resolved, probe, 'runtime_stdio_handoff', {
    projectRoot: null,
    projectRootSource: 'request_argument',
    summary: 'CodeStory is handing the initialized MCP session to its verified stdio runtime.',
  });
  let handoffReady = true;
  runFailOpenMcp(() => status, {
    shouldHandoff: () => handoffReady,
    startRuntime: () => spawnStdioRuntime(resolved, runtimeCwd, ['pipe', 'pipe', 'pipe']),
    onRuntimeFailure: (failure) => {
      handoffReady = false;
      const reason = failure.error ? `${resolved.source}_cli_unspawnable` : 'runtime_stdio_child_exit';
      const error = failure.error?.message
        || (failure.code != null
          ? `codestory-cli serve --stdio exited with status ${failure.code}`
          : failure.reason);
      status = fallbackDiagnostic(resolved, {
        ...probe,
        status: failure.code ?? null,
        error,
        stderr: probe.stderr || '',
      }, reason, {
        projectRoot: null,
        projectRootSource: 'request_argument',
        summary: 'CodeStory launched its verified CLI, but the stdio runtime failed during handoff.',
      });
    },
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
      cleanPublicProjectPath,
      downloadFile,
      extractArchive,
      failOpenToolCatalog,
      parseFailOpenResourceRequest,
      projectBoundResourceUri,
      strictUriComponentDecode,
      strictUriComponentEncode,
      acquireManagedCliLock,
      managedCliLockWaitMs,
      releaseAssetRetryBudgetMs,
      managedAssetIdentity,
      releaseAssetIdentity,
      isWindowsBatchCli,
      requireDirectCli,
      reclaimStaleManagedCliPendingOwners,
      removeManagedCliInitializationIf,
      processStartIdentity,
      probeManagedCliStdio,
      provisionManagedCli,
      quarantineManagedCliVersion,
      releaseManagedCliLock,
      resolveManagedCli,
      runFailOpenMcp,
      sameFilesystemPath,
      shutdownHandoffChild,
      stageExtractedManagedCli,
      managedCliRetentionReport,
      managedCliVersionEntries,
      removeManagedCliVersion,
      verifyPublishedManagedCli,
      verifyManagedCliVersion,
    },
  };
}
