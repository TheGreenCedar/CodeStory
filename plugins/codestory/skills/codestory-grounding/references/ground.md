# `ground` — Compact Codebase Context Snapshot

Produces a budget-aware grounding snapshot of the entire indexed codebase: root symbols, per-file coverage, compressed file summaries, coverage buckets, and recommended follow-up queries.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

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
orientation: confidence=partial entrypoints=1/2 subsystems=4/7 candidates=224/816 uncertainty=bounded_candidate_window,compressed_presentation
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
omitted entrypoint evidence, limited subsystem breadth, and budget-driven
presentation compression. `ground --why` includes the same limitations in its
confidence and gap notes.

## Examples

```bash
# Default balanced grounding
<codestory-cli> ground --project <target-workspace>

# Strict grounding for quick context
<codestory-cli> ground --project <target-workspace> --budget strict

# Max depth, JSON output
<codestory-cli> ground --project <target-workspace> --budget max --format json

# Ground without refreshing the index
<codestory-cli> ground --project <target-workspace> --refresh none
```
