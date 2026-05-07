---
name: codestory-grounding
description: Ground repository answers and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, run a graph query, or fetch a snippet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before answering architecture, navigation, or implementation questions.

## Workflow

1. Build the CLI first with `cargo build --release -p codestory-cli` when verification depends on local code changes.
2. If `target/release/codestory-cli(.exe)` is missing, build it with `cargo build --release -p codestory-cli`. If it already exists and is fresh enough for the code you are verifying, use it directly.
3. Run `target/release/codestory-cli(.exe) index --project <workspace> --refresh full` when validating fixes for prior indexing errors, schema/version changes, or graph/query-rule changes. Use `--refresh none` only after a successful fresh build and successful index run in the same verification session.
4. Run `target/release/codestory-cli(.exe) ground --project <workspace>` for a compact context snapshot, then use `search`, `symbol`, `trail`, `query`, or `snippet` to narrow focus.
5. Treat command output as evidence, then open only the files needed for edits or verification.

## LLM Default Browser Loop

Use this short loop before making repo claims or edits:

1. `doctor --project <workspace>` to check project/cache/index/retrieval health.
2. `index --project <workspace> --refresh full` if doctor reports no cache/index, stale failures, or a schema/indexing change needs proof; otherwise keep read commands on `--refresh none`.
3. `ground --project <workspace>` for the compact map.
4. `search --project <workspace> --query "<symbol or question>" --why` to find candidate evidence and ranking reasons.
5. Use `symbol`, `trail`, `snippet`, or `explore` on the best node/query to verify definitions, relationships, and code.
6. Use `ask --project <workspace> "<question>"` only after evidence is indexed; keep citations/evidence enabled unless the user asks for a terse handoff.

## Freshness Rules

- Workspace crates: `codestory-contracts`, `codestory-workspace`, `codestory-store`, `codestory-indexer`, `codestory-runtime`, `codestory-cli`, and `codestory-bench`.
- Binary freshness: rebuild `codestory-cli` after changing `crates/codestory-cli`, `crates/codestory-runtime`, `crates/codestory-contracts`, `crates/codestory-workspace`, `crates/codestory-indexer`, `crates/codestory-store`, or shared CLI-facing types.
- Index freshness: use `index --refresh full` when checking whether historical indexing failures are actually gone. Incremental runs can leave stale error rows if the affected files are not reprocessed.
- Query freshness: use `search` or `trail` with `--refresh none` only after the index has just been rebuilt successfully in the same session.

## Result Interpretation

- `search` can return both typed symbol hits and `[unknown]` usage-like hits for the same name. Prefer the typed hit when verifying symbol surfacing.
- `search` may include `did_you_mean` suggestions when semantic retrieval found close matches but lexical lookup did not. Treat these as navigation hints, not exact matches.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- OpenAPI schema files index endpoint symbols such as `GET /api/users`; client literal calls can create speculative edges to those endpoints, so check certainty before treating a frontend/backend trail as verified.
- Markdown snippets can use ANSI syntax highlighting in interactive terminals. Prefer `--output-file` or JSON when you need machine-stable text.
- If `index` still reports errors after a fix, rerun with `--refresh full` before concluding the fix failed.

## Prerequisite

Use the release binary directly. This skill requires a local Rust toolchain and a built `target/release/codestory-cli(.exe)` binary. If the binary is missing, or stale relative to CLI-facing code changes, run `cargo build --release -p codestory-cli` from the repo root before querying. Prefer the built binary over `cargo run --release` for repeated queries because it avoids repeated Cargo startup and shared build-lock contention on this workspace.

- `target/release/codestory-cli(.exe) index`: Index symbols, edges, and files via tree-sitter + semantic resolution
- `target/release/codestory-cli(.exe) ground`: Produce a compact codebase context snapshot
- `target/release/codestory-cli(.exe) search`: Search indexed symbols and repo text
- `target/release/codestory-cli(.exe) symbol`: Inspect a single symbol's details, children, and relationships
- `target/release/codestory-cli(.exe) trail`: Follow a symbol's call/reference graph as a directed trail
- `target/release/codestory-cli(.exe) snippet`: Fetch source code context around a symbol
- `target/release/codestory-cli(.exe) query`: Run a piped graph query such as `trail(symbol: 'Foo') | filter(kind: function)`
- `target/release/codestory-cli(.exe) explore`: Open a terminal explorer or emit a bundled symbol/trail/snippet report
- `target/release/codestory-cli(.exe) bookmark`: Save, list, or remove investigation focus bookmarks
- `target/release/codestory-cli(.exe) setup embeddings`: Install and start managed llama.cpp/BGE-base embedding assets; defaults to Vulkan, with `--variant cpu` as fallback
- `target/release/codestory-cli(.exe) serve`: Expose local HTTP JSON or stdio tool access for agents and editor integrations

Always pass `--project <workspace>` explicitly so queries target the intended checkout even when you invoke the binary from the repo root. If a subcommand is unavailable in the current checkout, report that plainly and fall back to direct repo inspection instead of inventing grounded results.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) — Build or refresh the symbol index
- [ground](references/ground.md) — Compact codebase context snapshot
- [doctor](references/doctor.md) — Read-only project/cache/index/retrieval health check
- [search](references/search.md) — Search indexed symbols and repo text
- [symbol](references/symbol.md) — Inspect a symbol's details and relationships
- [trail](references/trail.md) — Follow a symbol's call/reference graph
- [snippet](references/snippet.md) — Fetch source code context around a symbol
- [query](references/query.md) — Structured graph query pipelines: `trail`, `symbol`, `search`, `filter`, and `limit`
- [explore](references/explore.md) — Interactive terminal exploration with Markdown/JSON fallback
- [bookmark](references/bookmark.md) — Save reusable investigation focus nodes
- [ask](references/ask.md) — DB-first answer packet with citations and optional handoff bundle
- [setup](references/setup.md) — Managed llama.cpp/BGE-base embedding setup
- [serve](references/serve.md) — Local HTTP JSON API or stdio tool protocol
