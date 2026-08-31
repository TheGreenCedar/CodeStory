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
  /benchmarks\//,
  /codestory-bench/,
  /language-expansion-holdout/,
  /eval[_-]manifest/,
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
  // Remove cfg(test) mod blocks and trailing test modules heuristically.
  return source
    .replace(/#\[cfg\(test\)\][\s\S]*?(?=\n(?:pub\s+)?(?:fn|struct|enum|impl|mod|const|type|use|#\[|$))/g, "\n")
    .replace(/\nmod tests\s*\{[\s\S]*\}\s*$/m, "\n");
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
