# CLI Navigation Next Wave Requirements

## Introduction

These requirements cover all ten next-wave ideas as one CLI-first initiative. Requirement IDs are stable and are referenced by implementation tasks and validation.

## Requirements

### Requirement 1: Framework Route Extraction 2.0

#### Acceptance Criteria

1. WHEN the **FrameworkRouteRegistry** indexes a supported framework file, THE **FrameworkRouteRegistry** SHALL emit normalized framework, method, path, raw path, params, source file, line, confidence, and source convention metadata for each discovered route.
2. WHEN a route has graph-visible handler evidence, THE **FrameworkRouteRegistry** SHALL link the route to the handler with certainty and confidence metadata instead of relying only on display-name matching.
3. WHEN route extraction is heuristic, file-convention-only, or partial, THE **FrameworkRouteRegistry** SHALL preserve that confidence label and coverage warning instead of implying full support.

### Requirement 2: Wider Web Framework Coverage

#### Acceptance Criteria

1. WHEN new web frameworks are added or promoted, THE **FrameworkRouteFixtureSuite** SHALL include fixtures for static routes, nested routing, dynamic parameters, handler-link cases, and at least one unsupported-pattern note before support is claimed.
2. WHEN a framework is only partially supported, THE **CoverageReporter** SHALL list unsupported patterns, unresolved handler cases, and whether support is partial or heuristic.
3. WHEN coverage expands beyond the existing stack list, THE **DocumentationSurface** SHALL update the coverage playbook, README-facing support summary, and route-search eval expectations.

### Requirement 3: Automated Framework Coverage Matrix

#### Acceptance Criteria

1. WHEN coverage checks run, THE **CoverageReporter** SHALL produce a machine-readable framework matrix with framework, language, fixture status, confidence floor, handler-link status, unsupported patterns, known gaps, and promotable status.
2. WHEN CLI Markdown is requested, THE **CoverageReporter** SHALL render the same matrix in a concise human-readable format.
3. WHEN a framework fixture fails, THE **CoverageReporter** SHALL make the framework status non-promotable until the failure is fixed.

### Requirement 4: Explore Packet Deepening

#### Acceptance Criteria

1. WHEN `explore` targets a route, handler, symbol, or file-adjacent query, THE **ExploreInvestigationPacket** SHALL include stable sections for status, resolution, navigation, relationship evidence, grouped source slices, related files, route context when available, budget notes, freshness, and next commands.
2. WHEN output is ambiguous, truncated, stale, fallback-backed, or index coverage is partial, THE **ExploreInvestigationPacket** SHALL report explicit resolution, truncation, retrieval, and coverage notes in Markdown and JSON.
3. WHEN profile presets are introduced, THE **ExploreInvestigationPacket** SHALL support route, bug, refactor, and test-impact profiles without changing the default behavior.

### Requirement 5: Affected Analysis 2.0

#### Acceptance Criteria

1. WHEN changed files are analyzed from positional paths, stdin, or git-diff fallback, THE **AffectedImpactAnalyzer** SHALL normalize paths, report matched and unmatched files, and expand matched containers to impacted symbols, routes, public APIs, and likely tests within bounded depth.
2. WHEN impacted symbols, routes, or tests are suggested, THE **AffectedImpactAnalyzer** SHALL include graph depth, relationship reason, and confidence for each candidate, and label tests as focused hints rather than proof of complete verification.
3. WHEN graph evidence is insufficient, stale, generated/vendor-heavy, or route evidence is absent, THE **AffectedImpactAnalyzer** SHALL report blind spots and next commands instead of returning silent certainty.

### Requirement 6: Measured Performance Review

#### Acceptance Criteria

1. WHEN a performance branch starts, THE **PerformanceReviewHarness** SHALL capture baseline timings before code changes, including command, commit, environment knobs, cold/warm cache status, and headline timings.
2. WHEN bottlenecks are reported, THE **PerformanceReviewHarness** SHALL identify the measured path, sample size, timing, dominant cost center, and suspected cause.
3. WHEN a candidate optimization is proposed, THE **PerformanceReviewHarness** SHALL define a no-regression threshold, compare before/after metrics, and mark regressions as rejected in the validation record.

### Requirement 7: Targeted Parallelization and Async Opportunities

#### Acceptance Criteria

1. WHEN parallelization is considered, THE **ParallelizationCandidateGate** SHALL require evidence that the exact candidate path is CPU- or I/O-bound and safely isolated.
2. WHEN a parallel candidate is implemented, THE **ParallelizationCandidateGate** SHALL name the work unit boundary, maximum concurrency, ordering requirements, resource risks, serial fallback, and deterministic-output checks.
3. WHEN broad semantic score parallelization, broad async runtime migration, or cargo-wide concurrency is proposed, THE **ParallelizationCandidateGate** SHALL reject it unless fresh evidence overturns the prior regression.

### Requirement 8: Search Quality 2.0

#### Acceptance Criteria

1. WHEN search-ranking changes are made, THE **SearchQualityHarness** SHALL measure recall, MRR, max latency, and fallback source for exact symbols, natural-language queries, routes, handlers, negative/noisy queries, and repo-text fallback.
2. WHEN expected anchors are missing or low-ranked, THE **SearchQualityHarness** SHALL report the failing query class, expected anchor, and whether evidence came from indexed symbol hits, repo-text hits, or both.
3. WHEN route extraction changes, THE **SearchQualityHarness** SHALL include route-discovery expectations in the eval set.

### Requirement 9: README and Docs Refresh

#### Acceptance Criteria

1. WHEN the next-wave features are implemented, THE **DocumentationSurface** SHALL update README workflows for build, index, ground/search, `files`, `explore`, `context`, `affected`, route coverage, and evaluation commands without leading readers into MCP/server work.
2. WHEN docs describe support status, THE **DocumentationSurface** SHALL distinguish supported, heuristic, partial, unsupported, stale, and non-promotable coverage.
3. WHEN CLI behavior changes, THE **DocumentationSurface** SHALL update repo-local skill references, recovery/failure guidance, and docs contract tests in the same branch.

### Requirement 10: First-Class Route/Endpoint Model

#### Acceptance Criteria

1. WHEN route metadata is persisted or exposed, THE **RouteEndpointModel** SHALL carry kind, framework, method, path, raw path, params, source file, line, confidence, source convention, handler link, and provenance.
2. WHEN OpenAPI endpoints, framework routes, and client literal evidence overlap, THE **RouteEndpointModel** SHALL preserve all sources and expose their relationship without losing source provenance.
3. WHEN downstream CLI JSON consumes route data, THE **RouteEndpointModel** SHALL keep transport-neutral DTOs owned by contracts/runtime rather than adding server-specific types.
