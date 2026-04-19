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
| `--limit` | integer | `10` | Maximum number of results (capped at 50) |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |

## Query Behavior

- **Symbol-like queries** (e.g. `AppController`, `run_indexing`) search the indexed symbol table.
- **Natural-language queries** (e.g. `"how does incremental indexing work"`) also perform a repo-wide text scan and merge results by score.
- When hybrid retrieval finds strong semantic matches but no lexical match, Markdown and JSON output include `did_you_mean` suggestions.

## Output

```
# Search
query: `AppController`
hits: 3
- [abc123] AppController [STRUCT] `src/lib.rs`:42 score=0.95
- [def456] AppController::new [FUNCTION] `src/lib.rs`:100 score=0.80
- [ghi789] app_controller [MODULE] `src/app/mod.rs`:1 score=0.60
```

Each hit includes: node ID, display name, kind, file path, line number, and relevance score.

When a name appears more than once, prefer typed symbol hits such as `[function]`, `[struct]`, `[field]`, or `[file]` over `[unknown]` hits when you are verifying symbol surfacing. `[unknown]` results are often usage-like callsite or reference nodes, not the canonical definition.

## Examples

```bash
# Search for a symbol
target/release/codestory-cli(.exe) search --project . --query AppController

# Natural-language search, more results
target/release/codestory-cli(.exe) search --project . --query "how does the grounding snapshot work" --limit 20

# JSON output
target/release/codestory-cli(.exe) search --project . --query TrailResult --format json
```
