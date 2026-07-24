const { createHash } = require('crypto');
const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const receiptName = '.codestory-dev-cli.json';
const receiptSchemaVersion = 1;
const receiptPurpose = 'codestory-dev-plugin-cli';
const receiptPluginId = 'codestory@CodeStoryDev';
const receiptPluginName = 'codestory';
const sha256Pattern = /^[0-9a-f]{64}$/u;
const commitPattern = /^[0-9a-f]{40}$/u;

function sourceBuildTarget(platform = process.platform, arch = process.arch) {
  if (platform === 'win32' && arch === 'x64') return 'windows-x64';
  if (platform === 'win32' && arch === 'arm64') return 'windows-arm64';
  if (platform === 'linux' && arch === 'x64') return 'linux-x64';
  if (platform === 'linux' && arch === 'arm64') return 'linux-arm64';
  if (platform === 'darwin' && arch === 'x64') return 'macos-x64';
  if (platform === 'darwin' && arch === 'arm64') return 'macos-arm64';
  return null;
}

function expectedBinaryName(platform = process.platform) {
  return platform === 'win32' ? 'codestory-cli.exe' : 'codestory-cli';
}

function directDirectory(pathname, label) {
  const metadata = fs.lstatSync(pathname);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error(`${label}_not_direct_directory`);
  }
}

function directFile(pathname, label) {
  const metadata = fs.lstatSync(pathname);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) {
    throw new Error(`${label}_not_direct_file`);
  }
  return metadata;
}

function pathInside(candidate, root) {
  const relative = path.relative(path.resolve(root), path.resolve(candidate));
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}

function collectDirectoryFiles(root, relative = '', files = []) {
  const directory = relative ? path.join(root, relative) : root;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
    Buffer.from(left.name).compare(Buffer.from(right.name)))) {
    const childRelative = relative ? path.join(relative, entry.name) : entry.name;
    const child = path.join(root, childRelative);
    const metadata = fs.lstatSync(child);
    if (metadata.isSymbolicLink()) {
      throw new Error(`directory_contract_symlink:${childRelative.split(path.sep).join('/')}`);
    }
    if (metadata.isDirectory()) {
      collectDirectoryFiles(root, childRelative, files);
      continue;
    }
    if (!metadata.isFile()) {
      throw new Error(`directory_contract_non_file:${childRelative.split(path.sep).join('/')}`);
    }
    files.push(childRelative);
  }
  return files;
}

function directoryContractSha256(root, options = {}) {
  directDirectory(root, 'directory_contract_root');
  const excluded = new Set((options.exclude || []).map((entry) => entry.split('/').join(path.sep)));
  const files = collectDirectoryFiles(root).filter((entry) => !excluded.has(entry));
  if (files.length === 0) throw new Error('directory_contract_empty');
  const digest = createHash('sha256');
  for (const relativeNative of files.sort((left, right) =>
    Buffer.from(left.split(path.sep).join('/')).compare(Buffer.from(right.split(path.sep).join('/'))))) {
    const relative = Buffer.from(relativeNative.split(path.sep).join('/'), 'utf8');
    const payload = fs.readFileSync(path.join(root, relativeNative));
    const relativeLength = Buffer.alloc(8);
    relativeLength.writeBigUInt64LE(BigInt(relative.length));
    const payloadLength = Buffer.alloc(8);
    payloadLength.writeBigUInt64LE(BigInt(payload.length));
    digest.update(relativeLength);
    digest.update(relative);
    digest.update(payloadLength);
    digest.update(payload);
  }
  return digest.digest('hex');
}

function exactKeys(value, expected) {
  return value
    && typeof value === 'object'
    && !Array.isArray(value)
    && Object.keys(value).sort().join(',') === [...expected].sort().join(',');
}

function pluginCacheIdentity(root) {
  const version = path.basename(root);
  const pluginName = path.basename(path.dirname(root));
  const marketplace = path.basename(path.dirname(path.dirname(root)));
  const cache = path.basename(path.dirname(path.dirname(path.dirname(root))));
  if (cache !== 'cache' || pluginName !== receiptPluginName || marketplace !== 'CodeStoryDev') {
    return null;
  }
  return {
    pluginId: `${pluginName}@${marketplace}`,
    pluginName,
    version,
  };
}

function readReceipt(receiptPath) {
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(receiptPath, 'utf8'));
  } catch (error) {
    throw new Error(`codestory_dev_receipt_json:${error.message}`);
  }
  return parsed;
}

function normalizeVersion(value) {
  const match = String(value || '').match(/\bcodestory-cli\s+v?(\d+\.\d+\.\d+)\b/u);
  return match ? match[1] : null;
}

function validateDevCliReceipt(root, options = {}) {
  const receiptPath = path.join(root, receiptName);
  try {
    fs.lstatSync(receiptPath);
  } catch (error) {
    if (error.code === 'ENOENT') return { state: 'absent' };
    return {
      state: 'invalid',
      receiptPath,
      reason: `codestory_dev_receipt_stat:${error.message}`,
    };
  }
  try {
    directDirectory(root, 'codestory_dev_plugin_root');
    directFile(receiptPath, 'codestory_dev_receipt');
    const receipt = readReceipt(receiptPath);
    if (!exactKeys(receipt, [
      'schema_version',
      'purpose',
      'plugin_id',
      'plugin_name',
      'plugin_version',
      'source_commit',
      'source_package_sha256',
      'target',
      'cli',
    ])) {
      throw new Error('codestory_dev_receipt_keys');
    }
    if (
      receipt.schema_version !== receiptSchemaVersion
      || receipt.purpose !== receiptPurpose
      || receipt.plugin_id !== receiptPluginId
      || receipt.plugin_name !== receiptPluginName
    ) {
      throw new Error('codestory_dev_receipt_identity');
    }
    if (!/^\d+\.\d+\.\d+$/u.test(String(receipt.plugin_version || ''))) {
      throw new Error('codestory_dev_receipt_plugin_version');
    }
    if (
      options.expectedPluginVersion
      && receipt.plugin_version !== options.expectedPluginVersion
    ) {
      throw new Error('codestory_dev_receipt_plugin_version_mismatch');
    }
    if (!commitPattern.test(String(receipt.source_commit || ''))) {
      throw new Error('codestory_dev_receipt_source_commit');
    }
    if (!sha256Pattern.test(String(receipt.source_package_sha256 || ''))) {
      throw new Error('codestory_dev_receipt_source_digest');
    }
    const target = sourceBuildTarget(options.platform, options.arch);
    if (!target || receipt.target !== target) {
      throw new Error('codestory_dev_receipt_target');
    }
    const cacheIdentity = pluginCacheIdentity(root);
    if (options.requireCacheIdentity !== false) {
      if (
        !cacheIdentity
        || cacheIdentity.pluginId !== receipt.plugin_id
        || cacheIdentity.pluginName !== receipt.plugin_name
        || cacheIdentity.version !== receipt.plugin_version
      ) {
        throw new Error('codestory_dev_receipt_cache_identity');
      }
    }
    if (!exactKeys(receipt.cli, ['path', 'name', 'bytes', 'sha256', 'version'])) {
      throw new Error('codestory_dev_receipt_cli_keys');
    }
    const binary = expectedBinaryName(options.platform);
    const expectedRelative = `bin/${binary}`;
    if (
      receipt.cli.path !== expectedRelative
      || receipt.cli.name !== binary
      || receipt.cli.version !== receipt.plugin_version
      || !Number.isSafeInteger(receipt.cli.bytes)
      || receipt.cli.bytes <= 0
      || !sha256Pattern.test(String(receipt.cli.sha256 || ''))
    ) {
      throw new Error('codestory_dev_receipt_cli_identity');
    }
    const cliPath = path.resolve(root, ...receipt.cli.path.split('/'));
    if (!pathInside(cliPath, root)) throw new Error('codestory_dev_receipt_cli_escape');
    directDirectory(path.dirname(cliPath), 'codestory_dev_cli_parent');
    const cliMetadata = directFile(cliPath, 'codestory_dev_cli');
    if (!pathInside(fs.realpathSync(cliPath), fs.realpathSync(root))) {
      throw new Error('codestory_dev_receipt_cli_realpath_escape');
    }
    if (path.basename(cliPath) !== receipt.cli.name) {
      throw new Error('codestory_dev_receipt_cli_name');
    }
    if (cliMetadata.size !== receipt.cli.bytes) {
      throw new Error('codestory_dev_receipt_cli_size');
    }
    if ((options.platform || process.platform) !== 'win32' && (cliMetadata.mode & 0o111) === 0) {
      throw new Error('codestory_dev_receipt_cli_not_executable');
    }
    const actualCliSha256 = createHash('sha256').update(fs.readFileSync(cliPath)).digest('hex');
    if (actualCliSha256 !== receipt.cli.sha256) {
      throw new Error('codestory_dev_receipt_cli_digest');
    }
    const actualSourceDigest = directoryContractSha256(root, {
      exclude: [receiptName, receipt.cli.path],
    });
    if (actualSourceDigest !== receipt.source_package_sha256) {
      throw new Error('codestory_dev_receipt_package_digest');
    }
    const probe = options.probeVersion
      ? options.probeVersion(cliPath)
      : spawnSync(cliPath, ['--version'], {
        encoding: 'utf8',
        shell: false,
        timeout: 3000,
        windowsHide: true,
      });
    if (probe.error || probe.status !== 0) {
      throw new Error('codestory_dev_receipt_cli_probe');
    }
    const probedVersion = options.probeVersion
      ? normalizeVersion(probe.stdout)
      : normalizeVersion(`${probe.stdout || ''}\n${probe.stderr || ''}`);
    if (probedVersion !== receipt.cli.version) {
      throw new Error('codestory_dev_receipt_cli_version');
    }
    const finalCliMetadata = directFile(cliPath, 'codestory_dev_cli');
    const finalCliSha256 = createHash('sha256').update(fs.readFileSync(cliPath)).digest('hex');
    if (finalCliMetadata.size !== receipt.cli.bytes || finalCliSha256 !== actualCliSha256) {
      throw new Error('codestory_dev_receipt_cli_changed');
    }
    const finalSourceDigest = directoryContractSha256(root, {
      exclude: [receiptName, receipt.cli.path],
    });
    if (finalSourceDigest !== actualSourceDigest) {
      throw new Error('codestory_dev_receipt_package_changed');
    }
    return {
      state: 'verified',
      path: cliPath,
      sha256: finalCliSha256,
      version: receipt.plugin_version,
      cliVersion: probedVersion,
      receiptPath,
      sourceCommit: receipt.source_commit,
      sourcePackageSha256: finalSourceDigest,
      target,
    };
  } catch (error) {
    return {
      state: 'invalid',
      receiptPath,
      reason: error.message,
    };
  }
}

module.exports = {
  directoryContractSha256,
  expectedBinaryName,
  pluginCacheIdentity,
  receiptName,
  receiptPluginId,
  receiptPluginName,
  receiptPurpose,
  receiptSchemaVersion,
  sourceBuildTarget,
  validateDevCliReceipt,
};
