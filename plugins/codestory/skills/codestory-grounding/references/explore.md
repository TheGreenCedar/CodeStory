# `explore` - Bundled Symbol Browser

Resolves one target and returns a combined status, search, results, route
context when applicable, symbol, trail, navigation, source packet, and snippet
view. In an interactive terminal it can open the TUI; use `--no-tui` or
`--format json` for stable agent output.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> explore --project <target-workspace> --query WorkspaceIndexer --no-tui` | Markdown bundle with status/retrieval/freshness, query resolution, navigation results, route context when applicable, symbol details, trail, grouped source packet, related files, budget notes, and snippet context. |
| Failure path | If the target is ambiguous or missing, run `search --project <target-workspace> --query WorkspaceIndexer --why`, then retry with `--id <node_id>` or `--file <fragment>`. | Avoids guessing which symbol the report describes. |
| Integration edge | Use explore after `search --why`; feed the resolved node id into `context --id`, `trail --id`, or `snippet --id` when the next step needs sharper evidence. | Converts broad search into a focused browser handoff. |

## Notes

- Use `--format json` for downstream tools.
- Use `--no-tui` in non-interactive agent runs to keep output copy-paste stable.
- Use `--profile architecture` for subsystem anchors where the agent needs wider production relationship evidence and related-hit source packets.
- If query resolution is `ambiguous`, do not let `explore` pick for you. Run
  `search --project <target-workspace> --query <query> --why`, then retry with `--id <node_id>`
  or `--file <fragment>`.
- Route or OpenAPI endpoint targets include `route_context` with method, path, raw path, params, confidence, source convention, provenance, and handler evidence when resolvable.
- JSON includes `source_packet.budget`, `source_packet.files`, `source_packet.related_files`, and `source_packet.notes`.
- Source slices are line-numbered, grouped by file, merged when nearby, and may include gap markers or truncation notes when the adaptive budget is reached.
- Coverage warnings are evidence. If `explore` reports a usable partial index, confirm with `files`, `doctor`, or direct source reads before making broad claims.
- Missing route context means no route evidence was available for the target in
  the indexed graph slice; it is not proof that the route is unaffected or
  unsupported across the whole repo.
- The TUI panes are keyboard reachable with Tab/Shift-Tab and include Status,
  Search, Results, Detail, Source, Trail, and Snippet.
- `explore` includes production-only neighborhood trails; run `trail --include-tests` separately when test callers matter.
