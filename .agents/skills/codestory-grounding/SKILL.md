---
name: codestory-grounding
description: Use when repository claims, plans, reviews, edits, or test choices need source-backed context before answering or changing code; use extensively.
---

# CodeStory Grounding

Use this skill after it is installed in the agent's global skill directory.
Resolve two values before running commands: `<codestory-cli>` is the tool binary,
and `<target-workspace>` is the repository being grounded. The CodeStory source
checkout is only the tool artifact unless the user is editing CodeStory itself.

## Core Loop

1. Prefer `CODESTORY_CLI`, then `codestory-cli` on `PATH`; run `scripts/setup.ps1`
   or `scripts/setup.sh` from this skill only when no binary is available.
2. Always pass `--project <target-workspace>` explicitly.
3. Use `doctor` when cache, freshness, or retrieval health matters; use
   `index --refresh full` for first runs or historical indexing failures, and
   incremental refreshes during normal edit loops.
4. For retrieval rollout, sidecar, runtime, CLI, benchmark, or smoke-CI work,
   choose the proof layer from `references/retrieval-rollout.md` before running
   broad or expensive verification.
5. Start broad repo, product, architecture, review, and planning questions with
   `packet`. Start exact target discovery with `search --why`.
6. Use `context` only after selecting one concrete target by id, query, or
   bookmark. Do not pass broad natural-language questions directly to `context`.
7. Use `symbol`, `trail --story --hide-speculative`, `snippet`, and `explore`
   for source-backed detail around a selected target.
8. Use `files` before claiming language/path coverage, and `affected` before
   choosing regression tests from changed files.
9. Use `drill` or `drill-suite` for repeatable answer-quality checks that need
   source-truth verification artifacts.
10. Keep ordinary grounding CLI-first. Use `serve --stdio` only for warm
   transport, protocol, or stdio integration work.

## Evidence Rules

- Treat CodeStory output as evidence, then open only files needed for edits,
  source-truth verification, or user-requested worktree proof.
- When `packet` reports `sufficient` and `follow_up_commands` is empty, answer
  from the packet; budget truncation alone is not a gap. Preserve supported-claim
  wording and exact source identifiers from `sufficiency.covered_claims` and
  citation display names. Do not merge repeated exact anchors into shorthand that
  drops required prefixes; write each exact anchor independently when naming
  declarations, tables, symbols, selectors, or other source-defined terms. Include
  a compact "Support files" list from `answer.citations` and
  `sufficiency.avoid_opening_paths`. The older `sufficiency.avoid_opening` field
  is human-readable compatibility prose, not the raw path contract. Do not run
  ordinary source reads, `rg`, `grep`, or `git show` only to verify packet
  citations; run more commands only for a named unresolved gap, an edit target,
  or a user-requested worktree proof.
- When `packet` reports `partial`, read `sufficiency.follow_up_commands` and run
  those commands in order. Prefer listed targeted `search --why` commands before
  escalating to a larger packet budget. As soon as a follow-up packet becomes
  sufficient, stop exploration and answer from that packet.
- When `search --why` emits `search_plan`, use its subqueries, anchor groups,
  bridge evidence, next commands, and source-truth checks as the follow-up plan,
  not as final answer prose.
- For `drill`, read `drill-summary.json` and `evidence_packet.readiness` before
  drafting. Keep `partial`, `inferred`, and `needs_source_read` claims uncertain
  until the source-truth checklist has been completed.
- Treat repo-text, semantic suggestions, speculative OpenAPI edges, and
  cross-language framework hits as navigation hints until typed graph evidence,
  snippets, trails, or direct source reads support the claim.
- If `doctor` reports retrieval as partial, stale, stubbed, hash-vector, or
  failed, treat product retrieval as unavailable until `retrieval_mode=full` is
  restored. Repo-text output is diagnostic only; do not use it as a substitute
  for mandatory sidecar evidence.
- Under `graph_first_v1`, `retrieval_mode=full` means graph and lexical sidecars
  are complete, generated `symbol_search_doc` and component-report virtual docs
  are current, and Qdrant is complete only for selected dense anchors. A zero
  dense-anchor manifest is valid only when reported explicitly; otherwise
  Qdrant mismatch or unavailability is fail-closed. Search evidence should name
  provenance such as `exact`, `lexical_source`, `symbol_doc`, `graph_neighbor`,
  `component_report`, or `dense_anchor`.

## Command Routing

- Setup and health: `setup embeddings`, `doctor`, `index`, `ground`, `cache rehydrate`.
- Broad task packet: `packet`; answer from it when sufficient, otherwise follow
  the named follow-up commands.
- Candidate discovery: `search --why`; choose concrete ids before `context`.
- Focused source view: `symbol`, `trail`, `snippet`, `explore`, `context`.
- Coverage and impact: `files`, `affected`.
- Reusable focus: `bookmark`.
- Structured or repeatable evaluation: `query`, `drill`, `drill-suite`.
- Local integration surface: `serve`.

When detailed flags, output fields, examples, or troubleshooting rules are
needed, load only the relevant command reference below.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) - Build or refresh the symbol index
- [cache](references/cache.md) - Reuse compatible CodeStory caches across worktrees
- [ground](references/ground.md) - Compact codebase context snapshot
- [doctor](references/doctor.md) - Read-only project/cache/index/retrieval health check
- [packet](references/packet.md) - Broad task packet with sufficiency contract
- [search](references/search.md) - Search mandatory sidecar indexes
- [context](references/context.md) - Deep evidence packet for a concrete target
- [symbol](references/symbol.md) - Inspect a symbol's details and relationships
- [trail](references/trail.md) - Follow a symbol's call/reference graph
- [snippet](references/snippet.md) - Fetch source code context around a symbol
- [drill](references/drill.md) - Build a repeatable evidence packet for agent-grounding drills
- [drill-suite](references/drill-suite.md) - Run a manifest-defined cross-repo real-repo agent drill matrix
- [query](references/query.md) - Structured graph query pipelines
- [explore](references/explore.md) - Interactive terminal exploration with Markdown/JSON output
- [files](references/files.md) - Indexed file inventory and coverage markers
- [affected](references/affected.md) - Changed-file impact analysis
- [bookmark](references/bookmark.md) - Save reusable investigation focus nodes
- [setup](references/setup.md) - Managed embedding setup
- [retrieval-rollout](references/retrieval-rollout.md) - Proof table for retrieval rollout layers, sidecar repair, benchmark gates, and CI smoke triage
- [serve](references/serve.md) - Local integration surface outside the normal CLI-navigation docs/spec workflow
