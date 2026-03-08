# `trail` — Follow a Symbol's Call/Reference Graph

Builds a directed graph trail starting from a target symbol. Supports neighborhood exploration, outgoing-reference traversal, and incoming-reference traversal with configurable depth, direction, and filtering.

## Usage

```
target/release/codestory-cli(.exe) trail [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--id` | string | — | Node ID of the root symbol (conflicts with `--query`) |
| `--query` | string | — | Symbol name to resolve as root (conflicts with `--id`) |
| `--mode` | enum | `neighborhood` | Trail mode: `neighborhood`, `referenced`, `referencing` |
| `--depth` | integer | *auto* | Max traversal depth (default: 2 for neighborhood, 0 for referenced/referencing) |
| `--direction` | enum | *auto* | Edge direction filter: `incoming`, `outgoing`, `both` |
| `--max-nodes` | integer | `24` | Maximum nodes in the trail (clamped 1–200) |
| `--include-tests` | flag | `false` | Include test and bench callers |
| `--show-utility-calls` | flag | `false` | Include utility/helper call edges |
| `--layout` | enum | `horizontal` | Layout direction: `horizontal` or `vertical` |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |

## Trail Modes

| Mode | Behavior |
|------|----------|
| `neighborhood` | Explore the immediate call graph around the symbol (default depth 2) |
| `referenced` | Follow all symbols that the target calls/references outward |
| `referencing` | Follow all symbols that call/reference the target inward |

## Output

```
# Trail
resolved: `AppController::open_project` -> [abc123] open_project [FUNCTION]
mode: neighborhood  depth: 2  direction: both  max_nodes: 24
nodes: 8  edges: 12  omitted_edges: 3  truncated: false
- [abc123] open_project [FUNCTION] `src/lib.rs`:150 (depth 0)
- [def456] Storage::open [FUNCTION] `src/storage.rs`:20 (depth 1)
- [ghi789] main [FUNCTION] `src/main.rs`:5 (depth 1)
edges:
- [abc123] -> [def456] CALL
- [ghi789] -> [abc123] CALL
```

## Examples

```bash
# Neighborhood trail (default)
target/release/codestory-cli(.exe) trail --project . --query AppController

# Follow all outgoing references, deeper
target/release/codestory-cli(.exe) trail --project . --query "run_indexing" --mode referenced --depth 3

# Incoming callers only, include tests
target/release/codestory-cli(.exe) trail --project . --query Storage::open --mode referencing --include-tests

# Larger trail, vertical layout, JSON
target/release/codestory-cli(.exe) trail --project . --query EventBus --max-nodes 50 --layout vertical --format json
```

## Interpreting Trail Noise

Focus on whether unrelated resolved targets disappeared after a fix. Local helper calls can still show up as `[unknown]` nodes such as `once`, `from`, or `copied`; that is usually acceptable if they are no longer being resolved to unrelated symbols elsewhere in the repo.
