# Review Action Plan

This page is the durable summary of the branch review/remediation trail. Temporary agent execution plans were consolidated here so contributor docs keep the durable decisions without preserving branch scratchpads.

## Current Merge Bar

- Production packet/search code must not depend on benchmark holdout literals,
  benchmark repo names, fixture paths, or expected-answer shapes.
- Eval probes must stay disabled outside test builds.
- Agent packet/search readiness must report full sidecar retrieval, not semantic-only fallback.
- Language support claims must distinguish parser-backed graph coverage, structural collectors, and agent-facing packet quality.
- Repo-scale e2e stats must be recorded in `docs/testing/codestory-e2e-stats-log.md`.

## Branch Result

- Exact Requests/Express and row-shaped benchmark-family behavior moved behind the test-only eval-probe boundary.
- Production generalization lint now guards compact marker and holdout-family literals.
- Runtime and CLI language filtering now use the shared language-support registry where user-visible behavior should follow support claims.
- Runtime packet steering now lives in named term, source-pattern, claim,
  product-profile, command-profile, evidence-role, citation-helper,
  required-probe, citation-capping, and sufficiency modules instead of generic
  orchestration branches.
- Packet evidence roles now use a typed internal role abstraction; user-facing
  labels are emitted only at markdown/trace/claim-key boundaries.
- Indexing-flow required probes are generic product concepts, not exact
  CodeStory method-name anchors; exact local symbols remain valid citations and
  tests, but they are not production steering requirements.
- Search-execution probes and product claims are generic product concepts, not
  ripgrep holdout answer templates; exact search-pipeline wording remains
  eval/benchmark-only.
- Final proof should use fresh `ready` and `doctor` output after any docs-only proof edits, because docs change the sidecar input hash.

## Follow-Ups

- Continue splitting `crates/codestory-runtime/src/agent/orchestrator.rs` by
  moving the remaining flow-template collectors and packet tests behind named
  packet modules.
- Add semantic-resolution buckets and cross-file evidence for newer parser-backed languages before claiming every language is first-class in agent packet quality.
- Keep legitimate framework/domain heuristics in named profiles or collectors as coverage broadens.
