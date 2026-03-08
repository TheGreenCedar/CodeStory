---
name: codestory-grounding
description: Ground repository answers and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, or fetch a snippet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before answering architecture, navigation, or implementation questions.

## Workflow

1. Build the CLI first with `cargo build --release -p codestory-cli` when verification depends on local code changes.
2. If `target/release/codestory-cli(.exe)` is missing, build it with `cargo build --release -p codestory-cli`. If it already exists and is fresh enough for the code you are verifying, use it directly.
3. Run `target/release/codestory-cli(.exe) index --project <workspace> --refresh full` when validating fixes for prior indexing errors, schema/version changes, or graph/query-rule changes. Use `--refresh none` only after a successful fresh build and successful index run in the same verification session.
4. Run `target/release/codestory-cli(.exe) ground --project <workspace>` for a compact context snapshot, then use `search`, `symbol`, `trail`, or `snippet` to narrow focus.
5. Treat command output as evidence, then open only the files needed for edits or verification.

## Freshness Rules

- Binary freshness: rebuild `codestory-cli` after changing `crates/codestory-cli`, `crates/codestory-app`, `crates/codestory-index`, `crates/codestory-storage`, or shared CLI-facing types.
- Index freshness: use `index --refresh full` when checking whether historical indexing failures are actually gone. Incremental runs can leave stale error rows if the affected files are not reprocessed.
- Query freshness: use `search` or `trail` with `--refresh none` only after the index has just been rebuilt successfully in the same session.

## Result Interpretation

- `search` can return both typed symbol hits and `[unknown]` usage-like hits for the same name. Prefer the typed hit when verifying symbol surfacing.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- If `index` still reports errors after a fix, rerun with `--refresh full` before concluding the fix failed.

## Prerequisite

Use the release binary directly. This skill requires a local Rust toolchain and a built `target/release/codestory-cli(.exe)` binary. If the binary is missing, or stale relative to CLI-facing code changes, run `cargo build --release -p codestory-cli` from the repo root before querying. Prefer the built binary over `cargo run --release` for repeated queries because it avoids repeated Cargo startup and shared build-lock contention on this workspace.

- `target/release/codestory-cli(.exe) index`: Index symbols, edges, and files via tree-sitter + semantic resolution
- `target/release/codestory-cli(.exe) ground`: Produce a compact codebase context snapshot
- `target/release/codestory-cli(.exe) search`: Search indexed symbols and repo text
- `target/release/codestory-cli(.exe) symbol`: Inspect a single symbol's details, children, and relationships
- `target/release/codestory-cli(.exe) trail`: Follow a symbol's call/reference graph as a directed trail
- `target/release/codestory-cli(.exe) snippet`: Fetch source code context around a symbol

Always pass `--project <workspace>` explicitly so queries target the intended checkout even when you invoke the binary from the repo root. If a subcommand is unavailable in the current checkout, report that plainly and fall back to direct repo inspection instead of inventing grounded results.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) — Build or refresh the symbol index
- [ground](references/ground.md) — Compact codebase context snapshot
- [search](references/search.md) — Search indexed symbols and repo text
- [symbol](references/symbol.md) — Inspect a symbol's details and relationships
- [trail](references/trail.md) — Follow a symbol's call/reference graph
- [snippet](references/snippet.md) — Fetch source code context around a symbol
