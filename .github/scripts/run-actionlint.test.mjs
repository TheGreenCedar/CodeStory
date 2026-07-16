import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  cacheIsTrusted,
  platformKey,
  verifyArchiveChecksum,
} from "./run-actionlint.mjs";

test("platform selection covers every checksum-pinned actionlint asset", () => {
  const supported = [
    ["darwin", "arm64"],
    ["darwin", "x64"],
    ["linux", "arm64"],
    ["linux", "x64"],
    ["win32", "arm64"],
    ["win32", "x64"],
  ];
  for (const [platform, arch] of supported) {
    assert.equal(platformKey(platform, arch), `${platform}-${arch}`);
  }
  assert.throws(() => platformKey("freebsd", "x64"), /does not have a declared asset/u);
});

test("archive verification rejects any checksum mismatch", () => {
  const bytes = Buffer.from("checksum fixture\n");
  const expected = "170696f87efa6a1ae958c597193b6942a74f2a81ac108d243526e682d1b85ca4";
  assert.equal(verifyArchiveChecksum(bytes, expected), expected);
  assert.throws(() => verifyArchiveChecksum(bytes, "0".repeat(64)), /checksum mismatch/u);
});

test("cached actionlint is trusted only with matching marker and version", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-actionlint-test-"));
  const binary = path.join(directory, "actionlint");
  const marker = path.join(directory, "archive.sha256");
  const digest = "a".repeat(64);
  writeFileSync(binary, "fixture");
  writeFileSync(marker, `${digest}\n`);
  const values = {
    binary,
    marker,
    expectedSha256: digest,
    expectedVersion: "1.7.12",
  };
  assert.equal(cacheIsTrusted({ ...values, versionReader: () => "1.7.12" }), true);
  assert.equal(cacheIsTrusted({ ...values, versionReader: () => "1.7.11" }), false);
  writeFileSync(marker, `${"b".repeat(64)}\n`);
  assert.equal(cacheIsTrusted({ ...values, versionReader: () => "1.7.12" }), false);
});
