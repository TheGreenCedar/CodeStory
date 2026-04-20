# `query` — Run Small Graph Query Pipelines

Runs a compact pipeline over indexed CodeStory data. Use it when you want a short, scriptable answer without manually chaining `search`, `symbol`, and `trail` commands.

## Usage

```
target/release/codestory-cli(.exe) query [OPTIONS] <QUERY>
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `<QUERY>` | string | required | Pipeline expression such as `search(query: 'Foo') | filter(kind: function) | limit(5)` |
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |
| `--output-file` | path | *stdout* | Write output to a file; the parent directory must already exist |

## Operations

| Operation | Produces | Arguments |
|-----------|----------|-----------|
| `search(query: 'Foo')` | Indexed symbol hits | `query` string, or the first positional string |
| `symbol(query: 'Foo')` | One resolved symbol plus direct children | `query` string, or the first positional string |
| `trail(symbol: 'Foo')` | Trail nodes around a resolved symbol | `symbol` string, optional `depth`, optional `direction` (`incoming`, `outgoing`, `both`) |
| `filter(...)` | Filters current items | optional `kind`, `file`, and `depth` |
| `limit(5)` | Truncates current items | positional integer, or `n: 5` |

Unknown operation names, unknown named arguments, invalid node kinds, and malformed strings are rejected with a caret pointing at the bad query segment.

## Examples

```bash
# Find matching functions
target/release/codestory-cli(.exe) query --project . "search(query: 'check_winner') | filter(kind: function) | limit(5)"

# Inspect one symbol and keep children from a file fragment
target/release/codestory-cli(.exe) query --project . "symbol(query: 'AppController') | filter(file: 'runtime')"

# Follow outgoing trail context for a symbol
target/release/codestory-cli(.exe) query --project . "trail(symbol: 'ResolutionPass', depth: 2, direction: outgoing) | filter(kind: function) | limit(10)"

# Machine-readable output
target/release/codestory-cli(.exe) query --project . "search(query: 'WorkspaceIndexer') | limit(3)" --format json
```
