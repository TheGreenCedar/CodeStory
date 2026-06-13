# Requirements Document

## Introduction

This document defines the fix contract for the AST-first retrieval review remediation. It treats the three external reviews as evidence to verify, not as orders to copy, and converts confirmed issues into testable requirements.

## Glossary

- **Benchmark-family steering**: Product code that recognizes a named benchmark family or repository and injects hardcoded probes, claims, citations, or file paths.
- **Production packet path**: Runtime code used by normal `ask` or packet answer generation outside benchmark/eval-only harnesses.
- **Support claim**: The user-facing statement that a language is parser-backed, structural-only, unsupported, or otherwise covered at a defined evidence level.
- **Unresolved sidecar candidate**: A sidecar retrieval hit that cannot be mapped back to an indexed CodeStory symbol/file hit.
- **Whole-index count**: Count over the entire stored indexed file inventory.
- **Filtered visible count**: Count after `files` path/language/role filters are applied, before or after the display limit as explicitly named.

## Requirements

### Requirement 1: Remove Production Benchmark-Family Steering

**Description**: Production packet behavior must retrieve and cite real graph/sidecar/source evidence instead of injecting hardcoded benchmark-family answers.

#### Acceptance Criteria

1. WHEN packet answers are assembled in the production runtime path, THE **PacketRetrievalProductPath** SHALL NOT call or depend on Chinook, MDN, Okio, Monolog, Alamofire, or other named benchmark-family static citation helpers.
2. WHEN benchmark-family probes are still useful for evaluation, THE **EvaluationProbeBoundary** SHALL store them in eval-only manifests or explicitly opt-in eval code that is unreachable from default product packet execution.
3. WHEN production retrieval/indexing code is linted, THE **GeneralizationGuard** SHALL fail on the review-named benchmark-family literals and static benchmark path fragments outside tests and eval-only boundaries.
4. WHEN packet sufficiency tests run, THE **VerificationGate** SHALL prove production packet behavior still works with benchmark-family steering disabled or removed.

### Requirement 2: Consolidate Language Support Truth

**Description**: Language support claims must come from one shared registry instead of drift-prone hardcoded tables.

#### Acceptance Criteria

1. WHEN a file extension, stored language name, support mode, evidence tier, or claim label is needed, THE **LanguageSupportRegistry** SHALL provide the value from one shared contract in `codestory-contracts`.
2. WHEN semantic symbol documents are built, THE **SemanticDocumentBuilder** SHALL label every registry-supported parser-backed and structural language or explicitly omit unsupported languages through the same registry decision.
3. WHEN workspace discovery, indexer support profiles, runtime semantic docs, and CLI `files` summaries are compared, THE **VerificationGate** SHALL detect extension or claim drift between those surfaces.
4. WHEN language-support docs or review action docs describe support status, THE **ReviewEvidenceLedger** SHALL update or supersede stale "done" claims that contradict current code.

### Requirement 3: Surface Sidecar Resolution Gaps

**Description**: Packet retrieval must preserve the difference between no sidecar evidence and unresolved sidecar evidence.

#### Acceptance Criteria

1. WHEN single sidecar search receives candidates that cannot resolve to indexed symbols, THE **SidecarResolutionDiagnostics** SHALL keep rejecting unresolved-only results with a diagnostic.
2. WHEN packet batch sidecar search receives unresolved candidates for a subquery, THE **SidecarResolutionDiagnostics** SHALL record per-query candidate count, resolved-hit count, and unresolved-candidate count.
3. WHEN packet sufficiency evaluates subquery evidence, THE **PacketRetrievalProductPath** SHALL treat unresolved-only sidecar candidates as an evidence gap rather than as successful retrieval or indistinguishable emptiness.
4. WHEN packet batch tests run, THE **VerificationGate** SHALL cover empty, unresolved-only, resolved-only, and mixed sidecar subqueries.

### Requirement 4: Make `files` Counts Truthful Under Filters

**Description**: The `files` API and CLI must make whole-index inventory and filtered visible rows impossible to confuse.

#### Acceptance Criteria

1. WHEN `IndexedFilesDto` is returned with path, language, or role filters, THE **IndexedFilesSurface** SHALL expose either distinct whole-index and filtered counts or labels that make the summary scope explicit.
2. WHEN CLI markdown renders `files` output, THE **IndexedFilesSurface** SHALL distinguish whole-index file/language totals from filtered visible row counts and truncation.
3. WHEN JSON output is used, THE **IndexedFilesSurface** SHALL preserve backward-compatible fields where feasible while adding unambiguous filtered count fields.
4. WHEN filters are tested, THE **VerificationGate** SHALL cover path, language, role, and truncation scenarios.

### Requirement 5: Track Receiver Resolution and Parameter Extraction Debt Honestly

**Description**: First-class language claims must not hide known receiver-call and parameter-extraction limitations.

#### Acceptance Criteria

1. WHEN docs describe parser-backed language support, THE **ReceiverResolutionRoadmap** SHALL state that cross-package, polymorphic, inheritance-heavy, and framework-handler resolution need dedicated tests before specific product claims rely on them.
2. WHEN typed receiver-call behavior is claimed, THE **VerificationGate** SHALL include fixtures for same-file and cross-file receiver calls or explicitly limit the claim to the cases currently covered.
3. WHEN manual string-based parameter extraction remains in production, THE **ReceiverResolutionRoadmap** SHALL document it as a transitional implementation boundary with known replacement criteria.
4. WHEN receiver resolution is fixed later, THE **ReceiverResolutionRoadmap** SHALL route it through global resolution support or another cross-file-aware lookup rather than only local file node/edge scans.

### Requirement 6: Pin Verification Before Merge or Push

**Description**: The remediation cannot close on source edits alone.

#### Acceptance Criteria

1. WHEN implementation is complete, THE **VerificationGate** SHALL run `cargo fmt --check`, `cargo check --all-targets`, `node scripts/lint-retrieval-generalization.mjs`, and targeted Rust/Node tests for touched surfaces.
2. WHEN language support or parser-backed claims change, THE **VerificationGate** SHALL run full test binaries for `fidelity_regression` and `tictactoe_language_coverage`, not filtered test names.
3. WHEN the branch is committed or prepared for merge, THE **VerificationGate** SHALL run the repo-scale release CLI e2e stats gate and update `docs/testing/codestory-e2e-stats-log.md` unless the user explicitly waives that expensive gate.
4. WHEN final status is reported, THE **ReviewEvidenceLedger** SHALL list what was verified, what was not verified, and any remaining product-risk assumptions.
