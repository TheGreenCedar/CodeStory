# Status Contract

When plugin MCP is live, the `status` tool with an explicit absolute `project`
is the first and canonical runtime truth. Call it before `ground`, `files`,
`packet`, or `search`, and pass the same project to every tool. The server has no
global workspace binding.

Reuse a status result until repository, runtime, or index state changes, or a
tool reports stale evidence or a freshness failure. A blocked sidecar-backed
surface does not invalidate an allowed local graph route.

## Status Fields

| Status field | Meaning | Agent action |
| --- | --- | --- |
| `server_version` | Version of the active MCP server binary. | Use as active runtime evidence once MCP is live. |
| `cli_version` | Version reported by the active CLI runtime. | Use as the active CLI version, not the source checkout version. |
| `server_executable` | Executable path serving this MCP session. | Use as active runtime evidence; do not guess from source paths. |
| `server_executable_sha256` | Checksum of the active MCP server binary when available. | Use to confirm the exact runtime binary after install, repair, or reload. |
| `runtime_update` | Non-blocking release and managed-install advisory. `state=available` reports an optional update; `restart_recommended=true` means a newer checksum-valid managed CLI is already installed. Release metadata is cached and refreshed outside the status request path. | Do not treat update availability as a readiness failure. Continue using each compatible surface according to `allowed_surfaces`; reload the host when convenient if restart is recommended. |
| `sidecar_contract_version` | Sidecar schema contract compiled into the active CLI. | Use to diagnose sidecar/runtime contract drift. |
| `plugin_runtime` | Plugin launch source and managed CLI metadata, including `plugin_runtime.plugin_root`, `plugin_cache_version`, `build_source`, and `repo_ref` when provisioned. | Treat `managed` as installed plugin runtime, `local_dev_override` as source/dev override, and `managed_unavailable` as blocked managed setup. |
| `runtime_truth` | Grouped runtime source, plugin root, managed CLI path, launcher source, and references to canonical readiness fields. | Use as the concise bounded runtime identity summary. Follow its `*_ref` fields into top-level status instead of expecting cloned readiness payloads. |
| `sidecar_setup` | Plugin sidecar setup policy and last repair state. | Diagnostic sidecar policy detail. For agent repair, follow `recommended_next_calls` and prefer MCP `sidecar_setup` with `action=repair`. |
| `readiness_broker` | Durable repair, local refresh, native embedding resource ownership, stale-lock reconciliation, persistence status, and GPU proof (`proof_status`, `embed_smoke_ok`, `embed_smoke_ms`). | Inspect before retrying repair. A foreign or unverifiable `native_embedding_runtime` busy state blocks repair; a same-project reusable native owner should be followed through `recommended_next_calls`. `gpu_proof.proof_status == "verified"` and `gpu_proof.meaningful_accelerator_work_proven == true` require observed acceleration plus a live timed embed smoke when accelerator is required. |
| `index_publication` | Durable identity of the complete core database generation currently served at the live path. It is null when the live database is fenced by an incomplete legacy run. | Use its generation, generation ID, run ID, mode, and publication time to distinguish old-or-new complete reads during refresh. |
| `local_refresh` | Single-flight local refresh state and owner metadata. While `state=refreshing`, `serving_publication` identifies the last complete generation that remains readable. | Continue using local surfaces whose allowed bit is true. Do not treat staged work as live. Read tool-call `_meta.codestory_publication` identifies the exact complete response generation; `served_from=last_complete_publication` means a writer was refreshing concurrently. |
| `embedding_launch_metadata.launch_mode` | Embedding sidecar launch mode when available, such as `native_spawned` or Docker Compose embed. | On macOS arm64, expect `native_spawned` for accelerated Metal; Docker/Vulkan on Apple Silicon is stale or unrepaired. |
| `embedding_accelerator_request_provider` / `embedding_accelerator_request_device` | Requested accelerator provider/device. These fields are intent, not proof. | Use them to diagnose mismatched requests, for example `metal` with no device on Apple Silicon versus `vulkan`/`Vulkan0` on Windows or Linux Vulkan paths. |
| `embedding_device_state` / `embedding_device_observation_source` | Observed embedding device state and where the observation came from. | Treat these as proof inputs. `manual_env` and device-inventory observations remain diagnostic; complete accelerator proof requires the verified broker GPU proof above. Treat `accelerator_request_unobserved` as blocked unless CPU mode is explicitly allowed. |
| `allowed_surfaces.<surface>.allowed` | A concrete MCP surface is allowed. Ordinary surface entries stay compact and name their canonical verdict with `readiness_goal`; failure summaries are not cloned into every entry. | Use local graph entries such as `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` only when their surface is allowed. Follow `readiness_goal` into top-level `readiness` for full diagnostics. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Sidecar-backed agent surfaces are allowed. | Use `packet`, `search`, and `context` confidently when their own allowed bit is true and `retrieval_mode=full`. |

New background repair failures record one shared `terminal_envelope`. Treat
that structured envelope as the primary failure contract. A persisted result
created by an older runtime can omit it; in that compatibility case, use
`wait_error`, then the bounded stderr/stdout tails. Do not mistake either form
of terminal evidence for an active repair lock.

## Runtime Repair

Use managed project-scoped `status`, release install records, or source-build
checks only for maintainer/debug transcripts when MCP is missing or suspect.
They are not the supported agent repair path. `CODESTORY_CLI` is an explicit
local-dev override; installed `.mcp.json` launches the managed adapter first,
provisions from `github_release` when needed, and records the launch source in
`plugin_runtime`. If the managed runtime cannot spawn or be provisioned, the
adapter stays up with `repair_setup` diagnostics instead of closing transport.

If project-scoped `status` reports a repairable state and the task requires the
blocked surface, run the MCP `recommended_next_calls` loop: call
`sidecar_setup` with the same `project` and `action=repair` when recommended,
then call `status` again before using that surface. When an allowed local graph
surface satisfies the task, use it without repairing packet/search/context and
do not represent local navigation as full retrieval proof. Do not ask the human to install the binary unless network,
permissions, host reload, or release assets block the repair.
