import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  DELETED_TAXONOMY_APIS,
  decodeAsciiByteArrayLiterals,
  findBoundaryViolations,
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

test("current integration head is contaminated (gate: fail-on-contaminated-head)", () => {
  const result = runPacketGeneralizationBoundaryCheck(repositoryRoot);
  assert.equal(result.exitCode, 1, "contaminated production head must fail the boundary checker");
  assert.ok(result.findings.length > 0);
  const apis = new Set(
    result.findings.filter((f) => f.kind === "deleted_taxonomy_api").map((f) => f.detail),
  );
  assert.ok(apis.has("packet_flow_requirements_for_terms"));
  assert.ok(apis.has("packet_terms_indicate_hook_cache_flow"));
  assert.ok(
    DELETED_TAXONOMY_APIS.some((api) => apis.has(api)),
    "at least one deleted taxonomy API must still be present on contaminated head",
  );
  assert.ok(
    result.findings.some((f) => f.kind === "encoded_brand_literal"),
    "encoded SWR detector must be visible on contaminated head",
  );
});
