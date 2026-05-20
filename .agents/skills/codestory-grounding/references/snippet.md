# `snippet` — Fetch Source Code Context Around a Symbol

Resolves a symbol and returns its source code with surrounding context lines. Useful for reading the actual implementation without opening the full file.

Markdown output uses ANSI syntax highlighting when stdout is an interactive terminal. Output files, pipes, and JSON output stay uncolored for automation.

## Usage

```
target/release/codestory-cli(.exe) snippet [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--id` | string | — | Node ID of the symbol (conflicts with `--query`) |
| `--query` | string | — | Symbol name to resolve (conflicts with `--id`) |
| `--file` | string | — | Limit `--query` resolution to paths containing this fragment |
| `--context` | integer | `4` | Number of surrounding context lines above and below the symbol. Alias: `--lines` |
| `--function-body` | flag | `false` | Prefer the selected function/method implementation body when source ranges are available |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |
| `--output-file` | path | *stdout* | Write output to a file; the parent directory must already exist |

## Output

Markdown output includes `context: scope=<line_context|function_body> requested_lines=<n> max_snippet_bytes=<bytes>`. JSON includes the same `scope`, `requested_context`, `snippet_truncated`, and `max_snippet_bytes` fields. If `snippet_truncated` is true, the byte cap stopped the output; raising `--context` alone may not reveal more code.

When `--function-body` is set, snippet prefers an implementation/body-looking function or method hit over a declaration-looking hit when possible. If source ranges are unavailable, it falls back to normal line context.

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

## Examples

```bash
# Snippet with default 4 lines of context
target/release/codestory-cli(.exe) snippet --project . --query "AppController::new"

# More context
target/release/codestory-cli(.exe) snippet --project . --query run_indexing --context 10

# Agent-friendly alias for the same context setting
target/release/codestory-cli(.exe) snippet --project . --query run_indexing --lines 40

# Prefer the full implementation body when available
target/release/codestory-cli(.exe) snippet --project . --query run_indexing --function-body --lines 8

# Disambiguate by file and write stable Markdown
target/release/codestory-cli(.exe) snippet --project . --query TicTacToe --file rust_tictactoe.rs --output-file tictactoe.md

# By node ID, JSON output
target/release/codestory-cli(.exe) snippet --project . --id abc123def456 --format json
```
