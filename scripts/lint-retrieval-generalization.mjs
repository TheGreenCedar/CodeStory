#!/usr/bin/env node
/**
 * CI guard: keep production and release-control code independent from checked
 * evaluation/query corpora. Rust production is checked for derived corpus
 * content and structural paths after masking `#[cfg(test)]` items. Inventoried
 * non-Rust product/release surfaces are checked for direct and adjacent/split
 * corpus dependencies. Explicit benchmark/proof harnesses remain outside the
 * protected scan because they must load those corpora.
 */
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { sourcetrailQueries } from "./cross-repo-sourcetrail-queries.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const extraScanRoots = (
  process.env.CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_SCAN_ROOTS ?? ""
)
  .split(path.delimiter)
  .filter(Boolean);
const explicitScanRoots = (
  process.env.CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS ?? ""
)
  .split(path.delimiter)
  .filter(Boolean);
const explicitNonRustScanRoots = (
  process.env.CODESTORY_RETRIEVAL_GENERALIZATION_NON_RUST_SCAN_ROOTS ?? ""
)
  .split(path.delimiter)
  .filter(Boolean);

const groundingSkillDir = path.join(
  repoRoot,
  "plugins",
  "codestory",
  "skills",
  "codestory-grounding",
);

const protectedNonRustDirs = [
  path.join(repoRoot, "scripts"),
  path.join(repoRoot, ".github"),
  path.join(repoRoot, ".cursor", "rules"),
  path.join(repoRoot, "plugins", "codestory"),
  path.join(repoRoot, "docker"),
  path.join(repoRoot, "crates", "codestory-retrieval", "assets"),
];

const requiredProtectedNonRustFiles = [
  path.join(repoRoot, ".codex", "environments", "environment.toml"),
  path.join(repoRoot, ".cursor", "rules", "codestory.mdc"),
  path.join(groundingSkillDir, "SKILL.md"),
  path.join(repoRoot, "scripts", "codestory-evidence-provenance.mjs"),
  path.join(repoRoot, "scripts", "codestory-release-evidence-gate.mjs"),
  path.join(repoRoot, "scripts", "codex-worktree-setup.mjs"),
  path.join(repoRoot, "scripts", "codex-worktree-setup.ps1"),
  path.join(repoRoot, "scripts", "codex-worktree-setup.sh"),
  path.join(repoRoot, "scripts", "install-codestory.ps1"),
  path.join(repoRoot, ".github", "scripts", "check-codestory-release.py"),
  path.join(repoRoot, ".github", "scripts", "detect-codestory-release.py"),
  path.join(repoRoot, ".github", "scripts", "package-codestory-release.py"),
  path.join(repoRoot, ".github", "workflows", "auto-release.yml"),
  path.join(repoRoot, ".github", "workflows", "release.yml"),
];

const corpusHarnessNonRustFiles = new Set([
  path.join(repoRoot, "scripts", "autoresearch-pipeline-score.mjs"),
  path.join(repoRoot, "scripts", "codestory-agent-ab-benchmark.mjs"),
  path.join(repoRoot, "scripts", "codestory-agent-ab-score.mjs"),
  path.join(repoRoot, "scripts", "codestory-agent-value-score.mjs"),
  path.join(repoRoot, "scripts", "codestory-benchmark-contract.mjs"),
  path.join(repoRoot, "scripts", "codestory-language-holdout-integrity.mjs"),
  path.join(repoRoot, "scripts", "codestory-manual-friction-check.mjs"),
  path.join(repoRoot, "scripts", "cross-repo-sourcetrail-queries.mjs"),
  path.join(repoRoot, "scripts", "fetch-holdout-repos.mjs"),
  path.join(repoRoot, "scripts", "lint-retrieval-generalization.mjs"),
  path.join(repoRoot, "scripts", "measure-peak-memory.ps1"),
  path.join(repoRoot, "scripts", "prove-drill-packet-parity.mjs"),
  path.join(repoRoot, "scripts", "score-drill-ledger.mjs"),
  path.join(repoRoot, "scripts", "setup-retrieval-env.mjs"),
  path.join(repoRoot, "scripts", "setup-retrieval-env.ps1"),
  path.join(repoRoot, ".github", "scripts", "test-detect-codestory-release.py"),
  path.join(repoRoot, ".github", "workflows", "release-candidate-evidence.yml"),
  path.join(repoRoot, ".github", "workflows", "retrieval-sidecar-smoke.yml"),
].map((filePath) => path.resolve(filePath)));
const corpusSupportNonRustFiles = new Set([
  path.join(repoRoot, "scripts", "lint-retrieval-generalization.mjs"),
  path.join(repoRoot, "scripts", "setup-retrieval-env.mjs"),
  path.join(repoRoot, "scripts", "setup-retrieval-env.ps1"),
  path.join(repoRoot, ".github", "scripts", "test-detect-codestory-release.py"),
].map((filePath) => path.resolve(filePath)));
const allowedHarnessDependencyLines = [
  ["drill-suite.md", "scripts/score-drill-ledger.mjs"],
  ["retrieval-rollout.md", ".github/workflows/retrieval-sidecar-smoke.yml"],
].map(([fileName, includes]) => ({
  filePath: path.join(groundingSkillDir, "references", fileName),
  includes,
}));

const executableJavaScriptExtensions = new Set([
  ".cjs", ".js", ".mjs", ".ts", ".tsx",
]);
const hashCommentExtensions = new Set([
  ".ps1", ".py", ".sh", ".toml", ".yaml", ".yml",
]);
const javaScriptStringOrCommentPattern = /("(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|`(?:\\[\s\S]|[^`\\])*`)|(\/\*[\s\S]*?\*\/|\/\/[^\r\n]*)/g;
const javaScriptTemplateTokenPattern = /("(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*')|(\$\{|[{}])|(\/\*[\s\S]*?\*\/|\/\/[^\r\n]*)/g;
const protectedNonRustExtensions = new Set([
  ...executableJavaScriptExtensions,
  ".json", ".md", ".mdc", ".ps1", ".py", ".sh", ".toml", ".yaml", ".yml",
]);
const agentInstructionExtensions = new Set([".md", ".mdc"]);
const agentInstructionPathFragments = [
  path.join("plugins", "codestory", "skills", "codestory-grounding"),
  path.join(".cursor", "rules"),
];

const defaultNonRustScanRoots = protectedNonRustDirs;
const usesDefaultNonRustScanRoots = explicitNonRustScanRoots.length === 0;
const nonRustScanRoots = usesDefaultNonRustScanRoots
  ? defaultNonRustScanRoots
  : explicitNonRustScanRoots.filter((root) => root && existsSync(root));

if (usesDefaultNonRustScanRoots) {
  const missingProtectedPaths = [
    ...defaultNonRustScanRoots,
    ...requiredProtectedNonRustFiles,
    ...corpusHarnessNonRustFiles,
  ].filter((requiredPath) => !existsSync(requiredPath));
  if (missingProtectedPaths.length > 0) {
    console.error("lint-retrieval-generalization: missing protected non-Rust path(s)");
    for (const missingPath of missingProtectedPaths) {
      console.error(`  ${path.relative(repoRoot, missingPath)}`);
    }
    process.exit(2);
  }
}

const structuralScanDirs = readdirSync(path.join(repoRoot, "crates"), { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && entry.name !== "codestory-bench")
  .map((entry) => path.join(repoRoot, "crates", entry.name, "src"))
  .filter(existsSync);

const requiredScanDirs = [
  path.join(repoRoot, "crates", "codestory-runtime", "src", "agent"),
  path.join(repoRoot, "crates", "codestory-retrieval", "src"),
];

const requiredProductionOnlyFiles = [];

const usesDefaultScanRoots = explicitScanRoots.length === 0;
const missingRequiredPaths = usesDefaultScanRoots
  ? [...requiredScanDirs, ...requiredProductionOnlyFiles]
    .filter((requiredPath) => !existsSync(requiredPath))
  : [];
if (missingRequiredPaths.length > 0) {
  console.error("lint-retrieval-generalization: missing required production scan path(s)");
  for (const missingPath of missingRequiredPaths) {
    console.error(`  ${path.relative(repoRoot, missingPath)}`);
  }
  process.exit(2);
}

const scanDirs = [
  ...(usesDefaultScanRoots
    ? requiredScanDirs
    : explicitScanRoots.filter((root) => root && existsSync(root))),
  ...extraScanRoots.filter((root) => root && existsSync(root)),
];

const productionOnlyFiles = usesDefaultScanRoots ? requiredProductionOnlyFiles : [];

const evalOnlyProductionFiles = new Set([
  path.join(repoRoot, "crates", "codestory-runtime", "src", "agent", "eval_probes.rs"),
]);

const benchmarkIdentityScriptFiles = [
  path.join(repoRoot, "scripts", "codestory-agent-ab-benchmark.mjs"),
  path.join(repoRoot, "scripts", "codestory-manual-friction-check.mjs"),
  path.join(repoRoot, "scripts", "cross-repo-sourcetrail-queries.mjs"),
];
const benchmarkPromptScriptOverride =
  process.env.CODESTORY_RETRIEVAL_GENERALIZATION_PROMPT_SCRIPT;
const benchmarkPromptScriptFiles = [
  {
    filePath: benchmarkPromptScriptOverride
      ? path.resolve(benchmarkPromptScriptOverride)
      : path.join(repoRoot, "scripts", "codestory-agent-ab-benchmark.mjs"),
    startMarker: "const PUBLIC_REPOS =",
    endMarker: "const ALL_REPOS =",
  },
];
const benchmarkTaskRoot = path.join(repoRoot, "benchmarks", "tasks");
const benchmarkEvalProbeManifestPath = path.join(benchmarkTaskRoot, "eval-probes.json");
const benchmarkEvalProbeSourcePath = path.join(
  repoRoot,
  "crates",
  "codestory-runtime",
  "src",
  "agent",
  "eval_probes.rs",
);
const evalCorpusRoots = [
  benchmarkTaskRoot,
  path.join(repoRoot, "crates", "codestory-cli", "tests", "fixtures", "packet_search_eval"),
  path.join(repoRoot, "crates", "codestory-bench", "tests", "fixtures", "agent_quality"),
];

const missingBenchmarkBoundaryFiles = [
  ...benchmarkIdentityScriptFiles,
  ...benchmarkPromptScriptFiles.map(({ filePath }) => filePath),
  benchmarkEvalProbeManifestPath,
  benchmarkEvalProbeSourcePath,
  ...evalCorpusRoots,
].filter((scriptPath, index, paths) =>
  paths.indexOf(scriptPath) === index && !existsSync(scriptPath)
);
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

const evalCorpusBoundaryPatternList = evalCorpusBoundaryPatterns();
const evalCorpusCompactPatternList = compactBoundaryPatterns(
  evalCorpusBoundaryPatternList,
);
const corpusHarnessDependencyPatternList = corpusHarnessDependencyPatterns();
const corpusHarnessDependencyRegexes = corpusHarnessDependencyPatternList.map(
  (pattern) => new RegExp(`(?:^|[^a-z0-9_.-])${pattern}(?=$|[^a-z0-9_.-])`, "i"),
);
const corpusHarnessCompactPatternList = compactBoundaryPatterns(
  corpusHarnessDependencyPatternList,
);

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
  "data[/\\\\]indexer",
  "ExecSharedCliOptions",
  "EventProcessorWithJsonOutput",
  "Subcommand::Exec",
  "ThreadStartParams",
  "TurnStartParams",
  "chinook",
  "mdn",
  "okio",
  "monolog",
  "alamofire",
  "ChinookDatabase",
  "form-validation",
  "commonMain/kotlin/okio",
  "src/Monolog",
  "Source/Core/Session\\.swift",
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
  "StringUtils",
  "commons-lang",
  "PreparedRequest",
  "HTTPAdapter",
  "createApplication",
  "app\\.use",
  "lib/express\\.js",
  "Jekyll",
  "LogRecord",
  "AbstractProcessingHandler",
  "useSWR",
  "swr",
  "gin\\.go",
  "RouterGroup\\.Handle",
  "Engine\\.addRoute",
  "Engine\\.handleHTTPRequest",
  "AutoMapper",
  "TypeMapPlanBuilder",
  "RealBufferedSource",
  "RealBufferedSink",
  "DataRequest",
  "SessionDelegate",
  "novalidate",
  "showError",
  "source/animate\\.css",
  "nvm",
  "install\\.sh\\s+nvm",
  "bash_completion\\s+__nvm",
  "--with-holdout-clone",
  ...evalCorpusBoundaryPatternList,
  ...benchmarkManifestDerivedPatterns(),
  ...benchmarkEvalProbeDerivedPatterns(),
  ...benchmarkScriptPromptDerivedPatterns(),
  ...benchmarkQueryCatalogDerivedPatterns(),
];

const bannedLiteralPatterns = [
  "payload_collection",
];

const bannedCompactPatterns = [
  "swr",
  "useswr",
  "stringutils",
  "charsequenceutils",
  "preparedrequest",
  "httpadapter",
  "createapplication",
  "appuse",
  "jekyll",
  "logrecord",
  "automapper",
  "realbufferedsource",
  "realbufferedsink",
  "datarequest",
  "sessiondelegate",
  "sourceanimatecss",
  ...evalCorpusCompactPatternList,
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

function evalCorpusBoundaryPatterns() {
  const corpusFiles = [
    ...evalCorpusRoots.flatMap((root) => walkFiles(root, () => true)),
    ...benchmarkIdentityScriptFiles,
    benchmarkEvalProbeSourcePath,
  ];
  if (corpusFiles.length === 0) {
    throw new Error("eval/query corpus boundary contains no files");
  }
  return [
    ...evalCorpusRoots.map((root) => path.relative(repoRoot, root).replaceAll(path.sep, "/")),
    ...corpusFiles.map((filePath) => path.relative(repoRoot, filePath).replaceAll(path.sep, "/")),
    ...benchmarkIdentityScriptFiles.map((filePath) => path.basename(filePath)),
  ].map(escapeRegExp);
}

function corpusHarnessDependencyPatterns() {
  const paths = new Set(
    [...corpusHarnessNonRustFiles]
      .filter((filePath) => !corpusSupportNonRustFiles.has(filePath))
      .flatMap((filePath) => [
        path.relative(repoRoot, filePath).replaceAll(path.sep, "/"),
        path.basename(filePath),
      ]),
  );
  return [...paths].map(escapeRegExp);
}

function compactBoundaryPatterns(boundaryPatterns) {
  return boundaryPatterns
    .map((pattern) => compactProductionSource(pattern.replaceAll("\\", "")))
    .filter((pattern) => pattern.length >= 12);
}

function benchmarkManifestDerivedPatterns() {
  if (!existsSync(benchmarkTaskRoot)) {
    throw new Error(`benchmark task root is missing: ${benchmarkTaskRoot}`);
  }
  const markers = new Set();
  const manifestFiles = walkFiles(
    benchmarkTaskRoot,
    (candidate) => candidate.endsWith(".task.json"),
  );
  if (manifestFiles.length === 0) {
    throw new Error(`benchmark task root has no .task.json manifests: ${benchmarkTaskRoot}`);
  }
  let parsedTaskCount = 0;
  for (const filePath of manifestFiles) {
    let manifest;
    try {
      manifest = JSON.parse(readFileSync(filePath, "utf8"));
    } catch (error) {
      throw new Error(`failed to parse benchmark manifest ${filePath}: ${error}`);
    }
    for (const task of benchmarkManifestTasks(manifest)) {
      parsedTaskCount += 1;
      addSpecificMarker(markers, task.id);
      addRepoMarkers(markers, task.repo);
      addSpecificMarker(markers, task.prompt, { allowExactPhrase: true });
      for (const expectedFile of task.expected_files ?? []) {
        addSpecificMarker(markers, expectedFile, { allowSpecificComposite: true });
      }
      for (const expectedFile of task.expected_verification_files ?? []) {
        addSpecificMarker(markers, expectedFile, { allowSpecificComposite: true });
      }
      for (const symbol of task.expected_symbols ?? []) {
        if (typeof symbol === "string") {
          addSpecificMarker(markers, symbol);
        } else {
          addSpecificMarker(markers, symbol?.name);
          addSpecificMarker(markers, symbol?.qualified_name, { allowSpecificComposite: true });
          addSpecificMarker(markers, symbol?.path, { allowSpecificComposite: true });
        }
      }
      for (const claim of task.expected_claims ?? []) {
        addSpecificMarker(markers, claim?.text, { allowExactPhrase: true });
      }
      for (const claim of task.forbidden_claims ?? []) {
        addSpecificMarker(markers, claim?.text, { allowExactPhrase: true });
      }
    }
  }
  if (parsedTaskCount === 0 || markers.size === 0) {
    throw new Error("benchmark manifests produced no generalization markers");
  }
  return [...markers].sort().map(escapeRegExp);
}

function benchmarkScriptPromptDerivedPatterns() {
  const markers = new Set();
  const stringLiteralSource = javascriptStringLiteralSource();
  const promptProperty = new RegExp(`\\bprompt\\s*:\\s*(${stringLiteralSource})`, "g");

  for (const { filePath, startMarker, endMarker } of benchmarkPromptScriptFiles) {
    const source = readFileSync(filePath, "utf8");
    const start = source.indexOf(startMarker);
    const end = source.indexOf(endMarker, start + startMarker.length);
    if (start < 0 || end < 0 || end <= start) {
      throw new Error(
        `benchmark prompt script is missing corpus boundary markers: ${filePath}`,
      );
    }
    const corpusSource = source.slice(start, end);
    const discoveredPromptCount = [...corpusSource.matchAll(/\bprompt\s*:/g)].length;
    let parsedPromptCount = 0;
    let match;
    while ((match = promptProperty.exec(corpusSource)) != null) {
      parsedPromptCount += 1;
      addSpecificMarker(markers, decodeJavaScriptStringLiteral(match[1]), {
        allowExactPhrase: true,
      });
    }
    if (parsedPromptCount === 0 || parsedPromptCount !== discoveredPromptCount) {
      throw new Error(
        `benchmark prompt script discovered ${discoveredPromptCount} prompt properties but parsed ${parsedPromptCount} literal prompts: ${filePath}`,
      );
    }
    promptProperty.lastIndex = 0;
  }

  return [...markers].sort().map(escapeRegExp);
}

function benchmarkQueryCatalogDerivedPatterns() {
  const markers = new Set();
  if (!Array.isArray(sourcetrailQueries) || sourcetrailQueries.length === 0) {
    throw new Error("cross-repo query catalog exported no queries");
  }
  for (const [index, entry] of sourcetrailQueries.entries()) {
    if (
      typeof entry?.query !== "string"
      || !Array.isArray(entry?.expect)
      || entry.expect.some((expected) => typeof expected !== "string")
    ) {
      throw new Error(`cross-repo query catalog entry ${index} has an invalid shape`);
    }
    addSpecificMarker(markers, entry.query, { allowExactPhrase: true });
    for (const expected of entry.expect) {
      if (queryCatalogExpectedMarkerIsSpecific(expected)) {
        addSpecificMarker(markers, expected, { allowSpecificComposite: true });
      }
    }
  }

  return [...markers].sort().map(escapeRegExp);
}

function benchmarkEvalProbeDerivedPatterns() {
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(benchmarkEvalProbeManifestPath, "utf8"));
  } catch (error) {
    throw new Error(`failed to parse eval probe manifest: ${error}`);
  }
  const markers = new Set();
  for (const group of ["flow_hint_rules", "required_probe_rules"]) {
    const rules = manifest?.[group];
    if (!Array.isArray(rules)) {
      throw new Error(`eval probe manifest is missing ${group}`);
    }
    for (const [index, rule] of rules.entries()) {
      if (!Array.isArray(rule?.queries) || rule.queries.some((query) => typeof query !== "string")) {
        throw new Error(`eval probe manifest ${group}[${index}] has invalid queries`);
      }
      for (const query of rule.queries) {
        addSpecificMarker(markers, query, { allowSpecificComposite: true });
      }
    }
  }
  if (!Array.isArray(manifest?.citation_rank_adjustments)) {
    throw new Error("eval probe manifest is missing citation_rank_adjustments");
  }
  for (const adjustment of manifest.citation_rank_adjustments) {
    addSpecificMarker(markers, adjustment?.normalized_display);
    addSpecificMarker(markers, adjustment?.path, { allowSpecificComposite: true });
  }

  const source = readFileSync(benchmarkEvalProbeSourcePath, "utf8");
  addEvalProbeSourceQueryMarkers(markers, source);
  if (markers.size === 0) {
    throw new Error("eval probe corpora produced no generalization markers");
  }
  return [...markers].sort().map(escapeRegExp);
}

function addEvalProbeSourceQueryMarkers(markers, source) {
  let parsedLiteralCount = 0;
  const singleQueryCall =
    /\bpush_unique_term\(\s*queries\s*,\s*("(?:\\.|[^"\\])*")\s*\)/g;
  let singleMatch;
  while ((singleMatch = singleQueryCall.exec(source)) != null) {
    parsedLiteralCount += 1;
    addSpecificMarker(markers, JSON.parse(singleMatch[1]), {
      allowSpecificComposite: true,
    });
  }

  const queryArrayCall =
    /\bpush_unique_terms\(\s*queries\s*,\s*&\[([\s\S]*?)\]\s*,?\s*\)/g;
  let arrayMatch;
  while ((arrayMatch = queryArrayCall.exec(source)) != null) {
    const body = arrayMatch[1];
    const literals = staticStringLiteralSpans(body);
    if (literals.length === 0 || rustArrayNonLiteralRemainder(body, literals).trim() !== "") {
      throw new Error("eval probe source query array contains an unparsed entry");
    }
    for (const { literal } of literals) {
      parsedLiteralCount += 1;
      addSpecificMarker(markers, staticStringLiteralContent(literal), {
        allowSpecificComposite: true,
      });
    }
  }

  if (parsedLiteralCount === 0) {
    throw new Error("eval probe source produced no static query literals");
  }
}

function rustArrayNonLiteralRemainder(body, literals) {
  const chars = body.split("");
  for (const literal of literals) {
    for (let index = literal.startOffset; index < literal.endOffset; index += 1) {
      chars[index] = " ";
    }
  }
  return chars.join("").replaceAll(",", "");
}

function javascriptStringLiteralSource() {
  return '(?:"(?:\\\\.|[^"\\\\])*"|\'(?:\\\\.|[^\'\\\\])*\'|`(?:\\\\.|[^`\\\\])*`)';
}

function queryCatalogExpectedMarkerIsSpecific(value) {
  const normalized = value.toLowerCase().replace(/[^a-z0-9]+/g, "");
  if (["application", "typename"].includes(normalized)) {
    return false;
  }
  return /[\\/.:]/.test(value) || /[a-z][A-Z]/.test(value);
}

function decodeJavaScriptStringLiteral(literal) {
  const quote = literal[0];
  if (quote === '"') {
    return JSON.parse(literal);
  }
  const contents = literal.slice(1, -1);
  if (quote === "`" && contents.includes("${")) {
    throw new Error("template expressions are not supported in benchmark string literals");
  }
  const jsonContents = contents
    .replaceAll(`\\${quote}`, quote)
    .replaceAll('"', '\\"');
  try {
    return JSON.parse(`"${jsonContents}"`);
  } catch (error) {
    throw new Error(`failed to decode benchmark string literal ${literal}: ${error}`);
  }
}

function benchmarkManifestTasks(manifest) {
  if (Array.isArray(manifest?.tasks)) {
    return manifest.tasks.filter((task) => task && typeof task === "object");
  }
  if (manifest && typeof manifest === "object") {
    return [manifest];
  }
  return [];
}

function addRepoMarkers(markers, repo) {
  addSpecificMarker(markers, repo?.name);
  for (const slug of repoUrlSlugs(repo?.url)) {
    addSpecificMarker(markers, slug);
  }
}

function repoUrlSlugs(url) {
  if (typeof url !== "string" || url.trim().length === 0) {
    return [];
  }
  const trimmed = url.trim().replace(/\.git$/i, "");
  let pathname;
  try {
    pathname = new URL(trimmed).pathname;
  } catch {
    pathname = trimmed;
  }
  const parts = pathname
    .split(/[\\/]/)
    .map((part) => part.trim())
    .filter(Boolean);
  if (parts.length === 0) {
    return [];
  }
  const repoName = parts[parts.length - 1];
  const ownerName = parts.length >= 2
    ? `${parts[parts.length - 2]}/${repoName}`
    : null;
  return [ownerName, repoName].filter(Boolean);
}

function walkFiles(root, predicate) {
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
    if (stat.isFile() && predicate(current)) {
      files.push(current);
    }
  }
  return files;
}

function walkProtectedNonRustFiles(root) {
  return walkFiles(root, (filePath) => {
    const extension = path.extname(filePath).toLowerCase();
    if (!protectedNonRustExtensions.has(extension)) {
      return false;
    }
    if (
      agentInstructionExtensions.has(extension)
      && !agentInstructionPathFragments.some((fragment) =>
        filePath.includes(`${path.sep}${fragment}${path.sep}`)
      )
    ) return false;
    if (!usesDefaultNonRustScanRoots) {
      return true;
    }
    if (corpusHarnessNonRustFiles.has(path.resolve(filePath))) {
      return false;
    }
    const segments = path.relative(repoRoot, filePath).split(path.sep);
    return (
      !segments.includes("tests")
      && !segments.includes("fixtures")
    );
  });
}

function addSpecificMarker(markers, value, options = {}) {
  if (typeof value !== "string") {
    return;
  }
  const marker = value.trim();
  if (marker.length < 8 || benchmarkMarkerTooGeneric(marker, options)) {
    return;
  }
  markers.add(marker);
}

function benchmarkMarkerTooGeneric(marker, options = {}) {
  if (options.allowExactPhrase && marker.split(/\s+/).length >= 5) {
    return false;
  }
  if (
    options.allowSpecificComposite
    && /[\\/.:]/.test(marker)
    && /[a-zA-Z]/.test(marker)
  ) {
    return false;
  }
  const normalized = marker.toLowerCase().replace(/[^a-z0-9]+/g, "");
  return (
    normalized.length < 8 ||
    [
      "codestory",
      "request",
      "requests",
      "response",
      "responses",
      "dispatch",
      "router",
      "routepath",
      "approute",
      "comments",
      "indexfile",
      "runindex",
      "buildindex",
      "servicesrs",
      "sourcegroup",
      "indexercommand",
      "subcommand",
      "eventprocessor",
      "jsonoutput",
      "jsonlevent",
      "schema",
      "source",
      "storage",
      "indexing",
      "configuration",
      "validation",
      "serialize",
      "serializes",
      "serialized",
      "serialization",
      "foreignkey",
      "references",
      "formatto",
      "formaterror",
      "formaterrorcode",
      "formatwindowserror",
      "internalmutate",
    ].includes(normalized)
  );
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

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

function prepareProductionFile(filePath) {
  const production = productionSource(filePath);
  return {
    filePath,
    production,
    lines: production.split(/\r?\n/),
    literals: null,
  };
}

function prepareNonRustFile(filePath) {
  const production = readFileSync(filePath, "utf8");
  const extension = path.extname(filePath).toLowerCase();
  const staticSource = maskNonRustComments(production, extension);
  const lines = staticSource.split(/\r?\n/);
  return {
    filePath,
    production,
    lines,
    logicalLines: logicalNonRustLines(lines, extension),
    sourceLines: production.split(/\r?\n/),
    staticSource,
    literals: null,
  };
}

function maskNonRustComments(source, extension) {
  if (executableJavaScriptExtensions.has(extension)) {
    return maskJavaScriptComments(source);
  }
  if (hashCommentExtensions.has(extension)) {
    return maskHashComments(source, extension);
  }
  return source;
}

function maskJavaScriptComments(source) {
  return source.replace(
    javaScriptStringOrCommentPattern,
    (token, stringLiteral) => stringLiteral?.startsWith("`")
      ? maskTemplateExpressionComments(stringLiteral)
      : stringLiteral ?? token.replace(/[^\r\n]/g, " "),
  );
}

function maskTemplateExpressionComments(template) {
  let depth = 0;
  return template.replace(
    javaScriptTemplateTokenPattern,
    (token, stringLiteral, brace, comment) => {
      if (stringLiteral != null) return token;
      if (brace != null) {
        if (brace === "${" || (brace === "{" && depth > 0)) depth += 1;
        else if (brace === "}" && depth > 0) depth -= 1;
        return token;
      }
      return depth > 0 ? comment.replace(/[^\r\n]/g, " ") : token;
    },
  );
}

function maskHashComments(source, extension) {
  const masked = source.split("");
  const escape = extension === ".ps1" ? "`" : "\\";
  const outsideEscape = extension === ".ps1" || extension === ".sh";
  const doubledQuoteEscape = extension === ".ps1" || extension === ".yaml"
    || extension === ".yml";
  let quote = null;

  for (let index = 0; index < source.length; index += 1) {
    if (quote != null) {
      if (source.startsWith(quote, index)) {
        if (
          quote.length === 1
          && doubledQuoteEscape
          && source[index + 1] === quote
        ) {
          index += 1;
        } else {
          index += quote.length - 1;
          quote = null;
        }
      } else if (
        !quote.startsWith("'")
        && source[index] === escape
        && index + 1 < source.length
      ) {
        index += 1;
      }
      continue;
    }

    if (outsideEscape && source[index] === escape && index + 1 < source.length) {
      index += 1;
      continue;
    }
    if (
      source[index] === "'"
      && (extension === ".yaml" || extension === ".yml")
      && /[a-zA-Z0-9_]/.test(source[index - 1] ?? "")
    ) {
      continue;
    }
    if (source[index] === "\"" || source[index] === "'") {
      const delimiter = source[index];
      quote = (extension === ".py" || extension === ".toml")
        && source.startsWith(delimiter.repeat(3), index)
        ? delimiter.repeat(3)
        : delimiter;
      index += quote.length - 1;
      continue;
    }
    if (source[index] !== "#") {
      continue;
    }
    if (
      extension === ".sh"
      && index > 0
      && !/[\s;&|()]/.test(source[index - 1])
    ) {
      continue;
    }
    while (index < source.length && source[index] !== "\r" && source[index] !== "\n") {
      masked[index] = " ";
      index += 1;
    }
    index -= 1;
  }
  return masked.join("");
}

function logicalNonRustLines(lines, extension) {
  const yamlScalarKinds = extension === ".yaml" || extension === ".yml"
    ? yamlBlockScalarKinds(lines)
    : null;
  const logicalLines = [];
  for (let start = 0; start < lines.length; start += 1) {
    let end = start;
    let text = lines[end];
    while (
      end + 1 < lines.length
      && lineContinuationMarker(text, extension, yamlScalarKinds?.[end]) != null
    ) {
      text = yamlScalarKinds?.[end]?.endsWith(">")
        ? `${text} ${lines[end + 1].trimStart()}`
        : text.slice(0, -1) + lines[end + 1].trimStart();
      end += 1;
    }
    logicalLines.push({
      text,
      startLine: start + 1,
      endLine: end + 1,
      shellLike: extension === ".sh"
        || yamlScalarKinds?.[start]?.startsWith("run")
        || isYamlRunLine(lines[start], extension),
    });
    start = end;
  }
  return logicalLines;
}

function yamlBlockScalarKinds(lines) {
  const scalarKinds = Array(lines.length).fill(null);
  let blockIndent = null;
  let scalarKind = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const indent = line.match(/^\s*/)[0].length;
    if (blockIndent != null) {
      if (line.trim() === "" || indent > blockIndent) {
        scalarKinds[index] = scalarKind;
        continue;
      }
      blockIndent = null;
    }
    const header = line.trimStart().match(
      /^([^#\n]+):\s*([|>])(?:[1-9][+-]?|[+-][1-9]?)?\s*$/,
    );
    if (header != null) {
      blockIndent = indent;
      const shellPrefix = /^(?:-\s*)?run$/.test(header[1].trim()) ? "run" : "";
      scalarKind = `${shellPrefix}${header[2]}`;
    }
  }
  return scalarKinds;
}

function isYamlRunLine(line, extension) {
  return (extension === ".yaml" || extension === ".yml")
    && /^\s*(?:-\s*)?run\s*:/.test(line);
}

function lineContinuationMarker(line, extension, yamlScalarKind) {
  let marker = null;
  if (executableJavaScriptExtensions.has(extension) || extension === ".sh") {
    marker = "\\";
  } else if (extension === ".ps1") {
    marker = "`";
  } else if ((extension === ".yaml" || extension === ".yml") && yamlScalarKind != null) {
    marker = "\\`".includes(line.at(-1)) ? line.at(-1) : null;
  }
  if (marker == null || !line.endsWith(marker)) {
    return null;
  }
  const escape = extension === ".ps1" || marker === "`" ? "`" : "\\";
  const quote = openQuoteAtEnd(line.slice(0, -1), escape);
  if (quote === "'") {
    return null;
  }
  if (executableJavaScriptExtensions.has(extension) && quote == null) {
    return null;
  }
  return marker;
}

function openQuoteAtEnd(text, escape) {
  let quote = null;
  for (let index = 0; index < text.length; index += 1) {
    if (quote == null) {
      if (text[index] === "\"" || text[index] === "'" || text[index] === "`") {
        quote = text[index];
      }
      continue;
    }
    if (quote !== "'" && text[index] === escape) {
      index += 1;
    } else if (text[index] === quote) {
      quote = null;
    }
  }
  return quote;
}

function scanCorpusHarnessDependencies(prepared) {
  const hits = [];
  for (const line of prepared.logicalLines) {
    const normalizedLine = normalizeNativeSeparators(line.text, line.shellLike);
    const hasProtectedDependency = corpusHarnessDependencyRegexes.some((pattern) =>
      pattern.test(normalizedLine)
      && !allowedHarnessDependencyLines.some(({ filePath, includes }) =>
        path.resolve(prepared.filePath) === filePath
        && line.text.includes(includes)
        && pattern.test(includes)
      )
    );
    if (hasProtectedDependency) {
      hits.push(
        `${prepared.filePath}:${line.startLine}:${prepared.sourceLines[line.startLine - 1]}`,
      );
    }
  }
  return hits;
}

function scanProductionFile(prepared, patterns, combinedRe) {
  const lines = prepared.logicalLines ?? prepared.lines.map((text, index) => ({
    text,
    startLine: index + 1,
    endLine: index + 1,
  }));
  const sourceLines = prepared.sourceLines ?? prepared.lines;
  const hitsByPattern = new Map();
  for (const line of lines) {
    const normalizedLine = normalizeNativeSeparators(line.text, line.shellLike);
    if (!combinedRe.test(normalizedLine)) {
      continue;
    }
    for (const { pattern, re } of patterns) {
      const sourceLine = sourceLines[line.startLine - 1];
      if (re.test(normalizedLine) && !lineAllowedForPattern(pattern, sourceLine)) {
        if (!hitsByPattern.has(pattern)) {
          hitsByPattern.set(pattern, []);
        }
        hitsByPattern.get(pattern).push(`${prepared.filePath}:${line.startLine}:${sourceLine}`);
      }
    }
  }
  return hitsByPattern;
}

function scanProductionStringLiterals(prepared, pattern, re) {
  const lines = prepared.lines;
  const hits = [];
  for (let index = 0; index < lines.length; index += 1) {
    for (const literal of staticStringLiteralsOnLine(lines[index])) {
      if (
        re.test(normalizeNativeSeparators(literal))
        && !lineAllowedForPattern(pattern, lines[index])
      ) {
        hits.push(`${prepared.filePath}:${index + 1}:${lines[index]}`);
        break;
      }
    }
  }
  return hits;
}

function compactProductionSource(text) {
  return staticStringLiteralContent(text)
    .replace(/["'`]/g, "")
    .replace(/[^a-zA-Z0-9]+/g, "")
    .toLowerCase();
}

function normalizeNativeSeparators(text, shellLike = false) {
  const normalized = text.replaceAll("\\", "/");
  return shellLike ? normalized.replace(/["']/g, "") : normalized;
}

function staticStringLiteralContent(literal) {
  const raw = literal.match(/^b?r(#+)?"([\s\S]*)"(#*)$/);
  if (raw && (raw[1] ?? "") === raw[3]) {
    return raw[2];
  }
  if (literal.length >= 2 && ["\"", "'", "`"].includes(literal[0])) {
    return literal.slice(1, -1);
  }
  return literal;
}

function scanProductionCompactPatterns(
  prepared,
  marker,
  minimumLiteralCount = 1,
  allowSurroundingText = false,
) {
  const production = prepared.staticSource ?? prepared.production;
  const markerLower = marker.toLowerCase();
  const hits = [];
  if (prepared.literals == null) {
    prepared.literals = staticStringLiteralSpans(production);
  }
  const literals = prepared.literals;
  for (let start = 0; start < literals.length; start += 1) {
    let compact = "";
    for (let end = start; end < literals.length; end += 1) {
      if (
        end > start
        && !literalJoinGapAllowsCompactScan(
          production.slice(literals[end - 1].endOffset, literals[end].startOffset),
        )
      ) {
        break;
      }
      compact += compactProductionSource(literals[end].literal);
      const containsMarker = allowSurroundingText
        ? compact.includes(markerLower)
        : compact === markerLower;
      if (end - start + 1 >= minimumLiteralCount && containsMarker) {
        hits.push(
          compactPatternHit(prepared.filePath, literals[start].line, literals[end].line, marker),
        );
        break;
      }
      if (!allowSurroundingText && compact.length >= markerLower.length) {
        break;
      }
    }
  }
  return hits;
}

function staticStringLiteralSpans(text) {
  const literals = [];
  const lineStarts = [0];
  for (let index = 0; index < text.length; index += 1) {
    if (text[index] === "\n") {
      lineStarts.push(index + 1);
    }
  }

  const stringLiteral = /(?:b?r#*"[^"]*"#*|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|`(?:\\[\s\S]|[^`\\])*`)/g;
  let match;
  while ((match = stringLiteral.exec(text)) != null) {
    literals.push({
      literal: match[0],
      startOffset: match.index,
      endOffset: match.index + match[0].length,
      line: lineNumberAtOffset(lineStarts, match.index),
    });
  }
  return literals;
}

function lineNumberAtOffset(lineStarts, offset) {
  let low = 0;
  let high = lineStarts.length - 1;
  while (low <= high) {
    const mid = Math.floor((low + high) / 2);
    if (lineStarts[mid] <= offset) {
      low = mid + 1;
    } else {
      high = mid - 1;
    }
  }
  return high + 1;
}

function literalJoinGapAllowsCompactScan(gap) {
  const withoutContinuations = gap
    .replace(/\\\r?\n/g, "")
    .replace(/`\r?\n/g, "");
  const withoutJoinCalls = withoutContinuations
    .replace(/\.(?:concat|join)\s*\(/g, "")
    .replace(/\bpath\.(?:join|resolve)\s*\(/g, "");
  return /^[\s,+()[\].]*$/.test(withoutJoinCalls);
}

function compactPatternHit(filePath, startLine, endLine, marker) {
  const lineDisplay = startLine === endLine ? startLine : `${startLine}-${endLine}`;
  return (
    `${filePath}:${lineDisplay}: `
    + `compact production source contains split benchmark marker ${marker}`
  );
}

function staticStringLiteralsOnLine(line) {
  const literals = [];
  const stringLiteral = /(?:b?r#*"[^"]*"#*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)/g;
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

function scanRankerFilenameLiterals(prepared) {
  const lines = prepared.lines;
  const hits = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (rankerFilenameLiteralPattern.test(lines[index])) {
      hits.push(`${prepared.filePath}:${index + 1}:${lines[index]}`);
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

const bannedRegexPatterns = bannedPatterns.map((pattern) => ({
  pattern,
  re: new RegExp(pattern, "i"),
}));
const bannedCombinedRegex = new RegExp(
  bannedPatterns.map((pattern) => `(?:${pattern})`).join("|"),
  "i",
);
const bannedLiteralRegexPatterns = bannedLiteralPatterns.map((pattern) => ({
  pattern,
  re: new RegExp(pattern, "i"),
}));

for (const filePath of [...scanFiles].sort()) {
  const prepared = prepareProductionFile(filePath);
  if (!isEvalOnlyProductionFile(filePath)) {
    const productionHits = scanProductionFile(
      prepared,
      bannedRegexPatterns,
      bannedCombinedRegex,
    );
    for (const { pattern } of bannedRegexPatterns) {
      const hits = productionHits.get(pattern) ?? [];
      if (hits.length > 0) {
        console.error(
          `Banned pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
    for (const { pattern, re } of bannedLiteralRegexPatterns) {
      const hits = scanProductionStringLiterals(prepared, pattern, re);
      if (hits.length > 0) {
        console.error(
          `Banned literal pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
    for (const pattern of bannedCompactPatterns) {
      const hits = scanProductionCompactPatterns(prepared, pattern);
      if (hits.length > 0) {
        console.error(
          `Banned compact benchmark marker /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
  }
  if (filePath.endsWith(`${path.sep}ranker.rs`)) {
    const hits = scanRankerFilenameLiterals(prepared);
    if (hits.length > 0) {
      console.error(
        `Banned filename literals in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
}

const corpusRegexPatterns = evalCorpusBoundaryPatternList.map((pattern) => ({
  pattern,
  re: new RegExp(pattern, "i"),
}));
const corpusCombinedRegex = new RegExp(
  corpusRegexPatterns.map(({ pattern }) => `(?:${pattern})`).join("|"),
  "i",
);
const structuralFiles = new Set();
for (const root of structuralScanDirs) {
  for (const filePath of walkRustProductionFiles(root)) structuralFiles.add(filePath);
}
for (const filePath of [...structuralFiles].sort()) {
  if (isEvalOnlyProductionFile(filePath)) continue;
  const prepared = prepareProductionFile(filePath);
  const hitsByPattern = scanProductionFile(prepared, corpusRegexPatterns, corpusCombinedRegex);
  for (const { pattern } of corpusRegexPatterns) {
    const hits = hitsByPattern.get(pattern) ?? [];
    if (hits.length > 0) {
      console.error(`Production dependency on eval/query corpus /${pattern}/ in ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`);
      failed = true;
    }
  }
  for (const pattern of evalCorpusCompactPatternList) {
    const hits = scanProductionCompactPatterns(prepared, pattern, 1, true);
    if (hits.length > 0) {
      console.error(`Constructed production dependency on eval/query corpus /${pattern}/ in ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`);
      failed = true;
    }
  }
}

const protectedNonRustScanFiles = new Set();
for (const root of nonRustScanRoots) {
  for (const filePath of walkProtectedNonRustFiles(root)) {
    protectedNonRustScanFiles.add(filePath);
  }
}
if (usesDefaultNonRustScanRoots) {
  for (const filePath of requiredProtectedNonRustFiles) {
    protectedNonRustScanFiles.add(filePath);
  }
}
if (protectedNonRustScanFiles.size === 0) {
  console.error("lint-retrieval-generalization: no protected non-Rust files found");
  process.exit(2);
}

for (const filePath of [...protectedNonRustScanFiles].sort()) {
  const prepared = prepareNonRustFile(filePath);
  const harnessDependencyHits = scanCorpusHarnessDependencies(prepared);
  if (harnessDependencyHits.length > 0) {
    console.error(
      `Protected non-Rust path depends on an evaluation/proof harness ${path.relative(repoRoot, filePath)}:\n${harnessDependencyHits.join("\n")}\n`,
    );
    failed = true;
  }
  const productionHits = scanProductionFile(
    prepared,
    corpusRegexPatterns,
    corpusCombinedRegex,
  );
  for (const { pattern } of corpusRegexPatterns) {
    const hits = productionHits.get(pattern) ?? [];
    if (hits.length > 0) {
      console.error(
        `Banned eval/query pattern /${pattern}/ in protected non-Rust path ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
  for (const pattern of evalCorpusCompactPatternList) {
    const hits = scanProductionCompactPatterns(prepared, pattern, 1, true);
    if (hits.length > 0) {
      console.error(
        `Constructed eval/query dependency /${pattern}/ in protected non-Rust path ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
  for (const pattern of corpusHarnessCompactPatternList) {
    const hits = scanProductionCompactPatterns(prepared, pattern, 2, true);
    if (hits.length > 0) {
      console.error(
        `Constructed evaluation/proof harness dependency /${pattern}/ in protected non-Rust path ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
}

if (failed) {
  console.error(
    "retrieval generalization lint failed: remove eval/query dependencies from protected product paths",
  );
  process.exit(1);
}

console.log(
  `lint-retrieval-generalization: ok (${scanDirs.length} retrieval dir(s), ${scanFiles.size} retrieval file(s), ${structuralFiles.size} production file(s), ${protectedNonRustScanFiles.size} protected non-Rust file(s), ${bannedPatterns.length} patterns)`,
);
