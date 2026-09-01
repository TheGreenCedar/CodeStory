import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  DELETED_DOMAIN_EVIDENCE_ROLES,
  DELETED_HOLDOUT_PROBE_SPELLINGS,
  DELETED_PROBE_TABLE_APIS,
  DELETED_TAXONOMY_APIS,
  decodeAsciiByteArrayLiterals,
  findBoundaryViolations,
  normalizeIdentifier,
  runPacketGeneralizationBoundaryCheck,
} from "../lib/packet-generalization-boundary.mjs";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

function writeTree(root, files) {
  for (const [relative, contents] of Object.entries(files)) {
    const full = path.join(root, relative);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, contents);
  }
}

test("decodes ASCII byte-array literals including encoded swr", () => {
  const source = "let brand = [115, 119, 114]; let other = [100, 97, 114, 116];";
  const decoded = decodeAsciiByteArrayLiterals(source);
  assert.deepEqual(
    decoded.map((d) => d.text).sort(),
    ["dart", "swr"],
  );
});

test("fixture contaminated head fails for taxonomy, cleanup, encoded brand, and holdout anchors", () => {
  const contaminated = `
pub fn packet_terms_indicate_hook_cache_flow(terms: &[String]) -> bool {
    let encoded = [115, 119, 114];
    terms.iter().any(|t| t == "swr")
}
pub fn packet_drop_unrequested_markdown_siblings(rows: &mut Vec<()>) {}
pub fn packet_flow_requirements_for_terms(terms: &[String]) -> Vec<()> { vec![] }
pub fn append_flow_template_claims() {}
const HOLDOUT: &str = "language-expansion-holdout";
const PATH: &str = "src/requests/api.py";
`;
  const findings = findBoundaryViolations(contaminated, {
    filePath: path.join(repositoryRoot, "crates/codestory-agent/src/packet_terms.rs"),
    repoRoot: repositoryRoot,
  });
  const kinds = new Set(findings.map((f) => f.kind));
  assert.ok(kinds.has("deleted_taxonomy_api"), findings);
  assert.ok(kinds.has("encoded_brand_literal"), findings);
  assert.ok(kinds.has("historical_expected_anchor"), findings);
  assert.ok(
    findings.some((f) => f.detail.includes("packet_terms_indicate_hook_cache_flow")),
    findings,
  );
  assert.ok(
    findings.some((f) => f.detail.includes("packet_drop_unrequested_markdown_siblings")),
    findings,
  );
});

test("renaming a domain classifier while keeping benchmark-shaped behavior still fails", () => {
  // Hostile rename: surface looks new, but still encodes the holdout brand and
  // still implements a prompt→domain-flow classifier + cleanup pass.
  const renamed = `
pub fn packet_terms_indicate_cache_hook_pipeline(terms: &[String]) -> bool {
    let brand = [115, 119, 114];
    std::str::from_utf8(&brand).unwrap();
    terms.iter().any(|t| t.contains("hook"))
}
pub fn packet_drop_unrequested_sibling_noise(rows: &mut Vec<()>) {
    let _ = "dart-http-client-flow";
}
`;
  const findings = findBoundaryViolations(renamed, {
    filePath: path.join(repositoryRoot, "crates/codestory-agent/src/packet_scoring.rs"),
    repoRoot: repositoryRoot,
  });
  assert.ok(
    findings.some((f) => f.kind === "encoded_brand_literal"),
    "encoded brand must still fail after rename",
  );
  assert.ok(
    findings.some((f) => f.kind === "historical_expected_anchor" && f.detail.includes("dart-http-client-flow")),
    findings,
  );
});

test("clean generic seed / projection surface passes", () => {
  const clean = `
pub fn extract_packet_query_terms(question: &str) -> Vec<String> {
    question.split_whitespace().map(str::to_string).collect()
}
pub fn packet_citation_key(path: &str, start: u32, end: u32) -> String {
    format!("{path}:{start}:{end}")
}
`;
  const findings = findBoundaryViolations(clean, {
    filePath: path.join(repositoryRoot, "crates/codestory-agent/src/packet_plan.rs"),
    repoRoot: repositoryRoot,
  });
  assert.deepEqual(findings, []);
});

test("isolated clean fixture repository passes the live scanner", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "packet-boundary-clean-"));
  try {
    writeTree(root, {
      "crates/codestory-agent/src/packet_plan.rs":
        "pub fn extract_packet_query_terms(q: &str) -> Vec<String> { vec![q.to_string()] }\n",
      "crates/codestory-runtime/src/agent/mod.rs": "pub mod packet_plan;\n",
    });
    const result = runPacketGeneralizationBoundaryCheck(root);
    assert.equal(result.exitCode, 0, result.stderr);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("isolated contaminated fixture repository fails the live scanner", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "packet-boundary-dirty-"));
  try {
    writeTree(root, {
      "crates/codestory-agent/src/packet_terms.rs":
        `pub fn packet_terms_indicate_hook_cache_flow(terms: &[String]) -> bool { let _ = [115,119,114]; true }\n`,
      "crates/codestory-runtime/src/agent/orchestrator.rs":
        "fn rank() { packet_drop_unrequested_markdown_siblings(); }\n",
    });
    const result = runPacketGeneralizationBoundaryCheck(root);
    assert.equal(result.exitCode, 1, result.stdout);
    assert.match(result.stderr, /deleted_taxonomy_api|encoded_brand_literal/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("fixture with domain carriers, roles, and holdout probe spellings fails", () => {
  const leaked = `
pub enum PacketEvidenceRole {
    TransportAdapter,
    SourceEvidence,
}
pub fn packet_citation_owns_transport_adapter(c: &()) -> bool { true }
pub fn citation_owns_client_request_entrypoint(c: &()) -> bool { true }
fn match_probe(q: &str) -> bool {
    match q {
        "requestentrypoint" | "adapters" | "transportadapter" => true,
        _ => false,
    }
}
fn rank(role: PacketEvidenceRole) -> u8 {
    match role {
        PacketEvidenceRole::TransportAdapter => 4,
        _ => 1,
    }
}
`;
  const findings = findBoundaryViolations(leaked, {
    filePath: path.join(repositoryRoot, "crates/codestory-agent/src/packet_evidence_roles.rs"),
    repoRoot: repositoryRoot,
  });
  const kinds = new Set(findings.map((f) => f.kind));
  assert.ok(kinds.has("domain_ownership_predicate"), findings);
  assert.ok(kinds.has("domain_evidence_role"), findings);
  assert.ok(kinds.has("holdout_probe_spelling"), findings);
  assert.ok(DELETED_DOMAIN_EVIDENCE_ROLES.includes("TransportAdapter"));
  assert.ok(DELETED_HOLDOUT_PROBE_SPELLINGS.includes("requestentrypoint"));
});

test("space-separated and CX-R2 probe tables fail the strengthened checker", () => {
  assert.equal(normalizeIdentifier("flag parsing"), "flagparsing");
  assert.equal(normalizeIdentifier("search entrypoint"), "searchentrypoint");
  const leaked = `
fn packet_required_probe_multi_match_limit(query: &str) -> Option<usize> {
    match normalize_identifier(query).as_str() {
        "transportsend" | "commanddispatch" => Some(2),
        _ => None,
    }
}
fn packet_citation_matches_required_coverage_role(q: &str, c: &()) -> bool {
    let normalized_role = "clienttransportsend";
    normalized_role == "clienttransportsend"
}
pub fn push_search_flow_probe_queries(queries: &mut Vec<String>) {
    queries.push("flag parsing".to_string());
    queries.push("search entrypoint".to_string());
}
`;
  const findings = findBoundaryViolations(leaked, {
    filePath: path.join(repositoryRoot, "crates/codestory-runtime/src/agent/packet_capping.rs"),
    repoRoot: repositoryRoot,
  });
  const kinds = new Set(findings.map((f) => f.kind));
  assert.ok(kinds.has("holdout_probe_spelling"), findings);
  assert.ok(kinds.has("deleted_probe_table_api"), findings);
  assert.ok(kinds.has("coverage_role_alias_table"), findings);
  assert.ok(DELETED_PROBE_TABLE_APIS.includes("packet_required_probe_multi_match_limit"));
});

test("task-class seed tables that elevate into required probes fail the checker", () => {
  const leaked = `
fn task_class_seed_queries(task_class: PacketTaskClassDto) -> &'static [&'static str] {
    match task_class {
        PacketTaskClassDto::RouteTracing => &["route handler endpoint", "references"],
        PacketTaskClassDto::ArchitectureExplanation => &["architecture entrypoint"],
        PacketTaskClassDto::DataFlow => &["pipeline flow"],
        _ => &[],
    }
}
fn build_plan() {
    push_packet_query(&mut queries, "route handler endpoint", "task-class retrieval seed");
}
`;
  const findings = findBoundaryViolations(leaked, {
    filePath: path.join(repositoryRoot, "crates/codestory-agent/src/packet_plan.rs"),
    repoRoot: repositoryRoot,
  });
  const kinds = new Set(findings.map((f) => f.kind));
  assert.ok(kinds.has("deleted_probe_table_api"), findings);
  assert.ok(kinds.has("task_class_seed_spelling"), findings);
  assert.ok(kinds.has("task_class_seed_purpose"), findings);
  assert.ok(DELETED_PROBE_TABLE_APIS.includes("task_class_seed_queries"));
});

test("cfg(test) modules with char literals are masked from production scans", () => {
  const source = `
pub fn live() {}
#[cfg(test)]
mod legacy_source_scans {
    fn packet_first_sql_identifier(input: &str) -> Option<String> {
        let quote = match input.chars().next() {
            Some('"') | Some('\\'') | Some(']') => Some(']'),
            _ => None,
        };
        let _ = quote;
        Some("client transport send".to_string())
    }
}
pub fn also_live() {}
`;
  const findings = findBoundaryViolations(source, {
    filePath: path.join(repositoryRoot, "crates/codestory-runtime/src/agent/orchestrator.rs"),
    repoRoot: repositoryRoot,
  });
  assert.equal(
    findings.filter((f) => f.kind === "holdout_probe_spelling").length,
    0,
    findings,
  );
});

// ---------------------------------------------------------------------------
// Counterexamples: the five steering sites deleted in this PR, reproduced from
// their pre-deletion source. Each must be caught if it is ever written again,
// under any name, because the checker matches shape and not identifier.
// ---------------------------------------------------------------------------

function agentFile(name) {
  return path.join(repositoryRoot, "crates/codestory-agent/src", name);
}

function runtimeAgentFile(name) {
  return path.join(repositoryRoot, "crates/codestory-runtime/src/agent", name);
}

function kindsFor(source, filePath) {
  return new Set(
    findBoundaryViolations(source, { filePath, repoRoot: repositoryRoot }).map((f) => f.kind),
  );
}

test("site 1 counterexample: probe-term retention branching on prompt wording", () => {
  // crates/codestory-agent/src/packet_terms.rs, deleted at e74db13a.
  const deleted = `
fn packet_retains_non_primary_probe_term(question: &str, term: &str) -> bool {
    if matches!(term, "source" | "sources") {
        let lowered = question.to_ascii_lowercase();
        return lowered.contains("buffer")
            || lowered.contains("sink")
            || lowered.contains("read")
            || lowered.contains("write");
    }

    if matches!(term, "bench" | "benchmark" | "benchmarks") {
        let lowered = question.to_ascii_lowercase();
        return lowered.contains("architecture")
            && (lowered.contains("boundary")
                || lowered.contains("boundaries")
                || lowered.contains("across"));
    }

    false
}
`;
  const findings = findBoundaryViolations(deleted, {
    filePath: agentFile("packet_terms.rs"),
    repoRoot: repositoryRoot,
  });
  assert.ok(
    findings.some((f) => f.kind === "prompt_text_branch"),
    `prompt-wording retention must be caught: ${JSON.stringify(findings)}`,
  );
});

test("site 1 counterexample: the same retention renamed and inlined is still caught", () => {
  // Hostile variant: no `question` parameter name, no `contains`, chained binding.
  const renamed = `
fn packet_term_survives(user_prompt: &str, term: &str) -> bool {
    let prompt = user_prompt.to_ascii_lowercase();
    let phrasing = prompt.trim();
    matches!(term, "source") && phrasing.starts_with("buffered reader")
}
`;
  const kinds = kindsFor(renamed, agentFile("packet_terms.rs"));
  assert.ok(kinds.has("prompt_text_branch"), [...kinds].join(","));
});

test("site 2 counterexample: named schema entity extraction from the prompt", () => {
  // crates/codestory-agent/src/packet_required_probes.rs, deleted at e74db13a.
  const deleted = `
pub fn packet_named_schema_entity_queries(question: &str) -> Vec<String> {
    let lower = question.to_ascii_lowercase();
    let Some(start) = [" between ", " among "]
        .into_iter()
        .filter_map(|marker| lower.find(marker).map(|index| index + marker.len()))
        .min()
    else {
        return Vec::new();
    };
    let tail = &lower[start..];
    let segment = tail.replace(" and ", ",");
    let mut queries = Vec::new();
    for phrase in segment.split(',') {
        let words = phrase.split_whitespace().collect::<Vec<_>>();
        if words.iter().any(|word| {
            matches!(
                *word,
                "database"
                    | "relation"
                    | "relations"
                    | "relationship"
                    | "relationships"
                    | "schema"
                    | "sql"
                    | "table"
                    | "tables"
            )
        }) {
            continue;
        }
        queries.push(words.join(" "));
    }
    queries
}
`;
  const kinds = kindsFor(deleted, agentFile("packet_required_probes.rs"));
  assert.ok(kinds.has("schema_noun_cluster"), [...kinds].join(","));
});

test("site 3 counterexample: SQL dialect ranking and promotion in the orchestrator", () => {
  // crates/codestory-runtime/src/agent/orchestrator.rs, deleted at e74db13a.
  const deleted = `
fn sql_schema_dialect_rank(path: &str) -> f32 {
    let lower = packet_display_path(path).to_ascii_lowercase();
    if lower.contains("sqlite") {
        4.0
    } else if lower.contains("mysql") || lower.contains("postgres") || lower.contains("postgresql")
    {
        3.0
    } else if lower.contains("sqlserver") {
        1.0
    } else {
        0.0
    }
}

fn promote_sql_schema_dialect_files(answer: &mut AgentAnswerDto) {
    for marker in ["sqlite", "mysql", "postgres"] {
        let _ = marker;
    }
}
`;
  const findings = findBoundaryViolations(deleted, {
    filePath: runtimeAgentFile("orchestrator.rs"),
    repoRoot: repositoryRoot,
  });
  assert.ok(
    findings.filter((f) => f.kind === "sql_dialect_cluster").length >= 2,
    `both dialect functions must be caught: ${JSON.stringify(findings)}`,
  );
});

test("site 3 counterexample: the foreign-key promotion arm alone is caught", () => {
  // The narrowest slice of the deleted block: no dialect names at all, only
  // relational-constraint vocabulary. This is the reintroduction the flat
  // four-word threshold used to miss.
  const deleted = `
fn promote_sql_schema_relationship_constraints(answer: &mut AgentAnswerDto) {
    for citation in &mut answer.citations {
        let display = citation.display_name.to_ascii_lowercase();
        if !(display.contains("foreign")
            || display.contains("constraint")
            || display.contains("references")
            || display.contains("fk_"))
        {
            continue;
        }
        citation.coverage_role = Some(PACKET_MATERIAL_SCHEMA_ENTITY_ROLE.to_string());
    }
}
`;
  const kinds = kindsFor(deleted, runtimeAgentFile("orchestrator.rs"));
  assert.ok(kinds.has("schema_noun_cluster"), [...kinds].join(","));
});

test("generic graph vocabulary is not a schema cluster", () => {
  // Guard on the previous test's threshold: relation/references/relationship are
  // ordinary edge words and must stay legal in retrieval code.
  const legitimate = `
fn edge_kind_label(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::REFERENCES => "references",
        EdgeKind::RELATION => "relation",
        _ => "relationship",
    }
}
`;
  const kinds = kindsFor(legitimate, path.join(repositoryRoot, "crates/codestory-retrieval/src/ranker.rs"));
  assert.ok(!kinds.has("schema_noun_cluster"), [...kinds].join(","));
});

test("site 4 counterexample: SQL dialect variant-copy scoring", () => {
  // crates/codestory-agent/src/packet_scoring.rs, deleted at e74db13a.
  const deleted = `
pub fn packet_sql_schema_file_is_variant_copy(path: &str) -> bool {
    let lower = packet_display_path(path).to_ascii_lowercase();
    if !lower.ends_with(".sql") {
        return false;
    }
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    file_name.contains("autoincrement")
        || file_name.contains("serialpks")
        || file_name.contains("serial_pks")
        || file_name.contains("db2")
        || file_name.contains("oracle")
        || file_name.contains("sqlserver")
}
`;
  const kinds = kindsFor(deleted, agentFile("packet_scoring.rs"));
  assert.ok(kinds.has("sql_dialect_cluster"), [...kinds].join(","));
});

test("site 5 counterexample: filename-stem navigation scoring", () => {
  // crates/codestory-runtime/src/agent/packet_capping.rs, deleted at e74db13a.
  const deleted = `
fn packet_source_navigation_file_score(path: &str) -> u8 {
    let normalized = packet_display_path(path).replace('\\\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
        .to_ascii_lowercase();
    match stem.as_str() {
        "cli" | "cmd" | "command" | "commands" => 4,
        "lib" | "mod" | "index" => 3,
        "events" | "event" => 2,
        "main" | "app" | "server" | "router" | "routes" => 2,
        "handler" | "handlers" | "entrypoint" | "entrypoints" => 1,
        _ => 0,
    }
}
`;
  const kinds = kindsFor(deleted, runtimeAgentFile("packet_capping.rs"));
  assert.ok(kinds.has("filename_stem_cluster"), [...kinds].join(","));
});

test("bypass 1: a commented or quoted cfg(test) marker cannot hide production code", () => {
  // The masker used to run on raw text, so a marker inside a comment opened a
  // mask range that swallowed every following production item.
  const hidden = `
// #[cfg(test)]
// mod tests {
pub fn rank_by_dialect(path: &str) -> u8 {
    let lower = path.to_ascii_lowercase();
    if lower.contains("sqlite") || lower.contains("postgres") { 4 } else { 0 }
}
`;
  const kinds = kindsFor(hidden, runtimeAgentFile("orchestrator.rs"));
  assert.ok(kinds.has("sql_dialect_cluster"), `comment marker must not mask: ${[...kinds]}`);

  const quoted = `
const MARKER: &str = "#[cfg(test)] mod tests {";
pub fn rank_by_dialect(path: &str) -> u8 {
    let lower = path.to_ascii_lowercase();
    if lower.contains("sqlite") || lower.contains("mysql") { 4 } else { 0 }
}
`;
  const quotedKinds = kindsFor(quoted, runtimeAgentFile("orchestrator.rs"));
  assert.ok(quotedKinds.has("sql_dialect_cluster"), `string marker must not mask: ${[...quotedKinds]}`);
});

test("bypass 1: a real cfg(test) module is still masked", () => {
  const masked = `
pub fn live(path: &str) -> u8 { path.len() as u8 }

#[cfg(test)]
mod tests {
    fn rank_by_dialect(path: &str) -> u8 {
        let lower = path.to_ascii_lowercase();
        if lower.contains("sqlite") || lower.contains("postgres") { 4 } else { 0 }
    }
}
`;
  const kinds = kindsFor(masked, runtimeAgentFile("orchestrator.rs"));
  assert.equal(kinds.size, 0, [...kinds].join(","));
});

test("bypass 2: scanning zero production files fails instead of reporting ok", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "packet-boundary-empty-"));
  try {
    writeTree(root, { "README.md": "no rust here\n" });
    const result = runPacketGeneralizationBoundaryCheck(root);
    assert.notEqual(result.exitCode, 0, result.stdout);
    assert.match(result.stderr, /scanned 0 production packet files/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("widened globs reach production packet paths outside codestory-agent", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "packet-boundary-globs-"));
  try {
    writeTree(root, {
      "crates/codestory-agent/src/packet_plan.rs": "pub fn plan() {}\n",
      "crates/codestory-retrieval/src/query_features.rs":
        `pub fn stem_rank(s: &str) -> u8 { match s { "cli" | "router" | "handler" | "main" => 1, _ => 0 } }\n`,
    });
    const result = runPacketGeneralizationBoundaryCheck(root);
    assert.equal(result.exitCode, 1, result.stdout);
    assert.match(result.stderr, /filename_stem_cluster/);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("current production head must pass the strengthened boundary checker", () => {
  const result = runPacketGeneralizationBoundaryCheck(repositoryRoot);
  assert.equal(
    result.exitCode,
    0,
    `r3 production head must pass the boundary checker: ${result.stderr}\n${JSON.stringify(result.findings, null, 2)}`,
  );
  assert.equal(result.findings.length, 0);
  assert.ok(DELETED_TAXONOMY_APIS.length > 0, "banlist must remain non-empty");
  assert.ok(DELETED_DOMAIN_EVIDENCE_ROLES.length > 0);
  assert.ok(DELETED_HOLDOUT_PROBE_SPELLINGS.length > 0);
  assert.ok(DELETED_PROBE_TABLE_APIS.length > 0);
});
