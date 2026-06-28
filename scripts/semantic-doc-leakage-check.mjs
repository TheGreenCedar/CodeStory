#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const ROOT = process.cwd();
const SEMANTIC_DOC_SOURCE = path.join(
  ROOT,
  "crates",
  "codestory-runtime",
  "src",
  "semantic_doc_text.rs",
);
const BENCHMARK_TASK_ROOT = path.join(ROOT, "benchmarks", "tasks");
const OVERLAP_THRESHOLD = Number(
  process.env.CODESTORY_SEMANTIC_DOC_LEAKAGE_JACCARD ?? "0.82",
);

function readText(file) {
  return fs.readFileSync(file, "utf8");
}

function lineOf(text, offset) {
  return text.slice(0, offset).split(/\r?\n/).length;
}

function normalize(value) {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9_+.]+/g, " ")
    .trim()
    .replace(/\s+/g, " ");
}

function tokens(value) {
  return new Set(normalize(value).split(" ").filter(Boolean));
}

function jaccard(left, right) {
  const leftTokens = tokens(left);
  const rightTokens = tokens(right);
  const union = new Set([...leftTokens, ...rightTokens]);
  if (union.size === 0) {
    return 0;
  }

  let intersection = 0;
  for (const token of leftTokens) {
    if (rightTokens.has(token)) {
      intersection += 1;
    }
  }
  return intersection / union.size;
}

function extractRuntimeConceptPhrases() {
  const text = readText(SEMANTIC_DOC_SOURCE);
  const start = text.indexOf("pub(crate) fn runtime_concept_phrases");
  const end = text.indexOf("pub(crate) fn semantic_path_aliases");
  if (start === -1 || end === -1 || end <= start) {
    throw new Error("Could not locate runtime_concept_phrases production block");
  }

  const block = text.slice(start, end);
  const phrases = [];
  const stringLiteral = /"([^"\\]*(?:\\.[^"\\]*)*)"/g;
  let match;
  while ((match = stringLiteral.exec(block)) !== null) {
    const value = match[1].replace(/\\n/g, " ");
    if (value.includes(" ") && value.length > 12) {
      phrases.push({
        file: path.relative(ROOT, SEMANTIC_DOC_SOURCE),
        line: lineOf(text, start + match.index),
        text: value,
      });
    }
  }
  return phrases;
}

function collectTaskFiles(root) {
  const files = [];
  if (!fs.existsSync(root)) {
    return files;
  }
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const fullPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectTaskFiles(fullPath));
    } else if (entry.isFile() && entry.name.endsWith(".task.json")) {
      files.push(fullPath);
    }
  }
  return files.sort();
}

function collectPromptFields(value, prompts) {
  if (Array.isArray(value)) {
    for (const item of value) {
      collectPromptFields(item, prompts);
    }
    return;
  }
  if (value == null || typeof value !== "object") {
    return;
  }
  if (typeof value.prompt === "string" && value.prompt.trim()) {
    prompts.push(value.prompt);
  }
  for (const child of Object.values(value)) {
    collectPromptFields(child, prompts);
  }
}

function extractBenchmarkQueries() {
  const queries = [];
  for (const file of collectTaskFiles(BENCHMARK_TASK_ROOT)) {
    const text = readText(file);
    const parsed = JSON.parse(text);
    const prompts = [];
    collectPromptFields(parsed, prompts);
    for (const prompt of prompts) {
      const promptOffset = text.indexOf(prompt);
      queries.push({
        file: path.relative(ROOT, file),
        line: promptOffset === -1 ? 1 : lineOf(text, promptOffset),
        text: prompt,
      });
    }
  }
  return queries;
}

function main() {
  const phrases = extractRuntimeConceptPhrases();
  const queries = extractBenchmarkQueries();
  if (queries.length === 0) {
    throw new Error("No benchmark task prompts found for semantic-doc leakage check");
  }
  const exact = [];
  const highOverlap = [];

  for (const phrase of phrases) {
    const normalizedPhrase = normalize(phrase.text);
    for (const query of queries) {
      const normalizedQuery = normalize(query.text);
      if (normalizedPhrase === normalizedQuery) {
        exact.push({ phrase, query, overlap: 1 });
        continue;
      }

      const overlap = jaccard(phrase.text, query.text);
      if (overlap >= OVERLAP_THRESHOLD) {
        highOverlap.push({ phrase, query, overlap });
      }
    }
  }

  if (exact.length === 0 && highOverlap.length === 0) {
    console.log(
      `semantic-doc leakage check passed: ${phrases.length} runtime phrases checked against ${queries.length} benchmark queries`,
    );
    return;
  }

  console.error(
    `semantic-doc leakage check failed: ${exact.length} exact and ${highOverlap.length} high-overlap benchmark query matches`,
  );

  for (const hit of [...exact, ...highOverlap].slice(0, 25)) {
    console.error(
      [
        `- overlap=${hit.overlap.toFixed(3)}`,
        `${hit.phrase.file}:${hit.phrase.line}`,
        JSON.stringify(hit.phrase.text),
        "matches",
        `${hit.query.file}:${hit.query.line}`,
        JSON.stringify(hit.query.text),
      ].join(" "),
    );
  }

  if (exact.length + highOverlap.length > 25) {
    console.error(`... ${exact.length + highOverlap.length - 25} more matches`);
  }
  process.exit(1);
}

main();
