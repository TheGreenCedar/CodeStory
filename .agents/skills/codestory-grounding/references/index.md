# `index` — Build or Refresh the Symbol Index

Discovers project files, extracts symbols and edges via tree-sitter + semantic resolution, and persists everything to the SQLite index.

## Usage

```
python scripts/index.py [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory for the SQLite index |
| `--refresh` | enum | `auto` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Output format: `markdown` or `json` |

## Refresh Modes

| Mode | Behavior |
|------|----------|
| `auto` | Full index if empty, incremental otherwise |
| `full` | Re-index everything from scratch |
| `incremental` | Only re-index changed files |
| `none` | Open existing index without refreshing |

## Output

Returns project stats and, when a refresh runs, phase timings:

```
# Index
project: `codestory`
storage: `/path/to/codestory.db`
refresh: `auto(incremental)`
stats: nodes=4231 edges=8452 files=187 errors=3
timings_ms: parse=1200 flush=300 resolve=450 cleanup=80 cache_refresh=0
resolution: calls 120->15, imports 42->3
```

## Examples

```bash
# First-time index of the current repo
python scripts/index.py

# Force full re-index
python scripts/index.py --refresh full

# Index a different project, JSON output
python scripts/index.py --project ../other-repo --format json
```

## Refresh Troubleshooting

| Situation | Recommended refresh |
|----------|----------------------|
| First-time indexing or after cache deletion | `--refresh full` |
| Verifying a fix for prior indexing errors | `--refresh full` |
| Verifying schema/storage-version or graph/query-rule changes | `--refresh full` |
| Normal follow-up indexing after editing a few files | `--refresh incremental` or `auto` |
| Reusing a known-good index immediately after a successful fresh build + index run | `--refresh none` |

Prefer `--refresh full` when you need confidence that historical errors are gone. Incremental runs can leave stale error rows behind if the previously failing files are not reprocessed.
