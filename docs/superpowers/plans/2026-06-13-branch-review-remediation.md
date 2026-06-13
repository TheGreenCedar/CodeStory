# Branch Review Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove branch-review blockers by making packet retrieval fail closed, support claims truthful, benchmark steering eval-only, language tests meaningful, and review evidence durable.

**Architecture:** Keep production runtime behavior generic and evidence-derived; move benchmark-family behavior behind explicit eval/test boundaries. Treat `codestory-contracts` language profiles as public claims and add invariants that force parser-backed claims to match live indexer routing. Keep documentation as durable operator guidance, with raw run notebooks out of canonical docs.

**Tech Stack:** Rust 2024 workspace, Cargo tests, Node.js benchmark/lint scripts, Markdown docs.

---

## File Structure

- Modify `crates/codestory-runtime/src/agent/retrieval_primary.rs`: batch sidecar candidate resolution must fail closed and preserve diagnostics.
- Modify `crates/codestory-runtime/src/agent/orchestrator.rs`: benchmark-family packet probes and canned source claims must be disabled in production by default or moved behind eval-only gates.
- Modify `crates/codestory-runtime/src/agent/eval_probes.rs`: expose a single runtime predicate for eval-only family steering if one is not already reusable.
- Modify `scripts/lint-retrieval-generalization.mjs`: forbid exact benchmark-family steering strings in production runtime files.
- Modify `crates/codestory-contracts/src/language_support.rs`: remove `.cshtml` from parser-backed C# claims unless real Razor parsing is implemented.
- Modify `crates/codestory-indexer/src/lib.rs`: replace spot-checked parser routing tests with registry-wide parser-backed routing invariants.
- Modify `crates/codestory-workspace/src/lib.rs`: keep workspace extension checks honest about public support profiles versus compatibility-only filters.
- Modify `crates/codestory-indexer/tests/import_resolution.rs`: split import extraction smoke from actual cross-file resolution assertions.
- Modify `crates/codestory-indexer/tests/tictactoe_language_coverage.rs`: require `NodeKind::METHOD` for class/interface members in first-class language fixtures.
- Modify `crates/codestory-runtime/src/lib.rs` and `crates/codestory-runtime/src/support.rs`: add bounded file-text reads for semantic doc construction.
- Modify `docs/testing/codestory-e2e-stats-log.md`: repair malformed phase metric rows and add a fresh HEAD row only after the ignored e2e gate runs.
- Modify `docs/testing/oss-language-corpus.md`: correct current edge count and clarify artifact integrity versus freshness proof.
- Modify `docs/architecture/language-support.md`: align registry ownership wording with the actual split between public support profiles and workspace compatibility filters.
- Modify `docs/architecture/retrieval-parser-compat-matrix.md`: remove references to missing local plan artifacts.
- Delete or shrink `docs/review-action-plan.md`: keep branch-local remediation history out of canonical docs.
- Shrink `docs/testing/language-expansion-ab-report.md`: preserve verdicts and reproduction commands; remove raw local run catalogs and transcript-like appendices.

---

### Task 1: Fail Closed On Packet Batch Sidecar Resolution Errors

**Files:**
- Modify: `crates/codestory-runtime/src/agent/retrieval_primary.rs`
- Test: `crates/codestory-runtime/src/agent/retrieval_primary.rs` unit tests or existing runtime tests near packet sidecar coverage

- [x] **Step 1: Write a failing regression test**

Add a test near existing packet sidecar tests that constructs a packet batch sidecar path where `run_sidecar_query` returns candidates but `resolve_sidecar_candidates_with_stats` fails. The assertion must require `search_sidecar_packet_batch_inner` to return `Err(ApiError)` whose message contains `sidecar retrieval rejected` or `candidate resolution failed`.

Use this expected shape:

```rust
#[test]
fn packet_batch_rejects_candidate_resolution_errors() {
    // Arrange a sidecar query result with at least one candidate that cannot
    // resolve to an indexed symbol.
    // Act: call the packet batch helper.
    // Assert: the result is Err and the error message preserves the failure.
}
```

- [x] **Step 2: Run the failing test**

Run:

```powershell
cargo test -p codestory-runtime packet_batch_rejects_candidate_resolution_errors -- --nocapture
```

Expected: FAIL before implementation because the current code uses `unwrap_or` and converts the resolution error to zero counts.

- [x] **Step 3: Replace the fail-open block**

In `search_sidecar_packet_batch_inner`, replace the `unwrap_or(SidecarCandidateResolutionOutcome { ... })` block with error propagation through `sidecar_retrieval_unavailable_error`.

Target implementation shape:

```rust
let resolution = resolve_sidecar_candidates_with_stats(controller, &query_result.hits, max_results)
    .map_err(|error| {
        sidecar_retrieval_unavailable_error(
            controller,
            format!(
                "sidecar retrieval rejected packet batch query `{query}`: candidate resolution failed: {error}"
            ),
        )
    })?;
```

- [x] **Step 4: Assert unresolved candidates still reject**

If no test already covers the batch path, add a second assertion that a full-mode query with non-empty sidecar candidates and zero resolved hits is rejected. Update `sidecar_packet_batch_rejection_reason` to inspect `resolved_hits` and `query_result.hits`.

Target implementation shape:

```rust
fn sidecar_packet_batch_rejection_reason(
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
) -> Option<String> {
    if !sidecar_mode_can_serve_primary(&query_result.trace.retrieval_mode) {
        return Some(format!(
            "sidecar retrieval mode `{}` is not eligible for packet batch results",
            query_result.trace.retrieval_mode
        ));
    }
    if !query_result.hits.is_empty() && resolved_hits.is_empty() {
        return Some("sidecar candidates did not resolve to indexed symbols".to_string());
    }
    None
}
```

- [x] **Step 5: Verify**

Run:

```powershell
cargo test -p codestory-runtime packet_sufficiency_treats_unresolved_sidecar_candidates_as_gap -- --nocapture
cargo test -p codestory-runtime packet_batch -- --nocapture
git diff --check origin/main...HEAD
```

Expected: all pass.

---

### Task 2: Make Language Support Claims Truthful And Invariant Checked

**Files:**
- Modify: `crates/codestory-contracts/src/language_support.rs`
- Modify: `crates/codestory-indexer/src/lib.rs`
- Modify: `crates/codestory-workspace/src/lib.rs`
- Modify: `docs/architecture/language-support.md`

- [x] **Step 1: Write the parser-backed routing invariant**

In `crates/codestory-indexer/src/lib.rs`, replace the current spot-check loop over only `["kt", "kts", "swift", "dart", "sh", "bash"]` with a registry-wide loop.

Use this assertion shape:

```rust
for profile in codestory_contracts::language_support::LANGUAGE_SUPPORT_PROFILES {
    if profile.support_mode == LanguageSupportMode::ParserBackedGraph {
        for ext in profile.extensions {
            assert!(
                get_language_for_ext(ext).is_some(),
                "parser-backed language {} extension {} must route into live indexing",
                profile.language_name,
                ext
            );
        }
    }
}
```

- [x] **Step 2: Run the invariant to confirm the current failure**

Run:

```powershell
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims -- --nocapture
```

Expected: FAIL on `csharp` extension `cshtml`.

- [x] **Step 3: Remove `.cshtml` from parser-backed C#**

In `crates/codestory-contracts/src/language_support.rs`, change:

```rust
parser_profile("csharp", &["cs", "cshtml"]),
```

to:

```rust
parser_profile("csharp", &["cs"]),
```

Update tests that currently expect `Program.cshtml` to return `Some("csharp")`; the truthful assertion is that `.cshtml` has no parser-backed public support profile until Razor support exists.

- [x] **Step 4: Preserve workspace compatibility if needed**

If workspace discovery still needs to include `.cshtml` as a source candidate, keep that behavior in `crates/codestory-workspace/src/lib.rs`, but do not require a public registry profile for `.cshtml` in `workspace_supported_source_extensions_have_registry_profiles`.

Use explicit compatibility-only coverage:

```rust
let compatibility_only = ["cshtml", "svelte", "vue", "astro", "lua", "ps1", "scss", "sass", "less"];
```

Then assert registry profiles only for public support extensions, and assert compatibility-only extensions are accepted by workspace discovery separately.

- [x] **Step 5: Update docs**

In `docs/architecture/language-support.md`, replace any claim that workspace discovery consumes the shared registry for all extensions with:

```markdown
The shared registry owns public support claims. Workspace discovery also carries compatibility-only filters for file types that can be scanned or grouped without being claimed as parser-backed language support.
```

- [x] **Step 6: Verify**

Run:

```powershell
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims -- --nocapture
cargo test -p codestory-workspace workspace_supported_source_extensions_have_registry_profiles -- --nocapture
cargo test -p codestory-contracts language_support -- --nocapture
git diff --check origin/main...HEAD
```

Expected: all pass.

---

### Task 3: Remove Production Benchmark-Family Packet Steering

**Files:**
- Modify: `crates/codestory-runtime/src/agent/orchestrator.rs`
- Modify: `crates/codestory-runtime/src/agent/eval_probes.rs`
- Modify: `scripts/lint-retrieval-generalization.mjs`
- Modify: `docs/testing/language-expansion-ab-report.md`

- [x] **Step 1: Add or reuse one eval-only predicate**

Expose a runtime predicate in `eval_probes.rs` with production default `false`.

Use this behavior:

```rust
pub(crate) fn exact_family_steering_enabled() -> bool {
    std::env::var("CODESTORY_EVAL_PROBES")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
```

If an equivalent function already exists, reuse it and remove any separate
default-on legacy exact-family steering path.

- [x] **Step 2: Gate prompt-derived benchmark probes**

In `orchestrator.rs`, ensure the following call sites only run when the eval predicate is true:

```rust
push_prompt_named_file_probe_queries(&terms, &mut queries);
push_prompt_concept_derived_symbol_probes(terms, &mut queries);
```

Use this shape:

```rust
if eval_probes::exact_family_steering_enabled() {
    push_prompt_named_file_probe_queries(&terms, &mut queries);
    push_prompt_concept_derived_symbol_probes(terms, &mut queries);
}
```

- [x] **Step 3: Gate or delete canned benchmark-family source claims**

The functions that emit claims for exact repos such as `StringUtils`, Gin, `source/animate.css`, and AutoMapper must not run in production. Either move them into eval-only test helpers or guard the call in `packet_append_source_derived_flow_claims`.

Use this shape:

```rust
if eval_probes::exact_family_steering_enabled() {
    for claim in packet_source_derived_claims_for_citation(prompt, citation, &source) {
        push_unique_claim(claims, seen, claim);
    }
}
```

Keep generic source-derived claims that parse local source structure, but remove exact project-family claims from production.

- [x] **Step 4: Update tests**

Tests that expect exact probes for Commons Lang, SWR, Gin, animate.css, or AutoMapper must set `CODESTORY_EVAL_PROBES=1` for the duration of the test, or be rewritten as generic-shape tests that do not mention those families.

Use a scoped environment helper so tests restore the old value:

```rust
let previous = std::env::var_os("CODESTORY_EVAL_PROBES");
std::env::set_var("CODESTORY_EVAL_PROBES", "1");
// assertions
match previous {
    Some(value) => std::env::set_var("CODESTORY_EVAL_PROBES", value),
    None => std::env::remove_var("CODESTORY_EVAL_PROBES"),
}
```

- [x] **Step 5: Strengthen the generalization lint**

Add these banned production patterns to `scripts/lint-retrieval-generalization.mjs`:

```javascript
"StringUtils",
"commons-lang",
"useSWR",
"swr",
"gin.go",
"RouterGroup.Handle",
"Engine.addRoute",
"Engine.handleHTTPRequest",
"AutoMapper",
"TypeMapPlanBuilder",
"source/animate.css"
```

Allow them only in tests, docs, task manifests, and eval-only helpers.

- [x] **Step 6: Update the A/B report wording**

In `docs/testing/language-expansion-ab-report.md`, make the top verdict explicit:

```markdown
Production runtime defaults do not enable exact benchmark-family steering. Rows that used `CODESTORY_EVAL_PROBES=1` are eval-only diagnostics and are not promotion evidence.
```

- [x] **Step 7: Verify**

Run:

```powershell
cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture
cargo test -p codestory-runtime packet_plan -- --nocapture
node scripts\lint-retrieval-generalization.mjs
git diff --check origin/main...HEAD
```

Expected: all pass, and the lint fails if exact benchmark strings appear in production runtime paths outside eval-only gates.

---

### Task 4: Make Language Regression Tests Prove The Claimed Semantics

**Files:**
- Modify: `crates/codestory-indexer/tests/import_resolution.rs`
- Modify: `crates/codestory-indexer/tests/tictactoe_language_coverage.rs`

- [x] **Step 1: Split import extraction from resolution**

Rename the current single-file test to make its real contract explicit:

```rust
fn test_import_edges_are_extracted_across_languages() -> anyhow::Result<()> {
```

Rename `assert_imports_resolved` to:

```rust
fn assert_import_edges_extracted(edges: &[codestory_contracts::graph::Edge]) {
```

Keep the assertion that at least one `EdgeKind::IMPORT` exists.

- [x] **Step 2: Add a real cross-file resolution test**

Add fixtures with indexed targets in the same temporary workspace.

Use this shape for TypeScript:

```rust
let (nodes, edges) = index_workspace(&[
    (
        "src/foo.ts",
        r#"
export interface Foo { id: number }
"#,
    ),
    (
        "src/main.ts",
        r#"
import type { Foo } from "./foo";
const value: Foo = { id: 1 };
"#,
    ),
])?;
assert_import_resolved_to(&nodes, &edges, "src/main.ts", "src/foo.ts", "Foo");
```

Repeat with at least one Rust module import where the target file is present. Do not use stdlib imports for resolution assertions.

- [x] **Step 3: Add an assertion helper for resolved targets**

Use this helper shape:

```rust
fn assert_import_resolved_to(
    nodes: &[codestory_contracts::graph::Node],
    edges: &[codestory_contracts::graph::Edge],
    importer_suffix: &str,
    target_suffix: &str,
    target_name: &str,
) {
    let resolved = edges.iter().any(|edge| {
        edge.kind == EdgeKind::IMPORT
            && edge.resolved_target.is_some()
            && edge.confidence.unwrap_or(0.0) >= 0.55
            && edge.resolved_target.as_ref().is_some_and(|target_id| {
                nodes.iter().any(|node| {
                    &node.id == target_id
                        && matches_name(&node.serialized_name, target_name)
                        && file_path_for_node(
                            &nodes.iter().map(|node| (node.id.clone(), node.clone())).collect(),
                            node
                        )
                        .map(|path| path.replace('\\', "/").ends_with(target_suffix))
                        .unwrap_or(false)
                })
            })
    });
    assert!(resolved, "expected import from {importer_suffix} to resolve to {target_name} in {target_suffix}");
}
```

Refactor as needed so the helper does not allocate a node map inside a loop.

- [x] **Step 4: Tighten method-kind expectations**

In `tictactoe_language_coverage.rs`, update Kotlin/Swift/Dart class or protocol member expectations from `NodeKind::FUNCTION` to `NodeKind::METHOD` where the source member is owned by a class/interface/protocol.

Then change `has_node` so `NodeKind::FUNCTION` no longer accepts `NodeKind::METHOD` in this regression test:

```rust
node.kind == expected_kind
```

- [x] **Step 5: Verify**

Run:

```powershell
cargo test -p codestory-indexer --test import_resolution -- --nocapture
cargo test -p codestory-indexer --test tictactoe_language_coverage -- --nocapture
git diff --check origin/main...HEAD
```

Expected: all pass and failures would catch missing import binding or method/function kind drift.

---

### Task 5: Add Bounded Runtime File Reads For Semantic Docs

**Files:**
- Modify: `crates/codestory-runtime/src/lib.rs`
- Modify: `crates/codestory-runtime/src/support.rs`
- Test: `crates/codestory-runtime/src/lib.rs` or an existing runtime test module

- [x] **Step 1: Add bounded read helper**

In `support.rs`, add a helper that reads at most a fixed byte limit from a UTF-8-ish source file.

Use constants with conservative defaults:

```rust
pub(crate) const SEMANTIC_FILE_TEXT_MAX_BYTES: u64 = 1_000_000;
pub(crate) const SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES: usize = 64 * 1_024 * 1_024;
```

Helper shape:

```rust
pub(crate) fn read_file_text_limited(path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > max_bytes {
        return Ok(None);
    }
    std::fs::read_to_string(path).map(Some)
}
```

- [x] **Step 2: Use bounded reads in semantic file text cache**

In `build_semantic_file_text_cache`, replace unbounded `read_to_string` calls with `read_file_text_limited(..., SEMANTIC_FILE_TEXT_MAX_BYTES)`.

If the aggregate cache grows beyond `SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES`, stop caching additional file bodies and store `None` for later files.

- [x] **Step 3: Add tests**

Add tests for:

```rust
#[test]
fn semantic_file_text_cache_skips_files_above_byte_limit() { ... }

#[test]
fn semantic_file_text_cache_respects_aggregate_byte_limit() { ... }
```

Use tiny test-only limits if the helper accepts limits as arguments; otherwise test the helper directly with a file just over the limit using sparse metadata only if portable on Windows. Prefer direct helper tests with injectable limits.

- [x] **Step 4: Verify**

Run:

```powershell
cargo test -p codestory-runtime semantic_file_text_cache -- --nocapture
cargo test -p codestory-runtime llm_doc -- --nocapture
git diff --check origin/main...HEAD
```

Expected: all pass.

---

### Task 6: Clean Durable Documentation And Evidence Logs

**Files:**
- Modify: `docs/testing/codestory-e2e-stats-log.md`
- Modify: `docs/testing/oss-language-corpus.md`
- Modify: `docs/architecture/retrieval-parser-compat-matrix.md`
- Modify: `docs/testing/language-expansion-ab-report.md`
- Delete or reduce: `docs/review-action-plan.md`

- [x] **Step 1: Repair malformed phase metric rows**

In `docs/testing/codestory-e2e-stats-log.md`, rows under `## Phase Metrics` must match the table columns:

```markdown
| Date | Commit | Scenario | Total Index s | Graph Phase s | Semantic Phase s | Embeddings Reused | Embeddings Created | Embedding Errors |
```

Rows that have headline stats columns must be moved to the headline stats table or rewritten into this 9-column schema.

- [x] **Step 2: Correct OSS corpus count**

In `docs/testing/oss-language-corpus.md`, change the current edge count from `312,269` to `312,268` if the local integrity script still reports that value.

Run:

```powershell
node scripts\codestory-language-holdout-integrity.mjs
```

Expected: output includes `edges=312268`.

- [x] **Step 3: Clarify artifact integrity versus freshness**

Replace any wording that implies the integrity script reruns indexing with:

```markdown
The integrity script validates the recorded artifact shape and provenance. It is not a fresh indexing run unless the corpus test is rerun with `CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1`.
```

- [x] **Step 4: Remove missing local plan reference**

In `docs/architecture/retrieval-parser-compat-matrix.md`, remove the missing
local retrieval-language-support plan reference and replace it with a durable
rationale sentence tied to the workspace policy and current registry.

- [x] **Step 5: Remove branch-local review plan from canonical docs**

Delete `docs/review-action-plan.md` unless it contains durable guidance not represented elsewhere. If keeping a tiny version, make it a general checklist and remove branch-local remediation history, filtered validation commands, and PR-local wording.

- [x] **Step 6: Shrink the A/B report**

In `docs/testing/language-expansion-ab-report.md`, keep:

- current honest verdict,
- no-hidden-steering baseline,
- reproduction commands,
- links to durable scripts/manifests,
- explicit promotion blockers.

Remove:

- long `target/agent-benchmark/...` catalog sections,
- raw command transcript appendices,
- per-segment diary entries that are not durable conclusions.

- [x] **Step 7: Verify docs**

Run:

```powershell
$task6CleanupPattern = @(
  ("CODESTORY_PACKET_" + "EXACT_FAMILY_STEERING"),
  ("target/agent-benchmark/" + "segment"),
  ("retrieval-language-support_" + "038d3ae9"),
  ("External Review " + "Action Plan")
) -join "|"
rg -n $task6CleanupPattern docs benchmarks/tasks/README.md
node scripts\codestory-language-holdout-integrity.mjs
git diff --check origin/main...HEAD
```

Expected: no missing-plan reference, no branch-local review plan in canonical docs, no long raw benchmark segment catalog in the durable report, and integrity script passes.

---

### Task 7: Final Serialized Verification And Branch Evidence

**Files:**
- Modify: `docs/testing/codestory-e2e-stats-log.md` only if the ignored repo-scale e2e gate is run successfully at reviewed HEAD.

- [ ] **Step 1: Run narrow serialized suite**

Run commands one at a time:

```powershell
cargo check --workspace
cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture
cargo test -p codestory-runtime packet_sufficiency_treats_unresolved_sidecar_candidates_as_gap -- --nocapture
cargo test -p codestory-indexer --test import_resolution -- --nocapture
cargo test -p codestory-indexer --test tictactoe_language_coverage -- --nocapture
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims -- --nocapture
cargo test -p codestory-workspace workspace_supported_source_extensions_have_registry_profiles -- --nocapture
node scripts\lint-retrieval-generalization.mjs
node scripts\codestory-language-holdout-integrity.mjs
git diff --check origin/main...HEAD
```

Expected: all pass.

- [ ] **Step 2: Rebuild the CLI release binary**

Run:

```powershell
cargo build --release -p codestory-cli
```

Expected: release build passes.

- [ ] **Step 3: Refresh active runtime surfaces**

Run:

```powershell
target\release\codestory-cli.exe index --project . --refresh incremental
target\release\codestory-cli.exe retrieval status --project . --format json
target\release\codestory-cli.exe doctor --project . --format json
target\release\codestory-cli.exe files --project . --format json
target\release\codestory-cli.exe ready --project . --format json
```

Expected: index and doctor succeed; if retrieval is stale, run full retrieval indexing before claiming packet/search readiness.

- [ ] **Step 4: Run and log repo-scale e2e only if preparing to commit or merge**

Run:

```powershell
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Expected: pass. Append the fresh row for current `HEAD` to `docs/testing/codestory-e2e-stats-log.md`.

- [ ] **Step 5: Final diff review**

Run:

```powershell
git status --short
git diff --stat origin/main...HEAD
git diff --check origin/main...HEAD
```

Expected: only intentional remediation changes remain.
