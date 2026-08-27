import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { ROUTING_SCENARIOS } from "../codestory-agent-routing-conformance.mjs";
import {
  buildRoutingHostCommand,
  buildCodexPluginInstallCommand,
  authenticateSplitCandidateInstallation,
  materializeRoutingFixture,
  parseRoutingQualificationOptions,
  validateRoutingPreflight,
  validateRoutingArtifactMatrix,
  verifyStagedCandidateInstallation,
  writeRoutingQualificationArtifacts,
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
    codexHome: "/fixture/codex-home",
    model: "gpt-fixture",
    prompt: "fixture prompt",
  });
  assert.equal(codex.command, "codex");
  assert.deepEqual(codex.args, [
    "exec", "--json", "--ephemeral",
    "--config", 'approval_policy="never"',
    "--config", 'model_reasoning_effort="xhigh"',
    "--config", 'service_tier="default"',
    "--config", 'personality="pragmatic"',
    "--config", 'model_verbosity="low"',
    "--sandbox", "workspace-write", "--cd", "/fixture/project", "--model", "gpt-fixture", "-",
  ]);
  assert.equal(codex.stdin, "fixture prompt");
  assert.equal(codex.env.CODEX_HOME, "/fixture/codex-home");

  const cursorAgent = buildRoutingHostCommand({
    host: "cursor",
    executable: "cursor-agent",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    model: "cursor-fixture",
    prompt: "fixture prompt",
  });
  assert.deepEqual(cursorAgent.args, [
    "--print", "--output-format", "stream-json", "--stream-partial-output",
    "--mode", "ask", "--approve-mcps", "--trust", "--model", "cursor-fixture",
    "--plugin-dir", "/fixture/plugin",
    "fixture prompt",
  ]);

  const cursor = buildRoutingHostCommand({
    host: "cursor",
    executable: "cursor",
    projectRoot: "/fixture/project",
    pluginRoot: "/fixture/plugin",
    model: "cursor-fixture",
    prompt: "fixture prompt",
  });
  assert.equal(cursor.args[0], "agent");

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

test("qualification CLI has no synthetic transcript input", () => {
  assert.throws(
    () => parseRoutingQualificationOptions(["--transcript", "/tmp/fake.jsonl"]),
    /unknown option --transcript/u,
  );
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

async function splitCandidateFixture(root) {
  const sourceRoot = join(root, "source");
  await mkdir(join(sourceRoot, "plugins"), { recursive: true });
  await cp(pluginRoot, join(sourceRoot, "plugins", "codestory"), { recursive: true });
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
  const assetTarget = "macos-arm64";
  const cliBytes = Buffer.from("#!/bin/sh\nexit 0\n");
  const cliSha256 = sha256(cliBytes);
  const packageName = `codestory-cli-v${packageVersion}-${assetTarget}`;
  const packageRoot = join(root, "archive-build", packageName);
  await mkdir(packageRoot, { recursive: true });
  await writeFile(join(packageRoot, "codestory-cli"), cliBytes);
  await writeFile(join(packageRoot, "codestory-native-manifest.json"), JSON.stringify({
    release_version: packageVersion,
    asset_target: assetTarget,
    source: { commit: sourceCommit, tree: sourceTree, tracked_dirty: false },
    binary: { name: "codestory-cli", sha256: cliSha256 },
  }));
  const archivePath = join(root, `${packageName}.tar.gz`);
  const tar = spawnSync("tar", ["-czf", archivePath, "-C", join(root, "archive-build"), packageName]);
  assert.equal(tar.status, 0, tar.stderr?.toString());
  const archiveBytes = await readFile(archivePath);
  const archiveSha256 = sha256(archiveBytes);

  const receipt = {
    contract: "codestory.agent-benchmark-package/v2",
    arm: "candidate_0_18",
    package_version: packageVersion,
    archive_path: `${packageName}.tar.gz`,
    archive_sha256: archiveSha256,
    source_commit: sourceCommit,
    source_tree: sourceTree,
    schema_version: 3,
    protocol_revision: revision,
    discovery_contract_sha256: catalog.wireContract.discoveryContracts[revision],
  };
  const packageReceiptPath = join(root, "package-receipt.json");
  await writeFile(packageReceiptPath, `${JSON.stringify(receipt, null, 2)}\n`);

  const qualificationNonce = "c".repeat(64);
  return {
    packageReceiptPath,
    packageReceiptSha256: sha256(await readFile(packageReceiptPath)),
    archivePath,
    sourceRoot,
    qualificationNonce,
    stageRoot: join(root, "stage"),
  };
}

test("split candidate authentication stages source plugin archive CLI attestation and marker itself", async () => {
  const root = await mkdtemp(join(tmpdir(), "codestory-routing-split-"));
  try {
    const fixture = await splitCandidateFixture(root);
    const accepted = await authenticateSplitCandidateInstallation(fixture);
    assert.equal(accepted.expectedIdentity.publication.schema_version, 3);
    assert.equal(accepted.expectedIdentity.cli.sha256, sha256(Buffer.from("#!/bin/sh\nexit 0\n")));
    assert.equal(accepted.launcherEnv.CODESTORY_PLUGIN_DATA, await realpath(accepted.pluginData));
    assert.equal(JSON.parse(await readFile(accepted.staged.attestationPath, "utf8")).candidate.producer.kind, "local_candidate");

    const originalManaged = await readFile(accepted.staged.managedCli);
    await writeFile(accepted.staged.managedCli, "substituted\n");
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /managed CLI installation/u);
    await writeFile(accepted.staged.managedCli, originalManaged);

    const markerPath = join(accepted.staged.qualificationDir, "candidate-managed-install.json");
    const marker = JSON.parse(await readFile(markerPath, "utf8"));
    marker.archive_sha256 = "d".repeat(64);
    await writeFile(markerPath, JSON.stringify(marker));
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /qualification marker/u);

    const attestation = JSON.parse(await readFile(accepted.staged.attestationPath, "utf8"));
    attestation.plugin.source_tree = "e".repeat(40);
    await writeFile(accepted.staged.attestationPath, JSON.stringify(attestation));
    await assert.rejects(verifyStagedCandidateInstallation(accepted.staged), /attestation drifted/u);

    await writeFile(join(fixture.sourceRoot, "untracked.txt"), "drift\n");
    await assert.rejects(authenticateSplitCandidateInstallation({
      ...fixture, stageRoot: join(root, "stage-source-drift"),
    }), /not the clean receipt commit and tree/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("routing state preflight fails closed for missing continuation disposition and budget fallback", () => {
  assert.equal(validateRoutingPreflight("packet_single_continuation", {
    kind: "complete", status: "continuation_available",
    continuation: { continuation_id: "next", gap_ids: [{ gap_id: "gap" }] },
  }), true);
  assert.throws(() => validateRoutingPreflight("packet_single_continuation", {
    kind: "complete", status: "available", continuation: null,
  }), /real continuation/u);
  assert.equal(validateRoutingPreflight("packet_unavailable_to_source", {
    kind: "budget_exceeded", status: "unavailable", gaps: [{ kind: "output_budget_exceeded" }],
  }), true);
  assert.throws(() => validateRoutingPreflight("packet_unavailable_to_source", {
    kind: "complete", status: "unavailable", gaps: [{ kind: "retrieval_unavailable" }],
  }), /16 KiB output budget fallback/u);
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
    const oversized = await materializeRoutingFixture(
      join(repoRoot, "scripts", "fixtures", "codestory-agent-routing-project"), join(root, "oversized"), { oversized: true },
    );
    assert.match(await readFile(join(ordinary, "src", "catalog.rs"), "utf8"), /documented_route_0095/u);
    const oversizedFiles = (await readdir(join(oversized, "src"), { recursive: true }))
      .filter((path) => path.includes("oversized_routing_catalog_evidence_"));
    assert.equal(oversizedFiles.length, 32);
    assert.equal(Math.max(...oversizedFiles.map((path) => Buffer.byteLength(path))) > 800, true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
