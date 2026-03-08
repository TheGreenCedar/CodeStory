---
name: codestory-grounding
description: Ground repository answers and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, or fetch a snippet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before answering architecture, navigation, or implementation questions.

## Workflow

1. Build the CLI first with `cargo build -p codestory-cli` when verification depends on local code changes.
2. Use the built binary for repeated queries: `target/debug/codestory-cli(.exe) <subcommand> ...`. The repo-local Python wrappers now prefer the built binary automatically.
3. Run `python scripts/index.py --refresh full` when validating fixes for prior indexing errors, schema/version changes, or graph/query-rule changes. Use `--refresh none` only after a successful fresh build and successful index run in the same verification session.
4. Run `python scripts/ground.py` for a compact context snapshot, then use `search`, `symbol`, `trail`, or `snippet` to narrow focus.
5. Treat command output as evidence, then open only the files needed for edits or verification.

## Freshness Rules

- Binary freshness: rebuild `codestory-cli` after changing `crates/codestory-cli`, `crates/codestory-app`, `crates/codestory-index`, `crates/codestory-storage`, or shared CLI-facing types.
- Index freshness: use `index --refresh full` when checking whether historical indexing failures are actually gone. Incremental runs can leave stale error rows if the affected files are not reprocessed.
- Query freshness: use `search` or `trail` with `--refresh none` only after the index has just been rebuilt successfully in the same session.

## Result Interpretation

- `search` can return both typed symbol hits and `[unknown]` usage-like hits for the same name. Prefer the typed hit when verifying symbol surfacing.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- If `index` still reports errors after a fix, rerun with `--refresh full` before concluding the fix failed.

## Scripts

These scripts are repo-local wrappers around the built `target/debug/codestory-cli(.exe)` binary and require Python plus a local Rust toolchain. If the binary is missing, the wrapper builds it once with `cargo build -p codestory-cli`. `cargo run` remains a last-resort fallback and is slower because it can contend on Cargo locks. Invoke the wrappers as `python scripts/...` on Windows.

- `python scripts/index.py`: Index symbols, edges, and files via tree-sitter + semantic resolution (builds/refreshes the SQLite index)
- `python scripts/ground.py`: Produce a compact codebase context snapshot — root symbols, file coverage, and recommended queries (requires index)
- `python scripts/search.py`: Search indexed symbols and repo text (requires index)
- `python scripts/symbol.py`: Inspect a single symbol's details, children, and relationships (requires index)
- `python scripts/trail.py`: Follow a symbol's call/reference graph as a directed trail (requires index)
- `python scripts/snippet.py`: Fetch source code context around a symbol (requires index)

Pass extra arguments through unchanged. The scripts resolve the workspace root automatically, so they can be launched from anywhere inside the repo checkout.

Use `--dry-run` first if you only need to inspect the exact command the wrapper would execute. When the debug binary exists, the dry run prints the exe command; otherwise it prints the one-time build command followed by the expected exe invocation.

If a subcommand is unavailable in the current checkout, report that plainly and fall back to direct repo inspection instead of inventing grounded results.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) — Build or refresh the symbol index
- [ground](references/ground.md) — Compact codebase context snapshot
- [search](references/search.md) — Search indexed symbols and repo text
- [symbol](references/symbol.md) — Inspect a symbol's details and relationships
- [trail](references/trail.md) — Follow a symbol's call/reference graph
- [snippet](references/snippet.md) — Fetch source code context around a symbol
