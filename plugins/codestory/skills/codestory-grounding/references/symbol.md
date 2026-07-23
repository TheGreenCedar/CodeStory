# `symbol` — Inspect a Symbol's Details and Relationships

Resolves a symbol by ID or query, then returns its full metadata: kind, file location, children, incoming references, and outgoing calls.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

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
<codestory-cli> symbol --project <target-workspace> --query AppController

# Inspect by node ID
<codestory-cli> symbol --project <target-workspace> --id abc123def456

# Disambiguate a repeated symbol name by file
<codestory-cli> symbol --project <target-workspace> --query TicTacToe --file rust_tictactoe.rs

# JSON output
<codestory-cli> symbol --project <target-workspace> --query "WorkspaceIndexer" --format json
```
