const VOLATILE_KEY_CLASSES = {
  build_identity: new Set(["build_id", "build_identity"]),
  operation_id: new Set(["operation_id"]),
  packet_id: new Set(["packet_id"]),
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
  function visit(current) {
    if (Array.isArray(current)) return current.map(visit);
    if (!current || typeof current !== "object") return current;
    return Object.fromEntries(Object.entries(current).map(([key, member]) => {
      const marker = markerFor(key, declared);
      return [key, marker || visit(member)];
    }));
  }
  return visit(value);
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

/// Test-only future-profile batch simulator. Production v2 still consumes one
/// JSON-RPC object per line; this harness makes the later protocol contracts
/// executable without widening current dispatch.
export function simulateProfileBatch(profile, frames) {
  if (profile.batch === "reject_invalid_request") {
    return [{ jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid Request" } }];
  }
  const responses = [];
  for (const frame of frames) {
    if (!frame || typeof frame !== "object" || Array.isArray(frame) || typeof frame.method !== "string") {
      responses.push({ jsonrpc: "2.0", id: frame?.id ?? null, error: { code: -32600, message: "Invalid Request" } });
      continue;
    }
    if (frame.id === undefined) continue;
    if (frame.method === "tools/list") {
      responses.push({ jsonrpc: "2.0", id: frame.id, result: { tools: [profileTool(profile)] } });
    } else if (frame.method === "tools/call") {
      responses.push({ jsonrpc: "2.0", id: frame.id, result: profileResult(profile) });
    } else {
      responses.push({ jsonrpc: "2.0", id: frame.id, error: { code: -32601, message: "Method not found" } });
    }
  }
  return responses;
}
