import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { access, chmod, mkdir, mkdtemp, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));
const require = createRequire(import.meta.url);
const {
  classifyMcpRuntime,
  dirtyMarkerPathForProject,
  dirtyHookStatus,
  installDirtyHooks,
  uninstallDirtyHooks,
  writeDirtyMarker,
} = require(join(pluginRoot, "hooks", "codestory-runtime.cjs"));

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
          : process.platform === "darwin" && process.arch === "arm64"
            ? "macos-arm64"
            : null;
  assert.ok(target, `unsupported test platform: ${process.platform}-${process.arch}`);
  const archiveBase = `codestory-cli-v${version}-${target}`;
  const archiveName = `${archiveBase}.${target.startsWith("windows-") ? "zip" : "tar.gz"}`;
  return { archiveBase, archiveName };
}

async function writeFakeCli(cliPath) {
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'){if(args.includes('--wait-fresh')&&!args.includes('--repair')&&!args.includes('agent')){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}],local_refresh:{state:'fresh',reason:'already_fresh',blocks_local_surfaces:false,readiness_status:'ready',changed_file_count:0,new_file_count:0,removed_file_count:0,fatal_error_count:0}}));process.exit(0)}process.exit(9)}fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({source:process.env.CODESTORY_PLUGIN_CLI_SOURCE,path:process.env.CODESTORY_PLUGIN_CLI_PATH,sha256:process.env.CODESTORY_PLUGIN_CLI_SHA256,version:process.env.CODESTORY_PLUGIN_CLI_VERSION,pluginRoot:process.env.CODESTORY_PLUGIN_ROOT,pluginCacheVersion:process.env.CODESTORY_PLUGIN_CACHE_VERSION,repoRef:process.env.CODESTORY_PLUGIN_CLI_REPO_REF,buildSource:process.env.CODESTORY_PLUGIN_CLI_BUILD_SOURCE,archiveSha256:process.env.CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256,sidecarPolicy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,sidecarEnable:process.env.CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND,sidecarRepair:process.env.CODESTORY_PLUGIN_SIDECAR_NEXT_REPAIR_COMMAND,dirtyMarkerPath:process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PATH,dirtyMarkerRoot:process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT,args}))";
  if (process.platform === "win32") {
    await writeFile(
      cliPath,
      `@echo off\r\n"${process.execPath}" -e "${script}" %*\r\n`,
      "utf8",
    );
    return;
  }
  await writeFile(
    cliPath,
    `#!/bin/sh\n${JSON.stringify(process.execPath)} -e ${JSON.stringify(script)} "$@"\n`,
    "utf8",
  );
  await chmod(cliPath, 0o755);
}

async function writeRecordingCli(cliPath) {
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'&&process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR!=='1'&&args.includes('--wait-fresh')&&!args.includes('--repair')&&!args.includes('agent')){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}],local_refresh:{state:'fresh',reason:'already_fresh',blocks_local_surfaces:false,readiness_status:'ready',changed_file_count:0,new_file_count:0,removed_file_count:0,fatal_error_count:0}}));process.exit(0)}fs.appendFileSync(process.env.TEST_LOG,JSON.stringify({repair:process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR==='1',policy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,args})+'\\n')";
  if (process.platform === "win32") {
    await writeFile(cliPath, `@echo off\r\n"${process.execPath}" -e "${script}" %*\r\n`, "utf8");
    return;
  }
  await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} -e ${JSON.stringify(script)} "$@"\n`, "utf8");
  await chmod(cliPath, 0o755);
}

test("plugin metadata maps skill and direct stdio server", async () => {
  const manifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const mcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));

  assert.equal(manifest.name, "codestory");
  assert.equal(manifest.skills, "./skills/");
  assert.equal(manifest.hooks, "./hooks/claude-codex-hooks.json");
  assert.equal(manifest.mcpServers, "./.mcp.json");
  assert.equal(manifest.interface.capabilities.includes("Read"), true);
  assert.equal(
    manifest.interface.capabilities.includes("Lifecycle hooks"),
    true,
  );
  assert.equal(mcp.mcpServers.codestory.command, "node");
  assert.deepEqual(mcp.mcpServers.codestory.args, [
    "./scripts/codestory-mcp.cjs",
  ]);
  assert.equal(Object.hasOwn(mcp.mcpServers.codestory, "cwd"), false);
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

test("mcp launcher prefers a checksummed managed cli without PATH", async () => {
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

  try {
    await mkdir(cliDir, { recursive: true });
    await writeFakeCli(cliPath);
    const sha256 = createHash("sha256")
      .update(await readFile(cliPath))
      .digest("hex");
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli", sha256 }),
      "utf8",
    );

    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: dataDir,
        TEST_OUT: outFile,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(observed.path, cliPath);
    assert.equal(observed.sha256, sha256);
    assert.equal(observed.pluginRoot, pluginRoot);
    assert.equal(observed.pluginCacheVersion, "");
    assert.equal(observed.sidecarPolicy, "ask");
    assert.match(observed.sidecarEnable, /sidecar-policy enable/u);
    assert.match(observed.sidecarEnable, /--policy-file/u);
    assert.equal(
      observed.sidecarRepair.startsWith(`${JSON.stringify(cliPath)} ready --goal agent --repair`),
      true,
    );
    assert.match(observed.sidecarRepair, /ready --goal agent --repair/u);
    assert.match(observed.sidecarRepair, /--run-id shared-agent/u);
    assert.equal(observed.dirtyMarkerRoot, await realpath(repoRoot));
    assert.equal(observed.dirtyMarkerPath, dirtyMarkerPathForProject(repoRoot, dataDir));
    assert.deepEqual(observed.args, ["serve", "--stdio", "--refresh", "none"]);

    const enable = spawnSync(observed.sidecarEnable, {
      shell: true,
      env: {
        ...process.env,
        PLUGIN_DATA: "",
        COPILOT_PLUGIN_DATA: "",
      },
      encoding: "utf8",
    });
    assert.equal(enable.status, 0, enable.stderr);
    const policy = JSON.parse(
      await readFile(join(dataDir, "sidecar-setup-policy.json"), "utf8"),
    );
    assert.equal(policy.state, "enabled");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher infers Codex managed data from installed cache without env", async () => {
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
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli", sha256 }),
      "utf8",
    );
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
        PATH: pathDir,
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      cwd: repoRoot,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(observed.path, cliPath);
    assert.equal(observed.pluginRoot, installRoot);
    assert.equal(observed.pluginCacheVersion, version);
    assert.equal(observed.dirtyMarkerPath, dirtyMarkerPathForProject(repoRoot, dataDir));
  } finally {
    await rm(codexHome, { recursive: true, force: true });
    await rm(pathDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when only unusable PATH fallback is available", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const sourceVersion = readCargoVersion(await readFile(join(repoRoot, "crates", "codestory-cli", "Cargo.toml"), "utf8"));
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-mcp-"));
  const pathDir = await mkdtemp(join(tmpdir(), "codestory-path-candidate-"));
  const fakeCli = join(pathDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  await writeFile(
    fakeCli,
    process.platform === "win32"
      ? "@echo off\r\necho codestory-cli 0.0.1\r\n"
      : "#!/bin/sh\necho codestory-cli 0.0.1\n",
  );
  await chmod(fakeCli, 0o755);
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = [
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "resources/read", params: { uri: "codestory://status" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "ground", arguments: {} } }),
  ].join("\n") + "\n";

  try {
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: "",
        COPILOT_PLUGIN_DATA: "",
        PATH: pathDir,
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      cwd: repoRoot,
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.length, 3, result.stdout);
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.plugin_runtime.plugin_version, version);
    assert.equal(status.source_checkout_version, sourceVersion);
    assert.deepEqual(status.path_candidates, [
      {
        path: fakeCli,
        version: "0.0.1",
        active: true,
      },
    ]);
    assert.equal(status.plugin_runtime.plugin_root, pluginRoot);
    assert.equal(status.plugin_runtime.cli_source, "path_fallback");
    assert.equal(status.plugin_runtime.cli_path, fakeCli);
    assert.equal(status.runtime_truth.runtime_source, "path_fallback");
    assert.equal(status.runtime_truth.plugin_root, pluginRoot);
    assert.equal(status.runtime_truth.sidecar_policy, "ask");
    assert.equal(status.runtime_truth.sidecar_status.mode, "unavailable");
    assert.equal(status.runtime_truth.sidecar_status.run_id, "unavailable");
    assert.equal(status.runtime_truth.readiness_lanes.local_graph.status, "repair_setup");
    assert.equal(status.runtime_truth.readiness_lanes.agent_packet_search.profile, "agent");
    assert.equal(status.readiness[0].status, "repair_setup");
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.match(status.readiness[0].minimum_next[0], /Refresh or reinstall the CodeStory plugin/u);
    assert.equal(responses[2].result.isError, true);
    assert.equal(
      responses[2].result.structuredContent.code,
      "codestory_mcp_runtime_unavailable",
    );
    assert.equal(
      responses[2].result.structuredContent.plugin_runtime.plugin_root,
      pluginRoot,
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(pathDir, { recursive: true, force: true });
  }
});

test("mcp launcher waits for fresh local navigation without agent repair", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-repair-index-"));
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
        "fs.appendFileSync(logFile, JSON.stringify({ args, sidecarRepair: process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR === '1' }) + '\\n');",
        "if (command === '--version') { console.log('codestory-cli ' + version); process.exit(0); }",
        "if (command === 'ready') {",
        "  const ready = args.includes('--wait-fresh') && !args.includes('--repair') && !args.includes('agent');",
        "  console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: ready ? 'ready' : 'repair_index', summary: ready ? 'ready' : 'stale local graph', minimum_next: [], full_repair: [] }], local_refresh: { state: ready ? 'fresh' : 'stale', reason: ready ? 'refreshed' : 'index_changed', blocks_local_surfaces: !ready, readiness_status: ready ? 'ready' : 'repair_index', changed_file_count: ready ? 0 : 1, new_file_count: 0, removed_file_count: 0, fatal_error_count: 0 } }));",
        "  process.exit(0);",
        "}",
        "if (command === 'serve') { fs.writeFileSync(marker, 'serve-called'); process.exit(0); }",
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
        CODESTORY_PLUGIN_SIDECAR_POLICY: "ask",
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      encoding: "utf8",
      timeout: 15000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve-called");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    const readyCalls = calls.filter((call) => call.args[0] === "ready");
    assert.deepEqual(readyCalls.map((call) => call.args.slice(0, 5)), [
      ["ready", "--goal", "local", "--wait-fresh", "--project"],
    ]);
    assert.equal(calls.some((call) => call.args.includes("agent")), false);
    assert.equal(calls.some((call) => call.args.includes("--repair")), false);
    assert.equal(calls.some((call) => call.sidecarRepair), false);
    assert.ok(calls.some((call) => {
      return JSON.stringify(call.args) === JSON.stringify(["serve", "--stdio", "--refresh", "none"]);
    }));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when wait-fresh skips local refresh", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-index-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "fake-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "fake-codestory-cli.cmd" : "fake-codestory-cli",
  );
  const marker = join(dataDir, "serve-called.txt");
  const input = [
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "resources/read", params: { uri: "codestory://status" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "ground", arguments: {} } }),
  ].join("\n") + "\n";

  try {
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const version = process.env.TEST_CODESTORY_VERSION;",
        "const marker = process.env.TEST_OUT;",
        "const command = process.argv[2];",
        "if (command === '--version') { console.log('codestory-cli ' + version); process.exit(0); }",
        "if (command === 'ready') {",
        "  console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: 'repair_index', summary: 'No indexed symbols are available yet.', minimum_next: ['codestory-cli ready --goal local --wait-fresh --project \"fixture\" --format json'], full_repair: ['codestory-cli doctor --project \"fixture\"'] }], local_refresh: { state: 'skipped_locked', reason: 'index_locked', blocks_local_surfaces: true, readiness_status: 'repair_index', changed_file_count: 1, new_file_count: 0, removed_file_count: 0, fatal_error_count: 0 } }));",
        "  process.exit(0);",
        "}",
        "if (command === 'serve') { fs.writeFileSync(marker, 'serve-called'); process.exit(1); }",
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
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 15000,
    });

    assert.equal(result.status, 0, result.stderr);
    await assert.rejects(access(marker));
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.length, 3, result.stdout);
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.plugin_runtime.cli_source, "local_dev_override");
    assert.equal(status.runtime_truth.runtime_source, "local_dev_override");
    assert.equal(status.runtime_truth.sidecar_status.mode, "unavailable");
    assert.equal(status.runtime_truth.readiness_lanes.local_graph.refresh_state, "skipped_locked");
    assert.equal(status.readiness[0].status, "repair_index");
    assert.equal(status.readiness[0].repair_reason, "local_navigation_wait_fresh_skipped_locked");
    assert.equal(status.local_refresh.state, "skipped_locked");
    assert.equal(status.readiness[0].local_refresh.reason, "index_locked");
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.equal(status.allowed_surfaces.ground.status, "repair_index");
    assert.match(status.readiness[0].minimum_next[0], /ready --goal local --wait-fresh/u);
    assert.deepEqual(status.readiness[0].setup.local_wait_fresh_args.slice(0, 5), [
      "ready",
      "--goal",
      "local",
      "--wait-fresh",
      "--project",
    ]);
    assert.equal(responses[2].result.isError, true);
    assert.equal(responses[2].result.structuredContent.status, "repair_index");
    assert.equal(responses[2].result.structuredContent.local_refresh.state, "skipped_locked");
    assert.equal(responses[2].result.structuredContent.local_refresh.reason, "index_locked");
    assert.equal(responses[2].result.structuredContent.local_refresh.changed_file_count, 1);
    assert.equal(responses[2].result.structuredContent.local_refresh.fatal_error_count, 0);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when local navigation repair times out", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-repair-timeout-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "fake-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "fake-codestory-cli.cmd" : "fake-codestory-cli",
  );
  const marker = join(dataDir, "serve-called.txt");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: "status",
    method: "resources/read",
    params: { uri: "codestory://status" },
  }) + "\n";

  try {
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const version = process.env.TEST_CODESTORY_VERSION;",
        "const marker = process.env.TEST_OUT;",
        "const args = process.argv.slice(2);",
        "const command = args[0];",
        "if (command === '--version') { console.log('codestory-cli ' + version); process.exit(0); }",
        "if (command === 'ready' && args.includes('--wait-fresh')) { const end = Date.now() + 200; while (Date.now() < end) {} process.exit(0); }",
        "if (command === 'ready') { console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: 'repair_index', summary: 'stale local graph', minimum_next: [], full_repair: [] }] })); process.exit(0); }",
        "if (command === 'serve') { fs.writeFileSync(marker, 'serve-called'); process.exit(1); }",
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
        CODESTORY_PLUGIN_LOCAL_REPAIR_TIMEOUT_MS: "50",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_OUT: marker,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    await assert.rejects(access(marker));
    const response = JSON.parse(result.stdout.trim());
    const status = JSON.parse(response.result.contents[0].text);
    assert.equal(status.readiness[0].repair_reason, "local_navigation_wait_fresh_timeout");
    assert.equal(status.local_refresh.state, "failed");
    assert.equal(status.local_refresh.reason, "wait_fresh_timeout");
    assert.deepEqual(status.readiness[0].setup.local_wait_fresh_args.slice(0, 5), [
      "ready",
      "--goal",
      "local",
      "--wait-fresh",
      "--project",
    ]);
    assert.equal(status.allowed_surfaces.ground.allowed, false);
  } finally {
    await new Promise((resolve) => setTimeout(resolve, 300));
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
    params: { uri: "codestory://status" },
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
    assert.equal(status.readiness[0].repair_reason, "local_dev_override_cli_unspawnable");
    assert.equal(status.allowed_surfaces.ground.allowed, false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher fails open when managed cli probe fails", async () => {
  const { spawnSync } = await import("node:child_process");
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
    id: "tool",
    method: "tools/call",
    params: { name: "ground", arguments: {} },
  }) + "\n";

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
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli", sha256 }),
      "utf8",
    );

    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: dataDir,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const response = JSON.parse(result.stdout.trim());
    assert.equal(response.result.isError, true);
    assert.equal(
      response.result.structuredContent.repair_reason,
      "managed_cli_unspawnable",
    );
    assert.equal(
      response.result.structuredContent.plugin_runtime.plugin_version,
      version,
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher persists sidecar setup policy in plugin data", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-sidecar-policy-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");

  try {
    const enable = spawnSync(process.execPath, [launcher, "sidecar-policy", "enable"], {
      env: { PLUGIN_DATA: dataDir },
      encoding: "utf8",
    });
    assert.equal(enable.status, 0, enable.stderr);
    assert.equal(JSON.parse(enable.stdout).state, "enabled");

    const disable = spawnSync(process.execPath, [launcher, "sidecar-policy", "disable"], {
      env: { PLUGIN_DATA: dataDir },
      encoding: "utf8",
    });
    assert.equal(disable.status, 0, disable.stderr);
    assert.equal(JSON.parse(disable.stdout).state, "disabled");

    const policy = JSON.parse(
      await readFile(join(dataDir, "sidecar-setup-policy.json"), "utf8"),
    );
    assert.equal(policy.state, "disabled");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("enabled sidecar policy runs one agent repair at MCP startup", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-sidecar-enabled-"));
  const logFile = join(dataDir, "calls.jsonl");
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(
    cliDir,
    process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
  );
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");

  try {
    await mkdir(cliDir, { recursive: true });
    await writeRecordingCli(cliPath);
    const sha256 = createHash("sha256")
      .update(await readFile(cliPath))
      .digest("hex");
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli", sha256 }),
      "utf8",
    );
    await writeFile(
      join(dataDir, "sidecar-setup-policy.json"),
      JSON.stringify({ state: "enabled" }),
      "utf8",
    );

    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: dataDir,
        TEST_LOG: logFile,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);

    const text = await readFile(logFile, "utf8");
    const calls = text.trim().split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
    const repairCalls = calls.filter((call) => call.repair);
    assert.equal(repairCalls.length, 1, text);
    assert.equal(repairCalls[0].policy, "enabled");
    assert.deepEqual(repairCalls[0].args, [
      "ready",
      "--goal",
      "agent",
      "--repair",
      "--project",
      repoRoot,
      "--format",
      "json",
      "--run-id",
      "shared-agent",
    ]);
    assert.ok(calls.some((call) => {
      return JSON.stringify(call.args) === JSON.stringify(["serve", "--stdio", "--refresh", "none"]);
    }), text);
    const policy = JSON.parse(await readFile(join(dataDir, "sidecar-setup-policy.json"), "utf8"));
    assert.equal(policy.last_repair.state, "completed");
    assert.equal(policy.last_repair.project_root, repoRoot);
    assert.match(policy.last_repair.command, /ready --goal agent --repair/u);
    assert.match(policy.last_repair.command, /--run-id shared-agent/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher provisions a checksummed release asset into plugin data", async (t) => {
  const { spawnSync } = await import("node:child_process");
  const tarProbe = spawnSync("tar", ["--version"], { encoding: "utf8" });
  if (tarProbe.status !== 0) {
    t.skip("tar unavailable for archive fixture");
    return;
  }

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
    await mkdir(stageDir, { recursive: true });
    await writeFakeCli(cliPath);
    const packArgs = archiveName.endsWith(".zip")
      ? ["-a", "-cf", archivePath, "-C", releaseDir, archiveBase]
      : ["-czf", archivePath, "-C", releaseDir, archiveBase];
    const pack = spawnSync("tar", packArgs, { encoding: "utf8" });
    assert.equal(pack.status, 0, pack.stderr);
    const archiveSha256 = createHash("sha256")
      .update(await readFile(archivePath))
      .digest("hex");
    await writeFile(
      join(releaseDir, "SHA256SUMS.txt"),
      `${archiveSha256}  ${archiveName}\n`,
      "utf8",
    );

    const result = spawnSync(process.execPath, [launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        CODESTORY_PLUGIN_RELEASE_DIR: releaseDir,
        PLUGIN_DATA: dataDir,
        TEST_OUT: outFile,
      },
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(observed.version, version);
    assert.equal(observed.repoRef, `v${version}`);
    assert.equal(observed.buildSource, "github_release");
    assert.equal(observed.archiveSha256, archiveSha256);
    assert.match(
      observed.path,
      new RegExp(String.raw`codestory-cli[\\/]+${version.replaceAll(".", String.raw`\.`)}[\\/]bin[\\/]codestory-cli`, "u"),
    );
    assert.deepEqual(observed.args, ["serve", "--stdio", "--refresh", "none"]);

    const manifest = JSON.parse(
      await readFile(join(dataDir, "codestory-cli", version, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.version, version);
    assert.equal(manifest.repo_ref, `v${version}`);
    assert.equal(manifest.build_source, "github_release");
    assert.equal(manifest.archive, archiveName);
    assert.equal(manifest.archive_sha256, archiveSha256);
    assert.equal(typeof manifest.sha256, "string");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("startup hook bootstraps managed cli before reporting MCP visibility", async (t) => {
  const tarProbe = spawnSync("tar", ["--version"], { encoding: "utf8" });
  if (tarProbe.status !== 0) {
    t.skip("tar unavailable for archive fixture");
    return;
  }

  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-bootstrap-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-hook-release-"));
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");
  const { archiveBase, archiveName } = releaseAssetForPlatform(version);
  const stageDir = join(releaseDir, archiveBase);
  const cliPath = join(stageDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  const archivePath = join(releaseDir, archiveName);

  try {
    await mkdir(stageDir, { recursive: true });
    await writeFakeCli(cliPath);
    const packArgs = archiveName.endsWith(".zip")
      ? ["-a", "-cf", archivePath, "-C", releaseDir, archiveBase]
      : ["-czf", archivePath, "-C", releaseDir, archiveBase];
    const pack = spawnSync("tar", packArgs, { encoding: "utf8" });
    assert.equal(pack.status, 0, pack.stderr);
    const archiveSha256 = createHash("sha256")
      .update(await readFile(archivePath))
      .digest("hex");
    await writeFile(join(releaseDir, "SHA256SUMS.txt"), `${archiveSha256}  ${archiveName}\n`, "utf8");

    const result = spawnSync(process.execPath, [hookPath], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        CODESTORY_MCP_RESOURCES_EXPOSED: "",
        CODESTORY_PLUGIN_RELEASE_DIR: releaseDir,
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      },
      input: JSON.stringify({
        hook_event_name: "SessionStart",
        source: "startup",
        cwd: repoRoot,
      }),
      encoding: "utf8",
      timeout: 30000,
    });

    assert.equal(result.status, 0, result.stderr);
    const context = JSON.parse(result.stdout).hookSpecificOutput.additionalContext;
    assert.match(context, /managed_bootstrap: ready/u);
    assert.match(context, /managed_bootstrap_cli_source: managed/u);
    assert.match(context, /managed_bootstrap_local_refresh: fresh/u);
    assert.match(context, /mcp_resources_exposed: mcp_resources_not_model_visible/u);
    assert.match(context, /managed_cli_present: yes/u);
    assert.doesNotMatch(context, /where\.exe codestory-cli|command -v codestory-cli|adding CodeStory to PATH/u);

    const manifest = JSON.parse(
      await readFile(join(dataDir, "codestory-cli", version, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.version, version);
    assert.equal(manifest.build_source, "github_release");
    assert.equal(manifest.archive_sha256, archiveSha256);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
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

test("mcp launcher keeps managed provision failures primary without PATH", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-provision-fail-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-empty-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: "codestory://status" },
  }) + "\n";

  try {
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        CODESTORY_PLUGIN_RELEASE_DIR: releaseDir,
        PLUGIN_DATA: dataDir,
        PATH: "",
        ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const response = JSON.parse(result.stdout.trim());
    const status = JSON.parse(response.result.contents[0].text);
    assert.match(
      status.degraded_reason,
      /^managed_cli_provision_failed:managed_cli_asset_fetch_failed:SHA256SUMS\.txt:elapsed_ms=\d+:attempts=1:retry=restart_reload_status:/u,
    );
    assert.equal(status.plugin_runtime.cli_source, "path_fallback");
    assert.deepEqual(status.path_candidates, []);
    assert.equal(
      status.plugin_runtime.warnings.includes("managed_cli_unavailable_no_path_fallback"),
      true,
    );
    assert.match(status.readiness[0].minimum_next[0], /^Restart\/reload/u);
    assert.equal(
      status.readiness[0].minimum_next.some((step) => {
        return /where\.exe codestory-cli|command -v codestory-cli|codestory-cli --version/u.test(step);
      }),
      false,
    );
  } finally {
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
  assert.equal(hookConfig.hooks.UserPromptSubmit.length, 1);

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

test("hook manifest timeouts cover managed bootstrap budget", async () => {
  const hookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "claude-codex-hooks.json"), "utf8"),
  );
  const copilotHookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "copilot-hooks.json"), "utf8"),
  );
  const runtimeSource = await readFile(join(pluginRoot, "hooks", "codestory-runtime.cjs"), "utf8");
  const mcpSource = await readFile(join(pluginRoot, "scripts", "codestory-mcp.cjs"), "utf8");
  const numberFrom = (text, pattern, label) => {
    const match = text.match(pattern);
    assert.ok(match, `missing ${label}`);
    return Number.parseInt(match[1], 10);
  };

  const bootstrapTimeoutMs = numberFrom(
    runtimeSource,
    /function bootstrapTimeoutMs\(\) \{[\s\S]*?return Number\.isFinite\(parsed\) && parsed > 0 \? parsed : (\d+);/u,
    "bootstrap timeout default",
  );
  const releaseDownloadTimeoutMs = numberFrom(
    mcpSource,
    /const releaseDownloadTimeoutMs = (\d+);/u,
    "release download timeout",
  );
  const releaseDownloadAttempts = numberFrom(
    mcpSource,
    /const releaseDownloadAttempts = (\d+);/u,
    "release download attempts",
  );
  const localWaitFreshTimeoutMs = numberFrom(
    mcpSource,
    /function localWaitFreshTimeoutMs\(\) \{[\s\S]*?return Number\.isFinite\(parsed\) && parsed > 0 \? parsed : (\d+);/u,
    "local wait-fresh timeout default",
  );
  assert.equal(localWaitFreshTimeoutMs, 5000, "local wait-fresh default must stay MCP startup-safe");
  const requiredTimeoutSec = Math.max(
    Math.ceil((bootstrapTimeoutMs + 30000) / 1000),
    Math.ceil((releaseDownloadTimeoutMs * releaseDownloadAttempts + localWaitFreshTimeoutMs) / 1000),
  );
  const claudeTimeouts = Object.values(hookConfig.hooks)
    .flat()
    .flatMap((entry) => entry.hooks)
    .map((hook) => hook.timeout);
  const copilotTimeouts = copilotHookConfig.hooks.sessionStart.map((hook) => hook.timeoutSec);

  for (const timeoutSec of [...claudeTimeouts, ...copilotTimeouts]) {
    assert.equal(typeof timeoutSec, "number");
    assert.ok(
      timeoutSec >= requiredTimeoutSec,
      `hook timeout ${timeoutSec}s must cover managed bootstrap budget ${requiredTimeoutSec}s`,
    );
  }
});

async function withFakeCodeStoryCli(callback) {
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-test-"));
  const shPath = join(binDir, "codestory-cli");
  const cmdPath = join(binDir, "codestory-cli.cmd");

  await writeFile(
    shPath,
    "#!/bin/sh\nprintf 'FAKE_CODESTORY_CLI %s\\n' \"$*\"\n",
    "utf8",
  );
  await chmod(shPath, 0o755);
  await writeFile(
    cmdPath,
    "@echo off\r\necho FAKE_CODESTORY_CLI %*\r\n",
    "utf8",
  );

  try {
    await callback(binDir);
  } finally {
    await rm(binDir, { recursive: true, force: true });
  }
}

async function withTempHookInstall(callback) {
  const { cp } = await import("node:fs/promises");
  const installRoot = await mkdtemp(join(tmpdir(), "codestory-hook-install-"));
  try {
    await cp(join(pluginRoot, "hooks"), join(installRoot, "hooks"), {
      recursive: true,
    });
    await callback(installRoot);
  } finally {
    await rm(installRoot, { recursive: true, force: true });
  }
}

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

function runCodexHook(input, env) {
  const result = spawnSync(process.execPath, [join(pluginRoot, "hooks", "codestory-activate.cjs")], {
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

test("hook output keeps CodeStory ambient and checks MCP before CLI fallback", async () => {
  const { spawnSync } = await import("node:child_process");
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-mcp-first-"));

  try {
    await withFakeCodeStoryCli(async (binDir) => {
      const fakeCli = process.platform === "win32"
        ? join(binDir, "codestory-cli.cmd")
        : join(binDir, "codestory-cli");
      const env = {
        ...process.env,
        CODESTORY_CLI: fakeCli,
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      };

      const sessionResult = spawnSync(process.execPath, [hookPath], {
        env,
        input: JSON.stringify({
          hook_event_name: "SessionStart",
          source: "startup",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(sessionResult.status, 0, sessionResult.stderr);
      const sessionOutput = JSON.parse(sessionResult.stdout);
      const sessionContext = sessionOutput.hookSpecificOutput.additionalContext;
      assert.equal(sessionOutput.systemMessage, "CODESTORY:BACKGROUND");
      assert.match(sessionContext, /^CODESTORY SESSION GROUNDING ACTIVE \(startup\)/u);
      assert.match(sessionContext, /CODESTORY MCP RUNTIME DETECTION/u);
      assert.match(sessionContext, /mcp_config_installed: yes/u);
      assert.match(sessionContext, /mcp_process_launchable: yes/u);
      assert.match(sessionContext, /mcp_resources_exposed: mcp_resources_not_model_visible/u);
      assert.match(sessionContext, /do not add CodeStory to PATH/u);
      assert.doesNotMatch(sessionContext, /FAKE_CODESTORY_CLI/u);
      assert.doesNotMatch(sessionContext, /## Runtime Truth/u);

      const promptResult = spawnSync(process.execPath, [hookPath], {
        env,
        input: JSON.stringify({
          hook_event_name: "UserPromptSubmit",
          prompt: "Where is RefreshMode defined?",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(promptResult.status, 0, promptResult.stderr);
      const promptOutput = JSON.parse(promptResult.stdout);
      const promptContext = promptOutput.hookSpecificOutput.additionalContext;
      assert.equal(promptOutput.hookSpecificOutput.hookEventName, "UserPromptSubmit");
      assert.match(promptContext, /Prompt: Where is RefreshMode defined\?/u);
      assert.match(promptContext, /CODESTORY MCP RUNTIME DETECTION/u);
      assert.doesNotMatch(promptContext, /FAKE_CODESTORY_CLI/u);
      assert.doesNotMatch(promptContext, /attempted request packet/u);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook degraded output is short when no MCP or managed runtime is usable", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-degraded-"));
  const pathDir = await mkdtemp(join(tmpdir(), "codestory-hook-path-"));

  try {
    await writeNodeCli(pathDir, "console.log('FAKE_CODESTORY_CLI');");
    await withTempHookInstall(async (installRoot) => {
      const result = spawnSync(process.execPath, [join(installRoot, "hooks", "codestory-activate.cjs")], {
        env: {
          ...process.env,
          COPILOT_PLUGIN_DATA: "",
          PLUGIN_DATA: dataDir,
          PATH: pathDir,
          ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
        },
        input: JSON.stringify({
          hook_event_name: "UserPromptSubmit",
          prompt: "Find the hook failure.",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(result.status, 0, result.stderr);
      const context = JSON.parse(result.stdout).hookSpecificOutput.additionalContext;
      assert.match(context, /CodeStory degraded mode: no MCP or managed runtime surface is usable/u);
      assert.match(context, /First failing layer: MCP: no codestory server configured/u);
      assert.doesNotMatch(context, /CODESTORY BACKGROUND GROUNDING/u);
      assert.doesNotMatch(context, /FAKE_CODESTORY_CLI/u);
      assert.ok(context.length < 1600, context);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(pathDir, { recursive: true, force: true });
  }
});

test("hook failed preflight switches degraded guidance to bounded source fallback", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-preflight-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-preflight-bin-"));

  try {
    const cliPath = await writeNodeCli(
      binDir,
      "process.stderr.write('first preflight failed\\n' + 'x'.repeat(8000));process.exit(2);",
    );
    await withTempHookInstall(async (installRoot) => {
      const hookPath = join(installRoot, "hooks", "codestory-activate.cjs");
      const first = spawnSync(process.execPath, [hookPath], {
        env: {
          ...process.env,
          CODESTORY_CLI: cliPath,
          COPILOT_PLUGIN_DATA: "",
          PLUGIN_DATA: dataDir,
        },
        input: JSON.stringify({
          hook_event_name: "UserPromptSubmit",
          prompt: "Find the hook failure.",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(first.status, 0, first.stderr);
      const firstContext = JSON.parse(first.stdout).hookSpecificOutput.additionalContext;
      assert.match(firstContext, /Reason: first preflight failed/u);
      assert.match(firstContext, /CodeStory is unavailable for this session/u);
      assert.match(firstContext, /hook output truncated by hook budget/u);
      assert.doesNotMatch(firstContext, /CODESTORY BACKGROUND GROUNDING/u);
      assert.ok(firstContext.length < 5600, firstContext);

      const second = spawnSync(process.execPath, [hookPath], {
        env: {
          ...process.env,
          CODESTORY_CLI: "",
          COPILOT_PLUGIN_DATA: "",
          PLUGIN_DATA: dataDir,
        },
        input: JSON.stringify({
          hook_event_name: "SessionStart",
          source: "resume",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(second.status, 0, second.stderr);
      const secondContext = JSON.parse(second.stdout).hookSpecificOutput.additionalContext;
      assert.match(secondContext, /CodeStory is unavailable for this session/u);
      assert.match(secondContext, /bounded source reads/u);
      assert.doesNotMatch(secondContext, /repair archaeology/u);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook dedupes repeated request prompts within plugin state", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-dedupe-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-dedupe-bin-"));

  try {
    const cliPath = await writeNodeCli(binDir, "console.log('packet ok');");
    await withTempHookInstall(async (installRoot) => {
      const hookPath = join(installRoot, "hooks", "codestory-activate.cjs");
      const env = {
        ...process.env,
        CODESTORY_CLI: cliPath,
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      };
      const input = JSON.stringify({
        hook_event_name: "UserPromptSubmit",
        prompt: "Where is RefreshMode defined?",
        cwd: repoRoot,
      });
      const first = spawnSync(process.execPath, [hookPath], {
        env,
        input,
        encoding: "utf8",
      });
      const second = spawnSync(process.execPath, [hookPath], {
        env,
        input,
        encoding: "utf8",
      });

      assert.equal(first.status, 0, first.stderr);
      assert.equal(second.status, 0, second.stderr);
      const firstContext = JSON.parse(first.stdout).hookSpecificOutput.additionalContext;
      const secondOutput = JSON.parse(second.stdout);
      assert.match(firstContext, /event_taxonomy: user_prompt/u);
      assert.match(firstContext, /Packet skipped: sidecar-backed packet\/search readiness is not proven full/u);
      assert.equal(Object.hasOwn(secondOutput, "hookSpecificOutput"), false);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook invokes packet when agent packet search readiness is ready", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-packet-ready-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-packet-ready-bin-"));
  const logFile = join(dataDir, "calls.jsonl");

  try {
    const cliPath = await writeNodeCli(
      binDir,
      [
        "const fs = require('fs');",
        "const args = process.argv.slice(2);",
        "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify(args) + '\\n');",
        "if (args[0] === 'ready') {",
        "  console.log(JSON.stringify({ verdicts: [{ goal: 'agent_packet_search', status: 'ready', index: { freshness: { status: 'fresh', changed_file_count: 0, new_file_count: 0, removed_file_count: 0 } } }] }));",
        "  process.exit(0);",
        "}",
        "if (args[0] === 'packet') { console.log('packet ok'); process.exit(0); }",
        "process.exit(2);",
      ].join("\n"),
    );
    await withTempHookInstall(async (installRoot) => {
      const hookPath = join(installRoot, "hooks", "codestory-activate.cjs");
      const result = spawnSync(process.execPath, [hookPath], {
        env: {
          ...process.env,
          CODESTORY_CLI: cliPath,
          COPILOT_PLUGIN_DATA: "",
          PLUGIN_DATA: dataDir,
          TEST_LOG: logFile,
        },
        input: JSON.stringify({
          hook_event_name: "UserPromptSubmit",
          prompt: "Where is RefreshMode defined?",
          cwd: repoRoot,
        }),
        encoding: "utf8",
      });

      assert.equal(result.status, 0, result.stderr);
      const context = JSON.parse(result.stdout).hookSpecificOutput.additionalContext;
      assert.match(context, /packet ok/u);
      const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
      assert.deepEqual(calls.map((args) => args[0]), ["ready", "packet"]);
      assert.deepEqual(calls[0].slice(0, 4), ["ready", "--goal", "agent", "--project"]);
      assert.deepEqual(calls[1].slice(0, 4), ["packet", "--project", repoRoot, "--question"]);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook resets instruction dedupe on fresh startup session boundary", async () => {
  const { spawnSync } = await import("node:child_process");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-startup-dedupe-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-startup-dedupe-bin-"));

  try {
    const cliPath = await writeNodeCli(binDir, "console.log('ground ok');");
    await withTempHookInstall(async (installRoot) => {
      const hookPath = join(installRoot, "hooks", "codestory-activate.cjs");
      const env = {
        ...process.env,
        CODESTORY_CLI: cliPath,
        COPILOT_PLUGIN_DATA: "",
        PLUGIN_DATA: dataDir,
      };
      const startupInput = JSON.stringify({
        hook_event_name: "SessionStart",
        source: "startup",
        cwd: repoRoot,
      });
      const resumeInput = JSON.stringify({
        hook_event_name: "SessionStart",
        source: "resume",
        cwd: repoRoot,
      });
      const clearInput = JSON.stringify({
        hook_event_name: "SessionStart",
        source: "clear",
        cwd: repoRoot,
      });
      const firstStartup = spawnSync(process.execPath, [hookPath], {
        env,
        input: startupInput,
        encoding: "utf8",
      });
      const resume = spawnSync(process.execPath, [hookPath], {
        env,
        input: resumeInput,
        encoding: "utf8",
      });
      const clear = spawnSync(process.execPath, [hookPath], {
        env,
        input: clearInput,
        encoding: "utf8",
      });
      const secondStartup = spawnSync(process.execPath, [hookPath], {
        env,
        input: startupInput,
        encoding: "utf8",
      });

      assert.equal(firstStartup.status, 0, firstStartup.stderr);
      assert.equal(resume.status, 0, resume.stderr);
      assert.equal(clear.status, 0, clear.stderr);
      assert.equal(secondStartup.status, 0, secondStartup.stderr);
      const firstContext = JSON.parse(firstStartup.stdout).hookSpecificOutput.additionalContext;
      const resumeContext = JSON.parse(resume.stdout).hookSpecificOutput.additionalContext;
      const clearContext = JSON.parse(clear.stdout).hookSpecificOutput.additionalContext;
      const secondContext = JSON.parse(secondStartup.stdout).hookSpecificOutput.additionalContext;
      const fullInstructions = /CODESTORY BACKGROUND GROUNDING (?:ACTIVE|RULES)/u;
      assert.match(firstContext, fullInstructions);
      assert.doesNotMatch(resumeContext, fullInstructions);
      assert.match(clearContext, fullInstructions);
      assert.match(clearContext, /ground ok/u);
      assert.match(secondContext, fullInstructions);
      assert.match(secondContext, /ground ok/u);
    });
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook prompt output dedupes repeated prompts", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-prompt-dedupe-"));
  const env = {
    PLUGIN_DATA: dataDir,
    PATH: "",
  };

  try {
    const first = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RefreshMode defined?",
      cwd: repoRoot,
    }, env);
    const second = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RefreshMode defined?",
      cwd: repoRoot,
    }, env);
    const third = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is strict_sidecar_status defined?",
      cwd: repoRoot,
    }, env);

    assert.match(first.hookSpecificOutput.additionalContext, /event_taxonomy: user_prompt/u);
    assert.equal(Object.hasOwn(second, "hookSpecificOutput"), false);
    assert.match(third.hookSpecificOutput.additionalContext, /Where is strict_sidecar_status defined\?/u);
    const stateText = await readFile(join(dataDir, ".codestory-hook-output-state.json"), "utf8");
    const promptHash = createHash("sha256")
      .update("Where is RefreshMode defined?")
      .digest("hex")
      .slice(0, 16);
    assert.match(stateText, new RegExp(`prompt:${promptHash}`, "u"));
    assert.doesNotMatch(stateText, /Where is RefreshMode defined/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook resume and compact output use short runtime caps", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-short-events-"));
  const env = {
    PLUGIN_DATA: dataDir,
    PATH: "",
  };

  try {
    for (const source of ["resume", "compact"]) {
      const output = runCodexHook({
        hook_event_name: "SessionStart",
        source,
        cwd: repoRoot,
      }, env);
      const context = output.hookSpecificOutput.additionalContext;
      assert.match(context, new RegExp(`event_taxonomy: ${source}`, "u"));
      assert.match(context, /output_cap_chars: 2200/u);
      assert.equal(context.length <= 2200, true, `${source} context length ${context.length}`);
      assert.doesNotMatch(context, /CODESTORY BACKGROUND GROUNDING RULES/u);
      assert.doesNotMatch(context, /attempted session ground/u);
    }
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook goal heartbeat is quiet until readiness or freshness evidence changes", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-heartbeat-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-heartbeat-bin-"));
  const cliPath = await writeNodeCli(
    binDir,
    [
      "const status = process.env.TEST_AGENT_STATUS || 'repair_retrieval';",
      "const freshness = process.env.TEST_FRESHNESS_STATUS || 'stale';",
      "const changed = Number(process.env.TEST_CHANGED_FILES || 1);",
      "const args = process.argv.slice(2);",
      "if (args[0] === 'ready') {",
      "  console.log(JSON.stringify({ verdicts: [{ goal: 'agent_packet_search', status, index: { freshness: { status: freshness, changed_file_count: changed, new_file_count: 0, removed_file_count: 0 } } }] }));",
      "  process.exit(0);",
      "}",
      "process.exit(2);",
    ].join("\n"),
  );
  const env = {
    PLUGIN_DATA: dataDir,
    CODESTORY_CLI: cliPath,
    TEST_AGENT_STATUS: "repair_retrieval",
    TEST_FRESHNESS_STATUS: "stale",
    TEST_CHANGED_FILES: "1",
    PATH: "",
  };

  try {
    const first = runCodexHook({
      hook_event_name: "GoalLoopHeartbeat",
      cwd: repoRoot,
    }, env);
    assert.equal(Object.hasOwn(first, "hookSpecificOutput"), false);

    env.TEST_AGENT_STATUS = "ready";
    env.TEST_FRESHNESS_STATUS = "fresh";
    env.TEST_CHANGED_FILES = "0";
    const changed = runCodexHook({
      hook_event_name: "GoalLoopHeartbeat",
      cwd: repoRoot,
    }, env);
    assert.match(changed.hookSpecificOutput.additionalContext, /event_taxonomy: goal_heartbeat/u);
    assert.match(changed.hookSpecificOutput.additionalContext, /agent_readiness_evidence: agent_packet_search=ready/u);
    assert.match(changed.hookSpecificOutput.additionalContext, /freshness_evidence: fresh changed=0 new=0 removed=0/u);

    const repeated = runCodexHook({
      hook_event_name: "GoalLoopHeartbeat",
      cwd: repoRoot,
    }, env);
    assert.equal(Object.hasOwn(repeated, "hookSpecificOutput"), false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook MCP classifier distinguishes configured launchable and model-visible states", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-classify-"));
  const version = await readPluginVersion();
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(cliDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");

  try {
    const configured = classifyMcpRuntime({ pluginRoot, pluginDataDir: dataDir });
    assert.equal(configured.mcp_config_installed, true);
    assert.equal(configured.mcp_process_launchable, true);
    assert.equal(configured.mcp_resources_exposed, false);
    assert.equal(configured.mcp_resource_status, "mcp_resources_not_model_visible");
    assert.equal(configured.managed_cli_present, false);

    await mkdir(cliDir, { recursive: true });
    await writeFakeCli(cliPath);
    await writeFile(
      join(cliDir, "manifest.json"),
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli" }),
      "utf8",
    );
    const managed = classifyMcpRuntime({ pluginRoot, pluginDataDir: dataDir });
    assert.equal(managed.managed_cli_present, true);
    assert.equal(managed.managed_cli_path, cliPath);
    assert.equal(managed.degraded_no_surface, false);

    await writeFile(
      join(dataDir, ".codestory-mcp-runtime.json"),
      JSON.stringify({ source: "managed", path: cliPath }),
      "utf8",
    );
    const runtimeStateOnly = classifyMcpRuntime({ pluginRoot, pluginDataDir: dataDir });
    assert.equal(runtimeStateOnly.mcp_runtime_state_present, true);
    assert.equal(runtimeStateOnly.mcp_resources_exposed, false);
    assert.equal(runtimeStateOnly.mcp_resource_status, "mcp_resources_not_model_visible");
    assert.equal(runtimeStateOnly.degraded_no_surface, false);

    const previous = process.env.CODESTORY_MCP_RESOURCES_EXPOSED;
    try {
      process.env.CODESTORY_MCP_RESOURCES_EXPOSED = "1";
      const exposed = classifyMcpRuntime({ pluginRoot, pluginDataDir: dataDir });
      assert.equal(exposed.mcp_resources_exposed, true);
      assert.equal(exposed.mcp_resource_status, "mcp_resources_exposed");
      assert.equal(exposed.degraded_no_surface, false);
    } finally {
      if (previous === undefined) {
        delete process.env.CODESTORY_MCP_RESOURCES_EXPOSED;
      } else {
        process.env.CODESTORY_MCP_RESOURCES_EXPOSED = previous;
      }
    }
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("hook MCP classifier distinguishes launch failure and no runtime", async () => {
  const brokenRoot = await mkdtemp(join(tmpdir(), "codestory-broken-mcp-"));
  const emptyRoot = await mkdtemp(join(tmpdir(), "codestory-no-mcp-"));

  try {
    await writeFile(
      join(brokenRoot, ".mcp.json"),
      JSON.stringify({ mcpServers: { codestory: { command: "node", args: ["./missing.cjs"] } } }),
      "utf8",
    );
    const broken = classifyMcpRuntime({ pluginRoot: brokenRoot, pluginDataDir: null });
    assert.equal(broken.mcp_config_installed, true);
    assert.equal(broken.mcp_process_launchable, false);
    assert.equal(broken.mcp_resource_status, "mcp_resources_unavailable");
    assert.equal(broken.managed_cli_present, false);

    const none = classifyMcpRuntime({ pluginRoot: emptyRoot, pluginDataDir: null });
    assert.equal(none.mcp_config_installed, false);
    assert.equal(none.mcp_process_launchable, false);
    assert.equal(none.managed_cli_present, false);
    assert.equal(none.degraded_no_surface, true);
  } finally {
    await rm(brokenRoot, { recursive: true, force: true });
    await rm(emptyRoot, { recursive: true, force: true });
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

    await withFakeCodeStoryCli(async (binDir) => {
      const fakeCli = process.platform === "win32"
        ? join(binDir, "codestory-cli.cmd")
        : join(binDir, "codestory-cli");
      const result = spawnSync(
        process.execPath,
        [join(installRoot, "hooks", "codestory-activate.cjs")],
        {
          env: {
            ...process.env,
            CODESTORY_CLI: fakeCli,
            COPILOT_PLUGIN_DATA: "",
            PLUGIN_DATA: join(codexHome, "plugin-data"),
          },
          input: JSON.stringify({
            hook_event_name: "UserPromptSubmit",
            prompt: "Explain hook loading.",
            cwd: repoRoot,
          }),
          encoding: "utf8",
        },
      );

      assert.equal(result.status, 0, result.stderr);
      assert.doesNotMatch(result.stderr, /require is not defined/u);
      assert.match(
        JSON.parse(result.stdout).hookSpecificOutput.additionalContext,
        /CODESTORY REQUEST GROUNDING ACTIVE/u,
      );
    });
  } finally {
    await rm(codexHome, { recursive: true, force: true });
  }
});

test("hook output reports model-invisible MCP instead of PATH setup guidance", async () => {
  const { spawnSync } = await import("node:child_process");
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-no-resources-"));
  const version = await readPluginVersion();
  const cliDir = join(dataDir, "codestory-cli", version);
  const cliPath = join(cliDir, process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli");
  await mkdir(cliDir, { recursive: true });
  await writeFakeCli(cliPath);
  await writeFile(
    join(cliDir, "manifest.json"),
    JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli" }),
    "utf8",
  );
  await writeFile(
    join(dataDir, ".codestory-mcp-runtime.json"),
    JSON.stringify({ source: "managed", path: cliPath }),
    "utf8",
  );
  const result = spawnSync(process.execPath, [hookPath], {
    env: {
      ...process.env,
      CODESTORY_MCP_RESOURCES_EXPOSED: "",
      COPILOT_PLUGIN_DATA: "",
      PLUGIN_DATA: dataDir,
      PATH: "",
    },
    input: JSON.stringify({
      hook_event_name: "UserPromptSubmit",
      prompt: "Explain indexing flow.",
      cwd: repoRoot,
    }),
    encoding: "utf8",
  });

  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  const context = output.hookSpecificOutput.additionalContext;
  assert.match(context, /mcp_config_installed: yes/u);
  assert.match(context, /mcp_resources_exposed: mcp_resources_not_model_visible/u);
  assert.match(context, /managed_cli_present: yes/u);
  assert.match(context, /MCP resources are not visible/u);
  assert.doesNotMatch(context, /codestory-cli ENOENT/u);
  assert.doesNotMatch(context, /attempted request packet/u);
  assert.doesNotMatch(context, /adding CodeStory to PATH/u);
  await rm(dataDir, { recursive: true, force: true });
});

test("portable agent adapters are present", async () => {
  const copilotManifest = JSON.parse(
    await readFile(join(pluginRoot, ".github", "plugin", "plugin.json"), "utf8"),
  );
  const rootCopilotInstructions = await readFile(
    join(repoRoot, ".github", "copilot-instructions.md"),
    "utf8",
  );
  const rootCursorRule = await readFile(
    join(repoRoot, ".cursor", "rules", "codestory.mdc"),
    "utf8",
  );
  const pluginCursorRule = await readFile(
    join(pluginRoot, ".cursor", "rules", "codestory.mdc"),
    "utf8",
  );
  const portability = await readFile(
    join(pluginRoot, "docs", "agent-portability.md"),
    "utf8",
  );

  assert.equal(copilotManifest.hooks, "hooks/copilot-hooks.json");
  assert.equal(copilotManifest.skills, "skills/");
  for (const text of [
    rootCopilotInstructions,
    rootCursorRule,
    pluginCursorRule,
    portability,
  ]) {
    assert.match(text, /codestory:\/\/status/u);
    assert.match(text, /retrieval_mode=full/u);
  }
});

test("plugin docs are agent-first, status-first, and marketplace-aware", async () => {
  const rootReadme = await readFile(join(repoRoot, "README.md"), "utf8");
  const readme = await readFile(join(pluginRoot, "README.md"), "utf8");
  const skill = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
    "utf8",
  );
  const doctorReference = await readFile(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "doctor.md",
    ),
    "utf8",
  );
  const serveReference = await readFile(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "serve.md",
    ),
    "utf8",
  );
  const indexReference = await readFile(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "index.md",
    ),
    "utf8",
  );
  const usage = await readFile(join(repoRoot, "docs", "usage.md"), "utf8");
  const retrievalSidecars = await readFile(
    join(repoRoot, "docs", "ops", "retrieval-sidecars.md"),
    "utf8",
  );
  const statusRuntimeRequired = [
    "codestory://status",
    "server_version",
    "cli_version",
    "server_executable",
    "server_executable_sha256",
    "sidecar_contract_version",
    "plugin_runtime",
    "runtime_truth",
    "plugin_runtime.plugin_root",
    "plugin_cache_version",
    "sidecar_setup",
    "build_source",
    "repo_ref",
    "allowed_surfaces",
  ];
  const cliRepairRequired = ["where.exe codestory-cli", "codestory-cli --version"];
  const stdioLaunchRequired = [
    "scripts/codestory-mcp.cjs",
    "sidecar_setup",
    "github_release",
    "path_fallback",
    "closing transport",
  ];
  const marketplaceSourceRequired = [
    "The marketplace catalog repo is `TheGreenCedar/AgentPluginMarketplace`",
    "plugin source at `https://github.com/TheGreenCedar/CodeStory.git`",
    "source path `plugins/codestory`",
    "The CodeStory repo does not contain the marketplace catalog",
    "git-subdir",
  ];
  const ambientHookRequired = [
    "Hosts with lifecycle-hook adapters keep CodeStory ambient",
    "With lifecycle hooks enabled, the agent should first check CodeStory\nstatus",
    "If the host does not expose lifecycle hooks yet",
    "Agent Portability",
  ];
  const restartBoundaryRequired = [
    "Codex host/app restart may",
    "fresh Codex host/app session",
  ];
  const staleCliRepairRequired = [
    "If status reports `repair_setup`",
    "The agent runs the installer command from `recommended_next_calls`",
    "If `codestory://status` reports `repair_setup`",
    "do not ask the human to install the binary",
  ];
  const perSurfaceRequired = [
    "`allowed_surfaces.<surface>.allowed`",
    "`allowed_surfaces.packet.allowed`",
    "`allowed_surfaces.search.allowed`",
    "`allowed_surfaces.context.allowed`",
    "`retrieval_mode=full`",
  ];
  const localGraphSurfaceNames = [
    "ground",
    "files",
    "symbol",
    "definition",
    "callers",
    "callees",
    "trail",
    "trace",
    "references",
    "snippet",
    "affected",
    "symbols",
    "get_node",
    "neighbors",
    "shortest_path",
    "query_subgraph",
  ];
  const sidecarSurfaceNames = ["packet", "search", "context"];
  const publicSurfaceRequired = [
    "`allowed_surfaces.<surface>.allowed` for `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph`",
    "check each surface's own `.allowed` bit",
    "`allowed_surfaces.packet.allowed`, `allowed_surfaces.search.allowed`, and `allowed_surfaces.context.allowed` with `retrieval_mode=full`",
    "`context` is not a local-only browse surface",
  ];
  const ambientScopeRequired = [
    "strict startup grounding plus request-aware packets",
    "Hooks fail open",
    "Hook output is a starting packet, not final proof",
    "skip no-op ground output in huge\nor non-code folders",
    "no repo, no supported files, or zero indexed files",
    "Do not inject, summarize, or paste empty ground\noutput as context",
    "incremental by default",
    "Use `packet`, `search`, and `context` confidently",
    "Once sidecars are installed and status reports full\n   readiness, prefer these surfaces",
    "Keep the default `auto` refresh for ordinary agent setup",
    "Use explicit `--refresh full` only",
    "ready --goal local --repair --project <target-workspace> --format json",
  ];
  const forbidden = [
    "release-bound to " + "CodeStory `v" + "0.11.1`",
    "use that version " + "unless",
    "Install plugin entry `codestory` from the external marketplace catalog:",
    "The first run should be agent-owned. The skill checks whether `codestory-cli` is\npresent and current",
  ];
  const forbiddenPatterns = [
    /\bcodestory-cli-v\d+\.\d+\.\d+/,
    /\bcodestory-cli-vX\.Y\.Z/u,
    /release-bound to CodeStory `v\d+\.\d+\.\d+`/,
  ];
  const rootReadmeRequired = [
    "Install details, binary bootstrap",
    "[plugin README](plugins/codestory/README.md)",
    "managed MCP adapter",
    "normal users should\nnot run `codestory-cli` directly",
    "Codex uses the plugin's MCP server plus the\n`@CodeStory` skill",
  ];
  for (const text of [readme, skill]) {
    for (const phrase of statusRuntimeRequired) {
      assert.equal(text.includes(phrase), true, phrase);
    }
    for (const phrase of forbidden) {
      assert.equal(text.includes(phrase), false, phrase);
    }
    for (const pattern of forbiddenPatterns) {
      assert.equal(pattern.test(text), false, String(pattern));
    }
  }
  for (const phrase of forbidden) {
    assert.equal(rootReadme.includes(phrase), false, phrase);
  }
  assert.equal(
    readme.includes("C:\\Users\\alber"),
    false,
    "public plugin README must not contain maintainer workstation paths",
  );
  for (const pattern of forbiddenPatterns) {
    assert.equal(pattern.test(rootReadme), false, String(pattern));
  }
  for (const phrase of cliRepairRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
    assert.equal(skill.includes(phrase), true, phrase);
    assert.equal(doctorReference.includes(phrase), true, phrase);
    assert.equal(serveReference.includes(phrase), true, phrase);
  }
  for (const phrase of stdioLaunchRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
    assert.equal(serveReference.includes(phrase), true, phrase);
  }
  for (const phrase of marketplaceSourceRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
  }
  for (const phrase of ambientScopeRequired) {
    assert.equal(
      readme.includes(phrase) ||
        skill.includes(phrase) ||
        indexReference.includes(phrase) ||
        doctorReference.includes(phrase) ||
        serveReference.includes(phrase),
      true,
      phrase,
    );
  }
  for (const phrase of ambientHookRequired) {
    assert.equal(readme.includes(phrase), true, phrase);
  }
  assert.equal(
    restartBoundaryRequired.some((phrase) => readme.includes(phrase)),
    true,
    "readme must mention restart boundary",
  );
  assert.equal(
    restartBoundaryRequired.some((phrase) => serveReference.includes(phrase)),
    true,
    "serve reference must mention restart boundary",
  );
  for (const phrase of statusRuntimeRequired) {
    assert.equal(serveReference.includes(phrase), true, phrase);
  }
  for (const phrase of staleCliRepairRequired) {
    assert.equal(readme.includes(phrase) || skill.includes(phrase), true, phrase);
  }
  for (const phrase of rootReadmeRequired) {
    assert.equal(rootReadme.includes(phrase), true, phrase);
  }
  for (const phrase of perSurfaceRequired) {
    assert.equal(skill.includes(phrase), true, phrase);
    assert.equal(serveReference.includes(phrase), true, phrase);
  }
  for (const surface of localGraphSurfaceNames) {
    assert.equal(readme.includes(surface), true, surface);
    assert.equal(skill.includes(surface), true, surface);
    assert.equal(serveReference.includes(surface), true, surface);
    assert.equal(usage.includes(surface), true, surface);
    assert.equal(retrievalSidecars.includes(surface), true, surface);
  }
  for (const surface of sidecarSurfaceNames) {
    assert.equal(readme.includes(`allowed_surfaces.${surface}.allowed`), true, surface);
    assert.equal(skill.includes(`allowed_surfaces.${surface}.allowed`), true, surface);
    assert.equal(serveReference.includes(`allowed_surfaces.${surface}.allowed`), true, surface);
  }
  for (const text of [usage, retrievalSidecars]) {
    for (const phrase of statusRuntimeRequired) {
      assert.equal(text.includes(phrase), true, phrase);
    }
    for (const phrase of publicSurfaceRequired) {
      assert.equal(text.includes(phrase), true, phrase);
    }
  }
});

test("canonical grounding skill ships with plugin references", async () => {
  await access(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "ground.md",
    ),
  );
  await access(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "packet.md",
    ),
  );
  await access(
    join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.ps1"),
  );
  await access(
    join(pluginRoot, "skills", "codestory-grounding", "scripts", "setup.sh"),
  );
});
