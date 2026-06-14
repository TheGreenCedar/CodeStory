# CLI Navigation Next Wave Performance Review

This is the initial validation record for the CLI-first navigation branch. It is
not a transport, server, MCP, or watch-mode benchmark.

## Environment

| Field | Value |
| --- | --- |
| Date | 2026-05-20 |
| Commit | `fea0cc5` with a dirty working tree for this branch |
| Shell | PowerShell 7.6.1 |
| Rust | `rustc 1.90.0`, `cargo 1.90.0` |
| Binary | `target/debug/codestory-cli.exe` |
| Project | `C:/Users/alber/source/repos/codestory` |
| Cache state | warm existing cache, `--refresh none`; doctor reported stale index freshness |
| Index shape | 145 files, 43,938 nodes, 37,086 edges |
| Retrieval | hybrid ready, 6,029 semantic docs, ONNX BGE base, DirectML provider, stored int8 vectors |

The cache was intentionally not refreshed during these warm-read measurements so
the record captures read-path cost separately from indexing cost. Doctor reported
17 changed files, so these numbers are a branch validation baseline rather than
a release claim.

## Warm Read Baseline

Each command was run four times with the first run discarded. Times are wall
clock milliseconds from `Measure-Command`; stdout was redirected to `Out-Null`.

| Path | Command | Samples ms | Kept avg ms | Kept max ms |
| --- | --- | ---: | ---: | ---: |
| files JSON | `target/debug/codestory-cli.exe files --project . --refresh none --format json` | `761.4, 763.5, 749.4, 743.2` | 752.0 | 763.5 |
| search JSON | `target/debug/codestory-cli.exe search --project . --query build_coverage_buckets --refresh none --format json` | `2875.4, 2900.6, 2960.2, 2848.1` | 2903.0 | 2960.2 |
| explore JSON | `target/debug/codestory-cli.exe explore --project . --id -743279210528755755 --no-tui --refresh none --format json` | `921.8, 860.8, 871.9, 911.1` | 881.3 | 911.1 |
| affected JSON | `target/debug/codestory-cli.exe affected --project . crates/codestory-runtime/src/lib.rs --refresh none --format json` | `1011.4, 996.0, 1027.4, 1010.6` | 1011.3 | 1027.4 |

## Dominant Cost Centers

| Path | Observed cost center | Notes |
| --- | --- | --- |
| files | storage open plus summary/materialization | Matrix rendering is small compared with opening and reading persisted file inventory. |
| search | hybrid search and repo-text fallback eligibility | This warm read is the slowest path in the sample; use `search --why --format json` and search-quality eval before ranking changes. |
| explore | symbol, trail, snippet, and source-slice reads | Profile presets and relationship evidence are bounded by existing depth and node caps. |
| affected | graph traversal plus file-role/test aggregation | Current traversal is bounded by `--depth`; route/test evidence is scored and reported as hints. |

## No-Regression Gates

- Route/ranking changes must keep the search-quality eval at no lost expected
  anchors and no lower MRR unless the validation record explains the tradeoff.
- `files` coverage output must remain deterministic and include
  `coverage_evidence`, `unsupported_patterns`, `known_gaps`, and `promotable`.
- `explore` JSON must keep stable status, profile, resolution, navigation,
  relationship evidence, route context, source packet, trail, symbol, and
  snippet sections.
- `affected` JSON must report matched/unmatched paths, graph depth, reason,
  confidence, route evidence, blind spots, and next commands.
- Do not introduce broad async runtime migration, unbounded parallelism, or
  parallel Cargo verification without a fresh candidate-gate record.

## Current Parallelization Decision

No new parallelization is promoted by this branch. The measured paths are warm
read paths over SQLite, hybrid search, source reads, and bounded graph traversal.
The next performance candidate should start with query-level search profiling,
because the search warm-read path has the highest current max latency.
