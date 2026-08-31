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

/** Production APIs that encode domain probe / capping tables (CX-R2). */
export const DELETED_PROBE_TABLE_APIS = Object.freeze([
  "packet_required_probe_multi_match_limit",
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

/** Strip `#[cfg(test)]` item bodies for a conservative production view. */
export function maskCfgTestItems(source) {
  let out = "";
  let i = 0;
  const re = /#\[cfg\(test\)\]/g;
  let match;
  while ((match = re.exec(source)) != null) {
    out += source.slice(i, match.index);
    let j = match.index + match[0].length;
    while (j < source.length && /[\s/]/.test(source[j])) {
      // skip whitespace and line/block comments before the item
      if (source.startsWith("//", j)) {
        const nl = source.indexOf("\n", j);
        j = nl < 0 ? source.length : nl + 1;
        continue;
      }
      if (source.startsWith("/*", j)) {
        const end = source.indexOf("*/", j + 2);
        j = end < 0 ? source.length : end + 2;
        continue;
      }
      if (source.startsWith("#[", j)) {
        // additional attributes
        const close = source.indexOf("]", j);
        j = close < 0 ? source.length : close + 1;
        continue;
      }
      if (/\s/.test(source[j])) {
        j += 1;
        continue;
      }
      break;
    }
    // find item end: brace-matched block or semicolon
    while (j < source.length && source[j] !== "{" && source[j] !== ";") {
      j += 1;
    }
    if (j >= source.length) {
      i = source.length;
      break;
    }
    if (source[j] === ";") {
      i = j + 1;
      re.lastIndex = i;
      continue;
    }
    let depth = 0;
    let k = j;
    while (k < source.length) {
      const ch = source[k];
      // Skip line comments.
      if (ch === "/" && source[k + 1] === "/") {
        const nl = source.indexOf("\n", k + 2);
        k = nl < 0 ? source.length : nl + 1;
        continue;
      }
      // Skip block comments.
      if (ch === "/" && source[k + 1] === "*") {
        const end = source.indexOf("*/", k + 2);
        k = end < 0 ? source.length : end + 2;
        continue;
      }
      // Skip raw strings: r##"..."## or br"..." / cr#"..."#.
      if (
        ch === "r"
        || ((ch === "b" || ch === "c") && source[k + 1] === "r")
      ) {
        let rawAt = ch === "r" ? k : k + 1;
        if (source[rawAt] === "r") {
          let hashes = 0;
          let p = rawAt + 1;
          while (source[p] === "#") {
            hashes += 1;
            p += 1;
          }
          if (source[p] === '"') {
            const closer = `"${"#".repeat(hashes)}`;
            const end = source.indexOf(closer, p + 1);
            k = end < 0 ? source.length : end + closer.length;
            continue;
          }
        }
      }
      // Skip ordinary / byte / c strings.
      if (ch === '"' || ((ch === "b" || ch === "c") && source[k + 1] === '"')) {
        k = ch === '"' ? k + 1 : k + 2;
        while (k < source.length) {
          if (source[k] === "\\") {
            k += 2;
            continue;
          }
          if (source[k] === '"') {
            k += 1;
            break;
          }
          k += 1;
        }
        continue;
      }
      // Skip char literals, including '"' / '}' / '\'' which otherwise break brace depth.
      if (ch === "'") {
        k += 1;
        if (source[k] === "\\") {
          k += 2;
        } else {
          k += 1;
        }
        if (source[k] === "'") {
          k += 1;
        }
        continue;
      }
      if (ch === "{") depth += 1;
      else if (ch === "}") {
        depth -= 1;
        if (depth === 0) {
          k += 1;
          break;
        }
      }
      k += 1;
    }
    i = k;
    re.lastIndex = i;
  }
  out += source.slice(i);
  // Also drop a trailing `mod tests { ... }` if present without cfg (rare).
  return out.replace(/\nmod tests\s*\{[\s\S]*\}\s*$/m, "\n");
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
  }

  return findings;
}

export function collectProductionPacketFiles(repoRoot = defaultRepoRoot()) {
  const files = [];
  for (const rel of PRODUCTION_SCAN_GLOBS) {
    files.push(...listRustFiles(path.join(repoRoot, rel)));
  }
  return files.sort();
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
  const findings = scanRepository(repoRoot);
  if (findings.length === 0) {
    return {
      exitCode: 0,
      stdout: `packet-generalization-boundary: ok (${collectProductionPacketFiles(repoRoot).length} production packet file(s))\n`,
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
