# AST-First Retrieval Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove production benchmark overfit, unify language support claims, expose unresolved sidecar evidence, clarify `files` counts, and pin verification for the AST-first retrieval branch.

**Architecture:** Keep the product path generic: production packet retrieval must only use graph, sidecar, semantic, and generic source-shape evidence. Move shared language claim metadata into `codestory-contracts` so workspace discovery, indexer profiles, runtime semantic docs, CLI `files`, and docs cannot drift. Preserve benchmark-family knowledge only in benchmark/eval surfaces, and record unresolved sidecar candidates as packet diagnostics rather than silent success.

**Tech Stack:** Rust 2024 workspace, `serde`, `specta`, tree-sitter-based indexer, CodeStory runtime/CLI crates, Node lint script, Cargo tests.

---

## Scope Check

This remediation touches several subsystems, but they are not independent product features. Execute as one master plan with separate commit-sized tasks:

1. Product packet overfit removal.
2. Shared language-support registry.
3. Registry consumer wiring and drift tests.
4. Sidecar packet diagnostics.
5. `files` count semantics.
6. Docs and final gates.

Do not start the dynamic parser loading idea here. That is a separate architecture project.

## File Structure

Create:

- `crates/codestory-contracts/src/language_support.rs` - shared language support metadata, extension lookup, language lookup, labels, and path lookup.
- `docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md` - this plan.

Modify:

- `crates/codestory-contracts/src/lib.rs` - export `language_support`.
- `crates/codestory-contracts/src/api/dto.rs` - add explicit filtered/visible counts and packet sidecar diagnostic DTOs if trace-level structured diagnostics are chosen.
- `crates/codestory-indexer/src/lib.rs` - delegate support profile functions to `codestory-contracts`; keep parser construction local.
- `crates/codestory-workspace/src/lib.rs` - consume the shared registry for source language matching where it fits existing `Language` routing.
- `crates/codestory-runtime/src/semantic_doc_text.rs` - use shared registry for semantic doc language labels.
- `crates/codestory-runtime/src/lib.rs` - use shared registry labels for `files` summaries and compute filtered/visible file counts.
- `crates/codestory-runtime/src/agent/orchestrator.rs` - remove default-on exact-family steering and make unresolved sidecar diagnostics block sufficiency when evidence is unusable.
- `crates/codestory-runtime/src/agent/retrieval_primary.rs` - record packet sidecar per-query diagnostics.
- `crates/codestory-runtime/src/agent/packet_search.rs` - propagate sidecar packet diagnostics from retrieval-primary to packet callers.
- `crates/codestory-cli/src/main.rs` - clarify `files` markdown count labels.
- `crates/codestory-cli/tests/cli_golden_path.rs` - assert JSON/markdown count semantics and language support labels.
- `scripts/lint-retrieval-generalization.mjs` - ban the newly reviewed benchmark-family literals in production code.
- `docs/architecture/language-support.md` - update source-of-truth wording and receiver resolution limits.
- `docs/review-action-plan.md` - keep the supersession note and, if needed, point to the implemented remediation.
- `docs/testing/codestory-e2e-stats-log.md` - append final e2e stats before commit or merge.

## Task 1: Add The Generalization Guard And Remove Production Exact-Family Steering

**Files:**
- Modify: `scripts/lint-retrieval-generalization.mjs`
- Modify: `crates/codestory-runtime/src/agent/orchestrator.rs`
- Test: `crates/codestory-runtime/src/agent/orchestrator.rs`

- [ ] **Step 1: Add the benchmark-family literals to the lint guard**

In `scripts/lint-retrieval-generalization.mjs`, add these entries to `bannedPatterns` near the other benchmark/repo-specific names:

```js
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
```

- [ ] **Step 2: Run the lint to prove the current branch fails**

Run:

```powershell
node scripts/lint-retrieval-generalization.mjs
```

Expected before code removal: FAIL with banned pattern hits in `crates/codestory-runtime/src/agent/orchestrator.rs`.

- [ ] **Step 3: Remove the default-on steering flag**

In `crates/codestory-runtime/src/agent/orchestrator.rs`, delete:

```rust
const PACKET_EXACT_FAMILY_STEERING_ENV: &str = "CODESTORY_PACKET_EXACT_FAMILY_STEERING";

#[cfg(test)]
thread_local! {
    static PACKET_EXACT_FAMILY_STEERING_TEST_OVERRIDE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

fn packet_exact_family_steering_enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = PACKET_EXACT_FAMILY_STEERING_TEST_OVERRIDE.with(std::cell::Cell::get) {
        return enabled;
    }

    std::env::var(PACKET_EXACT_FAMILY_STEERING_ENV)
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(true)
}
```

- [ ] **Step 4: Remove the production call block**

In `agent_packet`, replace this block:

```rust
    maybe_append_sql_schema_file_citations(&project_root, &question, &mut answer);
    if packet_exact_family_steering_enabled() {
        maybe_append_chinook_sql_schema_file_citations(&project_root, &question, &mut answer);
        maybe_append_mdn_form_validation_file_citations(&project_root, &question, &mut answer);
        maybe_append_okio_buffer_flow_file_citations(&project_root, &question, &mut answer);
        maybe_append_monolog_record_flow_file_citations(&project_root, &question, &mut answer);
        maybe_append_alamofire_request_flow_file_citations(&project_root, &question, &mut answer);
    } else {
        answer
            .retrieval_trace
            .annotations
            .push("packet_exact_family_steering=false static_family_citations=skipped".into());
    }
```

with:

```rust
    maybe_append_sql_schema_file_citations(&project_root, &question, &mut answer);
```

- [ ] **Step 5: Delete exact-family static citation helpers and exact-family source claim helpers from production**

Use this command to list every symbol that must be deleted or moved to eval-only code:

```powershell
rg -n "chinook|mdn|okio|monolog|alamofire|packet_exact_family_steering|PACKET_EXACT_FAMILY_STEERING" crates\codestory-runtime\src\agent\orchestrator.rs
```

Delete production functions whose names include:

```text
packet_terms_indicate_chinook_sql_schema_flow
push_chinook_sql_schema_symbol_probe_queries
packet_terms_indicate_mdn_form_validation_flow
push_mdn_form_validation_symbol_probe_queries
packet_terms_indicate_okio_buffer_flow
push_okio_buffer_flow_symbol_probe_queries
packet_terms_indicate_monolog_record_flow
push_monolog_record_flow_symbol_probe_queries
packet_terms_indicate_alamofire_request_flow
push_alamofire_request_flow_symbol_probe_queries
packet_chinook_sql_schema_flow_claims
packet_mdn_form_validation_flow_claims
packet_okio_buffer_flow_claims
packet_monolog_record_flow_claims
packet_alamofire_request_flow_claims
maybe_append_chinook_sql_schema_file_citations
maybe_append_mdn_form_validation_file_citations
maybe_append_okio_buffer_flow_file_citations
maybe_append_monolog_record_flow_file_citations
maybe_append_alamofire_request_flow_file_citations
```

Also remove any tests whose purpose is to prove those exact-family helpers work. Keep generic SQL schema tests.

- [ ] **Step 6: Verify no exact-family literals remain in production `orchestrator.rs`**

Run:

```powershell
rg -n "chinook|mdn|okio|monolog|alamofire|packet_exact_family_steering|PACKET_EXACT_FAMILY_STEERING" crates\codestory-runtime\src\agent\orchestrator.rs
```

Expected: no output from production code. Test-only benchmark task strings may remain only if they are moved to `crates/codestory-runtime/src/agent/eval_probes.rs` or benchmark manifests before this check is run against production slices.

- [ ] **Step 7: Run targeted runtime tests**

Run:

```powershell
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Run the lint again**

Run:

```powershell
node scripts/lint-retrieval-generalization.mjs
```

Expected: PASS with output like:

```text
lint-retrieval-generalization: ok
```

- [ ] **Step 9: Commit**

```powershell
git add scripts/lint-retrieval-generalization.mjs crates/codestory-runtime/src/agent/orchestrator.rs
git commit -m "remove packet benchmark steering"
```

## Task 2: Create The Shared Language Support Registry

**Files:**
- Create: `crates/codestory-contracts/src/language_support.rs`
- Modify: `crates/codestory-contracts/src/lib.rs`
- Modify: `crates/codestory-indexer/src/lib.rs`

- [ ] **Step 1: Create `language_support.rs`**

Create `crates/codestory-contracts/src/language_support.rs` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageSupportMode {
    ParserBackedGraph,
    StructuralCollector,
}

impl LanguageSupportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParserBackedGraph => "parser_backed_graph",
            Self::StructuralCollector => "structural_collector",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageEvidenceTier {
    GraphFidelity,
    StructuralOnly,
}

impl LanguageEvidenceTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GraphFidelity => "graph_fidelity",
            Self::StructuralOnly => "structural_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageSupportProfile {
    pub language_name: &'static str,
    pub extensions: &'static [&'static str],
    pub support_mode: LanguageSupportMode,
    pub evidence_tier: LanguageEvidenceTier,
    pub claim_label: &'static str,
}

const PARSER_BACKED_GRAPH: &str = "parser-backed graph, fidelity-gated";
const STRUCTURAL_COLLECTOR: &str = "structural collector only";

pub const LANGUAGE_SUPPORT_PROFILES: &[LanguageSupportProfile] = &[
    parser_profile("python", &["py", "pyi"]),
    parser_profile("java", &["java"]),
    parser_profile("rust", &["rs"]),
    parser_profile("javascript", &["js", "jsx", "mjs", "cjs"]),
    parser_profile("typescript", &["ts", "tsx", "mts", "cts"]),
    parser_profile("cpp", &["cpp", "cc", "cxx", "hpp", "hh", "hxx"]),
    parser_profile("c", &["c", "h"]),
    parser_profile("go", &["go"]),
    parser_profile("ruby", &["rb"]),
    parser_profile("php", &["php"]),
    parser_profile("csharp", &["cs", "cshtml"]),
    parser_profile("kotlin", &["kt", "kts"]),
    parser_profile("swift", &["swift"]),
    parser_profile("dart", &["dart"]),
    parser_profile("bash", &["sh", "bash"]),
    structural_profile("html", &["html", "htm"]),
    structural_profile("css", &["css"]),
    structural_profile("sql", &["sql"]),
];

const fn parser_profile(
    language_name: &'static str,
    extensions: &'static [&'static str],
) -> LanguageSupportProfile {
    LanguageSupportProfile {
        language_name,
        extensions,
        support_mode: LanguageSupportMode::ParserBackedGraph,
        evidence_tier: LanguageEvidenceTier::GraphFidelity,
        claim_label: PARSER_BACKED_GRAPH,
    }
}

const fn structural_profile(
    language_name: &'static str,
    extensions: &'static [&'static str],
) -> LanguageSupportProfile {
    LanguageSupportProfile {
        language_name,
        extensions,
        support_mode: LanguageSupportMode::StructuralCollector,
        evidence_tier: LanguageEvidenceTier::StructuralOnly,
        claim_label: STRUCTURAL_COLLECTOR,
    }
}

pub fn normalize_extension(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

pub fn language_support_profile_for_ext(ext: &str) -> Option<&'static LanguageSupportProfile> {
    let ext = normalize_extension(ext);
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.extensions.iter().any(|candidate| *candidate == ext))
}

pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<&'static LanguageSupportProfile> {
    let language_name = language_name.trim().to_ascii_lowercase();
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .find(|profile| profile.language_name == language_name)
}

pub fn language_name_for_path(path: Option<&str>) -> Option<&'static str> {
    let ext = path?
        .rsplit('.')
        .next()?
        .trim_start_matches('.');
    language_support_profile_for_ext(ext).map(|profile| profile.language_name)
}

pub fn supported_extensions() -> impl Iterator<Item = &'static str> {
    LANGUAGE_SUPPORT_PROFILES
        .iter()
        .flat_map(|profile| profile.extensions.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn profile_lookup_covers_claimed_parser_and_structural_languages() {
        assert_eq!(
            language_support_profile_for_ext("kt")
                .expect("kotlin profile")
                .language_name,
            "kotlin"
        );
        assert_eq!(
            language_support_profile_for_ext(".swift")
                .expect("swift profile")
                .support_mode,
            LanguageSupportMode::ParserBackedGraph
        );
        assert_eq!(
            language_support_profile_for_ext("html")
                .expect("html profile")
                .evidence_tier,
            LanguageEvidenceTier::StructuralOnly
        );
        assert_eq!(
            language_name_for_path(Some("src/app/Program.cshtml")),
            Some("csharp")
        );
    }

    #[test]
    fn profile_extensions_are_unique() {
        let mut seen = HashSet::new();
        for extension in supported_extensions() {
            assert!(
                seen.insert(extension),
                "extension should have exactly one owner: {extension}"
            );
        }
    }
}
```

- [ ] **Step 2: Export the module**

In `crates/codestory-contracts/src/lib.rs`, add:

```rust
pub mod language_support;
```

- [ ] **Step 3: Replace indexer-local support types with contract re-exports**

In `crates/codestory-indexer/src/lib.rs`, replace the local `LanguageSupportMode`, `LanguageEvidenceTier`, and `LanguageSupportProfile` definitions with:

```rust
pub use codestory_contracts::language_support::{
    LanguageEvidenceTier, LanguageSupportMode, LanguageSupportProfile,
};
```

- [ ] **Step 4: Delegate indexer support profile functions to contracts**

Replace the bodies of `language_support_profile_for_ext` and `language_support_profile_for_language_name` in `crates/codestory-indexer/src/lib.rs` with:

```rust
pub fn language_support_profile_for_ext(ext: &str) -> Option<LanguageSupportProfile> {
    codestory_contracts::language_support::language_support_profile_for_ext(ext).copied()
}

pub fn language_support_profile_for_language_name(
    language_name: &str,
) -> Option<LanguageSupportProfile> {
    codestory_contracts::language_support::language_support_profile_for_language_name(language_name)
        .copied()
}
```

Delete the old local helper functions:

```text
normalize_extension
parser_graph_fidelity_profile
structural_profile
```

If `normalize_extension` is still used by parser construction, replace those local calls with:

```rust
let ext = codestory_contracts::language_support::normalize_extension(ext);
```

- [ ] **Step 5: Run contract and indexer tests**

Run:

```powershell
cargo test -p codestory-contracts language_support -- --nocapture
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add crates/codestory-contracts/src/lib.rs crates/codestory-contracts/src/language_support.rs crates/codestory-indexer/src/lib.rs
git commit -m "centralize language support registry"
```

## Task 3: Wire Runtime, Workspace, Semantic Docs, And Drift Tests To The Registry

**Files:**
- Modify: `crates/codestory-workspace/src/lib.rs`
- Modify: `crates/codestory-runtime/src/semantic_doc_text.rs`
- Modify: `crates/codestory-runtime/src/lib.rs`
- Modify: `crates/codestory-cli/tests/onboarding_contracts.rs`
- Modify: `crates/codestory-cli/tests/cli_golden_path.rs`
- Modify: `docs/architecture/language-support.md`

- [ ] **Step 1: Update semantic doc language lookup**

In `crates/codestory-runtime/src/semantic_doc_text.rs`, replace `semantic_doc_language_from_path` with:

```rust
pub(crate) fn semantic_doc_language_from_path(path: Option<&str>) -> Option<&'static str> {
    codestory_contracts::language_support::language_name_for_path(path)
}
```

- [ ] **Step 2: Update semantic doc tests**

In the existing semantic doc language test near the bottom of `crates/codestory-runtime/src/semantic_doc_text.rs`, include these cases:

```rust
let cases = [
    ("main.c", Some("c")),
    ("main.cpp", Some("cpp")),
    ("Main.java", Some("java")),
    ("main.js", Some("javascript")),
    ("main.py", Some("python")),
    ("main.rs", Some("rust")),
    ("main.ts", Some("typescript")),
    ("main.go", Some("go")),
    ("main.rb", Some("ruby")),
    ("main.php", Some("php")),
    ("Program.cs", Some("csharp")),
    ("View.cshtml", Some("csharp")),
    ("Main.kt", Some("kotlin")),
    ("Main.swift", Some("swift")),
    ("main.dart", Some("dart")),
    ("script.sh", Some("bash")),
    ("index.html", Some("html")),
    ("style.css", Some("css")),
    ("schema.sql", Some("sql")),
    ("README.md", None),
];
for (path, language) in cases {
    assert_eq!(semantic_doc_language_from_path(Some(path)), language);
}
```

- [ ] **Step 3: Use registry labels in runtime file summaries**

In `crates/codestory-runtime/src/lib.rs`, import contract language support instead of indexer support types:

```rust
use codestory_contracts::language_support::language_support_profile_for_language_name;
```

Then replace `language_support_summary_for_language`, `language_support_mode_label`, and `language_evidence_tier_label` with:

```rust
struct LanguageSupportSummary {
    support_mode: String,
    evidence_tier: String,
    claim_label: String,
}

fn language_support_summary_for_language(language: &str) -> LanguageSupportSummary {
    language_support_profile_for_language_name(language)
        .map(|profile| LanguageSupportSummary {
            support_mode: profile.support_mode.as_str().to_string(),
            evidence_tier: profile.evidence_tier.as_str().to_string(),
            claim_label: profile.claim_label.to_string(),
        })
        .unwrap_or_else(|| LanguageSupportSummary {
            support_mode: "unknown".to_string(),
            evidence_tier: "unknown".to_string(),
            claim_label: "no support claim recorded".to_string(),
        })
}
```

- [ ] **Step 4: Keep workspace routing aligned without changing parser ownership**

In `crates/codestory-workspace/src/lib.rs`, add this helper near `matches_source_group_language`:

```rust
fn registry_language_for_path(path: &Path) -> Option<&'static str> {
    path.to_str()
        .and_then(|path| codestory_contracts::language_support::language_name_for_path(Some(path)))
}
```

Then add a test in the existing test module that proves registry coverage includes every extension `matches_source_group_language` claims:

```rust
#[test]
fn workspace_supported_source_extensions_have_registry_profiles() {
    let claimed = [
        "rs", "py", "pyi", "java", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts",
        "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx", "go", "rb", "php", "cs",
        "cshtml", "kt", "kts", "swift", "dart", "sql", "html", "htm", "css", "sh", "bash",
    ];
    for extension in claimed {
        assert!(
            codestory_contracts::language_support::language_support_profile_for_ext(extension)
                .is_some(),
            "workspace source extension should have registry profile: {extension}"
        );
    }
}
```

Do not add Lua, PowerShell, Sass, Less, Vue, Astro, or Svelte to the shared first-class registry unless the implementation also defines the correct support claim for those surfaces in this same task.

- [ ] **Step 5: Update docs to name the new source of truth**

In `docs/architecture/language-support.md`, replace the old source-of-truth paragraph with:

```markdown
The source of truth for extension ownership, stored-language names, support
modes, evidence tiers, and claim labels is
`crates/codestory-contracts/src/language_support.rs`. The indexer maps those
shared support profiles to parser/rule construction in `get_language_for_ext`;
workspace discovery and runtime semantic document labels consume the same
registry so support claims cannot drift quietly across crates.
```

- [ ] **Step 6: Update onboarding doc contract checks**

In `crates/codestory-cli/tests/onboarding_contracts.rs`, update the language support doc check so it requires:

```rust
for required in [
    "crates/codestory-contracts/src/language_support.rs",
    "language_support_profile_for_ext",
    "language_support_profile_for_language_name",
    "get_language_for_ext",
] {
    assert!(
        language_support.contains(required),
        "language support docs should mention `{required}`"
    );
}
```

- [ ] **Step 7: Run focused tests**

Run:

```powershell
cargo test -p codestory-runtime semantic_doc_language -- --nocapture
cargo test -p codestory-workspace workspace_supported_source_extensions_have_registry_profiles -- --nocapture
cargo test -p codestory-cli --test onboarding_contracts language_support -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```powershell
git add crates/codestory-workspace/src/lib.rs crates/codestory-runtime/src/semantic_doc_text.rs crates/codestory-runtime/src/lib.rs crates/codestory-cli/tests/onboarding_contracts.rs docs/architecture/language-support.md
git commit -m "wire language support registry"
```

## Task 4: Add Packet Sidecar Diagnostics And Sufficiency Gaps

**Files:**
- Modify: `crates/codestory-contracts/src/api/dto.rs`
- Modify: `crates/codestory-runtime/src/agent/retrieval_primary.rs`
- Modify: `crates/codestory-runtime/src/agent/packet_search.rs`
- Modify: `crates/codestory-runtime/src/agent/orchestrator.rs`

- [ ] **Step 1: Add a packet sidecar diagnostic DTO**

In `crates/codestory-contracts/src/api/dto.rs`, add near `RetrievalShadowDto`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct PacketSidecarQueryDiagnosticDto {
    pub query: String,
    pub retrieval_mode: String,
    pub candidate_count: u32,
    pub resolved_hit_count: u32,
    pub unresolved_candidate_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}
```

Then add this field to `AgentRetrievalTraceDto`:

```rust
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packet_sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
```

Update every test fixture that builds `AgentRetrievalTraceDto` to include:

```rust
                packet_sidecar_diagnostics: Vec::new(),
```

- [ ] **Step 2: Return diagnostics from sidecar packet batch**

In `crates/codestory-runtime/src/agent/retrieval_primary.rs`, import the DTO:

```rust
use codestory_contracts::api::PacketSidecarQueryDiagnosticDto;
```

Add:

```rust
pub(crate) struct SidecarPacketBatchOutcome {
    pub results: Vec<(String, Vec<SearchHit>)>,
    pub diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}

fn packet_sidecar_query_diagnostic(
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
) -> PacketSidecarQueryDiagnosticDto {
    let candidate_count = query_result.hits.len();
    let resolved_hit_count = resolved_hits.len();
    let unresolved_candidate_count = candidate_count.saturating_sub(resolved_hit_count);
    PacketSidecarQueryDiagnosticDto {
        query: query_result.query.clone(),
        retrieval_mode: query_result.trace.retrieval_mode.clone(),
        candidate_count: u32::try_from(candidate_count).unwrap_or(u32::MAX),
        resolved_hit_count: u32::try_from(resolved_hit_count).unwrap_or(u32::MAX),
        unresolved_candidate_count: u32::try_from(unresolved_candidate_count).unwrap_or(u32::MAX),
        diagnostic: (unresolved_candidate_count > 0)
            .then(|| "sidecar candidates did not all resolve to indexed symbols".to_string()),
    }
}
```

Change `search_sidecar_packet_batch` to return `Result<SidecarPacketBatchOutcome, ApiError>`. Inside the loop, push diagnostics:

```rust
        let diagnostic = packet_sidecar_query_diagnostic(&query_result, &resolved_hits);
        diagnostics.push(diagnostic);
        results.push((query.clone(), resolved_hits));
```

Return:

```rust
    Ok(SidecarPacketBatchOutcome {
        results,
        diagnostics,
    })
```

- [ ] **Step 3: Preserve diagnostics in packet search callers**

In `crates/codestory-runtime/src/agent/packet_search.rs`, change `SemanticHybridBatchOutcome` to:

```rust
pub(crate) struct SemanticHybridBatchOutcome {
    pub results: Vec<(String, Vec<HybridSearchScoredHit>)>,
    pub fallbacks: Vec<SemanticFallbackRecordDto>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}
```

Also add a lexical batch outcome:

```rust
pub(crate) struct LexicalBatchOutcome {
    pub results: Vec<(String, Vec<SearchHit>)>,
    pub sidecar_diagnostics: Vec<PacketSidecarQueryDiagnosticDto>,
}
```

Update `search_lexical_hybrid_batch` to return `Result<LexicalBatchOutcome, ApiError>` and convert successful sidecar calls with:

```rust
                Ok(outcome) => {
                    return Ok(LexicalBatchOutcome {
                        results: outcome.results,
                        sidecar_diagnostics: outcome.diagnostics,
                    });
                }
```

Update `search_semantic_hybrid_batch` success path to fill `sidecar_diagnostics: outcome.diagnostics`.

- [ ] **Step 4: Attach diagnostics to packet trace**

In `crates/codestory-runtime/src/agent/orchestrator.rs`, wherever packet batch outcomes are consumed, append diagnostics:

```rust
answer
    .retrieval_trace
    .packet_sidecar_diagnostics
    .extend(outcome.sidecar_diagnostics);
```

For semantic outcomes:

```rust
answer
    .retrieval_trace
    .packet_sidecar_diagnostics
    .extend(outcome.sidecar_diagnostics);
```

- [ ] **Step 5: Make unresolved-only diagnostics block sufficiency**

In `build_packet_sufficiency_with_extra`, add:

```rust
    let unresolved_sidecar_queries = answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.candidate_count > 0
                && diagnostic.resolved_hit_count == 0
                && diagnostic.unresolved_candidate_count > 0
        })
        .map(|diagnostic| diagnostic.query.clone())
        .collect::<Vec<_>>();
```

Include `|| !unresolved_sidecar_queries.is_empty()` in the `Partial` status condition.

Add this gap:

```rust
    if !unresolved_sidecar_queries.is_empty() {
        gaps.push(format!(
            "{:?} packet had sidecar candidates that could not resolve to indexed symbols for: {}.",
            task_class,
            unresolved_sidecar_queries.join(", ")
        ));
    }
```

- [ ] **Step 6: Update sidecar packet tests**

Replace `packet_batch_allows_empty_and_unresolved_full_mode_queries` in `retrieval_primary.rs` with tests that keep empty queries allowed but assert unresolved diagnostics:

```rust
#[test]
fn packet_sidecar_query_diagnostic_distinguishes_empty_and_unresolved_candidates() {
    use codestory_retrieval::{CandidateSource, classify_query};

    let empty_full = QueryResult {
        query: "unlikely symbol".into(),
        features: classify_query("unlikely symbol"),
        hits: Vec::new(),
        trace: QueryTrace {
            retrieval_mode: "full".into(),
            degraded_reason: None,
            total_budget_ms: 500,
            elapsed_ms: 1,
            cancel_reason: None,
            cache_hit: false,
            stages: Vec::new(),
        },
    };
    let empty_diagnostic = packet_sidecar_query_diagnostic(&empty_full, &[]);
    assert_eq!(empty_diagnostic.candidate_count, 0);
    assert_eq!(empty_diagnostic.resolved_hit_count, 0);
    assert_eq!(empty_diagnostic.unresolved_candidate_count, 0);
    assert!(empty_diagnostic.diagnostic.is_none());

    let unresolved = QueryResult {
        query: "handler".into(),
        features: classify_query("handler"),
        hits: vec![CandidateHit::with_source(
            "semantic:handler",
            Some("handler".into()),
            0.5,
            CandidateSource::Qdrant,
        )],
        trace: QueryTrace {
            retrieval_mode: "full".into(),
            degraded_reason: None,
            total_budget_ms: 500,
            elapsed_ms: 1,
            cancel_reason: None,
            cache_hit: false,
            stages: Vec::new(),
        },
    };
    let unresolved_diagnostic = packet_sidecar_query_diagnostic(&unresolved, &[]);
    assert_eq!(unresolved_diagnostic.candidate_count, 1);
    assert_eq!(unresolved_diagnostic.resolved_hit_count, 0);
    assert_eq!(unresolved_diagnostic.unresolved_candidate_count, 1);
    assert!(
        unresolved_diagnostic
            .diagnostic
            .as_deref()
            .is_some_and(|value| value.contains("did not all resolve"))
    );
}
```

- [ ] **Step 7: Add a sufficiency regression**

In `orchestrator.rs` tests, create a packet fixture with one unresolved sidecar diagnostic and enough citations to otherwise pass. Assert status is `Partial` and gaps mention sidecar unresolved candidates:

```rust
#[test]
fn packet_sufficiency_treats_unresolved_sidecar_candidates_as_gap() {
    let question = "Explain how requests flow through dispatch and adapters.";
    let (mut answer, _) = build_sufficient_packet_fixture(
        question,
        PacketTaskClassDto::DataFlow,
        vec![
            packet_citation("dispatchRequest", "src/core/dispatch.rs", 10, NodeKind::FUNCTION, 9.0),
            packet_citation("Adapter", "src/adapters/http.rs", 20, NodeKind::FUNCTION, 8.5),
            packet_citation("Request", "src/request.rs", 30, NodeKind::CLASS, 8.0),
        ],
    );
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .push(PacketSidecarQueryDiagnosticDto {
            query: "adapter dispatch".to_string(),
            retrieval_mode: "full".to_string(),
            candidate_count: 2,
            resolved_hit_count: 0,
            unresolved_candidate_count: 2,
            diagnostic: Some("sidecar candidates did not all resolve to indexed symbols".to_string()),
        });
    let budget = PacketBudgetDto {
        requested: PacketBudgetModeDto::Compact,
        limits: packet_budget_limits(PacketBudgetModeDto::Compact),
        used: packet_budget_usage(&answer),
        truncated: false,
        omitted_sections: Vec::new(),
        omitted_citations: 0,
        omitted_graph_edges: 0,
        omitted_claims: 0,
        omitted_sections_detail: Vec::new(),
    };
    let sufficiency = build_packet_sufficiency(
        packet_fixture_project_root(),
        question,
        PacketTaskClassDto::DataFlow,
        &answer,
        &budget,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    assert!(
        sufficiency
            .gaps
            .iter()
            .any(|gap| gap.contains("sidecar candidates")),
        "unresolved sidecar diagnostics should appear as a sufficiency gap: {sufficiency:?}"
    );
}
```

- [ ] **Step 8: Run focused tests**

Run:

```powershell
cargo test -p codestory-runtime packet_sidecar_query_diagnostic -- --nocapture
cargo test -p codestory-runtime packet_sufficiency_treats_unresolved_sidecar_candidates_as_gap -- --nocapture
```

Expected: PASS.

- [ ] **Step 9: Commit**

```powershell
git add crates/codestory-contracts/src/api/dto.rs crates/codestory-runtime/src/agent/retrieval_primary.rs crates/codestory-runtime/src/agent/packet_search.rs crates/codestory-runtime/src/agent/orchestrator.rs
git commit -m "surface packet sidecar gaps"
```

## Task 5: Clarify `files` Whole-Index, Filtered, And Visible Counts

**Files:**
- Modify: `crates/codestory-contracts/src/api/dto.rs`
- Modify: `crates/codestory-runtime/src/lib.rs`
- Modify: `crates/codestory-cli/src/main.rs`
- Modify: `crates/codestory-cli/tests/cli_golden_path.rs`

- [ ] **Step 1: Add explicit count fields**

In `IndexedFilesSummaryDto`, add:

```rust
    #[serde(default)]
    pub filtered_file_count: u32,
    #[serde(default)]
    pub visible_file_count: u32,
```

Keep `file_count` and `indexed_file_count` as whole-index fields.

- [ ] **Step 2: Compute filtered and visible counts**

In `AppController::indexed_files`, after collecting `visible` and before truncating, add:

```rust
        let filtered_file_count = visible.len().min(u32::MAX as usize) as u32;
```

After truncation, add:

```rust
        let visible_file_count = visible.len().min(u32::MAX as usize) as u32;
```

When building `IndexedFilesSummaryDto`, set:

```rust
                filtered_file_count,
                visible_file_count,
```

- [ ] **Step 3: Clarify markdown summary labels**

In `render_files_summary`, replace the first summary line with:

```rust
    let _ = writeln!(
        markdown,
        "- index: {status}; whole index files: {}; indexed: {}; incomplete: {}; error files: {}; filtered files: {}; visible rows: {}; truncated: {}",
        output.summary.file_count,
        output.summary.indexed_file_count,
        output.summary.incomplete_file_count,
        output.summary.error_file_count,
        output.summary.filtered_file_count,
        output.summary.visible_file_count,
        output.summary.truncated
    );
```

- [ ] **Step 4: Update golden path JSON assertions**

In `assert_files_and_affected_read_existing_cache`, after the first `files` JSON call, add:

```rust
    assert!(
        files["summary"]["file_count"].as_u64().is_some_and(|count| count >= 1),
        "files JSON should keep whole-index file_count: {files:#}"
    );
    assert!(
        files["summary"]["filtered_file_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "files JSON should include filtered_file_count: {files:#}"
    );
    assert_eq!(
        files["summary"]["visible_file_count"].as_u64(),
        files["files"].as_array().map(|items| items.len() as u64),
        "visible_file_count should match returned rows: {files:#}"
    );
```

In the markdown assertion, add:

```rust
            && files_markdown.contains("whole index files:")
            && files_markdown.contains("filtered files:")
            && files_markdown.contains("visible rows:")
```

- [ ] **Step 5: Run CLI golden test**

Run:

```powershell
cargo test -p codestory-cli --test cli_golden_path assert_files_and_affected_read_existing_cache -- --nocapture
```

Expected: PASS if the test is directly addressable. If the function is not a test, run the nearest containing golden-path test that calls it.

- [ ] **Step 6: Commit**

```powershell
git add crates/codestory-contracts/src/api/dto.rs crates/codestory-runtime/src/lib.rs crates/codestory-cli/src/main.rs crates/codestory-cli/tests/cli_golden_path.rs
git commit -m "clarify files count semantics"
```

## Task 6: Document Receiver Resolution Boundaries And Update Review Status

**Files:**
- Modify: `docs/architecture/language-support.md`
- Modify: `docs/review-action-plan.md`
- Modify: `docs/specs/review-remediation-ast-first-retrieval/validation.md`

- [ ] **Step 1: Add receiver resolution boundary text**

In `docs/architecture/language-support.md`, add this paragraph after the current matrix:

```markdown
Typed receiver-call support is claimed only for the fixture-backed cases named
in the indexer regression suites. Current support covers simple local owner
qualified calls where tests prove the behavior. Cross-package receiver lookup,
polymorphic dispatch, inheritance-heavy target selection, framework-handler
resolution, and declarative parameter extraction require separate fixtures and
cannot be used as product claims until those fixtures pass.
```

- [ ] **Step 2: Add the manual extraction replacement criteria**

In the expansion checklist, add:

```markdown
11. Before widening typed receiver-call claims, add same-file and cross-file
    fixtures for the target language. If implementation still uses signature
    string slicing, document that as a transitional boundary; prefer a
    tree-sitter-query or global-resolution-backed implementation for new
    claims.
```

- [ ] **Step 3: Update the old action plan status**

In `docs/review-action-plan.md`, keep the existing supersession note and add:

```markdown
The active remediation work is tracked in
`docs/specs/review-remediation-ast-first-retrieval/` and the execution plan is
`docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md`.
```

- [ ] **Step 4: Run doc contract tests**

Run:

```powershell
cargo test -p codestory-cli --test onboarding_contracts -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add docs/architecture/language-support.md docs/review-action-plan.md docs/specs/review-remediation-ast-first-retrieval/validation.md
git commit -m "document retrieval remediation boundaries"
```

## Task 7: Run Final Verification And Update E2E Stats

**Files:**
- Modify: `docs/testing/codestory-e2e-stats-log.md`

- [ ] **Step 1: Run formatting**

Run:

```powershell
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Run full check**

Run:

```powershell
cargo check --all-targets
```

Expected: PASS.

- [ ] **Step 3: Run generalization lint**

Run:

```powershell
node scripts/lint-retrieval-generalization.mjs
```

Expected: PASS.

- [ ] **Step 4: Run language fidelity binaries**

Run these as full test binaries, not filters:

```powershell
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
```

Expected: PASS.

- [ ] **Step 5: Run runtime and CLI targeted suites**

Run:

```powershell
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
cargo test -p codestory-cli --test cli_golden_path -- --nocapture
cargo test -p codestory-cli --test onboarding_contracts -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Build release CLI**

Run:

```powershell
cargo build --release -p codestory-cli
```

Expected: PASS.

- [ ] **Step 7: Run repo-scale e2e stats**

Run:

```powershell
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Expected: PASS and printed stats including `index_seconds`, `semantic_docs`, `error_count`, and `search_dir_unchanged`.

- [ ] **Step 8: Append stats log row**

Open `docs/testing/codestory-e2e-stats-log.md` and append a row using the exact stats printed by Step 7. Include the current branch or commit hash and the date `2026-06-13`.

- [ ] **Step 9: Run whitespace check**

Run:

```powershell
git diff --check
```

Expected: PASS with no output.

- [ ] **Step 10: Commit verification stats**

```powershell
git add docs/testing/codestory-e2e-stats-log.md
git commit -m "log remediation e2e stats"
```

## Task 8: Self-Review Before Merge Or Push

**Files:**
- Read: `docs/specs/review-remediation-ast-first-retrieval/requirements.md`
- Read: `docs/specs/review-remediation-ast-first-retrieval/validation.md`
- Read: `docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md`

- [ ] **Step 1: Confirm requirement coverage**

Run:

```powershell
$validator = $env:SPECIFICATION_ARCHITECT_TRACEABILITY_VALIDATOR
python "$validator" --path docs/specs/review-remediation-ast-first-retrieval --requirements requirements.md --tasks tasks.md --research research.md
```

Expected:

```text
{'total_criteria': 24, 'covered_criteria': 24, 'coverage_percentage': 100.0}
missing= []
invalid= []
```

- [ ] **Step 2: Confirm no production benchmark-family literals remain**

Run:

```powershell
rg -n "chinook|mdn|okio|monolog|alamofire|PACKET_EXACT_FAMILY_STEERING|packet_exact_family_steering" crates\codestory-cli\src crates\codestory-indexer\src crates\codestory-runtime\src crates\codestory-retrieval\src
```

Expected: no production hits. Hits in benchmark manifests, docs, tests, or eval-only modules are acceptable only when they are not scanned by `scripts/lint-retrieval-generalization.mjs`.

- [ ] **Step 3: Confirm changed files are intentional**

Run:

```powershell
git status --short
git diff --stat
```

Expected: only files from this plan are modified.

- [ ] **Step 4: Final commit if needed**

If there are uncommitted review-only fixes after the task commits:

```powershell
git add docs/specs/review-remediation-ast-first-retrieval docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md
git commit -m "plan ast retrieval remediation"
```

## Execution Notes

- Serialize Cargo commands. This repo contends on shared package and build locks when parallel Cargo runs overlap.
- Do not delete or revert unrelated user changes in the worktree.
- Keep benchmark-family knowledge out of production Rust. Eval-only code and benchmark manifests are the only acceptable homes.
- Do not claim dynamic parser loading as part of this remediation.
- Before a push or merge, run the release e2e stats gate and update `docs/testing/codestory-e2e-stats-log.md`.

## Self-Review

Spec coverage:

- Requirement 1 maps to Tasks 1 and 8.
- Requirement 2 maps to Tasks 2 and 3.
- Requirement 3 maps to Task 4.
- Requirement 4 maps to Task 5.
- Requirement 5 maps to Task 6.
- Requirement 6 maps to Tasks 7 and 8.

Placeholder scan:

- No unfinished-marker or open-ended implementation placeholders.
- Deletion steps name exact symbols and validation commands.
- Code-changing tasks include concrete snippets or exact removal targets.

Type consistency:

- Shared language profile names match current indexer/runtime names.
- `filtered_file_count` and `visible_file_count` are used consistently across DTO, runtime, CLI, and tests.
- `PacketSidecarQueryDiagnosticDto` is used consistently in DTOs, retrieval primary, packet search, orchestrator, and sufficiency tests.

Plan complete and saved to `docs/superpowers/plans/2026-06-13-ast-first-retrieval-remediation.md`. Two execution options:

1. Subagent-Driven (recommended) - dispatch a fresh subagent per task, review between tasks, fast iteration.

2. Inline Execution - execute tasks in this session using executing-plans, batch execution with checkpoints.
