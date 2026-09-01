/**
 * Packet generalization boundary: keep production packet planning free of
 * benchmark corpora, holdout expected anchors, encoded brand detectors, and
 * deleted domain taxonomy / cleanup APIs.
 *
 * Vocabulary for those banned shapes is permitted only in tests, tooling, and
 * this checker / its fixtures.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const DELETED_TAXONOMY_APIS = Object.freeze([
  "packet_flow_requirements_for_terms",
  "append_flow_template_claims",
  "packet_append_event_output_flow_template_claims",
  "packet_append_indexing_pipeline_flow_template_claims",
  "packet_terms_indicate_indexing_flow",
  "packet_terms_indicate_request_dispatch_flow",
  "packet_terms_indicate_server_request_dispatch_flow",
  "packet_terms_indicate_server_route_dispatch_flow",
  "packet_terms_indicate_javascript_route_source_flow",
  "packet_terms_indicate_route_tree_dispatch_flow",
  "packet_terms_indicate_buffered_io_flow",
  "packet_terms_indicate_site_build_phase_flow",
  "packet_terms_indicate_log_record_handler_flow",
  "packet_terms_indicate_mapper_configuration_plan_flow",
  "packet_terms_indicate_prepared_session_adapter_flow",
  "packet_terms_indicate_search_execution_flow",
  "packet_terms_indicate_stylesheet_animation_flow",
  "packet_terms_indicate_html_css_template_structure_flow",
  "packet_terms_indicate_sql_schema_flow",
  "packet_terms_indicate_hook_cache_flow",
  "packet_terms_indicate_client_send_flow",
  "packet_terms_indicate_full_outbound_request_flow",
  "packet_terms_indicate_form_validation_flow",
  "packet_terms_indicate_event_loop_command_flow",
  "packet_terms_indicate_command_server_bootstrap_flow",
  "packet_terms_indicate_command_event_loop_flow",
  "packet_terms_indicate_network_command_input_flow",
  "packet_terms_indicate_command_dispatch_flow",
  "packet_terms_indicate_url_session_request_flow",
  "packet_terms_indicate_shell_version_use_flow",
  "packet_terms_indicate_shell_install_dispatch_flow",
  "packet_terms_indicate_string_predicate_flow",
  "packet_terms_indicate_runtime_formatting_flow",
  "packet_drop_unrequested_wide_char_siblings",
  "packet_drop_unrequested_python_siblings",
  "packet_drop_unrequested_windows_formatting_siblings",
  "packet_drop_unrequested_formatting_extension_siblings",
  "packet_drop_unrequested_formatter_specialization_siblings",
  "packet_drop_unrequested_single_letter_displays",
  "packet_drop_unrequested_named_client_adapter_siblings",
  "packet_drop_unrequested_example_and_binding_siblings",
  "packet_drop_unrequested_mapper_annotation_siblings",
  "packet_drop_unrequested_test_siblings",
  "packet_keep_shared_source_set_over_platform_duplicates",
  "packet_drop_unrequested_sql_schema_variant_siblings",
  "packet_drop_excess_unrequested_keyframe_siblings",
  "packet_drop_excess_unrequested_animation_class_siblings",
  "packet_drop_unrequested_animation_file_aliases",
  "packet_drop_unrequested_animation_file_only_sheets",
  "packet_drop_unrequested_non_stylesheet_animation_siblings",
  "packet_drop_unrequested_repo_root_stylesheet_siblings",
  "packet_drop_unrequested_non_primary_flow_siblings",
  "packet_drop_unrequested_duplicate_client_type_paths",
  "packet_drop_unrequested_export_macro_displays",
  "packet_drop_unrequested_system_format_failure_siblings",
  "packet_drop_unrequested_markdown_siblings",
]);

/** Historical holdout anchors that must never steer production packet code. */
export const HISTORICAL_EXPECTED_ANCHORS = Object.freeze([
  "src/requests/api.py",
  "src/requests/sessions.py",
  "src/requests/models.py",
  "src/requests/adapters.py",
  "PreparedRequest.prepare",
  "HTTPAdapter.send",
  "dart-http-client-flow",
  "language-expansion-holdout",
  "typescript-swr-hook-flow",
  "vercel-swr",
  "dart-lang-http",
]);

/**
 * Domain PacketEvidenceRole variants that steered capping / probe rank on the
 * failed freeze. Production may retain only structural path-based labels
 * (SourceEvidence, TestsAndRegressionCoverage).
 */
export const DELETED_DOMAIN_EVIDENCE_ROLES = Object.freeze([
  "SqlTableDefinition",
  "SqlRelationshipConstraint",
  "SqlSchemaFile",
  "IndexInputConfiguration",
  "IndexingWorkQueue",
  "InterceptorManagement",
  "RequestDispatch",
  "TransportAdapter",
  "ClientFactory",
  "EventLoop",
  "NetworkCommandInput",
  "CommandDispatch",
  "ArgumentPlanning",
  "SearchExecutionUnit",
  "CandidateFileConstruction",
  "SearchDriver",
  "CommandEntrypoint",
  "EventOutputProcessing",
  "AppServerRequestProtocol",
  "RuntimeOrchestration",
  "WorkspaceDiscoveryAndPlanning",
  "SnapshotRefresh",
  "PersistenceAndSearchProjection",
  "SymbolExtraction",
  "RouteHandling",
  "BufferedIo",
  "CollectionConfiguration",
]);

/** Hardcoded holdout probe spellings that must not grade ownership in production. */
export const DELETED_HOLDOUT_PROBE_SPELLINGS = Object.freeze([
  "requestentrypoint",
  "defaultinstance",
  "requestdispatch",
  "requestmethod",
  "requestinterceptor",
  "interceptorhandlers",
  "adapters",
  "transportadapter",
  "searchentrypoint",
  "searchexecution",
  "parallelsearch",
  "searchexecutionunit",
  "argumentplanning",
  "flagparsing",
  // CX-R2 residual tables
  "transportsend",
  "clientsendimplementation",
  "publicclientfacade",
  "httptoplevelhelper",
  "requestfinalization",
  "commanddispatch",
  "serverbootstrap",
  "eventloopsource",
  "sourcereadbuffer",
  "sinkwritebuffer",
  "htmlformrequiredconstraint",
  "urlsessioncallbackboundary",
  "mapperpublicapi",
  "sqlschemascripts",
  "handlerchain",
  "handlerdispatch",
  "requesthandler",
  "contextnexthandlerchain",
  "enginerequesthandler",
  "routeregistration",
  "enginecreationrouterstate",
  "formvalidationbypass",
  "indexingentrypoint",
  "filediscovery",
  "symbolextraction",
  "clienttransportsend",
  "commandserverbootstrap",
  "commandeventloop",
  "clientpublicfacade",
  "clientrequestfinalization",
  "formnativeconstraints",
  "formcustomvalidation",
  "sessioncallbacks",
  "bufferedsource",
  "bufferedsink",
  "bufferedwrapper",
]);

/** Production APIs that encode domain probe / capping tables (CX-R2 / CX-R3). */
export const DELETED_PROBE_TABLE_APIS = Object.freeze([
  "packet_required_probe_multi_match_limit",
  "task_class_seed_queries",
  "push_search_flow_probe_queries",
  "push_indexing_flow_required_probe_queries",
  "packet_citation_matches_route_dispatch_probe",
  "packet_citation_matches_route_registration_probe",
  "packet_citation_matches_route_engine_constructor_probe",
  "packet_citation_matches_buffered_wrapper_implementation",
  "packet_citation_matches_validation_bypass_probe",
  "packet_citation_matches_sql_schema_scripts_probe",
  "packet_citation_matches_public_api_surface_probe",
  "packet_required_probe_needs_full_token_coverage",
  "packet_required_probe_needs_buffered_wrapper_implementation",
]);

/** Fixed task-class retrieval seed phrases that steered required-probe capping (CX-R3). */
export const DELETED_TASK_CLASS_SEED_SPELLINGS = Object.freeze([
  "architectureentrypoint",
  "runtimeflow",
  "routehandlerendpoint",
  "pipelineflow",
  "storagehandoff",
  "errorpath",
  "failurehandling",
  "affectedsymbols",
  "impactedtests",
  "definitionreferences",
  "editcandidates",
  "testcoverage",
]);

/** Domain ownership predicate name patterns (CX-02). */
export const DOMAIN_OWNERSHIP_PREDICATE_PATTERNS = Object.freeze([
  /\bcitation_owns_[A-Za-z0-9_]+\b/g,
  /\bpacket_citation_owns_[A-Za-z0-9_]+\b/g,
]);

/** Match production normalize_identifier: keep ASCII alphanumerics, lowercase. */
export function normalizeIdentifier(value) {
  return String(value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "");
}

const PRODUCTION_SCAN_GLOBS = Object.freeze([
  "crates/codestory-agent/src",
  "crates/codestory-runtime/src/agent",
  "crates/codestory-runtime/src/packet.rs",
  "crates/codestory-runtime/src/search.rs",
  "crates/codestory-runtime/src/drill.rs",
  "crates/codestory-runtime/src/context.rs",
  "crates/codestory-runtime/src/ground.rs",
  "crates/codestory-retrieval/src",
  "crates/codestory-cli/src/packet.rs",
  "crates/codestory-cli/src/search.rs",
]);

/**
 * Vocabulary clusters that only appear in code steering answers toward a known
 * corpus. Each set carries the smallest cluster size that cannot occur by
 * accident in generic planning code.
 */
const DOMAIN_VOCABULARY_SHAPES = Object.freeze([
  Object.freeze({
    kind: "sql_dialect_cluster",
    minimum: 2,
    vocabulary: Object.freeze([
      "sqlite",
      "mysql",
      "postgres",
      "postgresql",
      "sqlserver",
      "mssql",
      "oracle",
      "db2",
      "mariadb",
      "autoincrement",
      "serialpks",
    ]),
  }),
  Object.freeze({
    // One occurrence is enough. Packet planning never needs to recognize a
    // query language by its syntax; code that does is reading a known corpus.
    kind: "sql_syntax_phrase",
    minimum: 1,
    vocabulary: Object.freeze([
      "createtable",
      "altertable",
      "droptable",
      "insertinto",
      "selectfrom",
      "foreignkey",
      "primarykey",
      "notnull",
    ]),
  }),
  Object.freeze({
    kind: "schema_noun_cluster",
    minimum: 3,
    // "relation" and "references" are ordinary graph words, so a cluster only
    // counts when enough of it is relational-schema vocabulary that nothing else
    // uses. Three generic graph nouns together stay legal.
    minimumCore: 2,
    core: Object.freeze([
      "table",
      "tables",
      "column",
      "columns",
      "schema",
      "foreign",
      "constraint",
      "constraints",
    ]),
    vocabulary: Object.freeze([
      "table",
      "tables",
      "column",
      "columns",
      "schema",
      "relation",
      "relations",
      "relationship",
      "relationships",
      "foreign",
      "constraint",
      "constraints",
      "references",
    ]),
  }),
  Object.freeze({
    kind: "filename_stem_cluster",
    minimum: 4,
    vocabulary: Object.freeze([
      "cli",
      "cmd",
      "command",
      "commands",
      "lib",
      "mod",
      "index",
      "main",
      "app",
      "server",
      "router",
      "routes",
      "route",
      "handler",
      "handlers",
      "entrypoint",
      "entrypoints",
      "controller",
      "middleware",
      "events",
      "event",
    ]),
  }),
  Object.freeze({
    kind: "corpus_entity_noun_cluster",
    minimum: 3,
    vocabulary: Object.freeze([
      "artist",
      "artists",
      "album",
      "albums",
      "track",
      "tracks",
      "invoice",
      "invoices",
      "invoiceline",
      "playlist",
      "playlists",
      "customer",
      "customers",
      "employee",
      "employees",
      "genre",
      "genres",
      "publisher",
      "publishers",
      "supplier",
      "shipper",
      "orderitem",
    ]),
  }),
]);

/** Identifiers that carry the caller's prompt text into a function body. */
const PROMPT_TEXT_BINDINGS = Object.freeze([
  "question",
  "prompt",
  "query_text",
  "task_phrasing",
]);

/** String-content tests that turn prompt text into a branch. */
const PROMPT_TEXT_PREDICATES = Object.freeze([
  "contains",
  "starts_with",
  "ends_with",
  "find",
  "rfind",
  "split_once",
  "rsplit_once",
  "matches",
]);

const PERMITTED_VOCABULARY_PATH_FRAGMENTS = Object.freeze([
  `${path.sep}tests${path.sep}`,
  `${path.sep}scripts${path.sep}tests${path.sep}`,
  `${path.sep}scripts${path.sep}lib${path.sep}packet-generalization-boundary.mjs`,
  `${path.sep}scripts${path.sep}check-packet-generalization-boundary.mjs`,
  `${path.sep}benches${path.sep}`,
  `${path.sep}codestory-bench${path.sep}`,
  `${path.sep}benchmarks${path.sep}`,
]);

const BENCHMARK_DEPENDENCY_PATTERNS = Object.freeze([
  /(?:^|[^A-Za-z0-9_/])benchmarks\//m,
  /codestory-bench/,
  /language-expansion-holdout/,
  /(?:^|[^A-Za-z0-9_])eval[_-]manifest\b/m,
  /task_manifest_snapshot/,
  /expected_files\s*:/,
  /expected_symbols\s*:/,
  /expected_claims\s*:/,
]);

function defaultRepoRoot() {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
}

export function decodeAsciiByteArrayLiterals(source) {
  const decoded = [];
  const re = /\[\s*((?:\d{1,3}\s*,\s*)*\d{1,3})\s*\]/g;
  let match;
  while ((match = re.exec(source)) != null) {
    const nums = match[1].split(",").map((part) => Number(part.trim()));
    if (nums.length === 0 || nums.some((n) => !Number.isInteger(n) || n < 0 || n > 255)) {
      continue;
    }
    if (!nums.every((n) => (n >= 32 && n <= 126) || n === 9 || n === 10 || n === 13)) {
      continue;
    }
    const text = String.fromCharCode(...nums);
    if (/[A-Za-z]/.test(text)) {
      decoded.push({ literal: match[0], text, index: match.index });
    }
  }
  return decoded;
}

function isPermittedVocabularyPath(filePath, repoRoot) {
  const relative = path.relative(repoRoot, filePath).split(path.sep).join("/");
  if (relative.startsWith("scripts/tests/") || relative.startsWith("scripts/lib/packet-generalization")) {
    return true;
  }
  if (relative.startsWith("scripts/check-packet-generalization-boundary.mjs")) {
    return true;
  }
  if (relative.includes("/tests/") || relative.endsWith("_test.rs") || relative.endsWith(".test.mjs")) {
    return true;
  }
  if (relative.startsWith("benchmarks/") || relative.startsWith("crates/codestory-bench/")) {
    return true;
  }
  // Eval-only probe hooks may name holdout fixtures; production planning must not import their
  // taxonomy. The module itself is permitted vocabulary so the checker can focus on planner code.
  if (relative.endsWith("/eval_probes.rs") || relative.endsWith("\\eval_probes.rs")) {
    return true;
  }
  // cfg(test) modules inside production files are still production scan targets;
  // callers mask them before scanning when needed. Path-level permit is for
  // dedicated test/tooling trees only.
  void PERMITTED_VOCABULARY_PATH_FRAGMENTS;
  return false;
}

function listRustFiles(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "target" || entry.name === "node_modules") continue;
      out.push(...listRustFiles(full));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      out.push(full);
    }
  }
  return out;
}

/**
 * Replace the contents of every comment, string, and char literal with spaces,
 * preserving byte offsets and newlines. Structural scans run against this view so
 * a marker written inside a comment or a literal cannot steer them.
 */
export function blankNonCode(source) {
  const out = source.split("");
  const blank = (from, to) => {
    for (let i = from; i < to && i < out.length; i += 1) {
      if (out[i] !== "\n") out[i] = " ";
    }
  };
  let i = 0;
  while (i < source.length) {
    const ch = source[i];
    if (ch === "/" && source[i + 1] === "/") {
      const nl = source.indexOf("\n", i);
      const end = nl < 0 ? source.length : nl;
      blank(i, end);
      i = end;
      continue;
    }
    if (ch === "/" && source[i + 1] === "*") {
      let depth = 1;
      let j = i + 2;
      while (j < source.length && depth > 0) {
        if (source[j] === "/" && source[j + 1] === "*") {
          depth += 1;
          j += 2;
          continue;
        }
        if (source[j] === "*" && source[j + 1] === "/") {
          depth -= 1;
          j += 2;
          continue;
        }
        j += 1;
      }
      blank(i, j);
      i = j;
      continue;
    }
    // Raw strings: r"..", r#".."#, br#".."#, cr#".."#.
    const rawPrefix = /^(?:b|c)?r(#*)"/.exec(source.slice(i, i + 8));
    if (rawPrefix != null && (i === 0 || !/[A-Za-z0-9_]/.test(source[i - 1]))) {
      const hashes = rawPrefix[1];
      const openAt = i + rawPrefix[0].length;
      const closer = `"${hashes}`;
      const end = source.indexOf(closer, openAt);
      const stop = end < 0 ? source.length : end;
      blank(openAt, stop);
      i = end < 0 ? source.length : end + closer.length;
      continue;
    }
    if (ch === '"' || ((ch === "b" || ch === "c") && source[i + 1] === '"')) {
      const openAt = ch === '"' ? i + 1 : i + 2;
      let j = openAt;
      while (j < source.length) {
        if (source[j] === "\\") {
          j += 2;
          continue;
        }
        if (source[j] === '"') break;
        j += 1;
      }
      blank(openAt, j);
      i = j + 1;
      continue;
    }
    if (ch === "'") {
      // A lifetime (`'a`) is not a literal; a char literal always closes with `'`.
      const escaped = source[i + 1] === "\\";
      const closeAt = escaped ? source.indexOf("'", i + 2) : i + 2;
      if (!escaped && source[closeAt] === "'") {
        blank(i + 1, closeAt);
        i = closeAt + 1;
        continue;
      }
      if (escaped && closeAt > 0 && closeAt - i <= 8) {
        blank(i + 1, closeAt);
        i = closeAt + 1;
        continue;
      }
      i += 1;
      continue;
    }
    i += 1;
  }
  return out.join("");
}

/** Byte ranges of every `#[cfg(test)]` item, located on the blanked view. */
function cfgTestItemRanges(source) {
  const blanked = blankNonCode(source);
  const ranges = [];
  const marker = /#\[cfg\(test\)\]/g;
  let match;
  while ((match = marker.exec(blanked)) != null) {
    let j = match.index + match[0].length;
    while (j < blanked.length) {
      if (/\s/.test(blanked[j])) {
        j += 1;
        continue;
      }
      if (blanked.startsWith("#[", j)) {
        const close = blanked.indexOf("]", j);
        j = close < 0 ? blanked.length : close + 1;
        continue;
      }
      break;
    }
    while (j < blanked.length && blanked[j] !== "{" && blanked[j] !== ";") {
      j += 1;
    }
    if (j >= blanked.length) {
      ranges.push([match.index, source.length]);
      break;
    }
    if (blanked[j] === ";") {
      ranges.push([match.index, j + 1]);
      marker.lastIndex = j + 1;
      continue;
    }
    let depth = 0;
    let k = j;
    while (k < blanked.length) {
      if (blanked[k] === "{") depth += 1;
      else if (blanked[k] === "}") {
        depth -= 1;
        if (depth === 0) {
          k += 1;
          break;
        }
      }
      k += 1;
    }
    ranges.push([match.index, k]);
    marker.lastIndex = k;
  }
  return ranges;
}

/** Strip `#[cfg(test)]` item bodies for a conservative production view. */
export function maskCfgTestItems(source) {
  const ranges = cfgTestItemRanges(source);
  if (ranges.length === 0) {
    return source;
  }
  let out = "";
  let cursor = 0;
  for (const [start, end] of ranges) {
    if (start < cursor) continue;
    out += source.slice(cursor, start);
    cursor = end;
  }
  out += source.slice(cursor);
  return out;
}

/**
 * Split a production view into functions. Brace matching runs on the blanked
 * view; the returned body is the real source so literal contents stay visible.
 */
export function splitRustFunctions(source) {
  const blanked = blankNonCode(source);
  const functions = [];
  const signature = /\bfn\s+([A-Za-z_][A-Za-z0-9_]*)/g;
  let match;
  while ((match = signature.exec(blanked)) != null) {
    let j = signature.lastIndex;
    let depth = 0;
    let bodyStart = -1;
    while (j < blanked.length) {
      const ch = blanked[j];
      if (ch === ";" && depth === 0 && bodyStart < 0) break;
      if (ch === "{") {
        if (bodyStart < 0) bodyStart = j;
        depth += 1;
      } else if (ch === "}") {
        depth -= 1;
        if (depth === 0) {
          j += 1;
          break;
        }
      }
      j += 1;
    }
    if (bodyStart < 0) continue;
    functions.push({ name: match[1], start: match.index, end: j, body: source.slice(bodyStart, j) });
    signature.lastIndex = j;
  }
  return functions;
}

/** String literals appearing directly in a function body. */
function functionStringLiterals(body) {
  const literals = [];
  const re = /"((?:[^"\\]|\\.){1,160})"/g;
  let match;
  while ((match = re.exec(body)) != null) {
    literals.push(match[1]);
  }
  return literals;
}

/**
 * Domain vocabulary clusters: a single function enumerating several members of a
 * corpus-specific vocabulary is steering answers, whatever the members are named.
 */
function findDomainVocabularyClusters(source, relative) {
  const findings = [];
  for (const fn of splitRustFunctions(source)) {
    const literals = functionStringLiterals(fn.body);
    const tokens = new Set(
      literals
        .flatMap((literal) => literal.split(/[^A-Za-z0-9]+/))
        .map((token) => token.toLowerCase())
        .filter(Boolean),
    );
    // A multi-word literal is one phrase, so its compacted form counts too:
    // "CREATE TABLE" is a single piece of SQL vocabulary, not two nouns.
    for (const literal of literals) {
      const compacted = normalizeIdentifier(literal);
      if (compacted) tokens.add(compacted);
    }
    for (const shape of DOMAIN_VOCABULARY_SHAPES) {
      const matched = shape.vocabulary.filter((word) => tokens.has(word));
      const core = shape.core == null
        ? matched
        : shape.core.filter((word) => tokens.has(word));
      if (matched.length >= shape.minimum && core.length >= (shape.minimumCore ?? 0)) {
        findings.push({
          kind: shape.kind,
          file: relative,
          detail: `${fn.name} enumerates ${matched.join(", ")}`,
        });
      }
    }
  }
  return findings;
}

/**
 * Production planning may read the prompt, but it must not branch on which words
 * the prompt happens to use. Any literal-valued string test against prompt text
 * is a hardcoded answer shape.
 */
function findPromptTextBranches(source, relative) {
  const findings = [];
  const predicates = PROMPT_TEXT_PREDICATES.join("|");
  for (const fn of splitRustFunctions(source)) {
    const carriers = new Set(PROMPT_TEXT_BINDINGS);
    const binding = new RegExp(
      `\\blet\\s+(?:mut\\s+)?([A-Za-z_][A-Za-z0-9_]*)\\s*=\\s*[^;]*\\b(?:${[...carriers].join("|")})\\b`,
      "g",
    );
    // Two passes so a binding chain (question -> lowered -> trimmed) is followed.
    for (let pass = 0; pass < 2; pass += 1) {
      binding.lastIndex = 0;
      let bound;
      while ((bound = binding.exec(fn.body)) != null) {
        carriers.add(bound[1]);
      }
    }
    // Only word arguments count. Testing the prompt for punctuation, a path
    // separator, or a file extension reads its structure, not its vocabulary.
    const test = new RegExp(
      `\\b(${[...carriers].join("|")})\\s*\\.\\s*(?:${predicates})\\s*\\(\\s*"([^"]*)"`,
      "g",
    );
    let branch;
    while ((branch = test.exec(fn.body)) != null) {
      const argument = branch[2];
      if (!/^[A-Za-z][A-Za-z ]{2,}$/.test(argument)) continue;
      findings.push({
        kind: "prompt_text_branch",
        file: relative,
        detail: `${fn.name} branches on prompt wording "${argument}"`,
      });
      break;
    }
  }
  return findings;
}

export function findBoundaryViolations(source, { filePath = "<memory>", repoRoot = defaultRepoRoot() } = {}) {
  const findings = [];
  const relative = filePath === "<memory>"
    ? "<memory>"
    : path.relative(repoRoot, filePath).split(path.sep).join("/");
  const productionView = maskCfgTestItems(source);

  if (!isPermittedVocabularyPath(filePath === "<memory>" ? path.join(repoRoot, "crates/codestory-agent/src/packet_terms.rs") : filePath, repoRoot)
      || filePath === "<memory>") {
    for (const pattern of BENCHMARK_DEPENDENCY_PATTERNS) {
      if (pattern.test(productionView)) {
        findings.push({
          kind: "benchmark_dependency",
          file: relative,
          detail: `matched ${pattern}`,
        });
      }
    }

    for (const anchor of HISTORICAL_EXPECTED_ANCHORS) {
      if (productionView.includes(anchor)) {
        findings.push({
          kind: "historical_expected_anchor",
          file: relative,
          detail: anchor,
        });
      }
    }

    for (const api of DELETED_TAXONOMY_APIS) {
      const re = new RegExp(`\\b${api}\\b`);
      if (re.test(productionView)) {
        findings.push({
          kind: "deleted_taxonomy_api",
          file: relative,
          detail: api,
        });
      }
    }

    for (const role of DELETED_DOMAIN_EVIDENCE_ROLES) {
      const re = new RegExp(`\\bPacketEvidenceRole::${role}\\b|\\bSelf::${role}\\b`);
      if (re.test(productionView)) {
        findings.push({
          kind: "domain_evidence_role",
          file: relative,
          detail: role,
        });
      }
      // Enum variant definitions also ban reintroduction.
      const enumRe = new RegExp(`\\b${role}\\b\\s*[,{]`);
      if (
        /enum\s+PacketEvidenceRole\b/.test(productionView)
        && enumRe.test(productionView)
      ) {
        findings.push({
          kind: "domain_evidence_role",
          file: relative,
          detail: `enum variant ${role}`,
        });
      }
    }

    for (const spelling of DELETED_HOLDOUT_PROBE_SPELLINGS) {
      // Match quoted literals in compacted or space-separated form after normalize.
      const stringLitRe = /["']([^"']{2,120})["']/g;
      let lit;
      const seenSpell = new Set();
      while ((lit = stringLitRe.exec(productionView)) != null) {
        const normalized = normalizeIdentifier(lit[1]);
        if (normalized === spelling && !seenSpell.has(spelling)) {
          seenSpell.add(spelling);
          findings.push({
            kind: "holdout_probe_spelling",
            file: relative,
            detail: `${spelling} <= "${lit[1]}"`,
          });
        }
      }
      // Unquoted match-arm identifiers only (not prose): | transportsend => or | transportsend |
      const armRe = new RegExp(
        `(?:^|[^A-Za-z0-9_])(?:\\|\\s*)?${spelling}\\s*(?:\\||=>)`,
      );
      if (armRe.test(productionView) && !seenSpell.has(spelling)) {
        findings.push({
          kind: "holdout_probe_spelling",
          file: relative,
          detail: spelling,
        });
      }
    }

    for (const api of DELETED_PROBE_TABLE_APIS) {
      const re = new RegExp(`\\b${api}\\b`);
      if (re.test(productionView)) {
        findings.push({
          kind: "deleted_probe_table_api",
          file: relative,
          detail: api,
        });
      }
    }

    for (const spelling of DELETED_TASK_CLASS_SEED_SPELLINGS) {
      const stringLitRe = /["']([^"']{2,120})["']/g;
      let lit;
      const seen = new Set();
      while ((lit = stringLitRe.exec(productionView)) != null) {
        const normalized = normalizeIdentifier(lit[1]);
        if (normalized === spelling && !seen.has(spelling)) {
          seen.add(spelling);
          findings.push({
            kind: "task_class_seed_spelling",
            file: relative,
            detail: `${spelling} <= "${lit[1]}"`,
          });
        }
      }
    }

    if (
      /task-class retrieval seed/i.test(productionView)
      || /purpose:\s*"task-class retrieval seed"/i.test(productionView)
    ) {
      findings.push({
        kind: "task_class_seed_purpose",
        file: relative,
        detail: "task-class retrieval seed",
      });
    }

    // Coverage-role alias table: clienttransportsend-style arms inside
    // packet_citation_matches_required_coverage_role.
    if (
      /fn\s+packet_citation_matches_required_coverage_role\b/.test(productionView)
      && /normalized_role\s*==\s*"clienttransportsend"|clienttransportsend|commandeventloop|formnativeconstraints/.test(
        productionView,
      )
    ) {
      findings.push({
        kind: "coverage_role_alias_table",
        file: relative,
        detail: "packet_citation_matches_required_coverage_role holdout aliases",
      });
    }

    for (const pattern of DOMAIN_OWNERSHIP_PREDICATE_PATTERNS) {
      pattern.lastIndex = 0;
      let match;
      const seen = new Set();
      while ((match = pattern.exec(productionView)) != null) {
        if (seen.has(match[0])) continue;
        seen.add(match[0]);
        findings.push({
          kind: "domain_ownership_predicate",
          file: relative,
          detail: match[0],
        });
      }
    }

    for (const decoded of decodeAsciiByteArrayLiterals(productionView)) {
      const lower = decoded.text.toLowerCase();
      if (lower === "swr" || HISTORICAL_EXPECTED_ANCHORS.some((a) => a.toLowerCase() === lower)) {
        findings.push({
          kind: "encoded_brand_literal",
          file: relative,
          detail: `${decoded.literal} => "${decoded.text}"`,
        });
      }
    }

    findings.push(...findDomainVocabularyClusters(productionView, relative));
    findings.push(...findPromptTextBranches(productionView, relative));
  }

  return findings;
}

export function collectProductionPacketFiles(repoRoot = defaultRepoRoot()) {
  const files = new Set();
  for (const rel of PRODUCTION_SCAN_GLOBS) {
    const target = path.join(repoRoot, rel);
    if (existsSync(target) && statSync(target).isFile()) {
      if (target.endsWith(".rs")) files.add(target);
      continue;
    }
    for (const file of listRustFiles(target)) {
      files.add(file);
    }
  }
  return [...files].sort();
}

export function scanRepository(repoRoot = defaultRepoRoot()) {
  const findings = [];
  for (const filePath of collectProductionPacketFiles(repoRoot)) {
    const source = readFileSync(filePath, "utf8");
    findings.push(...findBoundaryViolations(source, { filePath, repoRoot }));
  }
  return findings;
}

export function formatFindings(findings) {
  return findings
    .map((f) => `packet-generalization-boundary: ${f.kind} ${f.detail} in ${f.file}`)
    .join("\n");
}

export function runPacketGeneralizationBoundaryCheck(repoRoot = defaultRepoRoot()) {
  if (!existsSync(repoRoot) || !statSync(repoRoot).isDirectory()) {
    return {
      exitCode: 2,
      stdout: "",
      stderr: `packet-generalization-boundary: repository root not found: ${repoRoot}\n`,
      findings: [],
    };
  }
  const scanned = collectProductionPacketFiles(repoRoot);
  if (scanned.length === 0) {
    // Scanning nothing is the loudest possible bypass, not a clean result.
    return {
      exitCode: 2,
      stdout: "",
      stderr:
        "packet-generalization-boundary: scanned 0 production packet files; "
        + `expected sources under ${PRODUCTION_SCAN_GLOBS.join(", ")}\n`,
      findings: [],
    };
  }
  const findings = scanRepository(repoRoot);
  if (findings.length === 0) {
    return {
      exitCode: 0,
      stdout: `packet-generalization-boundary: ok (${scanned.length} production packet file(s))\n`,
      stderr: "",
      findings,
    };
  }
  return {
    exitCode: 1,
    stdout: "",
    stderr: `${formatFindings(findings)}\npacket-generalization-boundary: ${findings.length} violation(s)\n`,
    findings,
  };
}
