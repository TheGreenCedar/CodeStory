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
| `unavailable` | CodeStory could not converge within the managed path. | Use focused source inspection and state the evidence gap. |

`current_operation` is the runtime-owned activation snapshot. When present it
contains one stable `operation_id`, monotonic `revision`, `stage`, `attempt`,
and `progress`, plus retry delay and failure. Concurrent and serial retries for
the same native project/configuration key join that operation; they do not
start another refresh or repair flow. A `retained` local-navigation capability
names the exact complete core publication still usable for observational local
analysis after a replacement fails. It never upgrades broad search without the
matching retrieval publication and runtime proof.

When no complete publication exists yet, `ground`, `packet`, `search`, and
`context` return a successful structured result with `code=codestory_preparing`,
`state=preparing`, `retry_tool`, and `retry_after_ms`. That result is not
`isError: true`; hosts that stop on errors would never honor the delay.
`unavailable` remains an error. Broad tools use the same preparing response
while managed search is coming up. Retry that same tool. Do not ask the user to
enable, repair, approve, or configure an internal service.

While the plugin launcher is still provisioning the managed runtime itself,
`retry_after_ms` derives from the observed runtime-download progress: the
estimated remaining transfer time at the observed throughput, clamped between
250 ms and 10 s. A state with no measurable transfer reports the 1500 ms
fallback instead. Preparing responses repeat the same value inside the embedded
`operation` snapshot, and a preparing entry in `recommended_next_calls` carries
it as `after_ms`.

`ground`, `files`, and `affected` can build or refresh the bounded local map as
part of the call. Once a complete publication exists, local graph tools keep
using it during refresh and never read a half-published generation.

## Diagnostic status

`codestory://status{?project}` is an observational diagnostic resource
template. Expand `project` with the percent-encoded absolute repository root;
the returned content URI remains bound to that canonical root.
`codestory://agent-guide` is the project-free static resource. Read status only
when the direct tool loop stops converging, the tool reports stale evidence, or
the task explicitly asks for runtime diagnostics. A status read never starts
work.

The most useful fields are:

| Field | Meaning |
| --- | --- |
| `server_version`, `server_executable`, `server_executable_sha256` | Exact live MCP runtime identity. |
| `plugin_runtime` | Installed plugin and managed CLI source. |
| `runtime_truth` | Compact references to the canonical readiness and runtime fields. |
| `index_publication` | Complete core database generation currently being served. |
| `local_refresh` | Local map state and the complete publication retained during refresh. |
| `state`, `capabilities`, `current_operation`, `retry_after_ms`, `failure` | Uncached activation progress layered onto the observational status read, including one stable operation id, stage, attempt, retry delay, and terminal failure. |
| `retrieval_mode` | Persisted broad-search publication class; `full` is required infrastructure for trustworthy broad results, not proof that packet may run now. |
| `degraded_reason`, `live_ready` | Live readiness beside `retrieval_mode`. `live_ready` is true only when `retrieval_mode=full` and `degraded_reason` is absent. |
| `embedding_server` in maintainer diagnostics | Endpoint authority, listener, server process, query/bulk capacity and depth, opaque active request/phase, engine owner/native worker, load generation, and model-load identity. Project paths and request text are never included. |
| `readiness_lanes.agent_packet_search` | Current broad-search capability state. |
| `runtime_update` | Non-blocking installed-runtime update advisory. |

Reuse a status result until repository, runtime, or index state changes. Follow
its references instead of treating duplicated nested payloads as separate
truths.

## Evidence boundary

Successful broad MCP responses include `_meta.codestory_publication` with the
complete core and retrieval publication identities used by the runtime-owned
operation. Treat that metadata as the response's evidence boundary rather than
re-reading status around the call.

The envelope also carries `schema_version`, `minimum_compatible_schema_version`,
and `contract_runtime`. The last records the active CLI, the launcher-provided
plugin/CLI pair, whether that pair matches, and whether the explicit
`CODESTORY_CLI` skew channel is active. A missing schema version is the legacy v0
envelope. CLI JSON, HTTP, and MCP use the same stamp. When neither a core nor
retrieval identity exists, the stamp remains present with
`served_from=contract_only`; it must not claim a complete publication.

Schema version 2 is the v0.17.0 contract. It is not a purely additive bump:
`tools/call` arguments are validated against the published catalog and rejected
with JSON-RPC `-32602` instead of being repaired, so a client written against
schema 1 can see valid-looking requests refused. Compare against
`minimum_compatible_schema_version` rather than assuming forward compatibility.

The `initialize` result carries the same stamp plus `_meta.codestory_protocol`,
which reports the requested revision, the negotiated revision, every revision the
server implements, and whether the two agree. The server answers with a revision
it actually implements; it never echoes an unsupported one back as supported.
Through the packaged plugin the launcher answers `initialize` for the host, so
the handshake stamp is the launcher's: it is always `served_from=contract_only`
and reports the response contract version and the pinned pair, never a
publication identity. Read publication identities from a tool result.

Local navigation is useful while broad search prepares, but it is not full
retrieval proof. Trust a broad result only when the requested tool succeeds
against a current complete publication. Under accelerator-required policy,
maintainer proof additionally requires the exact engine/model identity, a
physical non-software adapter, and verified accelerator work.

## Maintainer recovery

CLI status, doctor, install records, and
the project-bound `codestory://diagnostics/retrieval-engine{?project}` resource
are maintainer surfaces. The engine diagnostic reports the live model digest,
linked ggml build, selected adapter, policy, smoke timing, and per-user
server/model-load identity. It is intentionally absent from the normal resource
catalog and user flow. Use these surfaces only after automatic retries stop
converging or when collecting an explicit proof transcript. `CODESTORY_CLI`
remains a local-development override; installed plugin sessions use the managed
launcher.
