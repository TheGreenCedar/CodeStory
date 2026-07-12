import assert from "node:assert/strict";
import test from "node:test";
import { spawn, spawnSync } from "node:child_process";
import { access, chmod, mkdir, mkdtemp, readFile, realpath, rm, stat, symlink, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { once } from "node:events";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));
const require = createRequire(import.meta.url);
const launcherTest = require(join(pluginRoot, "scripts", "codestory-mcp.cjs"))._test;
const {
  dirtyMarkerPathForProject,
  dirtyHookStatus,
  installDirtyHooks,
  uninstallDirtyHooks,
  writeDirtyMarker,
} = require(join(pluginRoot, "hooks", "codestory-runtime.cjs"));

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

async function writeFakeCli(cliPath) {
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'){if(args.includes('--wait-fresh')&&!args.includes('--repair')&&!args.includes('agent')){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}],local_refresh:{state:'fresh',reason:'already_fresh',blocks_local_surfaces:false,readiness_status:'ready',changed_file_count:0,new_file_count:0,removed_file_count:0,fatal_error_count:0}}));process.exit(0)}process.exit(9)}fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({source:process.env.CODESTORY_PLUGIN_CLI_SOURCE,path:process.env.CODESTORY_PLUGIN_CLI_PATH,sha256:process.env.CODESTORY_PLUGIN_CLI_SHA256,version:process.env.CODESTORY_PLUGIN_CLI_VERSION,pluginRoot:process.env.CODESTORY_PLUGIN_ROOT,launchCwd:process.env.CODESTORY_PLUGIN_LAUNCH_CWD,runtimeCwd:process.env.CODESTORY_PLUGIN_RUNTIME_CWD,pluginCacheVersion:process.env.CODESTORY_PLUGIN_CACHE_VERSION,repoRef:process.env.CODESTORY_PLUGIN_CLI_REPO_REF,buildSource:process.env.CODESTORY_PLUGIN_CLI_BUILD_SOURCE,archiveSha256:process.env.CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256,retention:process.env.CODESTORY_PLUGIN_CLI_RETENTION,activeStatePath:process.env.CODESTORY_PLUGIN_ACTIVE_STATE_PATH,sidecarPolicy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,sidecarEnable:process.env.CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND,sidecarRepair:process.env.CODESTORY_PLUGIN_SIDECAR_NEXT_REPAIR_COMMAND,dirtyMarkerPath:process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PATH,dirtyMarkerRoot:process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT,args}))";
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

async function writeRecordingCli(cliPath) {
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'&&process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR!=='1'&&args.includes('--wait-fresh')&&!args.includes('--repair')&&!args.includes('agent')){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}],local_refresh:{state:'fresh',reason:'already_fresh',blocks_local_surfaces:false,readiness_status:'ready',changed_file_count:0,new_file_count:0,removed_file_count:0,fatal_error_count:0}}));process.exit(0)}fs.appendFileSync(process.env.TEST_LOG,JSON.stringify({repair:process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR==='1',policy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,args})+'\\n')";
  if (process.platform === "win32") {
    await writeFile(cliPath, `@echo off\r\n"${process.execPath}" -e "${script}" -- %*\r\n`, "utf8");
    return;
  }
  await writeFile(cliPath, `#!/bin/sh\n${JSON.stringify(process.execPath)} -e ${JSON.stringify(script)} -- "$@"\n`, "utf8");
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
  assert.deepEqual(mcp.mcpServers.codestory.env, {
    CODESTORY_PLUGIN_LOCAL_REPAIR_TIMEOUT_MS: "15000",
  });
});

test("agent-facing guidance does not send agents to CLI fallback repair", async () => {
  const guidanceFiles = [
    join(pluginRoot, "hooks", "codestory-instructions.cjs"),
    join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
    join(pluginRoot, "skills", "codestory-grounding", "agents", "openai.yaml"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "status-contract.md"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "doctor.md"),
    join(pluginRoot, "skills", "codestory-grounding", "references", "serve.md"),
    join(repoRoot, "docs", "users", "troubleshooting.md"),
    join(repoRoot, "docs", "ops", "retrieval-sidecars.md"),
  ];

  for (const file of guidanceFiles) {
    const text = await readFile(file, "utf8");
    assert.doesNotMatch(text, /CLI Fallback/u, file);
    assert.doesNotMatch(text, /CLI fallback/u, file);
    assert.doesNotMatch(text, /managed CLI or local-dev CODESTORY_CLI preflight/u, file);
    assert.doesNotMatch(text, /Call `sidecar_setup`/u, file);
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
      JSON.stringify({ path: process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli", sha256, version }),
      "utf8",
    );
    const result = spawnSync(process.execPath, [launcher], {
      env: {
        PLUGIN_DATA: dataDir,
        TEST_OUT: outFile,
        TEST_CODESTORY_VERSION: version,
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
    const retention = JSON.parse(observed.retention);
    assert.deepEqual(
      retention.retained.map((entry) => entry.version),
      [version],
      JSON.stringify(retention),
    );
    assert.equal(retention.reclaimable_bytes, 0);
    assert.equal(observed.pluginRoot, pluginRoot);
    assert.equal(observed.pluginCacheVersion, "");
    assert.equal(observed.activeStatePath, undefined);
    assert.equal(observed.sidecarPolicy, "ask");
    assert.match(observed.sidecarEnable, /sidecar-policy enable/u);
    assert.match(observed.sidecarEnable, /--policy-file/u);
    assert.equal(observed.sidecarRepair, undefined);
    assert.equal(observed.dirtyMarkerRoot, undefined);
    assert.equal(observed.dirtyMarkerPath, undefined);
    assert.deepEqual(observed.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);

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
        pid: process.pid,
        token: "abandoned",
        purpose: "retention",
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
        "  projectRoot: process.env.CODESTORY_PLUGIN_PROJECT_ROOT || '',",
        "  projectRootSource: process.env.CODESTORY_PLUGIN_PROJECT_ROOT_SOURCE || '',",
        "  launchCwd: process.env.CODESTORY_PLUGIN_LAUNCH_CWD || '',",
        "  runtimeCwd: process.env.CODESTORY_PLUGIN_RUNTIME_CWD || '',",
        "  dirtyMarkerPath: process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PATH || '',",
        "  dirtyMarkerRoot: process.env.CODESTORY_PLUGIN_DIRTY_MARKER_PROJECT_ROOT || ''",
        "}) + '\\n');",
        "if (command === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (command === 'ready' && args.includes('--wait-fresh') && !args.includes('--repair') && !args.includes('agent')) {",
        "  console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: 'ready', summary: 'ready', minimum_next: [], full_repair: [] }], local_refresh: { state: 'fresh', reason: 'already_fresh', blocks_local_surfaces: false, readiness_status: 'ready', changed_file_count: 0, new_file_count: 0, removed_file_count: 0, fatal_error_count: 0 } }));",
        "  process.exit(0);",
        "}",
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
      encoding: "utf8",
      timeout: 15000,
    });

    assert.equal(result.status, 0, result.stderr);
    assert.equal(await readFile(marker, "utf8"), "serve-called");
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    const readyCalls = calls.filter((call) => call.args[0] === "ready");
    const serve = calls.find((call) => call.args[0] === "serve");
    assert.deepEqual(readyCalls, []);
    assert.ok(serve, "expected serve call");
    assert.deepEqual(serve.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);
    assert.match(serve.cwd, /runtime-cwd/u);
    assert.equal(serve.projectRoot, "");
    assert.equal(serve.projectRootSource, "");
    assert.equal(serve.launchCwd, pluginRoot);
    assert.notEqual(serve.runtimeCwd, pluginRoot);
    assert.match(serve.runtimeCwd, /runtime-cwd/u);
    assert.equal(serve.dirtyMarkerRoot, "");
    assert.equal(serve.dirtyMarkerPath, "");
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
  assert.match(transport, /project_required: pass the caller's repository root/u);
});

test("mcp launcher fails open when delegated stdio runtime exits", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-delegated-stdio-exit-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-delegated-stdio-bin-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const realRepoRoot = await realpath(repoRoot);
  const input = [
    JSON.stringify({
      jsonrpc: "2.0",
      id: "status",
      method: "resources/read",
      params: { uri: "codestory://status" },
    }),
    JSON.stringify({
      jsonrpc: "2.0",
      id: "sidecar-status",
      method: "tools/call",
      params: { name: "sidecar_setup", arguments: { action: "status" } },
    }),
  ].join("\n") + "\n";
  const cliPath = await writeNodeCli(
    binDir,
    [
      "const args = process.argv.slice(2);",
      "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
      "if (args[0] === 'serve') { process.exit(17); }",
      "process.exit(2);",
    ].join("\n"),
  );

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
    const staleCliPath = join(
      binDir,
      version,
      process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli",
    );
    await writeFile(
      join(dataDir, "sidecar-setup-policy.json"),
      JSON.stringify({
        state: "ask",
        updated_at: "2026-07-09T00:00:00.000Z",
        last_repair: {
          state: "completed",
          updated_at: "2026-07-09T00:00:00.000Z",
          project_root: realRepoRoot,
          command: `${JSON.stringify(staleCliPath)} ready --goal agent --repair`,
        },
      }),
      "utf8",
    );

    const result = spawnSync(process.execPath, [launcher], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        CODEX_THREAD_ID: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
      },
      input,
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.length, 2, result.stdout);
    const status = JSON.parse(responses[0].result.contents[0].text);
    assert.equal(status.degraded_reason, "runtime_stdio_child_exit");
    assert.equal(status.project_root, null);
    assert.equal(status.project_root_source, "request_argument");
    assert.equal(status.readiness[0].setup.probe_status, 17);
    assert.match(
      status.readiness[0].setup.probe_error,
      /codestory-cli serve --stdio exited with status 17/u,
    );
    assert.equal(status.sidecar_setup.last_repair.current, false);
    assert.equal(status.sidecar_setup.last_repair.stale_reason, "last_repair_cli_path_mismatch");
    assert.equal(responses[1].result.structuredContent.last_repair.current, false);
    assert.equal(
      responses[1].result.structuredContent.last_repair.stale_reason,
      "last_repair_cli_path_mismatch",
    );
  } finally {
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
    params: { uri: "codestory://status" },
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
    params: { uri: "codestory://status" },
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
    params: { uri: "codestory://status" },
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
    params: { uri: "codestory://status" },
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
    params: { uri: "codestory://status" },
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
      "      } else if (request.method === 'tools/call' && request.params && request.params.name === 'sidecar_setup') {",
      "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { structuredContent: { state: 'runtime-sidecar-setup' } } }) + '\\n');",
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

    const repaired = await sendRequest({
      jsonrpc: "2.0",
      id: "repair",
      method: "tools/call",
      params: { name: "sidecar_setup", arguments: { project: realRepoRoot, action: "repair" } },
    });
    assert.equal(repaired.result.structuredContent.state, "runtime-sidecar-setup");

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

test("bootstrap-status fails open when plugin-root launch lacks project state", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-bootstrap-no-project-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "recording-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "recording-codestory-cli.cmd" : "recording-codestory-cli",
  );
  const logFile = join(dataDir, "calls.jsonl");
  const marker = join(dataDir, "serve-called.txt");

  try {
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

    const result = spawnSync(process.execPath, [launcher, "bootstrap-status"], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
        TEST_OUT: marker,
      },
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    await assert.rejects(access(marker));
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((call) => call.args[0]), ["--version"]);
    const status = JSON.parse(result.stdout.trim());
    assert.equal(status.ready, false);
    assert.equal(status.degraded_reason, "project_root_unavailable");
    assert.equal(status.project_root, null);
    assert.equal(status.project_root_source, "plugin_active_state_missing");
    assert.equal(status.readiness[0].goal, "project_root");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("bootstrap-status carries Rust agent readiness into runtime truth", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-bootstrap-agent-ready-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-bootstrap-agent-ready-bin-"));
  const logFile = join(dataDir, "calls.jsonl");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliPath = await writeNodeCli(
    binDir,
    [
      "const fs = require('node:fs');",
      "const args = process.argv.slice(2);",
      "fs.appendFileSync(process.env.TEST_LOG, JSON.stringify(args) + '\\n');",
      "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
      "if (args[0] === 'ready' && args.includes('--goal') && args.includes('local')) {",
      "  console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: 'ready', summary: 'ready', minimum_next: [], full_repair: [] }], local_refresh: { state: 'fresh', reason: 'already_fresh', blocks_local_surfaces: false, readiness_status: 'ready', changed_file_count: 0, new_file_count: 0, removed_file_count: 0, fatal_error_count: 0 } }));",
      "  process.exit(0);",
      "}",
      "if (args[0] === 'ready' && args.includes('--goal') && args.includes('agent')) {",
      "  console.log(JSON.stringify({ verdicts: [{ goal: 'agent_packet_search', status: 'ready', summary: 'agent ready', minimum_next: [], full_repair: [] }], readiness_lanes: { agent_packet_search: { status: 'ready', profile: 'agent', run_id: 'shared-agent', sidecar_mode: 'full', next_command: 'codestory-cli retrieval status --profile agent --run-id shared-agent --format json' }, local_default: { status: 'repair_retrieval', profile: 'local', sidecar_mode: 'unavailable', degraded_reason: 'zoekt_unreachable' } }, readiness_broker: { schema_version: 2, project_id: 'project-123', persistence_status: 'observed' } }));",
      "  process.exit(0);",
      "}",
      "process.exit(2);",
    ].join("\n"),
  );

  try {
    const result = spawnSync(process.execPath, [launcher, "bootstrap-status", "--project", dataDir], {
      cwd: pluginRoot,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_LOG: logFile,
      },
      encoding: "utf8",
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    const status = JSON.parse(result.stdout.trim());
    assert.equal(status.ready, true);
    assert.equal(status.runtime_truth.sidecar_status_ref, "readiness_lanes.agent_packet_search");
    assert.equal(status.runtime_truth.readiness_refs.local_graph, "readiness[goal=local_navigation]");
    assert.equal(status.runtime_truth.readiness_broker_ref, "readiness_broker");
    assert.equal(Object.hasOwn(status.runtime_truth, "sidecar_status"), false);
    assert.equal(Object.hasOwn(status.runtime_truth, "readiness_lanes"), false);
    assert.equal(status.readiness_lanes.agent_packet_search.status, "ready");
    assert.deepEqual(status.readiness_broker, {
      schema_version: 2,
      project_id: "project-123",
      persistence_status: "observed",
    });
    assert.equal(status.readiness.some((verdict) => verdict.goal === "agent_packet_search" && verdict.status === "ready"), true);
    const calls = (await readFile(logFile, "utf8")).trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.deepEqual(calls.map((args) => args.slice(0, 3)), [
      ["--version"],
      ["ready", "--goal", "local"],
      ["ready", "--goal", "agent"],
    ]);
    assert.equal(calls.some((args) => args.includes("--repair")), false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
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
    assert.equal(observed.dirtyMarkerPath, undefined);
  } finally {
    await rm(codexHome, { recursive: true, force: true });
    await rm(pathDir, { recursive: true, force: true });
  }
});

test("mcp launcher blocks when managed runtime is unavailable", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
  const sourceVersion = readCargoVersion(await readFile(join(repoRoot, "crates", "codestory-cli", "Cargo.toml"), "utf8"));
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-mcp-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const input = [
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 2, method: "resources/read", params: { uri: "codestory://status" } }),
    JSON.stringify({ jsonrpc: "2.0", id: 3, method: "tools/list" }),
    JSON.stringify({ jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "sidecar_setup", arguments: { action: "status" } } }),
    JSON.stringify({ jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "sidecar_setup", arguments: { action: "repair" } } }),
    JSON.stringify({ jsonrpc: "2.0", id: 6, method: "tools/call", params: { name: "ground", arguments: {} } }),
  ].join("\n") + "\n";

  try {
    await writeFile(
      join(dataDir, "sidecar-setup-policy.json"),
      JSON.stringify({
        state: "ask",
        updated_at: "2026-07-09T00:00:00.000Z",
        last_repair: {
          state: "completed",
          updated_at: "2026-07-09T00:00:00.000Z",
          project_root: repoRoot,
          command: '"C:\\Users\\alber\\.codex\\plugins\\data\\codestory-TheGreenCedar\\codestory-cli\\0.12.3\\bin\\codestory-cli.exe" ready --goal agent --repair',
        },
      }),
      "utf8",
    );
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
    assert.equal(responses.length, 6, result.stdout);
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.plugin_runtime.plugin_version, version);
    assert.equal(status.source_checkout_version, null);
    assert.equal(status.plugin_runtime.plugin_root, pluginRoot);
    assert.equal(status.plugin_runtime.cli_source, "managed_unavailable");
    assert.equal(status.plugin_runtime.cli_path, null);
    assert.equal(status.runtime_truth.runtime_source, "managed_unavailable");
    assert.equal(status.runtime_truth.plugin_root, pluginRoot);
    assert.equal(status.runtime_truth.sidecar_policy, "ask");
    assert.equal(status.runtime_truth.sidecar_status_ref, null);
    assert.equal(status.runtime_truth.readiness_refs.local_graph, "readiness[goal=local_navigation]");
    assert.equal(status.runtime_truth.readiness_broker_ref, "readiness_broker");
    assert.equal(Object.hasOwn(status.runtime_truth.readiness_refs, "local_default"), false);
    assert.equal(Object.hasOwn(status.runtime_truth.readiness_refs, "agent_packet_search"), false);
    assert.equal(Object.hasOwn(status.runtime_truth, "sidecar_status"), false);
    assert.equal(Object.hasOwn(status.runtime_truth, "readiness_lanes"), false);
    assert.equal(status.readiness[0].status, "repair_setup");
    assert.equal(status.readiness[0].repair_reason, "managed_cli_unavailable");
    for (const key of [
      "schema_version",
      "install_id",
      "project_id",
      "canonical_root_hash",
      "workspace_root",
      "cli_version",
      "updated_at_epoch_ms",
      "snapshot_path",
      "persistence_status",
      "persistence_error",
      "operations",
      "resources",
      "reconciliation",
      "gpu_proof",
    ]) {
      assert.ok(Object.hasOwn(status.readiness_broker, key), `readiness_broker missing ${key}`);
    }
    assert.equal(status.readiness_broker.persistence_status, "unavailable");
    assert.equal(typeof status.readiness_broker.gpu_proof, "object");
    assert.notEqual(status.readiness_broker.gpu_proof, null);
    for (const key of [
      "requested",
      "requested_provider",
      "requested_device",
      "policy",
      "observed_state",
      "observation_source",
      "detected_provider",
      "detected_gpu",
      "cpu_allowed",
      "proof_status",
      "meaningful_accelerator_work_proven",
      "embed_smoke_ok",
      "embed_smoke_ms",
      "degraded_reason",
    ]) {
      assert.ok(
        Object.hasOwn(status.readiness_broker.gpu_proof, key),
        `readiness_broker.gpu_proof missing ${key}`,
      );
    }
    assert.equal(status.readiness_broker.gpu_proof.embed_smoke_ok, null);
    assert.equal(status.readiness_broker.gpu_proof.embed_smoke_ms, null);
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.equal(status.allowed_surfaces.sidecar_setup.allowed, true);
    assert.deepEqual(status.allowed_surfaces.sidecar_setup.allowed_actions, ["status", "enable", "disable", "ask"]);
    assert.deepEqual(status.allowed_surfaces.sidecar_setup.canonical_arguments, { action: "status" });
    assert.equal(status.sidecar_setup.last_repair.current, false);
    assert.match(
      status.sidecar_setup.last_repair.stale_reason,
      /last_repair_cli_version_mismatch:0\.12\.3!=/,
    );
    assert.doesNotMatch(JSON.stringify(status.recommended_next_calls), /"tool":"repair_all"/u);
    assert.match(status.readiness[0].minimum_next[0], /Refresh or reinstall the CodeStory plugin/u);
    assert.deepEqual(responses[2].result.tools.map((tool) => tool.name), ["sidecar_setup"]);
    assert.deepEqual(
      responses[2].result.tools[0].inputSchema.properties.action.enum,
      ["status", "enable", "disable", "ask"],
    );
    assert.equal(responses[3].result.structuredContent.last_repair.current, false);
    assert.match(
      responses[3].result.structuredContent.last_repair.stale_reason,
      /last_repair_cli_version_mismatch:0\.12\.3!=/,
    );
    assert.equal(responses[4].result.isError, true);
    assert.equal(
      responses[4].result.structuredContent.code,
      "repair_unavailable_diagnostic_fail_open",
    );
    assert.equal(responses[5].error.code, -32602);
    assert.match(responses[5].error.message, /grounding tools are unavailable/u);
    assert.match(responses[5].error.message, /restore a compatible stdio runtime/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher starts stdio without local or agent repair", async () => {
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
    assert.deepEqual(readyCalls, []);
    assert.equal(calls.some((call) => call.args.includes("agent")), false);
    assert.equal(calls.some((call) => call.args.includes("--repair")), false);
    assert.equal(calls.some((call) => call.sidecarRepair), false);
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
    assert.equal(status.allowed_surfaces.sidecar_setup.allowed, true);
    assert.deepEqual(status.allowed_surfaces.sidecar_setup.allowed_actions, ["status", "enable", "disable", "ask"]);
    assert.doesNotMatch(JSON.stringify(status.recommended_next_calls), /"tool":"repair_all"/u);
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
    id: "status",
    method: "resources/read",
    params: { uri: "codestory://status" },
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
    const status = JSON.parse(response.result.contents[0].text);
    assert.equal(
      status.readiness[0].repair_reason,
      "managed_cli_unspawnable",
    );
    assert.equal(
      status.plugin_runtime.plugin_version,
      version,
    );
    assert.doesNotMatch(JSON.stringify(status.recommended_next_calls), /"tool":"repair_all"/u);
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

    const repair = spawnSync(process.execPath, [launcher, "sidecar-policy", "repair"], {
      env: { PLUGIN_DATA: dataDir },
      encoding: "utf8",
    });
    assert.equal(repair.status, 0, repair.stderr);
    assert.equal(JSON.parse(repair.stdout).state, "enabled");

    const policy = JSON.parse(
      await readFile(join(dataDir, "sidecar-setup-policy.json"), "utf8"),
    );
    assert.equal(policy.state, "enabled");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("enabled sidecar policy defers repair until after MCP startup", async () => {
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
    assert.equal(repairCalls.length, 0, text);
    const serveCall = calls.find((call) => {
      return JSON.stringify(call.args) === JSON.stringify([
        "serve",
        "--stdio",
        "--multi-project",
        "--refresh",
        "none",
      ]);
    });
    assert.ok(serveCall, text);
    assert.equal(serveCall.policy, "enabled");
    const policy = JSON.parse(await readFile(join(dataDir, "sidecar-setup-policy.json"), "utf8"));
    assert.equal(policy.state, "enabled");
    assert.equal(policy.last_repair, undefined);
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
        TEST_CODESTORY_VERSION: version,
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
    assert.deepEqual(observed.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);

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
    assert.match(context, /CODESTORY SESSION GROUNDING ACTIVE/u);
    assert.match(context, /CodeStory MCP startup path/u);
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

test("mcp launcher keeps managed provision failures primary", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPluginVersion();
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
    assert.equal(status.plugin_runtime.cli_source, "managed_unavailable");
    assert.equal(
      status.plugin_runtime.warnings.includes("managed_cli_unavailable"),
      true,
    );
    assert.match(status.readiness[0].minimum_next[0], /^Restart\/reload/u);
    assert.equal(
      status.readiness[0].minimum_next.some((step) => {
        return /codestory-cli --version/u.test(step);
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

test("hook emits MCP activation guidance without running CLI", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-mcp-guidance-"));
  const binDir = await mkdtemp(join(tmpdir(), "codestory-hook-unused-cli-"));
  const marker = join(dataDir, "cli-called.txt");

  try {
    const cliPath = await writeNodeCli(binDir, "require(\"fs\").writeFileSync(process.env.TEST_MARKER, process.argv.slice(2).join(\" \"));");
    const output = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RefreshMode defined?",
      cwd: repoRoot,
    }, {
      CODESTORY_CLI: cliPath,
      PLUGIN_DATA: dataDir,
      TEST_MARKER: marker,
      PATH: "",
    });

    const context = output.hookSpecificOutput.additionalContext;
    assert.equal(output.systemMessage, "CODESTORY:BACKGROUND");
    assert.match(context, /CODESTORY REQUEST GROUNDING ACTIVE/u);
    assert.match(context, /CodeStory MCP startup path/u);
    assert.match(context, /codestory mcp ground status packet search/u);
    assert.match(context, /Do not treat hook text as grounding evidence/u);
    assert.doesNotMatch(context, /HOOK MCP BRIDGE/u);
    assert.doesNotMatch(context, /managed_bootstrap/u);
    assert.doesNotMatch(context, /packet ok/u);
    await assert.rejects(readFile(marker, "utf8"), /ENOENT/u);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(binDir, { recursive: true, force: true });
  }
});

test("hook dedupes repeated request prompts without storing prompt text", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-hook-prompt-dedupe-"));

  try {
    const first = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RefreshMode defined?",
      cwd: repoRoot,
    }, { PLUGIN_DATA: dataDir, PATH: "" });
    const second = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is RefreshMode defined?",
      cwd: repoRoot,
    }, { PLUGIN_DATA: dataDir, PATH: "" });
    const third = runCodexHook({
      hook_event_name: "UserPromptSubmit",
      prompt: "Where is strict_sidecar_status defined?",
      cwd: repoRoot,
    }, { PLUGIN_DATA: dataDir, PATH: "" });

    assert.match(first.hookSpecificOutput.additionalContext, /Where is RefreshMode defined?/u);
    assert.equal(Object.hasOwn(second, "hookSpecificOutput"), false);
    assert.match(third.hookSpecificOutput.additionalContext, /Where is strict_sidecar_status defined?/u);
    const stateText = await readFile(join(dataDir, ".codestory-hook-output-state.json"), "utf8");
    const promptHash = createHash("sha256")
      .update("Where is RefreshMode defined?")
      .digest("hex")
      .slice(0, 16);
    assert.match(stateText, new RegExp(promptHash, "u"));
    assert.doesNotMatch(stateText, /Where is RefreshMode defined/u);
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
