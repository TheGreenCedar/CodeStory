# Status Contract

When plugin MCP is live, `codestory://status` is the first and canonical runtime
truth. Read it before `ground`, `files`, `packet`, or `search`. This document is
the agent-facing field glossary alongside runtime `codestory://agent-guide`.

## Status Fields

| Status field | Meaning | Agent action |
| --- | --- | --- |
| `server_version` | Version of the active MCP server binary. | Use as active runtime evidence once MCP is live. |
| `cli_version` | Version reported by the active CLI runtime. | Use as the active CLI version, not the source checkout version. |
| `server_executable` | Executable path serving this MCP session. | Use as active runtime evidence; do not guess from source paths. |
| `server_executable_sha256` | Checksum of the active MCP server binary when available. | Use to confirm the exact runtime binary after install, repair, or reload. |
| `sidecar_contract_version` | Sidecar schema contract compiled into the active CLI. | Use to diagnose sidecar/runtime contract drift. |
| `plugin_runtime` | Plugin launch source and managed CLI metadata, including `plugin_runtime.plugin_root`, `plugin_cache_version`, `build_source`, and `repo_ref` when provisioned. | Treat `managed` as installed plugin runtime, `local_dev_override` as source/dev override, and `managed_unavailable` as blocked managed setup. |
| `runtime_truth` | Grouped runtime source, plugin root, managed CLI path, launcher source, sidecar policy/status, and readiness lanes. | Use as the concise bounded runtime summary; fall back to the source fields when a nested value needs detail. |
| `sidecar_setup` | Plugin sidecar setup policy and last repair state. | Ask before first automatic sidecar setup; respect `enabled` and `disabled`. |
| `allowed_surfaces.<surface>.allowed` | A concrete MCP surface is allowed. | Use local graph entries such as `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` only when their surface is allowed. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Sidecar-backed agent surfaces are allowed. | Use `packet`, `search`, and `context` confidently when their own allowed bit is true and `retrieval_mode=full`. |

## Runtime Repair

Use `where.exe codestory-cli`, `codestory-cli --version`, release install, or
source-build checks only when MCP is missing, the plugin needs repair, or the
user asks for a CLI transcript. `CODESTORY_CLI` is an explicit local-dev
override; installed `.mcp.json` launches the managed adapter first, provisions
from `github_release` when needed, and records the launch source in
`plugin_runtime`. If the managed runtime cannot spawn or be provisioned, the
adapter stays up with `repair_setup` diagnostics instead of closing transport.
Any `PATH` candidates in status are diagnostic only and are not launched by the
installed plugin runtime.

If `codestory://status` reports `repair_setup` because the active
`server_version` is older than the latest release, repair the CLI before local
navigation, packet, search, or context. The agent runs the installer command from `recommended_next_calls`; do not ask the human to install the binary unless
network, permissions, or release assets block the repair.
