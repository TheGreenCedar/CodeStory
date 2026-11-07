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

test("root test sources require every declaring crate root to be cfg(test)", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-runtime-config-"));
  try {
    const sourceRoot = path.join(root, "crate", "src");
    const testSource = path.join(sourceRoot, "tests", "case.rs");
    fs.mkdirSync(path.dirname(testSource), { recursive: true });
    fs.writeFileSync(testSource, "std::env::set_var(\"KEY\", \"value\");\n");
    fs.writeFileSync(path.join(sourceRoot, "main.rs"), "#[cfg(test)]\nmod tests;\n");
    const library = path.join(sourceRoot, "lib.rs");
    fs.writeFileSync(library, "mod tests;\n");

    assert.equal(isOutOfLineTestSource(testSource), false);

    fs.writeFileSync(library, "#[cfg(test)]\nmod tests;\n");
    assert.equal(isOutOfLineTestSource(testSource), true);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("attributed visible test modules must carry their own cfg(test) guard", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-runtime-config-"));
  try {
    const sourceRoot = path.join(root, "crate", "src");
    const testSource = path.join(sourceRoot, "tests", "affected.rs");
    fs.mkdirSync(path.dirname(testSource), { recursive: true });
    fs.writeFileSync(testSource, "std::env::set_var(\"KEY\", \"value\");\n");
    const owner = path.join(sourceRoot, "affected.rs");
    fs.writeFileSync(
      owner,
      '#[cfg(test)]\n#[path = "tests/affected.rs"]\npub(crate) mod tests;\n',
    );

    assert.equal(isOutOfLineTestSource(testSource), true);

    // A different explicit source is not an owner of affected.rs, even when
    // its test-only cfg expression is outside this checker's narrow syntax.
    fs.writeFileSync(
      path.join(sourceRoot, "source_context.rs"),
      '#[cfg(all(test, unix))]\n#[path = "source_context_tests.rs"]\nmod tests;\n',
    );
    fs.writeFileSync(path.join(sourceRoot, "source_context_tests.rs"), "");
    assert.equal(isOutOfLineTestSource(testSource), true);

    fs.writeFileSync(owner, '#[path = "tests/affected.rs"]\npub(crate) mod tests;\n');
    assert.equal(isOutOfLineTestSource(testSource), false);

    fs.writeFileSync(owner, '#[cfg(test)]\n#[path = "tests/affected.rs"]\npub(crate) mod tests;\n');
    const outside = path.join(root, "outside");
    fs.mkdirSync(outside);
    const alias = path.join(outside, "alias.rs");
    fs.linkSync(testSource, alias);
    const originalIdentity = fs.statSync(testSource, { bigint: true });
    const aliasIdentity = fs.statSync(alias, { bigint: true });
    assert.equal(aliasIdentity.dev, originalIdentity.dev);
    assert.equal(aliasIdentity.ino, originalIdentity.ino);
    fs.writeFileSync(
      path.join(sourceRoot, "unguarded_alias.rs"),
      '#[path = "../../outside/alias.rs"]\nmod tests;\n',
    );
    assert.equal(isOutOfLineTestSource(testSource), false);

  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
