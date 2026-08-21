const VOLATILE_KEY_CLASSES = {
  build_identity: new Set(["build_id", "build_identity"]),
  operation_id: new Set(["operation_id"]),
  packet_id: new Set(["packet_id"]),
  publication_id: new Set(["publication_id"]),
  request_id: new Set(["request_id"]),
  runtime_binary_hash: new Set(["runtime_binary_sha256", "runtime_binary_hash"]),
  source_identity: new Set(["source_head", "source_tree", "source_identity"]),
  timestamp: new Set(["created_at_epoch_ms", "updated_at_epoch_ms", "timestamp"]),
  timing: new Set(["duration_ms", "elapsed_ms", "wall_ms"]),
};

function markerFor(key, declared) {
  for (const kind of declared) {
    if (VOLATILE_KEY_CLASSES[kind]?.has(key)) return `<${kind}>`;
  }
  return null;
}

export function normalizeV2Transcript(value, normalization) {
  const declared = new Set(normalization?.volatile_classes || []);
  for (const kind of declared) {
    if (!Object.hasOwn(VOLATILE_KEY_CLASSES, kind)) {
      throw new Error(`unknown_v2_transcript_volatile_class:${kind}`);
    }
  }
  function visit(current) {
    if (Array.isArray(current)) return current.map(visit);
    if (!current || typeof current !== "object") return current;
    return Object.fromEntries(Object.entries(current).map(([key, member]) => {
      if (key === "inputSchema" || key === "outputSchema") return [key, member];
      const marker = markerFor(key, declared);
      return [key, marker || visit(member)];
    }));
  }
  return visit(value);
}

const PROFILE_FIXTURE_FIELDS = ["revision", "tool_fields", "result_fields", "result_form", "batch"];
const DARK_PROFILE_ORACLE = Object.freeze({
  "2024-11-05": Object.freeze({
    tool_fields: Object.freeze(["name", "description", "inputSchema"]),
    result_fields: Object.freeze(["content", "isError"]),
    result_form: "json_text_content_only",
    batch: "accept_independent_ordered_omit_notifications",
  }),
  "2025-03-26": Object.freeze({
    tool_fields: Object.freeze(["name", "description", "inputSchema", "annotations"]),
    result_fields: Object.freeze(["content", "isError"]),
    result_form: "json_text_content_only",
    batch: "accept_independent_ordered_omit_notifications",
  }),
  "2025-06-18": Object.freeze({
    tool_fields: Object.freeze(["name", "title", "description", "inputSchema", "outputSchema", "_meta"]),
    result_fields: Object.freeze(["content", "structuredContent", "isError", "_meta"]),
    result_form: "structured_content_and_identical_json_text",
    batch: "reject_invalid_request",
  }),
  "2025-11-25": Object.freeze({
    tool_fields: Object.freeze(["name", "title", "description", "inputSchema", "outputSchema", "_meta"]),
    result_fields: Object.freeze(["content", "structuredContent", "isError", "_meta"]),
    result_form: "structured_content_and_identical_json_text",
    batch: "reject_invalid_request",
  }),
});

const PROFILE_NORMALIZATION_CLASSES = Object.freeze([
  "operation_id",
  "request_id",
  "packet_id",
  "publication_id",
  "timestamps",
  "timing",
  "runtime_binary_hash",
  "source_identity",
  "build_identity",
]);

function sameArray(actual, expected) {
  return Array.isArray(actual)
    && actual.length === expected.length
    && actual.every((value, index) => value === expected[index]);
}

function rejectUnknownFields(value, allowed, label) {
  for (const field of Object.keys(value || {})) {
    if (!allowed.includes(field)) throw new Error(`${label}:${field}`);
  }
}

/// Validate the checked-in future-profile fixture against an independently
/// compiled dark oracle. The fixture is documentation and a drift ratchet; it
/// never supplies dispatch behavior.
export function validateProfileFixture(fixture) {
  rejectUnknownFields(
    fixture,
    ["profiles", "nonstandard_codestory_fields", "normalization"],
    "profile_fixture_unknown_top_level_field",
  );
  if (!Array.isArray(fixture?.profiles)) throw new Error("profile_fixture_profiles_missing");
  const expectedRevisions = Object.keys(DARK_PROFILE_ORACLE);
  if (!sameArray(fixture.profiles.map((profile) => profile?.revision), expectedRevisions)) {
    throw new Error("profile_fixture_revision_mismatch");
  }
  for (const profile of fixture.profiles) {
    const revision = profile.revision;
    rejectUnknownFields(profile, PROFILE_FIXTURE_FIELDS, `profile_fixture_unknown_field:${revision}`);
    const expected = DARK_PROFILE_ORACLE[revision];
    for (const field of ["tool_fields", "result_fields"]) {
      if (!sameArray(profile[field], expected[field])) throw new Error(`profile_fixture_mismatch:${revision}:${field}`);
    }
    for (const field of ["result_form", "batch"]) {
      if (profile[field] !== expected[field]) throw new Error(`profile_fixture_mismatch:${revision}:${field}`);
    }
  }
  if (!sameArray(fixture.nonstandard_codestory_fields, ["safety"])) {
    throw new Error("profile_fixture_nonstandard_fields_mismatch");
  }
  if (!sameArray(fixture.normalization, PROFILE_NORMALIZATION_CLASSES)) {
    throw new Error("profile_fixture_normalization_mismatch");
  }
}

function profileTool(profile) {
  const candidates = {
    name: "ground",
    title: "Ground",
    description: "Build a compact repository map.",
    inputSchema: { type: "object" },
    outputSchema: { type: "object" },
    annotations: { readOnlyHint: true },
    _meta: { codestory: "fixture" },
  };
  return Object.fromEntries(profile.tool_fields.map((field) => [field, candidates[field]]));
}

function profileResult(profile) {
  const structuredContent = { state: "ready" };
  const candidates = {
    content: [{ type: "text", text: JSON.stringify(structuredContent) }],
    structuredContent,
    isError: false,
    _meta: { codestory: "fixture" },
  };
  return Object.fromEntries(profile.result_fields.map((field) => [field, candidates[field]]));
}

function isJsonRpcRequest(frame) {
  if (!frame || typeof frame !== "object" || Array.isArray(frame)) return false;
  if (frame.jsonrpc !== "2.0" || typeof frame.method !== "string") return false;
  if (frame.params !== undefined && (!frame.params || typeof frame.params !== "object")) return false;
  return Object.keys(frame).every((field) => ["jsonrpc", "id", "method", "params"].includes(field));
}

function profileForRevision(revision) {
  const profile = DARK_PROFILE_ORACLE[revision];
  if (!profile) throw new Error(`unknown_dark_profile_revision:${revision}`);
  return profile;
}

function executeProfileRequest(profile, frame) {
  if (!isJsonRpcRequest(frame)) {
    return { jsonrpc: "2.0", id: frame?.id ?? null, error: { code: -32600, message: "Invalid Request" } };
  }
  if (frame.id === undefined) return null;
  if (frame.method === "tools/list") {
    return { jsonrpc: "2.0", id: frame.id, result: { tools: [profileTool(profile)] } };
  }
  if (frame.method === "tools/call") {
    return { jsonrpc: "2.0", id: frame.id, result: profileResult(profile) };
  }
  return { jsonrpc: "2.0", id: frame.id, error: { code: -32601, message: "Method not found" } };
}

export function simulateProfileRequest(revision, frame) {
  return executeProfileRequest(profileForRevision(revision), frame);
}

/// Test-only future-profile batch simulator. Production v2 still consumes one
/// JSON-RPC object per line; this harness makes the later protocol contracts
/// executable without widening current dispatch.
export function simulateProfileBatch(revision, frames) {
  const profile = profileForRevision(revision);
  if (profile.batch === "reject_invalid_request") {
    return [{ jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid Request" } }];
  }
  const responses = [];
  for (const frame of frames) {
    const response = executeProfileRequest(profile, frame);
    if (response) responses.push(response);
  }
  return responses;
}
