import assert from "node:assert/strict";
import test from "node:test";
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import { access, chmod, copyFile, link, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat, symlink, utimes, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import { deflateRawSync, gunzipSync, gzipSync } from "node:zlib";
import { PassThrough, Writable } from "node:stream";
import { EventEmitter } from "node:events";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));
const require = createRequire(import.meta.url);
const launcherTest = require(join(pluginRoot, "scripts", "codestory-mcp.cjs"))._test;
const devCliContract = require(join(pluginRoot, "scripts", "codestory-dev-cli-contract.cjs"));
const statusUri = launcherTest.projectBoundResourceUri("codestory://status", repoRoot);
const {
  dirtyMarkerPathForProject,
  dirtyHookStatus,
  installDirtyHooks,
  uninstallDirtyHooks,
  writeDirtyMarker,
} = require(join(pluginRoot, "hooks", "codestory-runtime.cjs"));

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("managed release provisioning rejects unshipped targets before URL construction", () => {
  assert.deepEqual(
    launcherTest.releaseAssetIdentity("0.16.0", "darwin", "arm64"),
    {
      target: "macos-arm64",
      asset: "codestory-cli-v0.16.0-macos-arm64.tar.gz",
    },
  );
  assert.deepEqual(
    launcherTest.releaseAssetIdentity("0.16.0", "win32", "x64"),
    {
      target: "windows-x64",
      asset: "codestory-cli-v0.16.0-windows-x64.zip",
    },
  );
  for (const [platform, architecture] of [
    ["darwin", "x64"],
    ["win32", "arm64"],
    ["linux", "x64"],
    ["linux", "arm64"],
  ]) {
    assert.throws(
      () => launcherTest.releaseAssetIdentity("0.16.0", platform, architecture),
      new RegExp(`^Error: unsupported_release_target:${platform}-${architecture}$`, "u"),
    );
  }
  assert.deepEqual(
    launcherTest.managedAssetIdentity("0.16.0", {
      platform: "linux",
      arch: "x64",
      explicitSource: true,
    }),
    {
      target: "linux-x64",
      asset: "codestory-cli-v0.16.0-linux-x64.tar.gz",
      buildSource: "explicit_package",
    },
  );
});

test("development receipts identify source-build targets independently of release packaging", () => {
  assert.deepEqual(
    [
      ["darwin", "arm64"],
      ["darwin", "x64"],
      ["linux", "arm64"],
      ["linux", "x64"],
      ["win32", "arm64"],
      ["win32", "x64"],
    ].map(([platform, architecture]) =>
      devCliContract.sourceBuildTarget(platform, architecture)),
    [
      "macos-arm64",
      "macos-x64",
      "linux-arm64",
      "linux-x64",
      "windows-arm64",
      "windows-x64",
    ],
  );
});

function launcherHandoffInput() {
  return [
    {
      jsonrpc: "2.0",
      id: "initialize",
      method: "initialize",
      params: {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: "plugin-static", version: "1" },
      },
    },
    { jsonrpc: "2.0", id: "native-tools", method: "tools/list" },
  ].map((request) => JSON.stringify(request)).join("\n") + "\n";
}

async function stopChildProcess(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const closed = once(child, "close").catch(() => []);
  try {
    child.stdin?.end();
  } catch {
    // Continue to the bounded signal path.
  }
  try {
    child.kill("SIGTERM");
  } catch {
    return;
  }
  await Promise.race([closed, delay(500)]);
  if (child.exitCode === null && child.signalCode === null) {
    try {
      child.kill("SIGKILL");
    } catch {
      return;
    }
    await Promise.race([closed, delay(500)]);
  }
}

test("fail-open tool schemas are the generated canonical MCP catalog", async () => {
  const catalog = JSON.parse(await readFile(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"));
  assert.deepEqual(launcherTest.failOpenToolCatalog(), catalog.tools);
  assert.deepEqual(catalog.resources.map(({ uri }) => uri), ["codestory://agent-guide"]);
  assert.ok(
    catalog.resourceTemplates.some(({ uriTemplate }) =>
      uriTemplate === "codestory://status{?project}"),
  );
  assert.ok(
    catalog.resourceTemplates.every(({ uriTemplate }) => uriTemplate.endsWith("{?project}")),
    "every advertised repository resource template must carry a project selector",
  );
  const snippet = catalog.tools.find(({ name }) => name === "snippet");
  assert.deepEqual(
    Object.keys(snippet.inputSchema.properties).sort(),
    ["choose", "context", "function_body", "id", "lines", "project", "query", "scope"],
  );
});

test("fail-open project resource URIs use the native strict encoding contract", () => {
  for (const project of ["/tmp/Code Story/%/café", String.raw`C:\Code Story\100% data\Δ`]) {
    const encoded = launcherTest.strictUriComponentEncode(project);
    assert.equal(launcherTest.strictUriComponentDecode(encoded, "resource project"), project);
    const publicProject = launcherTest.cleanPublicProjectPath(project);
    assert.equal(
      launcherTest.projectBoundResourceUri("codestory://status", project),
      `codestory://status?project=${launcherTest.strictUriComponentEncode(publicProject)}`,
    );
  }
  assert.equal(
    launcherTest.cleanPublicProjectPath(String.raw`\\?\C:\Code Story\repo`, "win32"),
    "C:/Code Story/repo",
  );
  assert.equal(
    launcherTest.cleanPublicProjectPath(String.raw`/tmp/a\b`, "linux"),
    String.raw`/tmp/a\b`,
  );
  for (const uri of [
    "codestory://status",
    "codestory://status?project=%2ftmp%2Frepo",
    "codestory://status?project=/tmp/repo",
    "codestory://status?project=%2Ftmp%2Frepo&project=%2Fother",
    "codestory://status?project=%ZZ",
  ]) {
    assert.throws(
      () => launcherTest.parseFailOpenResourceRequest(uri, undefined),
      /project|canonical|unknown/u,
      uri,
    );
  }
  assert.throws(
    () => launcherTest.parseFailOpenResourceRequest("codestory://agent-guide", "/tmp/repo"),
    /resource_project_unexpected/u,
  );
  const bound = launcherTest.parseFailOpenResourceRequest(statusUri, undefined);
  const legacy = launcherTest.parseFailOpenResourceRequest("codestory://status", repoRoot);
  assert.equal(bound.projectSource, "resource_uri");
  assert.equal(legacy.projectSource, "request_argument");
  assert.equal(bound.uri, legacy.uri);
});

test("fail-open handoff shutdown is bounded for a child that ignores stdin and SIGTERM", async () => {
  const child = new EventEmitter();
  child.stdin = new PassThrough();
  child.exitCode = null;
  child.signalCode = null;
  const signals = [];
  child.kill = (signal) => {
    signals.push(signal);
    if (signal === "SIGKILL") {
      child.signalCode = signal;
      child.emit("exit", null, signal);
      child.emit("close", null, signal);
    }
    return true;
  };

  launcherTest.shutdownHandoffChild(child, {
    handoffTerminationGraceMs: 1,
    handoffForceKillGraceMs: 1,
  });
  await delay(25);

  assert.equal(child.stdin.writableEnded, true);
  assert.deepEqual(signals, ["SIGTERM", "SIGKILL"]);
});
function threadActiveStatePath(dataDir, threadId) {
  const key = createHash("sha256").update(String(threadId)).digest("hex").slice(0, 16);
  return join(dataDir, `.codestory-active-thread-${key}.json`);
}

function readCargoVersion(manifestText) {
  let inPackage = false;
  for (const line of manifestText.split(/\r?\n/u)) {
    if (/^\[[^\]]+\]/u.test(line)) {
      inPackage = line.trim() === "[package]";
      continue;
    }
    if (!inPackage) {
      continue;
    }
    const versionMatch = line.match(/^version\s*=\s*"([^"]+)"/u);
    if (versionMatch) {
      return versionMatch[1];
    }
  }
  assert.fail("Cargo package must declare version");
}

async function readPluginVersion() {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  assert.equal(typeof manifest.version, "string");
  return manifest.version;
}

function releaseAssetForPlatform(version) {
  const target = process.platform === "win32" && process.arch === "x64"
    ? "windows-x64"
    : process.platform === "win32" && process.arch === "arm64"
      ? "windows-arm64"
      : process.platform === "linux" && process.arch === "x64"
        ? "linux-x64"
      : process.platform === "linux" && process.arch === "arm64"
          ? "linux-arm64"
          : process.platform === "darwin" && process.arch === "x64"
            ? "macos-x64"
            : process.platform === "darwin" && process.arch === "arm64"
              ? "macos-arm64"
              : null;
  assert.ok(target, `unsupported test platform: ${process.platform}-${process.arch}`);
  const archiveBase = `codestory-cli-v${version}-${target}`;
  const archiveName = `${archiveBase}.${target.startsWith("windows-") ? "zip" : "tar.gz"}`;
  return { archiveBase, archiveName };
}

function managedReleaseManifest(version, executablePath, sha256) {
  const { archiveName } = releaseAssetForPlatform(version);
  const target = archiveName.slice(`codestory-cli-v${version}-`.length).replace(/\.(?:zip|tar\.gz)$/u, "");
  return {
    path: executablePath,
    sha256,
    version,
    build_source: "github_release",
    repo_ref: `v${version}`,
    archive: archiveName,
    archive_url: `https://github.com/TheGreenCedar/CodeStory/releases/download/v${version}/${archiveName}`,
    archive_sha256: "0".repeat(64),
    target,
    stdio_initialize_verified: true,
  };
}

function explicitPackageManifest(version, executablePath, sha256) {
  const manifest = managedReleaseManifest(version, executablePath, sha256);
  return {
    ...manifest,
    build_source: "explicit_package",
    repo_ref: null,
    archive_url: `explicit-package:${manifest.archive_sha256}`,
  };
}

function crc32(content) {
  let crc = 0xffffffff;
  for (const byte of content) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function tarField(header, offset, length, value) {
  const encoded = value.toString(8).padStart(length - 1, "0");
  header.write(encoded, offset, length - 1, "ascii");
  header[offset + length - 1] = 0;
}

function tarGzFixture(name, content) {
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, "utf8");
  tarField(header, 100, 8, 0o755);
  tarField(header, 108, 8, 0);
  tarField(header, 116, 8, 0);
  tarField(header, 124, 12, content.length);
  tarField(header, 136, 12, 315532800);
  header.fill(0x20, 148, 156);
  header[156] = "0".charCodeAt(0);
  header.write("ustar\0", 257, 6, "ascii");
  header.write("00", 263, 2, "ascii");
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  header.write(checksum.toString(8).padStart(6, "0"), 148, 6, "ascii");
  header[154] = 0;
  header[155] = 0x20;
  const padding = Buffer.alloc((512 - (content.length % 512)) % 512);
  return gzipSync(Buffer.concat([header, content, padding, Buffer.alloc(1024)]), { mtime: 0 });
}

function rewriteTarChecksum(header) {
  header.fill(0x20, 148, 156);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  header.write(checksum.toString(8).padStart(6, "0"), 148, 6, "ascii");
  header[154] = 0;
  header[155] = 0x20;
}

function zipFixture(name, content, options = {}) {
  const encodedName = Buffer.from(name, "utf8");
  const compressed = deflateRawSync(content);
  const checksum = crc32(content);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  const flags = 0x800 | (options.dataDescriptor ? 0x8 : 0);
  local.writeUInt16LE(flags, 6);
  local.writeUInt16LE(8, 8);
  local.writeUInt32LE(options.dataDescriptor ? 0 : checksum, 14);
  local.writeUInt32LE(options.dataDescriptor ? 0 : compressed.length, 18);
  local.writeUInt32LE(options.dataDescriptor ? 0 : content.length, 22);
  local.writeUInt16LE(encodedName.length, 26);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(0x0314, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(flags, 8);
  central.writeUInt16LE(8, 10);
  central.writeUInt32LE(checksum, 16);
  central.writeUInt32LE(compressed.length, 20);
  central.writeUInt32LE(content.length, 24);
  central.writeUInt16LE(encodedName.length, 28);
  central.writeUInt32LE((0o100755 << 16) >>> 0, 38);
  const descriptor = options.dataDescriptor ? Buffer.alloc(16) : Buffer.alloc(0);
  if (options.dataDescriptor) {
    descriptor.writeUInt32LE(0x08074b50, 0);
    descriptor.writeUInt32LE(checksum, 4);
    descriptor.writeUInt32LE(compressed.length, 8);
    descriptor.writeUInt32LE(content.length, 12);
  }
  const centralOffset = local.length + encodedName.length + compressed.length + descriptor.length;
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(central.length + encodedName.length, 12);
  eocd.writeUInt32LE(centralOffset, 16);
  return Buffer.concat([local, encodedName, compressed, descriptor, central, encodedName, eocd]);
}

async function writeArchiveFixture(archivePath, entryName, content) {
  await writeFile(
    archivePath,
    archivePath.endsWith(".zip") ? zipFixture(entryName, content) : tarGzFixture(entryName, content),
  );
}

function fakeProbeChild(response, options = {}) {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.killSignals = [];
  child.kill = (signal = "SIGTERM") => {
    child.killSignals.push(signal);
    if (signal === "SIGKILL" && !options.ignoreSigkill) {
      process.nextTick(() => child.emit("exit", null, signal));
    }
    return true;
  };
  child.stdin = new Writable({
    write(_chunk, _encoding, callback) { callback(); },
    final(callback) {
      process.nextTick(() => {
        if (options.stdoutError) child.stdout.emit("error", new Error("synthetic stdout failure"));
        else child.stdout.write(`${JSON.stringify(response)}\n`);
      });
      callback();
    },
  });
  return child;
}

async function writeReleaseFixture(releaseDir, version, writeCli = writeFakeCli) {
  const { archiveBase, archiveName } = releaseAssetForPlatform(version);
  const stageDir = join(releaseDir, archiveBase);
  const cliName = process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli";
  const cliPath = join(stageDir, cliName);
  const archivePath = join(releaseDir, archiveName);
  await mkdir(stageDir, { recursive: true });
  await writeCli(cliPath);
  await writeArchiveFixture(archivePath, `${archiveBase}/${cliName}`, await readFile(cliPath));
  const archiveSha256 = createHash("sha256").update(await readFile(archivePath)).digest("hex");
  const sumsPath = join(releaseDir, "SHA256SUMS.txt");
  await writeFile(sumsPath, `${archiveSha256}  ${archiveName}\n`, "utf8");
  return { archiveName, archivePath, archiveSha256, cliName, sumsPath };
}

function spawnLauncher(launcher, env) {
  const child = spawn(process.execPath, [launcher], {
    env: { ...process.env, CODESTORY_CLI: "", ...env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "resources/read", params: { uri: statusUri } })}\n`);
  const runtimeMetadata = env.PLUGIN_DATA && join(env.PLUGIN_DATA, ".codestory-mcp-runtime.json");
  let handoffRequestId = 2;
  const handoffPoll = runtimeMetadata && setInterval(() => {
    try {
      if (child.exitCode !== null || (env.TEST_OUT && fs.existsSync(env.TEST_OUT))) {
        clearInterval(handoffPoll);
        return;
      }
      if (JSON.parse(fs.readFileSync(runtimeMetadata, "utf8")).source !== "managed") return;
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: handoffRequestId, method: "tools/list" })}\n`);
      handoffRequestId += 1;
    } catch {
      // Provisioning has not published runtime metadata yet.
    }
  }, 10);
  child.once("close", () => clearInterval(handoffPoll));
  const completed = once(child, "close").then(([status, signal]) => ({ status, signal, stdout, stderr }));
  return { child, completed };
}

async function waitForPath(pathname, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      await access(pathname);
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
  assert.fail(`timed out waiting for ${pathname}`);
}

async function writeFakeCli(cliPath) {
  const script = [
    "const fs=require('fs');const args=process.argv.slice(1);",
    "if(process.env.CODESTORY_PLUGIN_PROVISIONING_PROBE==='1'&&args[0]==='serve'){let input='';process.stdin.on('data',chunk=>{input+=chunk;const newline=input.indexOf('\\n');if(newline<0)return;const request=JSON.parse(input.slice(0,newline));process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{protocolVersion:request.params.protocolVersion,capabilities:{},serverInfo:{name:'fixture',version:'1'}}})+'\\n',()=>process.exit(0))})}",
    "else if(args[0]==='--version'){if(process.env.CODESTORY_PLUGIN_PROVISIONING_PROBE==='1'&&process.env.CODESTORY_TEST_PROBE_LOG)fs.appendFileSync(process.env.CODESTORY_TEST_PROBE_LOG,'probe\\n');const delay=Number(process.env.CODESTORY_TEST_PROBE_DELAY_MS||0);if(delay>0)Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,delay);console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}",
    "else{fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({source:process.env.CODESTORY_PLUGIN_CLI_SOURCE,path:process.env.CODESTORY_PLUGIN_CLI_PATH,sha256:process.env.CODESTORY_PLUGIN_CLI_SHA256,version:process.env.CODESTORY_PLUGIN_CLI_VERSION,warnings:process.env.CODESTORY_PLUGIN_CLI_WARNINGS,pluginRoot:process.env.CODESTORY_PLUGIN_ROOT,launchCwd:process.env.CODESTORY_PLUGIN_LAUNCH_CWD,runtimeCwd:process.env.CODESTORY_PLUGIN_RUNTIME_CWD,pluginCacheVersion:process.env.CODESTORY_PLUGIN_CACHE_VERSION,repoRef:process.env.CODESTORY_PLUGIN_CLI_REPO_REF,buildSource:process.env.CODESTORY_PLUGIN_CLI_BUILD_SOURCE,archiveSha256:process.env.CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256,retention:process.env.CODESTORY_PLUGIN_CLI_RETENTION,args}))}",
  ].join("");
  if (process.platform === "win32") {
    await writeFile(
      cliPath,
      `@echo off\r\n"${process.execPath}" -e "${script}" -- %*\r\n`,
      "utf8",
    );
    return;
  }
  await writeFile(
    cliPath,
    `#!/bin/sh\n${JSON.stringify(process.execPath)} -e ${JSON.stringify(script)} -- "$@"\n`,
    "utf8",
  );
  await chmod(cliPath, 0o755);
}

async function writeLifecycleCli(cliPath) {
  const script = [
    "const fs=require('fs');",
    "const args=process.argv.slice(1);",
    "if(args[0]==='--version'){console.log('codestory-cli '+process.env.TEST_CODESTORY_VERSION);process.exit(0)}",
    "if(args[0]!=='serve')process.exit(2);",
    "let initialized=false;let notified=false;let input='';",
    "process.stdin.setEncoding('utf8');",
    "process.stdin.on('data',chunk=>{input+=chunk;const lines=input.split(/\\r?\\n/u);input=lines.pop()||'';for(const line of lines){if(!line)continue;const request=JSON.parse(line);if(request.method==='initialize'){initialized=true;process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{protocolVersion:request.params.protocolVersion,capabilities:{tools:{listChanged:false},resources:{listChanged:false},prompts:{listChanged:false}},serverInfo:{name:'fixture',version:'1'}}})+'\\n')}else if(request.method==='notifications/initialized'){notified=true}else if(request.method==='tools/list'){if(!initialized||!notified)process.exit(42);fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({initialized,notified,args}));process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{tools:[]}})+'\\n')}else if(request.method==='resources/list'){process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{resources:[]}})+'\\n',()=>process.exit(17))}}});",
  ].join("");
  if (process.platform === "win32") {
    await writeFile(cliPath, `@echo off\r\n"${process.execPath}" -e "${script}" -- %*\r\n`, "utf8");
    return;
  }
  await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} -e ${JSON.stringify(script)} -- "$@"\n`, "utf8");
  await chmod(cliPath, 0o755);
}

async function writeVersionOnlyCli(cliPath) {
  if (process.platform === "win32") {
    await writeFile(cliPath, "@echo off\r\necho codestory-cli %TEST_CODESTORY_VERSION%\r\n", "utf8");
    return;
  }
  await writeFile(cliPath, "#!/bin/sh\necho codestory-cli \"$TEST_CODESTORY_VERSION\"\n", "utf8");
  await chmod(cliPath, 0o755);
}

async function writeManagedCliFixture(dataDir, version, body = version) {
  const cliName = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  const versionDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(versionDir, "bin", cliName);
  await mkdir(dirname(cliPath), { recursive: true });
  await writeFile(cliPath, body, "utf8");
  const sha256 = createHash("sha256").update(await readFile(cliPath)).digest("hex");
  await writeFile(
    join(versionDir, "manifest.json"),
    JSON.stringify({ path: `bin/${cliName}`, sha256, version }),
    "utf8",
  );
  return { cliPath, versionDir };
}

async function writeAttestedDevPluginFixture(root, version) {
  const { cp } = await import("node:fs/promises");
  const installRoot = join(
    root,
    ".codex",
    "plugins",
    "cache",
    "CodeStoryDev",
    "codestory",
    version,
  );
  await cp(pluginRoot, installRoot, { recursive: true });
  const sourcePackageSha256 = devCliContract.directoryContractSha256(installRoot);
  const cliName = devCliContract.expectedBinaryName();
  const cliPath = join(installRoot, "bin", cliName);
  await mkdir(dirname(cliPath), { recursive: true });
  await writeFakeCli(cliPath);
  const cliBytes = await readFile(cliPath);
  const cliSha256 = createHash("sha256").update(cliBytes).digest("hex");
  await writeFile(
    join(installRoot, devCliContract.receiptName),
    `${JSON.stringify({
      schema_version: devCliContract.receiptSchemaVersion,
      purpose: devCliContract.receiptPurpose,
      plugin_id: devCliContract.receiptPluginId,
      plugin_name: devCliContract.receiptPluginName,
      plugin_version: version,
      source_commit: "a".repeat(40),
      source_package_sha256: sourcePackageSha256,
      target: devCliContract.sourceBuildTarget(),
      cli: {
        path: `bin/${cliName}`,
        name: cliName,
        bytes: cliBytes.length,
        sha256: cliSha256,
        version,
      },
    }, null, 2)}\n`,
    "utf8",
  );
  return {
    cliPath,
    cliSha256,
    installRoot,
    launcher: join(installRoot, "scripts", "codestory-mcp.cjs"),
    sourcePackageSha256,
  };
}

test("plugin metadata maps skill and direct stdio server", async () => {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const mcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));
  const agentMetadata = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "agents", "openai.yaml"),
    "utf8",
  );

  assert.equal(manifest.name, "codestory");
  assert.equal(manifest.skills, "./skills/");
  assert.equal(manifest.hooks, "./hooks/claude-codex-hooks.json");
  assert.equal(manifest.mcpServers, "./.mcp.json");
  assert.equal(manifest.interface.capabilities.includes("Read"), true);
  assert.equal(
    manifest.interface.capabilities.includes(["Lifecycle", "hooks"].join(" ")),
    true,
  );
  assert.match(agentMetadata, /dependencies:\s*\r?\n\s+tools:/u);
  assert.match(agentMetadata, /type: "mcp"/u);
  assert.match(agentMetadata, /value: "codestory"/u);
  assert.match(agentMetadata, /allow_implicit_invocation: true/u);
  assert.equal(mcp.mcpServers.codestory.command, "node");
  assert.deepEqual(mcp.mcpServers.codestory.args, [
    "./scripts/codestory-mcp.cjs",
  ]);
  assert.equal(mcp.mcpServers.codestory.cwd, ".");
  assert.deepEqual(mcp.mcpServers.codestory.env, {});
});

test("agent-facing guidance keeps embedding lifecycle internal", async () => {
  const guidanceFiles = [
    join(pluginRoot, "hooks", "codestory-activate.cjs"),
    join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
    join(pluginRoot, "skills", "codestory-grounding", "agents", "openai.yaml"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "status-contract.md"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "doctor.md"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "serve.md"),
    join(repoRoot, "docs", "users", "troubleshooting.md"),
    join(repoRoot, "docs", "ops", "retrieval-engine.md"),
  ];

  for (const file of guidanceFiles) {
    const text = await readFile(file, "utf8");
    assert.doesNotMatch(text, /llama-server|sidecar setup|consent|ready --goal agent --repair/iu, file);
  }

  for (const file of [
    join(repoRoot, ".github", "copilot-instructions.md"),
    join(repoRoot, ".cursor", "rules", "codestory.mdc"),
    join(pluginRoot, ".cursor", "rules", "codestory.mdc"),
  ]) {
    const text = await readFile(file, "utf8");
    assert.match(text, /Call the CodeStory tool that matches the task/u, file);
    assert.doesNotMatch(text, /read `codestory:\/\/status` first/u, file);
    assert.doesNotMatch(text, /codestory-cli ready/u, file);
  }
});

test("plugin package version tracks the codestory-cli release version", async () => {
  const cliManifest = await readFile(
    join(repoRoot, "crates", "codestory-cli", "Cargo.toml"),
    "utf8",
  );
  const expectedVersion = readCargoVersion(cliManifest);
  const manifestPaths = [
    join(pluginRoot, ".codex-plugin", "plugin.json"),
    join(pluginRoot, ".claude-plugin", "plugin.json"),
    join(pluginRoot, ".github", "plugin", "plugin.json"),
  ];

  for (const manifestPath of manifestPaths) {
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    assert.equal(manifest.version, expectedVersion);
  }
});

test("source setup adapters prepare and pass the canonical embedded model", async () => {
  const [powershell, posix] = await Promise.all([
    readFile(join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.ps1"), "utf8"),
    readFile(join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.sh"), "utf8"),
  ]);

  for (const source of [powershell, posix]) {
    assert.match(source, /prepare-embedded-model\.mjs/u);
    assert.match(source, /CODESTORY_EMBED_MODEL_SOURCE/u);
    assert.match(source, /build[" ]*,?[" ]*--release/u);
    assert.match(source, /--locked/u);
  }
});

test("codestory repo ships plugin source, not marketplace catalog or server adapter runtime", async () => {
  await assert.rejects(
    access(join(repoRoot, ".agents", "plugins", "marketplace.json")),
  );
  await assert.rejects(
    access(join(pluginRoot, ".github", "plugin", "marketplace.json")),
  );
  await assert.rejects(
    access(
      join(repoRoot, ".agents", "skills", "codestory-grounding", "SKILL.md"),
    ),
  );
  await access(join(pluginRoot, "scripts", "codestory-mcp.cjs"));
});

test("dirty marker writer stores one project-keyed marker under plugin data", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-dirty-marker-"));
  const projectRoot = await mkdtemp(join(tmpdir(), "codestory-dirty-project-"));

  try {
    const realProjectRoot = await realpath(projectRoot);
    const first = writeDirtyMarker(projectRoot, {
      pluginDataDir: dataDir,
      dirty: true,
      source: "test-hook",
      pathSample: ["src/lib.rs", "src/changed.rs", ""],
    });
    const firstStat = await stat(first.path);
    const repeat = writeDirtyMarker(projectRoot, {
      pluginDataDir: dataDir,
      dirty: true,
      source: "test-hook",
      pathSample: ["src/lib.rs", "src/changed.rs", ""],
    });
    const repeatStat = await stat(first.path);
    const second = writeDirtyMarker(projectRoot, {
      pluginDataDir: dataDir,
      dirty: false,
      source: "test-hook",
    });

    assert.ok(first);
    assert.ok(repeat);
    assert.ok(second);
    assert.equal(repeat.unchanged, true);
    assert.equal(first.path, second.path);
    assert.equal(repeatStat.mtimeMs, firstStat.mtimeMs);
    assert.equal(first.path, dirtyMarkerPathForProject(projectRoot, dataDir));
    const marker = JSON.parse(await readFile(second.path, "utf8"));
    assert.equal(marker.schema_version, 1);
    assert.equal(marker.project_root, realProjectRoot);
    assert.equal(marker.dirty, false);
    assert.equal(marker.source, "test-hook");
    assert.equal(typeof marker.updated_at, "string");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(projectRoot, { recursive: true, force: true });
  }
});

test("dirty marker hook manager installs idempotently and preserves foreign hook content", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-data-"));
  const projectRoot = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-project-"));

  try {
    await mkdir(join(projectRoot, ".git", "hooks"), { recursive: true });
    const postMerge = join(projectRoot, ".git", "hooks", "post-merge");
    await writeFile(postMerge, "#!/bin/sh\necho foreign\n", "utf8");

    const before = dirtyHookStatus(projectRoot, { pluginDataDir: dataDir });
    assert.equal(before.status, "foreign_hook_present");

    const installed = installDirtyHooks(projectRoot, { pluginDataDir: dataDir });
    assert.equal(installed.status, "installed");
    assert.equal(installed.hooks.every((hook) => hook.state === "installed"), true);
    assert.equal(installed.hooks.every((hook) => hook.changed === true), true);
    const firstPostMerge = await readFile(postMerge, "utf8");
    assert.match(firstPostMerge, /echo foreign/u);
    assert.match(firstPostMerge, /codestory dirty marker/u);

    const repeated = installDirtyHooks(projectRoot, { pluginDataDir: dataDir });
    assert.equal(repeated.status, "installed");
    assert.equal(repeated.hooks.every((hook) => hook.changed === false), true);
    assert.equal(await readFile(postMerge, "utf8"), firstPostMerge);

    const uninstalled = uninstallDirtyHooks(projectRoot, { pluginDataDir: dataDir });
    assert.equal(uninstalled.status, "foreign_hook_present");
    assert.equal(await readFile(postMerge, "utf8"), "#!/bin/sh\necho foreign\n");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(projectRoot, { recursive: true, force: true });
  }
});

test("dirty marker hook command reports uninstall-required stale managed blocks", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-cli-data-"));
  const projectRoot = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-cli-project-"));
  const script = join(pluginRoot, "hooks", "codestory-dirty-hook.cjs");

  try {
    await mkdir(join(projectRoot, ".git", "hooks"), { recursive: true });
    const hookPath = join(projectRoot, ".git", "hooks", "post-checkout");
    await writeFile(
      hookPath,
      [
        "#!/bin/sh",
        "# >>> codestory dirty marker >>>",
        "node old-script.cjs mark --project old --plugin-data old || true",
        "# <<< codestory dirty marker <<<",
        "",
      ].join("\n"),
      "utf8",
    );

    const install = spawnSync(process.execPath, [
      script,
      "install",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
    ], { encoding: "utf8" });
    assert.equal(install.status, 0, install.stderr);
    const installed = JSON.parse(install.stdout);
    assert.equal(installed.status, "uninstall_required");
    assert.equal(installed.hooks.find((hook) => hook.hook === "post-checkout").state, "uninstall_required");

    const uninstall = spawnSync(process.execPath, [
      script,
      "uninstall",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
    ], { encoding: "utf8" });
    assert.equal(uninstall.status, 0, uninstall.stderr);
    assert.equal(JSON.parse(uninstall.stdout).status, "not_installed");

    const mark = spawnSync(process.execPath, [
      script,
      "mark",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
      "--source",
      "test-command",
    ], { encoding: "utf8" });
    assert.equal(mark.status, 0, mark.stderr);
    const markerResult = JSON.parse(mark.stdout);
    const marker = JSON.parse(await readFile(markerResult.path, "utf8"));
    assert.equal(marker.dirty, true);
    assert.equal(marker.source, "test-command");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(projectRoot, { recursive: true, force: true });
  }
});

test("mcp launcher prefers a checksummed explicit package without PATH", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-cli-"));
  const outFile = join(dataDir, "env.json");
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(
    cliDir,
    process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
  );
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const privateReleaseBaseUrl = "https://private-packages.invalid";

  try {
    await mkdir(cliDir, { recursive: true });
    await writeFakeCli(cliPath);
    const sha256 = createHash("sha256")
      .update(await readFile(cliPath))
      .digest("hex");
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify(explicitPackageManifest(
        version,
        process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
        sha256,
      )),
      "utf8",
    );
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: dataDir,
        TEST_OUT: outFile,
        TEST_CODESTORY_VERSION: version,
        CODESTORY_PLUGIN_RELEASE_BASE_URL: privateReleaseBaseUrl,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      input: launcherHandoffInput(),
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(await realpath(observed.path), await realpath(cliPath));
    assert.equal(observed.sha256, sha256);
    const retention = JSON.parse(observed.retention);
    assert.deepEqual(
      retention.retained.map((entry) => entry.version),
      [version],
      JSON.stringify(retention),
    );
    assert.equal(retention.reclaimable_bytes, 0);
    assert.equal(observed.pluginRoot, pluginRoot);
    assert.equal(observed.pluginCacheVersion, "");
    assert.deepEqual(observed.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher uses an attested CodeStoryDev CLI from the installed cache without PATH", async () => {
  if (process.platform === "win32") return;
  const root = await mkdtemp(join(tmpdir(), "codestory-attested-dev-cli-"));
  const dataDir = join(root, "plugin-data");
  const outFile = join(root, "env.json");
  const version = await readPluginVersion();
  try {
    const fixture = await writeAttestedDevPluginFixture(root, version);
    await mkdir(dataDir, { recursive: true });
    const result = spawnSync(process.execPath, [fixture.launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_OUT: outFile,
        PATH: "",
      },
      input: launcherHandoffInput(),
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "local_dev_override");
    assert.equal(observed.buildSource, "codestory_dev_receipt");
    assert.equal(observed.sha256, fixture.cliSha256);
    assert.equal(await realpath(observed.path), await realpath(fixture.cliPath));
    assert.equal(await realpath(observed.pluginRoot), await realpath(fixture.installRoot));
    assert.equal(observed.pluginCacheVersion, version);
    assert.match(observed.warnings, /codestory_dev_receipt:verified/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("declared CodeStoryDev receipt failures never fall through to raw or managed CLI selection", async () => {
  if (process.platform === "win32") return;
  const version = await readPluginVersion();
  for (const variant of ["invalid-receipt", "ambiguous-raw-override"]) {
    const root = await mkdtemp(join(tmpdir(), "codestory-dev-receipt-no-fallback-"));
    const dataDir = join(root, "plugin-data");
    const runtimeOut = join(root, "runtime.json");
    try {
      const fixture = await writeAttestedDevPluginFixture(root, version);
      const managedDir = join(dataDir, "codestory-cli", version);
      const managedCli = join(managedDir, process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
      await mkdir(managedDir, { recursive: true });
      await writeFakeCli(managedCli);
      const managedSha256 = createHash("sha256").update(await readFile(managedCli)).digest("hex");
      await writeFile(
        join(managedDir, "manifest.json"),
        JSON.stringify(managedReleaseManifest(version, managedCli.slice(managedDir.length + 1), managedSha256)),
        "utf8",
      );
      if (variant === "invalid-receipt") {
        await writeFile(join(fixture.installRoot, "README.md"), "changed package bytes", "utf8");
      }
      const input = `${JSON.stringify({
        jsonrpc: "2.0",
        id: variant,
        method: "resources/read",
        params: { uri: statusUri },
      })}\n`;
      const result = spawnSync(process.execPath, [fixture.launcher], {
        env: {
          ...process.env,
          CODESTORY_CLI: variant === "ambiguous-raw-override" ? fixture.cliPath : "",
          CODESTORY_PLUGIN_DISABLE_PROVISION: "1",
          PLUGIN_DATA: dataDir,
          TEST_CODESTORY_VERSION: version,
          TEST_OUT: runtimeOut,
          PATH: "",
        },
        input,
        encoding: "utf8",
        timeout: 5000,
      });
      assert.equal(result.status, 0, result.stderr);
      const response = JSON.parse(result.stdout.trim());
      const status = JSON.parse(response.result.contents[0].text);
      assert.equal(status.plugin_runtime.cli_source, "local_dev_receipt_invalid");
      assert.equal(status.plugin_runtime.cli_path, null);
      assert.equal(status.plugin_runtime.managed_binary_path, null);
      if (variant === "invalid-receipt") {
        assert.equal(
          status.degraded_reason,
          "codestory_dev_receipt_invalid:codestory_dev_receipt_package_digest",
        );
      } else {
        assert.equal(status.degraded_reason, "codestory_dev_cli_ambiguous_override");
      }
      assert.equal(fs.existsSync(runtimeOut), false, `${variant} unexpectedly launched a runtime`);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test("candidate managed CLI metadata is accepted only for the exact proof archive", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-candidate-cli-"));
  const version = "0.0.1";
  const archiveSha256 = "a".repeat(64);
  const qualificationNonce = "c".repeat(64);
  const qualificationDir = join(dataDir, "qualification");
  const target = releaseAssetForPlatform(version).archiveName
    .slice(`codestory-cli-v${version}-`.length)
    .replace(/\.(?:zip|tar\.gz)$/u, "");
  try {
    const fixture = await writeManagedCliFixture(dataDir, version);
    const manifest = managedReleaseManifest(
      version,
      fixture.cliPath.slice(fixture.versionDir.length + 1),
      createHash("sha256").update(await readFile(fixture.cliPath)).digest("hex"),
    );
    manifest.build_source = "candidate_archive";
    manifest.repo_ref = "b".repeat(40);
    manifest.archive_sha256 = archiveSha256;
    manifest.archive_url = `candidate-archive:${archiveSha256}`;
    await writeFile(
      join(fixture.versionDir, "manifest.json"),
      JSON.stringify(manifest),
      "utf8",
    );
    const probe = () => ({
      status: 0,
      error: null,
      version,
      stdout: "",
      stderr: "",
    });
    assert.equal(
      launcherTest.verifyPublishedManagedCli(
        fixture.versionDir,
        version,
        target,
        probe,
      ).verified,
      false,
    );
    process.env.CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256 = archiveSha256;
    assert.equal(
      launcherTest.verifyPublishedManagedCli(
        fixture.versionDir,
        version,
        target,
        probe,
      ).verified,
      false,
    );
    await mkdir(qualificationDir, { mode: 0o700 });
    await writeFile(
      join(qualificationDir, "candidate-managed-install.json"),
      JSON.stringify({
        schema_version: 1,
        purpose: "codestory-candidate-managed-install",
        archive_sha256: archiveSha256,
        qualification_nonce_sha256: createHash("sha256")
          .update(qualificationNonce)
          .digest("hex"),
      }),
      { encoding: "utf8", mode: 0o600 },
    );
    process.env.CODESTORY_EMBED_QUALIFICATION_DIR = await realpath(qualificationDir);
    process.env.CODESTORY_EMBED_QUALIFICATION_NONCE = qualificationNonce;
    assert.equal(
      launcherTest.verifyPublishedManagedCli(
        fixture.versionDir,
        version,
        target,
        probe,
      ).verified,
      true,
    );
  } finally {
    delete process.env.CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256;
    delete process.env.CODESTORY_EMBED_QUALIFICATION_DIR;
    delete process.env.CODESTORY_EMBED_QUALIFICATION_NONCE;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("explicit package provenance cannot satisfy public release verification", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-explicit-provenance-"));
  const version = "0.0.1";
  const target = releaseAssetForPlatform(version).archiveBase
    .slice(`codestory-cli-v${version}-`.length);
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousBaseUrl = process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL;
  try {
    const fixture = await writeManagedCliFixture(dataDir, version);
    const sha256 = createHash("sha256").update(await readFile(fixture.cliPath)).digest("hex");
    const explicit = explicitPackageManifest(
      version,
      fixture.cliPath.slice(fixture.versionDir.length + 1),
      sha256,
    );
    await writeFile(
      join(fixture.versionDir, "manifest.json"),
      JSON.stringify(explicit),
      "utf8",
    );
    const probe = () => ({
      status: 0,
      error: null,
      version,
      stdout: "",
      stderr: "",
    });

    delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    delete process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL;
    assert.equal(
      launcherTest.verifyPublishedManagedCli(fixture.versionDir, version, target, probe).verified,
      false,
    );

    process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL = "https://private-packages.invalid";
    assert.equal(
      launcherTest.verifyPublishedManagedCli(fixture.versionDir, version, target, probe).verified,
      true,
    );

    await writeFile(
      join(fixture.versionDir, "manifest.json"),
      JSON.stringify(managedReleaseManifest(
        version,
        fixture.cliPath.slice(fixture.versionDir.length + 1),
        sha256,
      )),
      "utf8",
    );
    delete process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL;
    assert.equal(
      launcherTest.verifyPublishedManagedCli(fixture.versionDir, version, target, probe).verified,
      true,
    );
    process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL = "https://private-packages.invalid";
    assert.equal(
      launcherTest.verifyPublishedManagedCli(fixture.versionDir, version, target, probe).verified,
      false,
    );
    const mislabeledPrivate = managedReleaseManifest(
      version,
      fixture.cliPath.slice(fixture.versionDir.length + 1),
      sha256,
    );
    mislabeledPrivate.archive_url =
      `https://private-packages.invalid/${mislabeledPrivate.archive}`;
    await writeFile(
      join(fixture.versionDir, "manifest.json"),
      JSON.stringify(mislabeledPrivate),
      "utf8",
    );
    assert.equal(
      launcherTest.verifyPublishedManagedCli(fixture.versionDir, version, target, probe).verified,
      false,
    );
  } finally {
    if (previousReleaseDir === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    else process.env.CODESTORY_PLUGIN_RELEASE_DIR = previousReleaseDir;
    if (previousBaseUrl === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL;
    else process.env.CODESTORY_PLUGIN_RELEASE_BASE_URL = previousBaseUrl;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("plugin path comparison uses file identity and platform missing-path rules", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-path-identity-"));
  const executable = join(root, "codestory-cli");
  const hardLink = join(root, "codestory-cli-link");
  try {
    await writeFile(executable, "fixture", "utf8");
    await link(executable, hardLink);
    assert.equal(launcherTest.sameFilesystemPath(executable, hardLink), true);
    assert.equal(launcherTest.sameFilesystemPath(executable, join(root, "missing")), false);
    assert.equal(
      launcherTest.sameFilesystemPath(join(root, "Missing"), join(root, "missing")),
      process.platform === "win32",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("managed cli retention keeps active plus a verified adjacent version", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-"));
  try {
    assert.equal(launcherTest.compareManagedCliVersions("0.14.10", "0.14.9"), 1);
    const oldest = await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const newer = await writeManagedCliFixture(dataDir, "0.14.2");
    const malformedDir = join(dataDir, "codestory-cli", "0.13.9");
    await mkdir(malformedDir, { recursive: true });
    await writeFile(join(malformedDir, "partial"), "stale", "utf8");
    const probeVersion = (resolved) => ({
      status: 0,
      error: null,
      version: resolved.version,
      stdout: "",
      stderr: "",
    });
    const resolved = {
      source: "managed",
      version: "0.14.1",
      path: active.cliPath,
      warnings: [],
    };
    const probe = probeVersion(resolved);

    const dryRun = launcherTest.managedCliRetentionReport(resolved, probe, {
      dataDir,
      dryRun: true,
      probeVersion,
    });
    assert.deepEqual(dryRun.retained.map((entry) => entry.version), ["0.14.2", "0.14.1"]);
    assert.deepEqual(dryRun.reclaimable.map((entry) => entry.version), ["0.14.0", "0.13.9"]);
    assert.equal(dryRun.removed_bytes, 0);
    assert.equal(dryRun.reclaimable_bytes > 0, true);
    await access(oldest.versionDir);
    await access(malformedDir);

    const applied = launcherTest.managedCliRetentionReport(resolved, probe, {
      dataDir,
      probeVersion,
    });
    assert.deepEqual(applied.retained.map((entry) => entry.version), ["0.14.2", "0.14.1"]);
    assert.deepEqual(applied.removed.map((entry) => entry.version), ["0.14.0", "0.13.9"]);
    assert.equal(applied.removed_bytes, dryRun.reclaimable_bytes);
    await assert.rejects(access(oldest.versionDir));
    await assert.rejects(access(malformedDir));
    await access(active.versionDir);
    await access(newer.versionDir);

    const afterActivation = launcherTest.managedCliRetentionReport(
      { ...resolved, version: "0.14.2", path: newer.cliPath },
      { ...probe, version: "0.14.2" },
      { dataDir, dryRun: true, probeVersion },
    );
    assert.deepEqual(afterActivation.retained.map((entry) => entry.version), ["0.14.2", "0.14.1"]);
    assert.equal(afterActivation.retained.find((entry) => entry.version === "0.14.1").reason, "rollback");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention reports a locked Windows executable without pruning it", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-lock-"));
  try {
    const stale = await writeManagedCliFixture(dataDir, "0.13.9");
    const rollback = await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const probeVersion = (resolved) => ({
      status: 0,
      error: null,
      version: resolved.version,
      stdout: "",
      stderr: "",
    });
    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probeVersion({ version: "0.14.1" }),
      {
        dataDir,
        platform: "win32",
        probeVersion,
        unlinkSync(pathname) {
          if (pathname.startsWith(stale.versionDir)) {
            const error = new Error("locked");
            error.code = "EPERM";
            throw error;
          }
          return rm(pathname, { force: false });
        },
      },
    );

    assert.deepEqual(report.retained.map((entry) => entry.version), ["0.14.1", "0.14.0"]);
    assert.equal(report.reclaimable.find((entry) => entry.version === "0.13.9").reason, "locked:EPERM");
    assert.equal(report.removed_bytes, 0);
    await access(stale.versionDir);
    await access(rollback.versionDir);
    await access(active.versionDir);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention suppresses deletion when the active manifest escapes its version", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-escape-"));
  try {
    const stale = await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const outside = join(dataDir, "outside-cli");
    await writeFile(outside, "outside", "utf8");
    const outsideSha = createHash("sha256").update(await readFile(outside)).digest("hex");
    await writeFile(
      join(active.versionDir, "manifest.json"),
      JSON.stringify({ path: "../../outside-cli", sha256: outsideSha, version: "0.14.1" }),
      "utf8",
    );
    const probe = { status: 0, error: null, version: "0.14.1", stdout: "", stderr: "" };

    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probe,
      {
        dataDir,
        probeVersion: () => probe,
      },
    );

    assert.equal(
      report.warnings.some((warning) => warning.includes("active_unverified:manifest_path_unsafe")),
      true,
    );
    assert.equal(report.removed.length, 0);
    await access(stale.versionDir);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention reclaims an abandoned lock and provisioning sentinel", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-abandoned-"));
  try {
    const stale = await writeManagedCliFixture(dataDir, "0.13.9");
    await writeFile(join(stale.versionDir, ".provisioning"), "2147483647\n", "utf8");
    await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const lockDir = join(dataDir, "codestory-cli", ".retention-lock");
    await mkdir(lockDir);
    await writeFile(
      join(lockDir, "owner.json"),
      JSON.stringify({
        pid: 2147483647,
        token: "abandoned",
        purpose: "retention",
        process_start_identity: "dead:process",
        started_at: "2000-01-01T00:00:00.000Z",
      }),
      "utf8",
    );
    const probeVersion = (resolved) => ({
      status: 0,
      error: null,
      version: resolved.version,
      stdout: "",
      stderr: "",
    });

    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probeVersion({ version: "0.14.1" }),
      { dataDir, probeVersion },
    );

    assert.deepEqual(report.removed.map((entry) => entry.version), ["0.13.9"]);
    await assert.rejects(access(stale.versionDir));
    await assert.rejects(access(lockDir));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention never reclaims an old lock owned by the live process", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-live-lock-"));
  try {
    const processStartIdentity = launcherTest.processStartIdentity(process.pid);
    assert.ok(processStartIdentity, `process start identity unavailable on ${process.platform}`);
    const stale = await writeManagedCliFixture(dataDir, "0.13.9");
    await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const lockDir = join(dataDir, "codestory-cli", ".retention-lock");
    await mkdir(lockDir);
    await writeFile(join(lockDir, "owner.json"), JSON.stringify({
      pid: process.pid,
      token: "live",
      purpose: "retention",
      process_start_identity: processStartIdentity,
      started_at: "2000-01-01T00:00:00.000Z",
    }), "utf8");
    const probeVersion = (resolved) => ({ status: 0, error: null, version: resolved.version });
    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probeVersion({ version: "0.14.1" }),
      { dataDir, probeVersion },
    );
    assert.deepEqual(report.removed, []);
    assert.equal(report.warnings.includes("managed_cli_retention_locked"), true);
    await access(lockDir);
    await access(stale.versionDir);

    await writeFile(join(lockDir, "owner.json"), JSON.stringify({
      pid: process.pid,
      token: "reused-pid",
      purpose: "retention",
      process_start_identity: "different-process-start",
      started_at: "2000-01-01T00:00:00.000Z",
    }), "utf8");
    const reclaimed = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probeVersion({ version: "0.14.1" }),
      { dataDir, probeVersion },
    );
    assert.deepEqual(reclaimed.removed.map((entry) => entry.version), ["0.13.9"]);
    await assert.rejects(access(lockDir));
    await assert.rejects(access(stale.versionDir));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli lock fails closed when self process identity is unavailable", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-lock-no-identity-"));
  const root = join(dataDir, "codestory-cli");
  await mkdir(root);
  try {
    assert.throws(
      () => launcherTest.acquireManagedCliLock(root, "no-identity", 0, {
        processStartIdentity: () => null,
      }),
      /managed_cli_process_identity_unavailable/u,
    );
    assert.deepEqual(await readdir(root), []);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli pending-owner cleanup protects live and young artifacts", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-pending-owner-"));
  const root = join(dataDir, "codestory-cli");
  await mkdir(root);
  const identity = launcherTest.processStartIdentity(process.pid);
  assert.ok(identity, `process start identity unavailable on ${process.platform}`);
  const liveToken = "1".repeat(32);
  const deadToken = "2".repeat(32);
  const youngToken = "3".repeat(32);
  const oldToken = "4".repeat(32);
  const reusedToken = "5".repeat(32);
  const live = join(root, `.retention-lock.owner-${process.pid}-${liveToken}`);
  const dead = join(root, `.retention-lock.owner-2147483647-${deadToken}`);
  const young = join(root, `.retention-lock.owner-8-${youngToken}`);
  const old = join(root, `.retention-lock.owner-9-${oldToken}`);
  const reused = join(root, `.retention-lock.owner-${process.pid}-${reusedToken}`);
  try {
    await writeFile(live, JSON.stringify({
      pid: process.pid,
      purpose: "waiter",
      token: liveToken,
      process_start_identity: identity,
      started_at: "2000-01-01T00:00:00.000Z",
    }));
    await writeFile(dead, JSON.stringify({
      pid: 2147483647,
      purpose: "waiter",
      token: deadToken,
      process_start_identity: "dead:process",
      started_at: new Date().toISOString(),
    }));
    await writeFile(young, "{partial");
    await writeFile(old, "{malformed");
    await writeFile(reused, JSON.stringify({
      pid: process.pid,
      purpose: "waiter",
      token: reusedToken,
      process_start_identity: "different-process-start",
      started_at: "2000-01-01T00:00:00.000Z",
    }));
    const staleTime = new Date(Date.now() - 11 * 60 * 1000);
    await utimes(old, staleTime, staleTime);
    await utimes(reused, staleTime, staleTime);

    assert.equal(launcherTest.reclaimStaleManagedCliPendingOwners(root), 3);
    await access(live);
    await access(young);
    await assert.rejects(access(dead));
    await assert.rejects(access(old));
    await assert.rejects(access(reused));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli waiter covers both configured asset retry windows", () => {
  assert.ok(launcherTest.releaseAssetRetryBudgetMs > 3 * 60 * 1000);
  assert.ok(launcherTest.managedCliLockWaitMs >= 2 * launcherTest.releaseAssetRetryBudgetMs);
});

test("managed cli initializing reclaim preserves a new ABA owner", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-initializing-aba-"));
  const initializing = join(dataDir, ".retention-lock.initializing");
  const oldOwner = { pid: 1, token: "old", purpose: "old" };
  const newOwner = { pid: process.pid, token: "new", purpose: "new" };
  try {
    await writeFile(initializing, JSON.stringify(oldOwner));
    const removed = launcherTest.removeManagedCliInitializationIf(
      initializing,
      (owner) => owner?.token === oldOwner.token,
      {
        afterRename() {
          fs.writeFileSync(initializing, JSON.stringify(newOwner), { flag: "wx" });
        },
      },
    );
    assert.equal(removed, true);
    assert.deepEqual(JSON.parse(await readFile(initializing, "utf8")), newOwner);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli staging rejects a version-only binary without MCP initialize", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-stdio-probe-"));
  const cliPath = join(dataDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  try {
    await writeVersionOnlyCli(cliPath);
    await assert.rejects(launcherTest.probeManagedCliStdio(cliPath, 1000), /stdio_initialize_/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli staging uses direct executables and requires the exact MCP contract", async () => {
  assert.equal(launcherTest.isWindowsBatchCli("C:\\tools\\codestory-cli.cmd", "win32"), true);
  assert.equal(launcherTest.isWindowsBatchCli("C:\\tools\\codestory-cli.bat", "win32"), true);
  assert.equal(launcherTest.isWindowsBatchCli("C:\\tools\\codestory-cli.exe", "win32"), false);
  assert.throws(
    () => launcherTest.requireDirectCli("C:\\tools\\codestory-cli.cmd", "win32"),
    /codestory_cli_batch_override_rejected/u,
  );
  const incompatible = {
    jsonrpc: "2.0",
    id: "managed-cli-staging",
    result: {
      protocolVersion: "2099-01-01",
      capabilities: [],
      serverInfo: { name: "", version: 1 },
    },
  };
  let spawnOptions;
  await assert.rejects(
    launcherTest.probeManagedCliStdio("fixture", 100, {
      spawn: (_file, _args, options) => {
        spawnOptions = options;
        return fakeProbeChild(incompatible);
      },
      terminationGraceMs: 5,
      forceKillGraceMs: 20,
    }),
    /stdio_initialize_incompatible/u,
  );
  assert.equal(spawnOptions.shell, false);
});

test("managed cli staging escalates and awaits a stubborn child", async () => {
  const compatible = {
    jsonrpc: "2.0",
    id: "managed-cli-staging",
    result: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      serverInfo: { name: "fixture", version: "1" },
    },
  };
  const child = fakeProbeChild(compatible);
  await launcherTest.probeManagedCliStdio("fixture", 100, {
    spawn: () => child,
    terminationGraceMs: 5,
    forceKillGraceMs: 20,
  });
  assert.deepEqual(child.killSignals, ["SIGTERM", "SIGKILL"]);

  await assert.rejects(
    launcherTest.probeManagedCliStdio("fixture", 100, {
      spawn: () => fakeProbeChild(compatible, { ignoreSigkill: true }),
      terminationGraceMs: 5,
      forceKillGraceMs: 10,
    }),
    /stdio_initialize_termination_timeout/u,
  );
});

test("managed cli staging bounds output and handles stream errors", async () => {
  await assert.rejects(
    launcherTest.probeManagedCliStdio("fixture", 100, {
      spawn: () => fakeProbeChild(null, { stdoutError: true }),
      terminationGraceMs: 5,
      forceKillGraceMs: 20,
    }),
    /managed_cli_stdio_initialize_stdout/u,
  );
  const child = fakeProbeChild(null);
  child.stdin = new Writable({
    write(_chunk, _encoding, callback) { callback(); },
    final(callback) {
      child.stdout.write("x".repeat(70 * 1024));
      callback();
    },
  });
  await assert.rejects(
    launcherTest.probeManagedCliStdio("fixture", 100, {
      spawn: () => child,
      terminationGraceMs: 5,
      forceKillGraceMs: 20,
    }),
    /stdio_initialize_stdout_limit/u,
  );
});

test("managed cli staging preserves the complete pinned native generation", async () => {
  const version = await readPluginVersion();
  const { archiveBase, archiveName } = releaseAssetForPlatform(version);
  const root = await mkdtemp(join(tmpdir(), "codestory-managed-layout-"));
  const extractDir = join(root, "extract");
  const packageRoot = join(extractDir, archiveBase);
  const stagingDir = join(root, "staging");
  const launcherName = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  const generation = "a".repeat(64);
  const generationDir = join(packageRoot, "codestory-native-generations", generation);
  try {
    await mkdir(generationDir, { recursive: true });
    await mkdir(stagingDir);
    await writeFile(join(packageRoot, launcherName), "launcher");
    await writeFile(
      join(packageRoot, "codestory-native-current-generation-v1.txt"),
      `${generation}\n`,
    );
    await writeFile(
      join(generationDir, process.platform === "win32"
        ? "codestory-cli-runtime.exe"
        : "codestory-cli-runtime"),
      "runtime",
    );
    await writeFile(join(generationDir, "native-library"), "library");

    assert.equal(
      launcherTest.stageExtractedManagedCli(extractDir, archiveName, stagingDir),
      join(stagingDir, launcherName),
    );
    assert.equal(
      await readFile(join(stagingDir, "codestory-native-current-generation-v1.txt"), "utf8"),
      `${generation}\n`,
    );
    assert.equal(
      await readFile(join(stagingDir, "codestory-native-generations", generation, "native-library"), "utf8"),
      "library",
    );

    await writeFile(join(packageRoot, "manifest.json"), "hostile");
    const rejectedStage = join(root, "rejected");
    await mkdir(rejectedStage);
    assert.throws(
      () => launcherTest.stageExtractedManagedCli(extractDir, archiveName, rejectedStage),
      /managed_cli_archive_reserved_path:manifest\.json/u,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("managed cli extracts zip and tar.gz with Node platform APIs", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-native-extract-"));
  const content = Buffer.from("native archive fixture\n");
  try {
    for (const extension of ["zip", "tar.gz"]) {
      const archive = join(dataDir, `fixture.${extension}`);
      const destination = join(dataDir, `extract-${extension.replace(".", "-")}`);
      await writeArchiveFixture(archive, "release/bin/codestory-cli", content);
      launcherTest.extractArchive(archive, destination);
      assert.deepEqual(await readFile(join(destination, "release", "bin", "codestory-cli")), content);
    }
    const descriptorArchive = join(dataDir, "descriptor.zip");
    await writeFile(
      descriptorArchive,
      zipFixture("release/bin/codestory-cli", content, { dataDescriptor: true }),
    );
    const descriptorDestination = join(dataDir, "extract-descriptor");
    launcherTest.extractArchive(descriptorArchive, descriptorDestination);
    assert.deepEqual(
      await readFile(join(descriptorDestination, "release", "bin", "codestory-cli")),
      content,
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli archive extraction fails closed on bombs and malformed metadata", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-bad-archive-"));
  const content = Buffer.from("fixture\n");
  const extract = (archive) => launcherTest.extractArchive(archive, join(dataDir, `out-${Math.random()}`));
  try {
    const crcArchive = join(dataDir, "crc.zip");
    const crcBytes = zipFixture("release/codestory-cli", content);
    const central = crcBytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    crcBytes.writeUInt32LE(0, 14);
    crcBytes.writeUInt32LE(0, central + 16);
    await writeFile(crcArchive, crcBytes);
    assert.throws(() => extract(crcArchive), /zip_entry_crc_mismatch/u);

    const bombArchive = join(dataDir, "bomb.zip");
    const bombBytes = zipFixture("release/codestory-cli", content);
    const bombCentral = bombBytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    bombBytes.writeUInt32LE(300 * 1024 * 1024, bombCentral + 24);
    await writeFile(bombArchive, bombBytes);
    assert.throws(() => extract(bombArchive), /archive_entry_size_limit_exceeded/u);

    const nameArchive = join(dataDir, "name.zip");
    const nameBytes = zipFixture("release/codestory-cli", content);
    nameBytes[30] ^= 1;
    await writeFile(nameArchive, nameBytes);
    assert.throws(() => extract(nameArchive), /zip_local_name_mismatch/u);

    for (const [label, mutate] of [
      ["flags", (bytes) => bytes.writeUInt16LE(0x808, 6)],
      ["method", (bytes) => bytes.writeUInt16LE(0, 8)],
      ["crc", (bytes) => bytes.writeUInt32LE(0, 14)],
      ["compressed-size", (bytes) => bytes.writeUInt32LE(1, 18)],
      ["uncompressed-size", (bytes) => bytes.writeUInt32LE(1, 22)],
    ]) {
      const archive = join(dataDir, `local-${label}.zip`);
      const bytes = zipFixture("release/codestory-cli", content);
      mutate(bytes);
      await writeFile(archive, bytes);
      assert.throws(() => extract(archive), /zip_local_metadata_mismatch/u);
    }

    const descriptorArchive = join(dataDir, "bad-descriptor.zip");
    const descriptorBytes = zipFixture("release/codestory-cli", content, { dataDescriptor: true });
    const descriptorCentral = descriptorBytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    descriptorBytes.writeUInt32LE(0, descriptorCentral - 12);
    await writeFile(descriptorArchive, descriptorBytes);
    assert.throws(() => extract(descriptorArchive), /zip_data_descriptor_mismatch/u);

    const commentArchive = join(dataDir, "comment.zip");
    const commentBytes = zipFixture("release/codestory-cli", content);
    commentBytes.writeUInt16LE(4, commentBytes.length - 2);
    await writeFile(commentArchive, commentBytes);
    assert.throws(() => extract(commentArchive), /zip_end_of_central_directory_missing/u);

    for (const [name, mode] of [["../escape", 0o100755], ["release/link", 0o120777]]) {
      const archive = join(dataDir, `${mode}.zip`);
      const bytes = zipFixture(name, content);
      const directory = bytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
      bytes.writeUInt32LE((mode << 16) >>> 0, directory + 38);
      await writeFile(archive, bytes);
      assert.throws(
        () => extract(archive),
        mode === 0o120777 ? /zip_symlink_unsupported/u : /archive_path_escape/u,
      );
    }

    const malformedTar = join(dataDir, "malformed.tar.gz");
    const malformed = gunzipSync(tarGzFixture("release/codestory-cli", content));
    malformed.fill("z".charCodeAt(0), 124, 136);
    rewriteTarChecksum(malformed.subarray(0, 512));
    await writeFile(malformedTar, gzipSync(malformed));
    assert.throws(() => extract(malformedTar), /tar_numeric_field_invalid/u);

    const unterminatedTar = join(dataDir, "unterminated.tar.gz");
    const unterminated = gunzipSync(tarGzFixture("release/codestory-cli", content));
    await writeFile(unterminatedTar, gzipSync(unterminated.subarray(0, unterminated.length - 512)));
    assert.throws(() => extract(unterminatedTar), /tar_terminator_invalid|tar_terminator_missing/u);

    const tarBomb = join(dataDir, "bomb.tar.gz");
    const tarBombBytes = gunzipSync(tarGzFixture("release/codestory-cli", content));
    tarField(tarBombBytes, 124, 12, 300 * 1024 * 1024);
    rewriteTarChecksum(tarBombBytes.subarray(0, 512));
    await writeFile(tarBomb, gzipSync(tarBombBytes));
    assert.throws(() => extract(tarBomb), /archive_entry_size_limit_exceeded/u);

    for (const [label, type] of [["extended", "x"], ["global", "g"]]) {
      const paxTar = join(dataDir, `bad-pax-${label}.tar.gz`);
      const paxBytes = gunzipSync(tarGzFixture("PaxHeader", Buffer.from("8x p=a\n")));
      paxBytes[156] = type.charCodeAt(0);
      rewriteTarChecksum(paxBytes.subarray(0, 512));
      await writeFile(paxTar, gzipSync(paxBytes));
      assert.throws(() => extract(paxTar), /tar_pax_length_invalid/u);
    }

    for (const [filename, type, expected] of [
      ["../escape", "0", /archive_path_escape/u],
      ["release/link", "2", /tar_entry_type_unsupported/u],
    ]) {
      const archive = join(dataDir, `tar-${type.charCodeAt(0)}.tar.gz`);
      const bytes = gunzipSync(tarGzFixture("release/codestory-cli", content));
      bytes.fill(0, 0, 100);
      bytes.write(filename, 0, 100, "utf8");
      bytes[156] = type.charCodeAt(0);
      rewriteTarChecksum(bytes.subarray(0, 512));
      await writeFile(archive, gzipSync(bytes));
      assert.throws(() => extract(archive), expected);
    }
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli pending-owner cleanup skips identity probes for 64 young live records", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-young-pending-"));
  const root = join(dataDir, "codestory-cli");
  await mkdir(root);
  let identityProbes = 0;
  try {
    for (let index = 0; index < 64; index += 1) {
      const token = index.toString(16).padStart(32, "0");
      await writeFile(
        join(root, `.retention-lock.owner-${process.pid}-${token}`),
        JSON.stringify({
          pid: process.pid,
          purpose: "waiter",
          token,
          process_start_identity: "young-live-owner",
          started_at: new Date().toISOString(),
        }),
      );
    }

    assert.equal(launcherTest.reclaimStaleManagedCliPendingOwners(root, true, () => {
      identityProbes += 1;
      return "young-live-owner";
    }), 0);
    assert.equal(identityProbes, 0);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli publication removes a killed waiter's pending owner", { timeout: 15000 }, async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-killed-waiter-"));
  const root = join(dataDir, "codestory-cli");
  const launcherPath = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  await mkdir(root);
  const held = launcherTest.acquireManagedCliLock(root, "holder");
  assert.ok(held);
  const childScript = String.raw`
    require(process.argv[1])._test.acquireManagedCliLock(process.argv[2], 'waiter', 60000);
  `;
  const waiter = spawn(process.execPath, ["-e", childScript, launcherPath, root], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  let waiterStderr = "";
  waiter.stderr.on("data", (chunk) => { waiterStderr += chunk; });
  const completed = once(waiter, "close");
  try {
    const prefix = `.retention-lock.owner-${waiter.pid}-`;
    const deadline = Date.now() + 5000;
    let pending;
    while (Date.now() < deadline && !pending) {
      pending = (await readdir(root)).find((name) => name.startsWith(prefix));
      if (!pending) await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assert.ok(pending, waiterStderr);
    waiter.kill("SIGKILL");
    await completed;
    await access(join(root, pending));

    launcherTest.releaseManagedCliLock(held);
    const recovered = launcherTest.acquireManagedCliLock(root, "recovered");
    assert.ok(recovered);
    launcherTest.releaseManagedCliLock(recovered);
    assert.equal(
      (await readdir(root)).some((name) => name.startsWith(".retention-lock.owner-")),
      false,
    );
  } finally {
    waiter.kill("SIGKILL");
    await completed;
    try { launcherTest.releaseManagedCliLock(held); } catch {}
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli publication recovers a killed initializer before owner publication", { timeout: 15000 }, async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-lock-initialization-"));
  const root = join(dataDir, "codestory-cli");
  const lockPath = join(root, ".retention-lock");
  const readyPath = join(dataDir, "initializer-ready");
  const launcherPath = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const childScript = String.raw`
    const fs = require('fs');
    const path = require('path');
    const launcher = require(process.argv[1])._test;
    const root = process.argv[2];
    const ready = process.argv[3];
    const linkSync = fs.linkSync;
    fs.linkSync = (existing, destination) => {
      if (path.basename(destination) === 'owner.json') {
        fs.writeFileSync(ready, 'ready');
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 60000);
      }
      return linkSync(existing, destination);
    };
    launcher.acquireManagedCliLock(root, 'initializer', 1000);
  `;
  await mkdir(root, { recursive: true });
  const initializer = spawn(process.execPath, ["-e", childScript, launcherPath, root, readyPath], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  let initializerStderr = "";
  initializer.stderr.on("data", (chunk) => { initializerStderr += chunk; });
  const completed = once(initializer, "close");
  try {
    await waitForPath(readyPath);
    await access(lockPath);
    await assert.rejects(access(join(lockPath, "owner.json")));
    const initializationOwner = JSON.parse(await readFile(`${lockPath}.initializing`, "utf8"));
    assert.equal(initializationOwner.pid, initializer.pid);

    const blocked = launcherTest.acquireManagedCliLock(root, "live-contender", 100);
    assert.equal(blocked, null, "a live initializer must retain its claim");
    await access(`${lockPath}.initializing`);
    await assert.rejects(access(join(lockPath, "owner.json")));

    initializer.kill("SIGKILL");
    await completed;
    const startedAt = Date.now();
    const recovered = launcherTest.acquireManagedCliLock(root, "recovered", 2000);
    assert.ok(recovered, initializerStderr);
    assert.equal(recovered.waited, true);
    assert.equal(recovered.reclaimed, true);
    assert.ok(Date.now() - startedAt < 2000, "recovery must beat the waiter timeout");
    launcherTest.releaseManagedCliLock(recovered);
    await assert.rejects(access(lockPath));
    await assert.rejects(access(`${lockPath}.initializing`));
    assert.equal(
      (await readdir(root)).some((name) => name.startsWith(".retention-lock.owner-")),
      false,
    );
  } finally {
    initializer.kill("SIGKILL");
    await completed;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention inventories versions when the active probe fails", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-unhealthy-"));
  try {
    const old = await writeManagedCliFixture(dataDir, "0.14.0");
    const active = await writeManagedCliFixture(dataDir, "0.14.1");
    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      { status: 1, error: null, version: null, stdout: "", stderr: "broken" },
      { dataDir, dryRun: true },
    );

    assert.deepEqual(report.reclaimable.map((entry) => entry.version), ["0.14.1", "0.14.0"]);
    assert.equal(report.reclaimable_bytes > 0, true);
    assert.equal(report.removed.length, 0);
    await access(old.versionDir);
    await access(active.versionDir);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("managed cli retention refuses a linked managed root", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-linked-"));
  const outside = await mkdtemp(join(tmpdir(), "codestory-managed-retention-outside-"));
  try {
    const outsideData = join(outside, "data");
    const active = await writeManagedCliFixture(outsideData, "0.14.1");
    await symlink(
      join(outsideData, "codestory-cli"),
      join(dataDir, "codestory-cli"),
      process.platform === "win32" ? "junction" : "dir",
    );
    const probe = { status: 0, error: null, version: "0.14.1", stdout: "", stderr: "" };

    const report = launcherTest.managedCliRetentionReport(
      { source: "managed", version: "0.14.1", path: active.cliPath, warnings: [] },
      probe,
      { dataDir, probeVersion: () => probe },
    );

    assert.equal(report.warnings.some((warning) => warning.includes("managed_cli_root_not_direct")), true);
    assert.equal(report.removed.length, 0);
    await access(active.versionDir);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(outside, { recursive: true, force: true });
  }
});

test("mcp launcher starts projectless when host launches from plugin root", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-active-project-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const realRepoRoot = await realpath(repoRoot);

  try {
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "SessionStart",
        cwd: realRepoRoot,
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "const command = args[0];",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({",
        "  cwd: process.cwd(),",
        "  args,",
        "  launchCwd: process.env.CODESTORY_PLUGIN_LAUNCH_CWD || '',",
        "  runtimeCwd: process.env.CODESTORY_PLUGIN_RUNTIME_CWD || '',",
        "  multiProject: process.env.CODESTORY_PLUGIN_MULTI_PROJECT || ''",
        "}) + '\\n');",
        "if (command === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (command === 'serve') { fs.writeFileSync(process.env.TEST_OUT, 'serve-called'); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input: launcherHandoffInput(),
      encoding: "utf8",
      timeout: 15000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve-called");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.ok(serve, "expected serve call");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    assert.match(serve.cwd, /runtime-cwd/u);
    assert.equal(serve.multiProject, "1");
    assert.equal(serve.launchCwd, pluginRoot);
    assert.notEqual(serve.runtimeCwd, pluginRoot);
    assert.match(serve.runtimeCwd, /runtime-cwd/u);
    const runtimeState = JSON.parse(await readFile(join(dataDir, ".codestory-mcp-runtime.json"), "utf8"));
    assert.equal(runtimeState.launchCwd, pluginRoot);
    assert.equal(runtimeState.runtimeCwd, serve.runtimeCwd);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("multi-project stdio ignores mutable active-workspace state", async () => {
  const launcher = await readFile(join(pluginRoot, "scripts", "codestory-mcp.cjs"), "utf8");
  const transport = await readFile(join(repoRoot, "crates", "codestory-cli", "src", "stdio_transport.rs"), "utf8");

  assert.match(launcher, /function stdioRuntimeEnv\(resolved, runtimeCwd\)/u);
  assert.match(launcher, /CODESTORY_PLUGIN_MULTI_PROJECT: '1'/u);
  assert.doesNotMatch(launcher, /CODESTORY_PLUGIN_PROJECT_ROOT:/u);
  assert.match(launcher, /\['serve', '--stdio', '--multi-project', '--refresh', 'none'\]/u);
  assert.match(transport, /fn stdio_workspace_mismatch\(runtime: &RuntimeContext\)/u);
  assert.match(transport, /CODESTORY_PLUGIN_MULTI_PROJECT/u);
  assert.match(transport, /project_required: `project` must be the caller's absolute repository root/u);
});

test("mcp launcher fails open when delegated stdio runtime exits", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-delegated-stdio-exit-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-delegated-stdio-bin-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const realRepoRoot = await realpath(repoRoot);
  const cliPath = await writeNodeCli(
    binDir,
    [
      "const args = process.argv.slice(2);",
      "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
      "if (args[0] === 'serve') { process.exit(17); }",
      "process.exit(2);",
    ].join("\n"),
  );

  let child = null;
  try {
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "SessionStart",
        cwd: realRepoRoot,
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    child = spawn(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const responses = [];
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      const lines = stdout.split(/\r?\n/u);
      stdout = lines.pop() || "";
      for (const line of lines) {
        if (line) responses.push(JSON.parse(line));
      }
    });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    const responseFor = async (id) => {
      const deadline = Date.now() + 3000;
      while (Date.now() < deadline) {
        const response = responses.find((candidate) => candidate.id === id);
        if (response) return response;
        await delay(10);
      }
      assert.fail(`timed out waiting for ${id}: ${stderr}`);
    };
    const completed = once(child, "close");

    child.stdin.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: "initialize",
      method: "initialize",
      params: {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: "plugin-static", version: "1" },
      },
    })}\n`);
    assert.equal((await responseFor("initialize")).result.serverInfo.version, version);
    child.stdin.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: "delegate",
      method: "tools/list",
    })}\n`);
    assert.match(
      (await responseFor("delegate")).error.message,
      /stdio handoff exited before completing the request/u,
    );
    child.stdin.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: "status",
      method: "resources/read",
      params: { uri: statusUri },
    })}\n`);
    const status = JSON.parse((await responseFor("status")).result.contents[0].text);
    assert.equal(status.degraded_reason, "runtime_stdio_child_exit");
    assert.equal(status.project_root, realRepoRoot);
    assert.equal(status.project_root_source, "resource_uri");
    assert.equal(status.readiness[0].setup.probe_status, 17);
    assert.match(
      status.readiness[0].setup.probe_error,
      /codestory-cli serve --stdio exited with status 17/u,
    );
    assert.equal(status.managed_retrieval.automatic, true);
    child.stdin.end();
    assert.equal((await completed)[0], 0);
    child = null;
  } finally {
    await stopChildProcess(child);
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("mcp launcher does not route from another thread's global active project state", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-wrong-thread-active-project-"));
  const previousRepo = join(dataDir, "previous-repo");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    await mkdir(previousRepo);
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: previousRepo,
        codexThreadId: "previous-thread",
        updatedAt: new Date(Date.now() - 1000).toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd(), projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '' }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready' || args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS: "600000",
        CODEX_THREAD_ID: "current-thread",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    assert.equal(serve.projectRoot, "");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher ignores thread-scoped and global project state", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-thread-active-project-"));
  const currentRepo = join(dataDir, "current-repo");
  const previousRepo = join(dataDir, "previous-repo");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const currentThread = "current-thread";

  try {
    await mkdir(currentRepo);
    await mkdir(previousRepo);
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: previousRepo,
        codexThreadId: "previous-thread",
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      threadActiveStatePath(dataDir, currentThread),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: currentRepo,
        codexThreadId: currentThread,
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({",
        "  args,",
        "  cwd: process.cwd(),",
        "  projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '',",
        "  projectRootSource: process.env.CODESTORY_PLUGIN_PROJECT_ROOT_SOURCE || '',",
        "  activeStatePath: process.env.CODESTORY_PLUGIN_ACTIVE_STATE_PATH || ''",
        "}) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, 'serve-called'); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS: "600000",
        CODEX_THREAD_ID: currentThread,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input: launcherHandoffInput(),
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve-called");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.ok(serve, "expected serve call");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    assert.match(serve.cwd, /runtime-cwd/u);
    assert.equal(serve.projectRoot, "");
    assert.equal(serve.projectRootSource, "");
    assert.equal(serve.activeStatePath, "");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher ignores fresh global active project state when current thread is unavailable", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-missing-thread-active-project-"));
  const previousRepo = join(dataDir, "previous-repo");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    await mkdir(previousRepo);
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: previousRepo,
        codexThreadId: "previous-thread",
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd(), projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '' }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready' || args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS: "600000",
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.ok(serve, "expected serve call");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    assert.match(serve.cwd, /runtime-cwd/u);
    assert.equal(serve.projectRoot, "");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher ignores unscoped global active project state", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-threaded-global-active-project-"));
  const previousRepo = join(dataDir, "previous-repo");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    await mkdir(previousRepo);
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: previousRepo,
        updatedAt: new Date().toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd(), projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '' }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready' || args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS: "600000",
        CODEX_THREAD_ID: "current-thread",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    assert.deepEqual(
      calls.find((call) => call.args[0] === "serve").args,
      ["serve", "--stdio", "--multi-project", "--refresh", "none"],
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher uses fresh active project state from before launcher start", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-prelaunch-active-project-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({
        event: "UserPromptSubmit",
        cwd: await realpath(repoRoot),
        codexThreadId: "current-thread",
        updatedAt: new Date(Date.now() - 10000).toISOString(),
      }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd() }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready' || args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODESTORY_PLUGIN_ACTIVE_PROJECT_TTL_MS: "600000",
        CODEX_THREAD_ID: "current-thread",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher ignores stale active project state from plugin root", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-stale-active-project-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    await writeFile(
      join(dataDir, ".codestory-active"),
      JSON.stringify({ event: "SessionStart", cwd: await realpath(repoRoot), updatedAt: "2000-01-01T00:00:00.000Z" }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd() }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready' || args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    assert.deepEqual(
      calls.find((call) => call.args[0] === "serve").args,
      ["serve", "--stdio", "--multi-project", "--refresh", "none"],
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("projectless mcp hands off to stdio without active project state", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-live-active-project-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");
  const realRepoRoot = await realpath(repoRoot);
  const activePath = join(dataDir, ".codestory-active");
  let child;

  try {
    await writeFile(
      activePath,
      JSON.stringify({ event: "SessionStart", cwd: realRepoRoot, updatedAt: "2000-01-01T00:00:00.000Z" }),
      "utf8",
    );
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify({ args, cwd: process.cwd(), projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '', activeStatePath: process.env.CODESTORY_PLUGIN_ACTIVE_STATE_PATH || '' }) + '\\n');",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'ready') { fs.writeFileSync(process.env.TEST_OUT, args[0]); process.exit(0); }",
        "if (args[0] === 'serve') {",
        "  fs.writeFileSync(process.env.TEST_OUT, args[0]);",
        "  let buffer = '';",
        "  process.stdin.setEncoding('utf8');",
        "  process.stdin.on('end', () => process.exit(0));",
        "  process.stdin.on('data', (chunk) => {",
        "    buffer += chunk;",
        "    const lines = buffer.split(/\\r?\\n/u);",
        "    buffer = lines.pop() || '';",
        "    for (const line of lines) {",
        "      if (!line.trim()) continue;",
        "      const request = JSON.parse(line);",
      "      if (request.method === 'initialize') {",
      "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { serverInfo: { name: 'codestory' } } }) + '\\n');",
      "      } else if (request.method === 'tools/list') {",
      "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { tools: [{ name: 'ground' }] } }) + '\\n');",
      "      } else if (request.method === 'tools/call' && request.params && request.params.name === 'ground') {",
      "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { structuredContent: { state: 'ready' } } }) + '\\n');",
      "      } else if (request.method === 'resources/read') {",
      "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { contents: [{ uri: request.params.uri, mimeType: 'application/json', text: JSON.stringify({ project_root: process.env.CODESTORY_PLUGIN_PROJECT_ROOT, project_root_source: process.env.CODESTORY_PLUGIN_PROJECT_ROOT_SOURCE }) }] } }) + '\\n');",
      "      }",
        "    }",
        "  });",
        "  return;",
        "}",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    child = spawn(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    const waiters = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      for (;;) {
        const newline = stdout.indexOf("\n");
        if (newline < 0) break;
        const line = stdout.slice(0, newline).trim();
        stdout = stdout.slice(newline + 1);
        if (line && waiters.length > 0) {
          waiters.shift().resolve(JSON.parse(line));
        }
      }
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const nextResponse = () => Promise.race([
      new Promise((resolve, reject) => waiters.push({ resolve, reject })),
      new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for MCP response: ${stderr}`)), 5000)),
    ]);
    const sendRequest = async (request) => {
      const pending = nextResponse();
      child.stdin.write(`${JSON.stringify(request)}\n`);
      return pending;
    };
    const init = await sendRequest({
      jsonrpc: "2.0",
      id: "init",
      method: "initialize",
      params: { protocolVersion: "2024-11-05" },
    });
    assert.equal(init.result.serverInfo.name, "codestory");

    const grounded = await sendRequest({
      jsonrpc: "2.0",
      id: "ground",
      method: "tools/call",
      params: { name: "ground", arguments: { project: realRepoRoot } },
    });
    assert.equal(grounded.result.structuredContent.state, "ready");

    const tools = await sendRequest({ jsonrpc: "2.0", id: "tools", method: "tools/list" });
    assert.deepEqual(tools.result.tools.map((tool) => tool.name), ["ground"]);

    assert.equal(await readFile(marker, "utf8"), "serve");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.match(serve.cwd, /runtime-cwd/u);
    assert.equal(serve.projectRoot, "");
    assert.equal(serve.activeStatePath, "");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
  } finally {
    if (child && !child.killed) {
      child.stdin.end();
      await Promise.race([once(child, "exit"), new Promise((resolve) => setTimeout(resolve, 1000))]);
      if (!child.killed) child.kill();
    }
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher infers Codex managed data from installed cache without plugin-data env", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const codexHome = await mkdtemp(join(tmpdir(), "codestory-installed-cache-"));
  const codexRoot = join(codexHome, ".codex");
  const installRoot = join(codexRoot, "plugins", "cache", "TheGreenCedar", "codestory", version);
  const dataDir = join(codexRoot, "plugins", "data", "codestory-TheGreenCedar");
  const outFile = join(dataDir, "env.json");
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(cliDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  const pathDir = await mkdtemp(join(tmpdir(), "codestory-stale-path-"));
  const staleCli = join(pathDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  const launcher = join(installRoot, "scripts", "codestory-mcp.cjs");
  const privateReleaseBaseUrl = "https://private-packages.invalid";

  try {
    await mkdir(join(installRoot, "scripts"), { recursive: true });
    await mkdir(join(installRoot, "hooks"), { recursive: true });
    await mkdir(join(installRoot, ".codex-plugin"), { recursive: true });
    await mkdir(cliDir, { recursive: true });
    await writeFile(
      launcher,
      await readFile(join(pluginRoot, "scripts", "codestory-mcp.cjs"), "utf8"),
      "utf8",
    );
    await copyFile(
      join(pluginRoot, "scripts", "codestory-dev-cli-contract.cjs"),
      join(installRoot, "scripts", "codestory-dev-cli-contract.cjs"),
    );
    await copyFile(
      join(pluginRoot, "generated-mcp-catalog.json"),
      join(installRoot, "generated-mcp-catalog.json"),
    );
    await writeFile(
      join(installRoot, "hooks", "codestory-runtime.cjs"),
      await readFile(join(pluginRoot, "hooks", "codestory-runtime.cjs"), "utf8"),
      "utf8",
    );
    await writeFile(
      join(installRoot, ".codex-plugin", "plugin.json"),
      JSON.stringify({ version }),
      "utf8",
    );
    await writeFakeCli(cliPath);
    const sha256 = createHash("sha256")
      .update(await readFile(cliPath))
      .digest("hex");
    const manifest = explicitPackageManifest(
      version,
      process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
      sha256,
    );
    await writeFile(join(cliDir, "manifest.json"), JSON.stringify(manifest), "utf8");
    await writeFile(
      staleCli,
      process.platform === "win32"
        ? "@echo off\r\necho codestory-cli 0.0.1\r\n"
        : "#!/bin/sh\necho codestory-cli 0.0.1\n",
      "utf8",
    );
    await chmod(staleCli, 0o755);

    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: "",
        COPILOT_PLUGIN_DATA: "",
        TEST_OUT: outFile,
        TEST_CODESTORY_VERSION: version,
        CODESTORY_PLUGIN_RELEASE_BASE_URL: privateReleaseBaseUrl,
        PATH: pathDir,
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      cwd: repoRoot,
      input: launcherHandoffInput(),
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(await realpath(observed.path), await realpath(cliPath));
    assert.equal(await realpath(observed.pluginRoot), await realpath(installRoot));
    assert.equal(observed.pluginCacheVersion, version);
    assert.equal(observed.dirtyMarkerPath, undefined);
  } finally {
    await rm(codexHome, { recursive: true, force: true });
    await rm(pathDir, { recursive: true, force: true });
  }
});

test("mcp launcher blocks when managed runtime is unavailable", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-mcp-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = [
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "resources/read", params: { uri: statusUri } }),
    JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/list" }),
    JSON.stringify({ jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "ground", arguments: {} } }),
    JSON.stringify({ jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "status", arguments: { project: repoRoot } } }),
    JSON.stringify({ jsonrpc: "2.0", id: 6, method: "tools/call", params: { name: "ground", arguments: { project: "." } } }),
    JSON.stringify({ jsonrpc: "2.0", id: 7, method: "tools/call", params: { name: "ground", arguments: { project: join(dataDir, "missing") } } }),
  ].join("\n") + "\n";

  try {
    const realRepoRoot = await realpath(repoRoot);
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: "",
        COPILOT_PLUGIN_DATA: "",
        CODESTORY_PLUGIN_DATA: dataDir,
        CODESTORY_PLUGIN_DISABLE_PROVISION: "1",
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      cwd: repoRoot,
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.length, 7, result.stdout);
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.project_root, realRepoRoot);
    assert.equal(status.project_root_source, "resource_uri");
    assert.equal(status.degraded_reason, "managed_cli_unavailable");
    assert.equal(status.project_selection, undefined);
    assert.equal(status.plugin_runtime.plugin_version, version);
    assert.equal(status.plugin_runtime.plugin_root, pluginRoot);
    assert.equal(status.plugin_runtime.cli_source, "managed_unavailable");
    assert.equal(status.plugin_runtime.cli_path, null);
    assert.deepEqual(status.runtime, {
      source: "managed_unavailable",
      state: "unavailable",
      automatic: true,
    });
    assert.equal(status.readiness[0].status, "unavailable");
    assert.equal(status.readiness[0].reason, "managed_cli_unavailable");
    assert.equal(Object.hasOwn(status, "readiness_broker"), false);
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.equal(status.managed_retrieval.automatic, true);
    assert.deepEqual(status.recommended_next_calls, [
      { method: "resources/read", uri: statusUri },
    ]);
    const toolNames = responses[2].result.tools.map((tool) => tool.name);
    const catalogSource = await readFile(join(repoRoot, "crates", "codestory-cli", "src", "stdio_catalog.rs"), "utf8");
    const canonicalTools = catalogSource.slice(
      catalogSource.indexOf("static TOOLS: &[ToolSpec]"),
      catalogSource.indexOf("static RESOURCES: &[ResourceSpec]"),
    );
    const canonicalToolNames = [...canonicalTools.matchAll(/\bname:\s*"([^"]+)"/gu)]
      .map((match) => match[1]);
    assert.deepEqual([...toolNames].sort(), [...canonicalToolNames].sort());
    const coldGroundTool = responses[2].result.tools.find((tool) => tool.name === "ground");
    assert.equal(coldGroundTool.safety.effect, "managed_activation");
    assert.equal(coldGroundTool.safety.requiresConfirmation, false);
    assert.equal(coldGroundTool.safety.localOnly, false);
    assert.equal(coldGroundTool.safety.openWorld, true);
    assert.equal(coldGroundTool.annotations.readOnlyHint, false);
    assert.equal(coldGroundTool.annotations.openWorldHint, true);
    assert.equal(responses[3].result.isError, true);
    assert.equal(responses[3].result.structuredContent.code, "project_required");
    assert.equal(responses[3].result.structuredContent.tool, "ground");
    assert.equal(responses[3].result.structuredContent.retry_tool, undefined);
    assert.match(responses[3].result.structuredContent.message, /absolute repository root/u);
    assert.equal(responses[4].result.structuredContent.current_operation, null);
    assert.equal(responses[5].result.isError, true);
    assert.equal(responses[5].result.structuredContent.code, "project_required");
    assert.equal(responses[6].result.isError, true);
    assert.equal(responses[6].result.structuredContent.code, "project_unavailable");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher owns initialize before handing off to the native runtime", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-initialize-owner-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-initialize-owner-bin-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const marker = join(dataDir, "serve-called.txt");
  const cliPath = await writeNodeCli(
    binDir,
    [
      "const fs = require('node:fs');",
      "const args = process.argv.slice(2);",
      "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
      "if (args[0] === 'serve') { fs.writeFileSync(process.env.TEST_OUT, 'serve-called'); setInterval(() => {}, 1000); }",
      "else process.exit(2);",
    ].join("\n"),
  );
  const initialize = {
    jsonrpc: "2.0",
    id: "initialize",
    method: "initialize",
    params: {
      protocolVersion: "2025-03-26",
      capabilities: {},
      clientInfo: { name: "plugin-static", version: "1" },
    },
  };

  try {
    const result = spawnSync(process.execPath, [launcher], {
      cwd: dataDir,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_OUT: marker,
      },
      input: `${JSON.stringify(initialize)}\n`,
      encoding: "utf8",
      timeout: 2000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.error, undefined);
    const response = JSON.parse(result.stdout.trim());
    assert.equal(response.id, initialize.id);
    assert.equal(response.result.serverInfo.version, version);
    await assert.rejects(access(marker));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("mcp launcher starts the multi-project stdio runtime through its bridge", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-direct-stdio-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "fake-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "fake-codestory-cli.cmd" : "fake-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");

  try {
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const version = process.env.TEST_CODESTORY_VERSION;",
        "const logFile = process.env.TEST_LOG;",
        "const marker = process.env.TEST_OUT;",
        "const args = process.argv.slice(2);",
        "const command = args[0];",
        "fs.appendFileSync(logFile, JSON.stringify({ args }) + '\\n');",
        "if (command === '--version') { console.log('codestory-cli ' + version); process.exit(0); }",
        "if (command === 'serve') {",
        "  fs.writeFileSync(marker, 'serve-called');",
        "  let input = '';",
        "  process.stdin.setEncoding('utf8');",
        "  process.stdin.on('data', (chunk) => {",
        "    input += chunk;",
        "    const lines = input.split(/\\r?\\n/u);",
        "    input = lines.pop() || '';",
        "    for (const line of lines) {",
        "      if (!line) continue;",
        "      const request = JSON.parse(line);",
        "      if (request.method === 'initialize') {",
        "        console.log(JSON.stringify({",
        "          jsonrpc: '2.0',",
        "          id: request.id,",
        "          result: {",
        "            protocolVersion: request.params.protocolVersion,",
        "            capabilities: {},",
        "            serverInfo: { name: 'codestory', version },",
        "          },",
        "        }));",
        "      } else if (request.method === 'tools/list') {",
        "        console.log(JSON.stringify({",
        "          jsonrpc: '2.0',",
        "          id: request.id,",
        "          result: { tools: [{ name: 'native-runtime' }] },",
        "        }));",
        "      }",
        "    }",
        "  });",
        "  process.stdin.on('end', () => process.exit(0));",
        "  return;",
        "}",
        "process.exit(2);",
        "",
      ].join("\n"),
      "utf8",
    );
    if (process.platform === "win32") {
      await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${cliScript}" %*\r\n`, "utf8");
    } else {
      await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(cliScript)} "$@"\n`, "utf8");
      await chmod(cliPath, 0o755);
    }

    const result = spawnSync(process.execPath, [launcher], {
      cwd: dataDir,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      input: [
        {
          jsonrpc: "2.0",
          id: "initialize",
          method: "initialize",
          params: {
            protocolVersion: "2025-03-26",
            capabilities: {},
            clientInfo: { name: "plugin-static", version: "1" },
          },
        },
        { jsonrpc: "2.0", id: "native-tools", method: "tools/list" },
      ].map((request) => JSON.stringify(request)).join("\n") + "\n",
      encoding: "utf8",
      timeout: 15000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve-called");
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.filter((response) => response.id === "initialize").length, 1);
    assert.equal(
      responses.find((response) => response.id === "initialize")?.result.serverInfo.version,
      version,
    );
    assert.deepEqual(
      responses.find((response) => response.id === "native-tools")?.result.tools,
      [{ name: "native-runtime" }],
    );
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version", "serve"]);
    assert.ok(calls.some((call) => {
      return JSON.stringify(call.args) === JSON.stringify([
        "serve",
        "--stdio",
        "--multi-project",
        "--refresh",
        "none",
      ]);
    }));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when CODESTORY_CLI override cannot spawn", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-override-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const missingCli = join(dataDir, process.platform === "win32" ? "missing.exe" : "missing");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";

  try {
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: missingCli,
        PLUGIN_DATA: dataDir,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const response = JSON.parse(result.stdout.trim());
    const status = JSON.parse(response.result.contents[0].text);
    assert.equal(status.plugin_runtime.plugin_version, version);
    assert.equal(status.plugin_runtime.cli_source, "local_dev_override");
    assert.equal(status.readiness[0].reason, "local_dev_override_cli_unspawnable");
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.equal(status.managed_retrieval.automatic, true);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when managed cli probe fails", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-managed-"));
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(
    cliDir,
    process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
  );
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";
  let child;

  try {
    await mkdir(cliDir, { recursive: true });
    if (process.platform === "win32") {
      await writeFile(cliPath, "@echo off\r\nexit /b 7\r\n", "utf8");
    } else {
      await writeFile(cliPath, "#!/bin/sh\nexit 7\n", "utf8");
      await chmod(cliPath, 0o755);
    }
    const sha256 = createHash("sha256")
      .update(await readFile(cliPath))
      .digest("hex");
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify(explicitPackageManifest(
        version,
        process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
        sha256,
      )),
      "utf8",
    );

    child = spawn(process.execPath, [launcher], {
      env: {
        ...process.env,
        PLUGIN_DATA: dataDir,
        CODESTORY_PLUGIN_RELEASE_DIR: join(dataDir, "missing-release"),
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const completed = once(child, "close");
    let buffer = "";
    const responses = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      const lines = buffer.split(/\r?\n/u);
      buffer = lines.pop() || "";
      responses.push(...lines.filter(Boolean).map((line) => JSON.parse(line)));
    });
    child.stdin.write(input);
    const firstDeadline = Date.now() + 2000;
    while (Date.now() < firstDeadline && responses.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const firstReason = JSON.parse(responses[0].result.contents[0].text).degraded_reason;
    assert.equal([
      "managed_cli_provisioning",
      "managed_cli_provision_failed:managed_cli_asset_fetch_failed",
    ].includes(firstReason), true);
    if (firstReason === "managed_cli_provisioning") {
      await waitForPath(join(dataDir, ".codestory-mcp-runtime.json"));
      child.stdin.end(input.replace('"status"', '"terminal"'));
    } else {
      child.stdin.end();
    }
    const [exitCode] = await completed;
    assert.equal(exitCode, 0);
    const response = responses.find((entry) => entry.id === "terminal") || responses[0];
    const status = JSON.parse(response.result.contents[0].text);
    assert.equal(
      status.readiness[0].reason,
      "managed_cli_provision_failed:managed_cli_asset_fetch_failed",
    );
    assert.equal(
      status.plugin_runtime.plugin_version,
      version,
    );
  } finally {
    await stopChildProcess(child);
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher upgrades a verified prior managed cli to the checksummed release", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-provisioned-cli-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-release-"));
  const outFile = join(dataDir, "env.json");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const { archiveBase, archiveName } = releaseAssetForPlatform(version);
  const stageDir = join(releaseDir, archiveBase);
  const cliName = process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli";
  const cliPath = join(stageDir, cliName);
  const archivePath = join(releaseDir, archiveName);

  try {
    const priorVersion = "0.0.0";
    const priorRelease = releaseAssetForPlatform(priorVersion);
    const priorDir = join(dataDir, "codestory-cli", priorVersion);
    const priorCli = join(priorDir, "bin", cliName);
    await mkdir(dirname(priorCli), { recursive: true });
    if (process.platform === "win32") {
      await writeFile(priorCli, `@echo off\r\nif "%1"=="--version" (echo codestory-cli ${priorVersion}& exit /b 0)\r\nexit /b 90\r\n`, "utf8");
    } else {
      await writeFile(priorCli, `#!/bin/sh\nif [ "$1" = "--version" ]; then echo 'codestory-cli ${priorVersion}'; exit 0; fi\nexit 90\n`, "utf8");
      await chmod(priorCli, 0o755);
    }
    const priorSha256 = createHash("sha256").update(await readFile(priorCli)).digest("hex");
    await writeFile(join(priorDir, "manifest.json"), JSON.stringify({
      path: `bin/${cliName}`,
      sha256: priorSha256,
      version: priorVersion,
      build_source: "explicit_package",
      repo_ref: null,
      archive: priorRelease.archiveName,
      archive_url: `explicit-package:${"0".repeat(64)}`,
      archive_sha256: "0".repeat(64),
      target: priorRelease.archiveBase.slice(`codestory-cli-v${priorVersion}-`.length),
      provisioned_at: "1970-01-01T00:00:00.000Z",
      stdio_initialize_verified: true,
    }), "utf8");

    await mkdir(stageDir, { recursive: true });
    await writeFakeCli(cliPath);
    await writeArchiveFixture(archivePath, `${archiveBase}/${cliName}`, await readFile(cliPath));
    const archiveSha256 = createHash("sha256")
      .update(await readFile(archivePath))
      .digest("hex");
    await writeFile(
      join(releaseDir, "SHA256SUMS.txt"),
      `${archiveSha256}  ${archiveName}\n`,
      "utf8",
    );

    const launched = spawnLauncher(launcher, {
      CODESTORY_PLUGIN_RELEASE_DIR: releaseDir,
      PLUGIN_DATA: dataDir,
      TEST_OUT: outFile,
      TEST_CODESTORY_VERSION: version,
    });
    const result = await launched.completed;

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(observed.version, version);
    assert.equal(observed.repoRef, "");
    assert.equal(observed.buildSource, "explicit_package");
    assert.equal(observed.archiveSha256, archiveSha256);
    assert.notEqual(observed.path, priorCli);
    const retention = JSON.parse(observed.retention);
    assert.equal(retention.active_version, version);
    assert.equal(
      retention.retained.some((entry) => entry.version === priorVersion && entry.reason === "rollback"),
      true,
      JSON.stringify(retention),
    );
    assert.match(
      observed.path,
      new RegExp(String.raw`codestory-cli[\\/]+${version.replaceAll(".", String.raw`\.`)}[\\/]codestory-cli`, "u"),
    );
    assert.deepEqual(observed.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);

    const manifest = JSON.parse(
      await readFile(join(dataDir, "codestory-cli", version, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.version, version);
    assert.equal(manifest.repo_ref, null);
    assert.equal(manifest.build_source, "explicit_package");
    assert.equal(manifest.archive, archiveName);
    assert.equal(manifest.archive_url, `explicit-package:${archiveSha256}`);
    assert.equal(manifest.archive_sha256, archiveSha256);
    assert.equal(manifest.stdio_initialize_verified, true);
    assert.equal(typeof manifest.sha256, "string");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("mcp launcher serves diagnostics while managed provisioning runs, then hands off", { timeout: 30000 }, async () => {
  const { createServer } = await import("node:http");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-background-provision-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-background-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const outFile = join(dataDir, "runtime.json");
  let child;
  let server;
  let releaseAssets = () => {};
  try {
    const fixture = await writeReleaseFixture(releaseDir, version, writeLifecycleCli);
    const assets = new Map([
      ["/SHA256SUMS.txt", await readFile(fixture.sumsPath)],
      [`/${fixture.archiveName}`, await readFile(fixture.archivePath)],
    ]);
    const assetGate = new Promise((resolve) => { releaseAssets = resolve; });
    server = createServer(async (request, response) => {
      await assetGate;
      const body = assets.get(request.url);
      if (!body) return response.writeHead(404).end();
      response.writeHead(200).end(body);
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));

    child = spawn(process.execPath, [launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        CODESTORY_PLUGIN_RELEASE_BASE_URL: `http://127.0.0.1:${server.address().port}`,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_OUT: outFile,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const completed = once(child, "close");
    let buffered = "";
    const responses = [];
    const waiters = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffered += chunk;
      const lines = buffered.split(/\r?\n/u);
      buffered = lines.pop() || "";
      for (const line of lines.filter(Boolean)) {
        const response = JSON.parse(line);
        if (waiters.length) waiters.shift()(response); else responses.push(response);
      }
    });
    const nextResponse = () => responses.shift() || Promise.race([
      new Promise((resolve) => waiters.push(resolve)),
      new Promise((_, reject) => setTimeout(() => reject(new Error("timed out waiting for diagnostic MCP")), 2000)),
    ]);
    const request = async (message) => {
      child.stdin.write(`${JSON.stringify(message)}\n`);
      return nextResponse();
    };

    const initialized = await request({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { protocolVersion: "2024-11-05" },
    });
    assert.equal(initialized.result.serverInfo.name, "codestory");
    assert.equal(initialized.result.capabilities.tools.listChanged, true);
    assert.equal(initialized.result.capabilities.prompts.listChanged, true);
    const statusUri = launcherTest.projectBoundResourceUri("codestory://status", repoRoot);
    const statusResponse = await request({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: statusUri },
    });
    const status = JSON.parse(statusResponse.result.contents[0].text);
    assert.equal(status.project_root, repoRoot);
    assert.equal(status.project_root_source, "resource_uri");
    assert.equal(statusResponse.result.contents[0].uri, statusUri);
    assert.ok(
      status.recommended_next_calls.every((call) =>
        call.method !== "resources/read" || call.uri === statusUri),
    );
    assert.equal(status.degraded_reason, "managed_cli_provisioning");
    assert.equal(status.runtime.state, "preparing");
    const coldResources = await request({
      jsonrpc: "2.0",
      id: "cold-resources",
      method: "resources/list",
    });
    assert.deepEqual(
      coldResources.result.resources.map(({ uri }) => uri),
      ["codestory://agent-guide"],
    );
    const coldGuideResponse = await request({
      jsonrpc: "2.0",
      id: "cold-guide",
      method: "resources/read",
      params: { uri: "codestory://agent-guide" },
    });
    const coldGuide = JSON.parse(coldGuideResponse.result.contents[0].text);
    assert.equal(coldGuide.project, undefined);
    assert.equal(coldGuide.diagnostics_uri_template, "codestory://status{?project}");
    const coldTemplates = await request({
      jsonrpc: "2.0",
      id: "cold-templates",
      method: "resources/templates/list",
    });
    assert.deepEqual(
      coldTemplates.result.resourceTemplates.map(({ uriTemplate }) => uriTemplate),
      ["codestory://status{?project}"],
    );
    const coldPrompts = await request({
      jsonrpc: "2.0",
      id: "cold-prompts",
      method: "prompts/list",
    });
    assert.deepEqual(coldPrompts.result.prompts, []);
    const coldTools = await request({
      jsonrpc: "2.0",
      id: "cold-tools",
      method: "tools/list",
    });
    assert.equal(coldTools.result.tools.length, 20);
    assert.ok(coldTools.result.tools.some((tool) => tool.name === "ground"));
    const coldGround = await request({
      jsonrpc: "2.0",
      id: "cold-ground",
      method: "tools/call",
      params: { name: "ground", arguments: { project: repoRoot } },
    });
    assert.equal(coldGround.result.isError, true);
    assert.equal(coldGround.result.structuredContent.code, "codestory_preparing");
    assert.equal(coldGround.result.structuredContent.retry_tool, "ground");
    assert.equal(coldGround.result.structuredContent.project, repoRoot);
    assert.deepEqual(coldGround.result.structuredContent.operation, {
      operation_id: "managed-runtime-provisioning",
      state: "preparing",
      stage: "dense_preparation",
      attempt: 1,
      retry_after_ms: 1500,
      failure: null,
    });
    assert.doesNotMatch(coldGround.result.structuredContent.message, /status/u);

    releaseAssets();
    const runtimeMetadata = join(dataDir, ".codestory-mcp-runtime.json");
    await waitForPath(runtimeMetadata);
    const deadline = Date.now() + 10000;
    while (Date.now() < deadline) {
      const metadata = JSON.parse(await readFile(runtimeMetadata, "utf8"));
      if (metadata.source === "managed") break;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const publishedRuntime = JSON.parse(await readFile(runtimeMetadata, "utf8"));
    assert.equal(publishedRuntime.source, "managed", JSON.stringify(publishedRuntime));
    assert.equal(
      responses.some((response) => response.method === "notifications/tools/list_changed"),
      false,
    );
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
    const notificationDeadline = Date.now() + 2000;
    while (
      Date.now() < notificationDeadline &&
      !responses.some((response) => response.method === "notifications/tools/list_changed")
    ) await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(
      responses.some((response) => response.method === "notifications/tools/list_changed"),
      true,
    );
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/list" })}\n`);
    let handedOff = null;
    const handoffDeadline = Date.now() + 2000;
    while (Date.now() < handoffDeadline && !handedOff) {
      try {
        handedOff = JSON.parse(await readFile(outFile, "utf8"));
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
    assert.ok(handedOff);
    assert.equal(handedOff.initialized, true);
    assert.equal(handedOff.notified, true);
    assert.deepEqual(handedOff.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 4, method: "resources/list" })}\n`);
    const failureDeadline = Date.now() + 2000;
    while (Date.now() < failureDeadline && !responses.some((response) => response.id === 4)) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assert.equal(responses.filter((response) => response.id === 4).length, 1);
    assert.deepEqual(responses.find((response) => response.id === 4)?.result.resources, []);
    await new Promise((resolve) => setTimeout(resolve, 100));
    child.stdin.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: 5,
      method: "resources/read",
      params: { uri: statusUri },
    })}\n`);
    const recoveryDeadline = Date.now() + 2000;
    while (Date.now() < recoveryDeadline && !responses.some((response) => response.id === 5)) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const recoveredStatus = responses.find((response) => response.id === 5);
    assert.equal(
      JSON.parse(recoveredStatus.result.contents[0].text).degraded_reason,
      "runtime_stdio_child_exit",
    );
    child.stdin.end();
    assert.equal((await completed)[0], 0);
    child = null;
  } finally {
    releaseAssets();
    if (child) child.kill("SIGKILL");
    if (server) await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("managed publication waiter keeps diagnostic MCP responsive", { timeout: 15000 }, async () => {
  const { createServer } = await import("node:http");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-responsive-waiter-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-responsive-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  let publisher;
  let waiter;
  let server;
  let releaseAssets = () => {};
  try {
    const fixture = await writeReleaseFixture(releaseDir, version);
    const assets = new Map([
      ["/SHA256SUMS.txt", await readFile(fixture.sumsPath)],
      [`/${fixture.archiveName}`, await readFile(fixture.archivePath)],
    ]);
    const gate = new Promise((resolve) => { releaseAssets = resolve; });
    server = createServer(async (request, response) => {
      await gate;
      response.writeHead(200).end(assets.get(request.url));
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const env = {
      ...process.env,
      CODESTORY_CLI: "",
      CODESTORY_PLUGIN_RELEASE_BASE_URL: `http://127.0.0.1:${server.address().port}`,
      PLUGIN_DATA: dataDir,
      TEST_CODESTORY_VERSION: version,
    };
    publisher = spawn(process.execPath, [launcher], { env, stdio: ["pipe", "pipe", "pipe"] });
    publisher.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } })}\n`);
    await waitForPath(join(dataDir, "codestory-cli", ".retention-lock", "owner.json"));

    waiter = spawn(process.execPath, [launcher], { env, stdio: ["pipe", "pipe", "pipe"] });
    let output = "";
    waiter.stdout.setEncoding("utf8");
    waiter.stdout.on("data", (chunk) => { output += chunk; });
    waiter.stdin.write([
      JSON.stringify({ jsonrpc: "2.0", id: 2, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
      JSON.stringify({ jsonrpc: "2.0", id: 3, method: "resources/read", params: { uri: statusUri } }),
      "",
    ].join("\n"));
    const deadline = Date.now() + 2000;
    while (Date.now() < deadline && !output.split(/\r?\n/u).some((line) => {
      if (!line) return false;
      return JSON.parse(line).id === 3;
    })) await new Promise((resolve) => setTimeout(resolve, 10));
    const statusResponse = output.split(/\r?\n/u).filter(Boolean)
      .map((line) => JSON.parse(line)).find((response) => response.id === 3);
    assert.ok(statusResponse, output);
    assert.equal(JSON.parse(statusResponse.result.contents[0].text).degraded_reason, "managed_cli_provisioning");
  } finally {
    releaseAssets();
    for (const child of [publisher, waiter]) {
      if (child && child.exitCode === null) child.kill("SIGKILL");
    }
    if (server) await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("diagnostic handoff recovers a child spawn error", { timeout: 5000 }, async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    "const {EventEmitter}=require('node:events');",
    "const {PassThrough}=require('node:stream');",
    "let failed=false;",
    "const status=()=>({plugin_runtime:{plugin_version:'test'},degraded_reason:failed?'managed_cli_handoff_unspawnable':'managed_cli_provisioning',recommended_next_calls:[]});",
    "run(status,{shouldHandoff:()=>!failed,startRuntime:()=>{const child=new EventEmitter();child.stdin=new PassThrough();child.stdout=new PassThrough();child.stderr=new PassThrough();process.nextTick(()=>child.emit('error',new Error('synthetic spawn error')));return child},onRuntimeFailure:()=>{failed=true}});",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  const completed = once(child, "close");
  let output = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stdin.write([
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
    JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }),
    "",
  ].join("\n"));
  const errorDeadline = Date.now() + 2000;
  while (Date.now() < errorDeadline && !output.split(/\r?\n/u).filter(Boolean)
    .map((line) => JSON.parse(line)).some((response) => response.id === 2)) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  child.stdin.end(`${JSON.stringify({
    jsonrpc: "2.0",
    id: 3,
    method: "resources/read",
    params: { uri: statusUri },
  })}\n`);
  assert.equal((await completed)[0], 0);
  const responses = output.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
  assert.equal(responses.find((response) => response.id === 2)?.error.code, -32000);
  const status = JSON.parse(responses.find((response) => response.id === 3).result.contents[0].text);
  assert.equal(status.degraded_reason, "managed_cli_handoff_unspawnable");
});

test("fail-open status tool preserves primary runtime failures and no-project precedence", { timeout: 5000 }, async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const failures = [
    ["managed_cli_asset_fetch_failed", "asset archive checksum failed", 1],
    ["managed_cli_probe_failed", "version probe exited with status 2", 2],
    ["managed_cli_handoff_unspawnable", "spawn EACCES", null],
    ["runtime_stdio_child_exit", "codestory-cli serve --stdio exited with status 17", 17],
  ];
  const statuses = failures.map(([reason, failure, status]) => ({
    plugin_runtime: { plugin_version: "test" },
    managed_retrieval: { state: "unavailable" },
    degraded_reason: reason,
    readiness: [{
      reason,
      summary: `runtime unavailable: ${reason}`,
      setup: { probe_error: failure, probe_status: status },
    }],
    recommended_next_calls: [],
  }));
  statuses.push(statuses[0], statuses[0]);
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    `const statuses=${JSON.stringify(statuses)};`,
    "run(()=>statuses.shift()||statuses.at(-1));",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  const completed = once(child, "close");
  let output = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stdin.end([
    ...failures.map((_, index) => JSON.stringify({
      jsonrpc: "2.0",
      id: index + 1,
      method: "tools/call",
      params: { name: "status", arguments: { project: repoRoot } },
    })),
    JSON.stringify({
      jsonrpc: "2.0",
      id: failures.length + 1,
      method: "tools/call",
      params: { name: "status", arguments: {} },
    }),
    JSON.stringify({
      jsonrpc: "2.0",
      id: failures.length + 2,
      method: "tools/call",
      params: {
        name: "status",
        arguments: { project: join(repoRoot, "missing-project-for-fail-open-proof") },
      },
    }),
    "",
  ].join("\n"));
  assert.equal((await completed)[0], 0);
  const responses = output.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
  failures.forEach(([reason, failure], index) => {
    const structured = responses.find((response) => response.id === index + 1)?.result.structuredContent;
    assert.equal(structured.degraded_reason, reason);
    assert.equal(structured.failure, failure);
    assert.equal(structured.current_operation, null);
  });
  const noProject = responses.find((response) => response.id === failures.length + 1)?.result.structuredContent;
  assert.equal(noProject.code, "project_required");
  assert.equal(noProject.state, "no_project");
  assert.equal(noProject.degraded_reason, undefined);
  assert.equal(noProject.diagnostics_uri, undefined);
  const unavailableProject = responses.find(
    (response) => response.id === failures.length + 2,
  )?.result.structuredContent;
  assert.equal(unavailableProject.code, "project_unavailable");
  assert.equal(unavailableProject.state, "unavailable");
  assert.equal(unavailableProject.diagnostics_uri, undefined);
});

test("managed cli publication is single-flight and atomically visible across two processes", { timeout: 30000 }, async () => {
  const { createServer } = await import("node:http");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-publication-contention-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-publication-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const outA = join(dataDir, "a.json");
  const outB = join(dataDir, "b.json");
  const probeLog = join(dataDir, "probes.log");
  let server;
  try {
    const fixture = await writeReleaseFixture(releaseDir, version);
    const assets = new Map([
      ["/SHA256SUMS.txt", await readFile(fixture.sumsPath)],
      [`/${fixture.archiveName}`, await readFile(fixture.archivePath)],
    ]);
    const requests = [];
    server = createServer((request, response) => {
      requests.push(request.url);
      const body = assets.get(request.url);
      if (!body) {
        response.writeHead(404).end();
        return;
      }
      setTimeout(() => response.writeHead(200).end(body), requests.length === 1 ? 250 : 0);
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const baseUrl = `http://publication-secret@127.0.0.1:${server.address().port}`;
    const common = {
      CODESTORY_PLUGIN_RELEASE_BASE_URL: baseUrl,
      PLUGIN_DATA: dataDir,
      TEST_CODESTORY_VERSION: version,
      CODESTORY_TEST_PROBE_LOG: probeLog,
    };
    const first = spawnLauncher(launcher, { ...common, TEST_OUT: outA });
    await new Promise((resolve) => setTimeout(resolve, 25));
    const second = spawnLauncher(launcher, { ...common, TEST_OUT: outB });
    const versionDir = join(dataDir, "codestory-cli", version);
    let finished = false;
    const visibility = (async () => {
      while (!finished) {
        try {
          await access(versionDir);
        } catch (error) {
          if (error.code === "ENOENT") {
            await new Promise((resolve) => setTimeout(resolve, 5));
            continue;
          }
          throw error;
        }
        const manifest = JSON.parse(await readFile(join(versionDir, "manifest.json"), "utf8"));
        const executable = join(versionDir, ...manifest.path.split("/"));
        const actual = createHash("sha256").update(await readFile(executable)).digest("hex");
        assert.equal(actual, manifest.sha256);
        await new Promise((resolve) => setTimeout(resolve, 5));
      }
    })();
    const results = await Promise.all([first.completed, second.completed]);
    finished = true;
    await visibility;
    for (const result of results) assert.equal(result.status, 0, result.stderr);
    for (const file of [outA, outB]) {
      await access(file).catch(() => assert.fail(JSON.stringify(results)));
    }
    const observed = await Promise.all([outA, outB].map(async (file) => JSON.parse(await readFile(file, "utf8"))));
    assert.equal(observed[0].path, observed[1].path);
    assert.equal(observed[0].sha256, observed[1].sha256);
    const publishedManifest = JSON.parse(await readFile(join(versionDir, "manifest.json"), "utf8"));
    assert.doesNotMatch(publishedManifest.archive_url, /publication-secret/u);
    assert.equal(requests.filter((url) => url === "/SHA256SUMS.txt").length, 1, JSON.stringify(requests));
    assert.equal(requests.filter((url) => url === `/${fixture.archiveName}`).length, 1, JSON.stringify(requests));
    assert.equal((await readFile(probeLog, "utf8")).trim().split(/\r?\n/u).filter(Boolean).length, 1);
    assert.equal(observed.some((entry) => entry.warnings.includes("managed_cli_publication:publisher")), true);
    assert.equal(observed.some((entry) => entry.warnings.includes("managed_cli_publication:waiter")), true);
  } finally {
    if (server) await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("managed cli publication reclaims crashes after lock and before publication", { timeout: 45000 }, async () => {
  const { createServer } = await import("node:http");
  const version = await readPluginVersion();
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-crash-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  let server;
  try {
    const fixture = await writeReleaseFixture(releaseDir, version);
    const assets = new Map([
      ["/SHA256SUMS.txt", await readFile(fixture.sumsPath)],
      [`/${fixture.archiveName}`, await readFile(fixture.archivePath)],
    ]);
    let holdResponses = true;
    server = createServer((request, response) => {
      const body = assets.get(request.url);
      if (!body) return response.writeHead(404).end();
      const send = () => response.writeHead(200).end(body);
      if (holdResponses) setTimeout(send, 5000);
      else send();
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const baseUrl = `http://127.0.0.1:${server.address().port}`;

    for (const crashPoint of ["after-lock", "before-publication"]) {
      const dataDir = await mkdtemp(join(tmpdir(), `codestory-${crashPoint}-`));
      try {
        const failedOut = join(dataDir, "failed.json");
        const recoveredOut = join(dataDir, "recovered.json");
        holdResponses = crashPoint === "after-lock";
        const crashed = spawnLauncher(launcher, {
          CODESTORY_PLUGIN_RELEASE_BASE_URL: baseUrl,
          PLUGIN_DATA: dataDir,
          TEST_CODESTORY_VERSION: version,
          TEST_OUT: failedOut,
          CODESTORY_TEST_PROBE_DELAY_MS: crashPoint === "before-publication" ? "5000" : "0",
        });
        if (crashPoint === "after-lock") {
          await waitForPath(join(dataDir, "codestory-cli", ".retention-lock", "owner.json"));
        } else {
          const root = join(dataDir, "codestory-cli");
          const deadline = Date.now() + 15000;
          while (Date.now() < deadline) {
            const children = await readdir(root).catch(() => []);
            if (children.some((name) => name.startsWith(`.provisioning-${version}-`))) break;
            await new Promise((resolve) => setTimeout(resolve, 10));
          }
          assert.equal((await readdir(root)).some((name) => name.startsWith(`.provisioning-${version}-`)), true);
        }
        crashed.child.kill("SIGKILL");
        await crashed.completed;
        holdResponses = false;
        const recovered = spawnLauncher(launcher, {
          CODESTORY_PLUGIN_RELEASE_BASE_URL: baseUrl,
          PLUGIN_DATA: dataDir,
          TEST_CODESTORY_VERSION: version,
          TEST_OUT: recoveredOut,
          CODESTORY_TEST_PROBE_DELAY_MS: "0",
        });
        const result = await recovered.completed;
        assert.equal(result.status, 0, result.stderr);
        await access(recoveredOut).catch(() => assert.fail(JSON.stringify(result)));
        const observed = JSON.parse(await readFile(recoveredOut, "utf8"));
        assert.match(observed.warnings, /managed_cli_publication:reclaimed_lock/u);
        const root = join(dataDir, "codestory-cli");
        await access(join(root, version, "manifest.json"));
        assert.equal(
          (await readdir(root)).some((name) => name.startsWith(".retention-lock.owner-")),
          false,
        );
      } finally {
        await rm(dataDir, { recursive: true, force: true });
      }
    }
  } finally {
    if (server) await new Promise((resolve) => server.close(resolve));
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("managed cli quarantines corrupt installs, retains two, and fails closed on a locked directory", { timeout: 30000 }, async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-corrupt-install-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-corrupt-release-"));
  const root = join(dataDir, "codestory-cli");
  const versionDir = join(root, version);
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousTestVersion = process.env.TEST_CODESTORY_VERSION;
  try {
    await writeReleaseFixture(releaseDir, version);
    process.env.CODESTORY_PLUGIN_RELEASE_DIR = releaseDir;
    process.env.TEST_CODESTORY_VERSION = version;
    await launcherTest.provisionManagedCli(dataDir, version, []);
    const corruptions = [
      async () => writeFile(join(versionDir, "manifest.json"), "{", "utf8"),
      async () => {
        const manifest = JSON.parse(await readFile(join(versionDir, "manifest.json"), "utf8"));
        manifest.version = "0.0.0";
        await writeFile(join(versionDir, "manifest.json"), JSON.stringify(manifest), "utf8");
      },
      async () => {
        const manifest = JSON.parse(await readFile(join(versionDir, "manifest.json"), "utf8"));
        manifest.sha256 = "f".repeat(64);
        await writeFile(join(versionDir, "manifest.json"), JSON.stringify(manifest), "utf8");
      },
      async () => {
        const manifestPath = join(versionDir, "manifest.json");
        const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
        const executable = join(versionDir, ...manifest.path.split("/"));
        if (process.platform === "win32") {
          await writeFile(executable, "@echo off\r\necho codestory-cli 0.0.0\r\n", "utf8");
        } else {
          await writeFile(executable, "#!/bin/sh\necho codestory-cli 0.0.0\n", "utf8");
          await chmod(executable, 0o755);
        }
        manifest.sha256 = createHash("sha256").update(await readFile(executable)).digest("hex");
        await writeFile(manifestPath, JSON.stringify(manifest), "utf8");
      },
    ];
    for (const corrupt of corruptions) {
      await corrupt();
      const warnings = [];
      const resolved = await launcherTest.provisionManagedCli(dataDir, version, warnings);
      assert.ok(resolved.path);
      assert.equal(warnings.some((warning) => warning.startsWith("managed_cli_publication:quarantine:")), true);
      assert.equal(warnings.some((warning) => warning.startsWith("managed_cli_publication:reprovision:")), true);
    }
    const quarantines = (await readdir(root)).filter((name) => name.startsWith(`.quarantine-${version}-`));
    assert.equal(quarantines.length, 2, JSON.stringify(quarantines));

    const lockedDir = join(root, "locked");
    await mkdir(lockedDir);
    assert.throws(
      () => launcherTest.quarantineManagedCliVersion(root, lockedDir, version, "locked", {
        renameSync() {
          const error = new Error("locked");
          error.code = "EPERM";
          throw error;
        },
      }),
      /managed_cli_quarantine_failed:EPERM/u,
    );
    await access(lockedDir);
  } finally {
    if (previousReleaseDir === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    else process.env.CODESTORY_PLUGIN_RELEASE_DIR = previousReleaseDir;
    if (previousTestVersion === undefined) delete process.env.TEST_CODESTORY_VERSION;
    else process.env.TEST_CODESTORY_VERSION = previousTestVersion;
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("managed cli resolution fails closed on a running Windows executable", { timeout: 15000 }, async (t) => {
  if (process.platform !== "win32") {
    t.skip("Windows executable locking semantics");
    return;
  }
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-locked-windows-cli-"));
  const versionDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(versionDir, "bin", "codestory-cli.exe");
  const readyPath = join(dataDir, "ready");
  let locked;
  try {
    await mkdir(dirname(cliPath), { recursive: true });
    await copyFile(process.execPath, cliPath);
    const sha256 = createHash("sha256").update(await readFile(cliPath)).digest("hex");
    await writeFile(
      join(versionDir, "manifest.json"),
      JSON.stringify(managedReleaseManifest(version, "bin/codestory-cli.exe", sha256)),
      "utf8",
    );
    locked = spawn(cliPath, ["-e", `require('fs').writeFileSync(${JSON.stringify(readyPath)}, 'ready');Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,60000)`], {
      cwd: dirname(cliPath),
      stdio: "ignore",
      windowsHide: true,
    });
    await waitForPath(readyPath);
    await assert.rejects(
      rm(cliPath),
      (error) => ["EACCES", "EBUSY", "EPERM"].includes(error.code),
    );

    const warnings = [];
    const resolved = await launcherTest.resolveManagedCli(dataDir, version, warnings);
    assert.equal(resolved, null);
    assert.equal(
      warnings.some((warning) => warning.startsWith("managed_cli_publication:terminal_failure:managed_cli_quarantine_failed")),
      true,
      JSON.stringify(warnings),
    );
    await access(cliPath);
  } finally {
    if (locked) {
      locked.kill("SIGKILL");
      await once(locked, "close");
    }
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("startup hook records active project without runtime bootstrap", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-minimal-"));
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");

  try {
    const result = spawnSync(process.execPath, [hookPath], {
      env: {
        ...process.env,
        CODESTORY_CLI: join(dataDir, "missing-codestory-cli"),
        CODEX_THREAD_ID: "hook-thread-id",
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      },
      input: JSON.stringify({
        hook_event_name: "SessionStart",
        source: "startup",
        cwd: repoRoot,
      }),
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const output = JSON.parse(result.stdout);
    const context = output.hookSpecificOutput.additionalContext;
    assert.equal(output.systemMessage, "CODESTORY:BACKGROUND");
    assert.match(context, /CODESTORY GROUNDING AVAILABLE/u);
    assert.match(context, /Call status only for diagnostics/u);
    assert.match(context, /retry that same tool/u);
    assert.match(context, /tool_search/u);
    assert.doesNotMatch(context, /HOOK MCP BRIDGE/u);
    assert.doesNotMatch(context, /managed_bootstrap/u);
    assert.doesNotMatch(context, /mcp_resources_exposed/u);

    const state = JSON.parse(await readFile(join(dataDir, ".codestory-active"), "utf8"));
    const threadState = JSON.parse(await readFile(threadActiveStatePath(dataDir, "hook-thread-id"), "utf8"));
    assert.equal(state.cwd, repoRoot);
    assert.equal(state.codexThreadId, "hook-thread-id");
    assert.equal(state.hook.bridge_removed, true);
    assert.equal(threadState.cwd, repoRoot);
    assert.equal(threadState.codexThreadId, "hook-thread-id");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader retries a transient failure", async () => {
  const { EventEmitter } = await import("node:events");
  const { PassThrough } = await import("node:stream");
  const launcher = require(join(pluginRoot, "scripts", "codestory-mcp.cjs"));
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-retry-"));
  const destination = join(dataDir, "SHA256SUMS.txt");
  let calls = 0;

  const fakeGet = (_url, onResponse) => {
    calls += 1;
    const request = new EventEmitter();
    request.setTimeout = () => request;
    request.destroy = (error) => {
      process.nextTick(() => request.emit("error", error));
      return request;
    };
    process.nextTick(() => {
      if (calls === 1) {
        request.emit("error", new Error("synthetic network reset"));
        return;
      }
      const response = new PassThrough();
      response.statusCode = 200;
      response.headers = {};
      onResponse(response);
      response.end("checksum fixture\n");
    });
    return request;
  };

  try {
    await launcher._test.downloadFile("https://example.invalid/SHA256SUMS.txt", destination, {
      attempts: 2,
      get: fakeGet,
      retryDelayMs: () => 1,
      timeoutMs: 100,
    });

    assert.equal(calls, 2);
    assert.equal(await readFile(destination, "utf8"), "checksum fixture\n");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader enforces a total body deadline", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-deadline-"));
  const destination = join(dataDir, "slow.bin");
  const server = createServer((_request, response) => {
    response.writeHead(200);
    const interval = setInterval(() => response.write("x"), 10);
    response.on("close", () => clearInterval(interval));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const started = Date.now();
    await assert.rejects(
      launcherTest.downloadFile(`http://127.0.0.1:${server.address().port}/slow`, destination, {
        attempts: 1,
        timeoutMs: 60,
      }),
      /timed out.*total/u,
    );
    assert.ok(Date.now() - started < 1000);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader bounds announced and streamed bytes without partial files", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-bounds-"));
  const fakeGet = (headers, body) => (_url, onResponse) => {
    const request = new EventEmitter();
    request.destroy = () => request;
    process.nextTick(() => {
      const response = new PassThrough();
      response.statusCode = 200;
      response.headers = headers;
      onResponse(response);
      response.end(body);
    });
    return request;
  };
  try {
    for (const [name, headers] of [
      ["announced.bin", { "content-length": "5" }],
      ["streamed.bin", {}],
    ]) {
      const destination = join(dataDir, name);
      await assert.rejects(
        launcherTest.downloadFile("https://example.invalid/bounded", destination, {
          attempts: 1,
          get: fakeGet(headers, "12345"),
          maxBytes: 4,
          timeoutMs: 100,
        }),
        /download_size_limit_exceeded/u,
      );
      assert.equal(fs.existsSync(destination), false);
    }
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher keeps managed provision failures primary", async () => {
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-provision-fail-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-empty-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: statusUri },
  }) + "\n";
  let child;

  try {
    child = spawn(process.execPath, [launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        CODESTORY_PLUGIN_RELEASE_DIR: releaseDir,
        PLUGIN_DATA: dataDir,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const completed = once(child, "close");
    let buffer = "";
    const responses = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      buffer += chunk;
      const lines = buffer.split(/\r?\n/u);
      buffer = lines.pop() || "";
      responses.push(...lines.filter(Boolean).map((line) => JSON.parse(line)));
    });
    child.stdin.write(input);
    const firstDeadline = Date.now() + 2000;
    while (Date.now() < firstDeadline && responses.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    const first = JSON.parse(responses[0].result.contents[0].text);
    if (first.degraded_reason === "managed_cli_provisioning") {
      await waitForPath(join(dataDir, ".codestory-mcp-runtime.json"));
      child.stdin.end(input.replace('"id":1', '"id":2'));
    } else {
      child.stdin.end();
    }
    assert.equal((await completed)[0], 0);
    const response = responses.find((entry) => entry.id === 2) || responses[0];
    const status = JSON.parse(response.result.contents[0].text);
    assert.equal(status.degraded_reason, "managed_cli_provision_failed:managed_cli_asset_fetch_failed");
    assert.doesNotMatch(JSON.stringify(status), new RegExp(releaseDir.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&"), "u"));
    assert.equal(
      status.plugin_runtime.warnings.includes("managed_cli_publication:terminal_failure:managed_cli_asset_fetch_failed"),
      true,
    );
    assert.equal(status.plugin_runtime.cli_source, "managed_unavailable");
    assert.equal(
      status.plugin_runtime.warnings.includes("managed_cli_unavailable"),
      true,
    );
  } finally {
    await stopChildProcess(child);
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("session-start hooks are thin and host manifests point at them", async () => {
  const hookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "claude-codex-hooks.json"), "utf8"),
  );
  const copilotHookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "copilot-hooks.json"), "utf8"),
  );
  const hostManifest = join(pluginRoot, ".claude-plugin", "plugin.json");
  const hookCommands = Object.values(hookConfig.hooks)
    .flat()
    .flatMap((entry) => entry.hooks);
  const hookScript = /hooks[\\/]([\w.-]+\.(?:js|mjs|cjs|ps1|sh))/u;

  assert.equal(copilotHookConfig.hooks.sessionStart.length, 1);
  assert.equal(Object.hasOwn(hookConfig.hooks, "UserPromptSubmit"), false);

  for (const hook of hookCommands) {
    assert.match(hook.command, /codestory-activate\.cjs/u);
    assert.match(hook.commandWindows, /codestory-activate\.cjs/u);
    assert.equal(
      Object.hasOwn(hook, "args"),
      false,
      "shell-guarded hooks should not rely on args-only launch",
    );
    const match = `${hook.command}\n${hook.commandWindows}`.match(hookScript);
    assert.ok(match, `cannot find hook script in command: ${hook.command}`);
    await access(join(pluginRoot, "hooks", match[1]));
  }

  const manifest = JSON.parse(await readFile(hostManifest, "utf8"));
  assert.equal(manifest.hooks, "./hooks/claude-codex-hooks.json");
});

test("hook records Codex thread id in active project state", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-thread-state-"));
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");

  try {
    const result = spawnSync(process.execPath, [hookPath], {
      env: {
        ...process.env,
        CODESTORY_HOOK_DISABLE_RUNTIME: "1",
        CODEX_THREAD_ID: "hook-thread-id",
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      },
      input: JSON.stringify({
        hook_event_name: "SessionStart",
        source: "startup",
        cwd: repoRoot,
      }),
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const state = JSON.parse(await readFile(join(dataDir, ".codestory-active"), "utf8"));
    const threadState = JSON.parse(await readFile(threadActiveStatePath(dataDir, "hook-thread-id"), "utf8"));
    assert.equal(state.cwd, repoRoot);
    assert.equal(state.codexThreadId, "hook-thread-id");
    assert.equal(threadState.cwd, repoRoot);
    assert.equal(threadState.codexThreadId, "hook-thread-id");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook manifest timeouts stay bounded for lightweight activation", async () => {
  const hookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "claude-codex-hooks.json"), "utf8"),
  );
  const copilotHookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "copilot-hooks.json"), "utf8"),
  );
  const claudeTimeouts = Object.values(hookConfig.hooks)
    .flat()
    .flatMap((entry) => entry.hooks)
    .map((hook) => hook.timeout);
  const copilotTimeouts = copilotHookConfig.hooks.sessionStart.map((hook) => hook.timeoutSec);

  for (const timeoutSec of [...claudeTimeouts, ...copilotTimeouts]) {
    assert.equal(typeof timeoutSec, "number");
    assert.ok(timeoutSec >= 5, `hook timeout ${timeoutSec}s is too short for node startup`);
    assert.ok(timeoutSec <= 300, `hook timeout ${timeoutSec}s must stay bounded`);
  }
});

async function writeNodeCli(binDir, source) {
  const scriptPath = join(binDir, "fake-codestory-cli.cjs");
  const cliPath = join(
    binDir,
    process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
  );
  await writeFile(scriptPath, source, "utf8");
  if (process.platform === "win32") {
    await writeFile(cliPath, `@echo off\r\n"${process.execPath}" "${scriptPath}" %*\r\n`, "utf8");
    return cliPath;
  }
  await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} ${JSON.stringify(scriptPath)} "$@"\n`, "utf8");
  await chmod(cliPath, 0o755);
  return cliPath;
}

function runHookProcess(script, input, env) {
  const result = spawnSync(process.execPath, [script], {
    env: {
      ...process.env,
      COPILOT_PLUGIN_DATA: "",
      ...env,
    },
    input: JSON.stringify(input),
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

function runCodexHook(input, env) {
  return runHookProcess(join(pluginRoot, "hooks", "codestory-activate.cjs"), input, env);
}

test("session hooks inject one bounded contract and prompt hooks stay silent", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-reinject-"));
  const longCwd = `C:\\${"very-long-directory\\".repeat(200)}repo`;
  try {
    for (const source of ["compact", "resume", "compact"]) {
      const output = runCodexHook({
        hook_event_name: "SessionStart",
        source,
        cwd: longCwd,
      }, { PLUGIN_DATA: dataDir, PATH: "" });
      const context = output.hookSpecificOutput.additionalContext;
      assert.ok(context.length <= 900, `hook output was ${context.length} characters`);
      assert.match(context, /target repository root/u);
      assert.match(context, /starting hint/u);
      assert.match(context, /search only for that tool by name/u);
      assert.doesNotMatch(context, /status first|poll status/u);
      assert.doesNotMatch(context, /truncated/u);
      assert.equal(context.endsWith("directly."), true);
    }
    const promptOutput = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RuntimeContext defined?",
      cwd: longCwd,
    }, { PLUGIN_DATA: dataDir, PATH: "" });
    assert.equal(Object.hasOwn(promptOutput, "hookSpecificOutput"), false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook heartbeat stays quiet and does not bridge hidden MCP", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-heartbeat-hidden-mcp-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-heartbeat-bin-"));
  const marker = join(dataDir, "cli-called.txt");

  try {
    const cliPath = await writeNodeCli(binDir, "require(\"fs\").writeFileSync(process.env.TEST_MARKER, process.argv.slice(2).join(\" \"));");
    const output = runCodexHook({
      hook_event_name: "GoalLoopHeartbeat",
      cwd: repoRoot,
    }, {
      CODESTORY_CLI: cliPath,
      PLUGIN_DATA: dataDir,
      TEST_MARKER: marker,
      PATH: "",
    });

    assert.equal(Object.hasOwn(output, "hookSpecificOutput"), false);
    await assert.rejects(readFile(marker, "utf8"), /ENOENT/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook script executes under Codex home module scope", async () => {
  const { cp } = await import("node:fs/promises");
  const { spawnSync } = await import("node:child_process");
  const codexHome = await mkdtemp(join(tmpdir(), "codestory-codex-home-"));
  const installRoot = join(
    codexHome,
    "plugins",
    "cache",
    "TheGreenCedar",
    "codestory",
    "0.0.0",
  );

  try {
    await writeFile(join(codexHome, "package.json"), '{"type":"module"}\n', "utf8");
    await cp(join(pluginRoot, "hooks"), join(installRoot, "hooks"), {
      recursive: true,
    });
    await cp(
      join(pluginRoot, "skills"),
      join(installRoot, "skills"),
      { recursive: true },
    );

    const result = spawnSync(
      process.execPath,
      [join(installRoot, "hooks", "codestory-activate.cjs")],
      {
        env: {
          ...process.env,
          CODESTORY_CLI: join(codexHome, "missing-codestory-cli"),
          COPILOT_PLUGIN_DATA: "",
          PLUGIN_DATA: join(codexHome, "plugin-data"),
          PATH: "",
        },
        input: JSON.stringify({
          hook_event_name: "SessionStart",
          source: "startup",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      },
    );

    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stderr, /require is not defined/u);
    assert.match(
      JSON.parse(result.stdout).hookSpecificOutput.additionalContext,
      /CODESTORY GROUNDING AVAILABLE/u,
    );
  } finally {
    await rm(codexHome, { recursive: true, force: true });
  }
});

test("portable agent adapters are present", async () => {
  const copilotManifest = JSON.parse(
    await readFile(join(pluginRoot, ".github", "plugin", "plugin.json"), "utf8"),
  );
  assert.equal(copilotManifest.hooks, "hooks/copilot-hooks.json");
  assert.equal(copilotManifest.skills, "skills/");

  const cursorMcp = JSON.parse(
    await readFile(join(pluginRoot, ".cursor", "mcp.json"), "utf8"),
  );
  assert.equal(
    cursorMcp.mcpServers.codestory.env.CODESTORY_PLUGIN_DATA,
    "/absolute/path/to/codestory-plugin-data",
  );
});

test("default plugin prompts stay portable", async () => {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const internalExamplePatterns = [
    /RefreshMode/u,
    /codestory-store/u,
    /codestory-indexer/u,
    /resolve or install codestory-cli/u,
  ];

  for (const prompt of manifest.interface.defaultPrompt) {
    for (const pattern of internalExamplePatterns) {
      assert.equal(pattern.test(prompt), false, prompt);
    }
  }
});

test("markdown link checker passes for shipped doc surfaces", () => {
  const result = spawnSync(process.execPath, [".github/scripts/check-doc-links.mjs"], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
});
