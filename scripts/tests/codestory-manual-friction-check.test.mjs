import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = "scripts/codestory-manual-friction-check.mjs";
const scriptAbsolute = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../codestory-manual-friction-check.mjs",
);

test("manual friction harness no longer exposes embedding setup", () => {
  const help = spawnSync(process.execPath, [script, "--help"], { encoding: "utf8" });
  assert.equal(help.status, 0, help.stderr);
  assert.doesNotMatch(help.stdout, /setup-embeddings|setup embeddings/iu);

  const removed = spawnSync(process.execPath, [script, "--setup-embeddings"], { encoding: "utf8" });
  assert.equal(removed.status, 2);
  assert.match(removed.stderr, /Unknown argument: --setup-embeddings/u);
});

test("missing release CLI diagnostic prepares the embedded model first", () => {
  const source = readFileSync(script, "utf8");
  const diagnostic = source.match(/release codestory-cli is missing;[^"]+/u)?.[0] ?? "";
  assert.match(diagnostic, /prepare-embedded-model\.mjs/u);
  assert.match(diagnostic, /CODESTORY_EMBED_MODEL_SOURCE/u);
  assert.ok(
    diagnostic.indexOf("prepare-embedded-model.mjs") < diagnostic.indexOf("cargo build --release"),
  );
  assert.ok(
    diagnostic.indexOf("CODESTORY_EMBED_MODEL_SOURCE") < diagnostic.indexOf("cargo build --release"),
  );
});

test("the missing-CLI path emits its model-first guidance instead of dying early", (t) => {
  // A source-text assertion cannot see runtime ordering: anything the harness
  // does before the cli_missing check (the grounding-skill read, repo scan)
  // could throw first and the guidance would never be emitted. Run the real
  // harness in a fixture root whose release CLI is absent by construction.
  const root = mkdtempSync(join(tmpdir(), "codestory-friction-missing-cli-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const skillDir = join(root, ".agents", "skills", "codestory-grounding");
  mkdirSync(skillDir, { recursive: true });
  writeFileSync(join(skillDir, "SKILL.md"), "# test skill\n");

  const result = spawnSync(process.execPath, [scriptAbsolute, "--quick"], {
    cwd: root,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const gap = result.stdout.match(/^GAP P0 all cli_missing: (.+)$/mu);
  assert.ok(gap, `cli_missing gap was never emitted: ${result.stdout}`);
  assert.match(gap[1], /prepare-embedded-model\.mjs/u);
  assert.match(gap[1], /CODESTORY_EMBED_MODEL_SOURCE/u);
  assert.ok(
    gap[1].indexOf("prepare-embedded-model.mjs") < gap[1].indexOf("cargo build --release"),
    "the emitted guidance must name model preparation before the build",
  );
  assert.match(result.stdout, /METRIC quality_gap=/u);
});
