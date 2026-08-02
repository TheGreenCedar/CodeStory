#!/usr/bin/env node

// Windows package timing used to claim linker evidence from `grep -Eic 'time'`
// over the build log, which the Cargo progress line `Compiling time v0.3.47`
// satisfies: that is the dependency named `time`, not a linker duration.
//
// This selector reads the same captured log as a structured build trace. An
// MSVC `link /TIME` report is an explicit boundary: it names the linker pass
// and interval it opened, and it closes with the linker's own total duration.
// A report only counts when it opens at `Pass 1: Interval #1`, closes at
// `Final: Total time`, keeps its interval identities strictly increasing, and
// stays inside the wall-clock build interval that contained the link. Anything
// else leaves the phase `unavailable` with a typed reason.
//
// Linker timing is observational. Missing or unusable timing never invalidates
// an otherwise authenticated package, so the selector reports `unavailable`
// and exits 0; only a malformed invocation of the selector itself exits 1.

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

export const SCHEMA = "codestory.windows-link-timing/v1";
export const LINK_PHASE = "msvc_link";
export const BUILD_PHASE = "cargo_graph";

// Both tokens carry a structural identity (`Pass N: Interval #M`, `Final:`) and
// an explicit second-denominated duration. No Cargo progress, warning, or crate
// name can produce either one.
const PASS_INTERVAL =
  /(?:^|[^0-9A-Za-z_])Pass\s+([0-9]{1,3}):\s+Interval\s+#([0-9]{1,3}),\s+time\s*=\s*([0-9]+(?:\.[0-9]+)?)s(?![0-9A-Za-z_.])/u;
const FINAL_TOTAL =
  /(?:^|[^0-9A-Za-z_])Final:\s+Total\s+time\s*=\s*([0-9]+(?:\.[0-9]+)?)s(?![0-9A-Za-z_.])/u;
const SECOND_TOLERANCE = 1e-6;

export const UNAVAILABLE_REASONS = Object.freeze({
  missing: "linker-log-missing",
  empty: "linker-log-empty",
  absent: "no-explicit-linker-report",
  incoherent: "incoherent-linker-report",
  exceedsBuild: "link-exceeds-build-interval",
});

function fail(message) {
  throw new Error(message);
}

function requireNonNegativeInteger(value, label) {
  const parsed = typeof value === "number" ? value : Number.parseInt(String(value), 10);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    fail(`${label} must be a non-negative integer`);
  }
  return parsed;
}

function seconds(text, label) {
  const parsed = Number.parseFloat(text);
  if (!Number.isFinite(parsed) || parsed < 0) fail(`${label} must be a finite non-negative duration`);
  return parsed;
}

function toMilliseconds(value) {
  return Math.round(value * 1000);
}

// Reports are recognised line by line so an interleaved Cargo diagnostic, a
// rustc `warning: linker stdout:` header, or an emitter indent cannot split a
// report apart or glue two of them together.
export function parseLinkTimingReports(text) {
  const invocations = [];
  const incoherent = [];
  let open = null;
  let truncated = 0;
  let orphanFinals = 0;
  let danglingIntervals = 0;

  const closeIncoherent = (report, reason) => {
    incoherent.push({ reason, intervals: report.intervals });
  };

  for (const line of String(text).split(/\r?\n/u)) {
    const interval = PASS_INTERVAL.exec(line);
    if (interval) {
      const pass = Number.parseInt(interval[1], 10);
      const index = Number.parseInt(interval[2], 10);
      const elapsed = seconds(interval[3], "linker interval duration");
      if (pass === 1 && index === 1) {
        if (open) truncated += 1;
        open = { intervals: [{ pass, interval: index, seconds: elapsed }] };
        continue;
      }
      if (!open) {
        danglingIntervals += 1;
        continue;
      }
      const previous = open.intervals[open.intervals.length - 1];
      if (pass <= previous.pass || index <= previous.interval) {
        closeIncoherent(open, "interval identities must strictly increase");
        open = null;
        continue;
      }
      open.intervals.push({ pass, interval: index, seconds: elapsed });
      continue;
    }

    const final = FINAL_TOTAL.exec(line);
    if (!final) continue;
    const total = seconds(final[1], "linker total duration");
    if (!open) {
      orphanFinals += 1;
      continue;
    }
    const measured = open.intervals.reduce((sum, entry) => sum + entry.seconds, 0);
    if (total + SECOND_TOLERANCE < measured || total <= 0) {
      closeIncoherent(open, "total duration must cover every reported interval");
      open = null;
      continue;
    }
    invocations.push({
      index: invocations.length + 1,
      intervals: open.intervals,
      total_seconds: total,
      total_ms: toMilliseconds(total),
    });
    open = null;
  }

  if (open) truncated += 1;

  return {
    invocations,
    incoherent,
    diagnostics: {
      truncated_reports: truncated,
      orphan_totals: orphanFinals,
      dangling_intervals: danglingIntervals,
    },
  };
}

function unavailable(reason, buildElapsedMs, diagnostics) {
  return {
    schema: SCHEMA,
    phase: LINK_PHASE,
    source: "msvc-link-time-report",
    status: "unavailable",
    reason,
    observational: true,
    invocation_count: 0,
    link_ms: null,
    invocations: [],
    build_interval: { phase: BUILD_PHASE, elapsed_ms: buildElapsedMs },
    diagnostics,
  };
}

const NO_DIAGNOSTICS = Object.freeze({
  truncated_reports: 0,
  orphan_totals: 0,
  dangling_intervals: 0,
});

export function selectWindowsLinkTiming({ log, buildElapsedMs }) {
  const buildMs = requireNonNegativeInteger(buildElapsedMs, "--build-elapsed-ms");
  if (log === null || log === undefined) {
    return unavailable(UNAVAILABLE_REASONS.missing, buildMs, { ...NO_DIAGNOSTICS });
  }
  if (String(log).trim() === "") {
    return unavailable(UNAVAILABLE_REASONS.empty, buildMs, { ...NO_DIAGNOSTICS });
  }

  const parsed = parseLinkTimingReports(log);
  if (parsed.incoherent.length > 0) {
    return unavailable(UNAVAILABLE_REASONS.incoherent, buildMs, parsed.diagnostics);
  }
  if (parsed.invocations.length === 0) {
    return unavailable(UNAVAILABLE_REASONS.absent, buildMs, parsed.diagnostics);
  }

  const linkSeconds = parsed.invocations.reduce((sum, entry) => sum + entry.total_seconds, 0);
  const linkMs = toMilliseconds(linkSeconds);
  // Linking happens inside the measured build boundary, so a trace claiming more
  // linker time than the step spent building is forged or mis-attributed.
  if (linkMs > buildMs) {
    return unavailable(UNAVAILABLE_REASONS.exceedsBuild, buildMs, parsed.diagnostics);
  }

  return {
    schema: SCHEMA,
    phase: LINK_PHASE,
    source: "msvc-link-time-report",
    status: "observed",
    reason: null,
    observational: true,
    invocation_count: parsed.invocations.length,
    link_ms: linkMs,
    invocations: parsed.invocations,
    build_interval: { phase: BUILD_PHASE, elapsed_ms: buildMs },
    diagnostics: parsed.diagnostics,
  };
}

export function summaryLine(record) {
  if (record.status === "observed") {
    const plural = record.invocation_count === 1 ? "invocation" : "invocations";
    return `- MSVC linker (${LINK_PHASE}): ${record.link_ms} ms across `
      + `${record.invocation_count} explicit linker ${plural}`;
  }
  return `- MSVC linker (${LINK_PHASE}): unavailable (${record.reason}); `
    + "no linker duration is claimed for this package";
}

function readLog(file) {
  if (!fs.existsSync(file)) return null;
  const stats = fs.statSync(file);
  if (!stats.isFile()) fail(`--input must be a regular file: ${file}`);
  return fs.readFileSync(file, "utf8");
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (!token.startsWith("--")) fail(`unexpected argument: ${token}`);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) fail(`missing value for ${token}`);
    if (values.has(token)) fail(`${token} may be supplied only once`);
    values.set(token, value);
    index += 1;
  }
  return values;
}

function one(values, name) {
  const value = values.get(name);
  if (value === undefined || value.trim() === "") fail(`missing ${name}`);
  return value;
}

function appendSummary(line) {
  const summary = process.env.GITHUB_STEP_SUMMARY;
  if (summary) fs.appendFileSync(summary, `${line}\n`);
}

function runSelect(values) {
  const out = one(values, "--out");
  const record = selectWindowsLinkTiming({
    log: readLog(one(values, "--input")),
    buildElapsedMs: one(values, "--build-elapsed-ms"),
  });
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, `${JSON.stringify(record, null, 2)}\n`);
  const line = summaryLine(record);
  console.log(line);
  appendSummary(line);
}

function main(argv) {
  const [command, ...rest] = argv;
  const values = parseArguments(rest);
  if (command === "select") return runSelect(values);
  return fail("usage: windows-link-timing.mjs select --input <log> --out <json> --build-elapsed-ms <ms>");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
