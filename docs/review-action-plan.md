# Review Action Plan

This page is the durable summary of the branch review/remediation trail. Temporary agent execution plans were consolidated here so contributor docs keep the durable decisions without preserving branch scratchpads.

## Current Merge Bar

- Production packet/search code must not depend on benchmark holdout literals or exact-family source steering.
- Eval probes must stay disabled outside test builds.
- Agent packet/search readiness must report full sidecar retrieval, not semantic-only fallback.
- Language support claims must distinguish parser-backed graph coverage, structural collectors, and agent-facing packet quality.
- Repo-scale e2e stats must be recorded in `docs/testing/codestory-e2e-stats-log.md`.

## Branch Result

- Exact Requests/Express and row-shaped benchmark-family behavior moved behind the test-only eval-probe boundary.
- Production generalization lint now guards compact marker and holdout-family literals.
- Runtime and CLI language filtering now use the shared language-support registry where user-visible behavior should follow support claims.
- Final proof should use fresh `ready` and `doctor` output after any docs-only proof edits, because docs change the sidecar input hash.

## Follow-Ups

- Split `crates/codestory-runtime/src/agent/orchestrator.rs` into packet planning, source-claim synthesis, sufficiency, and tests.
- Add semantic-resolution buckets and cross-file evidence for newer parser-backed languages before claiming every language is first-class in agent packet quality.
- Replace remaining product/framework-shaped routing heuristics with generic structural layers where practical.
