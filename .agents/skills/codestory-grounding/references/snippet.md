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
| `--context` | integer | `4` | Number of surrounding context lines above and below the symbol |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |

## Output

```
# Snippet
resolved: `AppController::new` -> [abc123] new [FUNCTION] `src/lib.rs`:100
file: `src/lib.rs`  lines: 96–115

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

# By node ID, JSON output
target/release/codestory-cli(.exe) snippet --project . --id abc123def456 --format json
```
