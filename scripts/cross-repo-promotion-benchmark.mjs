#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { sourcetrailQueries } from "./cross-repo-sourcetrail-queries.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const isWindows = process.platform === "win32";
const defaultCli = path.join(root, "target", "release", isWindows ? "codestory-cli.exe" : "codestory-cli");
const cli = process.env.CODESTORY_CROSS_REPO_BIN ?? process.env.CODESTORY_EMBED_RESEARCH_BIN ?? defaultCli;
const stamp = new Date().toISOString().replace(/[-:TZ.]/g, "").slice(0, 14);
const outDir =
  process.env.CODESTORY_CROSS_REPO_OUT_DIR ??
  path.join(root, "target", "autoresearch", "cross-repo-promotion", stamp);
const listOnly = process.argv.includes("--list") || process.env.CODESTORY_CROSS_REPO_LIST === "1";
const skipIndex = process.argv.includes("--skip-index") || process.env.CODESTORY_CROSS_REPO_SKIP_INDEX === "1";
const allowFail = process.env.CODESTORY_CROSS_REPO_ALLOW_FAIL === "1";
const requiredProjects = Number(process.env.CODESTORY_CROSS_REPO_REQUIRED_PROJECTS ?? 4);
const minAggregateHit10 = Number(process.env.CODESTORY_CROSS_REPO_MIN_HIT10 ?? 0.85);
const minAggregateMrr10 = Number(process.env.CODESTORY_CROSS_REPO_MIN_MRR10 ?? 0.7);
const minProjectHit10 = Number(process.env.CODESTORY_CROSS_REPO_MIN_PROJECT_HIT10 ?? 0.75);
const minAdversarialHit10 = Number(process.env.CODESTORY_CROSS_REPO_MIN_ADVERSARIAL_HIT10 ?? 0.75);
const maxSearchP95Ms = Number(process.env.CODESTORY_CROSS_REPO_MAX_SEARCH_P95_MS ?? 1000);
const extraIndexArgs = splitArgs(process.env.CODESTORY_CROSS_REPO_INDEX_ARGS);
const extraSearchArgs = splitArgs(process.env.CODESTORY_CROSS_REPO_SEARCH_ARGS);
const searchMode = (process.env.CODESTORY_CROSS_REPO_SEARCH_MODE ?? "serve").toLowerCase();
const profileName = readArgValue("--profile") ?? process.env.CODESTORY_CROSS_REPO_PROFILE ?? "inherit";
const defaultQueryBucket = "workflow";
const runEnv = { ...process.env };
const profileConfig = configureProfile(profileName, runEnv);
const selectedProjectIds = new Set(
  splitList(process.env.CODESTORY_CROSS_REPO_PROJECTS).map((item) => item.toLowerCase()),
);

const suites = [
  {
    id: "freelancer",
    name: "freelancer",
    language_mix: "Rust/Tauri + TypeScript/React desktop app",
    pathCandidates: [String.raw`C:\Users\alber\source\repos\freelancer`],
    queries: [
      {
        id: "freelancer-create-lead",
        query: "Tauri command creates a lead from input validates contact fields and persists through the lead repository",
        expect: ["create_lead", "CreateLeadInput", "LeadRepository"],
      },
      {
        id: "freelancer-convert-lead",
        query: "convert a qualified lead into a client while deleting the original lead record",
        expect: ["convert_lead"],
      },
      {
        id: "freelancer-dashboard-stats",
        query: "dashboard stats count leads clients projects invoices outstanding amount and recent business totals",
        expect: ["get_dashboard_stats"],
      },
      {
        id: "freelancer-email-value-object",
        query: "domain value object validates email addresses before storing lead or client contact information",
        expect: ["Email"],
      },
      {
        id: "freelancer-money-value-object",
        query: "money value object stores cents currency and formats invoice payment amounts",
        expect: ["Money"],
      },
      {
        id: "freelancer-database",
        query: "SQLite database opens the app data path and runs migrations for freelancer entities",
        expect: ["Database", "run_migrations"],
      },
      {
        id: "freelancer-event-store",
        query: "event store records domain events as json payloads with aggregate type id and timestamp",
        expect: ["EventStore", "DomainEvent"],
      },
      {
        id: "freelancer-project-repository",
        query: "project repository maps sqlite rows into Project records and supports filtered list pagination",
        expect: ["ProjectRepository", "ProjectFilters"],
      },
      {
        id: "freelancer-lead-repository",
        query: "lead repository creates updates deletes leads and optional lookup from rusqlite rows",
        expect: ["LeadRepository", "OptionalExt"],
      },
      {
        id: "freelancer-app-state",
        query: "Tauri application state wires database connection and command handlers in the run entrypoint",
        expect: ["AppState", "run"],
      },
      {
        id: "freelancer-react-dashboard",
        query: "React dashboard renders stat cards for active leads projects revenue and overdue invoices",
        expect: ["Dashboard", "StatCard"],
      },
      {
        id: "freelancer-time-tracking",
        query: "work item and time entry commands start timer stop timer and list time entries",
        expect: ["start_timer", "stop_timer", "TimeEntry"],
      },
      {
        id: "freelancer-client-from-lead",
        query: "client domain object can be constructed from a lead with retained source lead id and default currency",
        expect: ["Client::from_lead", "Client"],
      },
      {
        id: "freelancer-invoice-line-items",
        query: "invoice adds line items recalculates subtotal tax total and updates invoice status",
        expect: ["Invoice::add_line_item", "Invoice::recalculate_totals", "InvoiceStatus"],
      },
      {
        id: "freelancer-event-latest-version",
        query: "event store reads latest aggregate version and events after a timestamp for replay",
        expect: ["EventStore::get_latest_version", "EventStore::get_events_after"],
      },
      {
        id: "freelancer-transaction-wrapper",
        query: "database helper executes a closure inside a rusqlite transaction and commits on success",
        expect: ["Database::with_transaction"],
      },
      {
        id: "freelancer-communication-logging",
        query: "Tauri command logs a communication with related entity type direction subject and content",
        expect: ["log_communication", "Communication", "CommunicationFilters"],
      },
      {
        id: "freelancer-status-closed-vs-active",
        bucket: "adversarial",
        query: "which status helper decides whether leads are closed while projects use a separate active or closed helper",
        expect: ["LeadStatus::is_closed", "ProjectStatus::is_active", "ProjectStatus::is_closed"],
      },
      {
        id: "freelancer-money-negative-vs-zero",
        bucket: "adversarial",
        query: "find the money helper that distinguishes zero positive and negative amounts, not invoice totals",
        expect: ["Money::is_zero", "Money::is_positive", "Money::is_negative"],
      },
      {
        id: "freelancer-repository-row-mappers",
        bucket: "adversarial",
        query: "row_to_client row_to_lead and row_to_project map sqlite rows but belong to different repositories",
        expect: ["ClientRepository::row_to_client", "LeadRepository::row_to_lead", "ProjectRepository::row_to_project"],
      },
      {
        id: "freelancer-domain-event-metadata-version",
        bucket: "adversarial",
        query: "domain events attach metadata and explicit version separate from event store append logic",
        expect: ["DomainEvent::with_metadata", "DomainEvent::with_version"],
      },
      {
        id: "freelancer-timer-placeholders",
        bucket: "adversarial",
        query: "start timer and stop timer commands are present even though work item persistence is still placeholder-like",
        expect: ["start_timer", "stop_timer"],
      },
      {
        id: "freelancer-pagination-response",
        bucket: "adversarial",
        query: "pagination type and paginated response count pages separately from repository find_all filters",
        expect: ["Pagination", "PaginatedResponse"],
      },
      {
        id: "freelancer-invoice-status-outstanding",
        bucket: "adversarial",
        query: "invoice status helper treats draft sent overdue and paid differently for outstanding dashboard totals",
        expect: ["InvoiceStatus::is_outstanding", "InvoiceStatus::is_paid"],
      },
      {
        id: "freelancer-tauri-command-surface",
        bucket: "adversarial",
        query: "all business actions are exposed as Tauri commands from one command module rather than separate controllers",
        expect: ["create_invoice", "record_payment", "list_communications", "get_app_info"],
      },
    ],
  },
  {
    id: "traderotate",
    name: "traderotate",
    language_mix: "JavaScript Hardhat/Remix arbitrage runners",
    pathCandidates: [String.raw`C:\Users\alber\source\repos\traderotate`],
    queries: [
      {
        id: "traderotate-runtime-config",
        query: "runtime config merges default JSON config and validates network router quoter factory token addresses",
        expect: ["resolveRuntimeConfig", "validateNetworkConfig"],
      },
      {
        id: "traderotate-remix-context",
        query: "browser Remix runner resolves wallet provider chain signer owner address and network context",
        expect: ["getContext", "getWalletProvider", "getChain"],
      },
      {
        id: "traderotate-smart-batch-mode",
        query: "smart batch mode probes wallet capabilities for atomic wallet_sendCalls support",
        expect: ["getSmartBatchMode"],
      },
      {
        id: "traderotate-wallet-batch",
        query: "chunk MetaMask wallet_sendCalls below maximum batch size and send batches sequentially",
        expect: ["chunkCalls", "sendWalletBatch"],
      },
      {
        id: "traderotate-load-artifact",
        query: "load executor artifact from configured path or Hardhat artifacts fallback before deployment",
        expect: ["loadExecutorArtifact", "getArtifactCandidates"],
      },
      {
        id: "traderotate-store-executor-address",
        query: "store executor address per chain and owner in local storage or persisted node state",
        expect: ["saveStoredExecutorAddress", "getExecutorStorageKey", "getExecutorStateKey"],
      },
      {
        id: "traderotate-deploy-executor",
        query: "deploy executor and auto fund WETH after deployment using configured owner router and token addresses",
        expect: ["deployExecutor"],
      },
      {
        id: "traderotate-ensure-executor",
        query: "ensure executor reuses stored address or deploys fresh contract and then syncs settings",
        expect: ["ensureExecutor"],
      },
      {
        id: "traderotate-sync-contract-settings",
        query: "sync contract settings checks owner risk config approvals allowed tokens and setup calls",
        expect: ["syncContractSettings", "encodeRiskConfigStruct"],
      },
      {
        id: "traderotate-quoter-liquidity",
        query: "quote Uniswap exactInputSingle and compare pool liquidity before accepting candidate route",
        expect: ["quoteExactInputSingle", "getPoolLiquidity"],
      },
      {
        id: "traderotate-gas-diagnostics",
        query: "hunter loop computes gas ceiling diagnostics and fee plan before submitting a transaction",
        expect: ["getFeePlan", "buildGasCeilingDiagnostics", "huntOnce"],
      },
      {
        id: "traderotate-hunt-loop",
        query: "hunter loop checks owner balance risk config route quotes and submits arbitrage transaction",
        expect: ["huntOnce", "createCycleServices"],
      },
      {
        id: "traderotate-deep-merge-config",
        query: "deep merge nested network overrides without replacing unrelated default config sections",
        expect: ["deepMerge", "mergeConfig"],
      },
      {
        id: "traderotate-state-load-save",
        query: "node runner loads and saves json state file while tolerating missing or malformed state",
        expect: ["loadState", "saveState"],
      },
      {
        id: "traderotate-token-metadata-cache",
        query: "candidate route services cache token metadata symbols and decimals by token address",
        expect: ["maybeGetTokenMetadata", "createCycleServices"],
      },
      {
        id: "traderotate-deadline-builder",
        query: "build transaction deadline from latest chain timestamp and configured max deadline seconds",
        expect: ["buildDeadline"],
      },
      {
        id: "traderotate-contract-static-gas",
        query: "contract helper performs static call and gas estimation across ethers versions",
        expect: ["staticCall", "estimateGas"],
      },
      {
        id: "traderotate-chain-hex-vs-quantity",
        bucket: "adversarial",
        query: "distinguish chain hex formatting for wallet capability keys from generic quantity hex formatting",
        expect: ["toChainHex", "toHexQuantity"],
      },
      {
        id: "traderotate-storage-key-prefixes",
        bucket: "adversarial",
        query: "browser runner reads executor address from traderotate storage prefix and legacy crypt prefix",
        expect: ["getExecutorStorageKeys", "LEGACY_EXECUTOR_STORAGE_PREFIX", "EXECUTOR_STORAGE_PREFIX"],
      },
      {
        id: "traderotate-unsupported-chain-message",
        bucket: "adversarial",
        query: "unsupported chain message lists configured supported chains and warns local dev chains need overrides",
        expect: ["getUnsupportedChainMessage"],
      },
      {
        id: "traderotate-risk-config-normalization",
        bucket: "adversarial",
        query: "normalize on-chain risk config tuple into comparable bigint fields before deciding update",
        expect: ["normalizeRiskConfigStruct", "encodeRiskConfigStruct"],
      },
      {
        id: "traderotate-batch-completion-poll",
        bucket: "adversarial",
        query: "poll wallet_getCallsStatus until wallet batch confirms fails or times out",
        expect: ["waitForBatchCompletion"],
      },
      {
        id: "traderotate-address-normalization",
        bucket: "adversarial",
        query: "normalize address labels with ethers checksum and throw useful errors for invalid addresses",
        expect: ["normalizeAddress"],
      },
      {
        id: "traderotate-route-count",
        bucket: "adversarial",
        query: "count configured arbitrage routes from runtime config tokens and fee plan",
        expect: ["buildRouteCount"],
      },
      {
        id: "traderotate-funding-weth-flow",
        bucket: "adversarial",
        query: "after deploying executor wrap ETH into WETH then transfer WETH funding to executor",
        expect: ["deployExecutor", "WETH_ABI"],
      },
    ],
  },
  {
    id: "the-green-cedar",
    name: "the-green-cedar",
    language_mix: "TypeScript/Next.js/Payload CMS app on WSL",
    pathCandidates: [
      String.raw`\\wsl.localhost\Ubuntu\home\albert\.openclaw\workspace\projects\the-green-cedar`,
      String.raw`\\wsl$\Ubuntu\home\albert\.openclaw\workspace\projects\the-green-cedar`,
    ],
    queries: [
      {
        id: "cedar-posts-collection",
        query: "Payload posts collection with drafts versions slugs authors hero media categories and published access",
        expect: ["Posts"],
      },
      {
        id: "cedar-comments-moderation",
        query: "comments collection defaults public submissions to pending moderation and restricts editorial fields",
        expect: ["Comments", "hasEditorialAccess"],
      },
      {
        id: "cedar-editorial-access",
        query: "admin and editor access predicates for editorial writes previews comments and published filtering",
        expect: ["hasEditorialAccess", "adminsOrEditors"],
      },
      {
        id: "cedar-content-blocks",
        query: "configure rich text content blocks for callout code embed image gallery and quote blocks",
        expect: ["contentBlocks", "CalloutBlock", "GalleryBlock"],
      },
      {
        id: "cedar-rich-text-renderer",
        query: "render Lexical rich text block nodes including galleries pull quotes media figures and unknown nodes",
        expect: ["RichTextRenderer", "blockRenderers", "renderUnknownNode"],
      },
      {
        id: "cedar-media-figure",
        query: "media figure chooses responsive image source for card content gallery hero variants and captions",
        expect: ["MediaFigure"],
      },
      {
        id: "cedar-site-header-client",
        query: "site header client manages mobile drawer focus trap active navigation and theme toggle controls",
        expect: ["SiteHeaderClient", "getFocusableElements", "isActivePath"],
      },
      {
        id: "cedar-theme-toggle",
        query: "theme toggle resolves system preference stores selected theme and applies document theme state",
        expect: ["ThemeToggle", "resolveTheme", "applyTheme"],
      },
      {
        id: "cedar-editorial-summary-widget",
        query: "admin dashboard widget summarizes posts pages comments drafts and recent editorial activity",
        expect: ["EditorialSummaryWidget"],
      },
      {
        id: "cedar-editorial-attention-widget",
        query: "admin dashboard widget surfaces pending comments drafts and editorial items needing attention",
        expect: ["EditorialAttentionWidget"],
      },
      {
        id: "cedar-payload-config",
        bucket: "adversarial",
        query: "Payload build config registers collections globals lexical editor plugins dashboard widgets and SQLite database",
        expect: ["buildConfig", "contentBlocks", "Posts", "Comments"],
      },
      {
        id: "cedar-preview-state",
        query: "preview state checks editorial user access to allow draft preview and route preview API requests",
        expect: ["hasEditorialAccess", "preview"],
      },
      {
        id: "cedar-shared-fields",
        query: "shared hero and content field builders define reusable Payload field groups for pages and posts",
        expect: ["heroField", "contentField"],
      },
      {
        id: "cedar-reading-rail",
        query: "post reading rail slugifies headings tracks scroll progress reading minutes and related posts",
        expect: ["PostReadingRail", "slugify"],
      },
      {
        id: "cedar-comments-auth-buttons",
        query: "comment auth buttons call social sign in sign out and show client error status near the comment form",
        expect: ["CommentAuthButtons", "hasAuthClientError"],
      },
      {
        id: "cedar-topic-card",
        query: "topic card links category slugs to posts category pages and displays post counts",
        expect: ["TopicCard"],
      },
      {
        id: "cedar-wordpress-import-runner",
        query: "WordPress rich content import fetches legacy posts sanitizes html resolves hero images and writes Payload content",
        expect: ["fetchWordPressPosts", "sanitizeLegacyHtml", "resolveHeroImage", "run"],
      },
      {
        id: "cedar-generated-schema-distractor",
        bucket: "adversarial",
        query: "Payload generated schema exports posts comments and payload_kv tables but should not replace hand written collection configs",
        expect: ["posts", "comments", "payload_kv", "GeneratedDatabaseSchema"],
      },
      {
        id: "cedar-config-vs-generated-schema",
        bucket: "adversarial",
        query: "find the hand written Payload config registering Posts Comments content blocks and dashboard widgets instead of generated schema tables",
        expect: ["payload.config", "contentBlocks", "Posts", "Comments"],
      },
      {
        id: "cedar-clone-db-dependency-order",
        bucket: "adversarial",
        query: "clone libsql database orders tables by foreign key dependencies before inserting rows",
        expect: ["orderTablesForInsert", "readDependencies"],
      },
      {
        id: "cedar-clone-db-ident-quoting",
        bucket: "adversarial",
        query: "clone database script quotes sqlite identifiers before pragma and insert statements",
        expect: ["quoteIdent", "readColumns", "insertRows"],
      },
      {
        id: "cedar-inline-style-sanitizer",
        bucket: "adversarial",
        query: "WordPress importer sanitizes inline styles by allowed properties before preserving legacy HTML",
        expect: ["sanitizeInlineStyle", "allowedInlineStyles"],
      },
      {
        id: "cedar-media-download-filename",
        bucket: "adversarial",
        query: "WordPress importer decodes URL entities sanitizes filenames and infers extension from content type",
        expect: ["decodeUrlEntities", "sanitizeFilename", "extensionFromContentType", "downloadMedia"],
      },
      {
        id: "cedar-smoke-pages-mobile-menu",
        bucket: "adversarial",
        query: "QA smoke script opens mobile menu captures screenshot and checks broken images on pages",
        expect: ["smoke-pages", "attachPageCollectors", "findBrokenImages"],
      },
      {
        id: "cedar-report-compare-diffs",
        bucket: "adversarial",
        query: "QA compare reports summarizes result differences between two JSON reports by result name",
        expect: ["compareReports", "summarizeResult", "readReport"],
      },
    ],
  },
  {
    id: "sourcetrail",
    name: "Sourcetrail",
    language_mix: "Large C++ repository with Java indexer sources and Python example projects",
    pathCandidates: [String.raw`C:\Users\alber\source\repos\Sourcetrail`],
    queries: sourcetrailQueries,
  },
];

const activeSuites = suites.filter(
  (suite) => selectedProjectIds.size === 0 || selectedProjectIds.has(suite.id.toLowerCase()),
);

const resolvedSuites = activeSuites.map((suite) => ({
  ...suite,
  projectPath: suite.pathCandidates.find((candidate) => fs.existsSync(candidate)) ?? null,
}));

if (listOnly) {
  console.log(JSON.stringify(suiteManifest(resolvedSuites), null, 2));
  process.exit(0);
}

await main();

async function main() {
  const missing = resolvedSuites.filter((suite) => !suite.projectPath);
  if (missing.length) {
    throw new Error(
      `missing cross-repo project path(s): ${missing.map((suite) => suite.id).join(", ")}`,
    );
  }
  if (resolvedSuites.length < requiredProjects) {
    throw new Error(
      `cross-repo promotion requires at least ${requiredProjects} projects; got ${resolvedSuites.length}`,
    );
  }
  if (!fs.existsSync(cli)) {
    throw new Error(`missing codestory CLI binary: ${cli}`);
  }

  fs.mkdirSync(outDir, { recursive: true });
  const server = await startProfileServer(profileConfig);
  try {
    const projectResults = [];
    for (const suite of resolvedSuites) {
      projectResults.push(await runSuite(suite));
    }

    const allQueries = projectResults.flatMap((project) => project.queries);
    const aggregate = metrics(allQueries);
    const bucketScores = bucketMetrics(allQueries);
    const adversarialScore = bucketScores.adversarial;
    const allSearchMs = allQueries.map((query) => query.elapsed_ms).filter((ms) => Number.isFinite(ms));
    const gate = {
      passed:
        aggregate.hit_at_10 >= minAggregateHit10 &&
        aggregate.mrr_at_10 >= minAggregateMrr10 &&
        (!adversarialScore || adversarialScore.hit_at_10 >= minAdversarialHit10) &&
        percentile(allSearchMs, 0.95) <= maxSearchP95Ms &&
        projectResults.every((project) => project.score.hit_at_10 >= minProjectHit10),
      min_aggregate_hit_at_10: minAggregateHit10,
      min_aggregate_mrr_at_10: minAggregateMrr10,
      min_project_hit_at_10: minProjectHit10,
      min_adversarial_hit_at_10: minAdversarialHit10,
      max_search_p95_ms: maxSearchP95Ms,
    };
    const quality = 0.7 * aggregate.mrr_at_10 + 0.2 * aggregate.hit_at_10 + 0.1 * aggregate.hit_at_1;
    const result = {
      generated_at: new Date().toISOString(),
      cli,
      out_dir: outDir,
      profile: profileConfig.summary,
      skip_index: skipIndex,
      extra_index_args: extraIndexArgs,
      extra_search_args: extraSearchArgs,
      search_mode: searchMode,
      query_count: allQueries.length,
      project_count: projectResults.length,
      score: quality * 1_000_000,
      search_query_ms_mean: mean(allSearchMs),
      search_query_ms_p50: percentile(allSearchMs, 0.5),
      search_query_ms_p95: percentile(allSearchMs, 0.95),
      search_query_ms_max: percentile(allSearchMs, 1),
      aggregate,
      bucket_scores: bucketScores,
      gate,
      projects: projectResults,
    };

    fs.writeFileSync(path.join(outDir, "results.json"), `${JSON.stringify(result, null, 2)}\n`);
    fs.writeFileSync(path.join(outDir, "query-ranks.csv"), queryCsv(projectResults));
    fs.writeFileSync(path.join(outDir, "summary.md"), summaryMarkdown(result));

    console.log(`cross-repo promotion summary: ${path.join(outDir, "summary.md")}`);
    console.log(`METRIC cross_repo_score=${result.score.toFixed(6)}`);
    console.log(`METRIC cross_repo_hit_at_10=${aggregate.hit_at_10.toFixed(9)}`);
    console.log(`METRIC cross_repo_mrr_at_10=${aggregate.mrr_at_10.toFixed(9)}`);
    console.log(`METRIC cross_repo_hit_at_1=${aggregate.hit_at_1.toFixed(9)}`);
    console.log(`METRIC cross_repo_query_count=${result.query_count}`);
    if (adversarialScore) {
      console.log(`METRIC cross_repo_adversarial_hit_at_10=${adversarialScore.hit_at_10.toFixed(9)}`);
      console.log(`METRIC cross_repo_adversarial_mrr_at_10=${adversarialScore.mrr_at_10.toFixed(9)}`);
      console.log(`METRIC cross_repo_adversarial_query_count=${adversarialScore.query_count}`);
    }
    console.log(
      `METRIC cross_repo_projects_passed=${projectResults.filter((p) => p.score.hit_at_10 >= minProjectHit10).length}`,
    );
    console.log(`METRIC cross_repo_search_query_ms_p95=${result.search_query_ms_p95.toFixed(3)}`);
    console.log(`METRIC cross_repo_gate_passed=${gate.passed ? 1 : 0}`);

    if (!gate.passed && !allowFail) {
      process.exitCode = 2;
    }
  } finally {
    await stopProfileServer(server);
  }
}

async function runSuite(suite) {
  const suiteDir = path.join(outDir, suite.id);
  const cacheDir = path.join(suiteDir, "cache");
  const logsDir = path.join(suiteDir, "logs");
  fs.mkdirSync(logsDir, { recursive: true });
  console.log(`cross-repo suite ${suite.id}: ${suite.projectPath}`);

  let indexSeconds = null;
  let indexedFileCount = null;
  let indexedNodeCount = null;
  let semanticDocCount = null;
  let indexPhaseTimings = null;
  let embeddingModel = "";
  let retrievalMode = "";
  if (!skipIndex) {
    const index = runCli(
      [
        "index",
        "--project",
        suite.projectPath,
        "--cache-dir",
        cacheDir,
        "--refresh",
        "full",
        "--format",
        "json",
        ...extraIndexArgs,
      ],
      path.join(logsDir, "index.log"),
    );
    indexSeconds = index.elapsedMs / 1000;
    const indexJson = parseJson(index.stdout);
    indexPhaseTimings = indexJson.phase_timings ?? null;
    indexedFileCount = Number(indexJson.summary?.stats?.file_count ?? indexJson.stats?.file_count ?? 0);
    indexedNodeCount = Number(indexJson.summary?.stats?.node_count ?? indexJson.stats?.node_count ?? 0);
    if (!indexedFileCount || !indexedNodeCount) {
      throw new Error(
        `${suite.id} indexed ${indexedFileCount} files and ${indexedNodeCount} nodes; choose projects with supported source files`,
      );
    }
    semanticDocCount =
      indexJson.retrieval?.semantic_doc_count ??
      indexJson.summary?.retrieval?.semantic_doc_count ??
      indexJson.phase_timings?.semantic_docs_embedded ??
      null;
    embeddingModel =
      indexJson.retrieval?.embedding_model ?? indexJson.summary?.retrieval?.embedding_model ?? "";
    retrievalMode = indexJson.retrieval?.mode ?? indexJson.summary?.retrieval?.mode ?? "";
  }

  const queries = [];
  const searchServer = searchMode === "serve" ? await startSearchServer(suite, cacheDir, logsDir) : null;
  try {
    for (const query of suite.queries) {
      const search = searchServer
        ? await runServedSearch(searchServer, query, path.join(logsDir, `${query.id}.log`))
        : runCliSearch(suite, cacheDir, query, path.join(logsDir, `${query.id}.log`));
      const json = parseJson(search.stdout);
      const hits = json.indexed_symbol_hits ?? [];
      queries.push({
        id: query.id,
        bucket: query.bucket ?? defaultQueryBucket,
        query: query.query,
        expected: query.expect,
        elapsed_ms: search.elapsedMs,
        rank: findRank(hits, query),
        top: hits.slice(0, 5).map((hit) => hit.display_name ?? hit.node_ref ?? hit.file_path ?? ""),
      });
    }
  } finally {
    await stopSearchServer(searchServer);
  }

  return {
    id: suite.id,
    name: suite.name,
    language_mix: suite.language_mix,
    project_path: suite.projectPath,
    index_seconds: indexSeconds,
    indexed_file_count: indexedFileCount,
    indexed_node_count: indexedNodeCount,
    semantic_doc_count: semanticDocCount,
    index_phase_timings: indexPhaseTimings,
    embedding_model: embeddingModel,
    retrieval_mode: retrievalMode,
    search_seconds: sum(queries.map((query) => query.elapsed_ms)) / 1000,
    search_query_ms_mean: mean(queries.map((query) => query.elapsed_ms)),
    search_query_ms_p50: percentile(queries.map((query) => query.elapsed_ms), 0.5),
    search_query_ms_p95: percentile(queries.map((query) => query.elapsed_ms), 0.95),
    score: metrics(queries),
    bucket_scores: bucketMetrics(queries),
    queries,
  };
}

function runCliSearch(suite, cacheDir, query, logPath) {
  return runCli(
    [
      "search",
      "--project",
      suite.projectPath,
      "--cache-dir",
      cacheDir,
      "--query",
      query.query,
      "--limit",
      "10",
      "--repo-text",
      "off",
      "--refresh",
      "none",
      "--format",
      "json",
      ...extraSearchArgs,
    ],
    logPath,
  );
}

function runCli(args, logPath) {
  const start = performance.now();
  const proc = spawnSync(cli, args, {
    cwd: root,
    env: runEnv,
    encoding: "utf8",
    maxBuffer: 128 * 1024 * 1024,
  });
  const elapsedMs = performance.now() - start;
  const log = [
    `$ ${cli} ${args.map(shellQuote).join(" ")}`,
    "",
    "## stdout",
    proc.stdout ?? "",
    "",
    "## stderr",
    proc.stderr ?? "",
    "",
    `exit_code=${proc.status}`,
    `elapsed_ms=${elapsedMs.toFixed(3)}`,
  ].join("\n");
  fs.writeFileSync(logPath, log);
  if (proc.error) {
    throw proc.error;
  }
  if (proc.status !== 0) {
    throw new Error(`codestory CLI failed (${proc.status}); see ${logPath}`);
  }
  return { stdout: proc.stdout ?? "", stderr: proc.stderr ?? "", elapsedMs };
}

async function startSearchServer(suite, cacheDir, logsDir) {
  const port = await getFreePort();
  const addr = `127.0.0.1:${port}`;
  const stdoutPath = path.join(logsDir, "serve.stdout.log");
  const stderrPath = path.join(logsDir, "serve.stderr.log");
  const stdout = fs.openSync(stdoutPath, "w");
  const stderr = fs.openSync(stderrPath, "w");
  const args = [
    "serve",
    "--project",
    suite.projectPath,
    "--cache-dir",
    cacheDir,
    "--refresh",
    "none",
    "--addr",
    addr,
  ];
  const child = spawn(cli, args, {
    cwd: root,
    env: runEnv,
    stdio: ["ignore", stdout, stderr],
    windowsHide: true,
  });
  const server = {
    child,
    stdout,
    stderr,
    baseUrl: `http://${addr}`,
    command: `${cli} ${args.map(shellQuote).join(" ")}`,
  };
  try {
    await waitForHttpHealth(`${server.baseUrl}/health`, child, 60000);
    return server;
  } catch (error) {
    await stopSearchServer(server);
    throw error;
  }
}

async function stopSearchServer(server) {
  if (!server) {
    return;
  }
  server.child.kill();
  await new Promise((resolve) => setTimeout(resolve, 250));
  fs.closeSync(server.stdout);
  fs.closeSync(server.stderr);
}

async function runServedSearch(server, query, logPath) {
  const endpoint = new URL("/search", server.baseUrl);
  endpoint.searchParams.set("q", query.query);
  endpoint.searchParams.set("limit", "10");
  endpoint.searchParams.set("repo_text", "off");
  for (const arg of extraSearchArgs) {
    // Served mode intentionally supports only the stable benchmark search surface.
    // Extra CLI-only args should use CODESTORY_CROSS_REPO_SEARCH_MODE=cli.
    if (arg.trim()) {
      throw new Error(
        `CODESTORY_CROSS_REPO_SEARCH_ARGS is not supported with search_mode=serve; set CODESTORY_CROSS_REPO_SEARCH_MODE=cli`,
      );
    }
  }

  const start = performance.now();
  const stdout = await httpGet(endpoint);
  const elapsedMs = performance.now() - start;
  const log = [
    `$ ${server.command}`,
    `$ GET ${endpoint.toString()}`,
    "",
    "## stdout",
    stdout,
    "",
    "## stderr",
    "",
    "",
    "exit_code=0",
    `elapsed_ms=${elapsedMs.toFixed(3)}`,
  ].join("\n");
  fs.writeFileSync(logPath, log);
  return { stdout, stderr: "", elapsedMs };
}

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address ? address.port : null;
      server.close(() => {
        if (port) {
          resolve(port);
        } else {
          reject(new Error("failed to allocate a local port"));
        }
      });
    });
  });
}

async function waitForHttpHealth(url, child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`codestory serve exited before readiness with code ${child.exitCode}`);
    }
    try {
      await httpGet(url);
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw new Error(`codestory serve did not become ready: ${lastError?.message ?? "timeout"}`);
}

function httpGet(url) {
  return new Promise((resolve, reject) => {
    const endpoint = new URL(url);
    const req = http.request(
      {
        hostname: endpoint.hostname,
        port: Number(endpoint.port),
        path: `${endpoint.pathname}${endpoint.search}`,
        method: "GET",
        timeout: 120000,
      },
      (res) => {
        let data = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
            resolve(data);
          } else {
            reject(new Error(`HTTP ${res.statusCode}: ${data.slice(0, 200)}`));
          }
        });
      },
    );
    req.on("timeout", () => req.destroy(new Error("timeout")));
    req.on("error", reject);
    req.end();
  });
}

function findRank(hits, query) {
  const rank = hits.findIndex((hit) => {
    return query.expect.some((needle) => expectedSymbolMatchesHit(needle, hit));
  });
  return rank < 0 ? null : rank + 1;
}

function expectedSymbolMatchesHit(expected, hit) {
  const hitSegments = new Set(
    symbolSegments([hit.display_name, hit.node_ref, hit.file_path, hit.kind, hit.origin].filter(Boolean).join(" ")),
  );
  const expectedSegments = symbolSegments(expected);
  if (!expectedSegments.length) return false;
  return expectedSegments.every((segment) => hitSegments.has(segment));
}

function symbolSegments(value) {
  return String(value ?? "")
    .match(/[a-z0-9_]+/gi)
    ?.map((segment) => segment.toLowerCase())
    .filter(Boolean) ?? [];
}

function metrics(queryResults) {
  const count = queryResults.length;
  const ranks = queryResults.map((query) => query.rank).filter((rank) => rank !== null);
  const reciprocalSum = queryResults.reduce(
    (sumValue, query) => sumValue + (query.rank && query.rank <= 10 ? 1 / query.rank : 0),
    0,
  );
  const hitAt = (k) => ranks.filter((rank) => rank <= k).length / count;
  return {
    hit_at_1: hitAt(1),
    hit_at_3: hitAt(3),
    hit_at_5: hitAt(5),
    hit_at_10: hitAt(10),
    mrr_at_10: reciprocalSum / count,
    mean_rank_when_found: ranks.length ? sum(ranks) / ranks.length : null,
    misses: queryResults.filter((query) => query.rank === null).map((query) => query.id),
  };
}

function bucketMetrics(queryResults) {
  const buckets = new Map();
  for (const query of queryResults) {
    const bucket = query.bucket ?? defaultQueryBucket;
    if (!buckets.has(bucket)) {
      buckets.set(bucket, []);
    }
    buckets.get(bucket).push(query);
  }
  return Object.fromEntries(
    [...buckets.entries()]
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([bucket, rows]) => [bucket, { query_count: rows.length, ...metrics(rows) }]),
  );
}

function parseJson(output) {
  const text = String(output).trim();
  try {
    return JSON.parse(text);
  } catch {
    const startObject = text.indexOf("{");
    const startArray = text.indexOf("[");
    const start =
      startObject === -1 ? startArray : startArray === -1 ? startObject : Math.min(startObject, startArray);
    const endObject = text.lastIndexOf("}");
    const endArray = text.lastIndexOf("]");
    const end = Math.max(endObject, endArray);
    if (start >= 0 && end > start) {
      return JSON.parse(text.slice(start, end + 1));
    }
    throw new Error(`could not parse JSON from CLI output: ${text.slice(0, 500)}`);
  }
}

function summaryMarkdown(result) {
  const lines = [
    "# Cross-Repo Promotion Benchmark",
    "",
    `Generated: ${result.generated_at}`,
    `Gate: ${result.gate.passed ? "PASS" : "FAIL"}`,
    `Projects: ${result.project_count}`,
    `Queries: ${result.query_count}`,
    `Search mode: ${result.search_mode}`,
    `Score: ${result.score.toFixed(3)}`,
    `Aggregate Hit@10: ${formatRate(result.aggregate.hit_at_10)}`,
    `Aggregate MRR@10: ${formatRate(result.aggregate.mrr_at_10)}`,
    `Search latency p95: ${result.search_query_ms_p95.toFixed(1)} ms`,
    "",
    "## Bucket Results",
    "",
    "| Bucket | Queries | Hit@10 | MRR@10 | Misses |",
    "| --- | ---: | ---: | ---: | ---: |",
  ];
  for (const [bucket, score] of Object.entries(result.bucket_scores)) {
    lines.push(
      `| ${bucket} | ${score.query_count} | ${formatRate(score.hit_at_10)} | ${formatRate(score.mrr_at_10)} | ${score.misses.length} |`,
    );
  }
  lines.push(
    "",
    "## Index Footprint",
    "",
    "| Project | Index s | Files | Nodes | Semantic docs | Parse ms | Semantic build ms | Semantic embed ms |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  );
  for (const project of result.projects) {
    lines.push(
      `| ${project.id} | ${formatOptionalSeconds(project.index_seconds)} | ${formatOptionalInteger(project.indexed_file_count)} | ${formatOptionalInteger(project.indexed_node_count)} | ${formatOptionalInteger(project.semantic_doc_count)} | ${formatOptionalInteger(project.index_phase_timings?.parse_index_ms)} | ${formatOptionalInteger(project.index_phase_timings?.semantic_doc_build_ms)} | ${formatOptionalInteger(project.index_phase_timings?.semantic_embedding_ms)} |`,
    );
  }
  lines.push(
    "",
    "## Project Results",
    "",
    "| Project | Language mix | Queries | Hit@10 | MRR@10 | Misses | Search p95 ms |",
    "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
  );
  for (const project of result.projects) {
    lines.push(
      `| ${project.id} | ${project.language_mix} | ${project.queries.length} | ${formatRate(project.score.hit_at_10)} | ${formatRate(project.score.mrr_at_10)} | ${project.score.misses.length} | ${project.search_query_ms_p95.toFixed(1)} |`,
    );
  }
  lines.push("", "## Misses", "");
  const misses = result.projects.flatMap((project) =>
    project.queries
      .filter((query) => query.rank === null)
      .map((query) => ({ project: project.id, ...query })),
  );
  if (!misses.length) {
    lines.push("No misses.");
  } else {
    for (const miss of misses) {
      lines.push(`- ${miss.project}/${miss.id}: expected ${miss.expected.join(", ")}; top=${miss.top.join(" | ")}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

function queryCsv(projects) {
  const rows = [["project", "id", "bucket", "rank", "elapsed_ms", "expected", "query", "top"]];
  for (const project of projects) {
    for (const query of project.queries) {
      rows.push([
        project.id,
        query.id,
        query.bucket ?? defaultQueryBucket,
        query.rank ?? "",
        query.elapsed_ms.toFixed(3),
        query.expected.join("|"),
        query.query,
        query.top.join("|"),
      ]);
    }
  }
  return `${rows.map((row) => row.map(csvEscape).join(",")).join("\n")}\n`;
}

function suiteManifest(projects) {
  return {
    cli,
    out_dir: outDir,
    profile: profileConfig.summary,
    required_projects: requiredProjects,
    thresholds: {
      min_aggregate_hit_at_10: minAggregateHit10,
      min_aggregate_mrr_at_10: minAggregateMrr10,
      min_project_hit_at_10: minProjectHit10,
      min_adversarial_hit_at_10: minAdversarialHit10,
      max_search_p95_ms: maxSearchP95Ms,
    },
    projects: projects.map((project) => ({
      id: project.id,
      name: project.name,
      language_mix: project.language_mix,
      project_path: project.projectPath,
      path_candidates: project.pathCandidates,
      query_count: project.queries.length,
      queries: project.queries.map((query) => ({
        id: query.id,
        bucket: query.bucket ?? defaultQueryBucket,
        query: query.query,
        expect: query.expect,
      })),
    })),
  };
}

function configureProfile(name, env) {
  const normalized = String(name ?? "inherit").trim().toLowerCase();
  if (!normalized || normalized === "inherit" || normalized === "env") {
    return {
      name: "inherit",
      summary: {
        name: "inherit",
        note: "use caller-provided CodeStory embedding environment",
      },
      server: null,
    };
  }
  if (
    normalized !== "incumbent" &&
    normalized !== "current-incumbent" &&
    normalized !== "llama-bge-base-frontier"
  ) {
    throw new Error(`unknown cross-repo profile: ${name}`);
  }

  const port = Number(process.env.CODESTORY_CROSS_REPO_LLAMACPP_PORT ?? 8297);
  const llamaDir = process.env.CODESTORY_LLAMA_CPP_DIR ?? path.join(root, "target", "llamacpp", "b8840");
  const llamaExe = process.env.CODESTORY_LLAMA_CPP_SERVER ?? path.join(llamaDir, "llama-server.exe");
  const modelPath =
    process.env.CODESTORY_CROSS_REPO_MODEL_PATH ??
    path.join(root, "models", "gguf", "bge-base-en-v1.5", "bge-base-en-v1.5.Q8_0.gguf");
  const url = `http://127.0.0.1:${port}/v1/embeddings`;
  const docEmbedBatchSize = process.env.CODESTORY_CROSS_REPO_LLM_DOC_EMBED_BATCH_SIZE ?? "512";
  const storedVectorEncoding = process.env.CODESTORY_CROSS_REPO_STORED_VECTOR_ENCODING ?? "";
  const llamaServerBatch = process.env.CODESTORY_CROSS_REPO_LLAMACPP_SERVER_BATCH ?? "2048";
  const llamaServerUbatch = process.env.CODESTORY_CROSS_REPO_LLAMACPP_SERVER_UBATCH ?? "2048";

  env.CODESTORY_HYBRID_RETRIEVAL_ENABLED = "true";
  env.CODESTORY_SEMANTIC_DOC_SCOPE = "durable";
  env.CODESTORY_SEMANTIC_DOC_ALIAS_MODE = "alias_variant";
  env.CODESTORY_LLM_DOC_EMBED_BATCH_SIZE = docEmbedBatchSize;
  env.CODESTORY_EMBED_PROFILE = "bge-base-en-v1.5";
  env.CODESTORY_EMBED_BACKEND = "llamacpp";
  env.CODESTORY_EMBED_RUNTIME_MODE = "llamacpp";
  env.CODESTORY_EMBED_POOLING = "cls";
  env.CODESTORY_EMBED_LLAMACPP_URL = url;
  env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT = "4";
  env.CODESTORY_SEMANTIC_DOC_MAX_TOKENS =
    process.env.CODESTORY_CROSS_REPO_SEMANTIC_DOC_MAX_TOKENS ?? "384";
  delete env.CODESTORY_EMBED_EXECUTION_PROVIDER;
  delete env.CODESTORY_EMBED_MODEL_PATH;
  delete env.CODESTORY_EMBED_SESSION_COUNT;
  delete env.CODESTORY_EMBED_MAX_TOKENS;
  delete env.CODESTORY_EMBED_QUERY_PREFIX;
  delete env.CODESTORY_EMBED_DOCUMENT_PREFIX;
  delete env.CODESTORY_EMBED_LAYER_NORM;
  delete env.CODESTORY_EMBED_TRUNCATE_DIM;
  delete env.CODESTORY_EMBED_EXPECTED_DIM;
  if (storedVectorEncoding) {
    env.CODESTORY_STORED_VECTOR_ENCODING = storedVectorEncoding;
  } else {
    delete env.CODESTORY_STORED_VECTOR_ENCODING;
  }

  return {
    name: "current-incumbent",
    summary: {
      name: "current-incumbent",
      embedding_profile: "bge-base-en-v1.5",
      backend: "llamacpp",
      semantic_scope: "durable",
      doc_mode: "alias_variant",
      doc_embed_batch_size: Number(env.CODESTORY_LLM_DOC_EMBED_BATCH_SIZE),
      stored_vector_encoding: env.CODESTORY_STORED_VECTOR_ENCODING ?? "float32",
      semantic_doc_max_tokens: Number(env.CODESTORY_SEMANTIC_DOC_MAX_TOKENS),
      request_count: 4,
      llama_server_batch: Number(llamaServerBatch),
      llama_server_ubatch: Number(llamaServerUbatch),
      model_path: modelPath,
      llama_server_url: url,
    },
    server: {
      llamaDir,
      llamaExe,
      modelPath,
      port,
      url,
      args: [
        "-m",
        modelPath,
        "--embedding",
        "--pooling",
        "cls",
        "--host",
        "127.0.0.1",
        "--port",
        String(port),
        "--device",
        "Vulkan0",
        "-ngl",
        "999",
        "-c",
        "4096",
        "-b",
        llamaServerBatch,
        "-ub",
        llamaServerUbatch,
        "-np",
        "4",
        "-fa",
        "auto",
      ],
    },
  };
}

async function startProfileServer(config) {
  if (!config.server) {
    return null;
  }
  if (!fs.existsSync(config.server.llamaExe)) {
    throw new Error(`missing llama.cpp server: ${config.server.llamaExe}`);
  }
  if (!fs.existsSync(config.server.modelPath)) {
    throw new Error(`missing cross-repo profile model: ${config.server.modelPath}`);
  }
  const logsDir = path.join(outDir, "profile");
  fs.mkdirSync(logsDir, { recursive: true });
  const stdoutPath = path.join(logsDir, "llama-server.stdout.log");
  const stderrPath = path.join(logsDir, "llama-server.stderr.log");
  const stdout = fs.openSync(stdoutPath, "w");
  const stderr = fs.openSync(stderrPath, "w");
  const child = spawn(config.server.llamaExe, config.server.args, {
    cwd: config.server.llamaDir,
    stdio: ["ignore", stdout, stderr],
    windowsHide: true,
  });
  try {
    await waitForServer(config.server.url, child, 120000);
    return { child, stdout, stderr };
  } catch (error) {
    child.kill();
    fs.closeSync(stdout);
    fs.closeSync(stderr);
    throw error;
  }
}

async function stopProfileServer(server) {
  if (!server) {
    return;
  }
  server.child.kill();
  await new Promise((resolve) => setTimeout(resolve, 1000));
  fs.closeSync(server.stdout);
  fs.closeSync(server.stderr);
}

async function waitForServer(url, child, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`llama.cpp server exited before readiness with code ${child.exitCode}`);
    }
    try {
      await postEmbedding(url);
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
  }
  throw new Error(`llama.cpp server did not become ready: ${lastError?.message ?? "timeout"}`);
}

function postEmbedding(url) {
  return new Promise((resolve, reject) => {
    const endpoint = new URL(url);
    const body = JSON.stringify({ model: "probe", input: ["probe"] });
    const req = http.request(
      {
        hostname: endpoint.hostname,
        port: Number(endpoint.port),
        path: endpoint.pathname,
        method: "POST",
        headers: {
          "content-type": "application/json",
          "content-length": Buffer.byteLength(body),
        },
        timeout: 5000,
      },
      (res) => {
        let data = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
            resolve(data);
          } else {
            reject(new Error(`HTTP ${res.statusCode}: ${data.slice(0, 200)}`));
          }
        });
      },
    );
    req.on("timeout", () => req.destroy(new Error("timeout")));
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

function readArgValue(name) {
  const index = process.argv.indexOf(name);
  if (index >= 0 && process.argv[index + 1]) {
    return process.argv[index + 1];
  }
  const prefix = `${name}=`;
  const valueArg = process.argv.find((arg) => arg.startsWith(prefix));
  return valueArg ? valueArg.slice(prefix.length) : null;
}

function splitArgs(value) {
  if (!value || !value.trim()) {
    return [];
  }
  return value.trim().split(/\s+/);
}

function splitList(value) {
  if (!value || !value.trim()) {
    return [];
  }
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function shellQuote(value) {
  const text = String(value);
  return /\s/.test(text) ? JSON.stringify(text) : text;
}

function csvEscape(value) {
  const text = String(value ?? "");
  return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

function formatRate(value) {
  return value.toFixed(6);
}

function formatOptionalSeconds(value) {
  return Number.isFinite(value) ? value.toFixed(1) : "";
}

function formatOptionalInteger(value) {
  return Number.isFinite(value) ? String(Math.round(value)) : "";
}

function sum(values) {
  return values.reduce((total, value) => total + value, 0);
}

function mean(values) {
  return values.length ? sum(values) / values.length : 0;
}

function percentile(values, p) {
  if (!values.length) {
    return 0;
  }
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1));
  return sorted[idx];
}
