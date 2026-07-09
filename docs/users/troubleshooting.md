# Troubleshooting

Fix a blocked or stale CodeStory session. Start with the decision tree, then
work through the steps in order.

Trust boundaries: [Trust and readiness](trust-and-readiness.md). Terms:
[Glossary](../glossary.md).

**Need JSON field names?** Common status keys: `allowed_surfaces`,
`retrieval_mode`, and `recommended_next_calls`. Full agent contract:
[status-contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md).
CLI commands are maintainer/debug transcripts: [CLI reference](cli-reference.md#readiness-and-repair).

## Repair quick reference

| Symptom | Supported action | Check in output |
| --- | --- | --- |
| Repo map stale or blocked | Agent reads `codestory://status`, follows `recommended_next_calls`, then rereads status | Local graph surfaces are allowed after the reread |
| Broad search blocked | Same MCP `sidecar_setup repair` loop | `packet`, `search`, or `context` allowed and `retrieval_mode` is `full` |
| MCP down, need handoff | Reload/fix host MCP; CLI can collect a debug transcript only | `codestory://status` becomes visible in the agent host |
| Sidecar health | MCP status first; CLI `retrieval status` only for maintainer evidence | `retrieval_mode` is `full` before trusting packet/search |

## Decision tree

```mermaid
flowchart TD
  start([Session feels wrong]) --> q1{Can the agent read<br/>CodeStory status?}
  q1 -->|No| mcp[MCP not connected]
  q1 -->|Yes| q2{Is the repo map ready?}
  q2 -->|No| local[Local navigation lane]
  q2 -->|Yes| q3{Need packet or search?}
  q3 -->|No| ok[Local tools should work]
  q3 -->|Yes| q4{Is broad search ready?}
  q4 -->|No| sidecar[Packet/search lane]
  q4 -->|Yes| ok2[Broad search should work]
  mcp --> fix_mcp[MCP registration]
  local --> fix_local[Refresh or repair local index]
  sidecar --> fix_sidecar[Sidecar setup or repair]
  fix_mcp --> host[Host guide]
  fix_local --> reread
  fix_sidecar --> mcp_repair
  mcp_repair --> reread["Reread codestory://status"]
  host --> codex[Codex guide]
  host --> cursor[Cursor guide]
  host --> claude[Claude Code guide]
  host --> copilot[Copilot guide]
```

## Host x symptom

| Symptom | Codex | Cursor | Claude Code | Copilot |
| --- | --- | --- | --- | --- |
| MCP missing | Fresh thread after `/plugins` install | Check `.cursor/mcp.json`; reload MCP server | MCP configured separately from hooks | MCP not auto-started; configure or use CLI |
| Stale index / wrong symbols | New thread; hooks refresh on session start | Reload MCP; run local repair | New session; run local repair | New session; run [local repair](cli-reference.md#readiness-and-repair) |
| Packet/search blocked | Agent calls MCP `sidecar_setup repair` when status says so | Same; verify retrieval mode | Same | Use CLI [retrieval status](cli-reference.md#readiness-and-repair) only as a debug transcript |
| Version drift after update | Refresh marketplace, refresh plugin package, restart host, fresh status read | Reload MCP server | Restart session | Reinstall or point to current binary |

Host-specific steps: [Codex](codex.md#troubleshooting), [Cursor](cursor.md#troubleshooting), [Claude Code](claude-code.md#troubleshooting), [Copilot](copilot.md).

## Good session vs blocked session

Examples in plain English. Full trust rules: [Trust and readiness](trust-and-readiness.md).

**Good.** You ask "Where is `parse_config` defined?" The agent names a file
under `src/`, lists two callers, and those paths open correctly in your editor.

**Blocked (local).** The agent says a symbol does not exist even though you can
grep it, or cites files that were deleted last week. The repo map is stale or
not built.

**Good (broad search).** You ask "How does indexing flow from workspace
discovery to SQLite?" The agent says broad search is ready, returns a compact
answer with multiple cited files, and each path exists.

**Blocked (broad search).** The agent gives a long essay with no file citations,
or says packet/search is unavailable. Do not treat the answer as proof; repair
sidecars or ask narrower local questions.

## Step 1 -- Is my repo map ready?

**You:** In a fresh session, ask yourself:

- Can the agent find symbols and cite real file paths?
- Do trails and snippets match what is on disk?

If yes, local navigation is likely good. If no, go to [Local navigation stale or blocked](#local-navigation-stale-or-blocked).

<details>
<summary>Agent prompt (secondary)</summary>

Ask the agent:

```text
Read codestory://status, report allowed_surfaces, and tell me what is blocked and the next repair action.
```

The agent uses MCP status, `codestory://agent-guide`, and `sidecar_setup repair`
when status recommends repair. Re-read status after any repair.

</details>

If MCP is not connected, go to step 2.

## Step 2 -- CLI health transcript (power user)

**You:** Run diagnostics when MCP is missing or status looks wrong. Full command
reference: [CLI reference](cli-reference.md).

```sh
codestory-cli agent preflight --project <repo> --format json
codestory-cli doctor --project <repo>
```

**Agent:** Treats CLI output as a debug transcript only. CLI output does not
make CodeStory MCP live in the agent host.

On Windows PowerShell, use `.\target\release\codestory-cli.exe` for a
source-built binary.

## Local navigation stale or blocked

Symptoms: missing symbols, old file list, `ground` or `files` not allowed.

**Agent (MCP live):** Use allowed local graph tools only; request index refresh
through status guidance.

**You (CLI debug transcript):**

```sh
codestory-cli fix --project <repo> --format json
codestory-cli doctor --project <repo>
```

If the cache is suspect, get the cache path from `doctor`, move it aside, and
rebuild. Details: [CLI reference - stale cache](cli-reference.md#stale-local-cache).

Dirty-marker Git hooks (optional, local freshness after Git rewrite):

```sh
node plugins/codestory/hooks/codestory-dirty-hook.cjs install --project <repo> --plugin-data <plugin-data-dir>
```

## Packet/search degraded or blocked

Symptoms: `packet`, `search`, or `context` not allowed; retrieval mode not
`full`.

**Agent:** Call MCP `sidecar_setup repair` when status says so, then reread
`codestory://status`. Do not treat degraded output as proof. See
[Trust and readiness](trust-and-readiness.md#proof-vs-hint).

**You:** Sidecar model download and lifecycle:
[Retrieval sidecars ops](../ops/retrieval-sidecars.md).

CLI check:

```sh
codestory-cli retrieval status --project <repo> --format json
codestory-cli fix --project <repo> --format json
```

Require `retrieval_mode: "full"` before trusting packet/search evidence.
Command table: [CLI reference - readiness and repair](cli-reference.md#readiness-and-repair).

### Apple Silicon acceleration

On macOS arm64, the supported accelerated embedding path is the native Metal
sidecar. A healthy repaired status should report the embedding launch as
`native_spawned`, request provider `metal`, and then prove the observed device
state from sidecar logs or an explicit operator assertion. The request fields
show intent; `embedding_device_state`, `embedding_device_observation_source`,
and `retrieval_mode` decide readiness.

The old failure pattern is `accelerator_request_provider=vulkan`,
`accelerator_request_device=Vulkan0`, Docker/Colima embed launch, and
`accelerator_request_unobserved`. That is a stale or pre-release runtime for
Apple Silicon, not a Colima tuning problem: the Linux Docker sidecar cannot
observe macOS Metal and normally has no usable `/dev/dri`.

**Agent:** Read `codestory://status`, follow `recommended_next_calls`, call MCP
`sidecar_setup repair` when recommended, and reread status. Keep local navigation separate
from packet/search: a ready local graph can answer source-navigation questions
while packet/search remains blocked until `retrieval_mode` is `full`.

**You:** For a maintainer transcript, prewarm the managed Metal binary and then
rerun bootstrap/status:

```sh
node scripts/setup-retrieval-env.mjs --fetch-llama-server --fetch-only
codestory-cli retrieval bootstrap --project <repo> --format json
codestory-cli retrieval status --project <repo> --format json
```

CPU-backed embeddings are degraded and explicit. Use
`CODESTORY_EMBED_ALLOW_CPU=1` or `CODESTORY_EMBED_DEVICE_POLICY=allow_cpu` only
when CPU retrieval is acceptable for that machine; CPU opt-in is not the default
Apple Silicon success path.

## MCP visibility failure

Symptoms: skill or rule loads but no `codestory://status` or `mcp__codestory` tools.

| Host | Check |
| --- | --- |
| Codex | Read `codestory://status` through live MCP resources. Inspect `readiness_broker` and follow `recommended_next_calls`; status reads do not start repair. If resources are visible but `mcp__codestory` tools are hidden, report the host tool-visibility blocker; reload only after plugin install/config changes; see [Codex guide](codex.md#troubleshooting) |
| Cursor | MCP config path to `plugins/codestory/scripts/codestory-mcp.cjs`; reload server |
| Claude Code | MCP configured separately; hooks alone do not expose tools |
| Copilot | MCP not auto-started; configure manually or use CLI |

CLI health does not prove MCP is live in the agent host.

## Runtime drift after update

Symptoms: `repair_setup`, stale `server_executable`, or version mismatch in status.

**You:** Let the plugin provision the current release, or restart the host after
install. Confirm with a fresh `codestory://status` read.

**Local dev:** Set `CODESTORY_CLI` to a built binary; status labels this
`local_dev_override`.

### Codex marketplace refresh vs runtime reload

For Codex, marketplace refresh, package refresh, and runtime reload are separate:

```sh
codex plugin marketplace upgrade TheGreenCedar
codex plugin add codestory@TheGreenCedar
```

The first command only updates Codex's marketplace snapshot. The second refreshes
the installed plugin package when your Codex build supports terminal plugin
management. A running Codex host can still keep the old MCP adapter and managed
CLI alive until you start a fresh host session.

On Windows, older running CodeStory MCP processes can make
`codex plugin add codestory@TheGreenCedar` fail with `Access is denied` while
backing up the plugin cache. Current MCP adapters move their long-lived working
directory out of the plugin cache, but stale hosts from older packages can still
hold files open. Quit stale Codex windows, start a fresh host session, and retry
the `/plugins` refresh or terminal install. After refresh, confirm the active
runtime through `codestory://status`, not only `codex plugin list`.

## Still stuck?

- [Trust and readiness](trust-and-readiness.md) -- when to trust output
- [CLI reference - command by situation](cli-reference.md#command-by-situation) for command-by-situation table
- [Contributor debugging](../contributors/debugging.md) for crate-level investigation
- [Retrieval sidecars ops](../ops/retrieval-sidecars.md) for embedding backend repair
