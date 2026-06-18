# Requirements Document

## Introduction

This document defines the current remediation requirements for packet-runtime promotion on branch `codex/packet-answer-quality-hardening-review` after commit `34aa184c harden generic packet proof roles`.

## Glossary

- **FlowRequirementEngine**: The component that defines generic required roles and query seeds.
- **EvidenceRoleClassifier**: The component that classifies citations into proof roles.
- **CoverageReportBuilder**: The component that computes packet sufficiency.
- **StructuralLanguagePolicy**: The component that defines proof eligibility for source-range-first languages.
- **PacketAnswerSynthesizer**: The component that turns evidence into claims and labels.
- **BenchmarkProvenanceGate**: The component that computes publishable blockers.

## Requirements

### Requirement 1: Unified Role-Based Sufficiency

**Description**: Sufficiency must consume generic flow requirements rather than production claim-string catalogs.

#### Acceptance Criteria

1. WHEN a packet is scored, THE **CoverageReportBuilder** SHALL compute missing coverage from `FlowRequirementEngine` role ids and coverage modes.
2. WHEN a claim only points at evidence without explaining a required role, THE **CoverageReportBuilder** SHALL mark it diagnostic and exclude it from sufficiency.
3. WHEN required role coverage is present through eligible citations, THE **CoverageReportBuilder** SHALL NOT block sufficiency only because a planned probe query string was not repeated verbatim.
4. WHEN sidecar candidates are unresolved or not tied to an eligible proof role, THE **CoverageReportBuilder** SHALL report them as diagnostic navigation candidates only and SHALL NOT count them toward sufficiency.

### Requirement 2: Structural Language Proof Eligibility

**Description**: Source-range-first languages must have explicit proof rules that do not pretend every language has graph parity.

#### Acceptance Criteria

1. WHEN a SQL schema packet cites local source-scanned table or foreign-key evidence, THE **StructuralLanguagePolicy** SHALL allow that evidence to satisfy SQL table and relationship roles.
2. WHEN HTML form validation evidence lacks custom validation roles, THE **StructuralLanguagePolicy** SHALL require additional structural source evidence before the packet is sufficient.
3. WHEN a CSS animation prompt is detected, THE **FlowRequirementEngine** SHALL require stylesheet animation roles without requiring HTML app-shell roles.

### Requirement 3: Source-Backed Claim And Symbol Labels

**Description**: Packets must name dynamic-language symbols and claims from source evidence, not fixture templates.

#### Acceptance Criteria

1. WHEN JavaScript prototype or object-assignment methods are cited, THE **PacketAnswerSynthesizer** SHALL preserve source-defined aliases such as receiver method names when available.
2. WHEN a route or dispatch packet cites only nearby files but misses entrypoint or dispatch symbols, THE **PacketAnswerSynthesizer** SHALL leave a specific gap instead of emitting unrelated source-evidence claims.
3. WHEN benchmark expected claims are needed for scoring, THE **PacketAnswerSynthesizer** SHALL NOT contain those exact expected claim strings in production code.
4. WHEN a shell install or dispatch packet is scored, THE **FlowRequirementEngine** SHALL require generic shell install and dispatch roles derived from command/source shape, not benchmark repository identity.

### Requirement 4: Compact Budget Retains Proof First

**Description**: Compact packets should be promotable when proof roles fit, and partial when proof roles are actually omitted.

#### Acceptance Criteria

1. WHEN output is compacted, THE **CoverageReportBuilder** SHALL treat omitted sections as blocking only if they remove required proof roles.
2. WHEN proof roles are complete but verbose sections are truncated, THE **CoverageReportBuilder** SHALL allow sufficiency.
3. WHEN proof roles do not fit compact budget, THE **CoverageReportBuilder** SHALL list the missing roles and include a standard-budget follow-up command.

### Requirement 5: Benchmark Provenance Is Truthful

**Description**: Promotion blockers must distinguish product packet failures from harness provenance failures.

#### Acceptance Criteria

1. WHEN packet-runtime cold rows run after prepared sidecar cache setup, THE **BenchmarkProvenanceGate** SHALL mark cache policy as prepared if the repo appears in cache preparation.
2. WHEN packet-runtime warm rows run after prepared sidecar cache setup, THE **BenchmarkProvenanceGate** SHALL mark cache policy as prepared if the repo appears in cache preparation.
3. WHEN retrieval mode is full but cache policy is unprepared, THE **BenchmarkProvenanceGate** SHALL report a harness-contract blocker separately from packet quality blockers.

### Requirement 6: Promotion Gates Resist Overfit

**Description**: The next slice must improve language support without teaching production code benchmark identities.

#### Acceptance Criteria

1. WHEN production packet modules are scanned, THE **BenchmarkProvenanceGate** SHALL fail if manifest repo slugs, expected paths, expected symbols, or expected claim phrases appear outside eval/test boundaries.
2. WHEN targeted language regressions pass, THE **BenchmarkProvenanceGate** SHALL still require the publishable language-expansion packet-runtime gate before promotion.
3. WHEN a new role rule is added, THE **FlowRequirementEngine** SHALL express it as a generic source shape, not a repository or product-specific rule.
4. WHEN benchmark contract fingerprints or eval-probe catalogs are present, THE **BenchmarkProvenanceGate** SHALL keep them behind non-publishable eval/test boundaries and SHALL NOT treat them as publishable packet proof.
