# `bookmark` - Investigation Focus State

Saves node focuses for repeated investigations. Bookmark state lives in the
project cache and is explicit: read commands do not use it unless you pass a
bookmark flag.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> bookmark add --project <target-workspace> --query WorkspaceIndexer --comment "indexing entry point"` | Creates or reuses the `Investigation` category and returns a bookmark id. |
| Failure path | If a bookmark points at a node removed by reindexing, `bookmark list` reports it as stale or absent; rerun `search --why` and replace the bookmark. | Avoids silently using stale focus state. |
| Integration edge | Use `context --bookmark <bookmark_id>` when a deep evidence packet should reuse a saved focus. | Makes bookmark context opt-in and visible in the retrieval trace. |

## Notes

- `bookmark list --format json` is best for automation.
- `context --bookmark <bookmark_id>` is mutually exclusive with `--id` and `--query`.
- Full refreshes and projection cleanup may remove bookmark rows for deleted
  nodes; orphaned rows degrade as stale instead of crashing.
