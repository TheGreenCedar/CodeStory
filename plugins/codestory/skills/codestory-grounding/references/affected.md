# `affected` - Changed-File Impact Analysis

Maps changed files to indexed symbols, bounded dependent graph walks, and likely
test files. Use it to choose focused regression checks before running broader
repo gates.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| MCP simple paths | `affected` with `paths` | Preferred MCP shape for project-relative paths. |
| MCP compatibility paths | `affected` with `changed_paths` | Compatibility alias. Do not combine with another input source. |
| MCP status-rich input | `affected` with `change_records` | Preserves add/modify/delete/rename/copy status. |

## Notes

- `affected` expands matched file containers to contained symbols, then walks reverse graph dependents within the requested depth.
- MCP accepts exactly one input property, even when an array is empty. Each input array must contain 1-200 paths or records, and path strings must be non-empty. Invalid input is rejected before project activation; mixed properties fail with `affected_input_conflict` instead of silently choosing one.
- Analysis reads the last complete local index and computes one uncached, bounded workspace observation from the same storage snapshot. A complete inventory distinguishes admitted stale paths from valid files excluded by index policy; incomplete inventory or metadata evidence remains `unavailable_evidence`. Path-resolution failures abort the request.
- JSON includes `change_records`, `matched_files`, `unmatched_paths`, `uncovered_inputs`, `impacted_symbols`, `impacted_routes`, `impacted_tests`, `bounds`, `completeness`, and `follow_ups`. Follow-up commands are structured as optional `{program, args}` invocations; only CLI markdown renders shell text.
- Uncovered inputs are classified from positive evidence as `valid_uncovered`, `missing`, `expected_deleted`, `rename_unresolved`, `stale_index`, `malformed`, or `unavailable_evidence`.
- A present regular file such as an SVG outside graph coverage is `valid_uncovered`. An indexable file excluded from the complete admitted inventory, such as an ignored generated source, is also `valid_uncovered` rather than stale. Directories are `malformed`; resolution errors and paths outside the project abort rather than producing a positive class.
- `previous_path` is accepted only on rename/copy rows. The current path alone owns matched/uncovered classification; when it has no indexed identity, the previous path may seed bounded proxy graph evidence without turning the current path into a match. Deleted and untracked files retain current-path behavior and their submitted status.
- Test suggestions are ranked from indexed test-like paths reached by the graph. Empty test suggestions mean "not found in graph", not "no tests exist".
- Route suggestions come from typed route/endpoint metadata when it is present. Empty route suggestions mean no route evidence was found in the matched graph slice, not that routes are unaffected.
- `completeness.complete=false` or `completeness.truncated=true` blocks a complete no-impact claim. Runtime and MCP transport caps both degrade these nested fields and append field-specific reasons with original totals; read them before narrowing verification.
- Follow-ups are conditional. Complete fresh analysis emits none; valid uncovered assets explain the graph boundary without recommending reindex; only exact stale evidence for a requested path recommends incremental refresh; unrelated workspace staleness remains a blind spot instead of becoming a repair command; missing or ambiguous paths recommend a focused `files --path` lookup.
