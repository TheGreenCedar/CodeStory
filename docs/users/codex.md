# Codex

Use CodeStory in Codex with the plugin MCP path: MCP auto-start, lightweight
repo-state hooks, the grounding skill, and managed CLI bootstrap. Hooks route
the agent back to live MCP; they do not inject substitute grounding context.

## What you get

| You | Agent |
| --- | --- |
| Install the plugin once from `/plugins` | MCP starts via `scripts/codestory-mcp.cjs` |
| Open your repo and start a fresh thread | Passes that repo's absolute path on every live CodeStory MCP tool call |
| Ask repo questions with concrete terms | Calls project-scoped `status`, uses allowed surfaces, cites sources |

Codex is the reference host for the managed MCP plugin path. See
[capability matrix](README.md#capability-matrix).

Surfaces and readiness: [Glossary](../glossary.md).

## macOS requirements

CodeStory supports macOS 15 and later on Apple Silicon and Intel Macs. Install
the Xcode Command Line Tools and Node.js before using the managed plugin or
source-worktree setup. Docker Desktop or another compatible Docker engine is
required for managed Qdrant; make sure it is running before repairing
packet/search retrieval.

- Apple Silicon uses the checksum-pinned native `llama-server` and managed
  Metal path. A healthy agent lane reports `launch_mode=native_spawned`, live
  native-process identity, and verified GPU proof.
- Intel Macs support the native CLI, plugin provisioning, local index, and
  grounding. Packet/search requires either explicit CPU allowance or a trusted
  external embedding endpoint under an explicit operator policy. Intel never
  reports or implies managed Metal support.

The plugin downloads the matching signed and notarized CLI. Contributors who
build the CLI locally also need the Rust toolchain; normal plugin users do not.

## Install

1. Open Codex in the repository you want to ground.
2. In the Codex chat input, type `/plugins` to open the plugin manager.
3. Install **TheGreenCedar -> codestory**.

**UI path:** `/plugins` opens the plugin picker in the current Codex session.
Search or browse for **codestory** under the **TheGreenCedar** marketplace
entry, then choose **Install**. Refresh or uninstall from the same `/plugins`
screen.

Optional Windows terminal command, if your Codex build supports terminal
marketplace management:

```powershell
codex.cmd plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
```

Then install from `/plugins` as above. Marketplace catalog repo:
`TheGreenCedar/AgentPluginMarketplace`; plugin source lives in this repository
at `plugins/codestory`.

## Refresh or update

There are three separate steps:

1. On Windows, `codex.cmd plugin marketplace upgrade TheGreenCedar` refreshes
   the marketplace snapshot only. In Unix shells, use `codex` instead of
   `codex.cmd`.
2. `/plugins` refresh or, on Windows,
   `codex.cmd plugin add codestory@TheGreenCedar` updates the installed plugin
   package.
3. A fresh Codex host session starts the new MCP adapter. When the matching
   managed CLI is missing, diagnostic status is available immediately while
   the existing single-flight installer provisions it in the background; the
   next request after verification is handed to the real runtime without a
   host restart. The adapter itself is projectless; tool requests
   are routed by their required `project` argument, so other Codex tasks can use
   other repositories at the same time.

If terminal refresh fails on Windows with `Access is denied` while backing up
the plugin cache, quit stale Codex windows that may still be running the old
CodeStory MCP process, then retry the `/plugins` refresh or terminal install.
The marketplace may already show the new version before the running host has
reloaded the package; prove the active runtime with a fresh
project-scoped `status` call.

## Install verification

Run these three checks before your first real task:

1. **Adapter present** — Open `/plugins` and confirm **TheGreenCedar -> codestory**
   is listed as installed.
2. **MCP live** — Start a **new** Codex thread in the grounded repo. CodeStory
   MCP should be registered by the plugin and start when Codex reads its status
   or tools.
3. **First status read succeeds** — Use a normal repository question or the
   readiness probe in [First session](#first-session). The agent should answer
   in plain English whether your repo map is ready and whether broad search is
   available.

## First session

1. Start a **new** Codex thread in the grounded repository (not an old thread
   from before install).
2. Ask a normal repository question. For an explicit readiness probe, ask:

```text
Call CodeStory status with this repository's absolute path, ground the same project if allowed, and tell me which surfaces are ready before I edit.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready, says whether
broad search is available, and does not report a missing CLI or broken plugin.

The agent reports `allowed_surfaces`, [local navigation](../glossary.md#local-navigation-readiness) vs [packet/search](../glossary.md#agent-packetsearch-readiness) readiness, and any repair steps. You do not tag the plugin, install a CLI, or run the CLI yourself for normal setup.

## Example prompts

**Readiness before editing**

```text
Call CodeStory status for this repository and check allowed_surfaces before I change [path/to/file].
```

**Find ownership**

```text
Where is [Feature] defined and who calls it?
```

**Plan a change**

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Subsystem overview**

```text
How does [subsystem] work? Cite concrete files and note any coverage gaps.
```

## Prompt patterns

**Bad** — tree walk before grounding:

```text
Grep the repo for [SYMBOL] and read every matching file.
```

**Good** — repo question with concrete terms:

```text
Where is [SYMBOL] defined, who calls it, and which tests cover that path?
```

More pairs, anti-patterns, and language-flavored examples:
[Prompt patterns](prompt-patterns.md).

## Troubleshooting

| Symptom | What to try |
| --- | --- |
| No CodeStory `status` tool is visible | Reload only after plugin install or config changes, then start a fresh thread from the target repo |
| CodeStory resources are visible but `mcp__codestory` tools are hidden | Report the host blocker. Unscoped resources cannot safely select a repository in multi-project mode |
| Status shows `runtime_update.state=available` | Current compatible surfaces keep working; reload when convenient if `restart_recommended=true` |
| Status shows `repair_setup` | The active runtime could not start or prove compatibility; follow `recommended_next_calls` |
| Windows terminal refresh says `Access is denied` | Quit stale Codex windows running the old plugin, then refresh from `/plugins` or rerun `codex.cmd plugin add codestory@TheGreenCedar` |
| Packet/search blocked | Follow `recommended_next_calls`. Status reads are observational. A grounding/project tool activation starts or attaches local refresh and automatically enqueues agent repair only when the installed sidecar policy is already enabled; otherwise explicit MCP confirmation/repair remains required. See [Troubleshooting](troubleshooting.md#packetsearch-degraded-or-blocked) |
| Status/grounding call times out | If status remains visible, reread it and follow its bounded blocker/next call; do not kill or restart managed MCP for index or sidecar readiness. Reload only for host transport/registration failure, plugin/config replacement, or a runtime update whose status says `restart_recommended=true` |

### Managed platform matrix

| Host | Managed CLI | Managed accelerated embeddings |
| --- | --- | --- |
| Windows x64 | Yes | Native Vulkan |
| Windows arm64 | Yes | No managed accelerated sidecar cell; use a proven external endpoint or explicit degraded CPU opt-in |
| Linux x64 / arm64 | Yes | Docker Vulkan with a verified `/dev/dri` render node |
| macOS arm64 (macOS 15+) | Yes | Managed native Metal |
| macOS x64 (macOS 15+) | Yes | No managed Metal cell; use a trusted external endpoint under explicit policy or explicit degraded CPU opt-in |

Linux CUDA, HIP/ROCm, SYCL, and OpenVINO remain contract-only until packaging,
launch, and live GPU evidence exist. A compatible version difference is
advisory; an already-running MCP changes binaries only after an actual runtime
replacement and host reload.

### Native embedding busy decision tree

1. `native_embedding_runtime.status=stale` → call `sidecar_setup repair` (or follow `recommended_next_calls`) so the broker can reclaim.
2. `status=busy` and owner is same-project / reusable → continue; do not treat as a hard block.
3. `status=busy` and owner is foreign or unverifiable → wait and reread status; do not start a competing repair.
4. `status=available` → repair is free to proceed when policy allows.

### Readiness broker fields

Broker schema 3 scopes processes, operations, locks, snapshots, and verified
GPU runtime evidence with the lossless workspace identity. On case-sensitive
filesystems, roots that differ only by case remain separate; Windows aliases
for the same existing path remain one workspace. Schema 1/2 state is consulted
only when its recorded root and repository provenance map unambiguously to the
requested workspace. Otherwise CodeStory leaves that state isolated and
publishes a fresh schema-3 snapshot.

| Field | Healthy value | Action value |
| --- | --- | --- |
| `readiness_broker.reconciliation.status` | `clean` or `observed` | `active_repair` means wait and reread status; `stale_state_cleaned` means retry repair once |
| `readiness_broker.resources.native_embedding_runtime.status` | `available` | `busy` blocks repair only when the owner is foreign or unverifiable; same-project reusable owners should continue through `recommended_next_calls`; `stale` means retry repair so the broker can reclaim it |
| `readiness_broker.gpu_proof.proof_status` | `verified` when accelerator is required and live embed smoke succeeded | `gpu_unverified` means repair should stop before a long semantic rebuild; inspect `embed_smoke_ok` / `embed_smoke_ms` |
| `readiness_broker.persistence_status` | `persisted` | `failed` means inspect `persistence_error`; status may be live but not durable across processes |
| `sidecar_setup.last_worker_result` | `outcome=succeeded` for the matching repair `attempt_id` | `failed` or `abandoned` means branch on `terminal_envelope.error.code` when present. Legacy persisted results can omit the envelope; use `wait_error` and bounded tails as compatibility diagnostics. It is terminal evidence, not an active lock |
| `recommended_next_calls` | Start with `agent-guide`, then use allowed tools | For packet/search blockers, follow the listed `sidecar_setup repair` and status read sequence; if tools are hidden, stop and report host visibility |

Shared repair lanes: [Troubleshooting](troubleshooting.md).

`retrieval_mode=full` describes the persisted retrieval manifest; it does not
override live infrastructure. When acceleration is required, packet/search is
allowed only while the selected endpoint is reachable, the native executable,
arguments, and process-start identity still match, and GPU proof is `verified`.
If the endpoint dies, status changes the agent lane to `repair_retrieval` and
keeps local navigation available with actionable repair guidance.

Optional Git dirty-marker hooks (local graph freshness after checkout/merge):

```bash
node plugins/codestory/hooks/codestory-dirty-hook.cjs install --project <repo> --plugin-data <plugin-data-dir>
```

## Limitations

None relative to other hosts -- Codex ships the managed MCP adapter and
repo-state hook path. Other hosts may lack auto MCP start, hooks, or managed CLI
bootstrap; compare
[capability matrix](README.md#capability-matrix).

Plugin package details: [Plugin README](../../plugins/codestory/README.md).
