# `bookmark` - Investigation Focus State

Saves node focuses for repeated investigations. Bookmark state lives in the
project cache and is explicit: read commands do not use it unless you pass a
bookmark flag.

## Usage

```
target/release/codestory-cli(.exe) bookmark <add|list|remove> [OPTIONS]
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `bookmark add --id <node_id>` | none | Save an exact node id from `search`, `symbol`, `trail`, or `explore`. |
| `bookmark add --query <text>` | none | Resolve a symbol query and save the selected node. |
| `--category <name>` | `Investigation` | Group related investigation focuses. Missing categories are created on add. |
| `--comment <text>` | none | Add a short investigation note. |
| `bookmark list --category <name-or-id>` | all | List saved focuses, optionally scoped to a category. |
| `bookmark remove <bookmark_id>` | none | Remove one saved focus. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) bookmark add --project . --query WorkspaceIndexer --comment "indexing entry point"` | Creates or reuses the `Investigation` category and returns a bookmark id. |
| Failure path | If a bookmark points at a node removed by reindexing, `bookmark list` reports it as stale or absent; rerun `search --why` and replace the bookmark. | Avoids silently using stale focus state. |
| Integration edge | Use `ask --bookmark <bookmark_id>` when an answer should reuse a saved focus. | Makes bookmark context opt-in and visible in the retrieval trace. |

## Notes

- `bookmark list --format json` is best for automation.
- `ask --bookmark <bookmark_id>` is mutually exclusive with `--focus-id`.
- Full refreshes and projection cleanup may remove bookmark rows for deleted
  nodes; orphaned rows degrade as stale instead of crashing.
