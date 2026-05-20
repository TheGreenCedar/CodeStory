# Framework Route Modeling Draft

## Scope

This draft owns only ideas 1, 2, 3, and 10 from the CLI navigation next-wave suite:

- Framework Route Extraction 2.0.
- Wider Web Framework Coverage.
- Automated Framework Coverage Matrix.
- Route/Endpoint First-Class Model.

The lane is CLI-first and indexer/runtime/contracts owned. It does not add MCP tools, server routes, `projectPath`, or `serve --stdio` behavior.

## Current Baseline

- Framework routes are currently discovered in `crates/codestory-indexer/src/lib.rs` and emitted as graph nodes with labels such as `GET /users (express route; confidence=heuristic)`.
- Handler links are currently call edges from the route node to graph-visible handlers when `find_framework_route_handler` can resolve a candidate.
- OpenAPI endpoints are indexed separately as endpoint-like function nodes, and client literals can create speculative call edges to those endpoints.
- `docs/testing/framework-route-coverage.md` is the current verification playbook and already distinguishes fixture-backed support from heuristic hits.

## Blueprint Components

| Component | Responsibility | Implementation Boundary |
|---|---|---|
| **FrameworkRouteRegistry** | Discover framework routes, normalize route metadata, attach confidence/source-convention labels, and propose handler links. | `codestory-indexer`; no CLI parsing or store ownership. |
| **FrameworkRouteFixtureSuite** | Hold per-framework fixtures for declaration styles, nested routes, parameters, and handler-link cases. | Indexer tests and fixtures; every promoted framework status must have fixture evidence. |
| **CoverageReporter** | Produce a machine-readable and Markdown coverage matrix from fixture/eval status, known gaps, and confidence levels. | CLI/runtime read path over indexed/eval evidence; no live service dependency. |
| **RouteEndpointModel** | Represent framework routes and OpenAPI endpoints with stable internal metadata and provenance. | `codestory-contracts` DTOs plus runtime/store read helpers; indexer remains the writer of graph evidence. |

## Requirements

### FRM-R1: Framework Route Extraction 2.0

- **FRM-R1-AC1**: WHEN a supported framework file is indexed, THE **FrameworkRouteRegistry** SHALL emit normalized `framework`, `method`, `path`, `params`, `source_file`, `line`, `confidence`, and `source_convention` metadata.
- **FRM-R1-AC2**: WHEN handler evidence is graph-visible in the same indexed file or resolvable owner scope, THE **FrameworkRouteRegistry** SHALL emit a handler-link candidate with certainty and confidence instead of relying only on display-name matching.
- **FRM-R1-AC3**: WHEN extraction is heuristic, partial, or file-convention-only, THE **FrameworkRouteRegistry** SHALL preserve that status in route metadata and downstream coverage output.
- **FRM-R1-AC4**: WHEN a route uses framework-specific parameter syntax, THE **FrameworkRouteRegistry** SHALL normalize path parameters without losing the raw source path.

### FRM-R2: Wider Web Framework Coverage

- **FRM-R2-AC1**: WHEN a framework is added or promoted, THE **FrameworkRouteFixtureSuite** SHALL include fixtures for static routes, nested routes, dynamic parameters, and at least one unsupported-pattern note.
- **FRM-R2-AC2**: WHEN handler linking is claimed for a framework, THE **FrameworkRouteFixtureSuite** SHALL assert the route node, handler node, edge certainty, and confidence.
- **FRM-R2-AC3**: WHEN support is partial, THE **CoverageReporter** SHALL show the framework as partial or heuristic, not supported.
- **FRM-R2-AC4**: WHEN adding framework coverage beyond the current coverage list, THE implementation SHALL update the coverage playbook and route-search eval expectations in the same branch.

### FRM-R3: Automated Framework Coverage Matrix

- **FRM-R3-AC1**: WHEN coverage checks run, THE **CoverageReporter** SHALL produce JSON with framework, language, fixture status, confidence floor, handler-link status, unsupported patterns, and promotable status.
- **FRM-R3-AC2**: WHEN Markdown output is requested, THE **CoverageReporter** SHALL render the same matrix without changing the JSON contract.
- **FRM-R3-AC3**: WHEN any required fixture fails or a known gap is unacknowledged, THE **CoverageReporter** SHALL mark the framework non-promotable.
- **FRM-R3-AC4**: WHEN route extraction changes, THE coverage matrix SHALL be runnable through a narrow cargo test or CLI command and referenced by docs/testing guidance.

### FRM-R10: Route/Endpoint First-Class Model

- **FRM-R10-AC1**: WHEN route metadata is persisted or exposed, THE **RouteEndpointModel** SHALL carry `kind`, `framework`, `method`, `path`, `raw_path`, `params`, `source_file`, `line`, `confidence`, `source_convention`, `handler`, and `provenance`.
- **FRM-R10-AC2**: WHEN a framework route and OpenAPI endpoint share method/path, THE **RouteEndpointModel** SHALL preserve both sources and expose a relationship instead of deduping away either source.
- **FRM-R10-AC3**: WHEN CLI JSON consumes route data, THE DTOs SHALL be transport-neutral and owned by `codestory-contracts` or runtime read models, not server-specific types.
- **FRM-R10-AC4**: WHEN existing route nodes remain function-like in the graph for compatibility, THE model SHALL provide a typed metadata layer so callers do not parse `serialized_name`.

## Design Notes

- Keep the graph compatibility path boring: existing route nodes can remain `NodeKind::FUNCTION` until a schema migration is explicitly scoped, but route semantics should move out of the display label and into typed metadata.
- Add a small `RouteEndpointMetadata` DTO before broad extractor rewrites. It should be serializable, path-stable, and usable by CLI JSON, search evals, and coverage reporting.
- Treat route source provenance as a list, not a scalar. The same `GET /api/users` may come from OpenAPI, Express, and a client literal edge, and the model should report those as related evidence.
- Split route confidence from edge certainty. Route extraction confidence describes how the route was found; handler-link certainty describes how well the route is connected to code.
- Prefer fixture-backed extractors over broad regex growth. Heuristics are acceptable only when the coverage matrix advertises them as heuristic or partial.
- Do not route this work through `serve`, MCP/stdio catalogs, or browser-surface expansion. CLI output can consume runtime DTOs after the indexer/store path is stable.

## Implementation Tasks

- [ ] 1. Define typed route metadata contracts.
  - [ ] 1.1 Add `RouteEndpointMetadata`, `RouteEndpointKind`, `RouteSourceConvention`, and provenance shapes in the nearest contracts/runtime boundary.
  - [ ] 1.2 Add conversion from current framework route extraction results and OpenAPI endpoint records into the typed model.
  - [ ] 1.3 Keep existing graph node ids and labels stable while exposing typed metadata for new callers.
  - _Requirements: FRM-R10-AC1, FRM-R10-AC2, FRM-R10-AC3, FRM-R10-AC4_

- [ ] 2. Refactor framework route extraction around normalized route records.
  - [ ] 2.1 Replace ad hoc route fields with a normalized internal route record carrying raw path, normalized path, params, confidence, and source convention.
  - [ ] 2.2 Add handler-link candidate metadata before emitting call edges.
  - [ ] 2.3 Preserve partial/heuristic labels in both graph labels and typed metadata.
  - _Requirements: FRM-R1-AC1, FRM-R1-AC2, FRM-R1-AC3, FRM-R1-AC4_

- [ ] 3. Expand and harden framework fixture coverage.
  - [ ] 3.1 Add or split fixtures for each promoted framework across static, nested, parameterized, and unsupported cases.
  - [ ] 3.2 Assert route metadata, graph node membership, handler-link edge certainty, and confidence for promoted handler-link claims.
  - [ ] 3.3 Update route-search eval expectations when new route names should be discoverable.
  - _Requirements: FRM-R2-AC1, FRM-R2-AC2, FRM-R2-AC4_

- [ ] 4. Build the automated coverage matrix.
  - [ ] 4.1 Add a narrow reporter that reads fixture/eval declarations and emits JSON coverage status.
  - [ ] 4.2 Add Markdown rendering over the same data.
  - [ ] 4.3 Mark frameworks non-promotable when required fixtures fail, handler links are unproven, or known gaps are missing.
  - _Requirements: FRM-R2-AC3, FRM-R3-AC1, FRM-R3-AC2, FRM-R3-AC3, FRM-R3-AC4_

- [ ] 5. Wire CLI-first consumption without transport expansion.
  - [ ] 5.1 Expose route metadata through existing CLI/runtime read flows where route-aware output is already expected.
  - [ ] 5.2 Add JSON golden tests proving callers no longer parse confidence from route display labels.
  - [ ] 5.3 Confirm no MCP catalog, `serve --stdio`, `projectPath`, or server-route changes are required.
  - _Requirements: FRM-R10-AC3, FRM-R10-AC4_

## Validation Points

| Path | Command or Check | Expected Evidence |
|---|---|---|
| Normal path | `cargo test -p codestory-indexer --lib framework_route` | Supported fixtures produce typed metadata, graph nodes, route occurrences, and handler edges where claimed. |
| Failure path | Break or mark missing one required framework fixture | Coverage matrix marks that framework non-promotable and reports the failed/missing fixture. |
| Integration edge | Framework route and OpenAPI endpoint share `GET /api/users` | Route/endpoint output preserves both provenance records and shows the relationship without dropping either source. |
| Search edge | `cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval` | Route discovery expectations still report recall/MRR/latency for route and handler queries. |
| Transport guard | `rg -n "projectPath|serve --stdio|stdio|/route|/endpoint" docs/specs/cli-navigation-next-wave crates/codestory-cli/src` | Any match is reviewed to confirm this lane did not add MCP/server expansion. |
| Docs guard | `git diff -- docs/specs/cli-navigation-next-wave` | Only this draft file changes unless the integration branch intentionally promotes final docs later. |

## Traceability Matrix

| Acceptance Criteria | Tasks | Validation |
|---|---|---|
| FRM-R1-AC1, FRM-R1-AC4 | 2.1 | Normal path |
| FRM-R1-AC2 | 2.2 | Normal path |
| FRM-R1-AC3 | 2.3 | Normal path, Failure path |
| FRM-R2-AC1, FRM-R2-AC2 | 3.1, 3.2 | Normal path |
| FRM-R2-AC3 | 4.3 | Failure path |
| FRM-R2-AC4 | 3.3 | Search edge |
| FRM-R3-AC1, FRM-R3-AC2 | 4.1, 4.2 | Normal path |
| FRM-R3-AC3, FRM-R3-AC4 | 4.3 | Failure path |
| FRM-R10-AC1, FRM-R10-AC2 | 1.1, 1.2 | Integration edge |
| FRM-R10-AC3, FRM-R10-AC4 | 1.3, 5.1, 5.2, 5.3 | Transport guard, Docs guard |
