import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { isOutOfLineTestSource } from "./check-runtime-config-boundary.mjs";

test("out-of-line test sources require a cfg(test) module owner", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-runtime-config-"));
  try {
    const sourceRoot = path.join(root, "crate", "src");
    const testSource = path.join(sourceRoot, "app", "tests", "case.rs");
    fs.mkdirSync(path.dirname(testSource), { recursive: true });
    fs.writeFileSync(testSource, "std::env::set_var(\"KEY\", \"value\");\n");
    const owner = path.join(sourceRoot, "app.rs");
    fs.writeFileSync(owner, "#[cfg(test)]\nmod tests;\n");

    assert.equal(isOutOfLineTestSource(testSource), true);

    fs.writeFileSync(owner, "mod tests;\n");
    assert.equal(isOutOfLineTestSource(testSource), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
