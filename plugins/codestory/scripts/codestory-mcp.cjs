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
const { Worker, isMainThread, parentPort, workerData } = require('worker_threads');
const zlib = require('zlib');
const {
  cliVersionProbeTimeoutMs,
  sourceBuildTarget,
  validateDevCliReceipt,
} = require('./codestory-dev-cli-contract.cjs');

const pluginRoot = path.dirname(__dirname);
const launchCwd = workerData?.codestoryLaunchCwd || process.cwd();
const binaryName = process.platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
function positiveDurationEnv(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  const parsed = Number(raw);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : fallback;
}

// Release downloads are bounded by two independent limits. The stall timeout detects a dead
// connection (no bytes at all within the window) and is the limit that should normally fire; the
// total budget is only a backstop for a link that trickles forever. A single total deadline sized
// for a fast link is what made first use unusable on a slow one: a multi-hundred-megabyte archive
// simply cannot land inside it, so every attempt aborted mid-transfer no matter how healthy the
// connection was. Budgets span all attempts of one asset because attempts resume rather than
// restart, so retries no longer multiply the wall clock.
const releaseDownloadStallTimeoutMs = positiveDurationEnv(
  'CODESTORY_PLUGIN_DOWNLOAD_STALL_TIMEOUT_MS',
  60 * 1000,
);
const releaseChecksumTotalTimeoutMs = positiveDurationEnv(
  'CODESTORY_PLUGIN_DOWNLOAD_CHECKSUM_TIMEOUT_MS',
  60 * 1000,
);
const releaseArchiveTotalTimeoutMs = positiveDurationEnv(
  'CODESTORY_PLUGIN_DOWNLOAD_TIMEOUT_MS',
  60 * 60 * 1000,
);
// Attempts resume from the bytes already on disk, so a high cap costs little and lets a flaky
// link keep inching forward; the total budget above is what actually ends a hopeless download.
const releaseDownloadAttempts = positiveDurationEnv('CODESTORY_PLUGIN_DOWNLOAD_ATTEMPTS', 20);
const releaseDownloadRetryDelaysMs = [1000, 2000, 5000, 10000, 15000];
const releaseDownloadRetryJitterMs = 250;
const managedCliLockStaleMs = 10 * 60 * 1000;
const managedCliLockMaxAgeMs = 30 * 60 * 1000;
const managedCliIdentityProbeIntervalMs = 2000;
const releaseAssetRetryBudgetMs = releaseArchiveTotalTimeoutMs;
const managedCliStagingBudgetMs = 30 * 1000;
// A waiter blocks only the background provisioning task, never a tool call, so it can afford to
// outlast a publisher that is legitimately downloading the archive over a slow link.
const managedCliLockWaitMs =
  releaseChecksumTotalTimeoutMs + releaseArchiveTotalTimeoutMs + managedCliStagingBudgetMs;
const managedCliDownloadCacheDirName = '.download';
const managedCliDownloadCacheMaxAgeMs = 7 * 24 * 60 * 60 * 1000;
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
// Wire compatibility contract. `codestory_contracts::wire` owns these values;
// the generated MCP catalog records the same three read back out of the real
// binary, and `launcher wire contract matches the generated catalog` in the
// plugin test suite pins the launcher copy to that recording. The launcher must
// not depend on the catalog at run time: a packaging failure that loses the
// catalog must not also lose the skew detector.
const managedCliMcpProtocolVersion = '2024-11-05';
const supportedMcpProtocolVersions = Object.freeze(['2024-11-05']);
const publicationStampSchemaVersion = 2;
const minimumCompatiblePublicationStampSchemaVersion = 2;
const runtimeStderrObservedBytesCap = 16 * 1024 * 1024;
const runtimeStderrObservedChunksCap = 65_535;
const failOpenMaxFrameBytes = 1024 * 1024;

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

const SEMVER_SHAPE = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u;
const SHA256_SHAPE = /^[0-9a-f]{64}$/u;
const PINNED_CLI_TARGETS = ['macos-arm64', 'windows-x64', 'linux-x64'];

// The plugin's own version names the package; the pin names the CLI it runs. Splitting them is
// what lets a plugin-only release ship without cutting new native archives: the plugin version
// moves, the pin keeps naming the already-published, already-proven CLI. When the file is absent
// the two are the same, which is exactly the pre-pin behavior.
//
// A malformed pin fails closed rather than falling back: a pin that no longer parses must not
// silently change which binary every session runs.
function pinnedCliContract() {
  const pinPath = path.join(pluginRoot, 'cli-version.json');
  if (!fs.existsSync(pinPath)) return null;
  const pin = readJson(pinPath);
  const validShape =
    pin &&
    pin.schema_version === 1 &&
    typeof pin.cli_version === 'string' &&
    SEMVER_SHAPE.test(pin.cli_version) &&
    pin.release_tag === `v${pin.cli_version}` &&
    (pin.archives === undefined ||
      (pin.archives &&
        typeof pin.archives === 'object' &&
        !Array.isArray(pin.archives) &&
        Object.keys(pin.archives).every(
          (target) =>
            PINNED_CLI_TARGETS.includes(target) && SHA256_SHAPE.test(String(pin.archives[target])),
        )));
  if (!validShape) {
    const error = new Error('managed_cli_pin_invalid');
    error.pinInvalid = true;
    throw error;
  }
  return pin;
}

function pinnedCliVersion() {
  const pin = pinnedCliContract();
  return pin ? pin.cli_version : pluginVersion();
}

// The expected archive digest for this target when the pin carries one. Content-addressing on top
// of SHA256SUMS.txt: the checksums file arrives over the same channel as the archive, while the
// pin ships inside the reviewed plugin package.
function pinnedArchiveSha256(target) {
  const pin = pinnedCliContract();
  const digest = pin?.archives?.[target];
  return typeof digest === 'string' ? digest.toLowerCase() : null;
}

// The native lane's archive-digest owner, published beside the archives it describes.
//
// A native release's archive digests cannot be pinned in source: the archives are built FROM the
// source tree that would carry them. The release generates this manifest from the archives it just
// built, so the digests exist without the circularity. The schema below mirrors
// scripts/lib/release-manifest.mjs; the plugin ships without that directory, and
// plugin-static.test.mjs holds the two copies against each other.
//
// Containment, not authentication: until the manifest is signed it arrives over the same channel
// as the archive, so it detects corruption and drift, not a channel that lies consistently.
const RELEASE_MANIFEST_ASSET = 'codestory-release-manifest.json';
const RELEASE_MANIFEST_DOMAIN = 'codestory.release-manifest';
const RELEASE_MANIFEST_SCHEMA_VERSION = 1;

function releaseManifestArchiveEntry(manifest, version, target) {
  if (!isPlainObject(manifest)) throw new Error('release_manifest_invalid:not_an_object');
  if (manifest.domain !== RELEASE_MANIFEST_DOMAIN) {
    throw new Error('release_manifest_invalid:domain');
  }
  if (manifest.schema_version !== RELEASE_MANIFEST_SCHEMA_VERSION) {
    throw new Error('release_manifest_invalid:schema_version');
  }
  // A manifest for a different release is a valid manifest for the wrong bytes, which is exactly
  // the substitution a digest check is supposed to stop.
  if (manifest.version !== version || manifest.tag !== `v${version}`) {
    throw new Error('release_manifest_invalid:release_identity');
  }
  if (!/^[0-9a-f]{40}$/u.test(String(manifest.commit || ''))) {
    throw new Error('release_manifest_invalid:commit');
  }
  if (!isPlainObject(manifest.archives)) throw new Error('release_manifest_invalid:archives');
  const entry = manifest.archives[target];
  if (!isPlainObject(entry)) throw new Error('release_manifest_invalid:target');
  if (entry.filename !== archiveName(version, target)) {
    throw new Error('release_manifest_invalid:filename');
  }
  if (!Number.isSafeInteger(entry.bytes) || entry.bytes <= 0) {
    throw new Error('release_manifest_invalid:bytes');
  }
  if (!/^[0-9a-f]{64}$/u.test(String(entry.sha256 || ''))) {
    throw new Error('release_manifest_invalid:sha256');
  }
  return entry;
}

// Fetched with the checksum file, before the archive transfer, so a manifest that is malformed or
// describes another release stops the provision without paying for a multi-hundred-megabyte
// download first. A release published before this manifest existed carries none: the absence is
// recorded as a warning rather than treated as agreement, and the containment for those releases
// stays what it was -- SHA256SUMS.txt plus the source pin when the pin carries digests.
async function fetchReleaseManifestEntry(version, target, tempRoot, warnings) {
  const manifestPath = path.join(tempRoot, RELEASE_MANIFEST_ASSET);
  try {
    await fetchReleaseFile(version, RELEASE_MANIFEST_ASSET, manifestPath);
  } catch (error) {
    warnings.push(`managed_cli_publication:release_manifest_absent:${managedCliFailureCode(error)}`);
    return null;
  }
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  } catch {
    throw new Error('release_manifest_invalid:json');
  }
  return releaseManifestArchiveEntry(manifest, version, target);
}

// Held against the downloaded bytes BEFORE extraction, because extraction is the first step that
// acts on what the release channel supplied.
function bindArchiveToReleaseManifest(entry, observed, warnings) {
  if (!entry) return null;
  if (entry.sha256 !== observed.sha256) {
    throw new Error(`release_manifest_archive_mismatch:${entry.filename}:sha256`);
  }
  if (entry.bytes !== observed.bytes) {
    throw new Error(`release_manifest_archive_mismatch:${entry.filename}:bytes`);
  }
  warnings.push('managed_cli_publication:release_manifest_bound');
  return entry;
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

function inferredCursorPluginDataDir(
  root = pluginRoot,
  home = process.env.HOME || process.env.USERPROFILE || os.homedir(),
  options = {},
) {
  const parts = path.resolve(root).split(/[\\/]+/u);
  for (let index = 0; index <= parts.length - 5; index += 1) {
    if (
      parts[index].toLowerCase() !== '.cursor' ||
      parts[index + 1] !== 'plugins' ||
      parts[index + 2] !== 'cache'
    ) {
      continue;
    }
    const packageIndex = parts.findIndex(
      (part, candidateIndex) => candidateIndex >= index + 3 && part.toLowerCase() === 'codestory',
    );
    if (packageIndex === -1 || packageIndex === parts.length - 1) continue;
    const cursorRoot = parts.slice(0, index + 1).join(path.sep);
    const dataDir = path.join(cursorRoot, 'plugins', 'data', 'codestory');
    if (usablePluginDataDir(dataDir)) return dataDir;
  }

  if (!confirmedCursorIdentity(options.env || process.env)) return null;
  const fallback = path.join(home, '.cursor', 'plugins', 'data', 'codestory');
  return usablePluginDataDir(fallback) ? fallback : null;
}

const cursorDogfoodMarker = 'CODESTORY_CURSOR_DOGFOOD';

function confirmedCursorIdentity(env = process.env) {
  return Boolean(
    env.CURSOR_PLUGIN_ROOT
    || env[cursorDogfoodMarker] === '1'
  );
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
    || inferredCodexPluginDataDir()
    || inferredCursorPluginDataDir();
}

const cursorLocalOverrideFileName = 'local-overrides.json';
const cursorLocalOverrideMaxBytes = 64 * 1024;

function readCursorLocalOverrides(root = pluginRoot, options = {}) {
  const env = options.env || process.env;
  const explicitPluginData = Object.hasOwn(options, 'pluginData')
    ? options.pluginData
    : env.PLUGIN_DATA;
  const dataDir = typeof explicitPluginData === 'string' && path.isAbsolute(explicitPluginData)
    ? explicitPluginData
    : env[cursorDogfoodMarker] === '1'
      ? inferredCursorPluginDataDir(root, options.home, { env })
      : null;
  if (!dataDir) return null;
  const overridePath = path.join(dataDir, cursorLocalOverrideFileName);
  try {
    const metadata = fs.lstatSync(overridePath);
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > cursorLocalOverrideMaxBytes) {
      return null;
    }
    const value = JSON.parse(fs.readFileSync(overridePath, 'utf8'));
    if (
      value?.schema_version !== 1 ||
      !path.isAbsolute(value?.CODESTORY_CLI || '') ||
      Object.keys(value).sort().join(',') !== 'CODESTORY_CLI,schema_version'
    ) {
      return null;
    }
    return { CODESTORY_CLI: value.CODESTORY_CLI };
  } catch {
    return null;
  }
}

function applyCursorLocalOverrides() {
  if (process.env.CODESTORY_CLI) return;
  const overrides = readCursorLocalOverrides();
  if (overrides) process.env.CODESTORY_CLI = overrides.CODESTORY_CLI;
}

applyCursorLocalOverrides();

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
  if (platform === 'linux' && arch === 'x64') return 'linux-x64';
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

// The checksum file and the release manifest are both small metadata documents; only the archive
// gets the archive-sized ceiling and the archive-sized clock.
function releaseMetadataFile(name) {
  return name === 'SHA256SUMS.txt' || name === RELEASE_MANIFEST_ASSET;
}

function releaseFileMaxBytes(name) {
  return releaseMetadataFile(name) ? managedCliChecksumMaxBytes : managedCliArchiveMaxBytes;
}

function releaseFileTotalTimeoutMs(name) {
  return releaseMetadataFile(name) ? releaseChecksumTotalTimeoutMs : releaseArchiveTotalTimeoutMs;
}

// A single mutable record of what provisioning is currently doing. Tool calls answered while the
// runtime is still preparing read this so the client sees real download progress instead of a
// fixed "preparing" placeholder for the several minutes a large archive takes on a slow link.
const managedCliDownloadProgress = {
  stage: null,
  asset: null,
  attempt: 0,
  receivedBytes: 0,
  totalBytes: null,
  startedAt: null,
  updatedAt: null,
};

function publishManagedCliProgress() {
  if (!isMainThread && workerData?.codestoryMode === 'managed-provision' && parentPort) {
    parentPort.postMessage({ type: 'progress', progress: { ...managedCliDownloadProgress } });
  }
}

function applyManagedCliProgress(progress) {
  if (!progress || typeof progress !== 'object') return;
  for (const key of Object.keys(managedCliDownloadProgress)) {
    if (Object.hasOwn(progress, key)) managedCliDownloadProgress[key] = progress[key];
  }
}

function resetManagedCliDownloadProgress(stage, asset) {
  managedCliDownloadProgress.stage = stage;
  managedCliDownloadProgress.asset = asset;
  managedCliDownloadProgress.attempt = 0;
  managedCliDownloadProgress.receivedBytes = 0;
  managedCliDownloadProgress.totalBytes = null;
  managedCliDownloadProgress.startedAt = Date.now();
  managedCliDownloadProgress.updatedAt = Date.now();
  publishManagedCliProgress();
}

function recordManagedCliDownloadProgress(progress) {
  if (Number.isSafeInteger(progress.receivedBytes)) {
    managedCliDownloadProgress.receivedBytes = progress.receivedBytes;
  }
  // `totalBytes` is unknown at the start of each attempt; keep the last known value rather than
  // flickering the reported percentage back to null.
  if (Number.isSafeInteger(progress.totalBytes)) {
    managedCliDownloadProgress.totalBytes = progress.totalBytes;
  }
  if (Number.isSafeInteger(progress.attempt)) {
    managedCliDownloadProgress.attempt = progress.attempt;
  }
  managedCliDownloadProgress.updatedAt = Date.now();
  publishManagedCliProgress();
}

function formatByteSize(bytes) {
  if (!Number.isFinite(bytes) || bytes < 0) return null;
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

function managedCliDownloadProgressReport() {
  const { stage, asset, attempt, receivedBytes, totalBytes } = managedCliDownloadProgress;
  if (!stage) return null;
  const percent = Number.isSafeInteger(totalBytes) && totalBytes > 0
    ? Math.min(100, Math.floor((receivedBytes / totalBytes) * 100))
    : null;
  return {
    stage,
    asset,
    attempt: Math.max(1, attempt),
    received_bytes: receivedBytes,
    total_bytes: Number.isSafeInteger(totalBytes) ? totalBytes : null,
    percent,
  };
}

// Preparing responses derive their agent retry hint from the provisioning state this launcher
// actually observes instead of a fixed placeholder: a fixed delay makes agents busy-poll a
// multi-minute archive download and oversleep when readiness is imminent. While a transfer is
// measurable, the hint is the estimated remaining transfer time at the observed throughput. The
// clamp keeps degenerate estimates sane: the floor matches the runtime's own minimum activation
// retry delay, and the ceiling bounds how long an agent sleeps past a completion, failure, or
// stall the estimate could not foresee.
const provisioningRetryHintMinMs = 250;
const provisioningRetryHintMaxMs = 10000;
// Provisioning states carrying no measurable transfer keep the historical fixed hint.
const provisioningRetryHintFallbackMs = 1500;

function provisioningRetryHintMs(progress = managedCliDownloadProgress) {
  const { receivedBytes, totalBytes, startedAt, updatedAt } = progress;
  if (
    !Number.isSafeInteger(totalBytes) || totalBytes <= 0
    || !Number.isSafeInteger(receivedBytes) || receivedBytes <= 0
  ) {
    return provisioningRetryHintFallbackMs;
  }
  const remainingBytes = Math.max(0, totalBytes - receivedBytes);
  // Throughput is measured over the window that actually transferred the received bytes; a
  // wall-clock "now" would decay the rate during a stall and inflate the estimate open-endedly.
  const observedMs = Number.isFinite(startedAt) && Number.isFinite(updatedAt)
    ? updatedAt - startedAt
    : 0;
  if (remainingBytes > 0 && observedMs <= 0) return provisioningRetryHintFallbackMs;
  const estimatedRemainingMs = remainingBytes === 0 ? 0 : remainingBytes * (observedMs / receivedBytes);
  return Math.min(
    provisioningRetryHintMaxMs,
    Math.max(provisioningRetryHintMinMs, Math.round(estimatedRemainingMs)),
  );
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

// Download failures carry a structured tag rather than being re-derived from their message. Error
// text can embed untrusted or environment-specific detail, so anything that reaches a user-facing
// surface must be built from these allow-listed enums and numbers instead.
function downloadError(kind, message, extra = {}) {
  const error = new Error(message);
  error.downloadKind = kind;
  Object.assign(error, extra);
  return error;
}

function downloadFailureKind(error) {
  return typeof error?.downloadKind === 'string' ? error.downloadKind : 'network';
}

// Failures that cannot improve on a later attempt. Retrying these only burns the user's time
// before showing the same error, so the retry loop stops at the first one.
function downloadFailurePermanent(error) {
  const kind = downloadFailureKind(error);
  if (kind === 'publish') return error?.publishRetryable !== true;
  if (['size_limit', 'content_length', 'transport'].includes(kind)) return true;
  if (kind !== 'http_status') return false;
  const status = Number(error?.httpStatus);
  // 408/425/429 are explicitly "come back later"; every other 4xx is a fixed answer.
  return Number.isInteger(status) && status >= 400 && status < 500 &&
    ![408, 425, 429].includes(status);
}

const retryablePublishErrorCodes = new Set([
  'EACCES',
  'EBUSY',
  'EMFILE',
  'ENFILE',
  'ENOTEMPTY',
  'EPERM',
  'ETXTBSY',
]);

function publishError(errorOrCode) {
  const code = typeof errorOrCode === 'string'
    ? errorOrCode
    : String(errorOrCode?.code || 'unknown');
  return downloadError('publish', `download_publish_failed:${code}`, {
    publishCode: code,
    publishRetryable: retryablePublishErrorCodes.has(code),
  });
}

function parseContentRangeStart(header) {
  const match = /^bytes\s+(\d+)-(\d+)\/(\d+|\*)$/iu.exec(String(header || '').trim());
  if (!match) return null;
  const start = Number(match[1]);
  const total = match[3] === '*' ? null : Number(match[3]);
  if (!Number.isSafeInteger(start) || start < 0) return null;
  if (total !== null && (!Number.isSafeInteger(total) || total < 0)) return null;
  return { start, total };
}

function downloadFileOnce(url, destination, options = {}) {
  const stallTimeoutMs = options.stallTimeoutMs || releaseDownloadStallTimeoutMs;
  const timeoutMs = options.timeoutMs || releaseArchiveTotalTimeoutMs;
  const maxBytes = options.maxBytes ?? managedCliArchiveMaxBytes;
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    return Promise.reject(downloadError('size_limit', `download_size_limit_invalid:${maxBytes}`));
  }
  const resumeFrom = Number.isSafeInteger(options.resumeFrom) && options.resumeFrom > 0
    ? options.resumeFrom
    : 0;
  if (resumeFrom > maxBytes) {
    return Promise.reject(downloadError('size_limit', `download_size_limit_exceeded:${url}`));
  }
  const onProgress = typeof options.onProgress === 'function' ? options.onProgress : null;
  const deadlineMs = options.deadlineMs ?? Date.now() + timeoutMs;
  const redirectsRemaining = options.redirectsRemaining ?? 5;
  const parsedUrl = new URL(url);
  const loopbackHttp = parsedUrl.protocol === 'http:' &&
    ['127.0.0.1', '::1', '[::1]', 'localhost'].includes(parsedUrl.hostname);
  if (!options.get && parsedUrl.protocol !== 'https:' && !loopbackHttp) {
    return Promise.reject(downloadError('transport', 'download transport must be HTTPS'));
  }
  const get = options.get || (loopbackHttp ? http.get : https.get);
  return new Promise((resolve, reject) => {
    let settled = false;
    let output = null;
    let activeRequest = null;
    let activeResponse = null;
    let limiter = null;
    let stallTimer = null;
    // Bytes already on disk from earlier attempts, plus whatever this attempt appends.
    let downloadedBytes = resumeFrom;
    // Device and inode of the descriptor the bytes actually went into. Publication compares the
    // file standing at the partial path against this, so the name being swapped after the last
    // byte cannot substitute a different file for the one this transfer wrote.
    let partialIdentity = null;
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(deadlineTimer);
      if (stallTimer) clearTimeout(stallTimer);
      if (error) {
        if (limiter) limiter.destroy();
        if (output) output.destroy();
        if (activeResponse) activeResponse.destroy();
        if (activeRequest) activeRequest.destroy();
        error.downloadedBytes = downloadedBytes;
        reject(error);
      } else {
        resolve({ downloadedBytes, partial: partialIdentity });
      }
    };
    const armStall = () => {
      if (settled) return;
      if (stallTimer) clearTimeout(stallTimer);
      stallTimer = setTimeout(
        () => finish(downloadError('stalled', `download stalled after ${stallTimeoutMs}ms without data: ${url}`)),
        stallTimeoutMs,
      );
      // A stall timer must never hold the event loop open on its own.
      if (typeof stallTimer.unref === 'function') stallTimer.unref();
    };
    const remainingMs = Math.max(0, deadlineMs - Date.now());
    const deadlineTimer = setTimeout(
      () => finish(downloadError('timed_out', `download timed out after ${timeoutMs}ms total: ${url}`)),
      remainingMs,
    );
    const handleResponse = (response) => {
      activeResponse = response;
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        if (!response.headers.location || redirectsRemaining <= 0) {
          finish(downloadError('redirect', `download redirect failed: ${url}`));
          return;
        }
        const nextUrl = new URL(response.headers.location, url).toString();
        if (stallTimer) clearTimeout(stallTimer);
        downloadFileOnce(nextUrl, destination, {
          ...options,
          deadlineMs,
          redirectsRemaining: redirectsRemaining - 1,
        }).then((result) => {
          downloadedBytes = result?.downloadedBytes ?? downloadedBytes;
          partialIdentity = result?.partial ?? partialIdentity;
          finish(null);
        }, finish);
        return;
      }
      // 416 means the partial on disk is at least as long as the asset: it is unusable, and the
      // caller restarts from zero once the stale bytes are dropped.
      if (resumeFrom > 0 && response.statusCode === 416) {
        response.resume();
        finish(downloadError('range', `download_range_unsatisfiable:${url}`));
        return;
      }
      if (![200, 206].includes(response.statusCode)) {
        response.resume();
        finish(downloadError('http_status', `download failed ${response.statusCode}: ${url}`, { httpStatus: response.statusCode }));
        return;
      }
      // A server that ignores Range answers 200 with the whole body; the partial must be
      // discarded rather than appended to, or the file would be silently corrupt.
      let appendFrom = resumeFrom;
      if (response.statusCode === 206) {
        const range = parseContentRangeStart(response.headers['content-range']);
        if (!range || range.start !== resumeFrom) {
          response.resume();
          finish(downloadError('range', `download_range_mismatch:${url}`));
          return;
        }
      } else {
        appendFrom = 0;
      }
      downloadedBytes = appendFrom;

      const announced = response.headers['content-length'];
      let totalBytes = null;
      if (announced !== undefined) {
        const contentLength = Number(announced);
        if (!Number.isSafeInteger(contentLength) || contentLength < 0) {
          response.resume();
          finish(downloadError('content_length', `download_content_length_invalid:${url}`));
          return;
        }
        totalBytes = appendFrom + contentLength;
        if (totalBytes > maxBytes) {
          response.resume();
          finish(downloadError('size_limit', `download_size_limit_exceeded:${url}`));
          return;
        }
      }
      if (onProgress) onProgress({ receivedBytes: appendFrom, totalBytes });

      limiter = new Transform({
        transform(chunk, _encoding, callback) {
          downloadedBytes += chunk.length;
          if (downloadedBytes > maxBytes) {
            callback(downloadError('size_limit', `download_size_limit_exceeded:${url}`));
            return;
          }
          armStall();
          if (onProgress) onProgress({ receivedBytes: downloadedBytes, totalBytes });
          callback(null, chunk);
        },
      });
      // Open the partial through an explicit no-follow descriptor rather than by path. The stat that
      // chose `appendFrom` happened earlier, so a symlink planted in between would otherwise still be
      // followed here. `O_NOFOLLOW` refuses a symlink and `O_NONBLOCK` refuses to block on a fifo,
      // but neither says anything about a *hard link*: an extra name for a file outside the cache is
      // indistinguishable from our own partial by path. So the descriptor itself is re-checked
      // before a byte is written, and only a lone regular file is accepted.
      // Both flags are `0` on Windows, where the fstat below is the whole guard: it still refuses a
      // hard link (which Windows creates without privilege) but it cannot refuse a symlink planted
      // inside this window, so that one case stays open there.
      let partialFd;
      try {
        partialFd = fs.openSync(
          destination,
          fs.constants.O_WRONLY | fs.constants.O_CREAT |
            (appendFrom > 0 ? fs.constants.O_APPEND : 0) |
            (fs.constants.O_NOFOLLOW || 0) | (fs.constants.O_NONBLOCK || 0),
          0o600,
        );
      } catch (error) {
        response.resume();
        finish(downloadError(
          'partial_open',
          `download_partial_open_failed:${error?.code || 'unknown'}`,
        ));
        return;
      }
      try {
        const opened = fs.fstatSync(partialFd);
        if (!opened.isFile() || opened.nlink !== 1) {
          throw downloadError(
            'partial_open',
            `download_partial_open_failed:${opened.isFile() ? 'linked' : 'not_regular'}`,
          );
        }
        // Truncation happens on the already-verified descriptor instead of through `O_TRUNC`, so a
        // hard link planted at the partial path is refused above rather than emptied on the way in.
        if (appendFrom === 0) fs.ftruncateSync(partialFd, 0);
        partialIdentity = { dev: opened.dev, ino: opened.ino };
      } catch (error) {
        try {
          fs.closeSync(partialFd);
        } catch {
          // The descriptor is being abandoned either way.
        }
        response.resume();
        finish(downloadFailureKind(error) === 'partial_open'
          ? error
          : downloadError('partial_open', `download_partial_open_failed:${error?.code || 'unknown'}`));
        return;
      }
      output = fs.createWriteStream(destination, { fd: partialFd });
      pipeline(response, limiter, output, (error) => finish(error || null));
    };
    // Only pass a request-options object when a Range header is actually needed: the two-argument
    // form is what injected `get` doubles in tests implement.
    const request = resumeFrom > 0
      ? get(url, { headers: { Range: `bytes=${resumeFrom}-` } }, handleResponse)
      : get(url, handleResponse);
    activeRequest = request;
    request.on('error', finish);
    armStall();
  });
}

// Publication reads the partial through a descriptor, not a path, because the last window in the
// transfer is between the final byte and the rename: swap the partial for a link there and a plain
// `rename` moves the link into place as the "archive". `identity` is the device/inode the transfer
// actually wrote, so anything else standing at that name is refused before it can be published.
function openVerifiedPartial(partialPath, identity) {
  let fd;
  try {
    fd = fs.openSync(
      partialPath,
      fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0) | (fs.constants.O_NONBLOCK || 0),
    );
  } catch (error) {
    throw publishError(error);
  }
  try {
    const opened = fs.fstatSync(fd);
    const ours = opened.isFile() && opened.nlink === 1 &&
      (!identity || (opened.dev === identity.dev && opened.ino === identity.ino));
    if (!ours) throw publishError('partial_identity');
    return { fd, metadata: opened };
  } catch (error) {
    try {
      fs.closeSync(fd);
    } catch {
      // The descriptor is being abandoned either way.
    }
    throw downloadFailureKind(error) === 'publish'
      ? error
      : publishError(error);
  }
}

function descriptorsHaveSameContent(leftFd, leftMetadata, rightFd, rightMetadata) {
  if (leftMetadata.size !== rightMetadata.size) return false;
  const left = Buffer.allocUnsafe(1024 * 1024);
  const right = Buffer.allocUnsafe(left.length);
  let position = 0;
  while (position < leftMetadata.size) {
    const wanted = Math.min(left.length, leftMetadata.size - position);
    let leftBytes = 0;
    let rightBytes = 0;
    while (leftBytes < wanted) {
      const read = fs.readSync(leftFd, left, leftBytes, wanted - leftBytes, position + leftBytes);
      if (read <= 0) return false;
      leftBytes += read;
    }
    while (rightBytes < wanted) {
      const read = fs.readSync(rightFd, right, rightBytes, wanted - rightBytes, position + rightBytes);
      if (read <= 0) return false;
      rightBytes += read;
    }
    if (!left.subarray(0, wanted).equals(right.subarray(0, wanted))) return false;
    position += wanted;
  }
  return true;
}

function openReplaceableDownloadDestination(destination) {
  let fd;
  try {
    fd = fs.openSync(
      destination,
      fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0) | (fs.constants.O_NONBLOCK || 0),
    );
  } catch (error) {
    if (error?.code === 'ENOENT') return null;
    if (error?.code === 'ELOOP') throw publishError('destination_not_replaceable');
    throw publishError(error);
  }
  try {
    const opened = fs.fstatSync(fd);
    const named = fs.lstatSync(destination);
    if (
      !opened.isFile() || opened.nlink !== 1 || !named.isFile() || named.nlink !== 1 ||
      opened.dev !== named.dev || opened.ino !== named.ino
    ) {
      throw publishError('destination_not_replaceable');
    }
    return { fd, metadata: opened };
  } catch (error) {
    try {
      fs.closeSync(fd);
    } catch {
      // The descriptor is being abandoned either way.
    }
    throw downloadFailureKind(error) === 'publish' ? error : publishError(error);
  }
}

function unlinkNamedFileIfIdentity(filePath, identity) {
  const named = fs.lstatSync(filePath);
  if (
    !named.isFile() || named.nlink !== 1 ||
    named.dev !== identity.dev || named.ino !== identity.ino
  ) {
    throw publishError('destination_identity');
  }
  fs.unlinkSync(filePath);
}

// Production reaches this only while holding the managed provisioning lock. A retained cache
// destination may be reused byte-for-byte; otherwise rename directly over the verified regular
// file. Node/libuv gives that operation old-or-new atomicity on supported platforms: a lock may
// make the rename fail, but the old destination and completed source both remain for retry. Never
// pre-unlink the destination, because a rename failure after that would expose a missing archive.
function replaceOrReuseDownloadedFile(
  sourcePath,
  destination,
  sourceFd,
  sourceMetadata,
  sourceIdentity = sourceMetadata,
) {
  const existing = openReplaceableDownloadDestination(destination);
  if (!existing) {
    try {
      fs.renameSync(sourcePath, destination);
      return false;
    } catch (error) {
      throw publishError(error);
    }
  }
  let reuse;
  try {
    reuse = descriptorsHaveSameContent(sourceFd, sourceMetadata, existing.fd, existing.metadata);
  } catch (error) {
    throw publishError(error);
  } finally {
    try {
      fs.closeSync(existing.fd);
    } catch {
      // The descriptor is no longer needed once comparison is complete.
    }
  }
  if (reuse) {
    try {
      unlinkNamedFileIfIdentity(sourcePath, sourceIdentity);
    } catch {
      // The retained destination already has the exact completed bytes. A raced source name is
      // left alone rather than turning successful reuse into deletion of an unrelated path.
    }
    return true;
  }
  try {
    fs.renameSync(sourcePath, destination);
    return false;
  } catch (error) {
    throw downloadFailureKind(error) === 'publish' ? error : publishError(error);
  }
}

// Copies from the verified descriptor rather than re-opening the partial by name, so the
// cross-device path publishes the same bytes the same-device path would. The sibling staging file
// is created with `O_EXCL`, flushed, and then renamed over the destination atomically.
function copyVerifiedPartial(fd, destination, options = {}) {
  const staging = `${destination}.publish-${process.pid}-${randomBytes(6).toString('hex')}.tmp`;
  const writeSync = options.writeSync || fs.writeSync;
  let out;
  let stagingMetadata;
  try {
    out = fs.openSync(
      staging,
      fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_EXCL | (fs.constants.O_NOFOLLOW || 0),
      0o600,
    );
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let position = 0;
    for (;;) {
      const read = fs.readSync(fd, buffer, 0, buffer.length, position);
      if (read <= 0) break;
      let written = 0;
      while (written < read) {
        const progress = writeSync(out, buffer, written, read - written);
        if (!Number.isSafeInteger(progress) || progress <= 0 || progress > read - written) {
          throw Object.assign(new Error('download_publish_short_write'), { code: 'EIO' });
        }
        written += progress;
      }
      position += read;
    }
    fs.fsyncSync(out);
    stagingMetadata = fs.fstatSync(out);
    const completedOut = out;
    out = undefined;
    fs.closeSync(completedOut);
    replaceOrReuseDownloadedFile(
      staging,
      destination,
      fd,
      fs.fstatSync(fd),
      stagingMetadata,
    );
  } finally {
    if (out !== undefined) fs.closeSync(out);
    fs.rmSync(staging, { force: true });
  }
}

// `rename` cannot cross filesystems, and the partial deliberately lives under the managed CLI root
// so it survives a restart while the caller's destination may sit in a temp directory on another
// mount. Falling back to copy-then-unlink keeps publication correct wherever the two land.
function publishDownloadedFile(partialPath, destination, identity = null) {
  const { fd, metadata } = openVerifiedPartial(partialPath, identity);
  try {
    let reused = false;
    try {
      reused = replaceOrReuseDownloadedFile(partialPath, destination, fd, metadata);
    } catch (error) {
      if (error?.publishCode !== 'EXDEV') throw error;
      try {
        copyVerifiedPartial(fd, destination);
        fs.rmSync(partialPath, { force: true });
      } catch (copyError) {
        throw downloadFailureKind(copyError) === 'publish'
          ? copyError
          : publishError(copyError);
      }
      return;
    }
    if (reused) return;
    // A rename keeps the inode, so the published name must still be the verified file. If it is
    // not, something replaced the partial between the check and the rename: drop what landed
    // instead of handing a foreign file to the checksum step as this release's archive.
    const published = fs.lstatSync(destination);
    if (!published.isFile() || published.dev !== metadata.dev || published.ino !== metadata.ino) {
      fs.rmSync(destination, { force: true });
      throw publishError('published_identity');
    }
  } finally {
    try {
      fs.closeSync(fd);
    } catch {
      // Closing a descriptor whose file is already published or discarded changes nothing.
    }
  }
}

// The partial is the one attacker-reachable name in the provisioning path, so it is measured with
// `lstat`: a symlink planted there would otherwise report the target's size and make the transfer
// resume *through* the link into a file outside the managed cache. A hard link passes `isFile()`
// and is just as available to a same-user attacker, so a partial with more than one name is
// refused too. Anything but a lone regular file is unlinked here and the transfer restarts from
// zero; a directory cannot be unlinked this way and instead fails the no-follow open below.
function partialDownloadBytes(partialPath) {
  let metadata = null;
  try {
    metadata = fs.lstatSync(partialPath);
  } catch {
    return 0;
  }
  if (metadata.isFile() && metadata.nlink === 1) return metadata.size;
  try {
    fs.rmSync(partialPath, { force: true });
  } catch {
    // Best effort. The no-follow open and its fstat are what actually refuse to write through a
    // link that survives here.
  }
  return 0;
}

// Downloads into `<destination>.part` and only publishes `destination` once the transfer
// completes, so a failed run leaves a resumable partial instead of a truncated asset. When the
// partial lives in a persistent cache the resume also survives an MCP restart, which is what
// turns a repeatedly interrupted first run into forward progress.
async function downloadFile(url, destination, options = {}) {
  const attempts = options.attempts || releaseDownloadAttempts;
  const startedAt = Date.now();
  const totalTimeoutMs = options.timeoutMs || releaseArchiveTotalTimeoutMs;
  const deadlineMs = startedAt + totalTimeoutMs;
  const partialPath = options.partialPath || `${destination}.part`;
  const onProgress = typeof options.onProgress === 'function' ? options.onProgress : null;
  const purgePartial = () => fs.rmSync(partialPath, { force: true });
  // `attempts` is a strict cap. Because every attempt resumes rather than restarting, attempts are
  // cheap, so the default is generous and the real bound on a doomed download is the total
  // deadline. `stalledAttempts` only picks the backoff delay: an attempt that moved the partial
  // forward resets it, so a link that is working is not punished with the long waits.
  let lastError = null;
  let attempt = 0;
  let stalledAttempts = 0;
  let completedTransfer = null;
  while (attempt < attempts) {
    attempt += 1;
    const resumeFrom = partialDownloadBytes(partialPath);
    if (onProgress) onProgress({ receivedBytes: resumeFrom, attempt });
    try {
      if (!completedTransfer) {
        completedTransfer = await downloadFileOnce(url, partialPath, {
          ...options,
          resumeFrom,
          deadlineMs,
          onProgress: onProgress
            ? (progress) => onProgress({ ...progress, attempt })
            : undefined,
        });
      }
      publishDownloadedFile(partialPath, destination, completedTransfer?.partial || null);
      return;
    } catch (error) {
      lastError = error;
      const failureKind = downloadFailureKind(error);
      if (downloadFailurePermanent(error)) {
        // Publication happens only after the transfer is complete. Preserve
        // those verified bytes on a terminal local publish failure; unlike a
        // malformed response, they remain useful evidence and retry input.
        if (failureKind !== 'publish') purgePartial();
        break;
      }
      // A partial that the server rejects or that drifted out of sync is worthless; drop it so the
      // next attempt starts clean instead of failing the same way forever.
      if (failureKind === 'range') purgePartial();
      if (failureKind !== 'publish') completedTransfer = null;
      const advanced = partialDownloadBytes(partialPath) > resumeFrom;
      stalledAttempts = advanced ? 0 : stalledAttempts + 1;
      if (Date.now() >= deadlineMs || attempt >= attempts) break;
      const index = Math.max(0, stalledAttempts - 1);
      const delayMs = options.retryDelayMs
        ? options.retryDelayMs(attempt)
        : (releaseDownloadRetryDelaysMs[index] ||
          releaseDownloadRetryDelaysMs[releaseDownloadRetryDelaysMs.length - 1]) +
          Math.floor(Math.random() * releaseDownloadRetryJitterMs);
      const budgetedDelayMs = Math.max(0, Math.min(delayMs, deadlineMs - Date.now()));
      if (budgetedDelayMs > 0) await sleep(budgetedDelayMs);
    }
  }
  const elapsedMs = Date.now() - startedAt;
  const resumableBytes = partialDownloadBytes(partialPath);
  const resumeNote = resumableBytes > 0 ? ` resumable_bytes=${resumableBytes}` : '';
  const failure = new Error(
    `download failed after ${attempt} attempts over ${elapsedMs}ms:${resumeNote} ${lastError?.message || 'unknown error'}`,
  );
  failure.downloadFailure = {
    kind: downloadFailureKind(lastError),
    http_status: Number.isInteger(lastError?.httpStatus) ? lastError.httpStatus : null,
    resumable_bytes: resumableBytes,
    elapsed_ms: elapsedMs,
    attempts: attempt,
  };
  throw failure;
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

async function fetchReleaseFile(version, name, destination, options = {}) {
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
  // One stage name for the whole release fetch keeps the reported stage deterministic for clients;
  // `asset` in the progress detail distinguishes the checksum file from the archive.
  resetManagedCliDownloadProgress('downloading_runtime', name);
  try {
    await downloadFile(url, destination, {
      maxBytes,
      timeoutMs: releaseFileTotalTimeoutMs(name),
      partialPath: options.partialPath,
      onProgress: recordManagedCliDownloadProgress,
    });
  } catch (error) {
    const wrapped = new Error(releaseAssetFetchFailure(name, startedAt, releaseDownloadAttempts, error));
    if (error?.downloadFailure) {
      wrapped.downloadFailure = { ...error.downloadFailure, asset: name };
    }
    throw wrapped;
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

function reclaimStaleManagedCliInitialization(
  lockPath,
  checkProcessIdentity = true,
  processStartIdentityFor = processStartIdentity,
) {
  const initializationPath = `${lockPath}.initializing`;
  return removeManagedCliInitializationIf(initializationPath, (owner, metadata) => {
    const stale = managedCliLockOwnerIsStale(
      owner,
      checkProcessIdentity,
      processStartIdentityFor,
    );
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

function reclaimStaleManagedCliLock(
  lockPath,
  checkProcessIdentity = true,
  processStartIdentityFor = processStartIdentity,
) {
  const ownerPath = path.join(lockPath, 'owner.json');
  const owner = readJson(ownerPath);
  const initializationOwner = owner ? null : readJson(`${lockPath}.initializing`);
  let stale = managedCliLockOwnerIsStale(
    owner || initializationOwner,
    checkProcessIdentity,
    processStartIdentityFor,
  );
  if (stale === null) {
    try {
      stale = Date.now() - fs.statSync(lockPath).mtimeMs > managedCliLockStaleMs;
    } catch {
      return false;
    }
  }
  if (!stale) return false;
  const removed = removeManagedCliLockArtifact(lockPath);
  if (removed) {
    reclaimStaleManagedCliInitialization(
      lockPath,
      checkProcessIdentity,
      processStartIdentityFor,
    );
  }
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
  const selfIdentity = options.selfIdentity ?? processStartIdentityFor(process.pid);
  if (!selfIdentity) throw new Error('managed_cli_process_identity_unavailable');
  const nowFor = options.now || Date.now;
  const identityProbeThrottle = options.identityProbeThrottle || { nextIdentityCheckAt: 0 };
  const owner = {
    pid: process.pid,
    purpose,
    token,
    process_start_identity: selfIdentity,
    started_at: new Date().toISOString(),
  };
  const pendingOwnerPath = `${lockPath}.owner-${process.pid}-${token}`;
  const deadline = nowFor() + waitMs;
  let waited = false;
  let reclaimed = false;
  let firstAttempt = true;
  fs.writeFileSync(pendingOwnerPath, JSON.stringify(owner), { flag: 'wx', mode: 0o600 });
  try {
    while (true) {
      const now = nowFor();
      const nextIdentityCheckAt = Number.isFinite(identityProbeThrottle.nextIdentityCheckAt)
        ? identityProbeThrottle.nextIdentityCheckAt
        : 0;
      const checkProcessIdentity = now >= nextIdentityCheckAt;
      if (checkProcessIdentity) {
        identityProbeThrottle.nextIdentityCheckAt = now + managedCliIdentityProbeIntervalMs;
      }
      if (firstAttempt || checkProcessIdentity) {
        reclaimStaleManagedCliPendingOwners(
          root,
          checkProcessIdentity,
          processStartIdentityFor,
        );
        firstAttempt = false;
      }
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
        reclaimStaleManagedCliPendingOwners(root, false, processStartIdentityFor);
        return { lockPath, token, waited, reclaimed };
      } catch (error) {
        if (ownsInitialization && !publishedOwner) {
          releaseManagedCliInitialization(lockPath, owner);
        }
        if (error.code !== 'EEXIST') throw error;
        waited = true;
        if (
          reclaimStaleManagedCliLock(
            lockPath,
            checkProcessIdentity,
            processStartIdentityFor,
          ) ||
          reclaimStaleManagedCliInitialization(
            lockPath,
            checkProcessIdentity,
            processStartIdentityFor,
          )
        ) {
          reclaimed = true;
          continue;
        }
        if (now >= deadline) return null;
        sleepSync(50);
      }
    }
  } finally {
    fs.rmSync(pendingOwnerPath, { force: true });
  }
}

async function acquireManagedCliLockAsync(root, purpose, waitMs, options = {}) {
  const nowFor = options.now || Date.now;
  const sleepFor = options.sleep || sleep;
  const processStartIdentityFor = options.processStartIdentity || processStartIdentity;
  const selfIdentity = processStartIdentityFor(process.pid);
  if (!selfIdentity) throw new Error('managed_cli_process_identity_unavailable');
  const identityProbeThrottle = { nextIdentityCheckAt: 0 };
  const deadline = nowFor() + waitMs;
  let waited = false;
  while (true) {
    const lock = acquireManagedCliLock(root, purpose, 0, {
      ...options,
      identityProbeThrottle,
      now: nowFor,
      processStartIdentity: processStartIdentityFor,
      selfIdentity,
    });
    if (lock) return { ...lock, waited: waited || lock.waited };
    waited = true;
    const remaining = deadline - nowFor();
    if (remaining <= 0) return null;
    await sleepFor(Math.min(50, remaining));
  }
}

function safeFailureToken(value, fallback) {
  return String(value ?? '').match(/^[a-z0-9_]+/iu)?.[0]?.slice(0, 64) || fallback;
}

function managedCliVersionProbeFailure(probeOrReason, expectedVersion) {
  let kind;
  let detail;
  if (typeof probeOrReason === 'string') {
    const separator = probeOrReason.indexOf(':');
    kind = separator < 0 ? probeOrReason : probeOrReason.slice(0, separator);
    detail = separator < 0 ? null : probeOrReason.slice(separator + 1);
  } else if (probeOrReason?.error) {
    kind = 'version_probe_error';
    detail = probeOrReason.errorCode;
  } else if (probeOrReason?.status !== 0) {
    kind = 'version_probe_exit';
    detail = probeOrReason?.status;
  } else if (probeOrReason?.version !== expectedVersion) {
    kind = 'version_probe_mismatch';
  } else {
    return null;
  }

  if (kind === 'version_probe_error') {
    return `${kind}:${safeFailureToken(detail, 'unknown')}`;
  }
  if (kind === 'version_probe_exit') {
    const status = safeFailureToken(detail, 'unknown');
    const exitCode = Number(status);
    return `${kind}:${Number.isSafeInteger(exitCode) && exitCode > 0 ? exitCode : 'unknown'}`;
  }
  return kind === 'version_probe_mismatch' ? kind : null;
}

function managedCliFailureCode(error) {
  const message = String(error?.message || error || 'unknown_failure');
  const code = safeFailureToken(message, 'unknown_failure');
  if (code !== 'managed_cli_staging_verification_failed') return code;
  const probeFailure = managedCliVersionProbeFailure(message.slice(code.length + 1));
  return probeFailure ? `${code}:${probeFailure}` : code;
}

// The machine-readable failure code is deliberately reduced to a single safe token, which left the
// user staring at `managed_cli_asset_fetch_failed` with no idea that their download had simply been
// cut short. The context and hint below restore that explanation without reopening the sanitization
// hole: both are derived only from the structured `downloadFailure` tag and the already-sanitized
// failure code, never from raw error text (which can carry untrusted child-process output).
const managedCliProvisionFailure = { code: null, context: null, hint: null };

const downloadFailureKinds = new Set([
  'stalled',
  'timed_out',
  'http_status',
  'size_limit',
  'content_length',
  'transport',
  'range',
  'redirect',
  'partial_open',
  'publish',
  'network',
]);

function sanitizeDownloadFailure(failure) {
  if (!failure || typeof failure !== 'object') return null;
  const kind = downloadFailureKinds.has(failure.kind) ? failure.kind : 'network';
  const safeInt = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : null);
  return {
    kind,
    // The asset name is one of our own fixed release filenames, never user or server supplied.
    asset: typeof failure.asset === 'string' && /^[\w.+-]{1,128}$/u.test(failure.asset)
      ? failure.asset
      : null,
    http_status: Number.isInteger(failure.http_status) &&
      failure.http_status >= 100 && failure.http_status <= 599
      ? failure.http_status
      : null,
    resumable_bytes: safeInt(failure.resumable_bytes) ?? 0,
    elapsed_ms: safeInt(failure.elapsed_ms) ?? 0,
    attempts: safeInt(failure.attempts) ?? 0,
  };
}

const manualInstallHint =
  'To skip the download, install codestory-cli yourself and point CODESTORY_CLI at it.';

function managedCliDownloadHint(context, code) {
  if (code === 'archive_checksum_mismatch') {
    return 'The runtime archive failed checksum verification and was discarded. ' +
      'Retry the tool to download it again.';
  }
  if (!context) return null;
  const resumeNote = context.resumable_bytes > 0
    ? ` ${formatByteSize(context.resumable_bytes)} already downloaded is kept, and retrying resumes from there.`
    : '';
  switch (context.kind) {
    case 'stalled':
    case 'timed_out':
    case 'network':
      return 'The CodeStory runtime download could not complete over this connection.' +
        `${resumeNote} Retry the tool to continue downloading, or raise the budget with ` +
        `CODESTORY_PLUGIN_DOWNLOAD_TIMEOUT_MS / CODESTORY_PLUGIN_DOWNLOAD_STALL_TIMEOUT_MS. ${manualInstallHint}`;
    case 'http_status':
      if (context.http_status === 404) {
        return 'The release asset for this plugin version was not found. ' +
          `Update the plugin, or install codestory-cli yourself and point CODESTORY_CLI at it.`;
      }
      return `The release download was rejected (HTTP ${context.http_status ?? 'error'}). ` +
        `Retry later, or ${manualInstallHint.charAt(0).toLowerCase()}${manualInstallHint.slice(1)}`;
    case 'size_limit':
    case 'content_length':
      return `The release asset did not match its expected size bounds. ${manualInstallHint}`;
    case 'transport':
      return `The release download was blocked because it was not served over HTTPS. ${manualInstallHint}`;
    case 'range':
    case 'redirect':
    case 'partial_open':
      return 'The release download could not be resumed and was reset. Retry the tool to start it again.';
    case 'publish':
      return 'The runtime download completed, but the local publish step could not finish.' +
        `${resumeNote} Retry the tool to publish it again. ${manualInstallHint}`;
    default:
      return null;
  }
}

function recordManagedCliProvisionFailure(warnings, error) {
  const code = managedCliFailureCode(error);
  const failure = `managed_cli_provision_failed:${code}`;
  const context = sanitizeDownloadFailure(error?.downloadFailure);
  managedCliProvisionFailure.code = code;
  managedCliProvisionFailure.context = context;
  managedCliProvisionFailure.hint = managedCliDownloadHint(context, code);
  warnings.push(`managed_cli_publication:terminal_failure:${code}`, failure);
  return failure;
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
    !/^[0-9a-f]{64}$/iu.test(String(manifest.archive_sha256 || '')) ||
    !Number.isSafeInteger(manifest.archive_bytes) ||
    manifest.archive_bytes <= 0
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

// Mirrors codestory_contracts::wire::negotiate_mcp_protocol_version. The
// launcher answers `initialize` itself and suppresses the CLI's answer, so an
// echoed revision here would be a false compatibility claim the host can never
// see corrected.
function negotiateMcpProtocolVersion(requested) {
  const asked = typeof requested === 'string' ? requested.trim() : '';
  if (!asked) {
    return {
      requested: null,
      negotiated: managedCliMcpProtocolVersion,
      supported: [...supportedMcpProtocolVersions],
      status: 'defaulted',
      compatible: true,
    };
  }
  const agreed = supportedMcpProtocolVersions.includes(asked);
  return {
    requested: asked,
    negotiated: agreed ? asked : managedCliMcpProtocolVersion,
    supported: [...supportedMcpProtocolVersions],
    status: agreed ? 'agreed' : 'unsupported_client_revision',
    compatible: agreed,
  };
}

// Mirrors codestory_contracts::wire::classify_publication_stamp. Returns a skew
// token, or null when the stamp is inside the mutually supported window.
function publicationStampSkew(stamp) {
  if (!isPlainObject(stamp)) return 'publication_stamp_legacy_v0';
  const observed = stamp.schema_version;
  if (!Number.isSafeInteger(observed) || observed < 0) return 'publication_stamp_malformed';
  if (observed === 0) return 'publication_stamp_legacy_v0';
  if (observed > publicationStampSchemaVersion) return 'publication_stamp_producer_too_new';
  if (observed < minimumCompatiblePublicationStampSchemaVersion) {
    return 'publication_stamp_producer_too_old';
  }
  const producerMinimum = stamp.minimum_compatible_schema_version;
  if (Number.isSafeInteger(producerMinimum) && producerMinimum > publicationStampSchemaVersion) {
    return 'publication_stamp_producer_too_new';
  }
  return null;
}

function publicationStampText(value) {
  const text = typeof value === 'string' ? value.trim() : '';
  return text ? text : null;
}

// Mirrors `codestory_cli::runtime::codestory_publication_meta` for the one frame
// the packaged path never delegates. The launcher answers `initialize` itself
// and suppresses the runtime's own answer, so this is the only stamp a host
// behind either package MCP config can read at handshake; without it the
// packaged handshake is indistinguishable from a legacy v0 producer no matter
// which contract the pinned runtime implements. The launcher authors the frame,
// so the stamp describes the launcher's own knowledge: no publication identity
// exists at session start, hence `served_from=contract_only`.
function failOpenPublicationStamp(status) {
  const plugin = isPlainObject(status?.plugin_runtime) ? status.plugin_runtime : {};
  const cliVersion = publicationStampText(status?.cli_version);
  // The launcher-provided half of the pinned pair: exactly the value
  // `stdioRuntimeEnv` hands the runtime as `CODESTORY_PLUGIN_CLI_VERSION`.
  const pluginCliVersion = publicationStampText(plugin.plugin_cli_version);
  // `launcher` is the existing source token for "the launcher itself, with no
  // resolved CLI behind it"; the fallback stays inside that vocabulary rather
  // than inventing a value a consumer has never been told about.
  const cliSource = publicationStampText(plugin.cli_source) || 'launcher';
  return {
    schema_version: publicationStampSchemaVersion,
    minimum_compatible_schema_version: minimumCompatiblePublicationStampSchemaVersion,
    served_from: 'contract_only',
    publication: null,
    core_publication: null,
    retrieval_publication: null,
    contract_runtime: {
      cli_version: cliVersion,
      plugin_version: publicationStampText(plugin.plugin_version),
      plugin_cli_version: pluginCliVersion,
      cli_sha256: publicationStampText(plugin.cli_sha256),
      cli_source: cliSource,
      // `null` is "cannot compare", not "mismatch": the launcher answers
      // `initialize` before any runtime is required to exist, so an unresolved
      // CLI must not be reported as a failed pin.
      pinned_pair_matches: pluginCliVersion === null || cliVersion === null
        ? null
        : pluginCliVersion === cliVersion,
      known_override_skew_channel: Boolean(publicationStampText(process.env.CODESTORY_CLI))
        || cliSource === 'local_dev_override',
    },
    operation: { operation_id: null, attempt: null },
  };
}

// The pair check the `CODESTORY_CLI` override otherwise bypasses: the runtime's
// own `initialize` result must agree with the revision the launcher already
// promised the host and must stamp a publication schema this launcher can read.
function runtimeWireContractSkew(response, negotiatedProtocolVersion) {
  if (!isPlainObject(response)) return 'initialize_response_invalid';
  if (response.error !== undefined) return 'initialize_rejected';
  const result = response.result;
  if (!isPlainObject(result)) return 'initialize_result_invalid';
  if (result.protocolVersion !== negotiatedProtocolVersion) return 'protocol_version_skew';
  return publicationStampSkew(result._meta?.codestory_publication);
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
      // Provisioning is where the pinned pair is established, so an archive
      // whose runtime publishes a wire contract this launcher cannot read is
      // never staged.
      const stampSkew = publicationStampSkew(response.result._meta?.codestory_publication);
      if (stampSkew) {
        terminate(new Error(`managed_cli_stdio_initialize_wire_contract:${stampSkew}`));
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

// Partial archives live under the managed root rather than an ephemeral temp dir so an interrupted
// first run resumes after an MCP restart instead of starting the whole transfer over. The name is
// dot-prefixed, so version enumeration and retention already skip it.
// Every path that reads or deletes inside the download cache resolves it through here first. The
// cache is recursively deleted from, so a symlinked `.download` would make provisioning delete
// through it into whatever it points at.
function managedCliDownloadCacheRoot(root) {
  const cacheRoot = path.join(root, managedCliDownloadCacheDirName);
  if (fs.existsSync(cacheRoot)) {
    const metadata = fs.lstatSync(cacheRoot);
    if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
      throw new Error('managed_cli_download_cache_not_direct');
    }
  }
  return cacheRoot;
}

// The per-version directory needs the same guard as the cache root, and needs it after the mkdir:
// `mkdirSync({ recursive: true })` on an existing symlink-to-directory succeeds silently, and the
// no-follow open that protects the partial only constrains the final path component. A symlinked
// version entry would therefore route every provisioning byte outside the cache with no race at
// all. Provisioning treats a throw here as "no cache" and falls back to its temp directory, so
// refusing costs resume rather than correctness.
function managedCliDownloadCacheDir(root, version) {
  const cacheRoot = managedCliDownloadCacheRoot(root);
  const dir = path.join(cacheRoot, version);
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  const metadata = fs.lstatSync(dir);
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new Error('managed_cli_download_cache_not_direct');
  }
  // The lstat above covers the entry itself; resolving both ends also catches a link deeper in the
  // version name and pins the answer to a path that really sits inside the cache.
  const resolved = fs.realpathSync(dir);
  const resolvedRoot = fs.realpathSync(cacheRoot);
  if (resolved !== resolvedRoot && !resolved.startsWith(resolvedRoot + path.sep)) {
    throw new Error('managed_cli_download_cache_not_direct');
  }
  return resolved;
}

function trimManagedCliDownloadCache(root, version) {
  let cacheRoot;
  let children;
  try {
    cacheRoot = managedCliDownloadCacheRoot(root);
    children = fs.readdirSync(cacheRoot, { withFileTypes: true });
  } catch {
    return;
  }
  for (const child of children) {
    // A symlinked version entry is not a partial this process created, so it is never ours to
    // delete through.
    if (child.isSymbolicLink()) continue;
    const childPath = path.join(cacheRoot, child.name);
    try {
      // Partials for a version we are no longer provisioning can never be resumed, and even the
      // current version's partial goes stale once the release has had time to be re-cut.
      const expired = child.name !== version ||
        Date.now() - fs.statSync(childPath).mtimeMs > managedCliDownloadCacheMaxAgeMs;
      if (expired) fs.rmSync(childPath, { recursive: true, force: true });
    } catch {
      // Best effort: a partial we cannot inspect is simply re-downloaded.
    }
  }
}

function removeManagedCliDownloadCache(root, version) {
  try {
    const versionDir = path.join(managedCliDownloadCacheRoot(root), version);
    if (fs.lstatSync(versionDir).isSymbolicLink()) return;
    fs.rmSync(versionDir, { recursive: true, force: true });
  } catch {
    // Best effort: a leftover partial is trimmed on the next provisioning run.
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
    const extractDir = path.join(tempRoot, 'extract');
    trimManagedCliDownloadCache(root, version);
    // Keep the completed archive on the same filesystem as its own partial so publication is a
    // same-directory rename. The temp root frequently sits on a different mount from the managed
    // root, and a cross-device rename fails after the whole transfer has already succeeded.
    let archivePath = path.join(tempRoot, asset);
    let archivePartialPath = `${archivePath}.part`;
    try {
      const downloadCacheDir = managedCliDownloadCacheDir(root, version);
      archivePath = path.join(downloadCacheDir, asset);
      archivePartialPath = path.join(downloadCacheDir, `${asset}.part`);
    } catch (error) {
      // A cache we cannot use costs resume, not correctness: fall back to the temp dir.
      warnings.push(`managed_cli_publication:download_cache_unavailable:${managedCliFailureCode(error)}`);
    }
    const resumeBytes = partialDownloadBytes(archivePartialPath);
    if (resumeBytes > 0) warnings.push(`managed_cli_publication:resume_bytes:${resumeBytes}`);
    await fetchReleaseFile(version, 'SHA256SUMS.txt', sumsPath);
    // Both metadata documents are read before the archive transfer starts, so a manifest that is
    // malformed or belongs to another release costs one small request rather than a full download.
    const releaseManifestEntry = await fetchReleaseManifestEntry(version, target, tempRoot, warnings);
    const archiveUrl = await fetchReleaseFile(version, asset, archivePath, {
      partialPath: archivePartialPath,
    });
    const expected = expectedArchiveHash(fs.readFileSync(sumsPath, 'utf8'), asset);
    const actual = fileSha256(archivePath);
    if (actual !== expected) {
      throw new Error(`archive_checksum_mismatch:${asset}`);
    }
    // SHA256SUMS.txt travels over the same channel as the archive; the pin ships inside the
    // reviewed plugin package. When the pin names this version's digest, a real release download
    // must match it. Explicit packages (CODESTORY_PLUGIN_RELEASE_DIR and test fixtures) are
    // legitimately different bytes, the same relaxation the manifest metadata checks apply.
    if (buildSource === 'github_release') {
      const pinned = pinnedArchiveSha256(target);
      if (pinned && pinned !== actual) {
        throw new Error(`archive_pin_mismatch:${asset}`);
      }
    }
    // The native lane's digest authority, held against the downloaded bytes before extraction:
    // extraction is the first step that acts on what the release channel supplied.
    const archiveBytes = fs.statSync(archivePath).size;
    bindArchiveToReleaseManifest(releaseManifestEntry, { sha256: actual, bytes: archiveBytes }, warnings);
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
      // Recorded so the pre-publish native provision proof can hold the provisioned archive
      // against BOTH fields the release manifest carries. A length the manifest records and
      // nothing reads is a field that cannot fail.
      archive_bytes: archiveBytes,
      target,
      provisioned_at: new Date().toISOString(),
      stdio_initialize_verified: true,
    };
    const versionProbe = probeResolvedCli({ path: destination, provisioningProbe: true });
    const versionProbeFailure = managedCliVersionProbeFailure(versionProbe, version);
    if (versionProbeFailure) {
      throw new Error(`managed_cli_staging_verification_failed:${versionProbeFailure}`);
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
    removeManagedCliDownloadCache(root, version);
    managedCliDownloadProgress.stage = null;
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
    warnings.push(`managed_cli_verification_failed:${existing.reason}`);
  }
  if (options.provision === false) return null;
  try {
    return await provisionManagedCli(dataDir, version, warnings);
  } catch (error) {
    recordManagedCliProvisionFailure(warnings, error);
  }
  return null;
}

async function resolveCli(options = {}) {
  const version = pluginVersion();
  const warnings = [];
  let managedVersion;
  try {
    managedVersion = pinnedCliVersion();
  } catch {
    // A pin that no longer parses must not silently change which binary runs.
    const reason = 'managed_cli_pin_invalid';
    warnings.push(reason);
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
      managedFailure: reason,
      warnings,
    };
  }
  const devReceipt = validateDevCliReceipt(pluginRoot, {
    expectedPluginVersion: version,
    expectedCliVersion: managedVersion,
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

  const managed = await resolveManagedCli(pluginDataDir(), managedVersion, warnings, options);
  if (managed && managed.warning) warnings.push(managed.warning);
  if (managed && managed.path) {
    return { source: 'managed', version, warnings, ...managed };
  }

  const managedFailure =
    warnings.find((warning) => warning.startsWith('managed_cli_provision_failed:')) ||
    warnings.find((warning) => warning.startsWith('managed_cli_verification_failed:')) ||
    null;
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
    managedFailure,
    warnings,
  };
}

function managedCliProvisionFailureSnapshot() {
  return {
    code: managedCliProvisionFailure.code,
    context: managedCliProvisionFailure.context,
    hint: managedCliProvisionFailure.hint,
  };
}

function applyManagedCliProvisionFailure(snapshot) {
  if (!snapshot || typeof snapshot !== 'object') return;
  managedCliProvisionFailure.code = snapshot.code ?? null;
  managedCliProvisionFailure.context = snapshot.context ?? null;
  managedCliProvisionFailure.hint = snapshot.hint ?? null;
}

function runManagedProvisioningWorker(options = {}) {
  const WorkerClass = options.Worker || Worker;
  return new Promise((resolve, reject) => {
    const worker = new WorkerClass(__filename, {
      workerData: {
        codestoryMode: 'managed-provision',
        codestoryLaunchCwd: launchCwd,
      },
    });
    let settled = false;
    const fail = (error) => {
      if (settled) return;
      settled = true;
      reject(error instanceof Error ? error : new Error(String(error || 'managed_cli_worker_failed')));
    };
    worker.on('message', (message) => {
      if (message?.type === 'progress') {
        applyManagedCliProgress(message.progress);
        return;
      }
      if (message?.type === 'result') {
        if (settled) return;
        settled = true;
        applyManagedCliProvisionFailure(message.provisionFailure);
        resolve(message.outcome);
        return;
      }
      if (message?.type === 'error') {
        fail(new Error(message.code || 'managed_cli_worker_failed'));
      }
    });
    worker.once('error', fail);
    worker.once('exit', (code) => {
      if (!settled) fail(new Error(`managed_cli_worker_exit:${code}`));
    });
    worker.unref?.();
  });
}

async function runManagedProvisioningWorkerEntrypoint() {
  try {
    const resolved = await resolveCli();
    const probe = probeResolvedCli(resolved);
    const reason = failOpenReasonForProbe(resolved, probe);
    resolved.managedCliRetention = managedCliRetentionReport(resolved, probe, {
      dryRun: Boolean(reason),
    });
    parentPort.postMessage({
      type: 'result',
      outcome: { resolved, probe, reason },
      provisionFailure: managedCliProvisionFailureSnapshot(),
    });
  } catch (error) {
    parentPort.postMessage({ type: 'error', code: managedCliFailureCode(error) });
    process.exitCode = 1;
  } finally {
    parentPort.close();
  }
}

function normalizeVersion(value) {
  const match = String(value || '').match(/\b[vV]?(\d+\.\d+\.\d+)\b/u);
  return match ? match[1] : null;
}

function probeResolvedCli(resolved, options = {}) {
  if (!resolved.path) {
    return {
      status: null,
      error: `${resolved.source || 'unavailable'}_cli_unavailable`,
      errorCode: null,
      version: null,
      stdout: '',
      stderr: '',
    };
  }
  const env = resolved.provisioningProbe
    ? { ...process.env, CODESTORY_PLUGIN_PROVISIONING_PROBE: '1' }
    : process.env;
  const spawnCli = options.spawnCli || spawnCodeStoryCliSync;
  const result = spawnCli(resolved.path, ['--version'], {
    encoding: 'utf8',
    env,
    timeout: cliVersionProbeTimeoutMs,
    windowsHide: true,
  });
  const output = `${result.stdout || ''}\n${result.stderr || ''}`;
  return {
    status: result.status,
    error: result.error ? result.error.message : null,
    errorCode: result.error?.code || null,
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
    return resolved.managedFailure || 'managed_cli_unavailable';
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
  const probeFailure = managedCliVersionProbeFailure(probe, entry.version);
  if (probeFailure) return { verified: false, reason: probeFailure };
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
  // Retention is keyed on CLI identity, not plugin identity. The probe reports the CLI's own version
  // and the cache is laid out by CLI version, so comparing either against `resolved.version` (the
  // plugin's version) disables retention outright on every plugin-only release, where the plugin
  // moves ahead of the pinned CLI.
  const activeCliVersion = resolved.cliVersion || resolved.version;
  if (probe.version !== activeCliVersion) {
    report.warnings.push('managed_cli_retention_active_version_mismatch');
    reportUnverifiedManagedCliInventory(report, inventory.entries, 'active_version_mismatch');
    return report;
  }

  const active = inventory.entries.find((entry) => entry.version === activeCliVersion);
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
    // The launcher-provided half of the pinned pair, identical to the
    // `CODESTORY_PLUGIN_CLI_VERSION` the runtime stamps back as
    // `contract_runtime.plugin_cli_version`.
    plugin_cli_version: resolved.cliVersion || resolved.version || null,
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
    // `after_ms` matches the field the CLI runtime attaches to its own preparing
    // recommended-next-call, so agents see one timing contract across the handoff boundary.
    recommended_next_calls: preparing
      ? [{ method: 'tools/call', instruction: 'Retry the intended CodeStory tool shortly.', after_ms: provisioningRetryHintMs() }]
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

function jsonrpcError(id, code, message, data = undefined) {
  const error = { code, message };
  if (data !== undefined) error.data = data;
  return { jsonrpc: '2.0', id, error };
}

function failOpenFrameTooLargeError(lineBytes) {
  return jsonrpcError(
    null,
    -32700,
    `Parse error: stdio frame exceeded ${failOpenMaxFrameBytes} byte limit`,
    {
      code: 'stdio_frame_too_large',
      max_frame_bytes: failOpenMaxFrameBytes,
      line_bytes: lineBytes,
    },
  );
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

function failOpenToolCatalog(catalog = canonicalMcpCatalog) {
  if (!Array.isArray(catalog?.tools)) {
    throw new Error('generated_mcp_catalog_missing:run_generate_codestory_skill_syntax');
  }
  return JSON.parse(JSON.stringify(catalog.tools));
}

function emergencyStatusToolCatalog() {
  return [{
    name: 'status',
    description: 'Inspect CodeStory launcher readiness for one explicit repository.',
    inputSchema: {
      type: 'object',
      additionalProperties: false,
      properties: { project: { type: 'string', minLength: 1 } },
      required: ['project'],
    },
    annotations: {
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
      readOnlyHint: true,
    },
  }];
}

function catalogFailureStatus(status, catalogFailure) {
  const reason = 'generated_mcp_catalog_missing';
  const warning = `${reason}:${safeFailureToken(catalogFailure?.message, 'unreadable')}`;
  return {
    ...status,
    degraded_reason: reason,
    warnings: [...new Set([...(status.warnings || []), warning])],
    plugin_runtime: {
      ...(status.plugin_runtime || {}),
      warnings: [...new Set([...(status.plugin_runtime?.warnings || []), warning])],
    },
    runtime: { ...(status.runtime || {}), state: 'unavailable' },
    managed_retrieval: { ...(status.managed_retrieval || {}), state: 'unavailable' },
    readiness: [{
      goal: 'runtime',
      status: 'unavailable',
      summary: 'CodeStory could not load its generated MCP catalog.',
      reason,
      setup: { catalog_error: warning },
    }],
  };
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

function managedProvisioningOperation() {
  const progress = managedCliDownloadProgressReport();
  return {
    operation_id: 'managed-runtime-provisioning',
    state: 'preparing',
    stage: progress?.stage || 'downloading_runtime',
    attempt: progress?.attempt ?? 1,
    retry_after_ms: provisioningRetryHintMs(),
    failure: null,
    progress: progress
      ? {
        asset: progress.asset,
        received_bytes: progress.received_bytes,
        total_bytes: progress.total_bytes,
        percent: progress.percent,
      }
      : null,
  };
}

function managedProvisioningMessage() {
  const progress = managedCliDownloadProgressReport();
  if (!progress || progress.asset === 'SHA256SUMS.txt') {
    return 'CodeStory is preparing: downloading the runtime. Retry the same tool shortly.';
  }
  const received = formatByteSize(progress.received_bytes);
  const total = progress.total_bytes === null ? null : formatByteSize(progress.total_bytes);
  const measure = total
    ? `${progress.percent}% of ${total}`
    : `${received} so far`;
  return `CodeStory is preparing: downloading the runtime (${measure}). Retry the same tool shortly.`;
}

function failOpenToolResult(tool, status, argumentsValue = {}) {
  const preparing = status.managed_retrieval?.state === 'preparing';
  const readiness = Array.isArray(status.readiness) ? status.readiness[0] : null;
  const degradedReason = status.degraded_reason || readiness?.reason || (preparing ? 'managed_cli_provisioning' : 'runtime_unavailable');
  const managedFailure = status.warnings?.find((warning) =>
    /^managed_cli_(?:provision|verification)_failed:/u.test(String(warning || '')));
  const primaryFailure = managedFailure
    || readiness?.setup?.probe_error
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
    // The top-level hint repeats the operation snapshot's so one status response never carries
    // two disagreeing delays.
    const currentOperation = preparing ? managedProvisioningOperation() : null;
    const structuredContent = {
      project,
      state: preparing ? 'preparing' : 'unavailable',
      degraded_reason: degradedReason,
      capabilities: { local_navigation: 'unavailable', broad_search: preparing ? 'preparing' : 'unavailable' },
      current_operation: currentOperation,
      failure: preparing ? null : primaryFailure,
      failure_context: !preparing && managedFailure ? managedCliProvisionFailure.context : null,
      hint: !preparing && managedFailure ? managedCliProvisionFailure.hint : null,
      next_action: preparing ? 'retry_intended_tool' : 'use_source_inspection',
      retry_after_ms: currentOperation ? currentOperation.retry_after_ms : null,
      diagnostics_uri: diagnosticsUri,
    };
    return {
      content: [{ type: 'text', text: `state: ${structuredContent.state}\nresult: structured\n` }],
      structuredContent,
    };
  }
  const diagnosticsUri = projectBoundResourceUri('codestory://status', project);
  const failureHint = managedFailure ? managedCliProvisionFailure.hint : null;
  // The top-level hint repeats the operation snapshot's so one preparing response never carries
  // two disagreeing delays.
  const provisioningOperation = preparing ? managedProvisioningOperation() : null;
  const structuredContent = preparing ? {
    code: 'codestory_preparing',
    message: managedProvisioningMessage(),
    tool,
    project,
    state: 'preparing',
    retry_tool: tool,
    retry_after_ms: provisioningOperation.retry_after_ms,
    operation: provisioningOperation,
    diagnostics_uri: diagnosticsUri,
  } : {
    code: 'codestory_unavailable',
    message: failureHint
      ? `CodeStory is unavailable. ${failureHint} Meanwhile, continue with focused source inspection.`
      : 'CodeStory is unavailable. Continue with focused source inspection.',
    tool,
    project,
    state: 'unavailable',
    failure: primaryFailure,
    failure_context: managedFailure ? managedCliProvisionFailure.context : null,
    diagnostics_uri: diagnosticsUri,
  };
  const result = {
    content: [{ type: 'text', text: structuredContent.message }],
    structuredContent,
  };
  if (!preparing) {
    result.isError = true;
  }
  return result;
}

const shuttingDownHandoffs = new WeakSet();
const runtimeDiagnosticRedacted = '[redacted]';

function runtimeCorrelationId() {
  return randomBytes(16).toString('hex');
}

function sanitizeRuntimeDiagnosticText(value) {
  return String(value || '') ? runtimeDiagnosticRedacted : '';
}

function appendRuntimeStderrTail(current, chunk) {
  const previousBytes = Number.isSafeInteger(current?.observedBytes)
    ? current.observedBytes
    : 0;
  const previousChunks = Number.isSafeInteger(current?.observedChunks)
    ? current.observedChunks
    : 0;
  const incomingBytes = Buffer.byteLength(String(chunk || ''), 'utf8');
  const nextBytes = Math.min(runtimeStderrObservedBytesCap, previousBytes + incomingBytes);
  const nextChunks = Math.min(runtimeStderrObservedChunksCap, previousChunks + 1);
  return {
    observedBytes: nextBytes,
    observedChunks: nextChunks,
    bytesCapped: Boolean(current?.bytesCapped) || nextBytes < previousBytes + incomingBytes,
    chunksCapped: Boolean(current?.chunksCapped) || nextChunks < previousChunks + 1,
  };
}

function renderRuntimeStderrTail(current) {
  return {
    stderrBytes: Number.isSafeInteger(current?.observedBytes) ? current.observedBytes : 0,
    stderrChunks: Number.isSafeInteger(current?.observedChunks) ? current.observedChunks : 0,
    stderrBytesCapped: Boolean(current?.bytesCapped),
    stderrChunksCapped: Boolean(current?.chunksCapped),
  };
}

const runtimeFailureReasons = new Map([
  ['runtime_stdio_child_exit', 'CodeStory stdio handoff exited before completing the request.'],
  ['runtime_stdio_child_spawn', 'CodeStory stdio handoff failed to start.'],
  ['runtime_stdio_child_stdin', 'CodeStory stdio handoff stdin failed.'],
  [
    'runtime_wire_contract_skew',
    'CodeStory stdio handoff published a wire contract this plugin cannot read.',
  ],
]);

function runtimeFailureCode(value) {
  return runtimeFailureReasons.has(value) ? value : 'unknown_runtime_failure';
}

function safeRuntimeDiagnosticToken(value, fallback = 'unknown') {
  const candidate = String(value || '');
  return /^[A-Za-z0-9_.-]{1,128}$/u.test(candidate) ? candidate : fallback;
}

function optionalSafeRuntimeDiagnosticToken(value) {
  return value == null || value === '' ? null : safeRuntimeDiagnosticToken(value);
}

function runtimeFailureDetail(reasonCode, details = {}) {
  const typedReasonCode = runtimeFailureCode(reasonCode);
  const reason = runtimeFailureReasons.get(typedReasonCode)
    || 'CodeStory stdio handoff failed.';
  const fields = [
    `reason_code=${typedReasonCode}`,
    `exit_code=${Number.isSafeInteger(details.code) ? details.code : 'none'}`,
    `signal=${optionalSafeRuntimeDiagnosticToken(details.signal) || 'none'}`,
    `correlation_id=${optionalSafeRuntimeDiagnosticToken(details.correlationId) || 'unavailable'}`,
  ];
  const errorCode = optionalSafeRuntimeDiagnosticToken(details.errorCode);
  if (errorCode) fields.push(`error_code=${errorCode}`);
  if (Number.isSafeInteger(details.stderrBytes) && details.stderrBytes > 0) {
    fields.push(`stderr_bytes=${details.stderrBytes}`);
    fields.push(`stderr_chunks=${details.stderrChunks}`);
    fields.push(`stderr_bytes_capped=${Boolean(details.stderrBytesCapped)}`);
    fields.push(`stderr_chunks_capped=${Boolean(details.stderrChunksCapped)}`);
  }
  return `${reason} (${fields.join(' ')})`;
}

let launcherFatalHandlersInstalled = false;

function installLauncherFatalHandlers() {
  if (launcherFatalHandlersInstalled) return;
  launcherFatalHandlersInstalled = true;
  process.once('uncaughtException', () => {
    const suppliedCorrelation = String(process.env.CODESTORY_LOG_CORRELATION_ID || '');
    const correlationId = /^[A-Za-z0-9_-]{1,128}$/u.test(suppliedCorrelation)
      ? suppliedCorrelation
      : runtimeCorrelationId();
    const diagnostic = {
      event: 'launcher_uncaught_exception',
      level: 'ERROR',
      pid: process.pid,
      correlation_id: correlationId,
      error: runtimeDiagnosticRedacted,
      stack: runtimeDiagnosticRedacted,
    };
    try {
      fs.writeSync(2, `${JSON.stringify(diagnostic)}\n`);
    } catch {
      // Termination is unconditional even when the diagnostic sink is gone.
    }
    process.exit(1);
  });
}

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
  const graceMs = options.handoffTerminationGraceMs ?? 5000;
  const forceGraceMs = options.handoffForceKillGraceMs ?? 5000;
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
  const baseCurrentStatus = () => (typeof status === 'function' ? status() : status);
  const catalog = Object.hasOwn(options, 'catalog') ? options.catalog : canonicalMcpCatalog;
  let catalogFailure = null;
  let tools;
  let resources;
  let resourceTemplates;
  try {
    tools = failOpenToolCatalog(catalog);
    if (!Array.isArray(catalog.resources) || !Array.isArray(catalog.resourceTemplates)) {
      throw new Error('generated_mcp_catalog_missing:run_generate_codestory_skill_syntax');
    }
    resources = catalog.resources.filter(({ uri }) => uri === 'codestory://agent-guide');
    resourceTemplates = catalog.resourceTemplates.filter(({ uriTemplate }) =>
      uriTemplate === 'codestory://status{?project}');
  } catch (error) {
    catalogFailure = error;
    tools = emergencyStatusToolCatalog();
    resources = [];
    resourceTemplates = [{
      mimeType: 'application/json',
      name: 'Status',
      uriTemplate: 'codestory://status{?project}',
    }];
  }
  const currentStatus = () => {
    const current = baseCurrentStatus();
    return catalogFailure ? catalogFailureStatus(current, catalogFailure) : current;
  };
  let handoff = null;
  let handoffWrite = null;
  let initializeRequest = null;
  let negotiatedProtocol = null;
  let initializedNotification = null;
  let runtimeReadyNotified = false;
  let stdinEnded = false;
  let handoffStderrObservation = null;
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
    handoffStderrObservation = null;
    handoffFailureHandled = false;
    const failHandoff = (reasonCode, details = {}) => {
      if (handoffFailureHandled) return;
      handoffFailureHandled = true;
      const failedHandoff = handoff;
      const stderrObservation = renderRuntimeStderrTail(handoffStderrObservation);
      const failureDetails = {
        code: Number.isSafeInteger(details.code) ? details.code : null,
        signal: optionalSafeRuntimeDiagnosticToken(details.signal),
        errorCode: optionalSafeRuntimeDiagnosticToken(details.errorCode),
        spawnError: Boolean(details.spawnError),
        stdinError: Boolean(details.stdinError),
        correlationId: optionalSafeRuntimeDiagnosticToken(failedHandoff?.codestoryCorrelationId),
        stderrBytes: Number.isSafeInteger(stderrObservation.stderrBytes)
          ? Math.min(runtimeStderrObservedBytesCap, Math.max(0, stderrObservation.stderrBytes))
          : 0,
        stderrChunks: Number.isSafeInteger(stderrObservation.stderrChunks)
          ? Math.min(runtimeStderrObservedChunksCap, Math.max(0, stderrObservation.stderrChunks))
          : 0,
        stderrBytesCapped: Boolean(stderrObservation.stderrBytesCapped),
        stderrChunksCapped: Boolean(stderrObservation.stderrChunksCapped),
      };
      const detailedReason = runtimeFailureDetail(reasonCode, failureDetails);
      handoff = null;
      handoffWrite = null;
      shutdownHandoffChild(failedHandoff, options);
      if (typeof options.onRuntimeFailure !== 'function') {
        process.exit(failureDetails.code || 1);
        return;
      }
      for (const id of delegatedRequestIds) {
        process.stdout.write(`${JSON.stringify(jsonrpcError(
          JSON.parse(id),
          -32000,
          detailedReason,
        ))}\n`);
      }
      delegatedRequestIds.clear();
      options.onRuntimeFailure({
        reason: detailedReason,
        reasonCode: runtimeFailureCode(reasonCode),
        ...failureDetails,
      });
    };
    handoffWrite = (line) => {
      try {
        if (!handoff?.stdin || handoff.stdin.destroyed) {
          throw Object.assign(new Error('child stdin is unavailable'), { code: 'EPIPE' });
        }
        handoff.stdin.write(`${line}\n`, (error) => {
          if (error) {
            failHandoff('runtime_stdio_child_stdin', {
              errorCode: error?.code,
              stdinError: true,
            });
          }
        });
        return true;
      } catch (error) {
        failHandoff('runtime_stdio_child_stdin', {
          errorCode: error?.code,
          stdinError: true,
        });
        return false;
      }
    };
    handoff.stdin?.on?.('error', (error) => {
      failHandoff('runtime_stdio_child_stdin', {
        errorCode: error?.code,
        stdinError: true,
      });
    });
    if (handoff.stdout) {
      let stdout = '';
      let suppressInitialize = Boolean(initializeRequest);
      handoff.stdout.setEncoding('utf8');
      handoff.stdout.on('data', (chunk) => {
        // A failed handoff stops relaying immediately. Chunks that arrive after
        // the failure carry results from a runtime the launcher already refused.
        if (handoffFailureHandled) return;
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
            // The host never sees this frame — the launcher already answered
            // `initialize`. That makes the launcher the only reader of the
            // runtime's own compatibility claim, and the only place a
            // `CODESTORY_CLI` override can be caught at session runtime.
            const skew = runtimeWireContractSkew(
              parsed,
              negotiatedProtocol?.negotiated ?? managedCliMcpProtocolVersion,
            );
            if (skew) {
              failHandoff('runtime_wire_contract_skew', { errorCode: skew });
              return;
            }
            continue;
          }
          if (parsed?.id !== undefined) delegatedRequestIds.delete(JSON.stringify(parsed.id));
          process.stdout.write(`${output}\n`);
        }
      });
    }
    if (handoff.stderr) {
      handoff.stderr.setEncoding?.('utf8');
      handoff.stderr.on('data', (chunk) => {
        // Drain child stderr without retaining its free-form bytes. Only
        // saturating byte/chunk counts cross the diagnostic boundary.
        handoffStderrObservation = appendRuntimeStderrTail(handoffStderrObservation, chunk);
      });
    }
    handoff.on('close', (code, signal) => {
      if (signal || code) {
        failHandoff('runtime_stdio_child_exit', { code, signal });
        return;
      }
      process.exit(0);
    });
    handoff.on('error', (error) => {
      failHandoff('runtime_stdio_child_spawn', {
        errorCode: error?.code,
        spawnError: true,
      });
    });
    if (initializeRequest) {
      if (handoffWrite(JSON.stringify(initializeRequest))) {
        handoffWrite?.(JSON.stringify(initializedNotification || {
          jsonrpc: '2.0',
          method: 'notifications/initialized',
        }));
      }
    }
    if (stdinEnded) handoff.stdin.end();
    return handoff;
  };
  // Fail-open serves the project-bound status template and static guide. Do
  // not advertise other generated templates or prompts until the native
  // runtime owns their read/get handlers.
  const prompts = [];
  const guide = () => {
    return {
      message: 'Call the tool that matches the task and pass its absolute repository root. If it reports preparing, retry that same tool after its delay.',
      diagnostics_uri_template: 'codestory://status{?project}',
    };
  };
  const handleLine = (line) => {
    if (!line.trim()) return;
    let request;
    try {
      request = JSON.parse(line);
    } catch {
      process.stdout.write(`${JSON.stringify(jsonrpcError(null, -32700, 'Parse error'))}\n`);
      return;
    }
    if (!request || typeof request !== 'object' || Array.isArray(request)) {
      process.stdout.write(`${JSON.stringify(jsonrpcError(null, -32600, 'Invalid Request'))}\n`);
      return;
    }
    if (request.method === 'notifications/initialized') {
      initializedNotification = request;
      notifyRuntimeReady();
      return;
    }
    if (request.method === 'initialize' && request.id !== undefined) {
      initializeRequest = request;
    }
    const delegated = request.method === 'initialize' ? null : maybeHandoff();
    if (delegated) {
      if (request.id !== undefined) delegatedRequestIds.add(JSON.stringify(request.id));
      handoffWrite(line);
      return;
    }
    if (request.id === undefined) return;
    let response;
    if (request.method === 'initialize') {
      const liveStatus = currentStatus();
      negotiatedProtocol = negotiateMcpProtocolVersion(request.params?.protocolVersion);
      response = jsonrpcResult(request.id, {
        protocolVersion: negotiatedProtocol.negotiated,
        capabilities: {
          tools: { listChanged: true },
          resources: { subscribe: false, listChanged: true },
          prompts: { listChanged: true },
        },
        serverInfo: { name: 'codestory', version: resolvedVersionForStatus(liveStatus) },
        _meta: {
          codestory_publication: failOpenPublicationStamp(liveStatus),
          codestory_protocol: negotiatedProtocol,
        },
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
          statusValue.recommended_next_calls = statusValue.recommended_next_calls.map((call) => {
            if (call?.method === 'resources/read'
              && call?.uri_template === 'codestory://status{?project}') {
              return { method: call.method, uri: parsedResource.uri };
            }
            // The preparing diagnostic is snapshotted when provisioning starts; the retry hint
            // must track the download progress observed at this read, not that initial instant.
            if (call?.method === 'tools/call' && Number.isSafeInteger(call.after_ms)) {
              return { ...call, after_ms: provisioningRetryHintMs() };
            }
            return call;
          });
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
        ? jsonrpcResult(
            request.id,
            failOpenToolResult(tool, currentStatus(), request.params?.arguments ?? {}),
          )
        : jsonrpcError(request.id, -32602, `unknown tool: ${tool || '<missing>'}`);
    } else {
      response = jsonrpcError(request.id, -32601, `method not found: ${request.method || '<missing>'}`);
    }
    process.stdout.write(`${JSON.stringify(response)}\n`);
  };
  let buffer = '';
  let bufferBytes = 0;
  let discardedFrameBytes = 0;
  const reportDiscardedFrame = () => {
    process.stdout.write(`${JSON.stringify(failOpenFrameTooLargeError(discardedFrameBytes))}\n`);
    discardedFrameBytes = 0;
  };
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    let offset = 0;
    while (offset < chunk.length) {
      const newline = chunk.indexOf('\n', offset);
      const end = newline >= 0 ? newline + 1 : chunk.length;
      const segment = chunk.slice(offset, end);
      const segmentBytes = Buffer.byteLength(segment, 'utf8');
      if (discardedFrameBytes > 0) {
        discardedFrameBytes += segmentBytes;
        if (newline >= 0) reportDiscardedFrame();
      } else if (bufferBytes + segmentBytes > failOpenMaxFrameBytes) {
        discardedFrameBytes = bufferBytes + segmentBytes;
        buffer = '';
        bufferBytes = 0;
        if (newline >= 0) reportDiscardedFrame();
      } else {
        buffer += segment;
        bufferBytes += segmentBytes;
        if (newline >= 0) {
          let line = buffer.slice(0, -1);
          if (line.endsWith('\r')) line = line.slice(0, -1);
          buffer = '';
          bufferBytes = 0;
          handleLine(line);
        }
      }
      offset = end;
    }
  });
  process.stdin.on('end', () => {
    if (discardedFrameBytes > 0) reportDiscardedFrame();
    else if (buffer.trim()) handleLine(buffer);
    buffer = '';
    bufferBytes = 0;
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
  const correlationId = runtimeCorrelationId();
  const child = spawnCodeStoryCli(resolved.path, ['serve', '--stdio', '--multi-project', '--refresh', 'none'], {
    cwd: runtimeCwd,
    stdio,
    windowsHide: true,
    env: {
      ...stdioRuntimeEnv(resolved, runtimeCwd),
      CODESTORY_LOG_CORRELATION_ID: correlationId,
    },
  });
  child.codestoryCorrelationId = correlationId;
  return child;
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
      runManagedProvisioningWorker().then(({ resolved, probe, reason }) => {
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
        const reason = failure.reasonCode === 'runtime_wire_contract_skew'
          ? 'runtime_wire_contract_skew'
          : failure.stdinError
            ? 'runtime_stdio_child_stdin'
            : failure.spawnError ? 'managed_cli_handoff_unspawnable' : 'runtime_stdio_child_exit';
        status = fallbackDiagnostic(failed, {
          status: failure.code ?? null,
          error: failure.reason,
          version: failed.cliVersion || failed.version,
          stdout: '',
          stderr: '',
        }, reason, {
          projectRoot: null,
          projectRootSource: 'request_argument',
          summary: 'CodeStory managed CLI provisioning completed, but the stdio runtime failed during handoff.',
          setup: {
            runtime_exit_code: failure.code ?? null,
            runtime_exit_signal: failure.signal || null,
            runtime_correlation_id: failure.correlationId || null,
            runtime_stderr_bytes: failure.stderrBytes || 0,
            runtime_stderr_chunks: failure.stderrChunks || 0,
            runtime_stderr_bytes_capped: Boolean(failure.stderrBytesCapped),
            runtime_stderr_chunks_capped: Boolean(failure.stderrChunksCapped),
          },
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
      const reason = failure.reasonCode === 'runtime_wire_contract_skew'
        ? 'runtime_wire_contract_skew'
        : failure.stdinError
          ? 'runtime_stdio_child_stdin'
          : failure.spawnError ? `${resolved.source}_cli_unspawnable` : 'runtime_stdio_child_exit';
      const error = failure.code != null
        ? `codestory-cli serve --stdio exited with status ${failure.code}`
        : failure.reason;
      status = fallbackDiagnostic(resolved, {
        ...probe,
        status: failure.code ?? null,
        error,
        stderr: '',
      }, reason, {
        projectRoot: null,
        projectRootSource: 'request_argument',
        summary: 'CodeStory launched its verified CLI, but the stdio runtime failed during handoff.',
        setup: {
          runtime_exit_code: failure.code ?? null,
          runtime_exit_signal: failure.signal || null,
          runtime_correlation_id: failure.correlationId || null,
          runtime_stderr_bytes: failure.stderrBytes || 0,
          runtime_stderr_chunks: failure.stderrChunks || 0,
          runtime_stderr_bytes_capped: Boolean(failure.stderrBytesCapped),
          runtime_stderr_chunks_capped: Boolean(failure.stderrChunksCapped),
        },
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
  installLauncherFatalHandlers();
  if (!isMainThread && workerData?.codestoryMode === 'managed-provision') {
    runManagedProvisioningWorkerEntrypoint().catch((error) => {
      throw error;
    });
  } else {
    main().catch(runLauncherError);
  }
} else {
  module.exports = {
    _test: {
      compareManagedCliVersions,
      applyCursorLocalOverrides,
      cleanPublicProjectPath,
      downloadFile,
      downloadFailurePermanent,
      copyVerifiedPartial,
      pinnedCliContract,
      pinnedCliVersion,
      pinnedArchiveSha256,
      RELEASE_MANIFEST_ASSET,
      RELEASE_MANIFEST_DOMAIN,
      RELEASE_MANIFEST_SCHEMA_VERSION,
      releaseManifestArchiveEntry,
      fetchReleaseManifestEntry,
      inferredCodexPluginDataDir,
      inferredCursorPluginDataDir,
      confirmedCursorIdentity,
      pluginDataDir,
      readCursorLocalOverrides,
      cursorLocalOverrideFileName,
      cursorDogfoodMarker,
      bindArchiveToReleaseManifest,
      publishDownloadedFile,
      managedCliDownloadCacheDir,
      removeManagedCliDownloadCache,
      managedCliDownloadHint,
      managedCliDownloadProgress,
      managedCliProvisionFailure,
      managedProvisioningOperation,
      provisioningRetryHintMs,
      provisioningRetryHintMinMs,
      provisioningRetryHintMaxMs,
      provisioningRetryHintFallbackMs,
      sanitizeDownloadFailure,
      appendRuntimeStderrTail,
      renderRuntimeStderrTail,
      sanitizeRuntimeDiagnosticText,
      runtimeFailureDetail,
      runtimeCorrelationId,
      runtimeStderrObservedBytesCap,
      runtimeStderrObservedChunksCap,
      installLauncherFatalHandlers,
      runManagedProvisioningWorker,
      releaseDownloadStallTimeoutMs,
      releaseArchiveTotalTimeoutMs,
      releaseChecksumTotalTimeoutMs,
      trimManagedCliDownloadCache,
      extractArchive,
      failOpenToolResult,
      failOpenToolCatalog,
      failOpenMaxFrameBytes,
      managedCliFailureCode,
      managedCliVersionProbeFailure,
      recordManagedCliProvisionFailure,
      parseFailOpenResourceRequest,
      projectBoundResourceUri,
      strictUriComponentDecode,
      strictUriComponentEncode,
      acquireManagedCliLock,
      acquireManagedCliLockAsync,
      managedCliIdentityProbeIntervalMs,
      managedCliLockWaitMs,
      releaseAssetRetryBudgetMs,
      managedAssetIdentity,
      releaseAssetIdentity,
      isWindowsBatchCli,
      requireDirectCli,
      reclaimStaleManagedCliPendingOwners,
      removeManagedCliInitializationIf,
      processStartIdentity,
      probeResolvedCli,
      probeManagedCliStdio,
      negotiateMcpProtocolVersion,
      failOpenPublicationStamp,
      publicationStampSkew,
      runtimeWireContractSkew,
      supportedMcpProtocolVersions,
      managedCliMcpProtocolVersion,
      publicationStampSchemaVersion,
      minimumCompatiblePublicationStampSchemaVersion,
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
