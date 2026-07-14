#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const VERSION_PATTERN = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/u;
const NUMERIC_IDENTIFIER = /^\d+$/u;
const STRICT_NUMERIC_IDENTIFIER = /^(?:0|[1-9]\d*)$/u;

function isStrictSemver(version) {
  if (!VERSION_PATTERN.test(version)) return false;
  const withoutBuild = version.split("+", 1)[0];
  const prereleaseStart = withoutBuild.indexOf("-");
  if (prereleaseStart < 0) return true;
  return withoutBuild
    .slice(prereleaseStart + 1)
    .split(".")
    .every(identifier =>
      !NUMERIC_IDENTIFIER.test(identifier) || STRICT_NUMERIC_IDENTIFIER.test(identifier));
}

export function extractReleaseNotes(changelog, version) {
  if (!isStrictSemver(version)) {
    throw new Error(`release version must be strict semver, got ${JSON.stringify(version)}`);
  }

  const lines = changelog.split(/\r?\n/u);
  const heading = `## ${version}`;
  const matches = lines
    .map((line, index) => line.trimEnd() === heading ? index : -1)
    .filter(index => index >= 0);

  if (matches.length === 0) {
    throw new Error(`CHANGELOG.md is missing the exact release heading ${heading}`);
  }
  if (matches.length > 1) {
    throw new Error(`CHANGELOG.md contains duplicate release headings for ${heading}`);
  }

  const start = matches[0] + 1;
  const relativeEnd = lines.slice(start).findIndex(line => /^##(?:\s|$)/u.test(line));
  const end = relativeEnd < 0 ? lines.length : start + relativeEnd;
  const section = lines.slice(start, end).join("\n").trim();
  if (!section) {
    throw new Error(`CHANGELOG.md release heading ${heading} has no content`);
  }
  return `${section}\n`;
}

function parseArgs(argv) {
  const args = [...argv];
  const values = new Map();
  while (args.length > 0) {
    const flag = args.shift();
    if (!flag?.startsWith("--")) {
      throw new Error(`unexpected argument: ${flag ?? ""}`);
    }
    const value = args.shift();
    if (!value || value.startsWith("--")) {
      throw new Error(`${flag} requires a value`);
    }
    values.set(flag, value);
  }
  return {
    changelog: values.get("--changelog") ?? "CHANGELOG.md",
    output: values.get("--output") ?? "",
    version: values.get("--version") ?? "",
  };
}

function main(argv) {
  const options = parseArgs(argv);
  const notes = extractReleaseNotes(fs.readFileSync(options.changelog, "utf8"), options.version);
  if (options.output) {
    fs.mkdirSync(path.dirname(options.output), { recursive: true });
    fs.writeFileSync(options.output, notes, "utf8");
  } else {
    process.stdout.write(notes);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}
