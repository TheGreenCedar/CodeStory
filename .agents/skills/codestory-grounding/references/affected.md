# `affected` - Changed-File Impact Analysis

Maps changed files to indexed symbols, bounded dependent graph walks, and likely
test files. Use it to choose focused regression checks before running broader
repo gates.

## Usage

```
target/release/codestory-cli(.exe) affected [PATH ...] [OPTIONS]
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `PATH ...` | none | Changed repo-relative paths. If omitted, CodeStory reads `git diff --name-only HEAD`. |
| `--stdin` | off | Read changed paths from stdin, one per line, and combine them with path args. |
| `--depth <n>` | `2` | Dependent graph walk depth, clamped by the runtime. |
| `--filter <text>` | none | Keep impacted symbols whose display name or path contains the text. |
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Current diff | `target/release/codestory-cli(.exe) affected --project . --format markdown` | Impact summary based on `git diff --name-only HEAD`. |
| Explicit paths | `target/release/codestory-cli(.exe) affected --project . src/lib.rs --depth 3 --format json` | Matched/unmatched paths, impacted symbols, impacted routes/endpoints, likely tests, blind spots, and next commands. |
| Stdin | `git diff --name-only HEAD | target/release/codestory-cli(.exe) affected --project . --stdin` | Same analysis using external path selection. |

## Notes

- `affected` expands matched file containers to contained symbols, then walks reverse graph dependents within the requested depth.
- JSON includes `matched_files`, `unmatched_paths`, `impacted_symbols`, `impacted_routes`, `impacted_tests`, `blind_spots`, and `next_commands`.
- Test suggestions are ranked from indexed test-like paths reached by the graph. Empty test suggestions mean "not found in graph", not "no tests exist".
- Route suggestions come from typed route/endpoint metadata when it is present. Empty route suggestions mean no route evidence was found in the matched graph slice, not that routes are unaffected.
- `unmatched_paths` are workflow evidence, not noise. When paths do not match,
  rerun `files --path <fragment>` or refresh the index before assuming the graph
  is wrong.
- If `affected` reports stale or partial coverage, run `doctor --project .` and
  consider `index --project . --refresh full` before using the impact list to
  narrow verification.
