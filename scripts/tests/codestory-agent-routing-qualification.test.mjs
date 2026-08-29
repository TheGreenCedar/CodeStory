import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmod, cp, mkdir, mkdtemp, readFile, realpath, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { ROUTING_SCENARIOS } from "../codestory-agent-routing-conformance.mjs";
import {
  buildRoutingHostCommand,
  buildCodexPluginInstallCommand,
  authenticateSourceCandidateInstallation,
  cursorHostEnvironment,
  discoverCursorQualificationProviders,
  installCursorQualificationProvider,
  materializeRoutingFixture,
  parseRoutingQualificationOptions,
  validateRoutingPreflight,
  validateRoutingArtifactMatrix,
  verifyCursorQualificationProvider,
  verifyStagedCandidateInstallation,
  writeRoutingQualificationArtifacts,
  writeRoutingSessionCapture,
} from "../codestory-agent-routing-qualification.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");
const pluginRoot = join(repoRoot, "plugins", "codestory");

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

test("qualification host commands use official isolated Codex and Cursor session modes", () => {
  const codex = buildRoutingHostCommand({
    host: "codex",
    executable: "codex",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    installedPluginRoot: "/fixture/codex-home/plugins/codestory",
    codexHome: "/fixture/codex-home",
    model: "gpt-fixture",
    prompt: "fixture prompt",
  });
  assert.equal(codex.command, "codex");
  assert.deepEqual(codex.args, [
    "exec", "--json", "--ephemeral", "--skip-git-repo-check",
    "--config", 'approval_policy="never"',
    "--config", 'model_reasoning_effort="xhigh"',
    "--config", 'service_tier="default"',
    "--config", 'personality="pragmatic"',
    "--config", 'model_verbosity="low"',
    "--config", 'features.remote_plugin=false',
    "--config", 'mcp_servers.codestory.command="node"',
    "--config", 'mcp_servers.codestory.args=["./scripts/codestory-mcp.cjs"]',
    "--config", 'mcp_servers.codestory.cwd="/fixture/codex-home/plugins/codestory"',
    "--config", 'mcp_servers.codestory.env_vars=["CODESTORY_PLUGIN_DATA","PLUGIN_DATA","CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256","CODESTORY_EMBED_QUALIFICATION_DIR","CODESTORY_EMBED_QUALIFICATION_NONCE","CODESTORY_CACHE_ROOT","CODESTORY_STDIO_CACHE_ROOT","CODESTORY_EMBED_MODEL_SOURCE"]',
    "--config", 'mcp_servers.codestory.default_tools_approval_mode="approve"',
    "--sandbox", "workspace-write", "--cd", "/fixture/project", "--model", "gpt-fixture", "-",
  ]);
  assert.equal(codex.stdin, "fixture prompt");
  assert.equal(codex.env.CODEX_HOME, "/fixture/codex-home");
  assert.equal(codex.args.filter((value) => value.startsWith("mcp_servers.")).length, 5);

  const cursorAgent = buildRoutingHostCommand({
    host: "cursor",
    executable: "cursor-agent",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    model: "composer-2.5",
    prompt: "fixture prompt",
  });
  assert.deepEqual(cursorAgent.args, [
    "--print", "--output-format", "stream-json", "--stream-partial-output",
    "--mode", "ask", "--approve-mcps", "--trust", "--force", "--model", "composer-2.5",
    "--plugin-dir", "/fixture/plugin",
    "fixture prompt",
  ]);

  const cursor = buildRoutingHostCommand({
    host: "cursor",
    executable: "cursor",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    model: "composer-2.5-fast",
    prompt: "fixture prompt",
  });
  assert.equal(cursor.args[0], "agent");

  assert.throws(() => buildRoutingHostCommand({
    host: "cursor",
    executable: "cursor-agent",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    model: "gpt-5.6-sol-high",
    prompt: "fixture prompt",
  }), /Cursor qualification requires a Composer model/u);

  assert.throws(() => buildRoutingHostCommand({
    host: "codex",
    executable: "codex",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    codexHome: "/fixture/codex-home",
    model: "gpt-fixture",
    prompt: "fixture prompt",
  }), /Codex qualification requires the installed plugin root/u);

  assert.deepEqual(buildCodexPluginInstallCommand({
    executable: "codex", codexHome: "/fixture/codex-home", pluginRoot: "/fixture/plugin",
  }), {
    marketplaceRoot: "/fixture/codex-home/qualification-marketplace",
    marketplaceManifest: "/fixture/codex-home/qualification-marketplace/.agents/plugins/marketplace.json",
    marketplacePlugin: "/fixture/codex-home/qualification-marketplace/plugins/codestory",
    command: "codex",
    marketplaceArgs: ["plugin", "marketplace", "add", "/fixture/codex-home/qualification-marketplace", "--json"],
    pluginArgs: ["plugin", "add", "codestory@RoutingCandidate", "--json"],
    env: { CODEX_HOME: "/fixture/codex-home" },
  });
});

test("Cursor scored sessions prepare and execute against one private default cache", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-cursor-env-"));
  try {
    const home = join(root, "home");
    const state = join(root, "state");
    const env = await cursorHostEnvironment(home, state, {
      FIXTURE: "retained",
      CODESTORY_CACHE_ROOT: join(root, "wrong-cache"),
      CODESTORY_STDIO_CACHE_ROOT: join(root, "wrong-stdio-cache"),
    });
    assert.equal(env.HOME, home);
    assert.equal(env.FIXTURE, "retained");
    assert.equal(env.CODESTORY_CACHE_ROOT, undefined);
    assert.equal(env.CODESTORY_STDIO_CACHE_ROOT, undefined);
    assert.equal(env.CURSOR_CONFIG_DIR, join(state, "config"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("Cursor qualification installs one authenticated provider into both private account roots", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-cursor-provider-"));
  try {
    const home = join(root, "home");
    const revision = "candidate-revision";
    const cacheRoot = join(
      home, ".cursor", "plugins", "cache", "thegreencedar-codestory", "codestory", revision,
    );
    const cloneRoot = join(
      home, ".cursor", "plugins", "marketplaces", "github.com", "thegreencedar", "codestory",
      revision, "plugins", "codestory",
    );
    for (const providerRoot of [cacheRoot, cloneRoot]) {
      await mkdir(providerRoot, { recursive: true });
      await writeFile(join(providerRoot, "plugin.json"), '{"name":"codestory"}\n');
    }
    const candidateCli = join(root, "candidate", "codestory-cli");
    await mkdir(dirname(candidateCli), { recursive: true });
    await writeFile(candidateCli, "candidate-cli\n");
    await chmod(candidateCli, 0o700);

    assert.deepEqual(await discoverCursorQualificationProviders(home), {
      cursorHome: await realpath(home),
      cacheRoot: await realpath(cacheRoot),
      cloneRoot: await realpath(cloneRoot),
      dataDir: join(await realpath(home), ".cursor", "plugins", "data", "codestory"),
    });
    const installed = await installCursorQualificationProvider({
      cursorHome: home,
      candidatePluginRoot: pluginRoot,
      candidateCli,
      candidateCliSha256: sha256(await readFile(candidateCli)),
    });
    assert.equal(await verifyCursorQualificationProvider(installed), true);
    assert.equal((await stat(installed.localOverridePath)).mode & 0o777, 0o600);
    assert.deepEqual(JSON.parse(await readFile(installed.localOverridePath, "utf8")), {
      schema_version: 1,
      CODESTORY_CLI: await realpath(candidateCli),
    });
    const command = buildRoutingHostCommand({
      host: "cursor",
      executable: "cursor-agent",
      projectRoot: "/fixture/project",
      pluginRoot: installed.cacheRoot,
      model: "composer-2.5",
      prompt: "fixture prompt",
    });
    assert.equal(command.args[command.args.indexOf("--plugin-dir") + 1], installed.cacheRoot);

    await writeFile(join(installed.cacheRoot, "generated-mcp-catalog.json"), "drift\n");
    await assert.rejects(verifyCursorQualificationProvider(installed), /provider bytes drifted/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("Cursor qualification rejects duplicate and linked account providers", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-cursor-provider-hostile-"));
  try {
    const home = join(root, "home");
    const providerBase = join(
      home, ".cursor", "plugins", "cache", "thegreencedar-codestory", "codestory",
    );
    const cloneRoot = join(
      home, ".cursor", "plugins", "marketplaces", "github.com", "thegreencedar", "codestory",
      "one", "plugins", "codestory",
    );
    for (const providerRoot of [join(providerBase, "one"), join(providerBase, "two"), cloneRoot]) {
      await mkdir(providerRoot, { recursive: true });
      await writeFile(join(providerRoot, "plugin.json"), '{"name":"codestory"}\n');
    }
    await assert.rejects(discoverCursorQualificationProviders(home), /exactly one Cursor cache provider/u);

    await rm(join(providerBase, "two"), { recursive: true, force: true });
    await rm(join(providerBase, "one"), { recursive: true, force: true });
    const outside = join(root, "outside-provider");
    await mkdir(outside, { recursive: true });
    await writeFile(join(outside, "plugin.json"), '{"name":"codestory"}\n');
    await symlink(outside, join(providerBase, "one"));
    await assert.rejects(discoverCursorQualificationProviders(home), /symbolic link|escapes/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("qualification CLI has no synthetic transcript input", () => {
  assert.throws(
    () => parseRoutingQualificationOptions(["--transcript", "/tmp/fake.jsonl"]),
    /unknown option --transcript/u,
  );
});

test("qualification CLI accepts only the mission source candidate boundary", () => {
  assert.deepEqual(parseRoutingQualificationOptions([
    "--candidate-source-root", "/candidate/source",
  ]), {
    candidate_source_root: "/candidate/source",
  });
  for (const option of [
    "--candidate-cli", "--candidate-cli-sha256",
    "--package-receipt", "--package-receipt-sha256", "--archive", "--source-root",
  ]) {
    assert.throws(() => parseRoutingQualificationOptions([option, "/legacy"]), new RegExp(`unknown option ${option}`, "u"));
  }
});

test("qualification artifacts require exactly 32 separately validated host sessions", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-artifacts-"));
  try {
    const rows = ["codex", "cursor"].flatMap((host) => ROUTING_SCENARIOS.map(({ id }) => ({
      host,
      scenario_id: id,
      transcript: `${JSON.stringify({ host, id })}\n`,
      report: { schema_version: 1, status: "pass", host, scenario_id: id },
    })));
    assert.equal(validateRoutingArtifactMatrix(rows), true);
    const summary = await writeRoutingQualificationArtifacts(root, rows);
    assert.equal(summary.expected_sessions, 32);
    assert.equal(summary.completed_sessions, 32);
    assert.equal(summary.status, "pass");
    assert.equal(JSON.parse(await readFile(join(root, "summary.json"), "utf8")).completed_sessions, 32);
    assert.equal((await readFile(join(root, "codex", `${ROUTING_SCENARIOS[0].id}.jsonl`), "utf8")).length > 0, true);
    assert.equal((await readFile(join(root, "cursor", `${ROUTING_SCENARIOS[0].id}.report.json`), "utf8")).length > 0, true);
    assert.throws(() => validateRoutingArtifactMatrix(rows.slice(1)), /exactly 32/u);
    const duplicate = structuredClone(rows);
    duplicate[1] = structuredClone(duplicate[0]);
    assert.throws(() => validateRoutingArtifactMatrix(duplicate), /exactly once/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("qualification retains owner-only bounded host bytes before validation", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-capture-"));
  try {
    const capture = await writeRoutingSessionCapture(root, "codex", "exact_symbol_search", {
      stdout: '{"type":"turn.completed"}\n',
      stderr: "sanitized fixture stderr\n",
      code: 0,
      signal: null,
    });
    const metadata = JSON.parse(await readFile(capture.metadataPath, "utf8"));
    assert.equal(metadata.host, "codex");
    assert.equal(metadata.scenario_id, "exact_symbol_search");
    assert.equal(metadata.stdout_sha256, sha256(await readFile(capture.stdoutPath)));
    assert.equal(metadata.stderr_sha256, sha256(await readFile(capture.stderrPath)));
    for (const path of Object.values(capture)) {
      assert.equal((await stat(path)).mode & 0o777, 0o600);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("qualification static roster authenticates every linked routing reference", async () => {
  const source = await readFile(join(repoRoot, "scripts", "codestory-agent-routing-qualification.mjs"), "utf8");
  for (const reference of [
    "generated-mcp-syntax.md", "status-contract.md", "ground.md", "files.md", "affected.md",
    "packet.md", "search.md", "context.md", "symbol.md", "trail.md", "snippet.md",
  ]) {
    assert.equal(source.includes(`skills/codestory-grounding/references/${reference}`), true);
  }
});

async function sourceCandidateFixture(root, { liveIdentity = {} } = {}) {
  const sourceRoot = join(root, "source");
  await mkdir(join(sourceRoot, "plugins"), { recursive: true });
  await cp(pluginRoot, join(sourceRoot, "plugins", "codestory"), { recursive: true });
  await writeFile(join(sourceRoot, ".gitignore"), "/target/\n");
  for (const args of [
    ["init", "-q"],
    ["config", "user.email", "routing@example.invalid"],
    ["config", "user.name", "Routing Fixture"],
    ["add", "."],
    ["commit", "-qm", "fixture"],
  ]) {
    const completed = spawnSync("git", ["-C", sourceRoot, ...args]);
    assert.equal(completed.status, 0, completed.stderr?.toString());
  }
  const packageVersion = JSON.parse(await readFile(join(pluginRoot, "plugin.json"), "utf8")).version;
  const catalog = JSON.parse(await readFile(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"));
  const revision = catalog.wireContract.preferredMcpProtocolVersion;
  const sourceCommit = spawnSync("git", ["-C", sourceRoot, "rev-parse", "HEAD"], { encoding: "utf8" }).stdout.trim();
  const sourceTree = spawnSync("git", ["-C", sourceRoot, "rev-parse", "HEAD^{tree}"], { encoding: "utf8" }).stdout.trim();
  const observed = {
    version: liveIdentity.version ?? packageVersion,
    schema: liveIdentity.schema ?? 3,
    protocol: liveIdentity.protocol ?? revision,
    discoveryContracts: {
      ...catalog.wireContract.discoveryContracts,
      ...(liveIdentity.discoveryContracts ?? {}),
      ...(liveIdentity.discovery ? { [revision]: liveIdentity.discovery } : {}),
    },
  };
  const cliBytes = Buffer.from(`#!/usr/bin/env node
const frames = [];
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => frames.push(chunk));
process.stdin.on("end", () => {
  const discoveryContracts = ${JSON.stringify(observed.discoveryContracts)};
  for (const line of frames.join("").split(/\\r?\\n/u).filter(Boolean)) {
    const request = JSON.parse(line);
    const negotiated = request.params.protocolVersion;
    if (request.method === "initialize") process.stdout.write(JSON.stringify({
      jsonrpc: "2.0", id: request.id, result: {
        protocolVersion: negotiated,
        version: ${JSON.stringify(observed.version)},
        serverInfo: { name: "codestory", version: ${JSON.stringify(observed.version)} },
        capabilities: {},
        _meta: {
          codestory_publication: { schema_version: ${JSON.stringify(observed.schema)} },
          codestory_protocol: {
            negotiated,
            preferred: ${JSON.stringify(observed.protocol)},
            discovery_contract_sha256: discoveryContracts[negotiated],
          },
        },
      },
    }) + "\\n");
  }
});
`);
  const buildCalls = [];
  const buildCandidateCli = async (request) => {
    buildCalls.push(request);
    const { outputPath } = request;
    await mkdir(dirname(outputPath), { recursive: true });
    await writeFile(outputPath, cliBytes);
    await chmod(outputPath, 0o700);
  };

  const qualificationNonce = "c".repeat(64);
  return {
    candidateSourceRoot: sourceRoot,
    buildCandidateCli,
    qualificationNonce,
    stageRoot: join(root, "stage"),
    sourceCommit,
    sourceTree,
    packageVersion,
    revision,
    discovery: catalog.wireContract.discoveryContracts[revision],
    cliSha256: sha256(cliBytes),
    buildCalls,
  };
}

test("source candidate authentication builds and binds clean source, archived plugin bytes, owned CLI bytes, and live v3 identity", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-source-candidate-"));
  try {
    const fixture = await sourceCandidateFixture(root);
    const accepted = await authenticateSourceCandidateInstallation(fixture);
    assert.equal(accepted.expectedIdentity.publication.schema_version, 3);
    assert.equal(accepted.expectedIdentity.cli.sha256, fixture.cliSha256);
    assert.equal(accepted.publicIdentity.source_commit, fixture.sourceCommit);
    assert.equal(accepted.publicIdentity.source_tree, fixture.sourceTree);
    assert.equal(accepted.publicIdentity.protocol_revision, fixture.revision);
    assert.equal(accepted.publicIdentity.discovery_contract_sha256, fixture.discovery);
    assert.deepEqual(
      accepted.publicIdentity.discovery_contracts,
      JSON.parse(await readFile(join(accepted.pluginRoot, "generated-mcp-catalog.json"), "utf8")).wireContract.discoveryContracts,
    );
    assert.deepEqual(fixture.buildCalls, [{
      sourceRoot: await realpath(fixture.candidateSourceRoot),
      targetDir: join(await realpath(fixture.candidateSourceRoot), "target", "codestory-mission-candidate"),
      outputPath: join(await realpath(fixture.candidateSourceRoot), "target", "codestory-mission-candidate", "release", "codestory-cli"),
    }]);
    assert.equal(accepted.launcherEnv.CODESTORY_PLUGIN_DATA, await realpath(accepted.pluginData));
    assert.equal(JSON.parse(await readFile(accepted.staged.attestationPath, "utf8")).candidate.producer.kind, "source_candidate");

    const originalManaged = await readFile(accepted.staged.managedCli);
    await writeFile(accepted.staged.managedCli, "substituted\n");
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /managed CLI installation/u);
    await writeFile(accepted.staged.managedCli, originalManaged);

    const markerPath = join(accepted.staged.qualificationDir, "candidate-managed-install.json");
    const marker = JSON.parse(await readFile(markerPath, "utf8"));
    marker.archive_sha256 = "d".repeat(64);
    await writeFile(markerPath, JSON.stringify(marker));
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /qualification marker/u);
    marker.archive_sha256 = accepted.launcherEnv.CODESTORY_PLUGIN_CANDIDATE_ARCHIVE_SHA256;
    await writeFile(markerPath, JSON.stringify(marker));

    const attestation = JSON.parse(await readFile(accepted.staged.attestationPath, "utf8"));
    attestation.plugin.source_tree = "e".repeat(40);
    await writeFile(accepted.staged.attestationPath, JSON.stringify(attestation));
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /attestation drifted/u);

    await writeFile(join(fixture.candidateSourceRoot, "untracked.txt"), "drift\n");
    await assert.rejects(authenticateSourceCandidateInstallation({
      ...fixture, stageRoot: join(root, "stage-source-drift"),
    }), /source root is not a clean HEAD and tree/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("source candidate authentication rejects the full input-identity mismatch class", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-source-hostile-"));
  try {
    const standalone = await sourceCandidateFixture(join(root, "standalone"));
    const standalonePath = join(root, "standalone-cli");
    await standalone.buildCandidateCli({
      sourceRoot: standalone.candidateSourceRoot,
      targetDir: join(standalone.candidateSourceRoot, "target", "ignored"),
      outputPath: standalonePath,
    });
    await assert.rejects(authenticateSourceCandidateInstallation({
      ...standalone,
      buildCandidateCli: async () => standalonePath,
    }), /did not produce its owned CLI/u);

    const drifting = await sourceCandidateFixture(join(root, "drifting"));
    const buildOwnedCli = drifting.buildCandidateCli;
    drifting.buildCandidateCli = async (request) => {
      await buildOwnedCli(request);
      await writeFile(join(drifting.candidateSourceRoot, "source-drift.txt"), "drift\n");
    };
    await assert.rejects(authenticateSourceCandidateInstallation(drifting), /changed while/u);

    const profileDrift = await sourceCandidateFixture(join(root, "profile-drift"));
    const profileCatalogPath = join(
      profileDrift.candidateSourceRoot,
      "plugins",
      "codestory",
      "generated-mcp-catalog.json",
    );
    const profileCatalog = JSON.parse(await readFile(profileCatalogPath, "utf8"));
    profileCatalog.revisionProfiles["2025-06-18"].discoveryContractSha256 = "e".repeat(64);
    await writeFile(profileCatalogPath, `${JSON.stringify(profileCatalog, null, 2)}\n`);
    for (const args of [
      ["add", "plugins/codestory/generated-mcp-catalog.json"],
      ["commit", "-qm", "drift profile digest"],
    ]) {
      const completed = spawnSync("git", ["-C", profileDrift.candidateSourceRoot, ...args]);
      assert.equal(completed.status, 0, completed.stderr?.toString());
    }
    await assert.rejects(
      authenticateSourceCandidateInstallation(profileDrift),
      /2025-06-18 revision profile discovery digest/u,
    );

    for (const [name, liveIdentity, pattern] of [
      ["schema", { schema: 2 }, /live schema/u],
      ["protocol", { protocol: "2025-06-18" }, /live 2024-11-05 protocol/u],
      ["discovery", { discovery: "e".repeat(64) }, /live 2025-11-25 discovery identity/u],
      ["june-discovery", { discoveryContracts: { "2025-06-18": "e".repeat(64) } }, /live 2025-06-18 discovery identity/u],
    ]) {
      const fixture = await sourceCandidateFixture(join(root, name), { liveIdentity });
      await assert.rejects(authenticateSourceCandidateInstallation(fixture), pattern);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("routing state preflight fails closed for missing continuation fallback and proof disposition", () => {
  assert.equal(validateRoutingPreflight("packet_single_continuation", {
    kind: "complete", status: "continuation_available",
    continuation: { continuation_id: "next", gap_ids: [{ gap_id: "gap" }] },
  }), true);
  assert.throws(() => validateRoutingPreflight("packet_single_continuation", {
    kind: "complete", status: "available", continuation: null,
  }), /real continuation/u);
  assert.equal(validateRoutingPreflight("packet_named_fallback_to_source", {
    kind: "complete", status: "available", evidence: [], gaps: [{ kind: "evidence_missing" }],
  }), true);
  assert.throws(() => validateRoutingPreflight("packet_named_fallback_to_source", {
    kind: "complete", status: "available",
    evidence: [{ path: "src/fallback.rs" }], gaps: [{ kind: "evidence_missing" }],
  }), /exact fallback unresolved/u);
  assert.equal(validateRoutingPreflight("typed_proof_contract_proven", {
    kind: "complete", disposition: { kind: "contract_proven" },
  }), true);
  assert.throws(() => validateRoutingPreflight("typed_proof_contract_proven", {
    kind: "complete", disposition: { kind: "unknown" },
  }), /required proof disposition/u);
});

test("routing fixture materialization expands deterministic state per isolated scenario", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-fixture-"));
  try {
    const ordinary = await materializeRoutingFixture(
      join(repoRoot, "scripts", "fixtures", "codestory-agent-routing-project"), join(root, "ordinary"),
    );
    assert.match(await readFile(join(ordinary, "src", "catalog.rs"), "utf8"), /documented_route_0095/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
