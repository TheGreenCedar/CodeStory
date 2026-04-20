# `symbol` — Inspect a Symbol's Details and Relationships

Resolves a symbol by ID or query, then returns its full metadata: kind, file location, children, incoming references, and outgoing calls.

## Usage

```
target/release/codestory-cli(.exe) symbol [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--id` | string | — | Node ID to inspect (conflicts with `--query`) |
| `--query` | string | — | Symbol name to resolve (conflicts with `--id`) |
| `--file` | string | — | Limit `--query` resolution to paths containing this fragment |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |
| `--output-file` | path | *stdout* | Write output to a file; the parent directory must already exist |
| `--mermaid` | flag | `false` | Render a Mermaid symbol graph instead of Markdown/JSON |

> One of `--id` or `--query` is required. `--file` requires `--query`. If `--query` is ambiguous (multiple equally-ranked matches), the CLI will error and suggest a more qualified name or file filter.

## Target Resolution

When using `--query`, the CLI:
1. Runs a hybrid search across the index
2. Ranks results by exact/terminal/structural match quality
3. Selects the top-ranked hit, or errors if the top two are equally ranked (ambiguous)

## Output

```
# Symbol
resolved: `AppController` -> [abc123] AppController [STRUCT] `src/lib.rs`:42
focus: [abc123] AppController [STRUCT] `src/lib.rs`:42
children: 5
- [c1] new [FUNCTION] `src/lib.rs`:100
- [c2] open_project [FUNCTION] `src/lib.rs`:150
incoming: 3
- [CALL] from main [FUNCTION] `src/main.rs`:15
outgoing: 2
- [CALL] to Storage::open [FUNCTION] `src/storage.rs`:20
```

## Examples

```bash
# Inspect by query
target/release/codestory-cli(.exe) symbol --project . --query AppController

# Inspect by node ID
target/release/codestory-cli(.exe) symbol --project . --id abc123def456

# Disambiguate a repeated symbol name by file
target/release/codestory-cli(.exe) symbol --project . --query TicTacToe --file rust_tictactoe.rs

# JSON output
target/release/codestory-cli(.exe) symbol --project . --query "WorkspaceIndexer" --format json
```
