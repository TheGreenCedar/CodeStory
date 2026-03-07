---
name: codestory-grounding
description: Ground repository answers and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, or fetch a snippet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before answering architecture, navigation, or implementation questions.

## Workflow

1. Run `python scripts/index.py` to build or refresh the symbol/edge index. Run `python scripts/ground.py` to get a compact codebase context snapshot.
2. `search`, `symbol`, `trail`, or `snippet` require an existing index or an explicit `--refresh auto`. Use these to narrow your focus.
3. Treat command output as evidence, then open only the files needed for edits or verification.

## Scripts

These scripts are repo-local wrappers around `cargo run -p codestory-cli -- ...` and require Python and Cargo to be installed. They should be invoked as `python scripts/...` on Windows.

- `python scripts/index.py`: Index symbols, edges, and files via tree-sitter + semantic resolution (builds/refreshes the SQLite index)
- `python scripts/ground.py`: Produce a compact codebase context snapshot — root symbols, file coverage, and recommended queries (requires index)
- `python scripts/search.py`: Search indexed symbols and repo text (requires index)
- `python scripts/symbol.py`: Inspect a single symbol's details, children, and relationships (requires index)
- `python scripts/trail.py`: Follow a symbol's call/reference graph as a directed trail (requires index)
- `python scripts/snippet.py`: Fetch source code context around a symbol (requires index)

Pass extra arguments through unchanged. The scripts resolve the workspace root automatically, so they can be launched from anywhere inside the repo checkout.

Use `--dry-run` first if you only need to inspect the exact `cargo run -p codestory-cli -- <subcommand> ...` command that would execute.

If a subcommand is unavailable in the current checkout, report that plainly and fall back to direct repo inspection instead of inventing grounded results.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) — Build or refresh the symbol index
- [ground](references/ground.md) — Compact codebase context snapshot
- [search](references/search.md) — Search indexed symbols and repo text
- [symbol](references/symbol.md) — Inspect a symbol's details and relationships
- [trail](references/trail.md) — Follow a symbol's call/reference graph
- [snippet](references/snippet.md) — Fetch source code context around a symbol
