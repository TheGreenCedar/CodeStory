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
`snippet`, `context`.

There is no MCP `index`, `doctor`, `ready`, `explore`, `drill`, `query`,
`bookmark`, `serve`, or `cache` tool. Product tools own activation.

## Tool arguments

| Tool | Required besides `project` | Optional | Notes |
| --- | --- | --- | --- |
| `status` | | | Observational. Do not call first. |
| `packet` | `question` | `budget`, `task_class`, `probes`, `extra_probes`, `include_evidence`, `latency_budget_ms`, DrillOnce `parent_packet_id` / `option_ids` / generation pins | Broad questions. |
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

## Resources and prompts

Project-scoped resources use `{?project}` templates, for example
`codestory://status{?project}`. `codestory://agent-guide` is static and
project-free.

Host prompts `explain_symbol`, `trace_callflow`, and `impact_analysis` exist
only if the host exposes them. Prefer the matching tool.
