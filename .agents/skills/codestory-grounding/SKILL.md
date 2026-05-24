---
name: codestory-grounding
description: Use extensively to ground repository claims, plans, and edits with `codestory-cli` workspace queries before answering or changing code.
---

# CodeStory Grounding

Use this skill extensively after installing it in an agent's global skill
directory. On first use, set up the CodeStory CLI source/binary artifact once;
after that, point the skill at explicit target workspaces. The CodeStory source
checkout is a tool artifact unless the user is actually editing CodeStory; do
not assume the current directory is the repository being grounded.

## Command Roles

- `doctor`: read-only health check for project, cache, index, retrieval, semantic readiness, freshness, and relevant environment settings.
- `index`: build or refresh the SQLite graph, search, snapshot, and semantic cache.
- `ground`: broad repo-level orientation snapshot; use `--why` for coverage, retrieval mode, gaps, and next commands.
- `packet`: bounded broad-task answer packet with citations, budget usage, sufficiency status, gaps, and follow-up commands.
- `search`: lightweight candidate discovery for symbols, files, literals, API paths, modules, or behavior terms; use `--why` for ranking reasons and broad-query Search Plan evidence when present.
- `context`: deep evidence/context bundle for one concrete target selected by `--id`, `--query`, or `--bookmark`; it is not question answering and does not interpret natural-language questions.
- `symbol`: inspect one exact symbol and relationships.
- `trail`: follow caller, callee, and reference graph around a symbol; use `--story --hide-speculative` for readable flow evidence.
- `snippet`: fetch source context around a symbol.
- `drill`: run a deterministic agent-grounding packet for a natural-language question and concrete anchors, including search/symbol/trail/explore/snippet artifacts, bridge evidence, consumer summaries, endpoint/source-truth files, an Evidence Packet, Answer Readiness report, compact `drill-summary.json`, claim-ledger template, and source-verification checklist.
- `drill-suite`: run a manifest-defined real-repo drill matrix from the CodeStory owner checkout, writing per-repo drill artifacts plus aggregate `suite-report.md`/`suite-report.json`; use it for cross-repo agent-UX regression measurement without baking workstation-specific repo names into the CLI.
- `query`: run structured graph-query pipelines.
- `explore`: interactive or bundled navigation view around a target, including grouped line-numbered source packets.
- `files`: list indexed file inventory, language counts, inferred source/test/generated/vendor roles, and partial-index markers.
- `affected`: map changed files to impacted symbols and likely test files using indexed graph dependents.
- `bookmark`: save, list, or remove investigation focus nodes.
- `setup embeddings`: install managed embedding assets.
- `serve`: local integration surface; do not use it for the CLI-navigation docs/spec workflow unless the user explicitly asks for transport work.

## Core Rules

1. Treat the globally installed skill as the front door; treat the CodeStory source checkout and release binary as setup artifacts.
2. Treat `<codestory-cli>` and `<target-workspace>` as separate values. Resolve both before running commands.
3. Prefer a caller-provided `CODESTORY_CLI` or an installed `codestory-cli` on `PATH`. Run this skill's setup script when neither exists.
4. Build from the CodeStory source checkout only during one-time setup, CodeStory development, or when no installed CLI is available.
5. Always pass `--project <target-workspace>` explicitly so queries target the intended checkout.
6. Use `packet` for broad task questions. Use `search` for lightweight candidate discovery. Use `context` only after you have one concrete retrieval target.
7. Use `explore` when one call should collect resolution, relationships, source slices, related files, and coverage notes around a chosen target.
8. Use `files` before claiming coverage for a language/path area, and use `affected` before selecting regression tests for a change.
9. Do not pass broad product or architecture questions to `context`. Start with `packet`; use `drill --question` when the user needs a deterministic answer-quality packet and source-truth checklist; deepen with reported follow-up commands or concrete anchors.
10. Treat command output as evidence, then open only the files needed for edits or verification.
11. When `packet` reports `sufficient` and `follow_up_commands` is empty, answer from the packet; budget truncation alone is not a gap. Carry the packet's supported-claim wording into the final answer. Include a compact "Support files" list with every relevant path from `answer.citations` and `sufficiency.avoid_opening`, not only paths mentioned in prose. Use only named follow-up commands, edit targets, or verification files.
12. For architecture answers from `drill`, read `evidence_packet.readiness` before drafting. Do not present `partial`, `inferred`, or `needs_source_read` claims as verified until the source-truth checklist has been completed.
13. Treat repo-text and cross-language framework evidence as hints unless the packet also includes typed graph evidence, snippets, or source-truth checks.
14. Keep navigation, route coverage, performance, and search-quality work CLI-first. Do not route these workflows through MCP, stdio, HTTP, or server behavior.

## One-Time Global Setup

Do this once per machine or when the CodeStory source artifact moves:

1. Confirm the skill is installed under the agent's global skill directory, for example `<agent-skill-home>/codestory-grounding`.
2. Run the setup script from this skill directory:
   ```powershell
   scripts/setup.ps1
   ```
   On Unix-like systems:
   ```sh
   sh scripts/setup.sh
   ```
3. Use the printed `CODESTORY_CLI=...` path as `<codestory-cli>`, or persist it for future sessions.
4. If you need a different source artifact, set `CODESTORY_REPO_URL` and `CODESTORY_REPO_REF` explicitly before setup; otherwise setup uses the script's `DEFAULT_CODESTORY_REPO_REF`. That ref pins the CLI source checkout, not this installed skill version.
5. For each target repository, run `doctor`, `index`, and `ground` with `--project <target-workspace>`.

Do not rebuild or re-clone CodeStory for every target repository. Rebuild only
when the CodeStory source artifact changes or the user asks to test local
CodeStory edits.

## Binary Resolution

Use this order after the one-time setup:

1. If `CODESTORY_CLI` is set, use that exact executable.
2. Else if `codestory-cli` resolves on `PATH`, use `codestory-cli`.
3. Else run `scripts/setup.ps1` on Windows or `sh scripts/setup.sh` on Unix-like systems from this skill directory, then use the printed `CODESTORY_CLI=...` path.
4. Else, if the user has a local CodeStory checkout they want to use, build with `cargo build --release -p codestory-cli --manifest-path <codestory-source>/Cargo.toml` and use the release binary under that checkout.

After resolving the binary, keep examples mentally expanded as:

```
<codestory-cli> <command> --project <target-workspace> ...
```

## Template Workflows

### Fresh repo orientation

```
<codestory-cli> doctor --project <target-workspace>
<codestory-cli> index --project <target-workspace> --refresh full
<codestory-cli> ground --project <target-workspace> --why
<codestory-cli> packet --project <target-workspace> --question "<broad task question>" --budget compact
<codestory-cli> search --project <target-workspace> --query "<architecture term>" --why
<codestory-cli> files --project <target-workspace> --format markdown
```

### Candidate-to-context workflow

```
<codestory-cli> search --project <target-workspace> --query "<symbol/file/literal/API path>" --why
# choose a concrete node_id from search output
<codestory-cli> context --project <target-workspace> --id <node-id>
```

### Exact symbol investigation

```
<codestory-cli> symbol --project <target-workspace> --id <node-id>
<codestory-cli> explore --project <target-workspace> --id <node-id> --no-tui
<codestory-cli> trail --project <target-workspace> --id <node-id> --story --hide-speculative
<codestory-cli> snippet --project <target-workspace> --id <node-id> --context 40
<codestory-cli> context --project <target-workspace> --id <node-id> --bundle out/context-<name>
```

### Change impact workflow

```
<codestory-cli> index --project <target-workspace> --refresh incremental
<codestory-cli> affected --project <target-workspace> --format markdown
# or pass explicit changed paths when git diff is not the right source:
<codestory-cli> affected --project <target-workspace> src/lib.rs tests/lib_test.rs --depth 3 --format json
```

### Broad repo/product question workflow

Do not pass the broad question to `context`. Start with `packet`; use `drill`
when the user needs an answer-quality check and source-truth checklist, not just
navigation.

```
<codestory-cli> ground --project <target-workspace> --why
<codestory-cli> packet --project <target-workspace> --question "<broad task question>" --budget compact
<codestory-cli> search --project <target-workspace> --repo-text on --query "<concrete term>" --why
<codestory-cli> search --project <target-workspace> --repo-text on --query "<another concrete term>" --why
# select anchors
<codestory-cli> context --project <target-workspace> --id <node-id>
```

When `search --why` emits `search_plan`, use its subqueries, anchor groups,
repo-text promotion status, bridge evidence, next commands, and source-truth
checks as the CodeStory-first plan. Do not treat the Search Plan as final answer
text; it is the handoff into `symbol`, `trail`, `snippet`, `explore`, `drill`,
and source-truth verification.

### Real-repo agent-quality drill workflow

Use this workflow when the goal is to test whether CodeStory helps an agent answer a realistic architecture question.

```
<codestory-cli> drill --project <target-workspace> --refresh full --question "<question>" --anchors AnchorA,AnchorB,AnchorC --output-dir target/drill/<slug> --format json
# Read drill-report.json first:
# - evidence_packet.readiness.safe_to_say
# - evidence_packet.readiness.inferred_claims
# - evidence_packet.readiness.needs_verification
# - evidence_packet.readiness.source_truth_checks
# Read drill-summary.json for compact status, freshness, retrieval, bridge counts, and verdict next action.
# Draft the CodeStory-only answer, then open only source files named or implied by source_truth_checks.
```

For a repeatable cross-repo regression drill from the CodeStory checkout, put cases in a JSON manifest and pass it explicitly:

```
<codestory-cli> drill-suite --project <codestory-source> --case-file <drill-cases.json> --refresh full --output-dir target/codestory-cross-repo-test/<stamp> --format json
# Read suite-report.json and suite-report.md for per-repo verdicts, freshness/retrieval state, bridge status, and next actions.
```

### Stale or unhealthy semantic retrieval

```
<codestory-cli> doctor --project <target-workspace>
<codestory-cli> setup embeddings --project <target-workspace>
<codestory-cli> index --project <target-workspace> --refresh full
<codestory-cli> doctor --project <target-workspace>
```

If retrieval is still partial, stale, or failed, use `search --repo-text on --why`, `symbol`, `trail`, and `snippet`; treat `context` output as incomplete if it reports gaps.

### Route coverage and quality evaluation

```
<codestory-cli> files --project <target-workspace> --format json
cargo test -p codestory-indexer --lib framework_route
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-cli --test agent_quality_eval
```

The `agent_quality_eval` command above is the quick deterministic fixture gate. To score local real-repo manifests on this workstation, run the ignored evaluator explicitly:

```
cargo test -p codestory-cli --test agent_quality_eval local_real_repo_manifests_score_or_explicitly_skip_missing_repos -- --ignored --nocapture
```

### Performance review baseline

```
# From <codestory-source>
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
- Broad architecture `search` can return `search_plan`. Treat `typed_anchor` and
  `promoted` groups as candidate anchors, keep `ambiguous` and
  `needs_source_read` groups visibly uncertain, and follow the plan's
  source-truth checks before making final claims.
- `search` may include `did_you_mean` suggestions when semantic retrieval found close matches but lexical lookup did not. Treat these as navigation hints, not exact matches.
- `context --query` first resolves the query to a concrete target. If the target is ambiguous, use `search --why`, then rerun `context --id`.
- `trail` should be judged by whether unrelated resolved targets disappeared. Local helper names like `once`, `from`, or `copied` can still appear as `[unknown]` nodes without indicating bad semantic resolution.
- OpenAPI schema files index endpoint symbols such as `GET /api/users`; client literal calls can create speculative edges to those endpoints, so check certainty before treating a frontend/backend trail as verified.
- Framework route symbols include confidence labels. Treat `file_convention` and `decorator` routes as stronger than broad `heuristic` routes, and confirm handler links before claiming an end-to-end route path.
- Framework integration symbols have bounded fixture-backed support. `tauri:command:*` covers first-argument string-literal `invoke` calls, including multiline/generic calls, and Rust `#[tauri::command]`/`generate_handler!` registrations while rejecting argument strings and comments. `payload:collection:*` covers `CollectionConfig` slug blocks and Payload method calls with `collection:` options while rejecting unrelated slugs, props, and string substrings. Treat evidence outside those covered forms as unsupported until source verification proves it.
- `drill` Evidence Packet readiness is the agent-facing contract: `anchored` and `supported` claims may be drafted from CodeStory evidence; `partial`, `inferred`, and `needs_source_read` claims must stay visibly uncertain until source verification completes. Use `drill-summary.json`/`suite-report.json` for compact status comparisons; stale freshness or symbolic-only retrieval are agent-UX degradation signals even when anchors resolve.
- The agent-quality evaluator is deterministic. Its gate fails unsupported high-confidence claims, overclaims, high-confidence material source corrections, and poor confidence calibration; do not replace it with live LLM judging in CI.
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
