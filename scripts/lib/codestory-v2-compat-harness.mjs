const VOLATILE_KEY_CLASSES = {
  build_identity: new Set(["build_id", "build_identity"]),
  operation_id: new Set(["operation_id", "request_id"]),
  packet_id: new Set(["packet_id", "publication_id"]),
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
