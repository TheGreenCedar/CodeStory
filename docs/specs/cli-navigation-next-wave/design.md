# CLI Navigation Next Wave Design

## Overview

The design keeps CodeStory CLI-first. Indexer and runtime layers own new behavior; CLI renders it. This spec does not change MCP, HTTP, server routes, `projectPath`, watch behavior, or `serve --stdio`; any future work in those areas requires a separate spec and must not be inferred from this plan.

## Component Specifications

### Component: FrameworkRouteRegistry

**Purpose**: Extract framework route facts with normalized path/method/confidence metadata.

**Design**:

- Split route extraction into framework-specific extractors behind a common registry.
- Normalize route paths, params, methods, nested scopes, controller prefixes, and file-convention paths.
- Emit handler-link candidates only when there is graph-visible evidence, and keep handler-link certainty separate from route extraction confidence.
- Attach confidence labels such as `file_convention`, `decorator`, `annotation`, and `heuristic`.
- Preserve the raw source path next to the normalized path so framework-specific parameter syntax is not lost.

**Implements**: Req 1.1, 1.2, 1.3, 2.1.

### Component: FrameworkRouteFixtureSuite

**Purpose**: Make framework support claims fixture-backed and promotable.

**Design**:

- Keep per-framework fixture cases for static routes, nested routes, dynamic parameters, grouped routes, controller prefixes, and unsupported patterns.
- Assert handler-link claims with route node, handler node, edge certainty, and confidence.
- Drive non-promotable status when required fixtures fail or a known gap is undocumented.

**Implements**: Req 2.1, 2.2, 3.3.

### Component: RouteEndpointModel

**Purpose**: Provide a transport-neutral route/endpoint data model for framework routes and schema endpoints.

**Design**:

- Add internal route metadata that can be projected to graph nodes and CLI JSON.
- Preserve source provenance for OpenAPI endpoints, framework routes, and overlapping route declarations.
- Carry route fields: `kind`, `framework`, `method`, `path`, `raw_path`, `params`, `source_file`, `line`, `confidence`, `source_convention`, `handler`, and `provenance`.
- Prefer a typed metadata layer over parsing route meaning back out of display names when graph compatibility keeps existing route nodes function-like.
- Keep route DTOs in contracts/runtime only if needed by CLI output; do not introduce server-specific types.

**Implements**: Req 10.1, 10.2, 10.3.

### Component: CoverageReporter

**Purpose**: Make coverage claims auditable.

**Design**:

- Generate a framework coverage matrix from fixtures and extractor metadata.
- Include framework, language, support status, confidence floor, handler-link support, unsupported patterns, fixture pass/fail state, and promotable status.
- Render JSON and Markdown for CLI/doc workflows.

**Implements**: Req 2.2, 3.1, 3.2, 3.3.

### Component: ExploreInvestigationPacket

**Purpose**: Make `explore` the one-call investigation packet.

**Design**:

- Add route-aware sections when the target resolves to a route or handler.
- Keep grouped line-numbered source slices and budget notes.
- Add profile presets only as explicit CLI flags and preserve current default output.
- Include stable sections for status, resolution, navigation, relationship evidence, trail, grouped source slices, related files, budget notes, freshness, and next commands.
- Include "why these files" and coverage warnings when evidence is partial, stale, fallback-backed, or route evidence is absent.
- Preserve ambiguity metadata and retry guidance instead of silently choosing a target.

**Implements**: Req 4.1, 4.2, 4.3.

### Component: AffectedImpactAnalyzer

**Purpose**: Turn changed files into actionable impact and test-selection hints.

**Design**:

- Expand changed files to contained symbols and route metadata.
- Walk bounded dependents and score likely tests using graph reachability, path role, import/use proximity, and route-handler relationships.
- Report blind spots for unmatched paths, unsupported languages, partial index errors, and missing handler links.
- Shape JSON around `matched_files`, `unmatched_paths`, `impacted_symbols`, `impacted_routes`, `impacted_tests`, `blind_spots`, and `next_commands`.
- Label likely tests as focused hints, not proof that adjacent tests are unnecessary.

**Implements**: Req 5.1, 5.2, 5.3.

### Component: PerformanceReviewHarness

**Purpose**: Make performance work measurement-first.

**Design**:

- Add explicit baseline capture for index, explore, affected, route extraction, search, and warm read loops.
- Baselines record command, commit, environment knobs, cold/warm cache status, headline timings, and the suspected dominant cost center.
- Prefer existing Criterion benches and repo-scale e2e stats before adding new harnesses.
- Record rejected experiments when metrics regress.

**Implements**: Req 6.1, 6.2, 6.3.

### Component: ParallelizationCandidateGate

**Purpose**: Keep async and parallel work bounded, measured, and reversible.

**Design**:

- Require every candidate to name exact code path, measured bottleneck, work unit boundary, maximum concurrency, ordering requirement, resource risk, and serial fallback.
- Check build/cache/store locks, memory pressure, writer contention, and nondeterministic result ordering before promotion.
- Treat broad semantic parallel score computation, broad async runtime migration, and cargo-wide concurrency as blocked unless fresh evidence reverses prior regression evidence.

**Implements**: Req 7.1, 7.2, 7.3.

### Component: SearchQualityHarness

**Purpose**: Keep ranking and route discovery measurable.

**Design**:

- Expand the existing search-quality eval into query classes for exact symbols, CamelCase, compounds, framework routes, handlers, affected-test hints, and repo-text fallback.
- Emit recall, MRR, max latency, fallback source, failing query class, expected anchor, and anchor bucket (`indexed_symbol_hits`, `repo_text_hits`, or both).
- Keep `search --why --format json` useful enough to explain lexical, semantic, graph, fallback, and match-quality contributions for top results.
- Run as an ignored evaluation lane plus focused unit regressions for ranking rules.

**Implements**: Req 8.1, 8.2, 8.3.

### Component: DocumentationSurface

**Purpose**: Keep user-facing and agent-facing docs aligned with the CLI.

**Design**:

- Refresh README command examples and grounding workflows.
- Add route coverage support status and evaluation command references.
- Update repo-local `codestory-grounding` skill refs with any new CLI behavior.
- Keep docs contract tests as drift guards.

**Implements**: Req 2.3, 9.1, 9.2, 9.3.

### Component: ValidationRecord

**Purpose**: Keep promotion evidence durable and reviewable.

**Design**:

- Reuse existing testing docs and e2e stats log shapes unless a missing metric blocks triage.
- Record search-quality thresholds only after the first expanded eval establishes a baseline.
- Store rejected performance or parallelization attempts with the measured regression and stop condition.

**Implements**: Req 6.3, 7.3, 8.1, 8.2, 8.3, 9.3.
