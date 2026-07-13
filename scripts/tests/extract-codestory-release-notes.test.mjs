import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import { extractReleaseNotes } from "../../.github/scripts/extract-codestory-release-notes.mjs";

test("extracts only the exact version section between adjacent headings", () => {
  const changelog = `# Changelog

## Unreleased

- future work

## 1.2.3

### Fixed

- selected release

## 1.2.2

- previous release
`;

  assert.equal(extractReleaseNotes(changelog, "1.2.3"), "### Fixed\n\n- selected release\n");
});

test("rejects a missing version heading", () => {
  assert.throws(
    () => extractReleaseNotes("# Changelog\n\n## 1.2.2\n\n- previous\n", "1.2.3"),
    /missing the exact release heading/u,
  );
});

test("rejects duplicate version headings", () => {
  assert.throws(
    () => extractReleaseNotes("## 1.2.3\n\n- one\n\n## 1.2.3\n\n- two\n", "1.2.3"),
    /duplicate release headings/u,
  );
});

test("rejects an empty version section", () => {
  assert.throws(
    () => extractReleaseNotes("## 1.2.3\n\n\n## 1.2.2\n\n- previous\n", "1.2.3"),
    /has no content/u,
  );
});

test("extracts the historical 0.14.3 notes without adjacent releases", () => {
  const notes = extractReleaseNotes(fs.readFileSync("CHANGELOG.md", "utf8"), "0.14.3");

  assert.match(notes, /^### Fixed\n/u);
  assert.match(notes, /Declared glibc 2\.31/u);
  assert.doesNotMatch(notes, /^## 0\.14\.2$/mu);
  assert.doesNotMatch(notes, /^## Unreleased$/mu);
});
