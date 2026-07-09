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
| `sidecar_setup` | Plugin sidecar setup policy and last repair state. | Diagnostic sidecar policy detail. For agent repair, follow `recommended_next_calls` and prefer MCP `sidecar_setup` with `action=repair`. |
| `readiness_broker` | Durable repair, local refresh, native embedding resource ownership, stale-lock reconciliation, persistence status, and GPU proof (`proof_status`, `embed_smoke_ok`, `embed_smoke_ms`). | Inspect before retrying repair. A foreign or unverifiable `native_embedding_runtime` busy state blocks repair; a same-project reusable native owner should be followed through `recommended_next_calls`. `gpu_proof.verified` requires observed acceleration plus a live timed embed smoke when accelerator is required. |
| `embedding_launch_metadata.launch_mode` | Embedding sidecar launch mode when available, such as `native_spawned` or Docker Compose embed. | On macOS arm64, expect `native_spawned` for accelerated Metal; Docker/Vulkan on Apple Silicon is stale or unrepaired. |
| `embedding_accelerator_request_provider` / `embedding_accelerator_request_device` | Requested accelerator provider/device. These fields are intent, not proof. | Use them to diagnose mismatched requests, for example `metal` with no device on Apple Silicon versus `vulkan`/`Vulkan0` on Windows or Linux Vulkan paths. |
| `embedding_device_state` / `embedding_device_observation_source` | Observed embedding device state and where the observation came from. | Treat `accelerated` as acceleration proof when paired with full retrieval status. Treat `accelerator_request_unobserved` as blocked unless CPU mode is explicitly allowed. |
| `allowed_surfaces.<surface>.allowed` | A concrete MCP surface is allowed. | Use local graph entries such as `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` only when their surface is allowed. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Sidecar-backed agent surfaces are allowed. | Use `packet`, `search`, and `context` confidently when their own allowed bit is true and `retrieval_mode=full`. |

## Runtime Repair

Use managed `codestory://status`, release install records, or source-build
checks only for maintainer/debug transcripts when MCP is missing or suspect.
They are not the supported agent repair path. `CODESTORY_CLI` is an explicit
local-dev override; installed `.mcp.json` launches the managed adapter first,
provisions from `github_release` when needed, and records the launch source in
`plugin_runtime`. If the managed runtime cannot spawn or be provisioned, the
adapter stays up with `repair_setup` diagnostics instead of closing transport.

If `codestory://status` reports a repairable state, run the MCP
`recommended_next_calls` loop: call `sidecar_setup` with `action=repair` when
recommended, then reread `codestory://status` before local navigation, packet,
search, or context. Do not ask the human to install the binary unless network,
permissions, host reload, or release assets block the repair.
