import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  normalizeV2Transcript,
  simulateProfileBatch,
  simulateProfileRequest,
  validateProfileFixture,
} from "../lib/codestory-v2-compat-harness.mjs";

const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const fixturePath = path.join(repoRoot, "scripts", "tests", "fixtures", "codestory-v2-transcripts.json");
const pluginRoot = path.join(repoRoot, "plugins", "codestory");
const generatedCatalogPath = path.join(pluginRoot, "generated-mcp-catalog.json");
const nativeCli = path.join(
  repoRoot,
  "target",
  "debug",
  process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli",
);
const launcher = createRequire(import.meta.url)(path.join(pluginRoot, "scripts", "codestory-mcp.cjs"))._test;

function materializeProjectFixture(value) {
  if (Array.isArray(value)) return value.map(materializeProjectFixture);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, materializeProjectFixture(child)]));
  }
  if (value === "<project_root>") return repoRoot;
  if (value === "<project_status_uri>") return `codestory://status?project=${encodeURIComponent(repoRoot)}`;
  return value;
}

function assertTranscriptEqual(actual, expected) {
  assert.equal(JSON.stringify(actual), JSON.stringify(expected));
}

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
    { request_id: "<request_id>", publication_id: "<publication_id>" },
    "every task-declared identity class must normalize",
  );
  assert.equal(
    JSON.stringify(normalizeV2Transcript({ semantic: "keep", retry_after_ms: 250, code: "codestory_preparing" }, fixture.normalization)),
    '{"semantic":"keep","retry_after_ms":250,"code":"codestory_preparing"}',
    "normalization and transcript comparison must preserve field order and semantic values",
  );
  const schema = { type: "object", properties: { packet_id: { type: "string" } } };
  assert.deepEqual(
    normalizeV2Transcript({ outputSchema: schema }, fixture.normalization),
    { outputSchema: schema },
    "schema property names are semantic declarations, not volatile values",
  );
  assert.throws(
    () => normalizeV2Transcript({ state: "ready" }, { volatile_classes: ["semantic_fields"] }),
    /unknown_v2_transcript_volatile_class:semantic_fields/u,
    "semantic fields cannot be added to the normalization allowlist",
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
  const normalizedNative = normalizeV2Transcript(responses, fixture.normalization);
  assertTranscriptEqual(normalizedNative[0].result, fixture.native_v2.initialize);
  assertTranscriptEqual(normalizedNative[1].result.tools, catalog.tools);
  assertTranscriptEqual(normalizedNative[2].result.resources, catalog.resources);
  assertTranscriptEqual(normalizedNative[3].result.resourceTemplates, catalog.resourceTemplates);
  assertTranscriptEqual(normalizedNative[4].result.prompts, catalog.prompts);
  assert.deepEqual(launcher.failOpenToolCatalog(), catalog.tools);

  const launcherState = await mkdtemp(path.join(os.tmpdir(), "codestory-v2-compat-launcher-"));
  try {
    const launcherProcess = spawnSync(process.execPath, [path.join(pluginRoot, "scripts", "codestory-mcp.cjs")], {
      cwd: repoRoot,
      encoding: "utf8",
      env: {
        ...process.env,
        CODESTORY_PLUGIN_DISABLE_PROVISION: "1",
        CODESTORY_PLUGIN_DATA: launcherState,
      },
      input: `${requests.map(JSON.stringify).join("\n")}\n`,
      timeout: 10_000,
    });
    assert.equal(launcherProcess.status, 0, launcherProcess.stderr);
    const launcherResponses = launcherProcess.stdout.trim().split(/\r?\n/u).map(JSON.parse);
    const normalizedLauncher = normalizeV2Transcript(launcherResponses, fixture.normalization);
    assertTranscriptEqual(normalizedLauncher[0].result, fixture.launcher_fail_open_v2.initialize);
    assertTranscriptEqual(normalizedLauncher[1].result, { tools: catalog.tools });
    assertTranscriptEqual(normalizedLauncher[2].result, { resources: catalog.resources.filter(({ uri }) => uri === "codestory://agent-guide") });
    assertTranscriptEqual(normalizedLauncher[3].result, { resourceTemplates: catalog.resourceTemplates.filter(({ uriTemplate }) => uriTemplate === "codestory://status{?project}") });
    assertTranscriptEqual(normalizedLauncher[4].result, { prompts: [] });
  } finally {
    await rm(launcherState, { recursive: true, force: true });
  }

  const success = launcher.failOpenToolResult("status", { state: "ready" }, { project: repoRoot });
  const preparing = launcher.failOpenToolResult("ground", { managed_retrieval: { state: "preparing" } }, { project: repoRoot });
  const unavailable = launcher.failOpenToolResult("ground", {}, { project: repoRoot });
  const toolError = launcher.failOpenToolResult("ground", {}, {});
  assertTranscriptEqual(
    normalizeV2Transcript(success, fixture.normalization),
    materializeProjectFixture(fixture.launcher_fail_open_v2.success),
  );
  assertTranscriptEqual(
    normalizeV2Transcript(preparing, fixture.normalization),
    materializeProjectFixture(fixture.launcher_fail_open_v2.preparing),
  );
  assertTranscriptEqual(
    normalizeV2Transcript(unavailable, fixture.normalization),
    materializeProjectFixture(fixture.launcher_fail_open_v2.unavailable),
  );
  assertTranscriptEqual(normalizeV2Transcript(toolError, fixture.normalization), fixture.launcher_fail_open_v2.tool_error);
});

test("future protocol fixtures execute revision-native result and batch contracts without enabling v2", async () => {
  const profileFixture = JSON.parse(await readFile(
    path.join(repoRoot, "crates", "codestory-cli", "tests", "fixtures", "mcp_protocol_profiles.json"),
    "utf8",
  ));
  validateProfileFixture(profileFixture);
  const profiles = profileFixture.profiles;
  assert.throws(
    () => validateProfileFixture({ ...profileFixture, profiles: [{ ...profiles[0], tool_fields: [...profiles[0].tool_fields, "safety"] }, ...profiles.slice(1)] }),
    /profile_fixture_mismatch:2024-11-05:tool_fields/u,
  );
  assert.throws(
    () => validateProfileFixture({ ...profileFixture, profiles: [{ ...profiles[0], fixture_label: "trusted" }, ...profiles.slice(1)] }),
    /profile_fixture_unknown_field:2024-11-05:fixture_label/u,
  );
  const batch = [
    { jsonrpc: "2.0", id: "first", method: "tools/list" },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: "second" },
    { jsonrpc: "2.0", id: "third", method: "tools/call", params: { name: "ground" } },
  ];
  for (const profile of profiles) {
    const toolsResponse = simulateProfileRequest(profile.revision, batch[0]);
    const callResponse = simulateProfileRequest(profile.revision, batch[3]);
    assert.deepEqual(Object.keys(toolsResponse.result.tools[0]), profile.tool_fields);
    assert.deepEqual(Object.keys(callResponse.result), profile.result_fields);
    if (profile.result_form === "structured_content_and_identical_json_text") {
      assert.deepEqual(JSON.parse(callResponse.result.content[0].text), callResponse.result.structuredContent);
    } else {
      assert.equal("structuredContent" in callResponse.result, false);
    }
    assert.deepEqual(
      simulateProfileRequest(profile.revision, { jsonrpc: "2.0", id: "unknown", method: "tools/list", extra: true }),
      { jsonrpc: "2.0", id: "unknown", error: { code: -32600, message: "Invalid Request" } },
      "the dark harness rejects request fields outside JSON-RPC",
    );

    const responses = simulateProfileBatch(profile.revision, batch);
    if (profile.batch === "reject_invalid_request") {
      assert.deepEqual(responses, [{ jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid Request" } }]);
      continue;
    }
    assert.deepEqual(responses.map((response) => response.id), ["first", "second", "third"]);
    assert.equal(responses[1].error.code, -32600);
  }
  assert.throws(() => simulateProfileBatch("future-from-fixture", batch), /unknown_dark_profile_revision/u);
});
