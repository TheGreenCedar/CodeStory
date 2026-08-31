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
import {
  RELEASE_MANIFEST_ASSET,
  RELEASE_MANIFEST_DOMAIN,
  RELEASE_MANIFEST_SCHEMA_VERSION,
  buildReleaseManifest,
} from "../../../scripts/lib/release-manifest.mjs";

const pluginRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = dirname(dirname(pluginRoot));
const require = createRequire(import.meta.url);
const launcherTest = require(join(pluginRoot, "scripts", "codestory-mcp.cjs"))._test;
const devCliContract = require(join(pluginRoot, "scripts", "codestory-dev-cli-contract.cjs"));
const generatedCatalog = JSON.parse(
  fs.readFileSync(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"),
);
const preferredRevision = generatedCatalog.wireContract.preferredMcpProtocolVersion;
const discoveryDigest = (revision = preferredRevision) =>
  generatedCatalog.wireContract.discoveryContracts[revision];
const toolTextJson = (response) => JSON.parse(response.result.content[0].text);
const statusUri = launcherTest.projectBoundResourceUri("codestory://status", repoRoot);
const {
  confirmedCursorIdentity: confirmedCursorHookIdentity,
  dirtyMarkerPathForProject,
  inferredCursorPluginDataDir: inferredCursorHookDataDir,
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
  assert.deepEqual(
    launcherTest.releaseAssetIdentity("0.16.0", "linux", "x64"),
    {
      target: "linux-x64",
      asset: "codestory-cli-v0.16.0-linux-x64.tar.gz",
    },
  );
  for (const [platform, architecture] of [
    ["darwin", "x64"],
    ["win32", "arm64"],
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
  for (const [revision, profile] of Object.entries(catalog.revisionProfiles)) {
    assert.equal(profile.tools.length, 21, `${revision} must advertise exactly 21 tools`);
    assert.equal(
      profile.tools.filter(({ name }) => name === "verify_indexed_direct_calls").length,
      1,
      `${revision} must advertise verify_indexed_direct_calls exactly once`,
    );
    assert.equal(
      profile.tools.filter(({ name }) => name === "prove_call_path").length,
      0,
      `${revision} must not advertise legacy prove_call_path in the public catalog`,
    );
  }
  assert.equal(catalog.tools.length, 21);
  assert.equal(catalog.tools.filter(({ name }) => name === "verify_indexed_direct_calls").length, 1);
  assert.equal(catalog.tools.filter(({ name }) => name === "prove_call_path").length, 0);
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
    ["choose", "context", "end_line", "file_path", "function_body", "id", "line", "lines", "path", "paths", "project", "query", "scope", "start_line", "symbol_id"],
  );
});

test("launcher wire contract matches the generated catalog read from the real CLI", async () => {
  // The launcher must not read the catalog at run time to find its skew
  // detector, so the constants are mirrored. This is the pin that keeps the
  // mirror honest: the catalog half is generated from the real binary.
  const catalog = JSON.parse(await readFile(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"));
  assert.deepEqual(catalog.wireContract, {
    publicationStampSchemaVersion: launcherTest.publicationStampSchemaVersion,
    minimumCompatiblePublicationStampSchemaVersion:
      launcherTest.minimumCompatiblePublicationStampSchemaVersion,
    supportedMcpProtocolVersions: [...launcherTest.supportedMcpProtocolVersions],
    preferredMcpProtocolVersion: launcherTest.managedCliMcpProtocolVersion,
    discoveryContracts: generatedCatalog.wireContract.discoveryContracts,
  });
  assert.equal(catalog.wireContract.publicationStampSchemaVersion, 3);
  assert.deepEqual(
    catalog.wireContract.supportedMcpProtocolVersions,
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
  );
});

test("launcher negotiates the MCP protocol revision instead of echoing it", () => {
  // CR-064: echoing an unimplemented revision hands the host a compatibility
  // claim nothing behind the launcher honours.
  assert.deepEqual(launcherTest.negotiateMcpProtocolVersion("2024-11-05"), {
    requested: "2024-11-05",
    negotiated: "2024-11-05",
    supported: ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
    preferred: preferredRevision,
    status: "agreed",
    compatible: true,
    discovery_contract_sha256: discoveryDigest("2024-11-05"),
  });
  assert.deepEqual(launcherTest.negotiateMcpProtocolVersion("2025-03-26"), {
    requested: "2025-03-26",
    negotiated: "2025-03-26",
    supported: ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
    preferred: preferredRevision,
    status: "agreed",
    compatible: true,
    discovery_contract_sha256: discoveryDigest("2025-03-26"),
  });
  for (const absent of [undefined, null, "", "   ", 7]) {
    assert.deepEqual(
      launcherTest.negotiateMcpProtocolVersion(absent),
      {
        requested: null,
        negotiated: preferredRevision,
        supported: ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
        preferred: preferredRevision,
        status: "defaulted",
        compatible: true,
        discovery_contract_sha256: discoveryDigest(),
      },
      `absent revision ${JSON.stringify(absent)} must default without claiming a client revision`,
    );
  }
});

test("launcher classifies the runtime initialize contract fail-closed", () => {
  const stamped = (stamp) => ({
    jsonrpc: "2.0",
    id: "initialize",
    result: {
      protocolVersion: "2024-11-05",
      _meta: { codestory_publication: stamp },
    },
  });

  assert.equal(
    launcherTest.runtimeWireContractSkew(
      stamped({ schema_version: 3, minimum_compatible_schema_version: 3 }),
      "2024-11-05",
    ),
    null,
  );
  assert.equal(
    launcherTest.runtimeWireContractSkew(
      { jsonrpc: "2.0", id: "initialize", result: { protocolVersion: "2024-11-05" } },
      "2024-11-05",
    ),
    "publication_stamp_legacy_v0",
    "a runtime that predates the stamp is legacy v0, not silently current",
  );
  assert.equal(
    launcherTest.runtimeWireContractSkew(stamped({ schema_version: 1 }), "2024-11-05"),
    "publication_stamp_producer_too_old",
  );
  assert.equal(
    launcherTest.runtimeWireContractSkew(
      stamped({ schema_version: 3, minimum_compatible_schema_version: 3 }),
      "2025-03-26",
    ),
    "protocol_version_skew",
    "the runtime must speak the revision the launcher already promised the host",
  );
  assert.equal(
    launcherTest.runtimeWireContractSkew(
      { jsonrpc: "2.0", id: "initialize", error: { code: -32600, message: "no" } },
      "2024-11-05",
    ),
    "initialize_rejected",
  );
  assert.equal(launcherTest.runtimeWireContractSkew("not-json-rpc", "2024-11-05"), "initialize_response_invalid");
  assert.equal(
    launcherTest.runtimeWireContractSkew({ jsonrpc: "2.0", id: "initialize" }, "2024-11-05"),
    "initialize_result_invalid",
  );
});

test("v3 launcher state rejects old new and wrong-v3 runtime identities", () => {
  const contracts = Object.fromEntries(
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"].map((revision, index) => [
      revision,
      String(index + 1).repeat(64),
    ]),
  );
  const session = launcherTest.v3LauncherSession("2025-06-18", contracts);
  assert.deepEqual(session, {
    requested: "2025-06-18",
    negotiated: "2025-06-18",
    discoveryContractSha256: contracts["2025-06-18"],
    publicationSchemaVersion: 3,
  });
  const response = (revision, digest, schemaVersion = 3) => ({
    jsonrpc: "2.0",
    id: "initialize",
    result: {
      protocolVersion: revision,
      _meta: {
        codestory_protocol: { discovery_contract_sha256: digest },
        codestory_publication: { schema_version: schemaVersion },
      },
    },
  });
  assert.equal(
    launcherTest.v3RuntimeWireContractSkew(
      response(session.negotiated, session.discoveryContractSha256),
      session,
    ),
    null,
  );
  assert.equal(
    launcherTest.v3RuntimeWireContractSkew(
      response("2024-11-05", contracts["2024-11-05"]),
      session,
    ),
    "protocol_version_skew",
  );
  assert.equal(
    launcherTest.v3RuntimeWireContractSkew(
      response(session.negotiated, "f".repeat(64)),
      session,
    ),
    "discovery_contract_skew",
  );
  assert.equal(
    launcherTest.v3RuntimeWireContractSkew(
      response(session.negotiated, session.discoveryContractSha256, 4),
      session,
    ),
    "publication_schema_skew",
  );
  assert.deepEqual(
    launcherTest.supportedMcpProtocolVersions,
    ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
  );
});

test("fail-open relay bounds hostile frames and survives null input and a missing catalog", async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const status = {
    plugin_runtime: { plugin_version: "0.16.3", warnings: [] },
    runtime: { state: "unavailable" },
    warnings: [],
    readiness: [{
      goal: "runtime",
      status: "unavailable",
      summary: "fixture unavailable",
      reason: "runtime_unavailable",
      setup: {},
    }],
    managed_retrieval: { state: "unavailable", automatic: true },
    degraded_reason: "runtime_unavailable",
  };
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    `run(${JSON.stringify(status)},{catalog:null});`,
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.stdin.end([
    "null",
    JSON.stringify({
      jsonrpc: "2.0",
      id: "null-arguments",
      method: "tools/call",
      params: { name: "status", arguments: null },
    }),
    JSON.stringify({ jsonrpc: "2.0", id: "catalog", method: "tools/list" }),
    JSON.stringify({
      jsonrpc: "2.0",
      id: "catalog-status",
      method: "resources/read",
      params: { uri: statusUri },
    }),
    "x".repeat(launcherTest.failOpenMaxFrameBytes + 1),
    JSON.stringify({ jsonrpc: "2.0", id: "after-oversized", method: "initialize" }),
    "",
  ].join("\n"));
  const [exitCode] = await once(child, "close");
  assert.equal(exitCode, 0, stderr);
  const responses = stdout.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
  assert.equal(responses.find((response) => response.error?.code === -32600)?.id, null);
  assert.equal(
    JSON.parse(
      responses.find((response) => response.id === "null-arguments")?.result.content[0].text,
    ).code,
    "project_required",
  );
  assert.deepEqual(
    responses.find((response) => response.id === "catalog")?.result.tools.map(({ name }) => name),
    ["status"],
  );
  const catalogStatus = JSON.parse(
    responses.find((response) => response.id === "catalog-status")?.result.contents[0].text,
  );
  assert.equal(catalogStatus.degraded_reason, "generated_mcp_catalog_missing");
  const oversized = responses.find((response) => response.error?.data?.code === "stdio_frame_too_large");
  assert.equal(oversized.error.data.max_frame_bytes, 1024 * 1024);
  assert.ok(oversized.error.data.line_bytes > oversized.error.data.max_frame_bytes);
  assert.equal(responses.find((response) => response.id === "after-oversized")?.error.code, -32603);
});

test("fail-open relay applies revision-native JSON-RPC batch rules", async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const status = {
    plugin_runtime: { plugin_version: "0.17.4", warnings: [] },
    runtime: { state: "unavailable" },
    warnings: [],
    readiness: [],
    managed_retrieval: { state: "unavailable", automatic: true },
    degraded_reason: "runtime_unavailable",
  };
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    `run(${JSON.stringify(status)});`,
  ].join("");
  const batch = [
    { jsonrpc: "2.0", id: "tools", method: "tools/list" },
    { jsonrpc: "2.0", method: "notifications/cancelled" },
    { jsonrpc: "2.0", id: "resources", method: "resources/list" },
  ];

  for (const revision of ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]) {
    const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.stdin.end([
      JSON.stringify({
        jsonrpc: "2.0",
        id: "initialize",
        method: "initialize",
        params: { protocolVersion: revision },
      }),
      JSON.stringify({
        jsonrpc: "2.0",
        id: "retired-packet-argument",
        method: "tools/call",
        params: {
          name: "packet",
          arguments: { project: "/tmp/repo", question: "why", include_evidence: true },
        },
      }),
      JSON.stringify(batch),
      "",
    ].join("\n"));
    const [exitCode] = await once(child, "close");
    assert.equal(exitCode, 0, stderr);
    const frames = stdout.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
    assert.equal(frames[0].result.protocolVersion, revision);
    assert.equal(frames[1].id, "retired-packet-argument");
    assert.equal(frames[1].error.code, -32602);
    assert.equal(frames[1].error.data.code, "invalid_params");
    assert.ok(frames[1].error.data.violations.some((violation) =>
      violation.pointer === "/arguments/include_evidence"
      && violation.code === "unknown_property"));
    if (revision === "2024-11-05" || revision === "2025-03-26") {
      assert.ok(Array.isArray(frames[2]), `${revision} must emit one batch response array`);
      assert.deepEqual(frames[2].map(({ id }) => id), ["tools", "resources"]);
      assert.ok(Array.isArray(frames[2][0].result.tools));
      assert.ok(Array.isArray(frames[2][1].result.resources));
    } else {
      assert.equal(Array.isArray(frames[2]), false);
      assert.equal(frames[2].id, null);
      assert.equal(frames[2].error.code, -32600);
    }
    assert.equal(frames.length, 3, `${revision} emitted an unexpected notification response`);
  }
});

test("fail-open preparing is a successful revision-native result in every profile", () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const status = {
    plugin_runtime: { plugin_version: "0.17.4", warnings: [] },
    runtime: { state: "preparing" },
    warnings: [],
    readiness: [],
    managed_retrieval: { state: "preparing", automatic: true },
    degraded_reason: "managed_cli_provisioning",
  };
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    `run(${JSON.stringify(status)});`,
  ].join("");

  for (const revision of ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]) {
    const input = [
      JSON.stringify({
        jsonrpc: "2.0",
        id: "initialize",
        method: "initialize",
        params: { protocolVersion: revision },
      }),
      JSON.stringify({
        jsonrpc: "2.0",
        id: "cold-packet",
        method: "tools/call",
        params: {
          name: "packet",
          arguments: { project: repoRoot, question: "Explain dispatch." },
        },
      }),
      "",
    ].join("\n");
    const result = spawnSync(process.execPath, ["-e", fixture], {
      input,
      encoding: "utf8",
      timeout: 5000,
    });
    assert.equal(result.status, 0, result.stderr);
    const frames = result.stdout.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
    const toolResult = frames[1].result;
    const text = JSON.parse(toolResult.content[0].text);
    assert.equal(toolResult.isError, false, `${revision} preparing must not be a tool error`);
    assert.deepEqual(Object.keys(text).sort(), ["kind", "operation", "retry_after_ms", "state"]);
    assert.equal(text.kind, "preparing");
    assert.equal(text.state, "preparing");
    assert.ok(text.retry_after_ms > 0);
    assert.equal(typeof text.operation, "object");
    if (revision === "2024-11-05" || revision === "2025-03-26") {
      assert.equal(toolResult.structuredContent, undefined);
    } else {
      assert.deepEqual(toolResult.structuredContent, text);
    }
  }
});

test("fail-open validates every selected profile input schema before dispatch", () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const status = {
    plugin_runtime: { plugin_version: "0.17.4", warnings: [] },
    runtime: { state: "preparing" },
    warnings: [],
    readiness: [],
    managed_retrieval: { state: "preparing", automatic: true },
    degraded_reason: "managed_cli_provisioning",
  };
  const fixture = [
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    `run(${JSON.stringify(status)});`,
  ].join("");
  const exactPathProbes = Array.from({ length: 16 }, (_, index) => ({
    kind: "exact_path",
    path: `src/${index}.rs`,
  }));
  const cases = [
    ["status-project-type", "status", { project: 7 }, "/arguments/project", "invalid_type"],
    ["packet-root-type", "packet", [], "/arguments", "invalid_type"],
    ["packet-question-required", "packet", { project: repoRoot }, "/arguments/question", "missing_required"],
    ["packet-question-type", "packet", { project: repoRoot, question: 7 }, "/arguments/question", "invalid_type"],
    ["packet-question-bound", "packet", { project: repoRoot, question: "" }, "/arguments/question", "below_min_length"],
    ["packet-budget-enum", "packet", { project: repoRoot, question: "why", budget: "impossible" }, "/arguments/budget", "invalid_enum_value"],
    ["packet-tagged-probe", "packet", { project: repoRoot, question: "why", probes: [{ kind: "exact_path", id: "wrong" }] }, "/arguments/probes/0", "invalid_selector"],
    ["packet-array-bound", "packet", { project: repoRoot, question: "why", probes: [...exactPathProbes, { kind: "exact_path", path: "src/overflow.rs" }] }, "/arguments/probes", "above_max_items"],
    ["packet-string-bound", "packet", { project: repoRoot, question: "why", probes: [{ kind: "exact_path", path: "x".repeat(241) }] }, "/arguments/probes/0", "invalid_selector"],
    ["packet-combined-bound", "packet", { project: repoRoot, question: "why", probes: exactPathProbes, extra_probes: ["overflow"] }, "/arguments", "combined_item_limit"],
    ["context-selector-required", "context", { project: repoRoot }, "/arguments", "invalid_selector"],
    ["context-selector-exclusive", "context", { project: repoRoot, query: "entry", id: "node-1" }, "/arguments", "invalid_selector"],
    ["search-query-type", "search", { project: repoRoot, query: 7 }, "/arguments/query", "invalid_type"],
    ["search-limit-bound", "search", { project: repoRoot, query: "entry", limit: 0 }, "/arguments/limit", "below_minimum"],
    ["search-additional-property", "search", { project: repoRoot, query: "entry", extra: true }, "/arguments/extra", "unknown_property"],
  ];

  for (const revision of ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]) {
    const input = [
      JSON.stringify({
        jsonrpc: "2.0",
        id: "initialize",
        method: "initialize",
        params: { protocolVersion: revision },
      }),
      ...cases.map(([id, tool, argumentsValue]) => JSON.stringify({
        jsonrpc: "2.0",
        id,
        method: "tools/call",
        params: { name: tool, arguments: argumentsValue },
      })),
      "",
    ].join("\n");
    const result = spawnSync(process.execPath, ["-e", fixture], {
      input,
      encoding: "utf8",
      timeout: 5000,
    });
    assert.equal(result.status, 0, result.stderr);
    const frames = result.stdout.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
    assert.equal(frames.length, cases.length + 1, result.stdout);
    for (const [index, [id, tool, , pointer, code]] of cases.entries()) {
      const response = frames[index + 1];
      assert.equal(response.id, id);
      assert.equal(response.error?.code, -32602, `${revision} ${id}: ${JSON.stringify(response)}`);
      assert.equal(response.error?.data?.code, "invalid_params");
      assert.equal(response.error?.data?.tool, tool);
      assert.ok(
        response.error?.data?.violations?.some((violation) =>
          violation.pointer === pointer && violation.code === code),
        `${revision} ${id} expected ${code} at ${pointer}: ${JSON.stringify(response)}`,
      );
    }
  }
});

test("fail-open schema interpreter covers const anyOf and allOf", () => {
  const validate = launcherTest.validatePublishedSchemaValue;
  assert.equal(typeof validate, "function");
  const schema = {
    type: "object",
    additionalProperties: false,
    properties: {
      kind: { type: "string", const: "tagged" },
      value: { anyOf: [{ type: "string", minLength: 1 }, { type: "integer", minimum: 1 }] },
    },
    required: ["kind", "value"],
    allOf: [{ not: { properties: { value: { const: "forbidden" } }, required: ["value"] } }],
  };
  assert.deepEqual(validate(schema, { kind: "tagged", value: 1 }, "/arguments"), []);
  const violations = validate(schema, { kind: "wrong", value: "forbidden" }, "/arguments");
  assert.ok(violations.some(({ code, pointer }) => code === "invalid_const_value" && pointer === "/arguments/kind"));
  assert.ok(violations.some(({ code, pointer }) => code === "forbidden_combination" && pointer === "/arguments"));
});

test("fail-open schema validation covers every generated input keyword", () => {
  const found = new Set();
  const collect = (schema) => {
    if (!schema || typeof schema !== "object" || Array.isArray(schema)) return;
    const isSchema = [
      "type", "properties", "required", "oneOf", "anyOf", "allOf", "not", "items", "enum", "const",
    ].some((keyword) => Object.hasOwn(schema, keyword));
    for (const [keyword, value] of Object.entries(schema)) {
      if (isSchema) found.add(keyword);
      if (isSchema && keyword === "properties") {
        Object.values(value).forEach(collect);
      } else if (Array.isArray(value)) {
        value.forEach(collect);
      } else {
        collect(value);
      }
    }
  };
  for (const profile of Object.values(generatedCatalog.revisionProfiles)) {
    profile.tools.forEach((tool) => collect(tool.inputSchema));
  }
  const validated = new Set(launcherTest.failOpenValidatedSchemaKeywords);
  assert.deepEqual(
    [...found].filter((keyword) => !validated.has(keyword)).sort(),
    [],
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

test("fail-open handoff converts child stdin failure into a bounded request error", async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const fixture = [
    "const {EventEmitter}=require('node:events');",
    "const {PassThrough,Writable}=require('node:stream');",
    `const run=require(${JSON.stringify(launcher)})._test.runFailOpenMcp;`,
    "const child=new EventEmitter();",
    "child.exitCode=null;child.signalCode=null;child.codestoryCorrelationId='stdin-fixture';",
    "child.stdin=new Writable({write(_chunk,_encoding,callback){const error=new Error('unlabeled private query');error.code='EPIPE';callback(error);}});",
    "child.stdout=new PassThrough();child.stderr=new PassThrough();child.kill=()=>true;",
    "const status={plugin_runtime:{plugin_version:'0.16.3',warnings:[]},runtime:{state:'unavailable'},warnings:[],readiness:[],managed_retrieval:{state:'unavailable'},degraded_reason:'runtime_unavailable'};",
    "run(status,{shouldHandoff:()=>true,startRuntime:()=>child,onRuntimeFailure:()=>{},handoffTerminationGraceMs:1,handoffForceKillGraceMs:1});",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.stdin.end(`${JSON.stringify({ jsonrpc: "2.0", id: "stdin", method: "tools/list" })}\n`);
  const [exitCode] = await once(child, "close");
  assert.equal(exitCode, 0, stderr);
  const response = stdout.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line))
    .find((candidate) => candidate.id === "stdin");
  assert.equal(response.error.code, -32000);
  assert.match(response.error.message, /handoff stdin failed/u);
  assert.match(response.error.message, /correlation_id=stdin-fixture/u);
  assert.doesNotMatch(`${stdout}\n${stderr}`, /unlabeled private query/u);
});

test("runtime death diagnostics retain only typed fields", () => {
  const correlationId = launcherTest.runtimeCorrelationId();
  assert.match(correlationId, /^[0-9a-f]{32}$/u);
  assert.equal(launcherTest.sanitizeRuntimeDiagnosticText("unlabeled private query"), "[redacted]");
  const detail = launcherTest.runtimeFailureDetail("runtime_stdio_child_exit", {
    code: 17,
    signal: null,
    correlationId,
    error: new Error("unlabeled private query"),
    errorCode: "EPIPE",
    stderrBytes: 23,
    stderrChunks: 2,
    stderrBytesCapped: false,
    stderrChunksCapped: false,
  });
  assert.match(detail, /reason_code=runtime_stdio_child_exit exit_code=17 signal=none/u);
  assert.match(detail, new RegExp(`correlation_id=${correlationId}`, "u"));
  assert.match(detail, /stderr_bytes=23 stderr_chunks=2/u);
  assert.match(detail, /error_code=EPIPE/u);
  assert.doesNotMatch(detail, /unlabeled private query/u);
  const untrustedReason = launcherTest.runtimeFailureDetail("unlabeled_private_query");
  assert.doesNotMatch(untrustedReason, /unlabeled_private_query/u);
  assert.match(untrustedReason, /reason_code=unknown_runtime_failure/u);
});

test("runtime stderr observation never retains unlabeled child text", () => {
  let observation = null;
  observation = launcherTest.appendRuntimeStderrTail(observation, "unlabeled ");
  observation = launcherTest.appendRuntimeStderrTail(observation, "private query\n");
  const rendered = launcherTest.renderRuntimeStderrTail(observation);

  assert.doesNotMatch(JSON.stringify(observation), /unlabeled|private query/u);
  assert.deepEqual(rendered, {
    stderrBytes: Buffer.byteLength("unlabeled private query\n", "utf8"),
    stderrChunks: 2,
    stderrBytesCapped: false,
    stderrChunksCapped: false,
  });
});

test("runtime stderr metadata counters saturate at fixed bounds", () => {
  let observation = {
    observedBytes: launcherTest.runtimeStderrObservedBytesCap - 1,
    observedChunks: launcherTest.runtimeStderrObservedChunksCap - 1,
  };
  observation = launcherTest.appendRuntimeStderrTail(observation, "private query");
  observation = launcherTest.appendRuntimeStderrTail(observation, "private query");
  assert.deepEqual(launcherTest.renderRuntimeStderrTail(observation), {
    stderrBytes: launcherTest.runtimeStderrObservedBytesCap,
    stderrChunks: launcherTest.runtimeStderrObservedChunksCap,
    stderrBytesCapped: true,
    stderrChunksCapped: true,
  });
});

test("launcher records an uncaught exception and terminates instead of continuing", async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const fixture = [
    `require(${JSON.stringify(launcher)})._test.installLauncherFatalHandlers();`,
    "setImmediate(() => { throw new Error('unlabeled private query'); });",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["ignore", "pipe", "pipe"] });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const [exitCode, signal] = await once(child, "close");
  assert.equal(exitCode, 1);
  assert.equal(signal, null);
  const diagnostic = JSON.parse(stderr.trim());
  assert.equal(diagnostic.event, "launcher_uncaught_exception");
  assert.equal(diagnostic.error, "[redacted]");
  assert.equal(diagnostic.stack, "[redacted]");
  assert.doesNotMatch(stderr, /unlabeled private query/u);
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

async function readPinnedCliVersion() {
  const pin = JSON.parse(
    await readFile(join(pluginRoot, "cli-version.json"), "utf8"),
  );
  assert.match(pin.cli_version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u);
  return pin.cli_version;
}

function releaseAssetForPlatform(version) {
  const target = process.platform === "win32" && process.arch === "x64"
    ? "windows-x64"
    : process.platform === "linux" && process.arch === "x64"
        ? "linux-x64"
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
    archive_bytes: 4096,
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
    `if(process.env.CODESTORY_PLUGIN_PROVISIONING_PROBE==='1'&&args[0]==='serve'){let input='';process.stdin.on('data',chunk=>{input+=chunk;const newline=input.indexOf('\\n');if(newline<0)return;const request=JSON.parse(input.slice(0,newline));process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{protocolVersion:request.params.protocolVersion,capabilities:{},serverInfo:{name:'fixture',version:'1'},_meta:{codestory_protocol:{discovery_contract_sha256:${JSON.stringify(discoveryDigest())}},codestory_publication:{schema_version:Number(process.env.CODESTORY_TEST_STAMP_SCHEMA_VERSION||'3'),minimum_compatible_schema_version:3}}}})+'\\n',()=>process.exit(0))})}`,
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
    "if(args[0]==='--version'){const delay=Number(process.env.CODESTORY_TEST_PROBE_DELAY_MS||0);if(delay>0)Atomics.wait(new Int32Array(new SharedArrayBuffer(4)),0,0,delay);console.log('codestory-cli '+process.env.TEST_CODESTORY_VERSION);process.exit(0)}",
    "if(args[0]!=='serve')process.exit(2);",
    "let initialized=false;let notified=false;let input='';",
    "process.stdin.setEncoding('utf8');",
    `const discoveryContracts=${JSON.stringify(generatedCatalog.wireContract.discoveryContracts)};process.stdin.on('data',chunk=>{input+=chunk;const lines=input.split(/\\r?\\n/u);input=lines.pop()||'';for(const line of lines){if(!line)continue;const request=JSON.parse(line);if(request.method==='initialize'){initialized=true;const revision=request.params.protocolVersion;process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{protocolVersion:revision,capabilities:{tools:{listChanged:false},resources:{listChanged:false},prompts:{listChanged:false}},serverInfo:{name:'fixture',version:'1'},_meta:{codestory_protocol:{discovery_contract_sha256:discoveryContracts[revision]},codestory_publication:{schema_version:Number(process.env.CODESTORY_TEST_STAMP_SCHEMA_VERSION||'3'),minimum_compatible_schema_version:3}}}})+'\\n')}else if(request.method==='notifications/initialized'){notified=true}else if(request.method==='tools/list'){if(!initialized||!notified)process.exit(42);fs.writeFileSync(process.env.TEST_OUT,JSON.stringify({initialized,notified,args}));process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{tools:[]}})+'\\n')}else if(request.method==='resources/list'){process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{resources:[]}})+'\\n',()=>process.exit(17))}}});`,
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

test("CLI version probe budget covers cold starts above three seconds", () => {
  const coldStartMs = 3250;
  const version = "0.16.0";
  const probe = launcherTest.probeResolvedCli(
    { path: process.execPath },
    {
      spawnCli(cliPath, args, options) {
        assert.equal(cliPath, process.execPath);
        assert.deepEqual(args, ["--version"]);
        assert.equal(options.timeout, devCliContract.cliVersionProbeTimeoutMs);
        assert.ok(options.timeout > coldStartMs);
        return {
          status: 0,
          error: null,
          stdout: `codestory-cli ${version}\n`,
          stderr: "",
        };
      },
    },
  );

  assert.equal(devCliContract.cliVersionProbeTimeoutMs, 15000);
  assert.equal(probe.status, 0, probe.error);
  assert.equal(probe.version, version);
});

function assertManagedProbeDetailsSanitized(output, {
  expectedProject,
  expectedDiagnosticsUri,
  hostilePath,
  hostileDetail,
  classified,
  code,
  warning,
  warnings,
}) {
  assert.equal(output.isError, true);
  assert.equal(output.structuredContent.project, expectedProject);
  assert.equal(output.structuredContent.diagnostics_uri, expectedDiagnosticsUri);
  assert.equal(output.structuredContent.failure, warning);
  assert.equal(output.structuredContent.failure_context, null);
  assert.deepEqual(output.content, [{
    type: "text",
    text: output.structuredContent.message,
  }]);
  const typedProbeTokens = JSON.stringify({ classified, code, warning, warnings });
  const outputRemainder = structuredClone(output);
  outputRemainder.structuredContent.project = "<allowed-project>";
  outputRemainder.structuredContent.diagnostics_uri = "<allowed-diagnostics-uri>";
  const serializedRemainder = JSON.stringify(outputRemainder);
  for (const [label, marker] of [
    ["path", hostilePath],
    ["detail", hostileDetail],
  ]) {
    const serializedMarker = JSON.stringify(marker).slice(1, -1);
    assert.equal(
      typedProbeTokens.includes(serializedMarker),
      false,
      `typed probe tokens retained hostile ${label}`,
    );
    assert.equal(
      serializedRemainder.includes(serializedMarker),
      false,
      `fail-open remainder retained hostile ${label}`,
    );
  }
}

test("managed probe failures stay sanitized through fail-open output", (t) => {
  const project = fs.mkdtempSync(
    join(tmpdir(), "codestory-v3-task16c-private-untrusted-detail-source-"),
  );
  t.after(() => fs.rmSync(project, { recursive: true, force: true }));
  const expectedProject = fs.realpathSync(project);
  assert.match(expectedProject, /private/u);
  assert.match(expectedProject, /untrusted-detail/u);
  const hostilePath = "C:\\private\\candidate.exe";
  const hostileDetail = "untrusted-detail";
  const hostile = `${hostilePath}\n${hostileDetail}`;
  const cases = [
    {
      probe: {
        error: `spawnSync failed: ${hostile}`,
        errorCode: `ETIMEDOUT:${hostile}`,
        status: null,
        version: null,
      },
      reason: "version_probe_error:ETIMEDOUT",
    },
    {
      probe: { error: null, status: 7, stderr: hostile, version: null },
      reason: "version_probe_exit:7",
    },
    {
      probe: { error: null, status: 0, stdout: hostile, version: "0.15.0" },
      reason: "version_probe_mismatch",
    },
  ];

  for (const { probe, reason } of cases) {
    const classified = launcherTest.managedCliVersionProbeFailure(probe, "0.16.0");
    assert.equal(classified, reason);
    const error = new Error(
      `managed_cli_staging_verification_failed:${classified}:${hostile}`,
    );
    const code = launcherTest.managedCliFailureCode(error);
    assert.equal(code, `managed_cli_staging_verification_failed:${reason}`);
    const warnings = [];
    const warning = launcherTest.recordManagedCliProvisionFailure(warnings, error);
    assert.deepEqual(warnings, [
      `managed_cli_publication:terminal_failure:${code}`,
      warning,
    ]);
    const output = launcherTest.failOpenToolResult(
      "ground",
      {
        plugin_runtime: { plugin_version: "0.16.0" },
        managed_retrieval: { state: "unavailable" },
        degraded_reason: warning,
        warnings,
        readiness: [{
          reason: warning,
          summary: "runtime unavailable",
          setup: { probe_error: "generic_probe_failure" },
        }],
      },
      { project },
    );
    const expectedDiagnosticsUri = launcherTest.projectBoundResourceUri(
      "codestory://status",
      expectedProject,
    );
    const assertionContext = {
      expectedProject,
      expectedDiagnosticsUri,
      hostilePath,
      hostileDetail,
      classified,
      code,
      warning,
      warnings,
    };
    assertManagedProbeDetailsSanitized(output, assertionContext);
    if (reason === "version_probe_error:ETIMEDOUT") {
      for (const [label, marker] of [
        ["path", hostilePath],
        ["detail", hostileDetail],
      ]) {
        const nestedLeak = structuredClone(output);
        nestedLeak.structuredContent.readiness = [{
          setup: { probe_stderr: marker },
        }];
        assert.throws(
          () => assertManagedProbeDetailsSanitized(nestedLeak, assertionContext),
          new RegExp(`fail-open remainder retained hostile ${label}`, "u"),
        );
      }
    }
  }
});

test("provisioning retry hints derive remaining transfer time within documented bounds", () => {
  const hint = launcherTest.provisioningRetryHintMs;
  // States carrying no measurable transfer keep the documented fallback.
  assert.equal(launcherTest.provisioningRetryHintFallbackMs, 1500);
  assert.equal(hint({ receivedBytes: 0, totalBytes: null, startedAt: null, updatedAt: null }), 1500);
  assert.equal(hint({ receivedBytes: 4096, totalBytes: null, startedAt: 0, updatedAt: 1000 }), 1500);
  assert.equal(hint({ receivedBytes: 0, totalBytes: 1024, startedAt: 0, updatedAt: 1000 }), 1500);
  assert.equal(hint({ receivedBytes: 10, totalBytes: 100, startedAt: 500, updatedAt: 500 }), 1500);
  // A quarter received in one second forecasts three more seconds of transfer.
  assert.equal(hint({ receivedBytes: 25, totalBytes: 100, startedAt: 0, updatedAt: 1000 }), 3000);
  // A slow large download clamps to the ceiling instead of parking the agent for minutes.
  assert.equal(
    hint({ receivedBytes: 1024, totalBytes: 1024 * 1024 * 1024, startedAt: 0, updatedAt: 1000 }),
    launcherTest.provisioningRetryHintMaxMs,
  );
  // An almost-finished transfer clamps to the floor instead of oversleeping readiness.
  assert.equal(
    hint({ receivedBytes: 1_048_575, totalBytes: 1_048_576, startedAt: 0, updatedAt: 10 }),
    launcherTest.provisioningRetryHintMinMs,
  );
  // A completed asset needs no rate to forecast: the next provisioning stage is imminent.
  assert.equal(
    hint({ receivedBytes: 2048, totalBytes: 2048, startedAt: 100, updatedAt: 100 }),
    launcherTest.provisioningRetryHintMinMs,
  );
});

test("preparing fail-open surfaces share one progress-derived retry hint", () => {
  const original = { ...launcherTest.managedCliDownloadProgress };
  try {
    Object.assign(launcherTest.managedCliDownloadProgress, {
      stage: "downloading_runtime",
      asset: "codestory-cli-v0.0.0-test.tar.gz",
      attempt: 2,
      receivedBytes: 25,
      totalBytes: 100,
      startedAt: 0,
      updatedAt: 1000,
    });
    const operation = launcherTest.managedProvisioningOperation();
    assert.equal(operation.retry_after_ms, 3000);
    const preparingStatus = {
      plugin_runtime: { plugin_version: "test" },
      managed_retrieval: { state: "preparing" },
      degraded_reason: "managed_cli_provisioning",
      warnings: [],
      readiness: [],
    };
    const ground = launcherTest.failOpenToolResult("ground", preparingStatus, { project: repoRoot });
    assert.equal(ground.structuredContent.retry_after_ms, 3000);
    assert.equal(ground.structuredContent.operation.retry_after_ms, 3000);
    const status = launcherTest.failOpenToolResult("status", preparingStatus, { project: repoRoot });
    assert.equal(status.structuredContent.retry_after_ms, 3000);
    assert.equal(status.structuredContent.current_operation.retry_after_ms, 3000);
  } finally {
    Object.assign(launcherTest.managedCliDownloadProgress, original);
  }
});

test("fail-open status reads refresh the preparing retry hint from live download progress", { timeout: 5000 }, async () => {
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const fixture = [
    `const launcherModule=require(${JSON.stringify(launcher)})._test;`,
    // The diagnostic snapshot predates the transfer, so its recommended call still carries the
    // no-signal fallback; only the read below observes the in-flight download.
    "const status={",
    'plugin_runtime:{plugin_version:"test"},',
    'managed_retrieval:{state:"preparing"},',
    'degraded_reason:"managed_cli_provisioning",',
    "readiness:[],",
    "recommended_next_calls:[",
    '{method:"tools/call",instruction:"Retry the intended CodeStory tool shortly.",after_ms:launcherModule.provisioningRetryHintFallbackMs},',
    '{method:"resources/read",uri_template:"codestory://status{?project}"}',
    "]};",
    'Object.assign(launcherModule.managedCliDownloadProgress,{stage:"downloading_runtime",asset:"archive.tar.gz",attempt:1,receivedBytes:25,totalBytes:100,startedAt:0,updatedAt:1000});',
    "launcherModule.runFailOpenMcp(()=>status);",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  const completed = once(child, "close");
  let output = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stdin.end(`${JSON.stringify({
    jsonrpc: "2.0",
    id: 1,
    method: "resources/read",
    params: { uri: statusUri },
  })}\n`);
  assert.equal((await completed)[0], 0);
  const responses = output.split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
  const status = JSON.parse(responses.find((response) => response.id === 1).result.contents[0].text);
  assert.deepEqual(status.recommended_next_calls, [
    { method: "tools/call", instruction: "Retry the intended CodeStory tool shortly.", after_ms: 3000 },
    { method: "resources/read", uri: statusUri },
  ]);
});

async function writeAttestedDevPluginFixture(root, pluginVersion, cliVersion = pluginVersion) {
  const { cp } = await import("node:fs/promises");
  const installRoot = join(
    root,
    ".codex",
    "plugins",
    "cache",
    "CodeStoryDev",
    "codestory",
    pluginVersion,
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
      plugin_version: pluginVersion,
      source_commit: "a".repeat(40),
      source_package_sha256: sourcePackageSha256,
      target: devCliContract.sourceBuildTarget(),
      cli: {
        path: `bin/${cliName}`,
        name: cliName,
        bytes: cliBytes.length,
        sha256: cliSha256,
        version: cliVersion,
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
  assert.match(agentMetadata, /read and follow the loaded codestory-grounding skill/isu);
  assert.match(agentMetadata, /sole source of truth/isu);
  assert.match(agentMetadata, /adds no parallel instructions/isu);
  assert.doesNotMatch(
    agentMetadata,
    /search.*context.*packet.*prove_call_path|unknown.*not absence|typed contract/isu,
  );
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
    join(pluginRoot, "rules", "codestory.mdc"),
  ]) {
    const text = await readFile(file, "utf8");
    assert.match(text, /canonical codestory-grounding skill.*sole source of truth/isu, file);
    assert.doesNotMatch(text, /Call the CodeStory tool that matches the task|Routing contract:|prove_call_path/u, file);
    assert.doesNotMatch(text, /read `codestory:\/\/status` first/u, file);
    assert.doesNotMatch(text, /codestory-cli ready/u, file);
  }
});

test("skill teaches MCP catalog arguments, not Clap flags", async () => {
  const skill = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
    "utf8",
  );
  assert.match(skill, /generated-mcp-syntax\.md/u);
  const catalog = JSON.parse(
    await readFile(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"),
  );
  const syntax = await readFile(
    join(
      pluginRoot,
      "skills",
      "codestory-grounding",
      "references",
      "generated-mcp-syntax.md",
    ),
    "utf8",
  );
  for (const tool of catalog.tools) {
    assert.match(syntax, new RegExp(`\`${tool.name}\``, "u"), tool.name);
  }
  for (const name of [
    "search.md",
    "ground.md",
    "trail.md",
    "context.md",
    "files.md",
    "affected.md",
    "packet.md",
    "snippet.md",
    "symbol.md",
  ]) {
    const text = await readFile(
      join(pluginRoot, "skills", "codestory-grounding", "references", name),
      "utf8",
    );
    assert.match(text, /generated-mcp-syntax\.md/u, name);
    assert.doesNotMatch(text, /See \[generated CLI syntax\]/u, name);
    assert.doesNotMatch(text, /<codestory-cli>/u, name);
    assert.doesNotMatch(text, /--probe\b/u, name);
    assert.doesNotMatch(text, / --(?:why|mode|file|refresh|plan-details)\b/u, name);
  }
});

test("plugin package version tracks the codestory-cli release version", async () => {
  const cliManifest = await readFile(
    join(repoRoot, "crates", "codestory-cli", "Cargo.toml"),
    "utf8",
  );
  const workspaceVersion = readCargoVersion(cliManifest);
  const manifestPaths = [
    join(pluginRoot, "plugin.json"),
    join(pluginRoot, ".codex-plugin", "plugin.json"),
    join(pluginRoot, ".cursor-plugin", "plugin.json"),
    join(pluginRoot, ".claude-plugin", "plugin.json"),
    join(pluginRoot, ".github", "plugin", "plugin.json"),
  ];

  // The portable and host manifests agree on the plugin identity.
  const versions = [];
  for (const manifestPath of manifestPaths) {
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    versions.push(manifest.version);
  }
  assert.equal(new Set(versions).size, 1, `host manifests disagree: ${versions}`);

  // The pin names the CLI the plugin runs. The workspace builds that CLI, so the pin and the
  // workspace version move together; the plugin version may only run ahead of them once the
  // plugin-only release lane exists, and never behind.
  const pin = JSON.parse(await readFile(join(pluginRoot, "cli-version.json"), "utf8"));
  assert.equal(pin.schema_version, 1);
  assert.equal(pin.cli_version, workspaceVersion, "pin must name the workspace CLI version");
  assert.equal(pin.release_tag, `v${pin.cli_version}`);
  const [pluginVersion] = versions;
  assert.ok(
    pluginVersion === pin.cli_version || semverGreater(pluginVersion, pin.cli_version),
    `plugin ${pluginVersion} must not trail its pinned CLI ${pin.cli_version}`,
  );
  if (pin.archives !== undefined) {
    const targets = Object.keys(pin.archives).sort();
    assert.deepEqual(targets, ["linux-x64", "macos-arm64", "windows-x64"]);
    for (const digest of Object.values(pin.archives)) {
      assert.match(digest, /^[0-9a-f]{64}$/u);
    }
  }
});

function semverGreater(left, right) {
  const parse = (value) => value.split("-")[0].split(".").map(Number);
  const [lmaj, lmin, lpat] = parse(left);
  const [rmaj, rmin, rpat] = parse(right);
  if (lmaj !== rmaj) return lmaj > rmaj;
  if (lmin !== rmin) return lmin > rmin;
  return lpat > rpat;
}

test("the CLI version pin decides what the managed path provisions", async () => {
  // Read the version out of the pin rather than restating it: a release bump
  // rewrites the pin, and a test that hardcodes the old number fails the bump
  // instead of the behaviour it is guarding.
  const pin = launcherTest.pinnedCliContract();
  assert.match(pin.cli_version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u);
  assert.equal(launcherTest.pinnedCliVersion(), pin.cli_version);
  assert.equal(pin.release_tag, `v${pin.cli_version}`);

  // A native bump drops the digests: they cannot exist until that release
  // publishes its archives, and the plugin lane re-adds them when it pins an
  // already-published CLI.
  if (pin.archives) {
    assert.equal(
      launcherTest.pinnedArchiveSha256("macos-arm64"),
      pin.archives["macos-arm64"],
    );
  } else {
    assert.equal(launcherTest.pinnedArchiveSha256("macos-arm64"), null);
  }
  assert.equal(launcherTest.pinnedArchiveSha256("no-such-target"), null);
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

test("dirty marker hook manager delegates install and status to an explicit CLI", async () => {
  if (process.platform === "win32") return;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-cli-data-"));
  const projectRoot = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-cli-project-"));
  const script = join(pluginRoot, "hooks", "codestory-dirty-hook.cjs");
  const fakeCli = join(dataDir, "fake-cli");
  const observedArgs = join(dataDir, "hook-args.json");

  try {
    await writeFile(
      fakeCli,
      `#!${process.execPath}\nrequire('fs').writeFileSync(process.env.HOOK_ARGS, JSON.stringify(process.argv.slice(2))); process.stdout.write(JSON.stringify({schema_version:1,status:'installed',hooks:[]}));\n`,
      "utf8",
    );
    await chmod(fakeCli, 0o755);

    const install = spawnSync(process.execPath, [
      script,
      "install",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
      "--cli",
      fakeCli,
    ], { encoding: "utf8", env: { ...process.env, HOOK_ARGS: observedArgs } });
    assert.equal(install.status, 0, install.stderr);
    assert.equal(JSON.parse(install.stdout).status, "installed");
    const delegated = JSON.parse(await readFile(observedArgs, "utf8"));
    assert.deepEqual(delegated.slice(0, 2), ["internal-dirty-hook", "install"]);
    assert.equal(delegated[delegated.indexOf("--project") + 1], projectRoot);
    assert.equal(delegated[delegated.indexOf("--plugin-data") + 1], dataDir);
    assert.equal(delegated[delegated.indexOf("--node") + 1], process.execPath);
    assert.equal(delegated[delegated.indexOf("--script") + 1], await realpath(script));
    await assert.rejects(access(join(projectRoot, ".git")));

    const status = spawnSync(process.execPath, [
      script,
      "status",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
    ], { encoding: "utf8", env: { ...process.env, CODESTORY_CLI: "", CODESTORY_PLUGIN_CLI_PATH: "" } });
    assert.equal(status.status, 0, status.stderr);
    assert.equal(JSON.parse(status.stdout).status, "cli_unavailable");

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

test("dirty hook status accepts only a checksummed runtime receipt and never provisions", async () => {
  if (process.platform === "win32") return;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-receipt-"));
  const projectRoot = await mkdtemp(join(tmpdir(), "codestory-dirty-hook-receipt-project-"));
  const script = join(pluginRoot, "hooks", "codestory-dirty-hook.cjs");
  const fakeCli = join(dataDir, "fake-cli");
  const sentinel = join(dataDir, "ran");
  try {
    await writeFile(
      fakeCli,
      `#!${process.execPath}\nrequire('fs').writeFileSync(process.env.HOOK_SENTINEL, 'ran'); process.stdout.write(JSON.stringify({schema_version:1,status:'not_installed',hooks:[]}));\n`,
      "utf8",
    );
    await chmod(fakeCli, 0o755);
    const digest = createHash("sha256").update(await readFile(fakeCli)).digest("hex");
    const receipt = join(dataDir, ".codestory-mcp-runtime.json");
    await writeFile(receipt, JSON.stringify({ path: fakeCli, sha256: digest }), "utf8");

    const verified = spawnSync(process.execPath, [
      script,
      "status",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
    ], {
      encoding: "utf8",
      env: { ...process.env, CODESTORY_CLI: "", CODESTORY_PLUGIN_CLI_PATH: "", HOOK_SENTINEL: sentinel },
    });
    assert.equal(verified.status, 0, verified.stderr);
    assert.equal(JSON.parse(verified.stdout).status, "not_installed");
    await access(sentinel);

    await rm(sentinel, { force: true });
    await writeFile(receipt, JSON.stringify({ path: fakeCli, sha256: "0".repeat(64) }), "utf8");
    const rejected = spawnSync(process.execPath, [
      script,
      "status",
      "--project",
      projectRoot,
      "--plugin-data",
      dataDir,
    ], {
      encoding: "utf8",
      env: { ...process.env, CODESTORY_CLI: "", CODESTORY_PLUGIN_CLI_PATH: "", HOOK_SENTINEL: sentinel },
    });
    assert.equal(rejected.status, 0, rejected.stderr);
    assert.equal(JSON.parse(rejected.stdout).status, "cli_unavailable");
    await assert.rejects(access(sentinel));
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(projectRoot, { recursive: true, force: true });
  }
});

test("production hook code neither duplicates Git config nor spawns Git", async () => {
  const javascriptSources = await Promise.all([
    readFile(join(pluginRoot, "hooks", "codestory-dirty-hook.cjs"), "utf8"),
    readFile(join(pluginRoot, "hooks", "codestory-runtime.cjs"), "utf8"),
  ]);
  for (const source of javascriptSources) {
    assert.doesNotMatch(source, /hooksPath|gitDirForProject|spawnSync\(['"]git['"]/u);
  }
  const rustSource = await readFile(
    join(repoRoot, "crates", "codestory-workspace", "src", "repository_hooks.rs"),
    "utf8",
  );
  const productionRust = rustSource.split("#[cfg(test)]\nmod tests")[0];
  assert.equal(productionRust.includes(`Command::new("git"`), false);
});

test("mcp launcher prefers a checksummed explicit package without PATH", async () => {
  const { spawnSync } = await import("node:child_process");
  const version = await readPinnedCliVersion();
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
  const pluginVersion = await readPluginVersion();
  const cliVersion = await readPinnedCliVersion();
  try {
    const fixture = await writeAttestedDevPluginFixture(root, pluginVersion, cliVersion);
    await mkdir(dataDir, { recursive: true });
    const result = spawnSync(process.execPath, [fixture.launcher], {
      env: {
        ...process.env,
        CODESTORY_CLI: "",
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: cliVersion,
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
    assert.equal(observed.pluginCacheVersion, pluginVersion);
    assert.match(observed.warnings, /codestory_dev_receipt:verified/u);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("declared CodeStoryDev receipt failures never fall through to raw or managed CLI selection", async () => {
  if (process.platform === "win32") return;
  const pluginVersion = await readPluginVersion();
  const cliVersion = await readPinnedCliVersion();
  for (const variant of ["invalid-receipt", "ambiguous-raw-override"]) {
    const root = await mkdtemp(join(tmpdir(), "codestory-dev-receipt-no-fallback-"));
    const dataDir = join(root, "plugin-data");
    const runtimeOut = join(root, "runtime.json");
    try {
      const fixture = await writeAttestedDevPluginFixture(root, pluginVersion, cliVersion);
      const managedDir = join(dataDir, "codestory-cli", cliVersion);
      const managedCli = join(managedDir, process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
      await mkdir(managedDir, { recursive: true });
      await writeFakeCli(managedCli);
      const managedSha256 = createHash("sha256").update(await readFile(managedCli)).digest("hex");
      await writeFile(
        join(managedDir, "manifest.json"),
        JSON.stringify(managedReleaseManifest(cliVersion, managedCli.slice(managedDir.length + 1), managedSha256)),
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
          TEST_CODESTORY_VERSION: cliVersion,
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
    delete manifest.archive_bytes;
    await writeFile(
      join(fixture.versionDir, "manifest.json"),
      JSON.stringify(manifest),
      "utf8",
    );
    const missingArchiveBytes = launcherTest.verifyPublishedManagedCli(
      fixture.versionDir,
      version,
      target,
      probe,
    );
    assert.equal(missingArchiveBytes.verified, false);
    assert.equal(missingArchiveBytes.reason, "manifest_release_metadata_invalid");
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

// A plugin-only release moves the plugin version without moving the pinned CLI version, which is the
// normal state of the plugin lane. Retention must key on CLI identity: comparing the running CLI's
// probe against the plugin version made every such release look like an active-version mismatch and
// silently switched managed-CLI pruning off for the whole release.
test("managed cli retention keeps pruning when the plugin version leads the cli version", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-retention-skew-"));
  try {
    const stale = await writeManagedCliFixture(dataDir, "0.15.9");
    const rollback = await writeManagedCliFixture(dataDir, "0.16.0");
    const active = await writeManagedCliFixture(dataDir, "0.16.1");
    const probeVersion = (candidate) => ({
      status: 0,
      error: null,
      version: candidate.cliVersion || candidate.version,
      stdout: "",
      stderr: "",
    });
    const resolved = {
      source: "managed",
      version: "0.16.4",
      cliVersion: "0.16.1",
      path: active.cliPath,
      warnings: [],
    };

    const report = launcherTest.managedCliRetentionReport(resolved, probeVersion(resolved), {
      dataDir,
      probeVersion,
    });

    assert.deepEqual(report.warnings, []);
    assert.deepEqual(report.retained.map((entry) => entry.version), ["0.16.1", "0.16.0"]);
    assert.equal(report.retained.find((entry) => entry.version === "0.16.1").reason, "active");
    assert.deepEqual(report.removed.map((entry) => entry.version), ["0.15.9"]);
    assert.equal(report.removed_bytes > 0, true);
    await assert.rejects(access(stale.versionDir));
    await access(rollback.versionDir);
    await access(active.versionDir);
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

test("managed cli async lock hoists self identity and identity-probe deadline across retries", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-managed-lock-identity-throttle-"));
  const root = join(dataDir, "codestory-cli");
  await mkdir(root);
  const held = launcherTest.acquireManagedCliLock(root, "holder");
  assert.ok(held);
  const identity = launcherTest.processStartIdentity(process.pid);
  assert.ok(identity, `process start identity unavailable on ${process.platform}`);
  const interval = launcherTest.managedCliIdentityProbeIntervalMs;
  const startedAt = 10_000;
  let now = startedAt;
  let sleepCalls = 0;
  const identityProbeTimes = [];

  try {
    const acquired = await launcherTest.acquireManagedCliLockAsync(
      root,
      "waiter",
      (interval * 2) + 100,
      {
        now: () => now,
        sleep: async (milliseconds) => {
          sleepCalls += 1;
          now += milliseconds;
        },
        processStartIdentity: () => {
          identityProbeTimes.push(now);
          return identity;
        },
      },
    );

    assert.equal(acquired, null);
    assert.equal(sleepCalls, ((interval * 2) + 100) / 50);
    // The first sample is the async acquisition's one self-identity read. The
    // remaining samples are the live lock owner's probes: one immediately,
    // then only when each shared two-second deadline expires.
    assert.deepEqual(identityProbeTimes, [
      startedAt,
      startedAt,
      startedAt + interval,
      startedAt + (interval * 2),
    ]);
  } finally {
    launcherTest.releaseManagedCliLock(held);
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
  // The archive budget has to be large enough that a multi-hundred-megabyte download can finish on
  // a slow link, and a waiter must outlast a publisher spending both asset budgets back to back.
  assert.ok(launcherTest.releaseArchiveTotalTimeoutMs >= 30 * 60 * 1000);
  assert.equal(launcherTest.releaseAssetRetryBudgetMs, launcherTest.releaseArchiveTotalTimeoutMs);
  assert.ok(
    launcherTest.managedCliLockWaitMs >=
      launcherTest.releaseChecksumTotalTimeoutMs + launcherTest.releaseArchiveTotalTimeoutMs,
  );
  // The stall timeout, not the total budget, is what should cut off a dead connection.
  assert.ok(launcherTest.releaseDownloadStallTimeoutMs < launcherTest.releaseArchiveTotalTimeoutMs);
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

function compatibleProbeResult(stamp = { schema_version: 3, minimum_compatible_schema_version: 3 }) {
  return {
    jsonrpc: "2.0",
    id: "managed-cli-staging",
    result: {
      protocolVersion: preferredRevision,
      capabilities: {},
      serverInfo: { name: "fixture", version: "1" },
      _meta: {
        codestory_protocol: { discovery_contract_sha256: discoveryDigest() },
        ...(stamp === null ? {} : { codestory_publication: stamp }),
      },
    },
  };
}

test("managed cli staging refuses to stage a runtime whose publication stamp it cannot read", async () => {
  // ARCH-035: provisioning establishes the pinned pair, so a runtime that
  // publishes a stamp outside the launcher's window never becomes the staged
  // CLI. `null` is the legacy v0 producer that predates the stamp entirely.
  const cases = [
    [null, "publication_stamp_legacy_v0"],
    [{ schema_version: 0 }, "publication_stamp_legacy_v0"],
    [{ schema_version: "3" }, "publication_stamp_malformed"],
    [{ schema_version: 1 }, "publication_stamp_producer_too_old"],
    [{ schema_version: 4 }, "publication_stamp_producer_too_new"],
    [
      { schema_version: 3, minimum_compatible_schema_version: 4 },
      "publication_stamp_producer_too_new",
    ],
  ];
  for (const [stamp, expected] of cases) {
    await assert.rejects(
      launcherTest.probeManagedCliStdio("fixture", 100, {
        spawn: () => fakeProbeChild(compatibleProbeResult(stamp)),
        terminationGraceMs: 5,
        forceKillGraceMs: 20,
      }),
      new RegExp(`managed_cli_stdio_initialize_wire_contract:${expected}$`, "u"),
      `stamp ${JSON.stringify(stamp)} must be refused as ${expected}`,
    );
  }
});

test("managed cli staging escalates and awaits a stubborn child", async () => {
  const compatible = compatibleProbeResult();
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
      "else if (args[0] === 'serve') { process.stderr.write('unlabeled '); setTimeout(() => { process.stderr.write('private query\\n'); process.exit(17); }, 25); }",
      "else { process.exit(2); }",
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
    assert.equal(status.readiness[0].setup.runtime_exit_code, 17);
    assert.equal(status.readiness[0].setup.runtime_exit_signal, null);
    assert.match(status.readiness[0].setup.runtime_correlation_id, /^[0-9a-f]{32}$/u);
    assert.equal(
      status.readiness[0].setup.runtime_stderr_bytes,
      Buffer.byteLength("unlabeled private query\n", "utf8"),
    );
    assert.ok(status.readiness[0].setup.runtime_stderr_chunks >= 1);
    assert.equal(status.readiness[0].setup.runtime_stderr_bytes_capped, false);
    assert.equal(status.readiness[0].setup.runtime_stderr_chunks_capped, false);
    assert.equal(status.readiness[0].setup.runtime_stderr_tail, undefined);
    assert.doesNotMatch(JSON.stringify(status), /unlabeled private query/u);
    assert.doesNotMatch(stderr, /unlabeled private query/u);
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
      `        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { protocolVersion: process.env.TEST_PROTOCOL_VERSION || ${JSON.stringify(preferredRevision)}, serverInfo: { name: 'codestory', version: '1' }, _meta: { codestory_protocol: { discovery_contract_sha256: ${JSON.stringify(discoveryDigest())} }, codestory_publication: { schema_version: Number(process.env.TEST_STAMP_SCHEMA_VERSION || '3'), minimum_compatible_schema_version: 3 } } } }) + '\\n');`,
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
      params: { protocolVersion: preferredRevision },
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
    JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: preferredRevision } }),
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
    const canonicalToolNames = generatedCatalog.revisionProfiles[preferredRevision].tools
      .map((tool) => tool.name);
    assert.deepEqual([...toolNames].sort(), [...canonicalToolNames].sort());
    const coldGroundTool = responses[2].result.tools.find((tool) => tool.name === "ground");
    const groundSafety = coldGroundTool._meta["com.thegreencedar.codestory/safety"];
    assert.equal(groundSafety.effect, "managed_activation");
    assert.equal(groundSafety.requiresConfirmation, false);
    assert.equal(groundSafety.localOnly, false);
    assert.equal(groundSafety.openWorld, true);
    assert.deepEqual(coldGroundTool.annotations, {
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: true,
    });
    const coldStatusTool = responses[2].result.tools.find((tool) => tool.name === "status");
    assert.deepEqual(coldStatusTool.annotations, {
      destructiveHint: false,
      idempotentHint: true,
      openWorldHint: false,
      readOnlyHint: true,
    });
    assert.equal(responses[3].result.isError, true);
    assert.equal(responses[3].result.structuredContent, undefined);
    assert.equal(toolTextJson(responses[3]).code, "project_required");
    assert.equal(toolTextJson(responses[3]).tool, "ground");
    assert.equal(toolTextJson(responses[3]).retry_tool, undefined);
    assert.match(toolTextJson(responses[3]).message, /absolute repository root/u);
    assert.equal(toolTextJson(responses[4]).current_operation, null);
    assert.equal(responses[5].result.isError, true);
    assert.equal(toolTextJson(responses[5]).code, "project_required");
    assert.equal(responses[6].result.isError, true);
    assert.equal(toolTextJson(responses[6]).code, "project_unavailable");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher preserves the managed CLI verification failure", async () => {
  const version = await readPinnedCliVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-invalid-managed-cli-"));
  const versionDir = join(dataDir, "codestory-cli", version);
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const reason = "managed_cli_verification_failed:manifest_version_mismatch";
  const input = [
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { protocolVersion: preferredRevision },
    }),
    JSON.stringify({
      jsonrpc: "2.0",
      id: 2,
      method: "resources/read",
      params: { uri: statusUri },
    }),
    JSON.stringify({
      jsonrpc: "2.0",
      id: 3,
      method: "tools/call",
      params: { name: "ground", arguments: { project: repoRoot } },
    }),
  ].join("\n") + "\n";

  try {
    await mkdir(versionDir, { recursive: true });
    await writeFile(
      join(versionDir, "manifest.json"),
      JSON.stringify({ version: "0.0.0" }),
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
    const status = JSON.parse(responses[1].result.contents[0].text);
    assert.equal(status.degraded_reason, reason);
    assert.ok(status.warnings.includes(reason), JSON.stringify(status.warnings));
    assert.equal(status.readiness[0].reason, reason);
    assert.equal(toolTextJson(responses[2]).failure, reason);
    assert.equal(responses[2].result.isError, true);
    assert.equal(responses[2].result.structuredContent, undefined);
    assert.equal(toolTextJson(responses[2]).code, "codestory_unavailable");
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

test("packaged initialize handshake carries the publication stamp the host reads", async () => {
  // The launcher answers `initialize` itself and suppresses the runtime's own
  // answer, so the runtime-side `initialize` stamp is invisible to a host wired
  // through the package MCP configs. This drives the packaged launcher as
  // a process — the only path the plugin configures — and pins the claim the
  // CHANGELOG and the shipped grounding skills make: the version is known at
  // handshake, before the first tool call.
  const version = await readPluginVersion();
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const initialize = {
    jsonrpc: "2.0",
    id: "initialize",
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "plugin-static", version: "1" },
    },
  };
  const packagedInitializeResult = (env, cwd) => {
    const result = spawnSync(process.execPath, [launcher], {
      cwd,
      env,
      input: `${JSON.stringify(initialize)}\n`,
      encoding: "utf8",
      timeout: 5000,
    });
    assert.equal(result.status, 0, result.stderr);
    const response = JSON.parse(result.stdout.trim().split(/\r?\n/u)[0]);
    assert.equal(response.id, "initialize");
    return response.result;
  };

  // The documented `CODESTORY_CLI` override: a runtime is available and the
  // launcher is about to hand the session off, yet the handshake it already
  // answered is still the only stamp the host will ever see.
  const overrideDataDir = await mkdtemp(join(tmpdir(), "codestory-handshake-stamp-"));
  const overrideBinDir = await mkdtemp(join(tmpdir(), "codestory-handshake-stamp-bin-"));
  try {
    const cliPath = await writeNodeCli(
      overrideBinDir,
      [
        "const args = process.argv.slice(2);",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] === 'serve') { setInterval(() => {}, 1000); }",
        "else process.exit(2);",
      ].join("\n"),
    );
    const cliSha256 = createHash("sha256").update(await readFile(cliPath)).digest("hex");
    const overrideResult = packagedInitializeResult({
      ...process.env,
      CODESTORY_CLI: cliPath,
      PLUGIN_DATA: overrideDataDir,
      TEST_CODESTORY_VERSION: version,
    }, overrideDataDir);

    const stamp = overrideResult._meta?.codestory_publication;
    assert.ok(
      stamp,
      "the packaged handshake must carry _meta.codestory_publication, not only _meta.codestory_protocol",
    );
    assert.deepEqual(stamp, {
      schema_version: 3,
      minimum_compatible_schema_version: 3,
      served_from: "contract_only",
      publication: null,
      core_publication: null,
      retrieval_publication: null,
      contract_runtime: {
        cli_version: version,
        plugin_version: version,
        plugin_cli_version: version,
        cli_sha256: cliSha256,
        cli_source: "local_dev_override",
        pinned_pair_matches: true,
        known_override_skew_channel: true,
      },
      operation: { operation_id: null, attempt: null },
    });
    // The launcher's own fail-closed reader must accept the launcher's own
    // handshake: a stamp the pinned reader would refuse is not a stamp.
    assert.equal(launcherTest.publicationStampSkew(stamp), null);
    assert.equal(overrideResult._meta.codestory_protocol.negotiated, "2024-11-05");
  } finally {
    await rm(overrideDataDir, { recursive: true, force: true });
    await rm(overrideBinDir, { recursive: true, force: true });
  }

  // Fail-open, no runtime at all: the host still learns the response contract
  // it is talking to instead of reading an unstamped legacy v0 handshake.
  const failOpenDataDir = await mkdtemp(join(tmpdir(), "codestory-handshake-stamp-failopen-"));
  try {
    const failOpenResult = packagedInitializeResult({
      PLUGIN_DATA: "",
      COPILOT_PLUGIN_DATA: "",
      CODESTORY_PLUGIN_DATA: failOpenDataDir,
      CODESTORY_PLUGIN_DISABLE_PROVISION: "1",
      PATH: "",
      ComSpec: process.env.ComSpec || process.env.COMSPEC || "",
    }, repoRoot);

    const stamp = failOpenResult._meta?.codestory_publication;
    assert.ok(stamp, "the fail-open handshake must carry the stamp too");
    assert.equal(stamp.schema_version, 3);
    assert.equal(stamp.minimum_compatible_schema_version, 3);
    assert.equal(stamp.served_from, "contract_only");
    assert.equal(launcherTest.publicationStampSkew(stamp), null);
    assert.equal(stamp.contract_runtime.cli_source, "managed_unavailable");
    assert.equal(stamp.contract_runtime.cli_version, null);
    assert.equal(
      stamp.contract_runtime.pinned_pair_matches,
      null,
      "an unresolved CLI cannot be reported as a failed pin",
    );
    assert.equal(stamp.contract_runtime.known_override_skew_channel, false);
  } finally {
    await rm(failOpenDataDir, { recursive: true, force: true });
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
        `            protocolVersion: process.env.TEST_PROTOCOL_VERSION || '2025-03-26',`,
        "            capabilities: {},",
        "            serverInfo: { name: 'codestory', version },",
        `            _meta: { codestory_protocol: { discovery_contract_sha256: ${JSON.stringify(discoveryDigest("2025-03-26"))} }, codestory_publication: { schema_version: Number(process.env.TEST_STAMP_SCHEMA_VERSION || '3'), minimum_compatible_schema_version: 3 } },`,
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

test("CODESTORY_CLI override that publishes an unreadable wire contract is refused at session runtime", async () => {
  // ARCH-035: the launcher answers `initialize` itself and suppresses the
  // runtime's answer, so the runtime's own compatibility claim reaches nobody
  // else. Under `CODESTORY_CLI` there is no pinned pair, no archive digest and
  // no catalog drift check left — this stamp comparison is the whole detector.
  const version = await readPluginVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-wire-skew-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const cliScript = join(dataDir, "skewed-codestory-cli.cjs");
  const cliPath = join(
    dataDir,
    process.platform === "win32" ? "skewed-codestory-cli.cmd" : "skewed-codestory-cli",
  );
  const servedFile = join(dataDir, "runtime-served.txt");
  let child;

  try {
    await writeFile(
      cliScript,
      [
        "const fs = require('node:fs');",
        "const args = process.argv.slice(2);",
        "if (args[0] === '--version') { console.log('codestory-cli ' + process.env.TEST_CODESTORY_VERSION); process.exit(0); }",
        "if (args[0] !== 'serve') process.exit(2);",
        "let buffer = '';",
        "process.stdin.setEncoding('utf8');",
        // Outlive the launcher closing our stdin so the delayed reply below is
        // actually produced; the launcher must refuse to relay it.
        "process.stdin.on('end', () => setTimeout(() => process.exit(0), 400));",
        "process.stdin.on('data', (chunk) => {",
        "  buffer += chunk;",
        "  const lines = buffer.split(/\\r?\\n/u);",
        "  buffer = lines.pop() || '';",
        "  for (const line of lines) {",
        "    if (!line.trim()) continue;",
        "    const request = JSON.parse(line);",
        "    if (request.method === 'initialize') {",
        `      process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { protocolVersion: '2025-03-26', capabilities: {}, serverInfo: { name: 'codestory', version: '0' }, _meta: { codestory_protocol: { discovery_contract_sha256: ${JSON.stringify(discoveryDigest("2025-03-26"))} }, codestory_publication: { schema_version: 1 } } } }) + '\\n');`,
        "    } else if (request.method === 'tools/list') {",
        // Answer in a later chunk so the reply lands after the launcher has
        // already refused this runtime: the relay must stay shut, not just
        // drop the rest of the chunk that carried the initialize frame.
        "      setTimeout(() => {",
        "        fs.writeFileSync(process.env.TEST_SERVED, 'runtime-answered');",
        "        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result: { tools: [{ name: 'skewed-runtime' }] } }) + '\\n');",
        "      }, 50);",
        "    }",
        "  }",
        "});",
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
      cwd: dataDir,
      env: {
        ...process.env,
        CODESTORY_CLI: cliPath,
        PLUGIN_DATA: dataDir,
        TEST_CODESTORY_VERSION: version,
        TEST_SERVED: servedFile,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    const pending = new Map();
    const received = new Map();
    const hostFrames = [];
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      for (;;) {
        const newline = stdout.indexOf("\n");
        if (newline < 0) break;
        const line = stdout.slice(0, newline).trim();
        stdout = stdout.slice(newline + 1);
        if (!line) continue;
        hostFrames.push(line);
        const response = JSON.parse(line);
        if (response.id === undefined) continue;
        if (pending.has(response.id)) pending.get(response.id)(response);
        else received.set(response.id, response);
      }
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    const sendRequest = async (request) => {
      const waiter = received.has(request.id)
        ? Promise.resolve(received.get(request.id))
        : new Promise((resolve) => pending.set(request.id, resolve));
      child.stdin.write(`${JSON.stringify(request)}\n`);
      return Promise.race([
        waiter,
        new Promise((_, reject) => setTimeout(
          () => reject(new Error(`timed out waiting for ${request.id}: ${stderr}`)),
          8000,
        )),
      ]);
    };

    const init = await sendRequest({
      jsonrpc: "2.0",
      id: "init",
      method: "initialize",
      params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "t", version: "1" } },
    });
    assert.equal(
      init.result.protocolVersion,
      "2025-03-26",
      "the launcher must answer with the negotiated revision",
    );
    assert.deepEqual(init.result._meta.codestory_protocol, {
      requested: "2025-03-26",
      negotiated: "2025-03-26",
      supported: ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"],
      preferred: preferredRevision,
      status: "agreed",
      compatible: true,
      discovery_contract_sha256: discoveryDigest("2025-03-26"),
    });

    const delegated = await sendRequest({ jsonrpc: "2.0", id: "tools", method: "tools/list" });
    assert.equal(
      delegated.result,
      undefined,
      `a skewed runtime must not serve a delegated request: ${JSON.stringify(delegated)}`,
    );
    assert.equal(delegated.error.code, -32000);
    assert.match(delegated.error.message, /reason_code=runtime_wire_contract_skew/u);
    assert.match(delegated.error.message, /error_code=publication_schema_skew/u);

    const diagnostic = await sendRequest({
      jsonrpc: "2.0",
      id: "status",
      method: "resources/read",
      params: { uri: launcherTest.projectBoundResourceUri("codestory://status", dataDir) },
    });
    const status = JSON.parse(diagnostic.result.contents[0].text);
    assert.equal(status.degraded_reason, "runtime_wire_contract_skew");
    assert.equal(status.readiness[0].status, "unavailable");
    assert.equal(status.allowed_surfaces.ground.allowed, false);

    // The refused runtime did produce a `tools/list` answer — the request was
    // already in flight when its initialize frame arrived. The contract is that
    // none of it reaches the host.
    for (let waited = 0; waited < 2000 && !fs.existsSync(servedFile); waited += 25) {
      await delay(25);
    }
    await delay(100);
    assert.equal(fs.existsSync(servedFile), true, "the fixture must have produced a reply to suppress");
    assert.equal(
      hostFrames.some((frame) => frame.includes("skewed-runtime")),
      false,
      `a refused runtime's output must never reach the host: ${hostFrames.join("\n")}`,
    );
  } finally {
    if (child) {
      child.stdin.end();
      await Promise.race([once(child, "exit"), delay(2000)]);
      if (child.exitCode === null && child.signalCode === null) child.kill();
    }
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
  const pluginVersion = await readPluginVersion();
  const cliVersion = await readPinnedCliVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-failopen-managed-"));
  const cliDir = join(dataDir, "codestory-cli", cliVersion);
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
        cliVersion,
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
      pluginVersion,
    );
  } finally {
    await stopChildProcess(child);
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("mcp launcher upgrades a verified prior managed cli to the checksummed release", async () => {
  const pluginVersion = await readPluginVersion();
  const cliVersion = await readPinnedCliVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-provisioned-cli-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-release-"));
  const outFile = join(dataDir, "env.json");
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const { archiveBase, archiveName } = releaseAssetForPlatform(cliVersion);
  const stageDir = join(releaseDir, archiveBase);
  const cliName = process.platform === "win32" ? "codestory-cli.cmd" : "codestory-cli";
  const cliPath = join(stageDir, cliName);
  const archivePath = join(releaseDir, archiveName);

  try {
    const priorVersion = "0.15.0";
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
      TEST_CODESTORY_VERSION: cliVersion,
    });
    const result = await launched.completed;

    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(await readFile(outFile, "utf8"));
    assert.equal(observed.source, "managed");
    assert.equal(observed.version, cliVersion);
    assert.equal(observed.repoRef, "");
    assert.equal(observed.buildSource, "explicit_package");
    assert.equal(observed.archiveSha256, archiveSha256);
    assert.notEqual(observed.path, priorCli);
    const retention = JSON.parse(observed.retention);
    assert.equal(retention.active_version, cliVersion);
    assert.equal(
      retention.retained.some((entry) => entry.version === priorVersion && entry.reason === "rollback"),
      true,
      JSON.stringify(retention),
    );
    assert.match(
      observed.path,
      new RegExp(String.raw`codestory-cli[\\/]+${cliVersion.replaceAll(".", String.raw`\.`)}[\\/]codestory-cli`, "u"),
    );
    assert.deepEqual(observed.args, ["serve", "--stdio", "--multi-project", "--refresh", "none"]);

    const manifest = JSON.parse(
      await readFile(join(dataDir, "codestory-cli", cliVersion, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.version, cliVersion);
    assert.equal(manifest.repo_ref, null);
    assert.equal(manifest.build_source, "explicit_package");
    assert.equal(manifest.archive, archiveName);
    assert.equal(manifest.archive_url, `explicit-package:${archiveSha256}`);
    assert.equal(manifest.archive_sha256, archiveSha256);
    assert.equal(manifest.stdio_initialize_verified, true);
    assert.equal(typeof manifest.sha256, "string");
    const runtime = JSON.parse(
      await readFile(join(dataDir, ".codestory-mcp-runtime.json"), "utf8"),
    );
    assert.equal(runtime.pluginVersion, pluginVersion);
    assert.equal(runtime.cliVersion, cliVersion);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

test("mcp launcher serves diagnostics while managed provisioning runs, then hands off", { timeout: 30000 }, async () => {
  const { createServer } = await import("node:http");
  const pluginVersion = await readPluginVersion();
  const cliVersion = await readPinnedCliVersion();
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-background-provision-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-background-release-"));
  const launcher = join(pluginRoot, "scripts", "codestory-mcp.cjs");
  const outFile = join(dataDir, "runtime.json");
  let child;
  let server;
  let releaseAssets = () => {};
  try {
    const fixture = await writeReleaseFixture(releaseDir, cliVersion, writeLifecycleCli);
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
        TEST_CODESTORY_VERSION: cliVersion,
        CODESTORY_TEST_PROBE_DELAY_MS: "1500",
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
    assert.equal(coldTools.result.tools.length, 21);
    assert.ok(coldTools.result.tools.some((tool) => tool.name === "ground"));
    assert.equal(
      coldTools.result.tools.filter((tool) => tool.name === "verify_indexed_direct_calls").length,
      1,
    );
    assert.equal(coldTools.result.tools.filter((tool) => tool.name === "prove_call_path").length, 0);
    const coldGround = await request({
      jsonrpc: "2.0",
      id: "cold-ground",
      method: "tools/call",
      params: { name: "ground", arguments: { project: repoRoot } },
    });
    assert.equal(coldGround.result.isError, false);
    const coldGroundPreparing = toolTextJson(coldGround);
    assert.equal(coldGround.result.structuredContent, undefined);
    assert.deepEqual(
      Object.keys(coldGroundPreparing).sort(),
      ["kind", "operation", "retry_after_ms", "state"],
    );
    assert.equal(coldGroundPreparing.kind, "preparing");
    assert.equal(coldGroundPreparing.state, "preparing");
    const { progress, ...operationCore } = coldGroundPreparing.operation;
    // The gated release server withholds every asset byte here, so no transfer is measurable and
    // the retry hint must be the documented no-signal fallback.
    assert.deepEqual(operationCore, {
      operation_id: "managed-runtime-provisioning",
      state: "preparing",
      stage: "downloading_runtime",
      attempt: 1,
      retry_after_ms: launcherTest.provisioningRetryHintFallbackMs,
      failure: null,
    });
    assert.equal(
      coldGroundPreparing.retry_after_ms,
      coldGroundPreparing.operation.retry_after_ms,
    );
    // Progress appears once a release asset fetch is in flight; the request can land just before
    // the background provisioner gets that far, so null is the only other legal value.
    if (progress !== null) {
      assert.deepEqual(
        Object.keys(progress).sort(),
        ["asset", "percent", "received_bytes", "total_bytes"],
      );
      assert.equal(typeof progress.received_bytes, "number");
    }
    releaseAssets();
    const managedRoot = join(dataDir, "codestory-cli");
    const probeDeadline = Date.now() + 5000;
    while (Date.now() < probeDeadline) {
      const entries = await readdir(managedRoot).catch(() => []);
      if (entries.some((entry) => entry.startsWith(`.provisioning-${cliVersion}-`))) break;
      await delay(10);
    }
    assert.ok(
      (await readdir(managedRoot)).some((entry) => entry.startsWith(`.provisioning-${cliVersion}-`)),
      "provisioning should reach its deliberately slow synchronous version probe",
    );
    const responsiveStartedAt = Date.now();
    const duringSlowProbe = await request({
      jsonrpc: "2.0",
      id: "during-slow-probe",
      method: "resources/read",
      params: { uri: statusUri },
    });
    assert.ok(Date.now() - responsiveStartedAt < 1000, "preparing relay must stay responsive");
    assert.equal(
      JSON.parse(duringSlowProbe.result.contents[0].text).degraded_reason,
      "managed_cli_provisioning",
    );
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
    assert.equal(publishedRuntime.pluginVersion, pluginVersion);
    assert.equal(publishedRuntime.cliVersion, cliVersion);
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
  const version = await readPinnedCliVersion();
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
    "run(status,{shouldHandoff:()=>!failed,startRuntime:()=>{const child=new EventEmitter();child.stdin=new PassThrough();child.stdout=new PassThrough();child.stderr=new PassThrough();process.nextTick(()=>{const error=new Error('unlabeled private query');error.code='EACCES';child.emit('error',error)});return child},onRuntimeFailure:(failure)=>{failed=true;process.stderr.write(JSON.stringify(failure))}});",
  ].join("");
  const child = spawn(process.execPath, ["-e", fixture], { stdio: ["pipe", "pipe", "pipe"] });
  const completed = once(child, "close");
  let output = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
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
  assert.doesNotMatch(output, /unlabeled private query/u);
  assert.doesNotMatch(stderr, /unlabeled private query/u);
  const failure = JSON.parse(stderr);
  assert.equal(failure.spawnError, true);
  assert.equal(failure.errorCode, "EACCES");
  assert.equal(failure.reasonCode, "runtime_stdio_child_spawn");
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
  const resultValue = (id) => toolTextJson(responses.find((response) => response.id === id));
  failures.forEach(([reason, failure], index) => {
    const structured = resultValue(index + 1);
    assert.equal(structured.degraded_reason, reason);
    assert.equal(structured.failure, failure);
    assert.equal(structured.current_operation, null);
  });
  const noProject = resultValue(failures.length + 1);
  assert.equal(noProject.code, "project_required");
  assert.equal(noProject.state, "no_project");
  assert.equal(noProject.degraded_reason, undefined);
  assert.equal(noProject.diagnostics_uri, undefined);
  const unavailableProject = resultValue(failures.length + 2);
  assert.equal(unavailableProject.code, "project_unavailable");
  assert.equal(unavailableProject.state, "unavailable");
  assert.equal(unavailableProject.diagnostics_uri, undefined);
});

test("managed cli publication is single-flight and atomically visible across two processes", { timeout: 30000 }, async () => {
  const { createServer } = await import("node:http");
  const version = await readPinnedCliVersion();
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
  const version = await readPinnedCliVersion();
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

// The plugin ships without scripts/, so the launcher carries its own copy of the release-manifest
// schema. Two copies of a contract drift, so the shipped one is held against the generator's.
function releaseManifestFixture(version, target, archiveName, archiveBytes, archiveSha256) {
  const filler = (filename) => ({ filename, bytes: 1024, sha256: "e".repeat(64) });
  const archives = {
    "macos-arm64": filler(`codestory-cli-v${version}-macos-arm64.tar.gz`),
    "linux-x64": filler(`codestory-cli-v${version}-linux-x64.tar.gz`),
    "windows-x64": filler(`codestory-cli-v${version}-windows-x64.zip`),
  };
  archives[target] = { filename: archiveName, bytes: archiveBytes, sha256: archiveSha256 };
  return buildReleaseManifest({
    version,
    tag: `v${version}`,
    commit: "a".repeat(40),
    archives,
  });
}

function releaseFixtureTarget(version) {
  const { archiveName } = releaseAssetForPlatform(version);
  return {
    archiveName,
    target: archiveName
      .slice(`codestory-cli-v${version}-`.length)
      .replace(/\.(?:zip|tar\.gz)$/u, ""),
  };
}

async function writeReleaseManifestFile(releaseDir, manifest) {
  await writeFile(
    join(releaseDir, RELEASE_MANIFEST_ASSET),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
}

test("the launcher's release-manifest schema matches the generator's", async () => {
  const version = await readPluginVersion();
  const { archiveName, target } = releaseFixtureTarget(version);
  assert.equal(launcherTest.RELEASE_MANIFEST_ASSET, RELEASE_MANIFEST_ASSET);
  assert.equal(launcherTest.RELEASE_MANIFEST_DOMAIN, RELEASE_MANIFEST_DOMAIN);
  assert.equal(launcherTest.RELEASE_MANIFEST_SCHEMA_VERSION, RELEASE_MANIFEST_SCHEMA_VERSION);
  const manifest = releaseManifestFixture(version, target, archiveName, 2048, "b".repeat(64));
  assert.deepEqual(launcherTest.releaseManifestArchiveEntry(manifest, version, target), {
    filename: archiveName,
    bytes: 2048,
    sha256: "b".repeat(64),
  });
  // A manifest that is well formed for a DIFFERENT release describes intact bytes of the wrong
  // release, which is the substitution the identity binding exists to refuse.
  assert.throws(
    () => launcherTest.releaseManifestArchiveEntry(manifest, "0.0.1", target),
    /release_manifest_invalid:release_identity/u,
  );
  for (const [mutate, reason] of [
    [(m) => ({ ...m, domain: "codestory.something-else" }), /domain/u],
    [(m) => ({ ...m, schema_version: 2 }), /schema_version/u],
    [(m) => ({ ...m, commit: "not-a-commit" }), /commit/u],
    [(m) => ({ ...m, archives: { ...m.archives, [target]: undefined } }), /target/u],
    [
      (m) => ({ ...m, archives: { ...m.archives, [target]: { ...m.archives[target], bytes: 0 } } }),
      /bytes/u,
    ],
    [
      (m) => ({
        ...m,
        archives: { ...m.archives, [target]: { ...m.archives[target], sha256: "nope" } },
      }),
      /sha256/u,
    ],
  ]) {
    assert.throws(
      () => launcherTest.releaseManifestArchiveEntry(mutate(manifest), version, target),
      reason,
    );
  }
});

test("managed provisioning binds the archive to the release manifest before extraction", { timeout: 30000 }, async () => {
  const version = await readPluginVersion();
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousTestVersion = process.env.TEST_CODESTORY_VERSION;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-manifest-bind-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-manifest-release-"));
  try {
    const fixture = await writeReleaseFixture(releaseDir, version);
    const { target } = releaseFixtureTarget(version);
    const archiveBytes = (await stat(fixture.archivePath)).size;
    process.env.CODESTORY_PLUGIN_RELEASE_DIR = releaseDir;
    process.env.TEST_CODESTORY_VERSION = version;

    await writeReleaseManifestFile(
      releaseDir,
      releaseManifestFixture(version, target, fixture.archiveName, archiveBytes, fixture.archiveSha256),
    );
    const warnings = [];
    const resolved = await launcherTest.provisionManagedCli(dataDir, version, warnings);
    assert.ok(resolved.path);
    assert.equal(warnings.includes("managed_cli_publication:release_manifest_bound"), true, warnings.join(","));
    const published = JSON.parse(
      await readFile(join(dataDir, "codestory-cli", version, "manifest.json"), "utf8"),
    );
    assert.equal(published.archive_sha256, fixture.archiveSha256);
    assert.equal(published.archive_bytes, archiveBytes);
  } finally {
    if (previousReleaseDir === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    else process.env.CODESTORY_PLUGIN_RELEASE_DIR = previousReleaseDir;
    if (previousTestVersion === undefined) delete process.env.TEST_CODESTORY_VERSION;
    else process.env.TEST_CODESTORY_VERSION = previousTestVersion;
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

// Ordering, not just refusal. The archive here is unextractable rubbish whose SHA256SUMS entry is
// honest, so whichever check runs first names the failure: a manifest mismatch means the binding
// ran BEFORE extraction, and the matching-manifest control proves the same bytes really do blow up
// in the extractor, so the first assertion is not passing for some unrelated reason.
test("a disagreeing release manifest stops provisioning before the archive is extracted", { timeout: 30000 }, async () => {
  const version = await readPluginVersion();
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousTestVersion = process.env.TEST_CODESTORY_VERSION;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-manifest-order-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-manifest-order-release-"));
  try {
    const { archiveName, target } = releaseFixtureTarget(version);
    const rubbish = Buffer.from("this is not an archive\n".repeat(64), "utf8");
    const archivePath = join(releaseDir, archiveName);
    await writeFile(archivePath, rubbish);
    const archiveSha256 = createHash("sha256").update(rubbish).digest("hex");
    await writeFile(join(releaseDir, "SHA256SUMS.txt"), `${archiveSha256}  ${archiveName}\n`, "utf8");
    process.env.CODESTORY_PLUGIN_RELEASE_DIR = releaseDir;
    process.env.TEST_CODESTORY_VERSION = version;

    await writeReleaseManifestFile(
      releaseDir,
      releaseManifestFixture(version, target, archiveName, rubbish.length, "c".repeat(64)),
    );
    await assert.rejects(
      () => launcherTest.provisionManagedCli(dataDir, version, []),
      new RegExp(`release_manifest_archive_mismatch:${archiveName.replace(/\./gu, "\\.")}:sha256`, "u"),
    );

    await writeReleaseManifestFile(
      releaseDir,
      releaseManifestFixture(version, target, archiveName, rubbish.length + 1, archiveSha256),
    );
    await assert.rejects(
      () => launcherTest.provisionManagedCli(dataDir, version, []),
      new RegExp(`release_manifest_archive_mismatch:${archiveName.replace(/\./gu, "\\.")}:bytes`, "u"),
    );

    // Control: with a manifest that agrees, the same bytes reach the extractor and fail there.
    await writeReleaseManifestFile(
      releaseDir,
      releaseManifestFixture(version, target, archiveName, rubbish.length, archiveSha256),
    );
    await assert.rejects(
      () => launcherTest.provisionManagedCli(dataDir, version, []),
      (error) => {
        assert.equal(/release_manifest/u.test(error.message), false, error.message);
        return true;
      },
    );
    assert.equal(fs.existsSync(join(dataDir, "codestory-cli", version)), false);
  } finally {
    if (previousReleaseDir === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    else process.env.CODESTORY_PLUGIN_RELEASE_DIR = previousReleaseDir;
    if (previousTestVersion === undefined) delete process.env.TEST_CODESTORY_VERSION;
    else process.env.TEST_CODESTORY_VERSION = previousTestVersion;
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

// The manifest is read with the checksum file, before the archive transfer. Proving that costs
// nothing extra: the release directory here has no archive at all, so whichever fetch runs first
// names the failure. A release-identity refusal means the manifest was read before the download
// was attempted; an asset-fetch failure would mean it was not.
test("a manifest for another release is refused before the archive is downloaded", { timeout: 30000 }, async () => {
  const version = await readPluginVersion();
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousTestVersion = process.env.TEST_CODESTORY_VERSION;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-manifest-early-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-manifest-early-release-"));
  try {
    const { archiveName, target } = releaseFixtureTarget(version);
    await writeFile(join(releaseDir, "SHA256SUMS.txt"), `${"d".repeat(64)}  ${archiveName}\n`, "utf8");
    // A well-formed manifest, for the wrong release.
    const other = "9.9.9";
    const otherName = archiveName.replace(`v${version}`, `v${other}`);
    await writeReleaseManifestFile(
      releaseDir,
      releaseManifestFixture(other, target, otherName, 1024, "d".repeat(64)),
    );
    process.env.CODESTORY_PLUGIN_RELEASE_DIR = releaseDir;
    process.env.TEST_CODESTORY_VERSION = version;
    await assert.rejects(
      () => launcherTest.provisionManagedCli(dataDir, version, []),
      /release_manifest_invalid:release_identity/u,
    );
  } finally {
    if (previousReleaseDir === undefined) delete process.env.CODESTORY_PLUGIN_RELEASE_DIR;
    else process.env.CODESTORY_PLUGIN_RELEASE_DIR = previousReleaseDir;
    if (previousTestVersion === undefined) delete process.env.TEST_CODESTORY_VERSION;
    else process.env.TEST_CODESTORY_VERSION = previousTestVersion;
    await rm(dataDir, { recursive: true, force: true });
    await rm(releaseDir, { recursive: true, force: true });
  }
});

// Releases published before the manifest existed carry none. That is a stated gap in the
// containment -- recorded as a warning so status and doctor can see it -- and not an agreement.
test("a release with no manifest provisions with the absence recorded, not assumed away", { timeout: 30000 }, async () => {
  const version = await readPluginVersion();
  const previousReleaseDir = process.env.CODESTORY_PLUGIN_RELEASE_DIR;
  const previousTestVersion = process.env.TEST_CODESTORY_VERSION;
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-manifest-absent-"));
  const releaseDir = await mkdtemp(join(tmpdir(), "codestory-manifest-absent-release-"));
  try {
    await writeReleaseFixture(releaseDir, version);
    process.env.CODESTORY_PLUGIN_RELEASE_DIR = releaseDir;
    process.env.TEST_CODESTORY_VERSION = version;
    const warnings = [];
    const resolved = await launcherTest.provisionManagedCli(dataDir, version, warnings);
    assert.ok(resolved.path);
    assert.equal(
      warnings.some((warning) => warning.startsWith("managed_cli_publication:release_manifest_absent:")),
      true,
      warnings.join(","),
    );
    assert.equal(warnings.includes("managed_cli_publication:release_manifest_bound"), false);
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
    assert.match(context, /read and follow the loaded codestory-grounding skill/u);
    assert.match(context, /sole source of truth/u);
    assert.match(context, /adds no parallel instructions/u);
    assert.doesNotMatch(context, /Strict routing|Call status|tool_search|prove_call_path/u);
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

test("release asset downloader resumes a partial transfer instead of restarting it", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-resume-"));
  const destination = join(dataDir, "runtime.bin");
  const body = Buffer.from("0123456789abcdefghijklmnopqrstuvwxyz");
  const served = [];
  let cutFirstTransfer = true;
  const server = createServer((request, response) => {
    const range = /^bytes=(\d+)-$/u.exec(request.headers.range || "");
    const start = range ? Number(range[1]) : 0;
    served.push(start);
    if (cutFirstTransfer) {
      cutFirstTransfer = false;
      // Announce the full length, deliver a prefix, then drop the connection mid-transfer.
      response.writeHead(200, { "content-length": String(body.length) });
      response.write(body.subarray(0, 10));
      setTimeout(() => response.destroy(), 10);
      return;
    }
    if (start > 0) {
      response.writeHead(206, {
        "content-length": String(body.length - start),
        "content-range": `bytes ${start}-${body.length - 1}/${body.length}`,
      });
      response.end(body.subarray(start));
      return;
    }
    response.writeHead(200, { "content-length": String(body.length) });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/runtime`,
      destination,
      { attempts: 3, retryDelayMs: () => 1, timeoutMs: 5000 },
    );
    assert.deepEqual(await readFile(destination), body);
    // The second attempt asked to continue from the bytes already on disk rather than from zero.
    assert.deepEqual(served, [0, 10]);
    assert.equal(fs.existsSync(`${destination}.part`), false);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader keeps a resumable partial across separate runs", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-restart-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  const body = Buffer.from("the-managed-runtime-archive-payload");
  let cutFirstTransfer = true;
  const server = createServer((request, response) => {
    const range = /^bytes=(\d+)-$/u.exec(request.headers.range || "");
    const start = range ? Number(range[1]) : 0;
    if (cutFirstTransfer) {
      cutFirstTransfer = false;
      response.writeHead(200, { "content-length": String(body.length) });
      response.write(body.subarray(0, 12));
      setTimeout(() => response.destroy(), 10);
      return;
    }
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${server.address().port}/runtime`;
  try {
    // First run exhausts its single attempt and fails, standing in for an MCP restart.
    await assert.rejects(
      launcherTest.downloadFile(url, destination, { attempts: 1, timeoutMs: 5000, partialPath }),
      /download failed after 1 attempts/u,
    );
    assert.equal(fs.existsSync(destination), false);
    assert.equal(fs.statSync(partialPath).size, 12);

    // A fresh run picks the partial back up rather than re-downloading what already landed.
    await launcherTest.downloadFile(url, destination, { attempts: 1, timeoutMs: 5000, partialPath });
    assert.deepEqual(await readFile(destination), body);
    assert.equal(fs.existsSync(partialPath), false);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// Provisioning downloads into a partial under the managed CLI root but published into a temp-dir
// destination, so on any host whose temp dir is a separate mount (tmpfs /tmp, a redirected TMPDIR,
// $HOME on another volume) every completed transfer died at the rename. The other download tests
// put both paths under one mkdtemp root and cannot observe it. CI cannot either: the packaged
// proof pins TMPDIR next to the plugin data.
test("release asset publication survives a partial and destination on different filesystems", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-exdev-"));
  const destination = join(dataDir, "dest", "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  await mkdir(join(dataDir, "dest"), { recursive: true });
  await mkdir(join(dataDir, "cache"), { recursive: true });
  const body = Buffer.from("the-managed-runtime-archive-payload");
  let served = 0;
  const server = createServer((request, response) => {
    served += 1;
    const start = Number(/^bytes=(\d+)-$/u.exec(request.headers.range || "")?.[1] ?? 0);
    if (start >= body.length) {
      response.writeHead(416, { "content-range": `bytes */${body.length}` });
      response.end();
      return;
    }
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${server.address().port}/runtime`;
  const realRename = fs.renameSync;
  try {
    // Stand in for a cross-device publication: the transfer completes, the rename cannot.
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath) {
        const error = new Error("EXDEV: cross-device link not permitted");
        error.code = "EXDEV";
        throw error;
      }
      return realRename(from, to);
    };
    await launcherTest.downloadFile(url, destination, {
      attempts: 3,
      timeoutMs: 5000,
      retryDelayMs: () => 1,
      partialPath,
    });
    assert.deepEqual(await readFile(destination), body);
    assert.equal(fs.existsSync(partialPath), false);
    // The payload transfers once. Before the fix each publication failure was classified as a
    // retryable network error and re-downloaded the whole asset.
    assert.equal(served, 1);
  } finally {
    fs.renameSync = realRename;
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset publication atomically replaces a retained destination", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-existing-destination-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const completed = Buffer.from("new-completed-runtime-payload");
  await writeFile(partialPath, completed);
  await writeFile(destination, "retained-old-runtime-payload");
  const realRename = fs.renameSync;
  let renameAttempts = 0;
  try {
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath && String(to) === destination) {
        renameAttempts += 1;
        assert.equal(fs.readFileSync(destination, "utf8"), "retained-old-runtime-payload");
      }
      return realRename(from, to);
    };

    launcherTest.publishDownloadedFile(partialPath, destination);

    assert.equal(renameAttempts, 1);
    assert.deepEqual(await readFile(destination), completed);
    assert.equal(fs.existsSync(partialPath), false);
  } finally {
    fs.renameSync = realRename;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset publication reuses identical retained bytes without renaming", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-existing-reuse-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const completed = Buffer.from("same-completed-runtime-payload");
  await writeFile(partialPath, completed);
  await writeFile(destination, completed);
  const realRename = fs.renameSync;
  let renameAttempts = 0;
  try {
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath && String(to) === destination) {
        renameAttempts += 1;
        assert.fail("identical retained bytes should be reused without a rename");
      }
      return realRename(from, to);
    };

    launcherTest.publishDownloadedFile(partialPath, destination);

    assert.equal(renameAttempts, 0);
    assert.deepEqual(await readFile(destination), completed);
    assert.equal(fs.existsSync(partialPath), false);
  } finally {
    fs.renameSync = realRename;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("a transient retained-destination lock preserves both completed files for retry", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-existing-lock-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const completed = Buffer.from("new-completed-runtime-payload");
  await writeFile(partialPath, completed);
  await writeFile(destination, "retained-old-runtime-payload");
  const realRename = fs.renameSync;
  let locked = true;
  let renameAttempts = 0;
  try {
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath && String(to) === destination) {
        renameAttempts += 1;
      }
      if (locked && String(from) === partialPath && String(to) === destination) {
        const error = new Error("synthetic retained destination lock");
        error.code = "EBUSY";
        throw error;
      }
      return realRename(from, to);
    };

    let failure;
    try {
      launcherTest.publishDownloadedFile(partialPath, destination);
      assert.fail("the retained destination lock should fail publication");
    } catch (error) {
      failure = error;
    }
    assert.equal(failure.downloadKind, "publish");
    assert.equal(failure.publishRetryable, true);
    assert.equal(launcherTest.downloadFailurePermanent(failure), false);
    assert.deepEqual(await readFile(partialPath), completed);
    assert.equal(await readFile(destination, "utf8"), "retained-old-runtime-payload");

    locked = false;
    launcherTest.publishDownloadedFile(partialPath, destination);
    assert.equal(renameAttempts, 2);
    assert.deepEqual(await readFile(destination), completed);
    assert.equal(fs.existsSync(partialPath), false);
  } finally {
    fs.renameSync = realRename;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("cross-device overwrite failure preserves the old destination and completed partial", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-exdev-existing-lock-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const completed = Buffer.from("new-completed-runtime-payload");
  await writeFile(partialPath, completed);
  await writeFile(destination, "retained-old-runtime-payload");
  const realRename = fs.renameSync;
  let locked = true;
  let stagingAttempts = 0;
  try {
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath && String(to) === destination) {
        const error = new Error("synthetic cross-device boundary");
        error.code = "EXDEV";
        throw error;
      }
      if (String(to) === destination && String(from).startsWith(`${destination}.publish-`)) {
        stagingAttempts += 1;
        if (locked) {
          const error = new Error("synthetic retained destination lock");
          error.code = "EBUSY";
          throw error;
        }
      }
      return realRename(from, to);
    };

    let failure;
    try {
      launcherTest.publishDownloadedFile(partialPath, destination);
      assert.fail("the staging overwrite lock should fail publication");
    } catch (error) {
      failure = error;
    }
    assert.equal(failure.downloadKind, "publish");
    assert.equal(failure.publishRetryable, true);
    assert.deepEqual(await readFile(partialPath), completed);
    assert.equal(await readFile(destination, "utf8"), "retained-old-runtime-payload");
    assert.equal(
      fs.readdirSync(dataDir).some((name) => name.includes(".publish-")),
      false,
    );

    locked = false;
    launcherTest.publishDownloadedFile(partialPath, destination);
    assert.equal(stagingAttempts, 2);
    assert.deepEqual(await readFile(destination), completed);
    assert.equal(fs.existsSync(partialPath), false);
  } finally {
    fs.renameSync = realRename;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset publication leaves a non-regular retained destination and partial untouched", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-nonregular-destination-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  await writeFile(partialPath, "new-completed-runtime-payload");
  await mkdir(destination);
  try {
    assert.throws(
      () => launcherTest.publishDownloadedFile(partialPath, destination),
      /download_publish_failed/u,
    );
    assert.equal((await stat(destination)).isDirectory(), true);
    assert.equal(await readFile(partialPath, "utf8"), "new-completed-runtime-payload");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset publication does not replace a retained destination symlink", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-symlink-destination-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const outside = join(dataDir, "outside.bin");
  await writeFile(partialPath, "new-completed-runtime-payload");
  await writeFile(outside, "precious-retained-payload");
  await symlink(outside, destination, "file");
  try {
    assert.throws(
      () => launcherTest.publishDownloadedFile(partialPath, destination),
      /download_publish_failed:destination_not_replaceable/u,
    );
    assert.equal(fs.lstatSync(destination).isSymbolicLink(), true);
    assert.equal(await readFile(outside, "utf8"), "precious-retained-payload");
    assert.equal(await readFile(partialPath, "utf8"), "new-completed-runtime-payload");
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("cross-device publication completes short writes before advancing the input", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-short-write-"));
  const source = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "runtime.bin");
  const body = Buffer.from("short writes must not truncate a completed managed runtime archive");
  await writeFile(source, body);
  const sourceFd = fs.openSync(source, "r");
  let writeCalls = 0;
  try {
    launcherTest.copyVerifiedPartial(sourceFd, destination, {
      writeSync(fd, buffer, offset, length) {
        writeCalls += 1;
        return fs.writeSync(fd, buffer, offset, Math.min(3, length));
      },
    });
    assert.ok(writeCalls > 1);
    assert.deepEqual(await readFile(destination), body);
  } finally {
    fs.closeSync(sourceFd);
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("a publication failure is permanent instead of restarting the transfer", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-publish-"));
  const partialPath = join(dataDir, "runtime.bin.part");
  const destination = join(dataDir, "nope", "runtime.bin");
  await writeFile(partialPath, "payload");
  try {
    assert.throws(
      () => launcherTest.publishDownloadedFile(partialPath, destination),
      /download_publish_failed:ENOENT/u,
    );
    assert.equal(
      launcherTest.downloadFailurePermanent({ downloadKind: "publish" }),
      true,
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader retries a transient publish without transferring again", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-publish-retry-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = `${destination}.part`;
  const body = Buffer.from("completed-runtime-payload");
  let served = 0;
  const fakeGet = (_url, onResponse) => {
    const request = new EventEmitter();
    request.destroy = () => request;
    served += 1;
    process.nextTick(() => {
      const response = new PassThrough();
      response.statusCode = 200;
      response.headers = { "content-length": String(body.length) };
      onResponse(response);
      response.end(body);
    });
    return request;
  };
  const realRename = fs.renameSync;
  let publishAttempts = 0;
  try {
    fs.renameSync = (from, to) => {
      if (String(from) === partialPath && String(to) === destination) {
        publishAttempts += 1;
        if (publishAttempts === 1) {
          const error = new Error("synthetic transient publish lock");
          error.code = "EPERM";
          throw error;
        }
      }
      return realRename(from, to);
    };
    await launcherTest.downloadFile("https://example.invalid/runtime", destination, {
      attempts: 3,
      get: fakeGet,
      retryDelayMs: () => 1,
      timeoutMs: 5000,
    });
    assert.equal(served, 1);
    assert.equal(publishAttempts, 2);
    assert.deepEqual(await readFile(destination), body);
    assert.equal(fs.existsSync(partialPath), false);
    assert.equal(
      launcherTest.downloadFailurePermanent({ downloadKind: "publish", publishRetryable: true }),
      false,
    );
  } finally {
    fs.renameSync = realRename;
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("terminal publish failure preserves the completed partial and its typed failure kind", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-publish-retained-"));
  const destination = join(dataDir, "missing", "runtime.bin");
  const partialPath = join(dataDir, "runtime.bin.part");
  const body = Buffer.from("completed-runtime-payload");
  let served = 0;
  const fakeGet = (_url, onResponse) => {
    const request = new EventEmitter();
    request.destroy = () => request;
    served += 1;
    process.nextTick(() => {
      const response = new PassThrough();
      response.statusCode = 200;
      response.headers = { "content-length": String(body.length) };
      onResponse(response);
      response.end(body);
    });
    return request;
  };
  try {
    let failure;
    try {
      await launcherTest.downloadFile("https://example.invalid/runtime", destination, {
        attempts: 3,
        get: fakeGet,
        partialPath,
        retryDelayMs: () => 1,
        timeoutMs: 5000,
      });
      assert.fail("publish should fail when the destination parent is absent");
    } catch (error) {
      failure = error;
    }
    assert.equal(failure.downloadFailure.kind, "publish");
    assert.equal(launcherTest.sanitizeDownloadFailure(failure.downloadFailure).kind, "publish");
    assert.equal(served, 1);
    assert.deepEqual(await readFile(partialPath), body);
    assert.equal(fs.existsSync(destination), false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

// The `.part` name is the one attacker-reachable file in the provisioning path. Sizing it with
// `stat` reported the symlink target's length, so the transfer resumed by appending release bytes
// straight into whatever the link pointed at, outside the managed cache.
test("release asset downloader refuses to resume through a symlinked partial", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-partial-symlink-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  const outside = join(dataDir, "outside.txt");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  await writeFile(outside, "precious", "utf8");
  await symlink(outside, partialPath, "file");
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((request, response) => {
    const start = Number(/^bytes=(\d+)-$/u.exec(request.headers.range || "")?.[1] ?? 0);
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/runtime`,
      destination,
      { attempts: 3, retryDelayMs: () => 1, timeoutMs: 5000, partialPath },
    );
    // The planted link is dropped rather than measured or written through, so provisioning still
    // completes and the file it pointed at is untouched.
    assert.deepEqual(await readFile(destination), body);
    assert.equal(await readFile(outside, "utf8"), "precious");
    assert.equal(fs.existsSync(partialPath), false);
    assert.equal(fs.lstatSync(destination).isFile(), true);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// The stat that sizes the partial and the open that writes it are separate syscalls, so dropping a
// non-regular partial is not on its own enough: the link can be planted in between. The write must
// be refused at the descriptor.
test("release asset downloader refuses a partial swapped for a symlink after it is sized", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-partial-swap-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  const outside = join(dataDir, "outside.txt");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  await writeFile(outside, "precious", "utf8");
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((request, response) => {
    const start = Number(/^bytes=(\d+)-$/u.exec(request.headers.range || "")?.[1] ?? 0);
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  let planted = false;
  try {
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/runtime`,
      destination,
      {
        attempts: 3,
        retryDelayMs: () => 1,
        timeoutMs: 5000,
        partialPath,
        // The first progress callback fires after the partial has been sized and before the transfer
        // opens it: exactly the window a planted link would exploit.
        onProgress() {
          if (planted) return;
          planted = true;
          fs.symlinkSync(outside, partialPath, "file");
        },
      },
    );
    assert.equal(planted, true);
    assert.equal(await readFile(outside, "utf8"), "precious");
    assert.deepEqual(await readFile(destination), body);
    assert.equal(fs.lstatSync(destination).isFile(), true);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// `O_NOFOLLOW` constrains the last path component only, and a hard link is not a symlink at all:
// `lstat().isFile()` is true for one, so a hard link planted at the partial path was sized, resumed
// and appended through into the file it shares an inode with. A partial with a second name is
// never one this process created.
test("release asset downloader refuses to resume through a hard-linked partial", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-partial-hardlink-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  const outside = join(dataDir, "outside.txt");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  await writeFile(outside, "precious", "utf8");
  await link(outside, partialPath);
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((request, response) => {
    const start = Number(/^bytes=(\d+)-$/u.exec(request.headers.range || "")?.[1] ?? 0);
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/runtime`,
      destination,
      { attempts: 3, retryDelayMs: () => 1, timeoutMs: 5000, partialPath },
    );
    // The extra name is dropped rather than resumed, so the linked file keeps its own bytes and the
    // published archive is a different inode entirely — not a second name for the attacker's file.
    assert.equal(await readFile(outside, "utf8"), "precious");
    assert.deepEqual(await readFile(destination), body);
    assert.notEqual(fs.statSync(destination).ino, fs.statSync(outside).ino);
    assert.equal(fs.statSync(destination).nlink, 1);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// The same window the symlink swap uses is open to a hard link, and there `O_NOFOLLOW` does
// nothing. The descriptor itself has to be refused: the transfer fstats what it opened and writes
// only into a lone regular file.
test("release asset downloader refuses a partial hard-linked after it is sized", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-partial-linkswap-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  const outside = join(dataDir, "outside.txt");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  await writeFile(outside, "precious", "utf8");
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((request, response) => {
    const start = Number(/^bytes=(\d+)-$/u.exec(request.headers.range || "")?.[1] ?? 0);
    response.writeHead(start > 0 ? 206 : 200, {
      "content-length": String(body.length - start),
      ...(start > 0
        ? { "content-range": `bytes ${start}-${body.length - 1}/${body.length}` }
        : {}),
    });
    response.end(body.subarray(start));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  let planted = false;
  try {
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/runtime`,
      destination,
      {
        attempts: 3,
        retryDelayMs: () => 1,
        timeoutMs: 5000,
        partialPath,
        // The first callback of the first attempt sits between the sizing lstat and the open.
        onProgress() {
          if (planted) return;
          planted = true;
          fs.linkSync(outside, partialPath);
        },
      },
    );
    assert.equal(planted, true);
    // The attempt that opened the planted link wrote nothing: neither the release bytes nor a
    // truncation reached it, and the next attempt started from a partial of its own.
    assert.equal(await readFile(outside, "utf8"), "precious");
    assert.deepEqual(await readFile(destination), body);
    assert.notEqual(fs.statSync(destination).ino, fs.statSync(outside).ino);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// Between the last byte and the rename the partial is still just a name. Publication therefore
// works from a no-follow descriptor and compares it against the device/inode the transfer wrote,
// so a link swapped in at that point is refused instead of renamed into place as the "archive".
test("release asset publication refuses a partial swapped for a symlink before the rename", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-publish-symlink-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  const outside = join(dataDir, "outside.txt");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  await writeFile(outside, "precious", "utf8");
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-length": String(body.length) });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  let planted = false;
  try {
    await assert.rejects(
      launcherTest.downloadFile(
        `http://127.0.0.1:${server.address().port}/runtime`,
        destination,
        {
          attempts: 1,
          retryDelayMs: () => 1,
          timeoutMs: 5000,
          partialPath,
          onProgress(progress) {
            if (planted || progress.receivedBytes !== body.length) return;
            planted = true;
            fs.rmSync(partialPath, { force: true });
            fs.symlinkSync(outside, partialPath, "file");
          },
        },
      ),
      /download_publish_failed/u,
    );
    assert.equal(planted, true);
    assert.equal(fs.existsSync(destination), false);
    assert.equal(await readFile(outside, "utf8"), "precious");
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// A plain regular file swapped in at the partial path passes every type check there is, so type is
// not the question publication asks: it asks whether this is the file the transfer wrote. Without
// the identity comparison the foreign bytes are published as this release's archive.
test("release asset publication refuses a partial replaced by another regular file", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-publish-swap-"));
  const destination = join(dataDir, "runtime.bin");
  const partialPath = join(dataDir, "cache", "runtime.bin.part");
  await mkdir(join(dataDir, "cache"), { recursive: true });
  const body = Buffer.from("the-managed-runtime-archive-payload");
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-length": String(body.length) });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  let planted = false;
  try {
    await assert.rejects(
      launcherTest.downloadFile(
        `http://127.0.0.1:${server.address().port}/runtime`,
        destination,
        {
          attempts: 1,
          retryDelayMs: () => 1,
          timeoutMs: 5000,
          partialPath,
          onProgress(progress) {
            if (planted || progress.receivedBytes !== body.length) return;
            planted = true;
            fs.rmSync(partialPath, { force: true });
            fs.writeFileSync(partialPath, "substituted-archive-payload");
          },
        },
      ),
      /download_publish_failed:partial_identity/u,
    );
    assert.equal(planted, true);
    assert.equal(fs.existsSync(destination), false);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

// `mkdirSync({ recursive: true })` succeeds silently on an existing symlink-to-directory, and the
// no-follow open protecting the partial only constrains the final component. A symlinked version
// entry therefore needed no race at all to put every provisioning byte outside the cache.
test("download cache refuses a symlinked per-version directory", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-cache-version-symlink-"));
  const root = join(dataDir, "codestory-cli");
  const outside = join(dataDir, "outside");
  await mkdir(join(root, ".download"), { recursive: true });
  await mkdir(outside, { recursive: true });
  await symlink(outside, join(root, ".download", "0.16.1"), "dir");
  try {
    assert.throws(
      () => launcherTest.managedCliDownloadCacheDir(root, "0.16.1"),
      /managed_cli_download_cache_not_direct/u,
    );
    // Nothing was handed back, so no partial path can be built through the link.
    assert.deepEqual(await readdir(outside), []);
    // A real directory is still accepted, and lands inside the cache root.
    const usable = launcherTest.managedCliDownloadCacheDir(root, "0.16.2");
    assert.equal(usable, join(await realpath(join(root, ".download")), "0.16.2"));
    assert.equal(fs.lstatSync(usable).isDirectory(), true);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("download cache trimming refuses to delete through a symlinked cache root", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-symlink-"));
  const root = join(dataDir, "codestory-cli");
  const outside = join(dataDir, "outside");
  await mkdir(root, { recursive: true });
  await mkdir(outside, { recursive: true });
  await writeFile(join(outside, "keep.txt"), "precious");
  await symlink(outside, join(root, ".download"), "dir");
  try {
    // Both cleanup paths resolve the cache root through the same guard, so neither follows the link.
    launcherTest.trimManagedCliDownloadCache(root, "0.16.1");
    launcherTest.removeManagedCliDownloadCache(root, "0.16.1");
    assert.equal(fs.existsSync(join(outside, "keep.txt")), true);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader survives a transfer slower than the stall window", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-slow-"));
  const destination = join(dataDir, "slow.bin");
  const chunks = 8;
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-length": String(chunks) });
    let sent = 0;
    const interval = setInterval(() => {
      sent += 1;
      response.write("x");
      if (sent === chunks) {
        clearInterval(interval);
        response.end();
      }
    }, 25);
    response.on("close", () => clearInterval(interval));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    // Every chunk arrives well after a 40ms budget would have expired, but each one resets the
    // stall window, so a steady trickle now completes instead of being cut off.
    await launcherTest.downloadFile(
      `http://127.0.0.1:${server.address().port}/slow`,
      destination,
      { attempts: 1, stallTimeoutMs: 400, timeoutMs: 5000 },
    );
    assert.equal((await readFile(destination, "utf8")).length, chunks);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader fails a silent connection on the stall window", async () => {
  const { createServer } = await import("node:http");
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-stall-"));
  const destination = join(dataDir, "silent.bin");
  const server = createServer((_request, response) => {
    response.writeHead(200);
    // Headers only: never send a body byte.
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const started = Date.now();
    await assert.rejects(
      launcherTest.downloadFile(`http://127.0.0.1:${server.address().port}/silent`, destination, {
        attempts: 1,
        stallTimeoutMs: 80,
        timeoutMs: 60_000,
      }),
      /stalled after 80ms without data/u,
    );
    // The stall window, not the hour-long total budget, is what ends a dead transfer.
    assert.ok(Date.now() - started < 5000);
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("release asset downloader stops immediately on a permanent status", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codestory-download-permanent-"));
  const destination = join(dataDir, "missing.bin");
  let calls = 0;
  const fakeGet = (_url, onResponse) => {
    const request = new EventEmitter();
    request.destroy = () => request;
    calls += 1;
    process.nextTick(() => {
      const response = new PassThrough();
      response.statusCode = 404;
      response.headers = {};
      onResponse(response);
      response.end("");
    });
    return request;
  };
  try {
    await assert.rejects(
      launcherTest.downloadFile("https://example.invalid/missing", destination, {
        attempts: 5,
        get: fakeGet,
        retryDelayMs: () => 1,
        timeoutMs: 5000,
      }),
      /download failed 404/u,
    );
    // A missing release asset is a fixed answer; retrying it only delays the real error.
    assert.equal(calls, 1);
    assert.equal(fs.existsSync(`${destination}.part`), false);
  } finally {
    await rm(dataDir, { recursive: true, force: true });
  }
});

test("download failure hints stay structured and actionable", () => {
  const stalled = launcherTest.sanitizeDownloadFailure({
    kind: "stalled",
    asset: "codestory-cli-v0.16.1-x86_64-apple-darwin.tar.gz",
    resumable_bytes: 52_428_800,
    elapsed_ms: 900_000,
    attempts: 6,
  });
  assert.equal(stalled.kind, "stalled");
  assert.equal(stalled.http_status, null);
  const stalledHint = launcherTest.managedCliDownloadHint(stalled, "managed_cli_asset_fetch_failed");
  assert.match(stalledHint, /50\.0 MB already downloaded is kept/u);
  assert.match(stalledHint, /CODESTORY_PLUGIN_DOWNLOAD_TIMEOUT_MS/u);

  const missing = launcherTest.sanitizeDownloadFailure({ kind: "http_status", http_status: 404 });
  assert.match(
    launcherTest.managedCliDownloadHint(missing, "managed_cli_asset_fetch_failed"),
    /was not found/u,
  );

  // Anything unrecognised collapses to a safe enum rather than echoing attacker-controlled text.
  const hostile = launcherTest.sanitizeDownloadFailure({
    kind: "C:\\private\\candidate.exe",
    asset: "untrusted detail\nsecond line",
    http_status: 99_999,
    resumable_bytes: -5,
  });
  assert.deepEqual(hostile, {
    kind: "network",
    asset: null,
    http_status: null,
    resumable_bytes: 0,
    elapsed_ms: 0,
    attempts: 0,
  });

  // A failure that is not a download failure must not be described as one.
  assert.equal(launcherTest.managedCliDownloadHint(null, "managed_cli_probe_failed"), null);
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
  const cursorHookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "cursor-hooks.json"), "utf8"),
  );
  const hostManifest = join(pluginRoot, ".claude-plugin", "plugin.json");
  const hookCommands = Object.values(hookConfig.hooks)
    .flat()
    .flatMap((entry) => entry.hooks);
  const hookScript = /hooks[\\/]([\w.-]+\.(?:js|mjs|cjs|ps1|sh))/u;

  assert.equal(copilotHookConfig.hooks.sessionStart.length, 1);
  assert.equal(cursorHookConfig.version, 1);
  assert.equal(cursorHookConfig.hooks.sessionStart.length, 1);
  assert.match(cursorHookConfig.hooks.sessionStart[0].command, /codestory-activate\.cjs/u);
  assert.match(cursorHookConfig.hooks.sessionStart[0].command, /\$\{CURSOR_PLUGIN_ROOT\}/u);
  assert.doesNotMatch(cursorHookConfig.hooks.sessionStart[0].command, /\$\{PLUGIN_ROOT\}/u);
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
  const cursorManifest = JSON.parse(
    await readFile(join(pluginRoot, ".cursor-plugin", "plugin.json"), "utf8"),
  );
  assert.equal(cursorManifest.hooks, "./hooks/cursor-hooks.json");
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
  const cursorHookConfig = JSON.parse(
    await readFile(join(pluginRoot, "hooks", "cursor-hooks.json"), "utf8"),
  );
  const claudeTimeouts = Object.values(hookConfig.hooks)
    .flat()
    .flatMap((entry) => entry.hooks)
    .map((hook) => hook.timeout);
  const copilotTimeouts = copilotHookConfig.hooks.sessionStart.map((hook) => hook.timeoutSec);
  const cursorTimeouts = cursorHookConfig.hooks.sessionStart.map((hook) => hook.timeout);

  for (const timeoutSec of [...claudeTimeouts, ...copilotTimeouts, ...cursorTimeouts]) {
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
      assert.match(context, /read and follow the loaded codestory-grounding skill/u);
      assert.match(context, /sole source of truth/u);
      assert.match(context, /adds no parallel instructions/u);
      assert.doesNotMatch(context, /status first|poll status/u);
      assert.doesNotMatch(context, /truncated/u);
      assert.doesNotMatch(context, /Strict routing|tool_search|prove_call_path/u);
      assert.equal(context.endsWith("instructions."), true);
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

test("portable plugin core and thin host adapters preserve their own contracts", async () => {
  const copilotManifest = JSON.parse(
    await readFile(join(pluginRoot, ".github", "plugin", "plugin.json"), "utf8"),
  );
  assert.equal(copilotManifest.hooks, "hooks/copilot-hooks.json");
  assert.equal(copilotManifest.skills, "skills/");

  const portableManifest = JSON.parse(
    await readFile(join(pluginRoot, "plugin.json"), "utf8"),
  );
  assert.equal(
    portableManifest.$schema,
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  );
  assert.deepEqual(Object.keys(portableManifest).sort(), [
    "$schema",
    "author",
    "description",
    "homepage",
    "keywords",
    "license",
    "name",
    "repository",
    "version",
  ].sort());
  assert.deepEqual(Object.keys(portableManifest.author).sort(), ["name", "url"]);

  const cursorManifest = JSON.parse(
    await readFile(join(pluginRoot, ".cursor-plugin", "plugin.json"), "utf8"),
  );
  assert.equal(cursorManifest.name, "codestory");
  assert.equal(cursorManifest.hooks, "./hooks/cursor-hooks.json");
  assert.equal(cursorManifest.mcpServers, "./mcp.cursor.json");
  assert.equal(cursorManifest.keywords.includes("cursor"), true);
  assert.deepEqual(cursorManifest.author, { name: "The Green Cedar" });
  for (const discovered of ["skills", "rules"]) {
    assert.equal(Object.hasOwn(cursorManifest, discovered), false, discovered);
  }

  const portableMcpText = await readFile(join(pluginRoot, "mcp.json"), "utf8");
  const portableMcp = JSON.parse(portableMcpText);
  assert.equal(
    portableMcp.$schema,
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  );
  assert.deepEqual(Object.keys(portableMcp).sort(), ["$schema", "mcpServers"]);
  assert.deepEqual(portableMcp.mcpServers.codestory, {
    type: "stdio",
    command: "node",
    args: ["${PLUGIN_ROOT}/scripts/codestory-mcp.cjs"],
    cwd: "${PLUGIN_ROOT}",
    env: {},
  });
  const arbitraryProject = await mkdtemp(join(tmpdir(), "codestory-cursor-project-"));
  try {
    const expandedLauncher = portableMcp.mcpServers.codestory.args[0]
      .replaceAll("${PLUGIN_ROOT}", pluginRoot);
    const check = spawnSync(process.execPath, ["--check", expandedLauncher], {
      cwd: arbitraryProject,
      encoding: "utf8",
    });
    assert.equal(
      check.status,
      0,
      `portable MCP launcher must resolve outside the active project cwd:\n${check.stderr}`,
    );
  } finally {
    await rm(arbitraryProject, { recursive: true, force: true });
  }
  assert.doesNotMatch(
    portableMcpText,
    /CODESTORY_PLUGIN_DATA|CODESTORY_CURSOR_DOGFOOD|absolute\/path|tool_timeout_sec/iu,
  );

  const codexManifest = JSON.parse(
    await readFile(join(pluginRoot, ".codex-plugin", "plugin.json"), "utf8"),
  );
  const claudeManifest = JSON.parse(
    await readFile(join(pluginRoot, ".claude-plugin", "plugin.json"), "utf8"),
  );
  const legacyMcp = JSON.parse(await readFile(join(pluginRoot, ".mcp.json"), "utf8"));
  assert.equal(codexManifest.mcpServers, "./.mcp.json");
  assert.equal(Object.hasOwn(claudeManifest, "mcpServers"), false);
  assert.deepEqual(legacyMcp.mcpServers.codestory, {
    command: "node",
    args: ["./scripts/codestory-mcp.cjs"],
    cwd: ".",
    env: {},
    tool_timeout_sec: 300,
  });

  const dogfoodMcp = JSON.parse(
    await readFile(join(repoRoot, ".cursor", "mcp.json"), "utf8"),
  );
  assert.deepEqual(dogfoodMcp.mcpServers.codestory.env, {
    CODESTORY_CURSOR_DOGFOOD: "1",
  });
  assert.deepEqual(dogfoodMcp.mcpServers.codestory.args, [
    "${workspaceFolder}/plugins/codestory/scripts/codestory-mcp.cjs",
  ]);

  const marketplace = JSON.parse(
    await readFile(join(repoRoot, ".cursor-plugin", "marketplace.json"), "utf8"),
  );
  assert.equal(marketplace.name, "codestory");
  assert.deepEqual(marketplace.plugins.map(({ source }) => source), ["plugins/codestory"]);
  assert.equal(fs.existsSync(join(pluginRoot, ".mcp.json")), true);
  assert.equal(fs.existsSync(join(pluginRoot, "mcp.cursor.json")), true);
  assert.equal(fs.existsSync(join(pluginRoot, "scripts", "cursor-mcp-resolve.cjs")), true);
  const cursorMcp = JSON.parse(await readFile(join(pluginRoot, "mcp.cursor.json"), "utf8"));
  assert.equal(cursorMcp.mcpServers.codestory.command, "node");
  assert.equal(cursorMcp.mcpServers.codestory.args[0], "-e");
  assert.match(cursorMcp.mcpServers.codestory.args[1], /Module\.runMain\s*\(/u);
  assert.doesNotMatch(
    cursorMcp.mcpServers.codestory.args[1],
    /require\(resolveCodestoryCursorLauncher/u,
  );
  assert.equal(Object.hasOwn(cursorMcp.mcpServers.codestory, "cwd"), false);
  assert.equal(fs.existsSync(join(pluginRoot, ".cursor", "mcp.json")), false);
  assert.equal(fs.existsSync(join(pluginRoot, ".cursor", "rules", "codestory.mdc")), false);
});

test("Cursor rules point to one canonical grounding skill", async () => {
  const pluginRule = await readFile(join(pluginRoot, "rules", "codestory.mdc"), "utf8");
  const dogfoodRule = await readFile(join(repoRoot, ".cursor", "rules", "codestory.mdc"), "utf8");
  const normalize = (text) => text
    .replace(
      /\[canonical codestory-grounding skill\]\([^\n)]+\)/u,
      "[canonical codestory-grounding skill](CANONICAL_SKILL)",
    );
  assert.equal(normalize(pluginRule), normalize(dogfoodRule));
  assert.match(pluginRule, /\[canonical codestory-grounding skill\]\(\.\.\/skills\/codestory-grounding\/SKILL\.md\)/u);
  assert.match(dogfoodRule, /\[canonical codestory-grounding skill\]\(\.\.\/\.\.\/plugins\/codestory\/skills\/codestory-grounding\/SKILL\.md\)/u);
  assert.match(pluginRule, /sole source of truth.*adds no parallel instructions/isu);
  assert.match(dogfoodRule, /sole source of truth.*adds no parallel instructions/isu);
  assert.doesNotMatch(pluginRule, /Routing contract:|Discovery leads come from|prove_call_path|docs\/users\/cursor/u);
  assert.doesNotMatch(dogfoodRule, /Routing contract:|Discovery leads come from|prove_call_path|docs\/users\/cursor/u);
});

test("Cursor plugin-data inference is identity-bound and local overrides use PLUGIN_DATA", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-data-"));
  const cachedPlugin = join(
    home,
    ".cursor",
    "plugins",
    "cache",
    "TheGreenCedar",
    "codestory",
    "0.17.0",
  );
  const dataDir = join(home, ".cursor", "plugins", "data", "codestory");
  const genericEnv = {
    CODESTORY_CURSOR_DOGFOOD: "",
    CURSOR_PLUGIN_ROOT: "",
    CURSOR_PROJECT_DIR: repoRoot,
    CURSOR_VERSION: "ambient-terminal-value",
  };
  const claudeEnv = { ...genericEnv, CLAUDE_PLUGIN_ROOT: pluginRoot };
  try {
    await mkdir(cachedPlugin, { recursive: true });
    await mkdir(dataDir, { recursive: true });
    assert.equal(
      launcherTest.inferredCursorPluginDataDir(cachedPlugin, home, { env: genericEnv }),
      dataDir,
    );
    assert.equal(
      inferredCursorHookDataDir(cachedPlugin, home, { env: genericEnv }),
      dataDir,
    );
    for (const env of [genericEnv, claudeEnv]) {
      assert.equal(launcherTest.confirmedCursorIdentity(env), false);
      assert.equal(confirmedCursorHookIdentity(env), false);
      assert.equal(
        launcherTest.inferredCursorPluginDataDir(pluginRoot, home, { env }),
        null,
      );
      assert.equal(inferredCursorHookDataDir(pluginRoot, home, { env }), null);
    }

    const dogfoodEnv = { [launcherTest.cursorDogfoodMarker]: "1" };
    assert.equal(
      launcherTest.inferredCursorPluginDataDir(pluginRoot, home, { env: dogfoodEnv }),
      dataDir,
    );
    assert.equal(
      inferredCursorHookDataDir(pluginRoot, home, { env: dogfoodEnv }),
      dataDir,
    );

    const cliPath = join(home, "bin", process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli");
    await writeFile(
      join(dataDir, launcherTest.cursorLocalOverrideFileName),
      `${JSON.stringify({ schema_version: 1, CODESTORY_CLI: cliPath })}\n`,
      "utf8",
    );
    assert.deepEqual(
      launcherTest.readCursorLocalOverrides(pluginRoot, {
        env: genericEnv,
        home,
        pluginData: dataDir,
      }),
      { CODESTORY_CLI: cliPath },
    );
    assert.equal(
      launcherTest.readCursorLocalOverrides(pluginRoot, {
        env: genericEnv,
        home,
        pluginData: "",
      }),
      null,
    );
    assert.deepEqual(
      launcherTest.readCursorLocalOverrides(pluginRoot, { env: dogfoodEnv, home }),
      { CODESTORY_CLI: cliPath },
    );
    assert.deepEqual(
      launcherTest.readCursorLocalOverrides(cachedPlugin, { env: genericEnv, home }),
      { CODESTORY_CLI: cliPath },
    );

    const probeLauncher = (env) => spawnSync(
      process.execPath,
      [
        "-e",
        `const test=require(${JSON.stringify(join(pluginRoot, "scripts", "codestory-mcp.cjs"))})._test;process.stdout.write(JSON.stringify({cli:process.env.CODESTORY_CLI,data:test.pluginDataDir()}));`,
      ],
      { env: { ...process.env, ...env }, encoding: "utf8" },
    );
    const launcherProbe = probeLauncher({
      ...genericEnv,
      CODESTORY_CLI: "",
      CODESTORY_PLUGIN_DATA: "",
      COPILOT_PLUGIN_DATA: "",
      HOME: home,
      PLUGIN_DATA: dataDir,
      USERPROFILE: home,
    });
    assert.equal(launcherProbe.status, 0, launcherProbe.stderr);
    assert.deepEqual(JSON.parse(launcherProbe.stdout), { cli: cliPath, data: dataDir });

    for (const invalid of [
      { schema_version: 1, CODESTORY_CLI: "relative/codestory-cli" },
      { schema_version: 1, CODESTORY_CLI: cliPath, extra: true },
    ]) {
      await writeFile(
        join(dataDir, launcherTest.cursorLocalOverrideFileName),
        `${JSON.stringify(invalid)}\n`,
        "utf8",
      );
      assert.equal(
        launcherTest.readCursorLocalOverrides(cachedPlugin, { env: genericEnv, home }),
        null,
      );
    }
    const overridePath = join(dataDir, launcherTest.cursorLocalOverrideFileName);
    const linkedOverride = join(home, "linked-local-overrides.json");
    await writeFile(linkedOverride, `${JSON.stringify({ schema_version: 1, CODESTORY_CLI: cliPath })}\n`);
    await rm(overridePath);
    await symlink(linkedOverride, overridePath);
    assert.equal(
      launcherTest.readCursorLocalOverrides(cachedPlugin, { env: genericEnv, home }),
      null,
    );
    await rm(overridePath);
    await writeFile(
      overridePath,
      `${JSON.stringify({ schema_version: 1, CODESTORY_CLI: cliPath })}\n`,
      "utf8",
    );

    for (const env of [genericEnv, claudeEnv]) {
      const negativeLauncher = probeLauncher({
        ...env,
        CODESTORY_CLI: "",
        CODESTORY_PLUGIN_DATA: "",
        COPILOT_PLUGIN_DATA: "",
        HOME: home,
        PLUGIN_DATA: "",
        USERPROFILE: home,
      });
      assert.equal(negativeLauncher.status, 0, negativeLauncher.stderr);
      assert.deepEqual(JSON.parse(negativeLauncher.stdout), { cli: "", data: null });

      const negativeDirtyHook = spawnSync(
        process.execPath,
        [join(pluginRoot, "hooks", "codestory-dirty-hook.cjs"), "mark", "--project", repoRoot],
        {
          env: {
            ...process.env,
            ...env,
            CODESTORY_PLUGIN_DATA: "",
            COPILOT_PLUGIN_DATA: "",
            HOME: home,
            PLUGIN_DATA: "",
            USERPROFILE: home,
          },
          encoding: "utf8",
        },
      );
      assert.equal(negativeDirtyHook.status, 0, negativeDirtyHook.stderr);
      assert.equal(JSON.parse(negativeDirtyHook.stdout).status, "plugin_data_required");
    }

    const dirtyHook = spawnSync(
      process.execPath,
      [
        join(pluginRoot, "hooks", "codestory-dirty-hook.cjs"),
        "mark",
        "--project",
        repoRoot,
        "--source",
        "cursor-plugin-static",
      ],
      {
        env: {
          ...process.env,
          CODESTORY_CURSOR_DOGFOOD: "1",
          CODESTORY_PLUGIN_DATA: "",
          COPILOT_PLUGIN_DATA: "",
          HOME: home,
          PLUGIN_DATA: "",
          USERPROFILE: home,
        },
        encoding: "utf8",
      },
    );
    assert.equal(dirtyHook.status, 0, dirtyHook.stderr);
    assert.equal(JSON.parse(dirtyHook.stdout).path.startsWith(dataDir), true);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("Cursor sessionStart emits the Cursor additional_context contract", async () => {
  const home = await mkdtemp(join(tmpdir(), "codestory-cursor-hook-"));
  const dataDir = join(home, ".cursor", "plugins", "data", "codestory");
  try {
    await mkdir(dataDir, { recursive: true });
    for (const event of ["sessionStart", "SessionStart"]) {
      const output = runHookProcess(
        join(pluginRoot, "hooks", "codestory-activate.cjs"),
        { hook_event_name: event, cwd: repoRoot },
        {
          CODESTORY_PLUGIN_DATA: "",
          CURSOR_PLUGIN_ROOT: pluginRoot,
          CURSOR_PROJECT_DIR: "",
          CURSOR_VERSION: "",
          HOME: home,
          PLUGIN_DATA: dataDir,
          PATH: "",
        },
      );
      assert.deepEqual(Object.keys(output), ["additional_context"]);
      assert.match(output.additional_context, /CODESTORY GROUNDING AVAILABLE/u);
      assert.match(output.additional_context, /read and follow the loaded codestory-grounding skill/u);
    }
    const state = JSON.parse(await readFile(join(dataDir, ".codestory-active"), "utf8"));
    assert.equal(state.cwd, repoRoot);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
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
