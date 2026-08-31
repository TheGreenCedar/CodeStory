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
