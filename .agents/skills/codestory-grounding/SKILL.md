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
- `drill`: run a deterministic agent-grounding packet for a natural-language question and concrete anchors, including search/symbol/trail/explore/snippet artifacts, bridge evidence, an Evidence Packet, Answer Readiness report, claim-ledger template, and source-verification checklist.
- `query`: run structured graph-query pipelines.
- `explore`: interactive or bundled navigation view around a target, including grouped line-numbered source packets.
- `files`: list indexed file inventory, language counts, inferred source/test/generated/vendor roles, and partial-index markers.
- `affected`: map changed files to impacted symbols and likely test files using indexed graph dependents.
- `bookmark`: save, list, or remove investigation focus nodes.
- `setup embeddings`: install managed embedding assets.
- `serve`: local integration surface; do not use it for the CLI-navigation docs/spec workflow unless the user explicitly asks for transport work.

## Core Rules

1. Build the CLI first with `cargo build --release -p codestory-cli` when verification depends on local code changes.
2. Use the release binary directly once fresh: `target/release/codestory-cli(.exe)`.
3. Always pass `--project <workspace>` explicitly so queries target the intended checkout.
4. Use `search` for lightweight candidate discovery. Use `context` only after you have one concrete retrieval target.
5. Use `explore` when one call should collect resolution, relationships, source slices, related files, and coverage notes around a chosen target.
6. Use `files` before claiming coverage for a language/path area, and use `affected` before selecting regression tests for a change.
7. Do not pass broad product or architecture questions to `context`. Break broad questions into concrete terms, choose anchors, then run `context --id <node-id>`.
8. Treat command output as evidence, then open only the files needed for edits or verification.
9. Keep navigation, route coverage, performance, and search-quality work CLI-first. Do not route these workflows through MCP, stdio, HTTP, or server behavior.
10. For architecture answers, read `evidence_packet.readiness` before drafting. Do not present `partial`, `inferred`, or `needs_source_read` claims as verified until the source-truth checklist has been completed.
11. Treat repo-text and cross-language framework evidence as hints unless the packet also includes typed graph evidence, snippets, or source-truth checks.

## Template Workflows

### Fresh repo orientation

```
target/release/codestory-cli(.exe) doctor --project <workspace>
target/release/codestory-cli(.exe) index --project <workspace> --refresh full
target/release/codestory-cli(.exe) ground --project <workspace> --why
target/release/codestory-cli(.exe) search --project <workspace> --query "<architecture term>" --why
target/release/codestory-cli(.exe) files --project <workspace> --format markdown
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
target/release/codestory-cli(.exe) explore --project <workspace> --id <node-id> --no-tui
target/release/codestory-cli(.exe) trail --project <workspace> --id <node-id> --story --hide-speculative
target/release/codestory-cli(.exe) snippet --project <workspace> --id <node-id> --context 40
target/release/codestory-cli(.exe) context --project <workspace> --id <node-id> --bundle out/context-<name>
```

### Change impact workflow

```
target/release/codestory-cli(.exe) index --project <workspace> --refresh incremental
target/release/codestory-cli(.exe) affected --project <workspace> --format markdown
# or pass explicit changed paths when git diff is not the right source:
target/release/codestory-cli(.exe) affected --project <workspace> src/lib.rs tests/lib_test.rs --depth 3 --format json
```

### Broad repo/product question workflow

Do not pass the broad question to `context`. Prefer `drill` when the user needs an answer-quality check, not just navigation.

```
target/release/codestory-cli(.exe) ground --project <workspace> --why
target/release/codestory-cli(.exe) search --project <workspace> --repo-text on --query "<concrete term>" --why
target/release/codestory-cli(.exe) search --project <workspace> --repo-text on --query "<another concrete term>" --why
# select anchors
target/release/codestory-cli(.exe) context --project <workspace> --id <node-id>
```

### Real-repo agent-quality drill workflow

Use this workflow when the goal is to test whether CodeStory helps an agent answer a realistic architecture question.

```
target/release/codestory-cli(.exe) drill --project <workspace> --refresh full --question "<question>" --anchors AnchorA,AnchorB,AnchorC --output-dir target/drill/<slug> --format json
# Read drill-report.json first:
# - evidence_packet.readiness.safe_to_say
# - evidence_packet.readiness.inferred_claims
# - evidence_packet.readiness.needs_verification
# - evidence_packet.readiness.source_truth_checks
# Draft the CodeStory-only answer, then open only source files named or implied by source_truth_checks.
```

### Stale or unhealthy semantic retrieval

```
target/release/codestory-cli(.exe) doctor --project <workspace>
target/release/codestory-cli(.exe) setup embeddings --project <workspace>
target/release/codestory-cli(.exe) index --project <workspace> --refresh full
target/release/codestory-cli(.exe) doctor --project <workspace>
```

If retrieval is still partial, stale, or failed, use `search --repo-text on --why`, `symbol`, `trail`, and `snippet`; treat `context` output as incomplete if it reports gaps.

### Route coverage and quality evaluation

```
target/release/codestory-cli(.exe) files --project <workspace> --format json
cargo test -p codestory-indexer --lib framework_route
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-cli --test agent_quality_eval
```

### Performance review baseline

```
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-runtime --test retrieval_eval
```

Capture the baseline before optimization, define the no-regression threshold, and reject broad parallelization unless the exact candidate path is measured as the bottleneck.

## Freshness Rules

- Workspace crates: `codestory-contracts`, `codestory-workspace`, `codestory-store`, `codestory-indexer`, `codestory-runtime`, `codestory-cli`, and `codestory-bench`.
- Binary freshness: rebuild `codestory-cli` after changing `crates/codestory-cli`, `crates/codestory-runtime`, `crates/codestory-contracts`, `crates/codestory-workspace`, `crates/codestory-indexer`, `crates/codestory-store`, or shared CLI-facing types.
- Index freshness: use `index --refresh full` when checking whether historical indexing failures are actually gone. Incremental runs can leave stale error rows if affected files are not reprocessed.
- Query freshness: use read commands with `--refresh none` only after the index has just been rebuilt successfully in the same session.
- Context freshness: do not treat `context` output as authoritative while `doctor` reports semantic partial, stale, or failed. Use lexical search plus repo-text fallback and focused snippets/trails until semantics are rebuilt.
- Skill-only manual tests: do not run `git status`, open docs, or inspect files directly unless the user asks for worktree evidence or CodeStory command output is insufficient for a specific edit.

## Result Interpretation

- Support status vocabulary:
  - `supported`: fixture-backed behavior is passing and the documented coverage floor is met.
  - `heuristic`: useful pattern-backed evidence that needs source review before full support claims.
  - `partial`: some cases are covered, but known patterns, handler links, or fixtures are missing.
  - `unsupported`: no support claim is made for that framework, syntax, language, or path.
  - `stale`: cache or semantic evidence may not match the current workspace; refresh before promoting claims.
  - `non-promotable`: required fixtures, known-gap notes, or eval evidence are missing or failing.
  - `ambiguous`: a query matched multiple plausible targets; rerun `search --why`, then use `--id` or `--file`.
  - `unmatched`: a changed path was not found in the persisted index; confirm with `files --path <fragment>` or refresh.
- `search` can return both typed symbol hits and `[unknown]` usage-like hits for the same name. Prefer the typed hit when verifying symbol surfacing.
- `search` may include `did_you_mean` suggestions when semantic retrieval found close matches but lexical lookup did not. Treat these as navigation hints, not exact matches.
- `context --query` first resolves the query to a concrete target. If the target is ambiguous, use `search --why`, then rerun `context --id`.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- OpenAPI schema files index endpoint symbols such as `GET /api/users`; client literal calls can create speculative edges to those endpoints, so check certainty before treating a frontend/backend trail as verified.
- Framework route symbols include confidence labels. Treat `file_convention` and `decorator` routes as stronger than broad `heuristic` routes, and confirm handler links before claiming an end-to-end route path.
- Framework integration symbols have bounded fixture-backed support. `tauri:command:*` covers first-argument string-literal `invoke` calls, including multiline/generic calls, and Rust `#[tauri::command]`/`generate_handler!` registrations while rejecting argument strings and comments. `payload:collection:*` covers `CollectionConfig` slug blocks and Payload method calls with `collection:` options while rejecting unrelated slugs, props, and string substrings. Treat evidence outside those covered forms as unsupported until source verification proves it.
- `drill` Evidence Packet readiness is the agent-facing contract: `anchored` and `supported` claims may be drafted from CodeStory evidence; `partial`, `inferred`, and `needs_source_read` claims must stay visibly uncertain until source verification completes.
- The agent-quality evaluator is deterministic. Use it to catch unsupported high-confidence claims, overclaims, material source corrections, and confidence-calibration regressions; do not replace it with live LLM judging in CI.
- `files`, `search`, and `explore` can report usable-but-partial indexes. Carry those coverage notes into decisions instead of silently assuming full coverage.
- `affected` is a graph-based test-selection hint, not a replacement for the test suite. Prefer impacted tests first, then run broader gates when shared code or coverage warnings are involved.
- Search-quality eval failures should be interpreted by query class, expected anchor, anchor bucket, MRR, max latency, and fallback source before ranking or route-support claims are promoted.
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
- [files](references/files.md) - Indexed file inventory and coverage markers
- [affected](references/affected.md) - Changed-file impact analysis
- [bookmark](references/bookmark.md) - Save reusable investigation focus nodes
- [setup](references/setup.md) - Managed embedding setup
- [serve](references/serve.md) - Local integration surface outside the normal CLI-navigation docs/spec workflow
