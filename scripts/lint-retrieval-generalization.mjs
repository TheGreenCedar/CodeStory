#!/usr/bin/env node
/**
 * CI guard: keep production and release-control code independent from checked
 * evaluation/query corpora. Rust production is checked for derived corpus
 * content and structural paths after masking `#[cfg(test)]` items. Inventoried
 * non-Rust product/release surfaces are checked for direct and adjacent/split
 * corpus dependencies. Explicit benchmark/proof harnesses remain outside the
 * protected scan because they must load those corpora.
 */
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
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
  path.join(repoRoot, ".github", "scripts", "test-detect-codestory-release.py"),
  path.join(repoRoot, ".github", "scripts", "check-workflow-policy.test.mjs"),
  path.join(repoRoot, ".github", "workflows", "release-candidate-evidence.yml"),
  path.join(repoRoot, ".github", "workflows", "retrieval-engine-smoke.yml"),
].map((filePath) => path.resolve(filePath)));
const corpusSupportNonRustFiles = new Set([
  path.join(repoRoot, "scripts", "lint-retrieval-generalization.mjs"),
  path.join(repoRoot, ".github", "scripts", "test-detect-codestory-release.py"),
  path.join(repoRoot, ".github", "scripts", "check-workflow-policy.test.mjs"),
].map((filePath) => path.resolve(filePath)));
const allowedHarnessReferences = [
  [
    path.join("plugins", "codestory", "skills", "codestory-grounding", "references", "drill-suite.md"),
    "scripts/score-drill-ledger.mjs",
    "`node scripts/score-drill-ledger.mjs <suite-report.json> <ledger.json> [scored-report.json]`.",
  ],
  [
    path.join(".github", "scripts", "check-workflow-policy.mjs"),
    "retrieval-engine-smoke.yml",
    'const retrievalFile = "retrieval-engine-smoke.yml";',
  ],
  [
    path.join(".github", "scripts", "check-workflow-policy.mjs"),
    ".github/workflows/retrieval-engine-smoke.yml",
    '".github/workflows/retrieval-engine-smoke.yml",',
  ],
  [
    path.join(".github", "scripts", "check-workflow-policy.mjs"),
    ".github/workflows/release-candidate-evidence.yml",
    'const evidenceFile = "release-candidate-evidence.yml";',
  ],
  [
    path.join(".github", "scripts", "check-workflow-policy.mjs"),
    ".github/workflows/release-candidate-evidence.yml",
    'export const releaseEvidenceWorkflowRef = "./.github/workflows/release-candidate-evidence.yml";',
  ],
  [
    path.join(".github", "workflows", "packaged-platform-pr.yml"),
    ".github/workflows/release-candidate-evidence.yml",
    "uses: ./.github/workflows/release-candidate-evidence.yml",
  ],
  [
    path.join(".github", "workflows", "release.yml"),
    ".github/workflows/release-candidate-evidence.yml",
    "uses: ./.github/workflows/release-candidate-evidence.yml",
  ],
  [
    path.join(".github", "workflows", "plugin-static.yml"),
    ".github/workflows/retrieval-engine-smoke.yml",
    "- .github/workflows/retrieval-engine-smoke.yml",
  ],
  [
    path.join(".github", "workflows", "plugin-static.yml"),
    ".github/workflows/retrieval-engine-smoke.yml",
    "- '.github/workflows/retrieval-engine-smoke.yml'",
  ],
  [
    path.join(".github", "scripts", "route-ci-proof.mjs"),
    ".github/workflows/retrieval-engine-smoke.yml",
    "\".github/workflows/retrieval-engine-smoke.yml\",",
  ],
].map(([relativePath, includes, use]) => ({
  relativePath,
  includes,
  use,
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

// The product's own crate vocabulary: a benchmark task that runs against this
// repository names these, and the product has to keep naming itself.
const crateNameTokens = readdirSync(path.join(repoRoot, "crates"), { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name.split(/[^A-Za-z0-9]+/).filter(Boolean));
const productIdentityTokens = new Set(
  crateNameTokens.flat().map((token) => token.toLowerCase()),
);

// This repository's own name, read from the crates rather than written down:
// every crate is `codestory-<layer>`, so the token they all share is what the
// repository is called. Deciding self-subjecthood on any crate-name token
// instead would hand the exclusion to a holdout that happened to be called
// `store`, `runtime`, or `bench`, and silently switch this lint off for it.
const productRepositoryNames = new Set(
  crateNameTokens.length === 0
    ? []
    : crateNameTokens
      .map((tokens) => tokens[0]?.toLowerCase())
      .filter((token, _index, tokens) =>
        token != null && tokens.every((other) => other === token)
      ),
);
if (productRepositoryNames.size === 0) {
  console.error(
    "lint-retrieval-generalization: crate names share no common prefix, so this "
    + "repository's own name cannot be derived",
  );
  process.exit(2);
}

// Words this product already writes in the layers that never see a query. A
// corpus that happens to share one of them -- `fmt`, `serialize`, `subcommand`
// -- stays covered by its other markers, because banning a word the product
// uses on its own terms would only teach people to spell around the ban.
// Reading the vocabulary out of those layers keeps the exclusion derived: it
// cannot be widened by hand to excuse a corpus symbol somebody wants to keep
// writing, and the retrieval and packet surfaces this lint guards are not in it,
// so steering code cannot vouch for its own words.
const productVocabularyRoots = [
  path.join(repoRoot, "crates", "codestory-contracts", "src"),
  path.join(repoRoot, "crates", "codestory-workspace", "src"),
  path.join(repoRoot, "crates", "codestory-store", "src"),
  path.join(repoRoot, "crates", "codestory-indexer", "src"),
  path.join(repoRoot, "crates", "codestory-cli", "src"),
];

// Search-plan term extraction is where holdout symbol injection lived before
// the v0.16.1 audit, and the planner beside it consumes those terms, so both are
// scanned by name even though the search modules around them are not yet under
// the corpus scan.
const requiredProductionOnlyFiles = [
  path.join(repoRoot, "crates", "codestory-runtime", "src", "search_plan.rs"),
  path.join(repoRoot, "crates", "codestory-runtime", "src", "search_scoring.rs"),
  path.join(repoRoot, "crates", "codestory-runtime", "src", "search_terms.rs"),
];

// Term extraction decides which words become queries, so a word table there is
// the injection this lint exists to catch. The language-level stopword list is
// the one table that cannot encode a repository: it names question filler.
const vocabularyTableFileNames = new Set(["search_terms.rs"]);
const vocabularyTableExemptConstants = ["SEARCH_PLAN_STOPWORDS"];
const vocabularyTableWindowLines = 40;
const vocabularyTableLiteralFloor = 5;

// File names that name a build convention rather than a repository. A corpus
// path ending in one of these contributes its path windows and nothing else.
const buildLayoutFileStems = new Set([
  "index", "init", "lib", "main", "mod", "setup", "app",
]);

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
// Additive only: an extra root can add a task's markers, never drop the checked-in
// corpus. A test proving that a new task extends the ban therefore never writes
// into the corpus every other run reads.
const benchmarkTaskRoots = [
  benchmarkTaskRoot,
  ...(process.env.CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS ?? "")
    .split(path.delimiter)
    .filter((root) => root && existsSync(root)),
];
const pendingSurfacePath = path.join(repoRoot, "scripts", "retrieval-generalization-pending.json");
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

const productVocabulary = readProductVocabulary();

// Every banned corpus token is read out of the checked-in benchmark surfaces,
// so adding a task manifest extends the ban with that task's repository,
// symbols, and files instead of waiting for someone to remember this file.
const benchmarkCorpusMarkerSet = benchmarkCorpusMarkers();

// Bans that derivation cannot reach, kept as literals on purpose and named one
// by one. Everything else in this lint is read out of the checked-in benchmark
// surfaces; these eight are what is left when that reading stops, and each says
// why. Deleting an entry lowers the ban set, so
// `scripts/tests/lint-retrieval-generalization.test.mjs` pins every one of them
// and also fails if an entry becomes derivable and should therefore go.
const residualBannedLiterals = [
  {
    pattern: "freelancer",
    reason:
      "Retired sibling holdout app. benchmarks/tasks/README.md names it as removed, and no manifest, prompt, or query catalog mentions it, so there is nothing left to derive it from.",
  },
  {
    pattern: "traderotate",
    reason:
      "Retired sibling holdout app, in the same README sentence as `freelancer` and in no other corpus surface.",
  },
  {
    pattern: "chinook",
    reason:
      "Bare nickname of the `lerocha/chinook-database` holdout. Deriving it means splitting repository names into parts, which also yields `animate` (from `animate.css`) and `commons` (from `commons-lang`) - ordinary words this lint must not ban.",
  },
  {
    pattern: "mdn",
    reason:
      "Owner segment and nickname of `mdn/learning-area`. Owner segments are deliberately not corpus identity, because banning one fails an Apache licence header rather than a corpus dependency.",
  },
  {
    pattern: "ElsewhereFeed",
    reason:
      "Symbol of a retired holdout app. No manifest carries it any more, so only a literal keeps it out of production.",
  },
  {
    pattern: "HiArgs",
    reason:
      "Written only into ripgrep claim prose, never into an `expected_symbols` entry. Prose identifiers are derived from camelCase and snake_case words alone, because PascalCase prose words are as often proper nouns - `SQLite` is this product's own storage engine, `JavaScript` is a language, `URLSession` and `ValidityState` are platform APIs.",
  },
  {
    pattern: "server\\.c",
    reason:
      "Corpus file name whose stem is a word this product writes on its own account, so `corpusFileNameMarkers` declines it. Its sibling `ae.c` is derived, because `ae` is nobody's vocabulary.",
  },
  {
    pattern: "--with-holdout-clone",
    reason:
      "Flag of a benchmark clone harness that no longer exists in this tree. Nothing derives a flag that has no declaring script left.",
  },
];

// Corpora overlap by design, so the same marker arrives from several of them;
// one pattern per marker keeps the report readable.
const bannedPatterns = [...new Set([
  ...residualBannedLiterals.map(({ pattern }) => pattern),
  ...evalCorpusBoundaryPatternList,
  ...benchmarkManifestDerivedPatterns(),
  ...benchmarkEvalProbeDerivedPatterns(),
  ...benchmarkScriptPromptDerivedPatterns(),
  ...benchmarkQueryCatalogDerivedPatterns(),
  ...benchmarkIdentityDerivedPatterns(),
])];

const bannedLiteralPatterns = [
  "payload_collection",
];

const bannedCompactPatterns = [...new Set([
  ...benchmarkCorpusCompactPatterns(),
  ...evalCorpusCompactPatternList,
])];

// Derivation-only view of the ban set, for the tests that have to tell a derived
// ban from a residual literal. It writes to the file the caller names, so the
// tests read the same construction the scan below uses and there is no second
// code path to drift. A file rather than stdout because the dump outgrew what a
// synchronous exit can flush through a pipe.
const dumpPatternsPath = process.env.CODESTORY_RETRIEVAL_GENERALIZATION_DUMP_PATTERNS;
if (dumpPatternsPath) {
  const residual = new Set(residualBannedLiterals.map(({ pattern }) => pattern));
  writeFileSync(dumpPatternsPath, JSON.stringify({
    residual: residualBannedLiterals,
    derived: bannedPatterns.filter((pattern) => !residual.has(pattern)),
    compact: bannedCompactPatterns,
  }));
  process.exit(0);
}

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
  return [...benchmarkCorpusMarkerSet.descriptive].sort().map(escapeRegExp);
}

// Repository identity is short enough ("swr", "okio", "mdn") that substring
// matching would flag unrelated words, so identity tokens carry their own
// boundaries instead of relying on length.
function benchmarkIdentityDerivedPatterns() {
  return [...benchmarkCorpusMarkerSet.identity]
    .sort()
    .map((token) => `(?:^|[^A-Za-z0-9_])${escapeRegExp(token)}(?![A-Za-z0-9_])`);
}

// Split string literals rejoin into the same marker, so the compact scan needs
// the same corpus vocabulary the line scan uses.
function benchmarkCorpusCompactPatterns() {
  const compact = new Set();
  for (const marker of benchmarkCorpusMarkerSet.descriptive) {
    const normalized = compactProductionSource(marker);
    // `useSWR` rejoins from "use" and "SWR", and `app.use` rejoins from "app"
    // and ".use"; the compact scan needs the short identifiers too, and it
    // compares whole literals so it can afford them.
    const floor = identifierShapedMarker(marker) || memberSymbolMarker(marker) ? 6 : 8;
    if (normalized.length >= floor) {
      compact.add(normalized);
    }
  }
  for (const token of benchmarkCorpusMarkerSet.identity) {
    const normalized = compactProductionSource(token);
    if (normalized.length >= 3) {
      compact.add(normalized);
    }
  }
  return [...compact].sort();
}

function benchmarkCorpusMarkers() {
  const records = [
    ...benchmarkManifestMarkerRecords(),
    ...benchmarkScriptRepoMarkerRecords(),
    ...benchmarkFixtureNameMarkerRecords(),
  ];
  const descriptive = new Set();
  const identity = new Set();
  const coverage = new Map();
  for (const record of records) {
    if (!coverage.has(record.family)) {
      coverage.set(record.family, 0);
    }
    const accepted = record.kind === "identity"
      ? addIdentityMarker(identity, record.marker)
      : addSpecificMarker(descriptive, record.marker, record.options);
    if (accepted) {
      coverage.set(record.family, coverage.get(record.family) + 1);
    }
  }
  if (coverage.size === 0 || descriptive.size === 0 || identity.size === 0) {
    throw new Error("benchmark corpora produced no generalization markers");
  }
  // A family whose every marker was filtered out is invisible to this lint, and
  // a silent gap is worse than a loud one: name it instead of shipping it.
  const uncovered = [...coverage]
    .filter(([, count]) => count === 0)
    .map(([family]) => family)
    .sort();
  if (uncovered.length > 0) {
    throw new Error(
      `benchmark corpus families produced no generalization markers: ${uncovered.join(", ")}`,
    );
  }
  return { descriptive, identity };
}

function benchmarkManifestMarkerRecords() {
  if (!existsSync(benchmarkTaskRoot)) {
    throw new Error(`benchmark task root is missing: ${benchmarkTaskRoot}`);
  }
  const manifestFiles = benchmarkTaskRoots.flatMap((root) =>
    walkFiles(root, (candidate) => candidate.endsWith(".task.json"))
  );
  if (manifestFiles.length === 0) {
    throw new Error(`benchmark task root has no .task.json manifests: ${benchmarkTaskRoot}`);
  }
  const records = [];
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
      const family = benchmarkTaskFamily(task, filePath);
      const push = (marker, options) => records.push({ family, marker, options });
      push(task.id);
      push(task.prompt, { allowExactPhrase: true });
      for (const claim of [...task.expected_claims ?? [], ...task.forbidden_claims ?? []]) {
        push(claim?.text, { allowExactPhrase: true });
      }
      for (const marker of repoIdentityMarkers(task.repo)) {
        records.push({ family, ...marker });
      }
      // A task on this repository lists the product's own files and symbols;
      // banning those would ban the product from naming itself. Its prompt and
      // claims still may not appear in production as whole phrases, but the
      // identifiers inside them are ours, so they stop here.
      if (benchmarkRepoIsProduct(task.repo)) {
        continue;
      }
      // A claim is the only place some corpus symbols are written down:
      // `HiArgs`, `aeProcessEvents`, and `search_parallel` appear in prose and
      // in no `expected_symbols` entry. The phrase ban cannot reach them
      // because production would quote the symbol, never the sentence.
      for (const claim of [...task.expected_claims ?? [], ...task.forbidden_claims ?? []]) {
        for (const identifier of proseIdentifierMarkers(claim?.text)) {
          push(identifier, { allowIdentifier: true });
        }
      }
      const expectedFiles = [
        ...task.expected_files ?? [],
        ...task.expected_verification_files ?? [],
      ];
      for (const expectedFile of expectedFiles) {
        for (const variant of pathMarkerVariants(expectedFile)) {
          push(variant, { allowSpecificComposite: true });
        }
        for (const basename of corpusFileNameMarkers(expectedFile)) {
          records.push({ family, kind: "identity", marker: basename });
        }
      }
      for (const symbol of task.expected_symbols ?? []) {
        const name = typeof symbol === "string" ? symbol : symbol?.name;
        for (const variant of symbolMarkerVariants(name)) {
          push(variant, { allowIdentifier: true });
        }
        if (memberSymbolMarker(name)) {
          push(name.trim(), { allowMemberSymbol: true });
        }
        if (typeof symbol !== "string") {
          push(symbol?.qualified_name, { allowSpecificComposite: true });
          for (const variant of pathMarkerVariants(symbol?.path)) {
            push(variant, { allowSpecificComposite: true });
          }
          for (const basename of corpusFileNameMarkers(symbol?.path)) {
            records.push({ family, kind: "identity", marker: basename });
          }
        }
      }
    }
  }
  if (parsedTaskCount === 0) {
    throw new Error("benchmark manifests produced no tasks");
  }
  return records;
}

// The A/B harness carries repositories that have no manifest of their own; they
// are corpus identity all the same.
function benchmarkScriptRepoMarkerRecords() {
  const records = [];
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
    const repoEntries = [...corpusSource.matchAll(/^ {2}([A-Za-z][A-Za-z0-9_-]*):\s*\{/gm)];
    if (repoEntries.length === 0) {
      throw new Error(`benchmark prompt script declared no repositories: ${filePath}`);
    }
    for (const [index, entry] of repoEntries.entries()) {
      const family = entry[1];
      const entryEnd = index + 1 < repoEntries.length
        ? repoEntries[index + 1].index
        : corpusSource.length;
      const entrySource = corpusSource.slice(entry.index, entryEnd);
      records.push({ family, kind: "identity", marker: family });
      const url = entrySource.match(/\burl\s*:\s*"([^"]*)"/);
      for (const marker of repoIdentityMarkers({ url: url?.[1] })) {
        records.push({ family, ...marker });
      }
    }
  }
  return records;
}

// Fixture file names identify their corpus repository even when the fixture
// carries no manifest fields at all.
function benchmarkFixtureNameMarkerRecords() {
  const fixtures = benchmarkTaskRoots.flatMap((root) =>
    walkFiles(
      root,
      (candidate) => candidate.endsWith(".json") && !candidate.endsWith(".schema.json"),
    )
  );
  if (fixtures.length === 0) {
    throw new Error(`benchmark task root has no corpus fixtures: ${benchmarkTaskRoot}`);
  }
  return fixtures.map((filePath) => ({
    family: "benchmark fixture names",
    marker: path.basename(filePath).replace(/\.[^.]+$/, "").replace(/\.task$/, ""),
    options: { allowSpecificComposite: true },
  }));
}

function benchmarkTaskFamily(task, filePath) {
  const name = typeof task?.repo?.name === "string" ? task.repo.name.trim() : "";
  if (name.length > 0) {
    return name;
  }
  return typeof task?.id === "string" && task.id.trim().length > 0
    ? task.id.trim()
    : path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}

function benchmarkRepoIsProduct(repo) {
  return [repo?.name, ...repoUrlSlugs(repo?.url)]
    .filter((value) => typeof value === "string")
    .some((value) => productRepositoryNames.has(value.split("/").pop().toLowerCase()));
}

// An `owner/repo` slug identifies the corpus whole, but the owner segment alone
// names a hosting account: banning `apache` or `square` on its own would fail an
// Apache licence header rather than a corpus dependency. Only the repository
// segment is corpus identity.
function repoIdentityMarkers(repo) {
  const markers = [];
  const slugs = repoUrlSlugs(repo?.url);
  for (const value of [repo?.name, ...slugs]) {
    if (typeof value !== "string" || value.trim().length === 0) {
      continue;
    }
    if (value.includes("/")) {
      markers.push({ marker: value, options: { allowSpecificComposite: true } });
      markers.push({ kind: "identity", marker: value.split("/").pop() });
      continue;
    }
    markers.push({ kind: "identity", marker: value });
  }
  return markers;
}

// Identifier-shaped words written into corpus prose, restricted to the two
// casings that only ever come from code. A lower-first camelCase word
// (`aeProcessEvents`) or a snake_case word (`search_parallel`) is somebody's
// symbol; PascalCase is not safe to take, because proper nouns wear it too --
// `SQLite` is this product's own storage engine, `JavaScript` is a language,
// and `URLSession` and `ValidityState` are platform APIs. Those stay out and
// are covered, where they must be, by the residual list below.
function proseIdentifierMarkers(text) {
  if (typeof text !== "string") {
    return [];
  }
  return [...new Set(
    text.split(/[^A-Za-z0-9_]+/).filter(codeCasedProseWord),
  )];
}

function codeCasedProseWord(word) {
  if (!/^[a-z][A-Za-z0-9_]*$/.test(word)) {
    return false;
  }
  const snakeCased = /^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/.test(word);
  const camelCased = /^[a-z][a-z0-9]*(?:[A-Z][A-Za-z0-9]*)+$/.test(word);
  return snakeCased || camelCased;
}

// A corpus file is quoted in pieces as often as whole, so windows of the path
// count too. `src/main/java` or `index.js` name a build layout rather than a
// repository, so a window has to carry a segment that could only have come from
// this corpus.
function pathMarkerVariants(filePath) {
  if (typeof filePath !== "string" || filePath.trim().length === 0) {
    return [];
  }
  const segments = filePath.replaceAll("\\", "/").split("/").filter(Boolean);
  if (segments.length === 0) {
    return [];
  }
  const windows = [segments[segments.length - 1]];
  for (let index = 0; index + 1 < segments.length; index += 1) {
    windows.push(`${segments[index]}/${segments[index + 1]}`);
  }
  // A directory such as `form-validation` or `lib_cxx` is quoted on its own as
  // readily as with its neighbours. Two joined words are the bar: `Execution`
  // and `_internal` are words any tree may use.
  windows.push(...segments.filter(pathSegmentNamesTwoWords));
  return [
    segments.join("/"),
    ...windows.filter(pathWindowIsDistinctive),
  ];
}

function pathWindowIsDistinctive(window) {
  if (window.split("/").some(pathSegmentIsDistinctive)) {
    return true;
  }
  // `src/main` and `index.js` are build layout; `data/indexer` and
  // `core/main.rs` are only long enough to be somebody's actual tree.
  return compactProductionSource(window).length >= 10;
}

function pathSegmentNamesTwoWords(segment) {
  return /[_-]/.test(segment)
    && segment.split(/[^A-Za-z0-9]+/).filter((word) => word.length >= 3).length >= 2;
}

function pathSegmentIsDistinctive(segment) {
  if (/[_-]/.test(segment) || /[A-Z]/.test(segment)) {
    return true;
  }
  return segment.replace(/\.[^.]+$/, "").length >= 8;
}

// A corpus file is cited by its own name far more often than by its full path:
// `server.c` and `ae.c` are how anything talks about the redis event loop.
// `pathWindowIsDistinctive` refuses them because they are short, so they get an
// identity marker instead, which carries word boundaries rather than length.
// `main.rs`, `index.js`, and the other build-layout names name a convention
// rather than a repository and stay out.
function corpusFileNameMarkers(filePath) {
  if (typeof filePath !== "string" || filePath.trim().length === 0) {
    return [];
  }
  const segments = filePath.replaceAll("\\", "/").split("/").filter(Boolean);
  const basename = segments[segments.length - 1];
  if (basename == null || !basename.includes(".")) {
    return [];
  }
  const stem = basename.slice(0, basename.lastIndexOf("."));
  if (
    stem.length === 0
    || buildLayoutFileStems.has(stem.toLowerCase())
    || stem.split(/[^A-Za-z0-9]+/).every((word) => productVocabulary.has(word.toLowerCase()))
  ) {
    return [];
  }
  return [basename];
}

// `app.use` is an `expected_symbols` name, and it is the whole symbol: a member
// reached through its receiver. Eight characters is the floor for a descriptive
// marker because a shorter one is usually a word, but `receiver.member` is two
// identifiers joined and cannot be a word at any length.
function memberSymbolMarker(name) {
  return typeof name === "string"
    && /^[A-Za-z][A-Za-z0-9_]*(?:\.|::|#)[A-Za-z][A-Za-z0-9_]*$/.test(name.trim());
}

// A qualified symbol carries its own member name, but only an identifier-shaped
// member is corpus identity: `DataRequest.validate` names one repository,
// `validate` names half the trade.
function symbolMarkerVariants(name) {
  if (typeof name !== "string" || name.trim().length === 0) {
    return [];
  }
  const trimmed = name.trim();
  return [
    trimmed,
    ...trimmed
      .split(/::|[.#]/)
      .map((part) => part.trim())
      .filter((part) => part.length > 0 && identifierShapedMarker(part)),
  ];
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

function repoUrlSlugs(url) {
  if (typeof url !== "string" || url.trim().length === 0) {
    return [];
  }
  const trimmed = url.trim().replace(/\.git$/i, "");
  let pathname = trimmed;
  // A hosted URL owns its owner segment; a local clone path does not, and its
  // parent directory names a checkout layout rather than a repository.
  let hosted = false;
  try {
    const parsed = new URL(trimmed);
    pathname = parsed.pathname;
    hosted = parsed.host.length > 0;
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
  const ownerName = hosted && parts.length >= 2
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
    return false;
  }
  const marker = value.trim();
  if (marker.length < markerLengthFloor(marker, options)) {
    return false;
  }
  if (benchmarkMarkerTooGeneric(marker, options)) {
    return false;
  }
  markers.add(marker);
  return true;
}

// Identifiers such as `useSWR` or `aeMain` are shorter than descriptive markers
// and still name exactly one corpus repository, and `app.use` is two of them
// joined through a receiver.
function markerLengthFloor(marker, options) {
  if (options.allowIdentifier && identifierShapedMarker(marker)) {
    return 5;
  }
  if (options.allowMemberSymbol && memberSymbolMarker(marker)) {
    return 5;
  }
  return 8;
}

// Multi-word identifiers only: `HTTPAdapter` and `use_swr` name something,
// `Session` and `validate` are vocabulary every repository shares.
function identifierShapedMarker(marker) {
  return /^[A-Za-z][A-Za-z0-9_]*$/.test(marker)
    && (marker.includes("_") || /[a-z][A-Z]/.test(marker) || /[A-Z][A-Z][a-z]/.test(marker));
}

function addIdentityMarker(markers, value) {
  if (typeof value !== "string") {
    return false;
  }
  const token = value.trim().toLowerCase();
  if (
    token.length < 3
    || !/^[a-z0-9][a-z0-9._-]*$/.test(token)
    || productIdentityTokens.has(token)
    || productVocabulary.has(token)
  ) {
    return false;
  }
  markers.add(token);
  return true;
}

function benchmarkMarkerTooGeneric(marker, options = {}) {
  if (options.allowExactPhrase && marker.split(/\s+/).length >= 5) {
    return false;
  }
  if (options.allowMemberSymbol && memberSymbolMarker(marker)) {
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
    normalized.length < markerLengthFloor(marker, options)
    || markerIsProductVocabulary(marker)
  );
}

// One plain word that this product already writes elsewhere is vocabulary, not
// identity: `serialize` and `subcommand` belong to the trade. A marker that
// joins two words -- `SourceGroup`, `event_processor`, `FOREIGN KEY`,
// `buildIndex` -- was somebody's symbol before it was anybody's vocabulary, and
// a single word the product never uses -- `novalidate`, `exthostcommands` --
// only ever arrived from a corpus. Neither is excused.
function markerIsProductVocabulary(marker) {
  const trimmed = marker.trim();
  return (
    /^[A-Za-z][a-z0-9]*$/.test(trimmed)
    && productVocabulary.has(trimmed.toLowerCase())
  );
}

// Identifiers only. A word the product merely mentions -- in prose ("mirrors
// the Sourcetrail-style database format") or in a string it prints ("Restart the
// Codex host") -- is not a word the product's code is built from, and excusing
// corpus names on the strength of a comment or a message would let either one
// unlock a ban.
function readProductVocabulary() {
  const words = new Set();
  for (const root of productVocabularyRoots) {
    if (!existsSync(root)) {
      throw new Error(`product vocabulary root is missing: ${root}`);
    }
    const files = walkFiles(
      root,
      (candidate) => candidate.endsWith(".rs") && !isExcludedRustFile(candidate),
    );
    for (const filePath of files) {
      const code = rustIdentifierSource(productionSource(filePath));
      for (const token of code.split(/[^A-Za-z0-9]+/)) {
        if (token.length >= 3) {
          words.add(token.toLowerCase());
        }
      }
    }
  }
  if (words.size === 0) {
    throw new Error("product vocabulary roots produced no words");
  }
  return words;
}

function rustIdentifierSource(text) {
  let code = "";
  let index = 0;
  while (index < text.length) {
    const character = text[index];
    if (character === "/" && text[index + 1] === "/") {
      index = endOfLine(text, index);
      code += "\n";
      continue;
    }
    if (character === "/" && text[index + 1] === "*") {
      index = rustBlockCommentEnd(text, index + 2);
      code += " ";
      continue;
    }
    if (character === "r" || character === "b") {
      const rawEnd = rustRawStringEnd(text, index);
      if (rawEnd !== null) {
        index = rawEnd;
        code += " ";
        continue;
      }
    }
    if (character === '"') {
      index = rustStringEnd(text, index + 1, '"');
      code += " ";
      continue;
    }
    code += character;
    index += 1;
  }
  return code;
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
    extension,
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
  if (extension === ".yaml" || extension === ".yml") {
    return maskYamlComments(source);
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
  const doubledQuoteEscape = extension === ".ps1";
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

function maskYamlComments(source) {
  const masked = source.split("");
  const blockScalarKinds = yamlBlockScalarKinds(source.split(/\r?\n/));
  let quote = null;
  let quoteInBlockScalar = false;
  let line = 0;

  for (let index = 0; index < source.length; index += 1) {
    const current = source[index];
    if (index > 0 && source[index - 1] === "\n") line += 1;
    if (quote != null) {
      if (
        !quoteInBlockScalar
        && quote === "'"
        && current === "'"
        && source[index + 1] === "'"
      ) index += 1;
      else if (quote === '"' && current === "\\") index += 1;
      else if (current === quote) {
        quote = null;
        quoteInBlockScalar = false;
      }
      continue;
    }
    quoteInBlockScalar = blockScalarKinds[line] != null;
    if (
      (current === "'" || current === '"')
      && (quoteInBlockScalar || yamlQuoteStartsScalar(source, index))
    ) {
      quote = current;
      continue;
    }
    quoteInBlockScalar = false;
    if (
      current !== "#"
      || (index > 0 && source[index - 1] !== "\n" && !/[\t ]/.test(source[index - 1]))
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

function yamlQuoteStartsScalar(source, index) {
  const lineStart = source.lastIndexOf("\n", index - 1) + 1;
  const before = source.slice(lineStart, index);
  const token = before.trimEnd();
  const delimiter = token.at(-1);
  if (delimiter == null || ":[{,?".includes(delimiter)) return true;
  return delimiter === "-" && /(?:^|[\t [{,])-$/.test(token);
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
    const yamlRunCommand = yamlRunCommandText(lines[start], extension);
    logicalLines.push({
      text: yamlRunCommand ?? text,
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
      /^([^#\n]+):\s*([|>])(?:[1-9][+-]?|[+-][1-9]?)?(?:\s+#.*)?\s*$/,
    );
    if (header != null) {
      blockIndent = indent;
      const key = header[1].trim().replace(/^-\s*/, "");
      const shellPrefix = ["run", "'run'", '"run"'].includes(key) ? "run" : "";
      scalarKind = `${shellPrefix}${header[2]}`;
    }
  }
  return scalarKinds;
}

function isYamlRunLine(line, extension) {
  return (extension === ".yaml" || extension === ".yml")
    && yamlRunLineMatch(line) != null;
}

function yamlRunLineMatch(line) {
  return line.match(/^\s*(?:-\s*)?(?:run|'run'|"run")\s*:\s*(.*)$/);
}

function yamlRunCommandText(line, extension) {
  if (extension !== ".yaml" && extension !== ".yml") return null;
  const match = yamlRunLineMatch(line);
  if (match == null) return null;
  const scalar = match[1].trim();
  if (/^[|>](?:[1-9][+-]?|[+-][1-9]?)?$/.test(scalar)) return null;
  if (scalar.startsWith("'") && scalar.endsWith("'")) return scalar.slice(1, -1).replaceAll("''", "'");
  if (scalar.startsWith('"') && scalar.endsWith('"')) return scalar.slice(1, -1).replace(/\\(["\\])/g, "$1");
  return scalar;
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
    const normalizedLine = normalizeNativeSeparators(line.text, line.shellLike).trim();
    const hasProtectedDependency = corpusHarnessDependencyRegexes.some((pattern) =>
      regexOccurrences(pattern, normalizedLine).some((occurrence) =>
        !harnessDependencyAllowed(prepared, line, pattern, occurrence)
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

function regexOccurrences(pattern, source) {
  const flags = pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`;
  return [...source.matchAll(new RegExp(pattern.source, flags))].map((match) => ({
    index: match.index,
    length: match[0].length,
  }));
}

function harnessDependencyAllowed(prepared, line, pattern, occurrence) {
  return allowedHarnessReferences.some(({ relativePath, includes, use }) =>
    allowedReferenceFileMatches(prepared.filePath, relativePath)
    && allowedReferenceUseMatches(prepared, use, line.startLine, line.endLine)
    && regexOccurrences(pattern, normalizeNativeSeparators(use)).some((allowed) =>
      allowed.index === occurrence.index && allowed.length === occurrence.length
    )
    && pattern.test(normalizeNativeSeparators(includes))
  );
}

function compactHarnessDependencyAllowed(prepared, literals, marker) {
  const startLine = Math.min(...literals.map(({ line }) => line));
  const endLine = Math.max(...literals.map(({ line }) => line));
  const source = prepared.sourceLines.slice(startLine - 1, endLine).join("\n").trim();
  return allowedHarnessReferences.some(({ relativePath, includes, use }) => {
    const includesCompact = compactProductionSource(normalizeNativeSeparators(includes));
    return allowedReferenceFileMatches(prepared.filePath, relativePath)
      && source === use
      && allowedReferenceUseMatches(prepared, use, startLine, endLine)
      && literals.some(({ literal }) =>
        normalizeNativeSeparators(staticStringLiteralContent(literal)).includes(includes)
      )
      && (marker.includes(includesCompact) || includesCompact.includes(marker));
  });
}

function allowedReferenceFileMatches(filePath, relativePath) {
  const roots = usesDefaultNonRustScanRoots ? [repoRoot] : nonRustScanRoots;
  return roots.some((root) =>
    path.resolve(filePath) === path.resolve(root, relativePath)
  );
}

function allowedReferenceUseMatches(prepared, use, startLine, endLine) {
  const matches = prepared.logicalLines.filter((line) =>
    normalizeNativeSeparators(line.text, line.shellLike).trim() === use
  );
  return matches.length === 1
    && matches[0].startLine === startLine
    && matches[0].endLine === endLine;
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
  const normalized = shellLike ? normalizeShellLikeText(text) : text;
  return normalized.replaceAll("\\", "/");
}

function normalizeShellLikeText(text) {
  let normalized = "";
  let quote = null;
  for (let index = 0; index < text.length; index += 1) {
    const current = text[index];
    if (quote == null && (current === "'" || current === '"')) {
      quote = current;
      continue;
    }
    if (current === quote) {
      quote = null;
      continue;
    }
    const escaped = current === "\\" && index + 1 < text.length
      && (quote == null || (quote === '"' && '$`"\\'.includes(text[index + 1])));
    if (escaped) {
      normalized += text[index + 1];
      index += 1;
    } else {
      normalized += current;
    }
  }
  return normalized;
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
  matchAllowed = () => false,
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
    const segments = [];
    for (let end = start; end < literals.length; end += 1) {
      if (
        end > start
        && !literalJoinGapAllowsCompactScan(
          production.slice(literals[end - 1].endOffset, literals[end].startOffset),
          prepared.extension,
          literals[end - 1].literal,
          literals[end].literal,
        )
      ) {
        break;
      }
      const latestBoundary = compact.length;
      const literalCompact = compactProductionSource(literals[end].literal);
      compact += literalCompact;
      segments.push({
        ...literals[end],
        compactStart: latestBoundary,
        compactEnd: compact.length,
      });
      const occurrences = end - start + 1 >= minimumLiteralCount
        ? compactMarkerOccurrences(
          compact,
          markerLower,
          allowSurroundingText,
          minimumLiteralCount > 1 ? latestBoundary : null,
        )
        : [];
      const rejected = occurrences.some(({ index, length }) => {
        const matchedLiterals = segments.filter((literal) =>
          literal.compactStart < index + length && literal.compactEnd > index
        );
        return !matchAllowed(matchedLiterals, markerLower);
      });
      if (rejected) {
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

function compactMarkerOccurrences(compact, marker, allowSurroundingText, requiredBoundary) {
  if (!allowSurroundingText) {
    return compact === marker
      && (
        requiredBoundary == null
        || (requiredBoundary > 0 && marker.length > requiredBoundary)
      )
      ? [{ index: 0, length: marker.length }]
      : [];
  }
  const occurrences = [];
  for (
    let offset = compact.indexOf(marker);
    offset >= 0;
    offset = compact.indexOf(marker, offset + 1)
  ) {
    if (
      requiredBoundary == null
      || (offset < requiredBoundary && offset + marker.length > requiredBoundary)
    ) {
      occurrences.push({ index: offset, length: marker.length });
    }
  }
  return occurrences;
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

function literalJoinGapAllowsCompactScan(gap, extension, previousLiteral, nextLiteral) {
  if (
    (extension === ".yaml" || extension === ".yml")
    && gap === ""
    && previousLiteral.startsWith("'")
    && nextLiteral.startsWith("'")
  ) {
    return false;
  }
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

// The inventory records the benchmark-family surfaces that already exist, down
// to how many lines each marker occupies. It never grants a file blanket cover:
// one more occurrence of a listed marker fails, and an entry that stops matching
// fails, so both adding to a listed surface and deleting one must edit this
// file.
//
// It is also bounded and attributable. Every surface has to carry a reason and
// the issue that tracks its deletion, and the declared `total_markers` has to
// equal the number of markers actually listed - no slack in either direction.
// Recording one more marker is therefore a reviewable diff that raises a stated
// number and answers "why is this still here", and deleting a surface forces
// that number down.
const pendingSurfaceIssuePattern =
  /^https:\/\/github\.com\/TheGreenCedar\/CodeStory\/issues\/\d+$/;
const pendingSurfaceReasonFloor = 60;

function loadPendingSurfaces() {
  let inventory;
  try {
    inventory = JSON.parse(readFileSync(pendingSurfacePath, "utf8"));
  } catch (error) {
    console.error(`lint-retrieval-generalization: unreadable pending inventory: ${error}`);
    process.exit(2);
  }
  const surfaces = new Map();
  let declaredMarkers = 0;
  for (const [file, entry] of Object.entries(inventory?.surfaces ?? {})) {
    const problem = pendingSurfaceEntryProblem(entry);
    if (problem != null) {
      console.error(`lint-retrieval-generalization: invalid pending entry for ${file}: ${problem}`);
      process.exit(2);
    }
    const counts = Object.entries(entry.markers);
    declaredMarkers += counts.length;
    surfaces.set(path.resolve(repoRoot, file), new Map(counts));
  }
  if (inventory?.total_markers !== declaredMarkers) {
    console.error(
      `lint-retrieval-generalization: pending inventory declares total_markers `
      + `${JSON.stringify(inventory?.total_markers)} but lists ${declaredMarkers}; `
      + `the declared total is the bound on this file and must match it exactly`,
    );
    process.exit(2);
  }
  return surfaces;
}

function pendingSurfaceEntryProblem(entry) {
  if (typeof entry !== "object" || entry == null || Array.isArray(entry)) {
    return "entry must be an object";
  }
  if (typeof entry.reason !== "string" || entry.reason.trim().length < pendingSurfaceReasonFloor) {
    return `reason must be at least ${pendingSurfaceReasonFloor} characters saying why the surface still exists`;
  }
  if (typeof entry.issue !== "string" || !pendingSurfaceIssuePattern.test(entry.issue.trim())) {
    return "issue must be a CodeStory issue URL tracking the surface's deletion";
  }
  const markers = entry.markers;
  if (typeof markers !== "object" || markers == null || Array.isArray(markers)) {
    return "markers must be an object";
  }
  const counts = Object.entries(markers);
  if (counts.length === 0) {
    return "markers must not be empty";
  }
  if (counts.some(([, count]) => !Number.isInteger(count) || count < 1)) {
    return "every marker count must be a positive integer";
  }
  return null;
}

function pendingSurfaceCovers(filePath, marker, hitCount) {
  const markers = pendingSurfaces.get(path.resolve(filePath));
  const recorded = markers?.get(marker);
  if (recorded === undefined) {
    return false;
  }
  observedPendingSurfaces.set(pendingSurfaceKey(path.resolve(filePath), marker), hitCount);
  return hitCount <= recorded;
}

function pendingSurfaceKey(filePath, marker) {
  return `${filePath} :: ${marker}`;
}

function stalePendingSurfaces() {
  // The inventory describes the shipped tree; a caller-supplied scan root has
  // no reason to reach any of it.
  if (!usesDefaultScanRoots) {
    return [];
  }
  const stale = [];
  for (const [filePath, markers] of pendingSurfaces) {
    for (const [marker, recorded] of markers) {
      const observed = observedPendingSurfaces.get(pendingSurfaceKey(filePath, marker));
      if (observed === undefined) {
        stale.push(`${path.relative(repoRoot, filePath)}: ${marker} (recorded ${recorded}, no longer matches)`);
        continue;
      }
      if (observed !== recorded) {
        stale.push(
          `${path.relative(repoRoot, filePath)}: ${marker} (recorded ${recorded}, tree has ${observed})`,
        );
      }
    }
  }
  return stale.sort();
}

// A vocabulary table is the shape the v0.16.1 audit found: a run of bare word
// literals that term extraction compares the question against. Individually
// those words are ordinary -- `storage`, `posts`, `exec cli` -- so no ban list
// can catch them; together, in a file that decides which words become queries,
// they are a repository's domain written into this crate.
function scanVocabularyTable(prepared) {
  if (!vocabularyTableFileNames.has(path.basename(prepared.filePath))) {
    return [];
  }
  const exemptLines = vocabularyTableExemptLines(prepared.lines);
  const entries = [];
  for (const [index, line] of prepared.lines.entries()) {
    if (exemptLines.has(index)) {
      continue;
    }
    for (const literal of staticStringLiteralsOnLine(line)) {
      const content = staticStringLiteralContent(literal);
      if (literalIsBareVocabulary(content)) {
        entries.push({ line: index + 1, content: content.toLowerCase() });
      }
    }
  }
  for (const [index, entry] of entries.entries()) {
    const window = new Set();
    for (const candidate of entries.slice(index)) {
      if (candidate.line - entry.line >= vocabularyTableWindowLines) {
        break;
      }
      window.add(candidate.content);
    }
    if (window.size >= vocabularyTableLiteralFloor) {
      return [
        `${prepared.filePath}:${entry.line}: ${window.size} bare word literals within `
        + `${vocabularyTableWindowLines} lines name a vocabulary, not a question: `
        + `${[...window].sort().join(", ")}`,
      ];
    }
  }
  return [];
}

// Only the declaration is exempt, never a mention of the constant's name: a
// planted table that happens to sit after `if SEARCH_PLAN_STOPWORDS.contains(..)`
// must not inherit the exemption.
function vocabularyTableExemptLines(lines) {
  const exempt = new Set();
  let open = false;
  for (const [index, line] of lines.entries()) {
    if (
      !open
      && /=\s*&\[/.test(line)
      && vocabularyTableExemptConstants.some((name) => line.includes(name))
    ) {
      open = true;
    }
    if (open) {
      exempt.add(index);
      if (line.includes("];")) {
        open = false;
      }
    }
  }
  return exempt;
}

// Words and short phrases only. A snake_case tag, a path, or a sentence is
// something other than vocabulary: reasons, roles, and prose instructions all
// have to keep their literals.
function literalIsBareVocabulary(content) {
  if (typeof content !== "string") {
    return false;
  }
  const words = content.trim().split(/[ -]+/);
  return (
    content.trim().length >= 3
    && words.length <= 3
    && words.every((word) => /^[A-Za-z][A-Za-z0-9]*$/.test(word))
  );
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

const pendingSurfaces = loadPendingSurfaces();
const observedPendingSurfaces = new Map();

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
      if (hits.length > 0 && !pendingSurfaceCovers(filePath, pattern, hits.length)) {
        console.error(
          `Banned pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
    for (const { pattern, re } of bannedLiteralRegexPatterns) {
      const hits = scanProductionStringLiterals(prepared, pattern, re);
      if (hits.length > 0 && !pendingSurfaceCovers(filePath, pattern, hits.length)) {
        console.error(
          `Banned literal pattern /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
        );
        failed = true;
      }
    }
    for (const pattern of bannedCompactPatterns) {
      const hits = scanProductionCompactPatterns(prepared, pattern);
      if (hits.length > 0 && !pendingSurfaceCovers(filePath, pattern, hits.length)) {
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
  const vocabularyTableHits = scanVocabularyTable(prepared);
  if (vocabularyTableHits.length > 0) {
    console.error(
      `Term vocabulary table in ${path.relative(repoRoot, filePath)} (production slice):\n${vocabularyTableHits.join("\n")}\n`,
    );
    failed = true;
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
    const hits = scanProductionCompactPatterns(
      prepared,
      pattern,
      2,
      true,
      (literals, marker) => compactHarnessDependencyAllowed(prepared, literals, marker),
    );
    if (hits.length > 0) {
      console.error(
        `Constructed evaluation/proof harness dependency /${pattern}/ in protected non-Rust path ${path.relative(repoRoot, filePath)}:\n${hits.join("\n")}\n`,
      );
      failed = true;
    }
  }
}

const stalePending = stalePendingSurfaces();
if (stalePending.length > 0) {
  console.error(
    `Pending benchmark-family surfaces no longer match the tree; correct or delete them in ${path.relative(repoRoot, pendingSurfacePath)}:\n${stalePending.join("\n")}\n`,
  );
  failed = true;
}

if (failed) {
  console.error(
    "retrieval generalization lint failed: remove eval/query dependencies from protected product paths",
  );
  process.exit(1);
}

const pendingSurfaceCount = [...pendingSurfaces.values()]
  .reduce((total, markers) => total + markers.size, 0);
console.log(
  `lint-retrieval-generalization: ok (${scanDirs.length} retrieval dir(s), ${scanFiles.size} retrieval file(s), ${structuralFiles.size} production file(s), ${protectedNonRustScanFiles.size} protected non-Rust file(s), ${bannedPatterns.length} patterns, ${pendingSurfaceCount} pending benchmark-family surface(s) in ${pendingSurfaces.size} file(s) awaiting deletion)`,
);
