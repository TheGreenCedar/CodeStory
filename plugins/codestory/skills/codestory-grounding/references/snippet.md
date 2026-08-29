# `snippet` — Fetch Source Code Context Around a Symbol

Resolves a symbol and returns its source code with surrounding context lines. Useful for reading the actual implementation without opening the full file.

This is not a substitute for the host's direct file-read action when the user
named one exact file and the requested evidence is file-local.

Markdown output uses ANSI syntax highlighting when stdout is an interactive terminal. Output files, pipes, and JSON output stay uncolored for automation.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Output

Markdown output includes `context: scope=<line_context|function_body> requested_lines=<n> max_snippet_bytes=<bytes>`. JSON includes the same `scope`, `requested_context`, `snippet_truncated`, and `max_snippet_bytes` fields, plus `range_source`, `fallback_reason`, and `truncation_guidance` when applicable. If `snippet_truncated` is true, the byte cap stopped the output; follow `truncation_guidance` rather than assuming a larger `context` will reveal the omitted code.

When MCP `scope` is `function_body` (or `function_body: true`), snippet prefers
an implementation/body-looking function or method hit over a declaration-looking
hit when possible. If indexed source ranges are missing or suspicious, supported
brace languages attempt a bounded brace-balanced fallback before degrading. If
fallback fails, output keeps `scope=line_context` and reports the fallback
reason explicitly.

The MCP `snippet` tool exposes `scope=line_context|function_body` and
`context=0..200`. `lines` aliases `context`, and `function_body` boolean aliases
`scope`. Pass only one member of each alias pair; unknown or conflicting fields
fail instead of being ignored.

```
# Snippet
resolved: `AppController::new` -> [abc123] new [FUNCTION] `src/lib.rs`:100
file: `src/lib.rs`  lines: 96–115
context: requested_lines=4 max_snippet_bytes=20000

    96: // --- AppController ---
    97:
    98: impl AppController {
    99:     /// Creates a new controller instance.
   100:     pub fn new() -> Self {
   101:         Self {
   102:             storage: None,
   103:             event_bus: EventBus::new(),
   104:         }
   105:     }
   106:
   107:     /// Opens a project from the given root.
   108:     pub fn open_project(&mut self, root: PathBuf) -> Result<()> {
```

Pass `query` or `id` (or `paths` / `path` / `file_path` / `symbol_id`). There is
no `file` or `output_file` field.
