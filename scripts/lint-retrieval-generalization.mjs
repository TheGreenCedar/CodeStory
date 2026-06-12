#!/usr/bin/env node
/**
 * CI guard: ban repo-specific path literals in retrieval integration production code.
 * Scope is Rust production retrieval integration files. Benchmark/eval harness
 * scripts and the env-gated eval probe module intentionally live outside this
 * guard because their manifests name holdout repos; keep that boundary explicit
 * instead of treating them as product code.
 * Scans Rust files after masking `#[cfg(test)]` items/modules so test fixtures
 * do not define the production contract.
 */
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const extraScanRoots = (
  process.env.CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_SCAN_ROOTS ?? ""
)
  .split(path.delimiter)
  .filter(Boolean);

const requiredScanDirs = [
  path.join(repoRoot, "crates", "codestory-cli", "src"),
  path.join(repoRoot, "crates", "codestory-indexer", "src"),
  path.join(repoRoot, "crates", "codestory-runtime", "src"),
  path.join(repoRoot, "crates", "codestory-retrieval"),
];

const requiredProductionOnlyFiles = [
  path.join(repoRoot, "crates", "codestory-cli", "src", "main.rs"),
  path.join(repoRoot, "crates", "codestory-runtime", "src", "agent", "orchestrator.rs"),
  path.join(repoRoot, "crates", "codestory-runtime", "src", "lib.rs"),
  path.join(repoRoot, "crates", "codestory-runtime", "src", "semantic_doc_text.rs"),
  path.join(repoRoot, "crates", "codestory-retrieval", "src", "ranker.rs"),
];

const missingRequiredPaths = [...requiredScanDirs, ...requiredProductionOnlyFiles]
  .filter((requiredPath) => !existsSync(requiredPath));
if (missingRequiredPaths.length > 0) {
  console.error("lint-retrieval-generalization: missing required production scan path(s)");
  for (const missingPath of missingRequiredPaths) {
    console.error(`  ${path.relative(repoRoot, missingPath)}`);
  }
  process.exit(2);
}

const scanDirs = [
  ...requiredScanDirs,
  ...extraScanRoots.filter((root) => root && existsSync(root)),
];

const productionOnlyFiles = requiredProductionOnlyFiles;

const evalOnlyProductionFiles = new Set([
  path.join(repoRoot, "crates", "codestory-runtime", "src", "agent", "eval_probes.rs"),
]);

const benchmarkIdentityScriptFiles = [
  path.join(repoRoot, "scripts", "codestory-agent-ab-benchmark.mjs"),
  path.join(repoRoot, "scripts", "codestory-manual-friction-check.mjs"),
  path.join(repoRoot, "scripts", "cross-repo-promotion-benchmark.mjs"),
  path.join(repoRoot, "scripts", "cross-repo-sourcetrail-queries.mjs"),
];

const missingBenchmarkBoundaryFiles = benchmarkIdentityScriptFiles
  .filter((scriptPath) => !existsSync(scriptPath));
if (missingBenchmarkBoundaryFiles.length > 0) {
  console.error("lint-retrieval-generalization: missing benchmark boundary script(s)");
  for (const missingPath of missingBenchmarkBoundaryFiles) {
    console.error(`  ${path.relative(repoRoot, missingPath)}`);
  }
  process.exit(2);
}

if (scanDirs.length === 0 && productionOnlyFiles.length === 0) {
  console.error("lint-retrieval-generalization: no scan roots found");
  process.exit(2);
}

const bannedPatterns = [
  "payload_config",
  "freelancer",
  "traderotate",
  "vscode",
  "codex-rs",
  "sourcetrail",
  "extHostCommands",
  "extensionService",
  "workbench\\.ts",
  "codex_exec::",
  "exec_events",
  "StorageAccess",
  "PersistentStorage",
  "SourceGroupCxxCdb",
  "IndexerJava",
  "ExecSharedCliOptions",
  "EventProcessorWithJsonOutput",
  "Subcommand::Exec",
  "ThreadStartParams",
  "TurnStartParams",
  "SocialEntries",
  "ElsewhereFeed",
  "src/lib_cxx",
  "src/lib_java",
  "src/lib/data/storage",
  "getPayloadClient",
  "comment_submission_guard",
  "axios",
  "redis",
  "ripgrep",
  "createInstance",
  "InterceptorManager",
  "dispatchRequest",
  "readQueryFromClient",
  "processCommand",
  "aeMain",
  "aeProcessEvents",
  "HiArgs",
  "SearchWorker",
  "search_parallel",
  "adapters\\.js",
  "server\\.c",
  "ae\\.c",
  "networking\\.c",
  "core/main\\.rs",
  "flags/hiargs\\.rs",
  "haystack\\.rs",
  "lib/axios\\.js",
  "lib/core/Axios\\.js",
];

const bannedLiteralPatterns = [
  "payload_collection",
];

const allowedPatternLines = [
  {
    pattern: "payload_collection",
    includes: "payload_collection_usage_source_targets",
  },
  {
    pattern: "payload_collection",
    includes: "related_payload_collection",
  },
];

const rankerFilenameLiteralPattern = /["'`][a-z0-9][a-z0-9._-]*\.[a-z0-9]+["'`]/i;

function isExcludedRustFile(filePath) {
  const relative = path.relative(repoRoot, filePath);
  const segments = relative.split(path.sep);
  const baseName = path.basename(filePath);
  return (
    segments.includes("tests")
    || baseName.endsWith("_tests.rs")
  );
}

function walkRustProductionFiles(root) {
  if (!existsSync(root)) {
    return [];
  }
  const files = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    const stat = statSync(current);
    if (stat.isDirectory()) {
      for (const entry of readdirSync(current)) {
        stack.push(path.join(current, entry));
      }
      continue;
    }
    if (stat.isFile() && current.endsWith(".rs") && !isExcludedRustFile(current)) {
      files.push(current);
    }
  }
  files.sort();
  return files;
}

function productionSource(filePath) {
  return maskCfgTestItems(readFileSync(filePath, "utf8"));
}

function maskCfgTestItems(text) {
  const spans = [];
  for (const group of findAttributeGroups(text)) {
    if (group.attributes.some((attribute) => attributeIsCfgTest(attribute.content))) {
      const itemEnd = findRustItemEnd(text, group.itemStart);
      spans.push([group.start, itemEnd ?? group.itemStart]);
      continue;
    }
    for (const attribute of group.attributes) {
      if (attributeIsCfgAttrTestOnly(attribute.content)) {
        spans.push([attribute.start, attribute.end]);
      }
    }
  }
  if (spans.length === 0) {
    return text;
  }

  const chars = text.split("");
  for (const [start, end] of spans) {
    for (let index = start; index < end && index < chars.length; index += 1) {
      if (chars[index] !== "\n" && chars[index] !== "\r") {
        chars[index] = " ";
      }
    }
  }
  return chars.join("");
}

function findAttributeGroups(text) {
  const attributes = findRustAttributes(text);
  const groups = [];
  for (const attribute of attributes) {
    const previous = groups[groups.length - 1];
    if (previous && isRustTriviaOnly(text, previous.end, attribute.start)) {
      previous.attributes.push(attribute);
      previous.end = attribute.end;
    } else {
      groups.push({
        start: attribute.start,
        end: attribute.end,
        attributes: [attribute],
        itemStart: attribute.end,
      });
    }
  }
  for (const group of groups) {
    group.itemStart = skipRustTrivia(text, group.end);
  }
  return groups;
}

function findRustAttributes(text) {
  const attributes = [];
  let index = 0;
  while (index < text.length) {
    const start = findNextAttributeStart(text, index);
    if (start == null) {
      break;
    }
    const end = findAttributeEnd(text, start);
    if (end == null) {
      break;
    }
    attributes.push({
      start,
      end,
      content: text.slice(start + 2, end - 1),
    });
    index = end;
  }
  return attributes;
}

function findNextAttributeStart(text, start) {
  let index = start;
  while (index < text.length) {
    const rawStringEnd = rustRawStringEnd(text, index);
    if (rawStringEnd != null) {
      index = rawStringEnd;
      continue;
    }

    const current = text[index];
    const next = text[index + 1];
    if (current === "/" && next === "/") {
      index = endOfLine(text, index);
      continue;
    }
    if (current === "/" && next === "*") {
      index = rustBlockCommentEnd(text, index + 2);
      continue;
    }
    if (current === '"') {
      index = rustStringEnd(text, index + 1, '"');
      continue;
    }
    if (current === "'" && rustLooksLikeCharLiteralStart(text, index)) {
      index = rustStringEnd(text, index + 1, "'");
      continue;
    }
    if (current === "#" && next === "[") {
      return index;
    }
    index += 1;
  }
  return null;
}

function findAttributeEnd(text, start) {
  let depth = 0;
  for (let index = start; index < text.length; index += 1) {
    const rawStringEnd = rustRawStringEnd(text, index);
    if (rawStringEnd != null) {
      index = rawStringEnd - 1;
      continue;
    }
    if (text[index] === '"') {
      index = rustStringEnd(text, index + 1, '"') - 1;
      continue;
    }
    if (text[index] === "'" && rustLooksLikeCharLiteralStart(text, index)) {
      index = rustStringEnd(text, index + 1, "'") - 1;
      continue;
    }
    if (text[index] === "[") {
      depth += 1;
    } else if (text[index] === "]") {
      depth -= 1;
      if (depth === 0) {
        return index + 1;
      }
    }
  }
  return null;
}

function skipRustTrivia(text, start) {
  let index = start;
  for (;;) {
    while (index < text.length && /\s/.test(text[index])) {
      index += 1;
    }
    if (text[index] === "/" && text[index + 1] === "/") {
      index = endOfLine(text, index);
      continue;
    }
    if (text[index] === "/" && text[index + 1] === "*") {
      index = rustBlockCommentEnd(text, index + 2);
      continue;
    }
    return index;
  }
}

function isRustTriviaOnly(text, start, end) {
  return skipRustTrivia(text, start) === end;
}

function attributeIsCfgTest(content) {
  const trimmed = content.trim();
  if (!trimmed.startsWith("cfg")) {
    return false;
  }
  const open = trimmed.indexOf("(");
  if (open < 0 || trimmed.slice(0, open).trim() !== "cfg") {
    return false;
  }
  const close = trimmed.lastIndexOf(")");
  if (close < open) {
    return false;
  }
  return cfgExpressionRequiresTest(trimmed.slice(open + 1, close));
}

function attributeIsCfgAttrTestOnly(content) {
  const trimmed = content.trim();
  if (!trimmed.startsWith("cfg_attr")) {
    return false;
  }
  const open = trimmed.indexOf("(");
  if (open < 0 || trimmed.slice(0, open).trim() !== "cfg_attr") {
    return false;
  }
  const close = trimmed.lastIndexOf(")");
  if (close < open) {
    return false;
  }
  const [condition] = splitTopLevelArgs(trimmed.slice(open + 1, close));
  return condition != null && cfgExpressionRequiresTest(condition);
}

function cfgExpressionRequiresTest(expression) {
  const trimmed = expression.trim();
  if (trimmed === "test") {
    return true;
  }
  if (/^not\s*\(\s*not\s*\(\s*test\s*\)\s*\)\s*$/.test(trimmed)) {
    return true;
  }
  const open = trimmed.indexOf("(");
  if (open < 0) {
    return false;
  }
  const head = trimmed.slice(0, open).trim();
  if (head !== "all") {
    return false;
  }
  const close = trimmed.lastIndexOf(")");
  if (close < open) {
    return false;
  }
  return splitTopLevelArgs(trimmed.slice(open + 1, close)).some(cfgExpressionRequiresTest);
}

function splitTopLevelArgs(text) {
  const args = [];
  let depth = 0;
  let start = 0;
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === '"') {
      index = rustStringEnd(text, index + 1, '"') - 1;
      continue;
    }
    if (text[index] === "(") {
      depth += 1;
    } else if (text[index] === ")") {
      depth -= 1;
    } else if (text[index] === "," && depth === 0) {
      args.push(text.slice(start, index).trim());
      start = index + 1;
    }
  }
  args.push(text.slice(start).trim());
  return args.filter(Boolean);
}

function findRustItemEnd(text, start) {
  const firstToken = findNextStructuralToken(text, start);
  if (!firstToken) {
    return endOfLine(text, start);
  }
  if (firstToken.token === ";") {
    return firstToken.index + 1;
  }

  let depth = 1;
  let index = firstToken.index + 1;
  while (index < text.length) {
    const token = findNextStructuralToken(text, index);
    if (!token) {
      return text.length;
    }
    if (token.token === "{") {
      depth += 1;
    } else if (token.token === "}") {
      depth -= 1;
      if (depth === 0) {
        return token.index + 1;
      }
    }
    index = token.index + 1;
  }
  return text.length;
}

function findNextStructuralToken(text, start) {
  let index = start;
  while (index < text.length) {
    const rawStringEnd = rustRawStringEnd(text, index);
    if (rawStringEnd != null) {
      index = rawStringEnd;
      continue;
    }

    const current = text[index];
    const next = text[index + 1];
    if (current === "/" && next === "/") {
      index = endOfLine(text, index);
      continue;
    }
    if (current === "/" && next === "*") {
      index = rustBlockCommentEnd(text, index + 2);
      continue;
    }
    if (current === '"') {
      index = rustStringEnd(text, index + 1, '"');
      continue;
    }
    if (current === "'" && rustLooksLikeCharLiteralStart(text, index)) {
      index = rustStringEnd(text, index + 1, "'");
      continue;
    }
    if (current === "{" || current === "}" || current === ";") {
      return { token: current, index };
    }
    index += 1;
  }
  return null;
}

function endOfLine(text, start) {
  const newline = text.indexOf("\n", start);
  return newline >= 0 ? newline + 1 : text.length;
}

function rustStringEnd(text, start, quote) {
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    if (escaped) {
      escaped = false;
      continue;
    }
    if (text[index] === "\\") {
      escaped = true;
      continue;
    }
    if (text[index] === quote) {
      return index + 1;
    }
  }
  return text.length;
}

function rustLooksLikeCharLiteralStart(text, index) {
  const next = text[index + 1];
  return next === "\\" || next === "{" || next === "}" || next === ";" || text[index + 2] === "'";
}

function rustRawStringEnd(text, start) {
  let index = start;
  if (text[index] === "b") {
    index += 1;
  }
  if (text[index] !== "r") {
    return null;
  }
  index += 1;
  let hashes = "";
  while (text[index] === "#") {
    hashes += "#";
    index += 1;
  }
  if (text[index] !== '"') {
    return null;
  }
  const terminator = `"${hashes}`;
  const end = text.indexOf(terminator, index + 1);
  return end >= 0 ? end + terminator.length : text.length;
}

function rustBlockCommentEnd(text, start) {
  let depth = 1;
  for (let index = start; index < text.length; index += 1) {
    if (text[index] === "/" && text[index + 1] === "*") {
      depth += 1;
      index += 1;
      continue;
    }
    if (text[index] === "*" && text[index + 1] === "/") {
      depth -= 1;
      index += 1;
      if (depth === 0) {
        return index + 1;
      }
    }
  }
  return text.length;
}

function scanProductionFile(filePath, pattern) {
  const re = new RegExp(pattern, "i");
  const lines = productionSource(filePath).split(/\r?\n/);
  const hits = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (re.test(lines[index]) && !lineAllowedForPattern(pattern, lines[index])) {
      hits.push(`${filePath}:${index + 1}:${lines[index]}`);
    }
  }
  return hits;
}

function scanProductionStringLiterals(filePath, pattern) {
  const re = new RegExp(pattern, "i");
  const lines = productionSource(filePath).split(/\r?\n/);
  const hits = [];
  for (let index = 0; index < lines.length; index += 1) {
    for (const literal of rustStringLiteralsOnLine(lines[index])) {
      if (re.test(literal) && !lineAllowedForPattern(pattern, lines[index])) {
        hits.push(`${filePath}:${index + 1}:${lines[index]}`);
        break;
      }
    }
  }
  return hits;
}

function rustStringLiteralsOnLine(line) {
  const literals = [];
  const stringLiteral = /(?:b?r#*"[^"]*"#*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')/g;
  let match;
  while ((match = stringLiteral.exec(line)) != null) {
    literals.push(match[0]);
  }
  return literals;
}

function lineAllowedForPattern(pattern, line) {
  return allowedPatternLines.some(
    (allowed) => allowed.pattern === pattern && line.includes(allowed.includes),
  );
}

function isEvalOnlyProductionFile(filePath) {
  return evalOnlyProductionFiles.has(path.resolve(filePath));
}

function scanRankerFilenameLiterals(filePath) {
  const lines = productionSource(filePath).split(/\r?\n/);
  const hits = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (rankerFilenameLiteralPattern.test(lines[index])) {
      hits.push(`${filePath}:${index + 1}:${lines[index]}`);
    }
  }
  return hits;
}

let failed = false;

const scanFiles = new Set(productionOnlyFiles);
for (const root of scanDirs) {
  for (const filePath of walkRustProductionFiles(root)) {
    scanFiles.add(filePath);
  }
}

if (scanFiles.size === 0) {
  console.error("lint-retrieval-generalization: no production Rust files found");
  process.exit(2);
}

for (const filePath of [...scanFiles].sort()) {
  if (!isEvalOnlyProductionFile(filePath)) {
    for (const pattern of bannedPatterns) {
      const hits = scanProductionFile(filePath, pattern);
      if (hits.length > 0) {
        console.error(
          `Banned pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
    for (const pattern of bannedLiteralPatterns) {
      const hits = scanProductionStringLiterals(filePath, pattern);
      if (hits.length > 0) {
        console.error(
          `Banned literal pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
  }
  if (filePath.endsWith(`${path.sep}ranker.rs`)) {
    const hits = scanRankerFilenameLiterals(filePath);
    if (hits.length > 0) {
      console.error(
        `Banned filename literals in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
}

if (failed) {
  console.error(
    "retrieval generalization lint failed: remove repo-specific literals from retrieval integration code",
  );
  process.exit(1);
}

console.log(
  `lint-retrieval-generalization: ok (${scanDirs.length} dir(s), ${scanFiles.size} production file(s), ${bannedPatterns.length} patterns)`,
);
