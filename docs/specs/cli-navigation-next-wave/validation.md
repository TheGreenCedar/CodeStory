# CLI Navigation Next Wave Validation Report

## 1. Requirements to Tasks Traceability Matrix

| Requirement | Acceptance Criterion | Implementing Task(s) | Status |
|---|---|---|---|
| 1. Framework Route Extraction 2.0 | 1.1 | Task 1 | Covered |
|  | 1.2 | Task 1 | Covered |
|  | 1.3 | Task 1 | Covered |
| 2. Wider Web Framework Coverage | 2.1 | Tasks 1, 2 | Covered |
|  | 2.2 | Task 2 | Covered |
|  | 2.3 | Tasks 2, 10 | Covered |
| 3. Automated Framework Coverage Matrix | 3.1 | Task 3 | Covered |
|  | 3.2 | Task 3 | Covered |
|  | 3.3 | Tasks 2, 3 | Covered |
| 4. Explore Packet Deepening | 4.1 | Task 5 | Covered |
|  | 4.2 | Task 5 | Covered |
|  | 4.3 | Task 5 | Covered |
| 5. Affected Analysis 2.0 | 5.1 | Task 6 | Covered |
|  | 5.2 | Task 6 | Covered |
|  | 5.3 | Task 6 | Covered |
| 6. Measured Performance Review | 6.1 | Task 7 | Covered |
|  | 6.2 | Task 7 | Covered |
|  | 6.3 | Task 7 | Covered |
| 7. Targeted Parallelization and Async Opportunities | 7.1 | Task 8 | Covered |
|  | 7.2 | Task 8 | Covered |
|  | 7.3 | Task 8 | Covered |
| 8. Search Quality 2.0 | 8.1 | Task 9 | Covered |
|  | 8.2 | Task 9 | Covered |
|  | 8.3 | Task 9 | Covered |
| 9. README and Docs Refresh | 9.1 | Task 10 | Covered |
|  | 9.2 | Task 10 | Covered |
|  | 9.3 | Task 10 | Covered |
| 10. First-Class Route/Endpoint Model | 10.1 | Task 4 | Covered |
|  | 10.2 | Task 4 | Covered |
|  | 10.3 | Task 4 | Covered |

## 2. Coverage Analysis

### Summary

- **Total Acceptance Criteria**: 30
- **Criteria Covered by Tasks**: 30
- **Coverage Percentage**: 100%

### Detailed Status

- **Covered Criteria**: 1.1, 1.2, 1.3, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 4.1, 4.2, 4.3, 5.1, 5.2, 5.3, 6.1, 6.2, 6.3, 7.1, 7.2, 7.3, 8.1, 8.2, 8.3, 9.1, 9.2, 9.3, 10.1, 10.2, 10.3.
- **Missing Criteria**: none.
- **Invalid References**: none.

## 3. Validation Commands

- Documentation-only validation: `cargo fmt --check`, `cargo test -p codestory-cli --test onboarding_contracts`, `git diff --check`.
- Framework/indexer implementation validation: `cargo test -p codestory-indexer --lib framework_route`, `cargo test -p codestory-indexer --test fidelity_regression`, `cargo test -p codestory-indexer --test tictactoe_language_coverage`, and targeted route fixture tests.
- Runtime/CLI behavior validation: `cargo check -p codestory-contracts -p codestory-indexer -p codestory-runtime -p codestory-cli`, focused CLI golden tests, and stdio contract tests proving no server-surface expansion.
- Explore/affected validation: JSON and Markdown golden tests for normal path, ambiguity/unmatched paths, stale/partial coverage, route evidence present, and route evidence absent.
- Performance/search validation: `search_quality_eval`, `retrieval_eval`, targeted Criterion checks when practical, and repo-scale ignored e2e stats before commit when behavior or performance claims change.
- Current performance record:
  [cli-navigation-next-wave-performance-review.md](../../testing/cli-navigation-next-wave-performance-review.md).
- Transport guard validation: review any `projectPath`, MCP catalog, server-route, watch, or `serve --stdio` changes and reject them for this spec; future transport work requires a separate spec and must not be inferred from this plan.

## 4. Implementation Validation Run

The integrated branch was validated with:

- `cargo fmt`
- `cargo fmt --check`
- `cargo check -p codestory-contracts -p codestory-indexer -p codestory-runtime -p codestory-cli`
- `cargo test -p codestory-contracts`
- `cargo test -p codestory-indexer --lib -- --nocapture`
- `cargo test -p codestory-runtime framework_route_coverage_matrix_lists_fixture_status_and_known_gaps -- --nocapture`
- `cargo test -p codestory-cli --test search_json_output symbol_json_exposes_typed_route_endpoint_metadata -- --nocapture`
- `cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval`
- `cargo test -p codestory-cli --test cli_golden_path tiny_workspace_browser_loop_works_from_existing_cache -- --nocapture`
- `cargo test -p codestory-cli --test onboarding_contracts -- --nocapture`
- `cargo test -p codestory-cli --test stdio_protocol_contracts -- --nocapture`
- `git diff --check`
- Legacy upstream-name scan over the working tree returned no matches.

## 5. Final Validation

All 30 acceptance criteria are traced to implementation tasks. Subagent draft review has been incorporated into the canonical blueprint, requirements, design, tasks, and validation docs. The plan is validated for implementation as a CLI-first next wave with no MCP/server expansion in scope.
