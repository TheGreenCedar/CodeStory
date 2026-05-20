# CLI Navigation Next Wave Implementation Plan

- [x] 1. Build the framework route registry
  - [x] 1.1 Extract current framework route heuristics into a registry with per-framework extractors.
  - [x] 1.2 Normalize nested paths, raw paths, route params, methods, controller prefixes, source conventions, and file-convention routes.
  - [x] 1.3 Add handler-link candidates with certainty/confidence only when graph evidence resolves a handler.
  - _Requirements: 1.1, 1.2, 1.3, 2.1_

- [x] 2. Expand web framework fixtures
  - [x] 2.1 Add fixtures for Next.js, Remix, Astro, Nuxt, Fastify, Koa, Hono, NestJS, Go Gin/Chi/Echo/Fiber, and any retained current frameworks.
  - [x] 2.2 Cover static routes, nested routing, route params, grouped routes, controller prefixes, handler-link claims, and unresolved-handler cases.
  - [x] 2.3 Mark each framework as supported, heuristic, partial, unsupported, or non-promotable in the coverage report, with at least one unsupported-pattern note per promoted framework.
  - _Requirements: 2.1, 2.2, 2.3, 3.3_

- [x] 3. Add the automated framework coverage matrix
  - [x] 3.1 Create JSON and Markdown coverage outputs with framework, language, fixture status, confidence floor, handler-link support, unsupported patterns, known gaps, and promotable status.
  - [x] 3.2 Wire the matrix into the testing/docs workflow without changing stdio or HTTP.
  - [x] 3.3 Fail or mark non-promotable when required fixtures regress.
  - _Requirements: 3.1, 3.2, 3.3_

- [x] 4. Introduce the first-class route/endpoint model
  - [x] 4.1 Define route metadata for kind, framework, method, path, raw path, params, file, line, confidence, source convention, handler link, and provenance.
  - [x] 4.2 Preserve OpenAPI endpoint, framework route, and client literal provenance when multiple sources describe the same route.
  - [x] 4.3 Project route metadata into existing graph/runtime/CLI paths without adding server-specific types or requiring callers to parse route display labels.
  - _Requirements: 10.1, 10.2, 10.3_

- [x] 5. Deepen `explore` investigation packets
  - [x] 5.1 Add stable sections for status, resolution, navigation, relationship evidence, route context, trail, source packet, related files, freshness, and next commands.
  - [x] 5.2 Add ambiguity metadata, "why these files" notes, stale/fallback/partial coverage warnings, and source-budget explanations.
  - [x] 5.3 Add explicit `--profile route|bug|refactor|test-impact` behavior while preserving the default output.
  - _Requirements: 4.1, 4.2, 4.3_

- [x] 6. Upgrade `affected` impact analysis
  - [x] 6.1 Normalize positional, stdin, and git-diff paths; report matched/unmatched paths before expanding matched files into impacted symbols, routes, public APIs, and likely tests.
  - [x] 6.2 Score impacted symbols, routes, and likely tests with graph reachability, relationship reason, depth, file role, import/use proximity, and route-handler evidence.
  - [x] 6.3 Report unmatched paths, partial indexes, stale indexes, generated/vendor files, unsupported languages, and missing handler links as blind spots with next commands.
  - _Requirements: 5.1, 5.2, 5.3_

- [x] 7. Run a measured performance review
  - [x] 7.1 Capture baseline timings for index, route extraction, search, explore, affected, and warm read loops with command, commit, environment knobs, and cold/warm cache status.
  - [x] 7.2 Identify dominant cost centers such as DB query count, repeated storage opens, source reads, graph traversal, repo-text scans, CLI JSON rendering, and lock/contention bottlenecks.
  - [x] 7.3 Define no-regression thresholds, record before/after measurements, and reject regressions in a durable validation record.
  - _Requirements: 6.1, 6.2, 6.3_

- [x] 8. Evaluate targeted parallelization opportunities
  - [x] 8.1 Measure whether route extraction, source reads, affected traversal, or search eval are CPU- or I/O-bound before changing concurrency.
  - [x] 8.2 Add bounded concurrency only where the work unit, max concurrency, ordering requirements, resource risks, serial fallback, and deterministic output checks are documented.
  - [x] 8.3 Preserve the block on broad semantic score parallelization, broad async runtime migration, and cargo-wide concurrency unless new evidence proves it helps.
  - _Requirements: 7.1, 7.2, 7.3_

- [x] 9. Expand search-quality evaluation
  - [x] 9.1 Add eval cases for exact symbols, CamelCase, compounds, natural-language queries, routes, handlers, likely tests, negative/noisy queries, and repo-text fallback.
  - [x] 9.2 Emit recall, MRR, max latency, fallback source, failed query class, expected anchor, and anchor bucket.
  - [x] 9.3 Require the eval for route/ranking changes, keep `search --why --format json` diagnosable, and document how to interpret failures.
  - _Requirements: 8.1, 8.2, 8.3_

- [x] 10. Refresh README and agent-facing docs
  - [x] 10.1 Update README workflows for build, index, ground/search, `files`, `explore`, `context`, `affected`, route coverage, and eval commands without drifting into MCP/server usage.
  - [x] 10.2 Update support-status and recovery language so supported, heuristic, partial, unsupported, stale, non-promotable, ambiguous, and unmatched-path cases are explicit.
  - [x] 10.3 Update repo-local skill references and docs contract tests for new CLI behavior after the behavior exists.
  - _Requirements: 9.1, 9.2, 9.3_
