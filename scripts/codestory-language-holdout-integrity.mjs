#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const repoRoot = process.cwd();
const manifestPath = path.join(
  repoRoot,
  "benchmarks",
  "tasks",
  "language-expansion-holdout",
  "language-support-ab.task.json",
);
const repoCacheDir = process.env.CODESTORY_AB_REPO_CACHE_DIR
  ? path.resolve(repoRoot, process.env.CODESTORY_AB_REPO_CACHE_DIR)
  : path.join(repoRoot, "target", "agent-benchmark", "repos");
const reportPath = process.env.CODESTORY_OSS_CORPUS_REPORT
  ? path.resolve(repoRoot, process.env.CODESTORY_OSS_CORPUS_REPORT)
  : path.join(
      repoRoot,
      "target",
      "oss-language-corpus",
      "reports",
      "oss-language-corpus-latest.jsonl",
    );

function fail(message) {
  console.error(`language holdout integrity failed: ${message}`);
  process.exit(1);
}

function readJson(filePath) {
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (error) {
    fail(`could not read JSON ${filePath}: ${error.message}`);
  }
}

function gitHead(dir) {
  try {
    return execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  } catch (error) {
    fail(`could not read git HEAD in ${dir}: ${error.message}`);
  }
}

function parseReportRows(filePath) {
  try {
    return fs
      .readFileSync(filePath, "utf8")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line, index) => {
        try {
          return JSON.parse(line);
        } catch (error) {
          fail(`invalid JSONL row ${index + 1} in ${filePath}: ${error.message}`);
        }
      });
  } catch (error) {
    fail(`could not read corpus report ${filePath}: ${error.message}`);
  }
}

const manifest = readJson(manifestPath);
const tasks = Array.isArray(manifest.tasks) ? manifest.tasks : [manifest];
if (tasks.length !== 18) {
  fail(`expected 18 language-expansion tasks, found ${tasks.length}`);
}

const languages = new Set();
const repoByCommit = new Map();
for (const task of tasks) {
  const repo = task.repo || {};
  const repoName = String(repo.name || "").trim();
  const ref = String(repo.ref || "").trim();
  const taskLanguages = Array.isArray(repo.languages) ? repo.languages : [];
  if (!repoName || !ref || taskLanguages.length === 0) {
    fail(`task ${task.id || "<unknown>"} is missing repo name, ref, or languages`);
  }
  for (const language of taskLanguages) {
    languages.add(language);
  }
  const checkout = path.join(repoCacheDir, repoName);
  if (!fs.existsSync(path.join(checkout, ".git"))) {
    fail(`missing materialized repo checkout ${checkout}`);
  }
  const head = gitHead(checkout);
  if (head !== ref) {
    fail(`${repoName} HEAD ${head} did not match manifest ref ${ref}`);
  }
  repoByCommit.set(ref, { repoName, languages: taskLanguages });
}

if (languages.size !== 18) {
  fail(`expected 18 unique languages, found ${languages.size}`);
}

const rows = parseReportRows(reportPath);
if (rows.length !== 18) {
  fail(`expected 18 OSS corpus report rows, found ${rows.length}`);
}

let rawFiles = 0;
let indexedFiles = 0;
let nodes = 0;
let edges = 0;
let errors = 0;
let fatalErrors = 0;
for (const row of rows) {
  const commit = String(row.commit || "");
  if (!repoByCommit.has(commit)) {
    fail(`report row for ${row.repo_name || row.language || "<unknown>"} uses unexpected commit ${commit}`);
  }
  if (row.status !== "passed") {
    fail(`${row.language || row.repo_name || commit} report status is ${row.status}`);
  }
  const rawCount = Number(row.raw_without_codestory?.files);
  const indexedCount = Number(row.with_codestory?.indexed_files);
  const rowErrors = Number(row.with_codestory?.errors);
  const rowFatalErrors = Number(row.with_codestory?.fatal_errors);
  if (!Number.isFinite(rawCount) || !Number.isFinite(indexedCount)) {
    fail(`${row.language || commit} report is missing raw/indexed file counts`);
  }
  if (rawCount !== indexedCount) {
    fail(`${row.language || commit} indexed ${indexedCount} files but raw baseline found ${rawCount}`);
  }
  if (rowErrors !== 0 || rowFatalErrors !== 0) {
    fail(`${row.language || commit} reported errors=${rowErrors} fatal_errors=${rowFatalErrors}`);
  }
  rawFiles += rawCount;
  indexedFiles += indexedCount;
  nodes += Number(row.with_codestory?.nodes || 0);
  edges += Number(row.with_codestory?.edges || 0);
  errors += rowErrors;
  fatalErrors += rowFatalErrors;
}

console.log(
  `language holdout integrity ok: tasks=${tasks.length} languages=${languages.size} repos=${repoByCommit.size} raw_files=${rawFiles} indexed_files=${indexedFiles} nodes=${nodes} edges=${edges} errors=${errors} fatal_errors=${fatalErrors}`,
);
