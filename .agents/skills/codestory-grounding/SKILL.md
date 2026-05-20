---
name: codestory-grounding
description: Ground repository claims and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, run a graph query, fetch a snippet, or build a deep context packet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before making architecture, navigation, or implementation claims.

## Command Roles

- `doctor`: read-only health check for project, cache, index, retrieval, semantic readiness, freshness, and relevant environment settings.
- `index`: build or refresh the SQLite graph, search, snapshot, and semantic cache.
- `ground`: broad repo-level orientation snapshot; use `--why` for coverage, retrieval mode, gaps, and next commands.
- `search`: lightweight candidate discovery for symbols, files, literals, API paths, modules, or behavior terms; use `--why` for ranking reasons.
- `context`: deep evidence/context bundle for one concrete target selected by `--id`, `--query`, or `--bookmark`; it is not question answering and does not interpret natural-language questions.
- `symbol`: inspect one exact symbol and relationships.
- `trail`: follow caller, callee, and reference graph around a symbol; use `--story --hide-speculative` for readable flow evidence.
- `snippet`: fetch source context around a symbol.
- `drill`: run a deterministic agent-grounding packet for a natural-language question and concrete anchors, including search/symbol/trail/snippet artifacts and a source-verification checklist.
- `query`: run structured graph-query pipelines.
- `explore`: interactive or bundled navigation view around a target.
- `bookmark`: save, list, or remove investigation focus nodes.
- `setup embeddings`: install managed embedding assets.
- `serve`: expose HTTP or stdio read-only browser surfaces.

## Core Rules

1. Build the CLI first with `cargo build --release -p codestory-cli` when verification depends on local code changes.
2. Use the release binary directly once fresh: `target/release/codestory-cli(.exe)`.
3. Always pass `--project <workspace>` explicitly so queries target the intended checkout.
4. Use `search` for lightweight candidate discovery. Use `context` only after you have one concrete retrieval target.
5. Do not pass broad product or architecture questions to `context`. Break broad questions into concrete terms, choose anchors, then run `context --id <node-id>`.
6. Treat command output as evidence, then open only the files needed for edits or verification.

## Template Workflows

### Fresh repo orientation

```
target/release/codestory-cli(.exe) doctor --project <workspace>
target/release/codestory-cli(.exe) index --project <workspace> --refresh full
target/release/codestory-cli(.exe) ground --project <workspace> --why
target/release/codestory-cli(.exe) search --project <workspace> --query "<architecture term>" --why
```

### Candidate-to-context workflow

```
target/release/codestory-cli(.exe) search --project <workspace> --query "<symbol/file/literal/API path>" --why
# choose a concrete node_id from search output
target/release/codestory-cli(.exe) context --project <workspace> --id <node-id>
```

### Exact symbol investigation

```
target/release/codestory-cli(.exe) symbol --project <workspace> --id <node-id>
target/release/codestory-cli(.exe) trail --project <workspace> --id <node-id> --story --hide-speculative
target/release/codestory-cli(.exe) snippet --project <workspace> --id <node-id> --context 40
target/release/codestory-cli(.exe) context --project <workspace> --id <node-id> --bundle out/context-<name>
```

### Broad repo/product question workflow

Do not pass the broad question to `context`.

```
target/release/codestory-cli(.exe) ground --project <workspace> --why
target/release/codestory-cli(.exe) search --project <workspace> --repo-text on --query "<concrete term>" --why
target/release/codestory-cli(.exe) search --project <workspace> --repo-text on --query "<another concrete term>" --why
# select anchors
target/release/codestory-cli(.exe) context --project <workspace> --id <node-id>
```

### Stale or unhealthy semantic retrieval

```
target/release/codestory-cli(.exe) doctor --project <workspace>
target/release/codestory-cli(.exe) setup embeddings --project <workspace>
target/release/codestory-cli(.exe) index --project <workspace> --refresh full
target/release/codestory-cli(.exe) doctor --project <workspace>
```

If retrieval is still partial, stale, or failed, use `search --repo-text on --why`, `symbol`, `trail`, and `snippet`; treat `context` output as incomplete if it reports gaps.

## Freshness Rules

- Workspace crates: `codestory-contracts`, `codestory-workspace`, `codestory-store`, `codestory-indexer`, `codestory-runtime`, `codestory-cli`, and `codestory-bench`.
- Binary freshness: rebuild `codestory-cli` after changing `crates/codestory-cli`, `crates/codestory-runtime`, `crates/codestory-contracts`, `crates/codestory-workspace`, `crates/codestory-indexer`, `crates/codestory-store`, or shared CLI-facing types.
- Index freshness: use `index --refresh full` when checking whether historical indexing failures are actually gone. Incremental runs can leave stale error rows if affected files are not reprocessed.
- Query freshness: use read commands with `--refresh none` only after the index has just been rebuilt successfully in the same session.
- Context freshness: do not treat `context` output as authoritative while `doctor` reports semantic partial, stale, or failed. Use lexical search plus repo-text fallback and focused snippets/trails until semantics are rebuilt.
- Skill-only manual tests: do not run `git status`, open docs, or inspect files directly unless the user asks for worktree evidence or CodeStory command output is insufficient for a specific edit.

## Result Interpretation

- `search` can return both typed symbol hits and `[unknown]` usage-like hits for the same name. Prefer the typed hit when verifying symbol surfacing.
- `search` may include `did_you_mean` suggestions when semantic retrieval found close matches but lexical lookup did not. Treat these as navigation hints, not exact matches.
- `context --query` first resolves the query to a concrete target. If the target is ambiguous, use `search --why`, then rerun `context --id`.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- OpenAPI schema files index endpoint symbols such as `GET /api/users`; client literal calls can create speculative edges to those endpoints, so check certainty before treating a frontend/backend trail as verified.
- Markdown snippets can use ANSI syntax highlighting in interactive terminals. Prefer `--output-file` or JSON when you need machine-stable text.
- Snippet output reports the requested context and byte cap; when `snippet_truncated` is true, increasing `--context` may not expand output unless the byte cap also changes in code.
- If `index` still reports errors after a fix, rerun with `--refresh full` before concluding the fix failed.

## References

Detailed argument tables, output examples, and usage patterns for each command:

- [index](references/index.md) - Build or refresh the symbol index
- [ground](references/ground.md) - Compact codebase context snapshot
- [doctor](references/doctor.md) - Read-only project/cache/index/retrieval health check
- [search](references/search.md) - Search indexed symbols and repo text
- [context](references/context.md) - Deep evidence packet for a concrete target
- [symbol](references/symbol.md) - Inspect a symbol's details and relationships
- [trail](references/trail.md) - Follow a symbol's call/reference graph
- [snippet](references/snippet.md) - Fetch source code context around a symbol
- [drill](references/drill.md) - Build a repeatable evidence packet for agent-grounding drills
- [query](references/query.md) - Structured graph query pipelines
- [explore](references/explore.md) - Interactive terminal exploration with Markdown/JSON fallback
- [bookmark](references/bookmark.md) - Save reusable investigation focus nodes
- [setup](references/setup.md) - Managed embedding setup
- [serve](references/serve.md) - Local HTTP JSON API or stdio tool protocol
