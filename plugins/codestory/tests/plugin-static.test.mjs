import assert from "node:assert/strict";
import test from "node:test";
import { access, chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, join, delimiter } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));

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
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}]}));process.exit(0)}fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({source:process.env.CODESTORY_PLUGIN_CLI_SOURCE,path:process.env.CODESTORY_PLUGIN_CLI_PATH,sha256:process.env.CODESTORY_PLUGIN_CLI_SHA256,version:process.env.CODESTORY_PLUGIN_CLI_VERSION,pluginRoot:process.env.CODESTORY_PLUGIN_ROOT,pluginCacheVersion:process.env.CODESTORY_PLUGIN_CACHE_VERSION,repoRef:process.env.CODESTORY_PLUGIN_CLI_REPO_REF,buildSource:process.env.CODESTORY_PLUGIN_CLI_BUILD_SOURCE,archiveSha256:process.env.CODESTORY_PLUGIN_CLI_ARCHIVE_SHA256,sidecarPolicy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,sidecarEnable:process.env.CODESTORY_PLUGIN_SIDECAR_ENABLE_COMMAND,args}))";
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
  const script = "const fs=require('fs');const args=process.argv.slice(1);if(args[0]==='--version'){console.log('codestory-cli '+(process.env.CODESTORY_PLUGIN_CLI_VERSION||process.env.TEST_CODESTORY_VERSION||'0.0.0'));process.exit(0)}if(args[0]==='ready'&&process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR!=='1'){console.log(JSON.stringify({verdicts:[{goal:'local_navigation',status:'ready',summary:'ready',minimum_next:[],full_repair:[]}]}));process.exit(0)}fs.appendFileSync(process.env.TEST_LOG,JSON.stringify({repair:process.env.CODESTORY_PLUGIN_SIDECAR_REPAIR==='1',policy:process.env.CODESTORY_PLUGIN_SIDECAR_POLICY_STATE,args})+'\\n')";
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

test("mcp launcher fails open when local navigation is not ready", async () => {
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
        "  console.log(JSON.stringify({ verdicts: [{ goal: 'local_navigation', status: 'repair_index', summary: 'No indexed symbols are available yet.', minimum_next: ['codestory-cli ready --goal local --repair --project \"fixture\" --format json'], full_repair: ['codestory-cli doctor --project \"fixture\"'] }] }));",
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
      timeout: 5000,
    });

    assert.equal(result.status, 0, result.stderr);
    await assert.rejects(access(marker));
    const responses = result.stdout.trim().split(/\r?\n/u).map((line) => JSON.parse(line));
    assert.equal(responses.length, 3, result.stdout);
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.plugin_runtime.cli_source, "local_dev_override");
    assert.equal(status.readiness[0].status, "repair_index");
    assert.equal(status.readiness[0].repair_reason, "local_navigation_repair_index");
    assert.equal(status.allowed_surfaces.ground.allowed, false);
    assert.equal(status.allowed_surfaces.ground.status, "repair_index");
    assert.match(status.readiness[0].minimum_next[0], /ready --goal local --repair/u);
    assert.equal(responses[2].result.isError, true);
    assert.equal(responses[2].result.structuredContent.status, "repair_index");
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

test("enabled sidecar policy schedules existing agent repair path", async () => {
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

    for (let attempt = 0; attempt < 20; attempt += 1) {
      const text = await readFile(logFile, "utf8").catch(() => "");
      const calls = text.trim().split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
      if (calls.some((call) => call.repair)) {
        assert.ok(calls.some((call) => call.args.includes("serve")), text);
        const repair = calls.find((call) => call.repair);
        assert.deepEqual(repair.args.slice(0, 4), ["ready", "--goal", "agent", "--repair"]);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    assert.fail(`repair call was not recorded:\n${await readFile(logFile, "utf8").catch(() => "")}`);
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

test("hook output keeps CodeStory ambient and attempts request-aware grounding", async () => {
  const { spawnSync } = await import("node:child_process");
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");

  await withFakeCodeStoryCli(async (binDir) => {
    const fakeCli = process.platform === "win32"
      ? join(binDir, "codestory-cli.cmd")
      : join(binDir, "codestory-cli");
    const env = {
      ...process.env,
      CODESTORY_CLI: fakeCli,
      COPILOT_PLUGIN_DATA: "",
      PLUGIN_DATA: join(repoRoot, ".tmp-plugin-data"),
      PATH: `${binDir}${delimiter}${process.env.PATH || ""}`,
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
    assert.equal(sessionOutput.systemMessage, "CODESTORY:BACKGROUND");
    assert.match(
      sessionOutput.hookSpecificOutput.additionalContext,
      /CODESTORY SESSION GROUNDING ACTIVE \(startup\)/u,
    );
    assert.match(
      sessionOutput.hookSpecificOutput.additionalContext,
      /Before manually opening source files/u,
    );
    assert.match(
      sessionOutput.hookSpecificOutput.additionalContext,
      /FAKE_CODESTORY_CLI ground --project/u,
    );

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
    assert.equal(promptOutput.hookSpecificOutput.hookEventName, "UserPromptSubmit");
    assert.match(
      promptOutput.hookSpecificOutput.additionalContext,
      /Prompt: Where is RefreshMode defined\?/u,
    );
    assert.match(
      promptOutput.hookSpecificOutput.additionalContext,
      /FAKE_CODESTORY_CLI packet --project/u,
    );
    assert.match(
      promptOutput.hookSpecificOutput.additionalContext,
      /--question Where is RefreshMode defined\?/u,
    );
  });
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

test("hook output fails open when runtime grounding is unavailable", async () => {
  const { spawnSync } = await import("node:child_process");
  const hookPath = join(pluginRoot, "hooks", "codestory-activate.cjs");
  const result = spawnSync(process.execPath, [hookPath], {
    env: {
      ...process.env,
      COPILOT_PLUGIN_DATA: "",
      PLUGIN_DATA: join(repoRoot, ".tmp-plugin-data"),
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
  assert.match(
    output.hookSpecificOutput.additionalContext,
    /attempted request packet but did not receive usable output/u,
  );
  assert.match(
    output.hookSpecificOutput.additionalContext,
    /codestory:\/\/status/u,
  );
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
    "plugin_runtime.plugin_root",
    "plugin_cache_version",
    "sidecar_setup",
    "build_source",
    "repo_ref",
    "allowed_surfaces",
  ];
  const cliRepairRequired = ["where.exe codestory-cli", "codestory-cli --version"];
  const stdioLaunchRequired = [
    "codestory-cli serve --stdio --refresh none",
    "scripts/codestory-mcp.cjs",
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
    "trail",
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
    "`allowed_surfaces.<surface>.allowed` for `ground`, `files`, `symbol`, `definition`, `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph`",
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
    "`codestory-cli serve --stdio --refresh none`",
    "managed MCP adapter",
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
