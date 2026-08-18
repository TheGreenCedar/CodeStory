import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  BUILD_PHASE,
  LINK_PHASE,
  SCHEMA,
  parseLinkTimingReports,
  selectWindowsLinkTiming,
  summaryLine,
} from "./windows-link-timing.mjs";

const selector = fileURLToPath(new URL("./windows-link-timing.mjs", import.meta.url));

// The 0.16.3 Windows package claimed linker evidence from this log alone: the
// substring search counted the Cargo progress line for the crate named `time`.
const CARGO_PROGRESS_ONLY = [
  "    Updating crates.io index",
  "   Compiling time v0.3.47",
  "   Compiling time-core v0.1.2",
  "   Compiling codestory-cli v0.16.3 (D:\\a\\CodeStory\\CodeStory\\crates\\codestory-cli)",
  "warning: unused import: `std::time::Instant`",
  "    Finished `release` profile [optimized] target(s) in 8m 31s",
].join("\n");

// One rustc-rendered report (linker output arrives inside a diagnostic, so the
// first line carries a header and the rest are emitter-indented) plus one raw
// report, interleaved with the same Cargo progress noise as above.
const TWO_REAL_REPORTS = [
  "   Compiling time v0.3.47",
  "   Compiling codestory-cli v0.16.3 (D:\\a\\CodeStory\\CodeStory\\crates\\codestory-cli)",
  "warning: linker stdout: Pass 1: Interval #1, time = 0.31250s",
  "                          Input1: Total time = 0.09375s",
  "                          Input2: Total time = 0.00000s",
  "                        Pass 2: Interval #2, time = 1.42188s",
  "                          Final: Total time = 1.75000s",
  "   Compiling codestory-cli-runtime v0.16.3",
  "Pass 1: Interval #1, time = 0.29688s",
  "                    Input1: Total time = 0.07813s",
  "Pass 2: Interval #2, time = 0.98438s",
  "Final: Total time = 1.30000s",
  "    Finished `release` profile [optimized] target(s) in 8m 31s",
].join("\n");

const BUILD_MS = 512345;

function temporaryDirectory(t) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "codestory-link-timing-"));
  t.after(() => rmSync(directory, { force: true, recursive: true }));
  return directory;
}

function runSelector(args, environment = {}) {
  return spawnSync(process.execPath, [selector, ...args], {
    encoding: "utf8",
    env: { ...process.env, ...environment },
  });
}

test("a Cargo progress log for the crate named time carries no linker timing", () => {
  assert.match(CARGO_PROGRESS_ONLY, /Compiling time v0\.3\.47/u);
  const record = selectWindowsLinkTiming({
    log: CARGO_PROGRESS_ONLY,
    buildElapsedMs: BUILD_MS,
  });
  assert.equal(record.status, "unavailable");
  assert.equal(record.reason, "no-explicit-linker-report");
  assert.equal(record.invocation_count, 0);
  assert.equal(record.link_ms, null);
  assert.deepEqual(record.invocations, []);
  assert.equal(record.schema, SCHEMA);
  assert.equal(record.phase, LINK_PHASE);
  assert.equal(record.observational, true);
  assert.deepEqual(record.build_interval, { phase: BUILD_PHASE, elapsed_ms: BUILD_MS });
  assert.equal(
    summaryLine(record),
    "- MSVC linker (msvc_link): unavailable (no-explicit-linker-report); "
      + "no linker duration is claimed for this package",
  );
});

test("captured explicit linker invocations and durations satisfy the phase", () => {
  const record = selectWindowsLinkTiming({ log: TWO_REAL_REPORTS, buildElapsedMs: BUILD_MS });
  assert.equal(record.status, "observed");
  assert.equal(record.reason, null);
  assert.equal(record.invocation_count, 2);
  assert.equal(record.link_ms, 3050);
  assert.deepEqual(record.build_interval, { phase: BUILD_PHASE, elapsed_ms: BUILD_MS });
  assert.deepEqual(record.invocations.map(entry => entry.total_ms), [1750, 1300]);
  assert.deepEqual(record.invocations[0].intervals, [
    { pass: 1, interval: 1, seconds: 0.3125 },
    { pass: 2, interval: 2, seconds: 1.42188 },
  ]);
  assert.deepEqual(record.invocations[1].intervals, [
    { pass: 1, interval: 1, seconds: 0.29688 },
    { pass: 2, interval: 2, seconds: 0.98438 },
  ]);
  assert.equal(
    summaryLine(record),
    "- MSVC linker (msvc_link): 3050 ms across 2 explicit linker invocations",
  );
});

test("half a report, a forged total, and near-miss tokens never open the phase", () => {
  const cases = [
    ["Final: Total time = 4.00000s", "no-explicit-linker-report"],
    ["Pass 1: Interval #1, time = 0.31250s", "no-explicit-linker-report"],
    [
      ["Pass 2: Interval #2, time = 1.00000s", "Final: Total time = 1.00000s"].join("\n"),
      "no-explicit-linker-report",
    ],
    ["note: linking took time = 4.00000s", "no-explicit-linker-report"],
    [
      ["Pass 1: Interval #1, time = 0.31250", "Final: Total time = 1.73438"].join("\n"),
      "no-explicit-linker-report",
    ],
    [
      ["XPass 1: Interval #1, time = 0.31250s", "Finalizer: Total time = 1.73438s"].join("\n"),
      "no-explicit-linker-report",
    ],
    [
      ["Pass 1: Interval #1, time = 1.50000s", "Final: Total time = 0.10000s"].join("\n"),
      "incoherent-linker-report",
    ],
    [
      [
        "Pass 1: Interval #1, time = 0.10000s",
        "Pass 2: Interval #2, time = 0.10000s",
        "Pass 2: Interval #2, time = 0.10000s",
        "Final: Total time = 9.00000s",
      ].join("\n"),
      "incoherent-linker-report",
    ],
  ];
  for (const [log, reason] of cases) {
    const record = selectWindowsLinkTiming({ log, buildElapsedMs: BUILD_MS });
    assert.equal(record.status, "unavailable", `expected ${JSON.stringify(log)} to stay unavailable`);
    assert.equal(record.reason, reason, `wrong reason for ${JSON.stringify(log)}`);
    assert.equal(record.link_ms, null);
    assert.equal(record.invocation_count, 0);
  }
});

test("a linker trace wider than its own build interval is rejected", () => {
  const log = [
    "Pass 1: Interval #1, time = 0.50000s",
    "Pass 2: Interval #2, time = 1.50000s",
    "Final: Total time = 2.00000s",
  ].join("\n");
  assert.equal(selectWindowsLinkTiming({ log, buildElapsedMs: 2000 }).status, "observed");
  const record = selectWindowsLinkTiming({ log, buildElapsedMs: 1999 });
  assert.equal(record.status, "unavailable");
  assert.equal(record.reason, "link-exceeds-build-interval");
  assert.equal(record.link_ms, null);
  assert.equal(record.build_interval.elapsed_ms, 1999);
});

test("truncated and orphaned fragments are reported without becoming evidence", () => {
  const parsed = parseLinkTimingReports([
    "Final: Total time = 4.00000s",
    "Pass 3: Interval #7, time = 0.25000s",
    "Pass 1: Interval #1, time = 0.31250s",
  ].join("\n"));
  assert.deepEqual(parsed.invocations, []);
  assert.deepEqual(parsed.diagnostics, {
    truncated_reports: 1,
    orphan_totals: 1,
    dangling_intervals: 1,
  });
});

test("a missing or empty linker log stays observational rather than failing the package", (t) => {
  const directory = temporaryDirectory(t);
  const out = path.join(directory, "records", "windows-link-timing.json");
  const summary = path.join(directory, "summary.md");
  const missing = runSelector([
    "select",
    "--input",
    path.join(directory, "absent.log"),
    "--out",
    out,
    "--build-elapsed-ms",
    String(BUILD_MS),
  ], { GITHUB_STEP_SUMMARY: summary });
  assert.equal(missing.status, 0, missing.stderr);
  const missingRecord = JSON.parse(readFileSync(out, "utf8"));
  assert.equal(missingRecord.status, "unavailable");
  assert.equal(missingRecord.reason, "linker-log-missing");
  assert.equal(missingRecord.link_ms, null);

  const emptyLog = path.join(directory, "empty.log");
  writeFileSync(emptyLog, "   \n\n");
  const empty = runSelector([
    "select",
    "--input",
    emptyLog,
    "--out",
    out,
    "--build-elapsed-ms",
    String(BUILD_MS),
  ], { GITHUB_STEP_SUMMARY: summary });
  assert.equal(empty.status, 0, empty.stderr);
  assert.equal(JSON.parse(readFileSync(out, "utf8")).reason, "linker-log-empty");
  assert.equal(
    readFileSync(summary, "utf8"),
    "- MSVC linker (msvc_link): unavailable (linker-log-missing); "
      + "no linker duration is claimed for this package\n"
      + "- MSVC linker (msvc_link): unavailable (linker-log-empty); "
      + "no linker duration is claimed for this package\n",
  );
});

test("the selector writes one receipt and one summary interval per package build", (t) => {
  const directory = temporaryDirectory(t);
  const log = path.join(directory, "msvc-link-time.log");
  const out = path.join(directory, "windows-link-timing.json");
  const summary = path.join(directory, "summary.md");
  writeFileSync(log, TWO_REAL_REPORTS);
  const observed = runSelector([
    "select",
    "--input",
    log,
    "--out",
    out,
    "--build-elapsed-ms",
    String(BUILD_MS),
  ], { GITHUB_STEP_SUMMARY: summary });
  assert.equal(observed.status, 0, observed.stderr);
  assert.equal(
    readFileSync(summary, "utf8"),
    "- MSVC linker (msvc_link): 3050 ms across 2 explicit linker invocations\n",
  );
  const record = JSON.parse(readFileSync(out, "utf8"));
  assert.equal(record.schema, SCHEMA);
  assert.equal(record.status, "observed");
  assert.equal(record.link_ms, 3050);
  assert.equal(record.build_interval.elapsed_ms, BUILD_MS);

  writeFileSync(log, CARGO_PROGRESS_ONLY);
  const defect = runSelector([
    "select",
    "--input",
    log,
    "--out",
    out,
    "--build-elapsed-ms",
    String(BUILD_MS),
  ], { GITHUB_STEP_SUMMARY: summary });
  assert.equal(defect.status, 0, defect.stderr);
  assert.match(defect.stdout, /unavailable \(no-explicit-linker-report\)/u);
  assert.doesNotMatch(defect.stdout, /\d+ ms/u);
  assert.equal(JSON.parse(readFileSync(out, "utf8")).status, "unavailable");
});

test("a malformed selector invocation fails loudly instead of inventing a record", (t) => {
  const directory = temporaryDirectory(t);
  const log = path.join(directory, "msvc-link-time.log");
  writeFileSync(log, TWO_REAL_REPORTS);
  const out = path.join(directory, "windows-link-timing.json");
  const invocations = [
    [[], /usage: windows-link-timing\.mjs select/u],
    [["report", "--input", log, "--out", out, "--build-elapsed-ms", "1"], /usage: windows-link-timing\.mjs select/u],
    [["select", "--input", log, "--build-elapsed-ms", "1"], /missing --out/u],
    [["select", "--out", out, "--build-elapsed-ms", "1"], /missing --input/u],
    [["select", "--input", log, "--out", out], /missing --build-elapsed-ms/u],
    [["select", "--input", log, "--out", out, "--build-elapsed-ms", "-1"], /must be a non-negative integer/u],
    [["select", "--input", log, "--out", out, "--build-elapsed-ms", "later"], /must be a non-negative integer/u],
    [["select", "--input", log, "--out", out, "--build-elapsed-ms", "1", "--input", log], /--input may be supplied only once/u],
  ];
  for (const [args, expected] of invocations) {
    const result = runSelector(args);
    assert.equal(result.status, 1, `expected ${JSON.stringify(args)} to fail`);
    assert.match(result.stderr, expected);
  }
});
