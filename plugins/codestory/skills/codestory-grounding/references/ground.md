# `ground` — Compact Codebase Context Snapshot

Produces a budget-aware grounding snapshot of the entire indexed codebase: root symbols, per-file coverage, compressed file summaries, coverage buckets, and recommended follow-up queries.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Budget Modes

| Mode | Behavior |
|------|----------|
| `strict` | Minimal snapshot — only top-level root symbols and compressed file list |
| `balanced` | Default — covers most files with representative symbols |
| `max` | Largest bounded snapshot; output may still compress files and symbols to stay within protocol limits |

## Output

```
# Grounding Snapshot
root: `codestory`
budget: `balanced`
coverage: files 187/187 symbols 1200/4231 compressed_files=42
orientation: confidence=partial entrypoints=1/2 subsystems=4/7 candidates=224/816 uncertainty=bounded_candidate_window,graph_signal_thin,compressed_presentation
stats: nodes=4231 edges=8452 files=187 errors=3
recommended_queries: WorkspaceIndexer, AppController, TrailResult
notes:
- 42 files compressed to symbol summaries
root_symbols:
- AppController [STRUCT] (score 0.95)
files:
- `src/lib.rs` [rust] symbols 12/30 full | AppController | EventBus
coverage_buckets:
- `high_coverage` files=120 symbols=900 samples=src/lib.rs, src/main.rs
```

`orientation` reports how well the selected root-symbol prefix represents
entrypoints and architecture subsystems. Its confidence is specific to compact
repository orientation; it does not upgrade source coverage or retrieval
sufficiency. Typed uncertainty names bounded candidate evaluation, missing or
omitted entrypoint evidence, limited subsystem breadth, budget-driven
presentation compression, and two graph-coverage limits: `graph_signal_thin`
when no evaluated candidate carried call-graph evidence, and
`lexical_fallback` when the order rests on names and layout alone. Read either
as a reason to verify structure with `trail` before making a structure claim,
not as evidence about the repository.

MCP `ground` arguments are `project` and optional `budget`. There is no
`refresh` field; the first call may refresh the local map.
