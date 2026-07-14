# CodeStory Tool State

Call the tool that matches the repository question and pass the same absolute
`project` path to every call. Do not read status first. CodeStory owns local
map refresh, managed search preparation, retry cooldowns, and runtime reuse
across repositories.

## Normal tool loop

| State | Meaning | Agent action |
| --- | --- | --- |
| `ready` | The requested capability is available. | Use the result. |
| `preparing` | CodeStory is starting or updating managed search. | Wait `retry_after_ms`, then retry the same tool with the same arguments. |
| `updating` | The repository map is moving to a new complete publication. | Retry the same tool after the reported delay; do not start another refresh. |
| `working_locally` | Local graph navigation is available while broad search prepares. | Continue with local tools and retry the original broad tool later. |
| `needs_environment` | Automatic preparation found one host requirement it cannot satisfy. | Report that single requirement in plain language. |
| `unavailable` | CodeStory could not converge within the managed path. | Use focused source inspection and state the evidence gap. |

`packet`, `search`, and `context` return `codestory_preparing` with
`retry_tool` and `retry_after_ms` while their managed dependencies are coming
up. Retry that same tool. Do not ask the user to enable, repair, approve, or
configure an internal service.

`ground`, `files`, and `affected` can build or refresh the bounded local map as
part of the call. Other local graph tools use the last complete publication and
never read a half-published generation.

## Diagnostic status

`codestory://status` is an observational diagnostic surface. Read it only when
the direct tool loop stops converging, the tool reports stale evidence, or the
task explicitly asks for runtime diagnostics. A status read never starts work.

The most useful fields are:

| Field | Meaning |
| --- | --- |
| `server_version`, `server_executable`, `server_executable_sha256` | Exact live MCP runtime identity. |
| `plugin_runtime` | Installed plugin and managed CLI source. |
| `runtime_truth` | Compact references to the canonical readiness and runtime fields. |
| `index_publication` | Complete core database generation currently being served. |
| `local_refresh` | Local map state and the complete publication retained during refresh. |
| `managed_retrieval` | Automatic broad-search lifecycle state. This is diagnostic, not a user control. |
| `retrieval_mode` | Persisted broad-search classification; `full` is required for trustworthy broad results. |
| `readiness_lanes.agent_packet_search` | Current broad-search capability state. |
| `readiness_broker` | Maintainer evidence for ownership, liveness, and accelerator proof. |
| `retrieval_diagnostics` | Detailed managed-runtime evidence for debugging. |
| `runtime_update` | Non-blocking installed-runtime update advisory. |

Reuse a status result until repository, runtime, or index state changes. Follow
its references instead of treating duplicated nested payloads as separate
truths.

## Evidence boundary

Local navigation is useful while broad search prepares, but it is not full
retrieval proof. Trust a broad result only when the requested tool succeeds
against a current complete publication. Under accelerator-required policy,
maintainer proof additionally requires a live selected endpoint, matching
process identity, and verified accelerator work.

Stale ownership remains fail-closed. Heartbeat age alone does not prove that a
process is abandoned, and CodeStory does not terminate an owner on that basis.

## Maintainer recovery

CLI status, doctor, install records, process IDs, ports, model paths, and backend
logs are maintainer diagnostics. They are not the normal agent or user repair
path. Use them only after automatic retries stop converging or when collecting
an explicit proof transcript. `CODESTORY_CLI` remains a local-development
override; installed plugin sessions use the managed launcher.
