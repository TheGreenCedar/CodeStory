# `explore` - Bundled Symbol Browser

Resolves one target and returns a combined status, search, results, symbol,
trail, navigation, and snippet view. In an interactive terminal it can open the
TUI; use `--no-tui` or `--format json` for stable agent output.

## Usage

```
target/release/codestory-cli(.exe) explore [OPTIONS] <--id <ID>|--query <QUERY>>
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--id <node_id>` | none | Resolve an exact node id from prior output. |
| `--query <text>` | none | Resolve by symbol query. Required when using `--file`. |
| `--file <fragment>` | none | Disambiguate a query by path fragment. |
| `--depth <n>` | `2` | Neighborhood trail depth. |
| `--max-nodes <n>` | `18` | Trail node cap, clamped to 1-120. |
| `--no-tui` | off | Print Markdown instead of opening the terminal explorer. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) explore --project . --query WorkspaceIndexer --no-tui` | Markdown bundle with status/retrieval/freshness, query resolution, navigation results, symbol details, trail, and snippet context. |
| Failure path | If the target is ambiguous or missing, run `search --project . --query WorkspaceIndexer --why`, then retry with `--id <node_id>` or `--file <fragment>`. | Avoids guessing which symbol the report describes. |
| Integration edge | Use explore after `search --why`; feed the resolved node id into `context --id`, `trail --id`, or `snippet --id` when the next step needs sharper evidence. | Converts broad search into a focused browser handoff. |

## Notes

- Use `--format json` for downstream tools.
- Use `--no-tui` in non-interactive agent runs to keep output copy-paste stable.
- The TUI panes are keyboard reachable with Tab/Shift-Tab and include Status,
  Search, Results, Detail, Trail, and Snippet.
- `explore` includes production-only neighborhood trails; run `trail --include-tests` separately when test callers matter.
