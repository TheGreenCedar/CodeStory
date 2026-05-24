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
4. Start broad repo, product, architecture, review, and planning questions with
   `packet`. Start exact target discovery with `search --why`.
5. Use `context` only after selecting one concrete target by id, query, or
   bookmark. Do not pass broad natural-language questions directly to `context`.
6. Use `symbol`, `trail --story --hide-speculative`, `snippet`, and `explore`
   for source-backed detail around a selected target.
7. Use `files` before claiming language/path coverage, and `affected` before
   choosing regression tests from changed files.
8. Use `drill` or `drill-suite` for repeatable answer-quality checks that need
   source-truth verification artifacts.
9. Keep ordinary grounding CLI-first. Use `serve --stdio` only for warm
   transport, protocol, or stdio integration work.

## Evidence Rules

- Treat CodeStory output as evidence, then open only files needed for edits,
  source-truth verification, or user-requested worktree proof.
- When `packet` reports `sufficient` and `follow_up_commands` is empty, answer
  from the packet; budget truncation alone is not a gap. Preserve supported-claim
  wording and include a compact "Support files" list from `answer.citations` and
  `sufficiency.avoid_opening`.
- When `search --why` emits `search_plan`, use its subqueries, anchor groups,
  bridge evidence, next commands, and source-truth checks as the follow-up plan,
  not as final answer prose.
- For `drill`, read `drill-summary.json` and `evidence_packet.readiness` before
  drafting. Keep `partial`, `inferred`, and `needs_source_read` claims uncertain
  until the source-truth checklist has been completed.
- Treat repo-text, semantic suggestions, speculative OpenAPI edges, and
  cross-language framework hits as navigation hints until typed graph evidence,
  snippets, trails, or direct source reads support the claim.
- If `doctor` reports semantic retrieval as partial, stale, or failed, prefer
  `search --repo-text on --why`, `symbol`, `trail`, and `snippet` until a full
  refresh and embedding setup restore healthy retrieval.

## Command Routing

- Setup and health: `setup embeddings`, `doctor`, `index`, `ground`.
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
- [ground](references/ground.md) - Compact codebase context snapshot
- [doctor](references/doctor.md) - Read-only project/cache/index/retrieval health check
- [packet](references/packet.md) - Broad task packet with sufficiency contract
- [search](references/search.md) - Search indexed symbols and repo text
- [context](references/context.md) - Deep evidence packet for a concrete target
- [symbol](references/symbol.md) - Inspect a symbol's details and relationships
- [trail](references/trail.md) - Follow a symbol's call/reference graph
- [snippet](references/snippet.md) - Fetch source code context around a symbol
- [drill](references/drill.md) - Build a repeatable evidence packet for agent-grounding drills
- [drill-suite](references/drill-suite.md) - Run a manifest-defined cross-repo real-repo agent drill matrix
- [query](references/query.md) - Structured graph query pipelines
- [explore](references/explore.md) - Interactive terminal exploration with Markdown/JSON fallback
- [files](references/files.md) - Indexed file inventory and coverage markers
- [affected](references/affected.md) - Changed-file impact analysis
- [bookmark](references/bookmark.md) - Save reusable investigation focus nodes
- [setup](references/setup.md) - Managed embedding setup
- [serve](references/serve.md) - Local integration surface outside the normal CLI-navigation docs/spec workflow
