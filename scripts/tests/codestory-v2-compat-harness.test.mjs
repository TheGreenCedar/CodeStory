import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { normalizeV2Transcript } from "../lib/codestory-v2-compat-harness.mjs";

const repoRoot = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const fixturePath = path.join(repoRoot, "scripts", "tests", "fixtures", "codestory-v2-transcripts.json");

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
});
