import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { normalizeV2Transcript, simulateProfileBatch } from "../lib/codestory-v2-compat-harness.mjs";

const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const fixturePath = path.join(repoRoot, "scripts", "tests", "fixtures", "codestory-v2-transcripts.json");
const pluginRoot = path.join(repoRoot, "plugins", "codestory");
const generatedCatalogPath = path.join(pluginRoot, "generated-mcp-catalog.json");
const nativeCli = path.join(repoRoot, "target", "debug", "codestory-cli");
const launcher = createRequire(import.meta.url)(path.join(pluginRoot, "scripts", "codestory-mcp.cjs"))._test;

test("v2 compatibility transcripts pin semantic fields while normalizing only declared volatility", async () => {
  const fixture = JSON.parse(await readFile(fixturePath, "utf8"));
  for (const surface of [fixture.native_v2, fixture.launcher_fail_open_v2]) {
    for (const required of [
      "initialize", "tools_list", "resources_list", "resource_templates_list", "prompts_list",
      "success", "preparing", "unavailable", "tool_error",
    ]) assert.ok(surface[required], `${surface.identity} must cover ${required}`);
  }

  const normalized = normalizeV2Transcript({
    operation_id: "operation-123",
    packet_id: "packet-456",
    created_at_epoch_ms: 123,
    duration_ms: 9,
    runtime_binary_sha256: "a".repeat(64),
    source_head: "b".repeat(40),
    semantic: { code: "codestory_preparing", retry_after_ms: 250 },
  }, fixture.normalization);
  assert.deepEqual(normalized, {
    operation_id: "<operation_id>",
    packet_id: "<packet_id>",
    created_at_epoch_ms: "<timestamp>",
    duration_ms: "<timing>",
    runtime_binary_sha256: "<runtime_binary_hash>",
    source_head: "<source_identity>",
    semantic: { code: "codestory_preparing", retry_after_ms: 250 },
  });
  assert.deepEqual(
    normalizeV2Transcript({ request_id: "request-1", publication_id: "publication-1" }, fixture.normalization),
    { request_id: "request-1", publication_id: "publication-1" },
    "only fixture-declared volatile identities may normalize",
  );
});

test("native and launcher-v2 fixtures freeze real catalog and fail-open result builders", async () => {
  const fixture = JSON.parse(await readFile(fixturePath, "utf8"));
  const catalogText = await readFile(generatedCatalogPath, "utf8");
  const catalog = JSON.parse(catalogText);
  const catalogSha256 = createHash("sha256").update(catalogText).digest("hex");
  assert.equal(fixture.native_v2.catalog_sha256, catalogSha256);
  assert.equal(fixture.launcher_fail_open_v2.catalog_sha256, catalogSha256);

  const requests = [
    { jsonrpc: "2.0", id: "initialize", method: "initialize", params: { protocolVersion: "2024-11-05", capabilities: {} } },
    { jsonrpc: "2.0", id: "tools", method: "tools/list" },
    { jsonrpc: "2.0", id: "resources", method: "resources/list" },
    { jsonrpc: "2.0", id: "templates", method: "resources/templates/list" },
    { jsonrpc: "2.0", id: "prompts", method: "prompts/list" },
  ];
  const native = spawnSync(nativeCli, ["serve", "--stdio", "--multi-project"], {
    cwd: repoRoot,
    encoding: "utf8",
    input: `${requests.map(JSON.stringify).join("\n")}\n`,
    timeout: 10_000,
  });
  assert.equal(native.status, 0, native.stderr);
  const responses = native.stdout.trim().split(/\r?\n/u).map(JSON.parse);
  assert.equal(responses[0].result.protocolVersion, fixture.native_v2.initialize.protocolVersion);
  assert.deepEqual(responses[1].result.tools, catalog.tools);
  assert.deepEqual(responses[2].result.resources, catalog.resources);
  assert.deepEqual(responses[3].result.resourceTemplates, catalog.resourceTemplates);
  assert.deepEqual(responses[4].result.prompts, catalog.prompts);
  assert.deepEqual(launcher.failOpenToolCatalog(), catalog.tools);

  const preparing = launcher.failOpenToolResult("ground", { managed_retrieval: { state: "preparing" } }, { project: repoRoot });
  const unavailable = launcher.failOpenToolResult("ground", {}, { project: repoRoot });
  const toolError = launcher.failOpenToolResult("ground", {}, {});
  assert.equal(preparing.structuredContent.code, fixture.launcher_fail_open_v2.preparing.structuredContent.code);
  assert.equal(unavailable.structuredContent.code, fixture.launcher_fail_open_v2.unavailable.structuredContent.code);
  assert.equal(toolError.structuredContent.code, fixture.launcher_fail_open_v2.tool_error.structuredContent.code);
});

test("future protocol fixtures execute revision-native result and batch contracts without enabling v2", async () => {
  const profiles = JSON.parse(await readFile(
    path.join(repoRoot, "crates", "codestory-cli", "tests", "fixtures", "mcp_protocol_profiles.json"),
    "utf8",
  )).profiles;
  const batch = [
    { jsonrpc: "2.0", id: "first", method: "tools/list" },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: "second" },
    { jsonrpc: "2.0", id: "third", method: "tools/call", params: { name: "ground" } },
  ];
  for (const profile of profiles) {
    const responses = simulateProfileBatch(profile, batch);
    if (profile.batch === "reject_invalid_request") {
      assert.deepEqual(responses, [{ jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid Request" } }]);
      continue;
    }
    assert.deepEqual(responses.map((response) => response.id), ["first", "second", "third"]);
    assert.equal(responses[1].error.code, -32600);
    assert.deepEqual(Object.keys(responses[0].result.tools[0]), profile.tool_fields);
    assert.deepEqual(Object.keys(responses[2].result), profile.result_fields);
  }
});
