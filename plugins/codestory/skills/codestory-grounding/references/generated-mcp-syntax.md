<!-- Compact projection of plugins/codestory/generated-mcp-catalog.json. plugin-static checks tool names. -->
# Generated CodeStory MCP syntax

This is the agent option source of truth for the packaged plugin. Every tool
requires `project` (absolute repository root). Extra fields fail with JSON-RPC
`-32602` because the catalog sets `additionalProperties: false`.

Clap `--help` and [generated CLI syntax](generated-cli-syntax.md) are maintainer
CLI docs. Do not send CLI flags as MCP arguments.

Live tools: `status`, `packet`, `search`, `ground`, `files`, `affected`,
`symbol`, `trail`, `callers`, `callees`, `trace`, `get_node`, `neighbors`,
`shortest_path`, `query_subgraph`, `definition`, `references`, `symbols`,
`snippet`, `context`, `verify_indexed_direct_calls`.

There is no MCP `index`, `doctor`, `ready`, `explore`, `drill`, `query`,
`bookmark`, `serve`, or `cache` tool. Product tools own activation.

## Tool arguments

| Tool | Required besides `project` | Optional | Notes |
| --- | --- | --- | --- |
| `status` | | | Observational. Do not call first. |
| `packet` | `question` | `budget`, `task_class`, `probes`, `extra_probes`, `latency_budget_ms`, continuation `parent_packet_id` / `option_ids` / generation pins | Broad evidence questions. No `include_evidence`. |
| `search` | `query` | `limit`, `repo_text` (`auto`/`on`/`off`) | Discovery, not packet recovery. |
| `ground` | | `budget` (`strict`/`balanced`/`max`) | First call may refresh the local map. |
| `files` | | `language`, `path`, `role`, `limit` | Refreshes the local map before dispatch. No `refresh` field. |
| `affected` | exactly one of `paths`, `changed_paths`, `change_records` | `depth`, `filter` | Never discovers git changes. |
| `symbol` | `query` or `id` | `choose` | |
| `trail` | `query` or `id` | `direction` (`incoming`/`outgoing`/`both`), `depth`, `max_nodes`, `story`, `choose` | There is no `mode` field. |
| `callers` | `query` or `id` | `depth`, `max_nodes`, `choose` | |
| `callees` | `query` or `id` | `depth`, `max_nodes`, `choose` | |
| `trace` | `query` or `id` | `direction`, `depth`, `max_nodes`, `story`, `choose` | |
| `get_node` | `query` or `id` | `choose` | |
| `neighbors` | `query` or `id` | `direction`, `depth`, `max_nodes`, `choose` | |
| `shortest_path` | `from_id`, `to_id` | `max_depth`, `max_nodes` | |
| `query_subgraph` | `query` or `id` | `direction`, `depth`, `max_nodes`, `choose` | Not a substitute for `packet`. |
| `definition` | `query` or `id` | `choose` | |
| `references` | `query` or `id` | `choose` | Incoming references. |
| `symbols` | | `parent_id`, `limit` | Root symbols, or children of `parent_id`. |
| `snippet` | `query`, `id`, `paths`, `path`, `file_path`, or `symbol_id` | `line`, `start_line`, `end_line`, `context`, `lines`, `scope`, `function_body`, `choose` | After packet/search/graph selects targets. |
| `context` | `query`, `id`, or `bookmark` | `include_evidence`, `max_results` | One concrete target, not a broad question. |
| `verify_indexed_direct_calls` | `call_path` | | Observational exact verification of a `call-path/v1` document (see below). Never translate free English into one, and never invoke this tool automatically. |

### `call-path/v1`

`call_path` is a text document, not JSON. One contract per document, one clause
per line:

```text
call-path/v1
start: crate::module::Alpha
step 1: direct call -> crate::module::Beta
step 2: direct call -> "src/gamma.rs"::Gamma
prohibit traversal through: crate::detail::Helper
exclude from projection: crate::test_support
```

The version line comes first. `start` and at least one `step` are required;
steps are numbered consecutively from 1, up to 6. The `prohibit traversal
through` and `exclude from projection` lines are optional and repeatable.
Selectors are qualified names, or `"path/to/file"::Name` to scope one file.
Signatures, wildcards, and internal node identities are not selectors.

Blank lines and indentation are ignored. Any other line the grammar cannot read
becomes an unresolved clause, and the whole verification then reports
`graph_disposition: "unknown"` instead of proving a smaller contract than you
wrote. The document is capped at 8192 bytes.

## Resources and prompts

Project-scoped resources use `{?project}` templates, for example
`codestory://status{?project}`. `codestory://agent-guide` is static and
project-free.

Host prompts `explain_symbol`, `trace_callflow`, and `impact_analysis` exist
only if the host exposes them. Prefer the matching tool.

## Wire profiles

CodeStory supports MCP revisions `2024-11-05`, `2025-03-26`, `2025-06-18`,
and `2025-11-25`, preferring the newest. The 2024 profile lists only
`name`/`description`/`inputSchema`; March adds annotations; June and November
add `title`, `outputSchema`, and Tool `_meta`. Older profiles return one JSON
text object. Modern profiles return schema-valid structured content and the
identical JSON text object. Tool errors are text-only in every profile.

CodeStory publication stamps use schema 3 with minimum compatible schema 3.
Each negotiated profile has its own discovery digest. The 2024 and March
profiles accept ordered JSON-RPC batches and omit notification responses; June
and November reject arrays with `-32600`.
