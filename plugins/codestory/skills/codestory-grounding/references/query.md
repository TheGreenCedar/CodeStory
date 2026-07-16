# `query` — Run Small Graph Query Pipelines

Runs a compact pipeline over indexed CodeStory data. Use it when you want a short, scriptable answer without manually chaining `search`, `symbol`, and `trail` commands.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Operations

| Operation | Produces | Arguments |
|-----------|----------|-----------|
| `search(query: 'Foo')` | Indexed symbol hits | `query` string, or the first positional string |
| `symbol(query: 'Foo')` | One resolved symbol plus direct children | `query` string, or the first positional string |
| `trail(symbol: 'Foo')` | Trail nodes around a resolved symbol | `symbol` string, optional `depth`, optional `direction` (`incoming`, `outgoing`, `both`) |
| `filter(...)` | Filters current items | optional `kind`, `file`, and `depth` |
| `limit(5)` | Truncates current items | positional integer, or `n: 5` |

Unknown operation names, unknown named arguments, invalid node kinds, and malformed strings are rejected with a caret pointing at the bad query segment. `query` is not a SQL interface; use `search --query <term> --why` for raw discovery or the graph-query DSL examples below for pipelines.

## Examples

```bash
# Find matching functions
<codestory-cli> query --project <target-workspace> "search(query: 'check_winner') | filter(kind: function) | limit(5)"

# Inspect one symbol and keep children from a file fragment
<codestory-cli> query --project <target-workspace> "symbol(query: 'AppController') | filter(file: 'runtime')"

# Follow outgoing trail context for a symbol
<codestory-cli> query --project <target-workspace> "trail(symbol: 'ResolutionPass', depth: 2, direction: outgoing) | filter(kind: function) | limit(10)"

# Machine-readable output
<codestory-cli> query --project <target-workspace> "search(query: 'WorkspaceIndexer') | limit(3)" --format json
```
