import assert from "node:assert/strict";
import test from "node:test";
import { access, chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
  assert.equal(mcp.mcpServers.codestory.command, "codestory-cli");
  assert.deepEqual(mcp.mcpServers.codestory.args, [
    "serve",
    "--stdio",
    "--refresh",
    "none",
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
  await assert.rejects(
    access(join(pluginRoot, "scripts", "codestory-mcp.mjs")),
  );
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
    "server_executable",
    "allowed_surfaces",
  ];
  const cliRepairRequired = ["where.exe codestory-cli", "codestory-cli --version"];
  const stdioLaunchRequired = [
    "codestory-cli serve --stdio --refresh none",
    "agent host `PATH`",
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
