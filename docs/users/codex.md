# Codex

Use CodeStory in Codex with the plugin MCP path: MCP auto-start, lightweight
repo-state hooks, the grounding skill, and managed CLI bootstrap. Hooks route
the agent back to live MCP; they do not inject substitute grounding context.

## What you get

| You | Agent |
| --- | --- |
| Install the plugin once from `/plugins` | MCP starts via `scripts/codestory-mcp.cjs` |
| Open your repo and start a fresh thread | Passes that repo's absolute path on every live CodeStory MCP tool call |
| Ask repo questions with concrete terms | Calls the matching project-scoped tool, lets CodeStory prepare what it needs, and cites sources |

Codex is the reference host for the managed MCP plugin path. See
[capability matrix](README.md#capability-matrix).

Surfaces and readiness: [Glossary](../glossary.md).

## macOS requirements

CodeStory supports macOS 15 and later on Apple Silicon and Intel Macs. Install
the Xcode Command Line Tools and Node.js before using the managed plugin or
source-worktree setup. Broad search uses embedded storage and a managed native
embedding runtime; it does not require Docker or another user-managed service.

Apple Silicon uses managed Metal acceleration automatically. Intel Macs keep
local navigation available. If broad search cannot use a supported local or
trusted external backend, the requested tool returns `unavailable` and the
agent continues with focused source inspection. Backend, process, and model
details live in the [maintainer operations guide](../ops/retrieval-sidecars.md),
not the normal user flow.

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
   MCP should be registered by the plugin and start with the first repository
   tool call.
3. **First repository question succeeds** — Ask a normal repository question.
   CodeStory prepares the local map and broad search automatically.

## First session

1. Start a **new** Codex thread in the grounded repository (not an old thread
   from before install).
2. Ask a normal repository question. For example:

```text
Use CodeStory to map this repository and show me the files and symbols that own [feature].
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent answers with repository-specific files and
symbols without asking you to configure, approve, or repair CodeStory.

The agent uses [local navigation](../glossary.md#local-navigation-readiness) while
[packet/search](../glossary.md#agent-packetsearch-readiness) prepares. You do
not approve an internal service, tag the plugin, install a CLI, or run repair
commands for normal setup.

## Example prompts

**Orient before editing**

```text
Use CodeStory to orient around [path/to/file] before I change it.
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
| No CodeStory tools are visible | Reload only after plugin install or config changes, then start a fresh thread from the target repo |
| CodeStory resources are visible but `mcp__codestory` tools are hidden | Report the host blocker. Unscoped resources cannot safely select a repository in multi-project mode |
| Status names the wrong repository | Retry the call with the target repository's absolute `project` path. Hook active-state files do not route MCP requests; a schema-3 project/workspace identity mismatch is rejected instead of reusing another repository's readiness. |
| Status shows `runtime_update.state=available` | Current compatible surfaces keep working; reload when convenient if `restart_recommended=true` |
| Status shows `repair_setup` | The active runtime could not start or prove compatibility; follow `recommended_next_calls` |
| Windows terminal refresh says `Access is denied` | Quit stale Codex windows running the old plugin, then refresh from `/plugins` or rerun `codex.cmd plugin add codestory@TheGreenCedar` |
| Broad search is preparing | Retry the same `packet`, `search`, or `context` call after its reported delay. CodeStory prepares the managed runtime automatically. See [Troubleshooting](troubleshooting.md#packetsearch-degraded-or-blocked) |
| A CodeStory call times out | Retry the same tool once. Read status only if it still does not converge; do not kill managed processes. Reload only for host transport/registration failure, plugin replacement, or a runtime update whose status says `restart_recommended=true` |

### Managed platform matrix

| Host | Managed CLI | Managed broad search |
| --- | --- | --- |
| Windows x64 | Yes | Native Vulkan |
| Windows arm64 | Yes | Managed CPU or a trusted external endpoint |
| Linux x64 / arm64 | Yes | Managed native Vulkan or CPU according to the packaged backend |
| macOS arm64 (macOS 15+) | Yes | Managed native Metal |
| macOS x64 (macOS 15+) | Yes | Managed CPU or a trusted external endpoint; never claims Metal |

Linux CUDA, HIP/ROCm, SYCL, and OpenVINO remain contract-only until packaging,
launch, and live GPU evidence exist. A compatible version difference is
advisory; an already-running MCP changes binaries only after an actual runtime
replacement and host reload.

CodeStory verifies process identity, publication freshness, and acceleration
before returning broad-search evidence. Those implementation diagnostics live
in the [retrieval operations guide](../ops/retrieval-sidecars.md); normal users
only need the tool state and retry guidance.

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
