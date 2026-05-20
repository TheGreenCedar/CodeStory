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
| `--max-nodes` | integer | `120` | Maximum nodes in the trail (clamped 1-200) |
| `--include-tests` | flag | `false` | Include test and bench callers |
| `--show-utility-calls` | flag | `false` | Include utility/helper call edges |
| `--hide-speculative` | flag | `false` | Hide uncertain/speculative edges and remove nodes disconnected from the trail focus |
| `--story` | flag | `false` | Render a readable narrative with entry points, grouped runtime/data/type flow, side effects, uncertainty, and test scope |
| `--layout` | enum | `horizontal` | Layout direction: `horizontal` or `vertical` |
| `--refresh` | enum | `none` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown`, `json`, or trail-only `dot` |
| `--output-file` | path | *stdout* | Write command output to a file; the parent directory must already exist |
| `--mermaid` | flag | `false` | Render a Mermaid flowchart instead of Markdown/JSON/DOT |

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
- [edge1] open_project -call-> Storage::open certainty=certain
- [edge2] main ~call~> open_project certainty=probable
- [edge3] open_project ?call?> maybe_helper certainty=uncertain
```

## Edge Certainty Notation

Markdown trail output renders edge certainty directly in the arrow shape:

| Certainty | Arrow | Meaning |
|-----------|-------|---------|
| `certain` / `definite` | `-call->` | Verified or high-confidence edge |
| `probable` | `~call~>` | Likely edge inferred from available evidence |
| `uncertain` / `speculative` | `?call?>` | Low-confidence edge; hide with `--hide-speculative` |
| missing certainty | `-call-> [unresolved]` | Legacy or unresolved certainty metadata |

## Story Mode

`--story` turns the trail graph into a text-first narrative for handoff to an
LLM or reviewer. Markdown output starts with `# Trail Story`; JSON output keeps
the normal trail context and adds its optional `story` object inside that shared
context. Story mode is explicit and does not apply to `--mermaid` or
`--format dot`.

Story sections:

| Section | What it makes explicit |
|---------|------------------------|
| Entry Points | The focus symbol and any rendered source nodes with no incoming edge. |
| Runtime Flow | Call/macro edges that look like runtime execution flow. |
| Data And Interface Flow | Usage/import/include-style edges that show wiring and consumers. |
| Type And Member Structure | Member, inheritance, type-usage, override, and generic/template structure. |
| Utility Calls | Helper calls when `--show-utility-calls` is enabled; otherwise hidden from the main story. |
| Side Effects | Likely mutating/runtime-effect calls inferred from edge kinds and labels. |
| Uncertainty | Probable, uncertain, speculative, or missing-certainty edges in words. |
| Tests | Whether tests/benches were included or excluded, plus visible test-like nodes. |
| Gaps And Limits | Truncation, omitted edges, empty trails, and applied filters. |

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

# Hide low-confidence edges in Markdown or JSON output
target/release/codestory-cli(.exe) trail --project . --query ResolutionPass --hide-speculative

# Narrative handoff for a reviewer or LLM
target/release/codestory-cli(.exe) trail --project . --query ResolutionPass --story

# Export a Graphviz DOT graph
target/release/codestory-cli(.exe) trail --project . --query ResolutionPass --format dot --output-file trail.dot

# Export a Mermaid flowchart
target/release/codestory-cli(.exe) trail --project . --query ResolutionPass --mermaid --output-file trail.mmd
```

## Interpreting Trail Noise

Focus on whether unrelated resolved targets disappeared after a fix. Local helper calls can still show up as `[unknown]` nodes such as `once`, `from`, or `copied`; that is usually acceptable if they are no longer being resolved to unrelated symbols elsewhere in the repo.
