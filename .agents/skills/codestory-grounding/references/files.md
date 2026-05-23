# `files` - Indexed File Inventory

Lists files known to the persisted CodeStory index. Use it to inspect coverage,
language mix, inferred roles, and partial-index markers before making broad
claims about what the graph can see.

## Usage

```
<codestory-cli> files [OPTIONS]
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--path <fragment>` | none | Only list files whose indexed path contains the fragment. |
| `--language <name>` | none | Only list files for one indexed language. |
| `--role <source|test|generated|vendor|unknown>` | none | Filter by inferred role. |
| `--limit <n>` | `500` | Cap file rows. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Inventory | `<codestory-cli> files --project <target-workspace> --format markdown` | Language counts, framework route coverage matrix, usable/partial index notes, and a capped file list. |
| Coverage check | `<codestory-cli> files --project <target-workspace> --language rust --format json` | Machine-readable file rows with `indexed`, `complete`, `role`, `error_count`, and `summary.framework_route_coverage`. |
| Test discovery | `<codestory-cli> files --project <target-workspace> --role test` | Test-like files inferred from path/name conventions. |

## Notes

- `files` reads persisted `FileInfo`; it does not scan the repo live unless `--refresh` asks for an index refresh.
- Treat `index usable` with incomplete or error counts as a partial-coverage signal, not a failure.
- `summary.framework_route_coverage` is the support matrix for framework route extraction. It includes `status`, `fixture_status`, `confidence_floor`, `handler_link_support`, `unsupported_patterns`, `known_gaps`, and `promotable`. Treat `partial`, `heuristic`, text-only handler support, and `promotable=false` as review prompts, not proof of full framework parity.
- Route coverage statuses:
  - `supported`: fixture-backed behavior is passing and documented coverage is met.
  - `heuristic`: pattern-backed evidence that needs source review.
  - `partial`: some cases are covered, but known route shapes, handler links, or fixtures are missing.
  - `unsupported`: no support claim is made.
  - `stale`: refresh before promoting the claim.
  - `non-promotable`: required fixtures, known-gap notes, or eval evidence are missing or failing.
- Role inference is path/name based. It is useful for navigation and test selection, but not a formal build-system classification.
