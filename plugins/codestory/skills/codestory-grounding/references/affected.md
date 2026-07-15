# `affected` - Changed-File Impact Analysis

Maps changed files to indexed symbols, bounded dependent graph walks, and likely
test files. Use it to choose focused regression checks before running broader
repo gates.

## Usage

```
<codestory-cli> affected [PATH ...] [OPTIONS]
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `PATH ...` | none | Changed repo-relative paths. If omitted, CodeStory reads `git diff --name-status HEAD`. |
| `--stdin` | off | Read changed paths or name-status rows from stdin and combine them with path args. |
| `--stdin-format <path|name-status>` | `path` | Interpret stdin as path-only lines or `git diff --name-status` rows. |
| `--changes <head|staged|unstaged|untracked>` | `head` | Select the default git source when no paths/stdin are supplied. |
| `--depth <n>` | `2` | Dependent graph walk depth, clamped by the runtime. |
| `--filter <text>` | none | Keep impacted symbols whose display name or path contains the text. |
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Current diff | `<codestory-cli> affected --project <target-workspace> --format markdown` | Impact summary based on `git diff --name-status HEAD`. |
| Explicit paths | `<codestory-cli> affected --project <target-workspace> src/lib.rs --depth 3 --format json` | Matched/unmatched paths, impacted symbols, impacted routes/endpoints, likely tests, blind spots, and next commands. |
| MCP explicit paths | `tools/call affected` with `changed_paths` or `change_records` | Same local-index impact DTO; stdio never discovers git changes, refreshes, indexes, or bootstraps retrievals. |
| Stdin paths | `git diff --name-only HEAD | <codestory-cli> affected --project <target-workspace> --stdin` | Same analysis using external path selection. |
| Stdin status | `git diff --name-status HEAD | <codestory-cli> affected --project <target-workspace> --stdin --stdin-format name-status --format json` | Preserves add/modify/delete/rename/copy status in `change_records`. |

## Notes

- `affected` expands matched file containers to contained symbols, then walks reverse graph dependents within the requested depth.
- JSON includes `change_records`, `matched_files`, `unmatched_paths`, `impacted_symbols`, `impacted_routes`, `impacted_tests`, `blind_spots`, and `next_commands`.
- Rename/copy rows preserve `previous_path`; deleted and untracked files are reported with specific unmatched-path reasons when the index cannot match them.
- Test suggestions are ranked from indexed test-like paths reached by the graph. Empty test suggestions mean "not found in graph", not "no tests exist".
- Route suggestions come from typed route/endpoint metadata when it is present. Empty route suggestions mean no route evidence was found in the matched graph slice, not that routes are unaffected.
- `unmatched_paths` are workflow evidence, not noise. When paths do not match,
  rerun `files --path <fragment>` or refresh the index before assuming the graph
  is wrong.
- If `affected` reports stale or partial coverage, run `doctor --project <target-workspace>` and
  consider `index --project <target-workspace> --refresh full` before using the impact list to
  narrow verification.
