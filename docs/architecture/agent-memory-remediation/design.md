# Design Document

## Overview

The remaining work is a sufficiency and harness cleanup, not a retrieval rewrite. The sidecar proof tier is already full; the failures are from eligibility, role coverage, compact budgeting, dynamic label synthesis, and packet-runtime provenance.

## Design Principles

- Retrieval discovers; evidence proves.
- Role ids beat claim-string families.
- Structural languages get honest source-tier rules.
- Benchmark exactness stays in manifests and tests.
- Compact output keeps proof before narration.

## Component Specifications

### Component: FlowRequirementEngine

**Purpose**: Own generic flow roles, query seeds, and coverage modes for planning, probes, and sufficiency.

**Location**: `crates/codestory-runtime/src/agent/packet_flow_requirements.rs`

**Changes**:

- Split `HTML_CSS_FLOW` into `HTML_TEMPLATE_FLOW`, `FORM_VALIDATION_FLOW`, and `CSS_ANIMATION_FLOW`.
- Add role ids for `form_custom_validation`, `form_submit_guard`, `css_animation_base`, `css_keyframes`, and `css_import_graph`.
- Return role ids and coverage modes to sufficiency, not just query strings.
- Keep the table static and dependency-free.

**Implements**: Req 1.1, Req 1.3, Req 2.3, Req 6.3

### Component: EvidenceRoleClassifier

**Purpose**: Convert citations and source ranges into proof roles with tier and resolution eligibility.

**Location**: `crates/codestory-runtime/src/agent/packet_evidence.rs`, `crates/codestory-runtime/src/agent/packet_evidence_roles.rs`

**Changes**:

- Add a role classifier result that carries `role_id`, evidence tier, resolution, producer, and file language.
- Treat generic source-evidence claims as navigation unless the role id matches a required role.
- Add source-range roles for structural selectors, attributes, table definitions, and constraints.

**Implements**: Req 1.2, Req 2.1, Req 2.2, Req 3.2

### Component: CoverageReportBuilder

**Purpose**: Decide `sufficient`, `partial`, or `insufficient` from covered, missing, ineligible, unresolved, and budget-omitted roles.

**Location**: `crates/codestory-runtime/src/agent/packet_sufficiency.rs`

**Changes**:

- Delete the parallel `PacketFlowRole` triad and consume `FlowRequirementEngine` directly.
- Replace `packet_claim_family` text matching with coverage-role aggregation.
- Populate `coverage_report.ineligible` with role and reason pairs.
- Let probe query misses become follow-up suggestions unless their role remains uncovered.
- Block sufficient status when manifest-quality telemetry reports a sufficient/quality mismatch in benchmark mode.

**Implements**: Req 1.1, Req 1.2, Req 1.3, Req 4.1, Req 4.2, Req 4.3

### Component: StructuralLanguagePolicy

**Purpose**: Declare which evidence tiers can prove structural languages and file types without resolved graph symbols.

**Location**: `crates/codestory-runtime/src/agent/packet_evidence.rs` or a narrow sibling module.

**Changes**:

- Map `.sql` schema roles to local source-scan or lexical source eligibility.
- Map `.html` form roles to element, attribute, event-handler, and custom-validation source ranges.
- Map `.css` animation roles to custom property, selector, import, and keyframe source ranges.
- Keep parser-backed Rust/Java/JS/Python/Kotlin/Swift/Dart proof stricter when resolved graph evidence exists.

**Implements**: Req 2.1, Req 2.2, Req 2.3

### Component: PacketAnswerSynthesizer

**Purpose**: Emit source-backed claims and symbol labels that agents can preserve without benchmark claim templates.

**Location**: `crates/codestory-runtime/src/agent/packet_claims.rs`, `crates/codestory-runtime/src/agent/packet_claim_profiles.rs`, `crates/codestory-runtime/src/agent/orchestrator.rs`

**Changes**:

- Preserve receiver aliases for source-defined methods when the citation display name is weaker than the source shape.
- For JavaScript-style assignment methods, prefer source-near alias labels over component-report labels.
- Stop emitting unrelated generic claims as sufficiency-eligible.
- Keep benchmark expected wording only in `benchmarks/tasks`, tests, and eval probes.

**Implements**: Req 3.1, Req 3.2, Req 3.3, Req 6.1

### Component: BenchmarkProvenanceGate

**Purpose**: Separate product packet failures from harness/cache/repo provenance failures.

**Location**: `scripts/codestory-agent-ab-benchmark.mjs`, `scripts/tests/codestory-agent-ab-analyzer.test.mjs`, `scripts/tests/codestory-benchmark-contract.test.mjs`

**Changes**:

- Pass `cache_prepared` and `cache_preparation` into cold and warm packet-runtime provenance.
- Add blocker categories: `packet_quality`, `packet_sufficiency`, `packet_sla`, `harness_cache_provenance`, and `repo_provenance`.
- Keep `--publishable` hard, but make diagnostics explain whether the product or harness failed.
- Add regression rows for the array-to-map cache-preparation bug.

**Implements**: Req 5.1, Req 5.2, Req 5.3, Req 6.2
