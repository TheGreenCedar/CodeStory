# `trail` — Follow a Symbol's Call/Reference Graph

Builds a directed graph trail starting from a target symbol. Supports neighborhood exploration, outgoing-reference traversal, and incoming-reference traversal with configurable depth, direction, and filtering.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

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
<codestory-cli> trail --project <target-workspace> --query AppController

# Follow all outgoing references, deeper
<codestory-cli> trail --project <target-workspace> --query "run_indexing" --mode referenced --depth 3

# Incoming callers only, include tests
<codestory-cli> trail --project <target-workspace> --query Storage::open --mode referencing --include-tests

# Larger trail, vertical layout, JSON
<codestory-cli> trail --project <target-workspace> --query EventBus --max-nodes 50 --layout vertical --format json

# Hide low-confidence edges in Markdown or JSON output
<codestory-cli> trail --project <target-workspace> --query ResolutionPass --hide-speculative

# Narrative handoff for a reviewer or LLM
<codestory-cli> trail --project <target-workspace> --query ResolutionPass --story

# Export a Graphviz DOT graph
<codestory-cli> trail --project <target-workspace> --query ResolutionPass --format dot --output-file trail.dot

# Export a Mermaid flowchart
<codestory-cli> trail --project <target-workspace> --query ResolutionPass --mermaid --output-file trail.mmd
```

## Interpreting Trail Noise

Focus on whether unrelated resolved targets disappeared after a fix. Local helper calls can still show up as `[unknown]` nodes such as `once`, `from`, or `copied`; that is usually acceptable if they are no longer being resolved to unrelated symbols elsewhere in the repo.
