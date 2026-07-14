import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const here = dirname(fileURLToPath(import.meta.url));
const scripts = join(here, "..");
const fixtures = join(here, "fixtures", "drill-suite-ledger");

test("suite ledger scoring preserves the extracted production report contract", () => {
  const outputDir = mkdtempSync(join(tmpdir(), "codestory-drill-ledger-"));
  const output = join(outputDir, "scored.json");
  const result = spawnSync(
    process.execPath,
    [
      join(scripts, "score-drill-ledger.mjs"),
      join(fixtures, "suite-report.before.json"),
      join(fixtures, "source-truth-ledger.json"),
      output,
    ],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  const expected = JSON.parse(readFileSync(join(fixtures, "suite-report.scored.json"), "utf8"));
  assert.deepEqual(JSON.parse(result.stdout), expected);
  assert.deepEqual(JSON.parse(readFileSync(output, "utf8")), expected);
  rmSync(outputDir, { recursive: true, force: true });
});

test("single drill scoring keeps the existing evaluator contract", () => {
  const dir = mkdtempSync(join(tmpdir(), "codestory-drill-ledger-"));
  const report = join(dir, "drill.json");
  const ledger = join(dir, "ledger.json");
  writeFileSync(
    report,
    JSON.stringify({
      evidence_packet: {
        packet_id: "packet-1",
        sufficiency: { status: "sufficient" },
        answer: { citations: [{ file_path: "src/a.rs" }] },
      },
    }),
  );
  writeFileSync(
    ledger,
    JSON.stringify({
      claims: [
        { classification: "correct", source_files: ["src/a.rs"] },
        {
          classification: "partial",
          changed_after_source_read: true,
          source_files: ["src/b.rs"],
        },
      ],
    }),
  );
  const result = spawnSync(
    process.execPath,
    [join(scripts, "score-drill-ledger.mjs"), report, ledger],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), {
    packet_id: "packet-1",
    packet_sufficiency: "sufficient",
    claim_count: 2,
    correct: 1,
    partial: 1,
    misleading: 0,
    unsupported: 0,
    material_revision_count: 1,
    quality_score: 0.75,
    cited_file_count: 1,
    ledger_file_count: 2,
    uncited_ledger_files: ["src/b.rs"],
  });
  rmSync(dir, { recursive: true, force: true });
});

test("suite scoring rejects malformed source-truth ledger DTOs", () => {
  const dir = mkdtempSync(join(tmpdir(), "codestory-drill-ledger-"));
  const ledger = JSON.parse(
    readFileSync(join(fixtures, "source-truth-ledger.json"), "utf8"),
  );
  delete ledger.cases[0].claims[0].id;
  const malformed = join(dir, "malformed.json");
  writeFileSync(malformed, JSON.stringify(ledger));
  const result = spawnSync(
    process.execPath,
    [
      join(scripts, "score-drill-ledger.mjs"),
      join(fixtures, "suite-report.before.json"),
      malformed,
    ],
    { encoding: "utf8" },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /claims\[0\]\.id must be a string/);
  rmSync(dir, { recursive: true, force: true });
});
