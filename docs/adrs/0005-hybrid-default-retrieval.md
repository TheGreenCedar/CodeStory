# ADR 0005: Hybrid Retrieval Defaults With Visible Fallback

## Current State

`codestory-runtime` already owns lexical search, semantic search-doc projection, graph-aware reranking, and the CLI-facing grounding services. Before this change, the runtime only enabled hybrid retrieval when `CODESTORY_HYBRID_RETRIEVAL_ENABLED` was explicitly turned on, and the CLI `search` surface still used symbolic-only ranking by default.

## Target State

Hybrid retrieval is the intended default for OSS users whenever local embedding assets are present. The six existing CLI workflows stay stable, but the runtime and CLI surfaces make retrieval state visible so users can tell whether results came from hybrid or symbolic ranking and why fallback happened.

## Decision

Keep SQLite-backed graph grounding and semantic symbol docs as the local source of truth, default hybrid retrieval on, and expose retrieval mode plus fallback metadata through existing DTO and CLI surfaces instead of introducing new verbs.

## Consequences

- `index`, `ground`, and `search` report retrieval mode and semantic-doc readiness.
- `search` now uses hybrid ranking when available and symbolic ranking only as an explicit fallback.
- Semantic symbol docs are richer so natural-language retrieval has better behavioral context without changing the public workflow set.
- Release gating now includes `cargo test -p codestory-runtime --test retrieval_eval` for exact-symbol, natural-language, and grounding or trail trust checks.
