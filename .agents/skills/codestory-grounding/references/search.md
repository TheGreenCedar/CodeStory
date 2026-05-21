# `search` — Search Indexed Symbols and Repo Text

Searches the symbol index for matching nodes, optionally augmented with grep-style text hits across the repo. Results are ranked by relevance score and deduplicated.

## Usage

```
target/release/codestory-cli(.exe) search [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--query` | string | **required** | Search term — symbol name or natural-language text |
| `--limit` | integer | `10` | Maximum results per provenance group, capped at 50 |
| `--repo-text` | enum | `auto` | Repo text scanning: `auto`, `on`, or `off`. `auto` also scans repo text when indexed hits are weak or no exact concrete anchor matched |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |
| `--output-file` | path | *stdout* | Write output to a file; the parent directory must already exist |
| `--hybrid-lexical` | float | runtime default | Override lexical weight for hybrid-search research |
| `--hybrid-semantic` | float | runtime default | Override semantic weight for hybrid-search research |
| `--hybrid-graph` | float | runtime default | Override graph-neighborhood weight for hybrid-search research |

## Query Behavior

- **Symbol-like queries** (e.g. `AppController`, `run_indexing`) search the indexed symbol table.
- **Natural-language queries** (e.g. `"how does incremental indexing work"`) also perform a repo-wide text scan and merge results by score.
- **Field-qualified queries** filter indexed and repo-text results after candidate retrieval. Supported filters are `kind:<node-kind-or-alias>`, `path:<path-fragment>`, `name:<symbol-fragment>`, and `lang:<language-or-extension>`. Example: `kind:function name:listUsers` or `path:routes.ts /api/users`.
- **Concrete anchors with weak indexed results** also trigger repo text in `auto` mode. This prevents stale names such as retired UI components from looking like valid direct symbol hits.
- When hybrid retrieval finds strong semantic matches but no lexical match, Markdown and JSON output include `did_you_mean` suggestions.
- Broad architecture-style queries can include `search_plan`. The plan reports
  extracted and dropped terms, bounded subqueries, candidate windows, anchor
  groups, repo-text promotion status, bridge evidence, next commands, and
  source-truth checks. It is a discovery plan, not final answer prose.
- Ranking boosts exact and terminal symbol names, CamelCase initials, compound terms, and path co-location. Test, fixture, vendor, and external hits are dampened unless the query asks for them.
- Import/re-export-looking exact hits are ranked below definition-looking hits when source-line evidence is available.
- Repo-text fallback remains explicit evidence. Treat repo-text hits as clues to inspect, not as silent graph success.
- For architecture questions, broad natural-language `search` is discovery only. If `query_assessment` says `weak_top_hit=true` or there is no exact anchor, move to `drill` with concrete anchors from `ground`/`search`; do not answer from broad search hits alone.
- `symbol`, `trail`, and `snippet` require a resolvable graph target. Semantic suggestions and repo-text hits can guide follow-up searches, but they are not promoted into graph targets by those commands.
- **Hybrid weight overrides** are intended for benchmarking and tuning. Omit all three `--hybrid-*` flags for production-like runtime defaults.

## Output

```
# Search
query: `AppController`
hits: 3
- [abc123] AppController [STRUCT] `src/lib.rs`:42 score=0.95
- [def456] AppController::new [FUNCTION] `src/lib.rs`:100 score=0.80
- [ghi789] app_controller [MODULE] `src/app/mod.rs`:1 score=0.60
```

Each hit includes: node ID, display name, kind, file path, line number, relevance score, provenance, and `match_quality` (`exact`, `normalized_exact`, `prefix`, `fuzzy`, `semantic_suggestion`, or `repo_text`).

Search output also includes `query_assessment` with exact symbol hit count, weak-hit/stale-anchor flags, any repo-text fallback reason, and a recommended next action. Use it to avoid treating weak semantic suggestions as proof of an exact anchor.

For broad architecture queries, JSON may include `search_plan`; Markdown renders
it when `--why` is set. Prefer `typed_anchor` and `promoted` plan groups as
follow-up anchors. Treat `ambiguous` and `needs_source_read` groups as uncertain
until direct source verification. Use the plan's next commands to continue with
`symbol`, `trail`, `snippet`, or `explore`.

When a name appears more than once, prefer typed symbol hits such as `[function]`, `[struct]`, `[field]`, or `[file]` over `[unknown]` hits when you are verifying symbol surfacing. `[unknown]` results are often usage-like callsite or reference nodes, not the canonical definition.

Repo-text hits from text-only surfaces such as `.svelte` files are evidence, not graph anchors. Use the excerpt to choose a symbol or open a snippet/source file for verification.

For ranking or route-search changes, run the search-quality eval and interpret
failures before promoting the change:

```bash
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
```

- Low recall: an expected anchor is missing from indexed-symbol hits, repo-text
  hits, or both.
- Low MRR: the expected anchor exists, but lower-quality or noisy hits outrank
  it.
- Missing Search Plan: a broad architecture query was not classified or
  decomposed. Check extracted terms, architecture intent labels, and
  `query_assessment` before changing weights.
- Promotion precision: repo-text-only or ambiguous groups must not be high
  confidence.
- High max latency: compare against the current fixture cap and performance
  baseline before tuning.
- Route/handler misses block route-support promotion until the coverage playbook
  documents the gap or the fixture/search expectation is fixed.
- Keep this eval CLI-first; do not require server, MCP, watch, or transport work
  for Search Quality 2.0.

## Examples

```bash
# Search for a symbol
target/release/codestory-cli(.exe) search --project . --query AppController

# Natural-language search, more results
target/release/codestory-cli(.exe) search --project . --query "how does the grounding snapshot work" --limit 20

# Force repo text scanning for a symbol-like query
target/release/codestory-cli(.exe) search --project . --query AppController --repo-text on

# Narrow an ambiguous result set by kind and file path
target/release/codestory-cli(.exe) search --project . --query "kind:function path:routes.ts /api/users" --repo-text off

# JSON output
target/release/codestory-cli(.exe) search --project . --query TrailResult --format json

# Search-quality eval harness after ranking changes
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
```
