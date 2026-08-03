import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  deriveProductRepositoryNames,
  parseBenchmarkPromptLiterals,
  pendingClaimProfileProblem,
  pendingInventoryTotalsProblem,
  runRetrievalGeneralizationLint,
} from "../lib/retrieval-generalization-lint.mjs";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

const PRE_DERIVATION_BAN_FLOOR = [
  "payload_config",
  "freelancer",
  "traderotate",
  "vscode",
  "codex-rs",
  "sourcetrail",
  "extHostCommands",
  "extensionService",
  "workbench.ts",
  "codex_exec::run",
  "exec_events",
  "StorageAccess",
  "PersistentStorage",
  "SourceGroupCxxCdb",
  "IndexerJava",
  "data/indexer",
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
  "Source/Core/Session.swift",
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
  "adapters.js",
  "server.c",
  "ae.c",
  "networking.c",
  "core/main.rs",
  "flags/hiargs.rs",
  "haystack.rs",
  "lib/axios.js",
  "lib/core/Axios.js",
  "StringUtils",
  "commons-lang",
  "PreparedRequest",
  "HTTPAdapter",
  "createApplication",
  "app.use",
  "lib/express.js",
  "Jekyll",
  "LogRecord",
  "AbstractProcessingHandler",
  "useSWR",
  "swr",
  "gin.go",
  "RouterGroup.Handle",
  "Engine.addRoute",
  "Engine.handleHTTPRequest",
  "AutoMapper",
  "TypeMapPlanBuilder",
  "RealBufferedSource",
  "RealBufferedSink",
  "DataRequest",
  "SessionDelegate",
  "novalidate",
  "showError",
  "source/animate.css",
  "nvm",
  "install.sh nvm",
  "bash_completion __nvm",
  "--with-holdout-clone",
  "payload_collection",
];

const PRE_DERIVATION_SPLIT_BAN_FLOOR = [
  '"CharSequence", "Utils"',
  '"app", ".use"',
  '"source/animate", ".css"',
];

const CORPUS_NAMES_RULED_OUT_OF_THE_BAN = new Set([
  "CodeStory",
  "codestory",
  "express",
  "fmt",
  "http",
  "requests",
]);

const CURRENT_HOLDOUT_LITERALS = [
  "axios",
  "redis",
  "ripgrep",
  "dispatchRequest",
  "readQueryFromClient",
  "HiArgs",
  "server.c",
  "core/main.rs",
  "haystack.rs",
];

const MANIFEST_MARKERS = [
  "A bug report says response helpers sometimes choose the wrong status, body, or content type when callers use res.send, res.json, or sendFile. Identify the primary files and functions to inspect before editing.",
  "Project::buildIndex directly parses source files instead of building indexing tasks.",
  "/data/indexer/",
  "run_exec_session",
  "createCacheHelper",
];
const IN_PHRASE_MARKER =
  "Application-level registration starts in the sansio app registration method.";

function write(root, relativePath, contents) {
  const destination = path.join(root, relativePath);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, contents);
  return destination;
}

function treeDigest(root) {
  const hash = crypto.createHash("sha256");
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    const entries = fs.readdirSync(current, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const entryPath = path.join(current, entry.name);
      const relativePath = path.relative(root, entryPath).replaceAll(path.sep, "/");
      const type = entry.isDirectory()
        ? "d"
        : entry.isSymbolicLink()
          ? "l"
          : "f";
      hash.update(`${type}:${relativePath}\0`);
      if (entry.isDirectory()) {
        stack.push(entryPath);
      } else if (entry.isSymbolicLink()) {
        hash.update(fs.readlinkSync(entryPath));
      } else if (entry.isFile()) {
        hash.update(fs.readFileSync(entryPath));
      }
    }
  }
  return hash.digest("hex");
}

function identifierWordShapes(token) {
  const lower = token.toLowerCase();
  const upper = token.toUpperCase();
  const capital = `${lower.slice(0, 1).toUpperCase()}${lower.slice(1)}`;
  return [
    ["separator_prefix", `boost_${lower}_paths`],
    ["separator_suffix", `${lower}_command_boost`],
    ["screaming_separator", `${upper}_PATH_BOOST`],
    ["pascal_type", `${capital}Ranker`],
    ["pascal_lead", `${capital}IndexBoost`],
    ["pascal_middle", `BoostFor${capital}Index`],
    ["camel_tail", `boostFor${capital}`],
    ["camel_tail_acronym", `boostFor${upper}`],
    ["acronym_then_word", `${upper}Index`],
    ["digit_suffix", `${lower}2`],
    ["digit_prefix", `rank2${capital}`],
    ["digit_then_lower", `rank2${lower}`],
  ];
}

function isIdentifierText(value) {
  return /^[A-Za-z][A-Za-z0-9]*$/u.test(value);
}

function shapeFixture(index, text) {
  const declaration = /^[A-Z]/u.test(text)
    ? `pub struct ${text};`
    : `pub fn ${text}() -> f32 { 1.0 }`;
  return `pub const PLANTED_${index}: &str = "${text}";\n${declaration}\n`;
}

function taskManifest({ id, name, url, symbol }) {
  return JSON.stringify({
    id,
    version: 1,
    suite: "public-core",
    task_class: "architecture_explanation",
    repo: {
      name,
      url,
      ref: "0".repeat(40),
    },
    prompt: "Explain how the probe repository handles its own requests end to end.",
    expected_files: ["src/probe_gadget.rs"],
    expected_symbols: [
      { name: symbol, path: "src/probe_gadget.rs", kind: "function" },
    ],
    expected_claims: [],
    forbidden_claims: [],
  });
}

function collectCorpusRepositoryNames(root) {
  const names = new Set();
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(entryPath);
      } else if (entry.name.endsWith(".task.json")) {
        const document = JSON.parse(fs.readFileSync(entryPath, "utf8"));
        const tasks = Array.isArray(document.tasks) ? document.tasks : [document];
        for (const task of tasks) {
          const name = task?.repo?.name?.trim();
          if (name) names.add(name);
          const url = task?.repo?.url?.trim();
          const slug = url
            ?.replace(/\/+$/u, "")
            .replace(/\.git$/u, "")
            .split("/")
            .pop();
          if (slug) names.add(slug);
        }
      }
    }
  }
  return [...names].sort();
}

function findingFor(result, relativePath, predicate = () => true) {
  const normalized = relativePath.replaceAll("\\", "/");
  return result.findings.some((finding) =>
    finding.file?.replaceAll("\\", "/").endsWith(normalized) && predicate(finding)
  );
}

function patternCore(pattern) {
  let core = pattern;
  for (const prefix of ["(?:^|[^A-Za-z0-9])", "(?:^|[^A-Za-z0-9_])"]) {
    if (core.startsWith(prefix)) core = core.slice(prefix.length);
  }
  for (const suffix of ["(?![A-Za-z0-9])", "(?![A-Za-z0-9_])"]) {
    if (core.endsWith(suffix)) core = core.slice(0, -suffix.length);
  }
  return core.replaceAll("\\", "").toLowerCase();
}

function normalizedFindingPattern(pattern) {
  return pattern?.replaceAll("\\", "").toLowerCase() ?? null;
}

function banFiredFor(result, relativePath, planted) {
  const lower = planted.toLowerCase();
  return findingFor(
    result,
    relativePath,
    ({ pattern }) => pattern != null && lower.includes(patternCore(pattern)),
  );
}

function rankerFilenameLiteral(line) {
  for (const match of line.matchAll(/(["'`])([^"'`]+)\1/gu)) {
    const token = match[2];
    if (
      /^[A-Za-z0-9]/u.test(token)
      && token.includes(".")
      && /^[a-z0-9._-]+$/u.test(token)
    ) {
      return token;
    }
  }
  return null;
}

function workflowTriggerPaths(workflow, triggerName) {
  const lines = workflow.split(/\r?\n/u);
  const start = lines.findIndex((line) => line.trimEnd() === `  ${triggerName}:`);
  assert.notEqual(start, -1, `workflow has no ${triggerName} trigger`);
  const paths = [];
  let insidePaths = false;
  for (const line of lines.slice(start + 1)) {
    const trimmed = line.trimStart();
    if (trimmed.length === 0 || trimmed.startsWith("#")) continue;
    if (line.length - trimmed.length <= 2) break;
    if (trimmed === "paths:") {
      insidePaths = true;
      continue;
    }
    if (insidePaths) {
      const match = trimmed.match(/^- ['"]?(.*?)['"]?$/u);
      if (match == null) {
        insidePaths = false;
      } else {
        paths.push(match[1]);
      }
    }
  }
  return paths;
}

function triggerCovers(filter, guarded) {
  if (filter === guarded) return true;
  return filter.endsWith("/**")
    && (guarded === filter.slice(0, -3) || guarded.startsWith(filter.slice(0, -2)));
}

test("the full hostile matrix shares one policy load and never writes into the checkout", {
  timeout: 90_000,
}, () => {
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-generalization-"));
  const productionRepositoryRoot = path.join(
    fixtureRoot,
    " in synthetic-repository",
  );
  const rustRoot = path.join(
    productionRepositoryRoot,
    "crates",
    "codestory-runtime",
    "src",
  );
  const retrievalRoot = path.join(
    productionRepositoryRoot,
    "crates",
    "codestory-retrieval",
    "src",
  );
  const extraRustRoot = path.join(fixtureRoot, "extra-rust");
  const nonRustRoot = path.join(fixtureRoot, "non-rust");
  const taskRoot = path.join(fixtureRoot, "tasks");
  fs.mkdirSync(rustRoot, { recursive: true });
  fs.mkdirSync(retrievalRoot, { recursive: true });
  fs.mkdirSync(extraRustRoot);
  fs.mkdirSync(nonRustRoot);
  fs.mkdirSync(taskRoot);
  assert.ok(
    path.relative(repositoryRoot, fixtureRoot).startsWith(".."),
    "hostile fixtures must live outside the checkout",
  );
  // Snapshot bytes, paths, and symlink targets for the whole intentionally
  // dirty worktree. Comparing the same state before and after is independent of
  // HEAD/index cleanliness and also catches a newly planted untracked fixture.
  const checkoutBefore = treeDigest(repositoryRoot);
  for (const fileName of ["search_plan.rs", "search_scoring.rs", "search_terms.rs"]) {
    write(rustRoot, fileName, "pub fn neutral_default_surface() {}\n");
  }
  // The claim-profile registry is a required production *data* file: the `.rs` walk never
  // reaches it, so the lint seeds it by name and refuses to run when it is absent.
  write(
    rustRoot,
    path.join("agent", "data", "claim_profiles.v2.json"),
    '{ "schema_version": 2, "pending_ratchet": 0, "profiles": [] }\n',
  );
  write(retrievalRoot, "lib.rs", "pub fn neutral_retrieval_surface() {}\n");
  write(
    extraRustRoot,
    "extra-default-additive.rs",
    'pub const EXTRA_ROOT_LEAK: &str = "createApplication";\n',
  );

  const rejected = [];
  const allowed = [];
  const reject = (relativePath, contents) => {
    rejected.push(relativePath);
    write(rustRoot, relativePath, contents);
  };
  const allow = (relativePath, contents) => {
    allowed.push(relativePath);
    write(rustRoot, relativePath, contents);
  };

  reject("cfg-after-test.rs", `
#[cfg(test)]
mod tests { const TEST_ONLY: &str = "codex-rs/test/src/lib.rs"; }
pub fn leaked() -> &'static str { "codex-rs/prod/src/lib.rs" }
`);
  reject("fake-cfg.rs", `
// #[cfg(test)]
pub const NOTE: &str = "#[cfg(test)]";
pub const RAW_NOTE: &str = r#"#[cfg(test)]"#;
pub fn leaked() -> &'static str { "codex-rs/prod/src/lib.rs" }
`);
  reject("current-holdouts.rs", `
pub const HOLDOUTS: &[&str] = &[
  "axios", "redis", "ripgrep", "dispatchRequest", "readQueryFromClient",
  "HiArgs", "server.c", "core/main.rs", "haystack.rs",
];
`);
  reject("query-phrase.rs", `
pub const QUERY: &str =
  "project loads settings refreshes source groups computes refresh info and builds an index";
`);
  reject("manifest-derived.rs", `
pub const PROMPT: &str = "A bug report says response helpers sometimes choose the wrong status, body, or content type when callers use res.send, res.json, or sendFile. Identify the primary files and functions to inspect before editing.";
pub const CLAIM: &str = "Project::buildIndex directly parses source files instead of building indexing tasks.";
pub const PATH: &str = "/data/indexer/";
pub const PROBE_A: &str = "run_exec_session";
pub const PROBE_B: &str = "createCacheHelper";
`);
  reject(
    "pattern-containing-in.rs",
    `pub const CLAIM: &str = ${JSON.stringify(IN_PHRASE_MARKER)};\n`,
  );
  reject("corpus-dependencies.rs", `
pub const TASKS: &str = "benchmarks/tasks/holdout-retrieval/axios-request-dispatch.task.json";
pub const QUERIES: &str = "scripts/cross-repo-sourcetrail-queries.mjs";
pub const PROBES: &str = "benchmarks/tasks/eval-probes.json";
`);
  reject("constructed-corpus-dependencies.rs", `
pub const TASKS: &str = concat!("benchmarks", "/tasks", "/eval-probes.json");
pub const PACKETS: &str = concat!("crates/codestory-cli/tests/fixtures/", "packet_search_eval");
pub const QUALITY: &str = concat!("crates/codestory-bench/tests/", "fixtures/agent_quality");
pub const INCLUDED: &str = include_str!(concat!("../../benchmarks/", "tasks/eval-probes.json"));
`);
  reject("nested-manifest-claim.rs", `
pub fn leaked() -> &'static str {
  "The top-level request helper opens a Session and delegates to Session.request."
}
`);
  allow("nested-manifest-test-only.rs", `
#[cfg(test)]
mod tests {
  const CLAIM: &str =
    "The top-level request helper opens a Session and delegates to Session.request.";
}
pub fn neutral() -> &'static str { "generic role coverage stays neutral" }
`);
  const splitConstructionFiles = new Map([
    ["split-family/use-s-wr.rs", {
      source: 'pub fn leaked() -> String { ["use", "s", "wr"].concat() }\n',
      marker: "useswr",
    }],
    ["split-family/string-utils.rs", {
      source: 'pub fn leaked() -> String { ["string", "utils"].concat() }\n',
      marker: "stringutils",
    }],
    ["split-family/charsequence-utils.rs", {
      source: 'pub fn leaked() -> String { ["charsequence", "utils"].concat() }\n',
      marker: "charsequenceutils",
    }],
    ["split-family/source-animate-css.rs", {
      source: 'pub fn leaked() -> String { ["source/animate", ".css"].concat() }\n',
      marker: "sourceanimatecss",
    }],
    ["split-family/multiline-swr.rs", {
      source: 'pub fn leaked() -> String {\n  [\n    "s",\n    "wr",\n  ].concat()\n}\n',
      marker: "swr",
    }],
    ["split-family/auto-mapper.rs", {
      source: 'pub fn leaked() -> String {\n  [\n    "auto",\n    "mapper",\n  ].concat()\n}\n',
      marker: "automapper",
    }],
    ["split-family/raw-swr.rs", {
      source: 'pub fn leaked() -> String {\n  [\n    r#"s"#,\n    r#"wr"#,\n  ].concat()\n}\n',
      marker: "swr",
    }],
    ["split-family/raw-string-utils.rs", {
      source: 'pub fn leaked() -> String {\n  [\n    r#"string"#,\n    r#"utils"#,\n  ].concat()\n}\n',
      marker: "stringutils",
    }],
  ]);
  for (const [relativePath, { source }] of splitConstructionFiles) {
    reject(relativePath, source);
  }
  allow("cfg-forms.rs", `
#[doc = "codex-rs/test-only"]
#[cfg(test)]
mod tests { const TEST_ONLY: &str = "codex-rs/test/src/lib.rs"; }
#[cfg_attr(test, doc = "codex-rs/test-only")]
pub fn production() -> &'static str { "workspace/app/src/lib.rs" }
#[cfg(not(not(test)))]
mod equivalent { const TEST_ONLY: &str = "codex-rs/test/src/lib.rs"; }
`);
  for (const name of ["test_support.rs", "eval_probes.rs", "diagnostics.rs", "fixtures.rs"]) {
    reject(`testlike/${name}`, 'pub const LEAK: &str = "createApplication";\n');
  }
  reject("paths/unix.rs", 'pub const PATH: &str = "data/indexer";\n');
  reject("paths/windows.rs", 'pub const PATH: &str = "data\\\\indexer";\n');
  for (const symbol of ["SourceGroup", "BuildIndex", "IndexerCommand", "EventProcessor"]) {
    reject(`injection-${symbol}.rs`, `pub const LEAK: &str = "${symbol}";\n`);
  }
  allow("product-vocabulary.rs", `
use serde::Serialize;
#[derive(Serialize)]
pub struct SubcommandStorage { pub subcommand: String, pub storage: String }
pub fn serialize_subcommand(value: &SubcommandStorage) -> String {
  serde_json::to_string(value).unwrap_or_default()
}
`);
  allow("hosting-accounts.rs", `
pub fn licence_notice() -> &'static str { "apache square gorilla pallets" }
`);
  allow("lowercase-route-path.rs", `
pub fn workflow_route_path(route: &crate::Route) -> String { route.route.path.clone() }
`);
  reject("qualified-route-path.rs", 'pub const LEAK: &str = "Route.Path";\n');
  const loggerFiles = new Map();
  for (const [index, probe] of ["logger.php", "Logger.php", "LOGGER.PHP"].entries()) {
    const relativePath = `logger-${index}.rs`;
    loggerFiles.set(relativePath, probe);
    reject(
      relativePath,
      `pub fn leaked(path: &str) -> bool { path.ends_with("${probe}") }\n`,
    );
  }
  allow("documented/search_terms.rs", `
// "anchor", "answer", "around", "cite", "cited", and "cites" are product words.
pub fn documented_choice() -> usize { 7 }
`);
  allow("stopwords/search_terms.rs", `
pub const SEARCH_PLAN_STOPWORDS: &[&str] = &[
  "and", "explain", "from", "how", "into", "show", "then", "with",
];
pub const REASON: &str = "natural_language_filler";
`);
  reject("word-table/search_terms.rs", `
pub const PLANTED_SYMBOL_TERMS: &[&str] =
  &["indexer", "service", "storage", "store", "posts", "feed", "auth", "trail"];
`);
  reject("commented-table/search_terms.rs", `
pub const PLANTED_SYMBOL_TERMS: &[&str] = &[
  "indexer", // pending
  "service", // pending
  "storage", // pending
  "store", // pending
  "posts", // pending
  "feed", // pending
  "auth", // pending
  "trail", // pending
];
`);
  reject("url-table/search_terms.rs", `
pub const PLANTED_TERMS: &[(&str, &str)] = &[
  ("https://example.test/a", "indexer"),
  ("https://example.test/b", "service"),
  ("https://example.test/c", "storage"),
  ("https://example.test/d", "store"),
  ("https://example.test/e", "posts"),
  ("https://example.test/f", "feed"),
];
`);
  const frameworkFiles = new Map();
  for (const probe of [
    "payload.config.ts",
    "payload-types.ts",
    "next.config.ts",
    "app.svelte",
    "/src/collections/posts",
    "/exec/src/cli.rs",
  ]) {
    const relativePath = `framework-${frameworkFiles.size}.rs`;
    frameworkFiles.set(relativePath, probe);
    reject(
      relativePath,
      `pub fn leaked() -> &'static str { "${probe}" }\n`,
    );
  }

  reject(
    "deep/new/ranking_scope_probe_generated.rs",
    'pub const LEAK: &str = "createApplication";\n',
  );
  reject(
    "orphan/app/tests.rs",
    [
      'pub const LEAK: &str = "createApplication";',
      'pub const CORPUS: &str = "benchmarks/tasks";',
      "",
    ].join("\n"),
  );
  allow("excluded/app.rs", "pub fn shipped() {}\n#[cfg(test)]\nmod tests;\n");
  allow(
    "excluded/app/tests.rs",
    'pub const TEST_ONLY: &str = "createApplication";\n',
  );
  write(rustRoot, "shipped/app.rs", "pub fn shipped() {}\nmod tests;\n");
  reject(
    "shipped/app/tests.rs",
    'pub const LEAK: &str = "createApplication";\n',
  );

  const floorFiles = new Map();
  PRE_DERIVATION_BAN_FLOOR.forEach((planted, index) => {
    const relativePath = `floor/floor-${index}.rs`;
    floorFiles.set(relativePath, planted);
    reject(
      relativePath,
      `pub fn planted_${index}() -> &'static str { ${JSON.stringify(planted)} }\n`,
    );
  });
  PRE_DERIVATION_SPLIT_BAN_FLOOR.forEach((planted, index) => {
    const relativePath = `floor/joined-${index}.rs`;
    reject(
      relativePath,
      `pub fn joined_${index}() -> [&'static str; 2] { [${planted}] }\n`,
    );
  });

  const identifierFloor = PRE_DERIVATION_BAN_FLOOR.filter(isIdentifierText);
  const identifierShapeFiles = new Map();
  let shapeIndex = 0;
  for (const token of identifierFloor) {
    for (const [shape, text] of identifierWordShapes(token)) {
      const relativePath = `identifier-shapes/shape-${shapeIndex}.rs`;
      identifierShapeFiles.set(relativePath, { token, shape, text });
      reject(relativePath, shapeFixture(shapeIndex, text));
      shapeIndex += 1;
    }
  }
  assert.ok(shapeIndex > 300, `expected >300 identifier shapes, got ${shapeIndex}`);

  const corpusNames = collectCorpusRepositoryNames(
    path.join(repositoryRoot, "benchmarks", "tasks"),
  );
  assert.ok(corpusNames.length > 20, "benchmark corpus should name many repositories");
  const corpusNameFiles = new Map();
  let corpusIndex = 0;
  for (const name of corpusNames) {
    const literalPath = `corpus-names/literal-${corpusIndex}.rs`;
    corpusNameFiles.set(literalPath, { name, text: `${name} cache key` });
    write(
      rustRoot,
      literalPath,
      `pub const PLANTED: &str = "${name} cache key";\n`,
    );
    if (isIdentifierText(name)) {
      for (const [shape, text] of identifierWordShapes(name)) {
        const relativePath = `corpus-names/${shape}-${corpusIndex}.rs`;
        corpusNameFiles.set(relativePath, { name, text });
        write(rustRoot, relativePath, shapeFixture(corpusIndex, text));
      }
    }
    corpusIndex += 1;
  }
  for (const word of [
    "tokio", "Tokio", "TokioRuntime", "useTokio",
    "answerswrongly", "AnswersWrongly", "plugin", "PluginHost",
    "pluginHost", "PLUGIN_HOST", "login", "LoginHandler",
    "origin", "OriginBoost", "ORIGIN_BOOST", "invite",
    "InviteToken", "demux", "DemuxState",
  ]) {
    allow(
      `substrings/product-${allowed.length}.rs`,
      `pub const PRODUCT_WORD: &str = "${word}";\n`,
    );
  }
  for (const [index, name] of corpusNames.filter(isIdentifierText).entries()) {
    allow(
      `substrings/lower-${index}.rs`,
      `pub const INSIDE_ONE_WORD: &str = "zz${name.toLowerCase()}zz";\n`,
    );
    allow(
      `substrings/upper-${index}.rs`,
      `pub const INSIDE_ONE_WORD: &str = "ZZ${name.toUpperCase()}ZZ";\n`,
    );
  }

  const extraTasks = [
    {
      id: "foreign-anchor",
      name: "generalization-probe",
      url: "https://github.com/example/generalization-probe.git",
      symbol: "ForeignGeneralizationAnchorHandler",
      banned: true,
    },
    ...["store", "runtime", "bench", "indexer"].map((name) => ({
      id: `impostor-${name}`,
      name,
      url: `https://github.com/example/${name}.git`,
      symbol: `Foreign${name[0].toUpperCase()}${name.slice(1)}ProbeHandler`,
      banned: true,
    })),
    {
      id: "false-label-axios",
      name: "codestory",
      url: "https://github.com/axios/axios.git",
      symbol: "FalseLabelAxiosProbeHandler",
      banned: true,
    },
    {
      id: "false-label-ripgrep",
      name: "codestory",
      url: "https://github.com/BurntSushi/ripgrep.git",
      symbol: "FalseLabelRipgrepProbeHandler",
      banned: true,
    },
    {
      id: "real-codestory",
      name: "codestory",
      url: "https://github.com/TheGreenCedar/CodeStory.git",
      symbol: "RealCodeStorySelfProbeHandler",
      banned: false,
    },
  ];
  for (const task of extraTasks) {
    write(taskRoot, `${task.id}.task.json`, taskManifest(task));
  }

  const rejectedNonRust = [
    ["leaked.ps1", "$corpus = \"scripts\\cross-repo-\" + `\n  \"sourcetrail-queries.mjs\"\n", ["scriptscrossreposourcetrailqueriesmjs"]],
    ["leaked.sh", "prefix=./scripts\nscript=${prefix#./}/fetch-holdout-repos.mjs\ncorpus=benchmarks/ta\\\nsks/eval-probes.json\n", ["fetch-holdout-repos.mjs", "benchmarks/tasks/eval-probes.json"]],
    ["workflow-command.yml", "run: |2-\n  node scripts/fetch-\\\n  holdout-repos.mjs\n", ["fetch-holdout-repos.mjs"]],
    ["surrounding-command.mjs", "const command = \"node scripts/fetch-\" + \"holdout-repos.mjs --json\";\nconst config = \"prefix benchmarks/ta\" + \"sks/eval-probes.json suffix\";\n", ["fetchholdoutreposmjs", "benchmarkstasksevalprobesjson"]],
    ["line-continuation.mjs", "const script = \"scripts/fetch-holdout-\\\nrepos.mjs\";\n", ["scripts/fetch-holdout-repos.mjs"]],
    ["joined-shell-word.sh", "node scripts/fetch-'holdout-repos.mjs'\n", ["fetch-holdout-repos.mjs"]],
    ["joined-workflow-word.yml", "run: |\n  node scripts/fetch-'holdout-repos.mjs'\n", ["fetch-holdout-repos.mjs"]],
    ["quoted-run-key.yml", "steps:\n  - \"run\": |\n      node scripts/fetch-'holdout-repos.mjs'\n", ["fetch-holdout-repos.mjs"]],
    ["escaped-shell-word.sh", "node scripts/fetch\\-holdout-repos.mjs\n", ["fetch-holdout-repos.mjs"]],
    ["quoted-yaml-scalar.yml", "value: 'clean # scripts/fetch-holdout-repos.mjs'\n", ["fetch-holdout-repos.mjs"]],
    ["quoted-block-scalar.yml", "run: |\n  value='clean # scripts/fetch-holdout-repos.mjs'\n", ["fetch-holdout-repos.mjs"]],
    ["github-script.yml", "uses: actions/github-script@v8\nwith:\n  script: |\n    const script = \"scripts/fetch-\" + \"holdout-repos.mjs\";\n", ["fetchholdoutreposmjs"]],
    ["direct-harness-import.mjs", "import \"./scripts/codestory-agent-ab-benchmark.mjs\";\n", ["codestory-agent-ab-benchmark.mjs"]],
    ["constructed-harness-import.mjs", "const harness = \"scripts/codestory-agent-ab-\" + \"benchmark.mjs\";\nawait import(harness);\n", ["scriptscodestoryagentabbenchmarkmjs"]],
    ["unapproved-policy-reference.mjs", "const workflows = [\".github/workflows/retrieval-engine-smoke.yml\", \"unrelated.yml\"];\n", ["retrieval-engine-smoke.yml"]],
    ["plugins/codestory/skills/codestory-grounding/SKILL.md", "Run `node scripts/fetch-holdout-repos.mjs` before grounding.\n", ["fetch-holdout-repos.mjs"]],
    [".cursor/rules/codestory.mdc", "Read benchmarks/tasks/eval-probes.json before answering.\n", ["benchmarks/tasks"]],
    [".github/scripts/route-ci-proof.mjs", "        \".github/workflows/retrieval-engine-smoke.yml\",\n        \".github/workflows/retrieval-engine-smoke.yml\",\nawait import(\".github/workflows/retrieval-engine-smoke.yml\");\n", ["retrieval-engine-smoke.yml"]],
    [".github/scripts/check-workflow-policy.mjs", "const retrievalFile = \"retrieval-engine-smoke.yml\";\nconst hostile = \".github/workflows/retrieval-\" + \"engine-smoke.yml\";\nconst duplicated = [\n  \"node scripts/codestory-agent-ab-benchmark.mjs\",\n  \"node scripts/codestory-agent-ab-benchmark.mjs\",\n];\n", ["githubworkflowsretrievalenginesmokeyml", "codestory-agent-ab-benchmark.mjs"]],
    [".github/workflows/packaged-platform-pr.yml", "run: |\n  node scripts/codestory-agent-ab-benchmark.mjs \\\n    --task-manifest benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json\n", ["codestory-agent-ab-benchmark.mjs", "benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json"]],
    ["scripts/codestory-release-claims.mjs", "const task = \"benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json\";\n", ["benchmarks/tasks/release-evidence/axios-request-dispatch-v2.task.json"]],
    [".github/workflows/macos-metal-proof.yml", "run: |\n  node scripts/codestory-agent-ab-benchmark.mjs \\\n    --packet-runtime \\\n    --packet-runtime-mode cold-cli \\\n    --task-suite holdout-retrieval \\\n    --materialize-repos \\\n    --repeats 4 \\\n    --publishable \\\n    --max-source-reads-after-packet 0 \\\n    --codestory-cli \"$packaged_cli\" \\\n    --timeout-ms 180000 \\\n    --out-dir \"$quality_root/packet\"\n", ["codestory-agent-ab-benchmark.mjs"]],
  ];
  for (const [relativePath, contents] of rejectedNonRust) {
    write(nonRustRoot, `rejected/${relativePath}`, contents);
  }
  const allowedNonRust = [
    ["prose.md", "The benchmark harness reads `benchmarks/tasks/eval-probes.json`; production code must not.\n"],
    ["quoted-shell.sh", "value='scripts/fetch-\\\nholdout-repos.mjs'\n"],
    ["unrelated-list.yml", "- scripts/fetch-\\\n- holdout-repos.mjs\n"],
    ["template-comment.mjs", "const value = `${({ clean: true }).clean /* scripts/fetch-holdout-repos.mjs */}`;\n"],
    ["quoted-shell-comment.sh", "value='clean\\' # scripts/fetch-holdout-repos.mjs\n"],
    ["quoted-powershell-comment.ps1", "$value = 'clean`' # scripts/fetch-holdout-repos.mjs\n"],
    ["quoted-yaml-comment.yml", "value: 'clean\\' # scripts/fetch-holdout-repos.mjs\n"],
    ["folded-workflow.yml", "run: >-\n  node scripts/fetch-\\\n  holdout-repos.mjs\n"],
    ["comment-only.yml", "# run: node scripts/fetch-\\\n# holdout-repos.mjs\nrun: echo clean\n"],
    ["plain-apostrophe.yml", "message: don't load it # scripts/fetch-holdout-repos.mjs\n"],
    ["punctuated-apostrophe.yml", "message: rock-'n roll # scripts/fetch-holdout-repos.mjs\n"],
    ["doubled-single-quote.yml", "value: 'scripts/fetch-''holdout-repos.mjs'\n"],
    [".github/scripts/route-ci-proof.mjs", "        \".github/workflows/retrieval-engine-smoke.yml\",\n"],
    [".github/scripts/check-workflow-policy.mjs", "const retrievalFile = \"retrieval-engine-smoke.yml\";\nconst exactQualificationReferences = [\n  \"retrieval-engine-smoke.yml\",\n  \"node scripts/codestory-agent-ab-benchmark.mjs\",\n];\n"],
    [".github/scripts/check-workflow-policy.mjs", "export const frozenCandidateQualityWorkflowRef = \"./.github/workflows/frozen-candidate-quality.yml\";\n"],
    [".github/workflows/packaged-platform-pr.yml", "uses: ./.github/workflows/frozen-candidate-quality.yml\n"],
    [".github/workflows/macos-metal-proof.yml", "run: |\n  node scripts/codestory-agent-ab-benchmark.mjs \\\n    --packet-runtime \\\n    --packet-runtime-mode cold-cli \\\n    --task-suite holdout-retrieval \\\n    --materialize-repos \\\n    --repeats 3 \\\n    --publishable \\\n    --max-source-reads-after-packet 0 \\\n    --codestory-cli \"$packaged_cli\" \\\n    --timeout-ms 180000 \\\n    --out-dir \"$quality_root/packet\"\n"],
  ];
  for (const [relativePath, contents] of allowedNonRust) {
    write(nonRustRoot, `allowed/${relativePath}`, contents);
  }
  write(nonRustRoot, "rejected/neutral.rs", "pub fn neutral() {}\n");
  write(nonRustRoot, "allowed/neutral.rs", "pub fn neutral() {}\n");

  const environment = Object.fromEntries(
    Object.entries(process.env).filter(
      ([name]) => !name.startsWith("CODESTORY_RETRIEVAL_GENERALIZATION_"),
    ),
  );
  Object.assign(environment, {
    CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_SCAN_ROOTS: extraRustRoot,
    CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS: taskRoot,
    GITHUB_ACTIONS: "true",
    GITHUB_WORKSPACE: path.join(fixtureRoot, "hostile-github-workspace"),
  });

  try {
    const result = runRetrievalGeneralizationLint({
      repositoryRoot,
      productionRepositoryRoot,
      environment,
      structuralScanRoots: [rustRoot, extraRustRoot],
      defaultNonRustScanRoots: [
        path.join(nonRustRoot, "rejected"),
        path.join(nonRustRoot, "allowed"),
      ],
      validatePendingSurfaceInventory: false,
    });
    assert.equal(
      treeDigest(repositoryRoot),
      checkoutBefore,
      "lint changed the whole checkout tree, including tracked bytes or untracked paths",
    );
    assert.equal(result.exitCode, 1, result.stderr);
    for (const finding of result.findings) {
      assert.ok(finding.file != null, `unattributed lint failure: ${finding.message}`);
      const findingPath = path.resolve(repositoryRoot, finding.file);
      assert.ok(
        findingPath === fixtureRoot
          || findingPath.startsWith(`${fixtureRoot}${path.sep}`),
        `hostile matrix reported a non-synthetic path: ${finding.file}`,
      );
    }
    assert.deepEqual(
      result.scanDirs,
      [rustRoot, retrievalRoot, extraRustRoot],
      "the canonical runtime/retrieval defaults must remain present before the additive extra root",
    );
    assert.ok(
      findingFor(result, "deep/new/ranking_scope_probe_generated.rs"),
      "default runtime-root discovery missed an unlisted nested production file",
    );
    assert.ok(
      banFiredFor(result, "extra-default-additive.rs", "createApplication"),
      "the additive extra scan root was not scanned alongside the production defaults",
    );
    assert.ok(result.stats.rustFiles > 300, "the synthetic hostile matrix became vacuous");

    for (const relativePath of rejected) {
      assert.ok(
        findingFor(result, relativePath),
        `expected a finding for rejected Rust fixture ${relativePath}`,
      );
    }
    for (const relativePath of allowed) {
      const fixtureFindings = result.findings.filter((finding) =>
        finding.file?.replaceAll("\\", "/").endsWith(relativePath)
      );
      assert.ok(
        fixtureFindings.length === 0,
        `allowed Rust fixture ${relativePath} received findings: ${JSON.stringify(fixtureFindings)}`,
      );
    }
    assert.ok(!result.stderr.includes("codex-rs/test/src/lib.rs"));
    assert.ok(result.stderr.includes("codex-rs/prod/src/lib.rs"));
    assert.ok(!result.stderr.includes("crates/codestory-retrieval/src/ranker.rs (production slice)"));

    for (const marker of CURRENT_HOLDOUT_LITERALS) {
      assert.ok(
        banFiredFor(result, "current-holdouts.rs", marker),
        `the current holdout fixture lost its ${marker} ban`,
      );
    }
    for (const marker of MANIFEST_MARKERS) {
      assert.ok(
        banFiredFor(result, "manifest-derived.rs", marker),
        `the manifest-derived fixture lost its ${marker} ban`,
      );
    }
    assert.ok(
      findingFor(
        result,
        "pattern-containing-in.rs",
        ({ pattern }) =>
          normalizedFindingPattern(pattern) === IN_PHRASE_MARKER.toLowerCase(),
      ),
      "structured findings must retain a complete derived pattern containing ` in `",
    );
    assert.ok(
      banFiredFor(
        result,
        "query-phrase.rs",
        "project loads settings refreshes source groups computes refresh info and builds an index",
      ),
      "the cross-repository query phrase lost its attributed ban",
    );
    for (const marker of [
      "benchmarks/tasks",
      "scripts/cross-repo-sourcetrail-queries.mjs",
      "benchmarks/tasks/eval-probes.json",
    ]) {
      assert.ok(
        banFiredFor(result, "corpus-dependencies.rs", marker),
        `the direct corpus boundary lost ${marker}`,
      );
    }
    for (const marker of ["benchmarkstasks", "packetsearcheval", "agentquality"]) {
      assert.ok(
        findingFor(
          result,
          "constructed-corpus-dependencies.rs",
          ({ message }) => message.toLowerCase().includes(marker),
        ),
        `the constructed corpus boundary lost ${marker}`,
      );
    }
    for (const [relativePath, { marker }] of splitConstructionFiles) {
      assert.ok(
        banFiredFor(result, relativePath, marker),
        `the split construction ${relativePath} lost its ${marker} ban`,
      );
    }
    assert.ok(
      banFiredFor(result, "orphan/app/tests.rs", "createApplication"),
      "an orphan tests.rs escaped the holdout-name pass",
    );
    assert.ok(
      findingFor(
        result,
        "orphan/app/tests.rs",
        ({ kind, pattern }) =>
          kind === "Production dependency on eval/query corpus"
          && normalizedFindingPattern(pattern) === "benchmarks/tasks",
      ),
      "an orphan tests.rs escaped the independent corpus-dependency pass",
    );
    assert.ok(
      findingFor(
        result,
        "qualified-route-path.rs",
        ({ pattern }) => pattern === "Route\\.Path",
      ),
      "the qualified member must be attributed to the case-sensitive Route.Path ban",
    );
    for (const [relativePath, probe] of loggerFiles) {
      assert.ok(
        findingFor(
          result,
          relativePath,
          ({ pattern }) => pattern === "Logger\\.php",
        ),
        `the case-folded filename ban itself did not report ${probe}`,
      );
    }
    for (const symbol of ["SourceGroup", "BuildIndex", "IndexerCommand", "EventProcessor"]) {
      assert.ok(
        banFiredFor(result, `injection-${symbol}.rs`, symbol),
        `the audited injection symbol lost its ${symbol} ban`,
      );
    }
    for (const [relativePath, marker] of frameworkFiles) {
      assert.ok(
        banFiredFor(result, relativePath, marker),
        `the framework filename shape lost ${marker}`,
      );
    }
    for (const relativePath of [
      "word-table/search_terms.rs",
      "commented-table/search_terms.rs",
      "url-table/search_terms.rs",
    ]) {
      assert.ok(
        findingFor(
          result,
          relativePath,
          ({ kind }) => kind === "Term vocabulary table",
        ),
        `the vocabulary table detector did not attribute ${relativePath}`,
      );
    }

    for (const [relativePath, planted] of floorFiles) {
      assert.ok(
        banFiredFor(result, relativePath, planted),
        `the pre-derivation floor lost ${planted}`,
      );
    }
    for (const [relativePath, { token, shape, text }] of identifierShapeFiles) {
      assert.ok(
        banFiredFor(result, relativePath, text),
        `the identifier floor lost ${token} as ${shape} (${text})`,
      );
    }
    for (const [relativePath, { name, text }] of corpusNameFiles) {
      assert.equal(
        banFiredFor(result, relativePath, text),
        !CORPUS_NAMES_RULED_OUT_OF_THE_BAN.has(name),
        `corpus-name ruling drifted for ${name} in ${relativePath}`,
      );
    }

    for (const task of extraTasks) {
      const baseDerived = result.baseDerivedPatterns.some((pattern) =>
        pattern.includes(task.symbol)
      );
      const derived = result.derivedPatterns.some((pattern) =>
        pattern.includes(task.symbol)
      );
      assert.equal(
        baseDerived,
        false,
        `extra task symbol ${task.symbol} was already present before the extra root`,
      );
      assert.equal(
        derived,
        task.banned,
        `self-subject derivation drifted for ${task.id}`,
      );
    }

    for (const [relativePath, , expectedPatterns] of rejectedNonRust) {
      const fixtureFindings = result.findings.filter((finding) =>
        finding.file?.replaceAll("\\", "/").endsWith(`rejected/${relativePath}`)
      );
      for (const expectedPattern of expectedPatterns) {
        assert.ok(
          fixtureFindings.some(({ pattern }) =>
            normalizedFindingPattern(pattern) === expectedPattern.toLowerCase()
          ),
          `expected exact pattern ${expectedPattern} for rejected non-Rust fixture ${relativePath}: ${JSON.stringify(fixtureFindings)}`,
        );
      }
    }
    const routeFindings = result.findings.filter((finding) =>
      finding.file?.replaceAll("\\", "/")
        .endsWith("rejected/.github/scripts/route-ci-proof.mjs")
    );
    assert.ok(
      routeFindings.some(({ message }) =>
        message.includes(
          ':3:await import(".github/workflows/retrieval-engine-smoke.yml");',
        )
      ),
      `the exact hostile route line was masked by another finding: ${JSON.stringify(routeFindings)}`,
    );
    const policyFindings = result.findings.filter((finding) =>
      finding.file?.replaceAll("\\", "/")
        .endsWith("rejected/.github/scripts/check-workflow-policy.mjs")
    );
    assert.ok(
      policyFindings.some(({ message }) =>
        message.includes(
          ':2:const hostile = ".github/workflows/retrieval-" + "engine-smoke.yml";',
        )
      ),
      `the exact hostile policy split was masked by another finding: ${JSON.stringify(policyFindings)}`,
    );
    for (const [relativePath] of allowedNonRust) {
      assert.ok(
        !findingFor(result, `allowed/${relativePath}`),
        `allowed non-Rust fixture ${relativePath} received a finding`,
      );
    }

    const guardedGroups = Object.keys(result.guardedPaths).sort();
    assert.deepEqual(guardedGroups, [
      "corpusDirs",
      "corpusFiles",
      "lintFiles",
      "productionDirs",
      "productionFiles",
      "protectedNonRustDirs",
      "protectedNonRustFiles",
    ]);
    const expectedProductionDirs = fs.readdirSync(
      path.join(repositoryRoot, "crates"),
      { withFileTypes: true },
    )
      .filter((entry) => entry.isDirectory() && entry.name !== "codestory-bench")
      .map((entry) => `crates/${entry.name}/src`)
      .filter((relativePath) => fs.existsSync(path.join(repositoryRoot, relativePath)));
    for (const expectedDir of expectedProductionDirs) {
      assert.ok(
        result.guardedPaths.productionDirs.includes(expectedDir),
        `guarded production roots lost ${expectedDir}`,
      );
    }
    const workflow = fs.readFileSync(
      path.join(repositoryRoot, ".github/workflows/retrieval-engine-smoke.yml"),
      "utf8",
    );
    const guarded = Object.values(result.guardedPaths)
      .flat()
      .filter((entry) => !entry.startsWith(".."));
    assert.ok(
      result.guardedPaths.protectedNonRustFiles.includes(
        ".github/workflows/frozen-candidate-quality.yml",
      ),
      "the explicit evaluation-only workflow owner must remain inside the guarded trigger inventory",
    );
    assert.ok(guarded.length >= 40, "guarded-path inventory became vacuous");
    for (const trigger of ["pull_request", "push"]) {
      const filters = workflowTriggerPaths(workflow, trigger);
      const uncovered = guarded.filter(
        (entry) => !filters.some((filter) => triggerCovers(filter, entry)),
      );
      assert.deepEqual(uncovered, [], `${trigger} misses guarded paths`);
    }
  } finally {
    fs.rmSync(fixtureRoot, { recursive: true, force: true });
  }
});

test("crate-name derivation tolerates one outlier and rejects a foreign majority", () => {
  const checkedIn = fs.readdirSync(path.join(repositoryRoot, "crates"), {
    withFileTypes: true,
  })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
  assert.deepEqual(
    [...deriveProductRepositoryNames(checkedIn, ["probe-vendor-shim"])],
    ["codestory"],
  );
  const crowded = Array.from({ length: 12 }, (_, index) => `store-${index}`);
  assert.throws(
    () => deriveProductRepositoryNames(checkedIn, crowded),
    /cannot be derived/u,
  );
});

test("prompt corpus parsing fails closed on one dynamic entry", () => {
  assert.throws(
    () => parseBenchmarkPromptLiterals(`
const PUBLIC_REPOS = {
  alphaprobe: { prompt: "first benchmark prompt remains a static literal for the guard" },
  betaprobe: { prompt: buildPromptAtRuntime() },
};
const ALL_REPOS = { ...PUBLIC_REPOS };
`),
    /discovered 2 prompt properties but parsed 1 literal prompts/u,
  );
});

test("ranker production has no repository filename literals", () => {
  const rankerPath = path.join(
    repositoryRoot,
    "crates/codestory-retrieval/src/ranker.rs",
  );
  const source = fs.readFileSync(rankerPath, "utf8");
  const production = source.split("#[cfg(test)]", 1)[0];
  const finding = production.split(/\r?\n/u)
    .map((line, index) => ({ line: index + 1, token: rankerFilenameLiteral(line) }))
    .find(({ token }) => token != null);
  assert.equal(
    finding,
    undefined,
    `ranker production contains a repository filename literal: ${JSON.stringify(finding)}`,
  );
});

// The pending inventory is checked as data, not against the checkout: the lint
// binary itself is what compares these declarations to the shipped tree, and it
// exits non-zero on any drift. These cases pin the comparison in both directions.
const PENDING_SURFACE_REASON =
  "Recorded because the surface still exists and the burn-down is tracked on its issue.";

function syntheticInventory() {
  return {
    total_markers: 2,
    total_marker_occurrences: 5,
    surfaces: {
      "crates/first/src/lib.rs": {
        issue: "https://github.com/TheGreenCedar/CodeStory/issues/1573",
        reason: PENDING_SURFACE_REASON,
        markers: { alpha: 3 },
      },
      "crates/second/src/lib.rs": {
        issue: "https://github.com/TheGreenCedar/CodeStory/issues/1573",
        reason: PENDING_SURFACE_REASON,
        markers: { beta: 2 },
      },
    },
  };
}

function syntheticBurnDownEntry(profile) {
  return {
    profile,
    issue: "https://github.com/TheGreenCedar/CodeStory/issues/1674",
    evidence:
      `Measured fire triple for ${profile}: fires on the fitted family, fires on a second `
      + "file type with a different claim, and measures zero on the helper.",
  };
}

function syntheticClaimProfileRatchet() {
  return {
    file: "crates/codestory-runtime/src/agent/data/claim_profiles.v2.json",
    declaration: '"status": "pending_migration"',
    count: 3,
    ratchet_ceiling: 5,
    issue: "https://github.com/TheGreenCedar/CodeStory/issues/1573",
    reason: PENDING_SURFACE_REASON,
    burn_down: [
      syntheticBurnDownEntry("example-one"),
      syntheticBurnDownEntry("example-two"),
    ],
  };
}

function syntheticProfileRegistry(pendingCount) {
  return '{ "status": "pending_migration" },\n'.repeat(pendingCount);
}

test("an exact pending inventory reports no totals problem", () => {
  assert.equal(pendingInventoryTotalsProblem(syntheticInventory()), null);
});

test("either declared inventory total drifting fails in both directions", () => {
  for (const field of ["total_markers", "total_marker_occurrences"]) {
    for (const delta of [-1, 1]) {
      const inventory = syntheticInventory();
      inventory[field] += delta;
      assert.match(
        pendingInventoryTotalsProblem(inventory) ?? "",
        new RegExp(`declares ${field} ${inventory[field]} but lists `, "u"),
        `${field} must be exact, not a floor or a ceiling`,
      );
    }
  }
});

test("occurrence drift inside one marker fails even when the marker count holds", () => {
  const inventory = syntheticInventory();
  inventory.surfaces["crates/first/src/lib.rs"].markers.alpha += 1;
  const problem = pendingInventoryTotalsProblem(inventory);
  assert.equal(problem.includes("total_markers "), false);
  assert.match(problem, /declares total_marker_occurrences 5 but lists 6 occurrences/u);
});

test("a matching claim-profile ratchet reports no problem", () => {
  const declared = syntheticClaimProfileRatchet();
  assert.equal(
    pendingClaimProfileProblem(declared, () => syntheticProfileRegistry(declared.count)),
    null,
  );
});

test("claim-profile ratchet drift fails deterministically in both directions", () => {
  const declared = syntheticClaimProfileRatchet();
  for (const observed of [declared.count - 1, declared.count + 1]) {
    assert.match(
      pendingClaimProfileProblem(declared, () => syntheticProfileRegistry(observed)) ?? "",
      new RegExp(`declared ${declared.count} time\\(s\\), tree has ${observed}`, "u"),
    );
  }
  assert.match(
    pendingClaimProfileProblem(declared, () => null),
    /which the tree no longer has/u,
  );
});

test("a malformed claim-profile ratchet fails closed", () => {
  const declared = syntheticClaimProfileRatchet();
  const registry = () => syntheticProfileRegistry(declared.count);
  for (const invalid of [
    undefined,
    null,
    { ...declared, count: -1 },
    { ...declared, count: 1.5 },
    { ...declared, declaration: "" },
    { ...declared, file: "" },
    { ...declared, reason: "too short" },
    { ...declared, issue: "https://example.com/issues/1" },
  ]) {
    assert.match(
      pendingClaimProfileProblem(invalid, registry) ?? "",
      /pending_claim_profiles must declare/u,
    );
  }
});

test("the claim-profile ratchet cannot be declared above its own ceiling", () => {
  const declared = syntheticClaimProfileRatchet();
  const registry = () => syntheticProfileRegistry(declared.count);
  for (const ceiling of [declared.count - 1, undefined, "5", 5.5]) {
    assert.match(
      pendingClaimProfileProblem({ ...declared, ratchet_ceiling: ceiling }, registry) ?? "",
      /must declare ratchet_ceiling as an integer at or above count/u,
    );
  }
});

test("every profile between the ceiling and the count needs a burn-down entry", () => {
  const declared = syntheticClaimProfileRatchet();
  const registry = (count) => () => syntheticProfileRegistry(count);

  // Lowering the count without recording the migration that lowered it fails.
  assert.match(
    pendingClaimProfileProblem({ ...declared, count: 2 }, registry(2)) ?? "",
    /ratchet_ceiling 5 and count 2 but lists 2 burn_down entr\(ies\)/u,
  );
  // So does deleting a recorded migration to make room for a new uncontracted profile.
  assert.match(
    pendingClaimProfileProblem(
      { ...declared, burn_down: [syntheticBurnDownEntry("example-one")] },
      registry(declared.count),
    ) ?? "",
    /ratchet_ceiling 5 and count 3 but lists 1 burn_down entr\(ies\)/u,
  );
  // A ledger that is not a ledger fails before the arithmetic does.
  assert.match(
    pendingClaimProfileProblem({ ...declared, burn_down: undefined }, registry(declared.count))
      ?? "",
    /must declare burn_down as the ledger of migrations/u,
  );
});

test("a burn-down entry without measured evidence and an owning issue fails", () => {
  const declared = syntheticClaimProfileRatchet();
  const registry = () => syntheticProfileRegistry(declared.count);
  const good = syntheticBurnDownEntry("example-one");
  for (
    const broken of [
      { ...good, evidence: "measured" },
      { ...good, evidence: undefined },
      { ...good, profile: "" },
      { ...good, issue: "https://example.com/issues/1" },
      "example-one",
      null,
    ]
  ) {
    assert.match(
      pendingClaimProfileProblem(
        { ...declared, burn_down: [broken, syntheticBurnDownEntry("example-two")] },
        registry,
      ) ?? "",
      /needs the profile it retired, the issue that retired it, and at least \d+ characters of measured evidence/u,
    );
  }

  assert.match(
    pendingClaimProfileProblem(
      {
        ...declared,
        burn_down: [
          syntheticBurnDownEntry("example-one"),
          syntheticBurnDownEntry("example-one"),
        ],
      },
      registry,
    ) ?? "",
    /lists example-one twice; one migration is one entry/u,
  );
});

test("the shipped claim-profile ratchet is the shipped registry document", () => {
  // The synthetic cases above prove the rule; this one proves the rule is pointed at the
  // registry that actually ships, so moving the profiles to data cannot leave the ratchet
  // counting a declaration no production file spells any more.
  const inventory = JSON.parse(
    fs.readFileSync(
      path.join(repositoryRoot, "scripts", "retrieval-generalization-pending.json"),
      "utf8",
    ),
  );
  assert.equal(
    pendingClaimProfileProblem(
      inventory.pending_claim_profiles,
      (file) => fs.readFileSync(path.join(repositoryRoot, file), "utf8"),
    ),
    null,
  );
});
