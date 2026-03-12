# ADR 0001: V2 Boundaries

## Status

Accepted.

## Decision

The workspace is organized around six owning layers plus benches:

- `codestory-contracts`
- `codestory-workspace`
- `codestory-store`
- `codestory-indexer`
- `codestory-runtime`
- `codestory-cli`
- `codestory-bench`

## Rationale

This split keeps source-of-truth responsibilities narrow:

- contracts owns shared types
- workspace owns discovery and refresh planning
- store owns persistence and snapshots
- indexer owns parse/extract/resolve behavior
- runtime owns orchestration and search
- CLI owns argument parsing and rendering

## Consequences

- New shared DTOs or graph/event types must land in `codestory-contracts`.
- New refresh logic starts in `codestory-workspace`, not runtime.
- New persistence behavior starts in `codestory-store`.
- New indexing or semantic work starts in `codestory-indexer`.
- New workflow orchestration starts in `codestory-runtime`.
- New commands or output formats start in `codestory-cli`.
