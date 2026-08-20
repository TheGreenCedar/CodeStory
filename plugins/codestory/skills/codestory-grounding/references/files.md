# `files` - Indexed File Inventory

Lists files known to the persisted CodeStory index. Use it to inspect coverage,
language mix, inferred roles, and partial-index markers before making broad
claims about what the graph can see.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Inventory | MCP `files` with `project` | Language counts and a capped file list. |
| Coverage check | MCP `files` with `language` | File rows for that language. |
| Test discovery | MCP `files` with `role=test` | Test-like files inferred from path/name conventions. |

## Notes

- MCP `files` has no `refresh` field. The catalog tool refreshes the local map
  before dispatch and does not wait for broad search.
- Treat `index usable` with incomplete or error counts as a partial-coverage signal, not a failure.
- `summary.framework_route_coverage` is the support matrix for framework route extraction. It includes `status`, `coverage_evidence`, `confidence_floor`, `handler_link_support`, `unsupported_patterns`, `known_gaps`, and `promotable`. Treat `partial`, `heuristic`, text-only handler support, and `promotable=false` as review prompts, not proof of full framework parity.
- Route coverage statuses:
  - `supported`: fixture-backed behavior is passing and documented coverage is met.
  - `heuristic`: pattern-backed evidence that needs source review.
  - `partial`: some cases are covered, but known route shapes, handler links, or fixtures are missing.
  - `unsupported`: no support claim is made.
  - `stale`: refresh before promoting the claim.
  - `non-promotable`: required fixtures, known-gap notes, or eval evidence are missing or failing.
- Role inference is path/name based. It is useful for navigation and test selection, but not a formal build-system classification.
