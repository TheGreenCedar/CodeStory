# Design Document

## Overview

The remediation design keeps the first fix boring on purpose: delete or isolate benchmark-family production shortcuts, put language claim metadata in a dependency-safe shared crate, and strengthen diagnostics/tests where evidence can currently disappear. It does not attempt the full dynamic parser architecture from one review because that would mix a product-correctness repair with a parser distribution redesign.

## Design Principles

- **Product path is generic**: production retrieval must not know benchmark families.
- **Claims derive from code**: docs and CLI output must reflect shared runtime contracts.
- **Diagnostics over silence**: unresolved evidence is a state worth reporting.
- **No dependency inversion**: shared language metadata belongs below workspace, indexer, and runtime.
- **Stage large refactors**: dynamic parser loading and broad module decomposition are separate architecture work.

## Component Specifications

### Component: ReviewEvidenceLedger

**Purpose**: Preserve reviewer findings, local code evidence, and explicit scope decisions for the remediation.

**Location**: `docs/specs/review-remediation-ast-first-retrieval/research.md`, `docs/review-action-plan.md`, `docs/architecture/language-support.md`

**Interface**:

```text
Inputs:
- External review files from C:/Users/alber/Downloads/
- Local code references from crates/, docs/, and scripts/

Outputs:
- Evidence table with source IDs
- Updated remediation status in repo docs
- Final verification notes

Implements: Req 2.4, Req 5.1, Req 5.3, Req 6.4
```

**Dependencies**:

- Review files remain outside the repo and should not be copied wholesale.
- Repo docs must be updated when they would otherwise preserve stale "done" claims.

### Component: PacketRetrievalProductPath

**Purpose**: Assemble production packet answers using graph, sidecar, semantic, and generic source-shape evidence only.

**Location**: `crates/codestory-runtime/src/agent/orchestrator.rs`

**Interface**:

```rust
fn agent_packet(...) -> Result<AgentAnswerDto, ApiError>;
fn maybe_append_sql_schema_file_citations(...); // generic SQL-only helper if kept
fn rank_packet_evidence(...);

// Removed from production path:
// maybe_append_chinook_sql_schema_file_citations
// maybe_append_mdn_form_validation_file_citations
// maybe_append_okio_buffer_flow_file_citations
// maybe_append_monolog_record_flow_file_citations
// maybe_append_alamofire_request_flow_file_citations
// packet_exact_family_steering_enabled default-on behavior
```

**Implements**: Req 1.1, Req 1.4, Req 3.3

**Design Notes**:

- Delete static family citation helpers from production code if no eval-only caller remains.
- If exact-family probes must survive for benchmark reproducibility, move them to `crates/codestory-runtime/src/agent/eval_probes.rs`, benchmark manifests, or scripts that are clearly outside product packet assembly.
- Keep generic source-shape logic only when it works across repos by inspecting indexed or source evidence, not by matching benchmark names.

### Component: EvaluationProbeBoundary

**Purpose**: Keep benchmark-family probes and repo-specific expected paths out of production packet behavior.

**Location**: `crates/codestory-runtime/src/agent/eval_probes.rs`, `benchmarks/tasks/`, `scripts/codestory-agent-ab-benchmark.mjs`

**Interface**:

```text
Eval probe source:
- Manifest-declared task family
- Explicit benchmark/eval command
- No default product runtime activation

Implements: Req 1.2
```

**Design Notes**:

- Do not preserve `CODESTORY_PACKET_EXACT_FAMILY_STEERING` as a default-on product escape hatch.
- Any opt-in eval knob must be named as eval/benchmark-only and must not alter default user packet behavior.

### Component: LanguageSupportRegistry

**Purpose**: Define language names, extensions, support modes, evidence tiers, and user-facing claim labels once.

**Location**: `crates/codestory-contracts/src/language_support.rs` plus exports from `crates/codestory-contracts/src/lib.rs`

**Interface**:

```rust
pub enum LanguageSupportMode {
    ParserBackedGraph,
    StructuralCollector,
    TextOnly,
    Unsupported,
}

pub enum LanguageEvidenceTier {
    GraphFidelity,
    StructuralOnly,
    TextOnly,
    Unsupported,
}

pub struct LanguageSupportProfile {
    pub language_name: &'static str,
    pub extensions: &'static [&'static str],
    pub support_mode: LanguageSupportMode,
    pub evidence_tier: LanguageEvidenceTier,
    pub claim_label: &'static str,
}

pub fn language_support_profile_for_ext(ext: &str) -> Option<&'static LanguageSupportProfile>;
pub fn language_support_profile_for_language_name(name: &str) -> Option<&'static LanguageSupportProfile>;
pub fn language_name_for_path(path: Option<&str>) -> Option<&'static str>;
```

**Implements**: Req 2.1, Req 2.3

**Dependencies**:

- `codestory-workspace` can depend on `codestory-contracts`.
- `codestory-indexer` can depend on `codestory-contracts` and still own parser/rule construction.
- `codestory-runtime` can depend on `codestory-contracts` and use the same registry for semantic docs and API output.

**Design Notes**:

- Keep parser handles, tree-sitter rules, and collector implementation out of contracts.
- Indexer `get_language_for_ext` should map registry-supported parser-backed entries to parser construction and tests should catch registry entries that lack parser routing when the claim says parser-backed.
- Workspace discovery should use registry extension metadata plus any intentionally discoverable text-only/template extensions.

### Component: SemanticDocumentBuilder

**Purpose**: Emit semantic document text with language labels derived from the shared registry.

**Location**: `crates/codestory-runtime/src/semantic_doc_text.rs`, `crates/codestory-runtime/src/lib.rs`

**Interface**:

```rust
pub(crate) fn semantic_doc_language_from_path(path: Option<&str>) -> Option<&'static str> {
    codestory_contracts::language_support::language_name_for_path(path)
}

fn build_llm_symbol_doc_text(...) -> String; // emits `language:` from registry lookup
```

**Implements**: Req 2.2

**Design Notes**:

- Remove or shrink the local hardcoded extension table.
- Add tests for every registry-supported parser-backed language and structural language whose symbol docs are expected to carry a language marker.

### Component: IndexedFilesSurface

**Purpose**: Report indexed file inventory with clear whole-index and filtered/visible counts.

**Location**: `crates/codestory-contracts/src/api/dto.rs`, `crates/codestory-runtime/src/lib.rs`, `crates/codestory-cli/src/main.rs`

**Interface**:

```rust
pub struct IndexedFilesSummaryDto {
    pub file_count: u32,              // Backward-compatible whole-index count.
    pub indexed_file_count: u32,      // Backward-compatible whole-index indexed count.
    pub filtered_file_count: u32,     // Count after filters before display limit.
    pub visible_file_count: u32,      // Count returned after limit.
    pub truncated: bool,
    // existing fields...
}
```

**Implements**: Req 4.1, Req 4.2, Req 4.3

**Design Notes**:

- Preserve existing fields if downstream contracts depend on them.
- CLI markdown should say `whole index files:`, `filtered files:`, and `visible rows:` or equivalent.
- JSON consumers get explicit fields and do not need to infer from `files.len()`.

### Component: SidecarResolutionDiagnostics

**Purpose**: Record per-query sidecar candidate, resolved-hit, and unresolved-candidate state for packet batches.

**Location**: `crates/codestory-runtime/src/agent/retrieval_primary.rs`, packet trace DTOs if needed in `crates/codestory-contracts/src/api/dto.rs`

**Interface**:

```rust
pub struct PacketSidecarQueryDiagnostic {
    pub query: String,
    pub candidate_count: u32,
    pub resolved_hit_count: u32,
    pub unresolved_candidate_count: u32,
    pub mode: String,
    pub diagnostic: Option<String>,
}

fn sidecar_packet_batch_rejection_reason(
    query_result: &QueryResult,
    resolved_hits: &[SearchHit],
) -> Option<String>;
```

**Implements**: Req 3.1, Req 3.2, Req 3.4

**Design Notes**:

- Do not fail the whole packet merely because one subquery is unresolved-only.
- Do preserve unresolved-only state so sufficiency and traces can say evidence was attempted but unusable.
- Tests should lock the intended distinction: empty full-mode query is not an error; unresolved-only full-mode query is a diagnostic and sufficiency gap.

### Component: ReceiverResolutionRoadmap

**Purpose**: Track receiver-call and parameter-extraction debt without overstating current support.

**Location**: `docs/architecture/language-support.md`, `docs/review-action-plan.md`, future indexer tests under `crates/codestory-indexer/tests/`

**Interface**:

```text
Current claim:
- Same-file/simple typed receiver support only where tests prove it.

Future claim:
- Cross-file typed receiver support only after fixtures prove imported owner lookup.
- Declarative parameter extraction only after AST/query attributes replace string-sliced signatures for the targeted languages.

Implements: Req 5.1, Req 5.2, Req 5.3, Req 5.4
```

**Design Notes**:

- Add negative or expected-failing fixtures before changing implementation if the receiver fix is scheduled later.
- When implementation starts, prefer `ResolutionSupport` or another global lookup over local file-only scans in `append_manual_receiver_call_edges`.
- Do not claim dynamic parser loading in this remediation.

### Component: GeneralizationGuard

**Purpose**: Fail CI when benchmark-family literals re-enter production retrieval/indexing code.

**Location**: `scripts/lint-retrieval-generalization.mjs`

**Interface**:

```js
const bannedPatterns = [
  // existing patterns...
  "chinook",
  "mdn",
  "okio",
  "monolog",
  "alamofire",
];
```

**Implements**: Req 1.3

**Design Notes**:

- Keep test and eval-only masking explicit.
- Add a regression test or self-test if the script has an existing test harness; otherwise run the lint directly as part of verification.

### Component: VerificationGate

**Purpose**: Run the narrow and branch-scale checks required before implementation can be considered done.

**Location**: repo root commands, `docs/testing/codestory-e2e-stats-log.md`

**Interface**:

```powershell
cargo fmt --check
cargo check --all-targets
node scripts/lint-retrieval-generalization.mjs
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

**Implements**: Req 6.1, Req 6.2, Req 6.3, Req 6.4

**Design Notes**:

- Cargo commands should be serialized in this repo.
- The release e2e stats gate is expensive but required before commit/merge unless explicitly waived.
