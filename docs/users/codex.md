# Codex

Use CodeStory in Codex with the plugin MCP path: MCP auto-start, lightweight
repo-state hooks, the grounding skill, and managed CLI bootstrap. Hooks route
the agent back to live MCP; they do not inject substitute grounding context.

## What you get

| You | Agent |
| --- | --- |
| Install the plugin once from `/plugins` | MCP starts via `scripts/codestory-mcp.cjs` |
| Open your repo and start a fresh thread | Records the repo target, then reads live CodeStory MCP resources before source reads |
| Ask repo questions with concrete terms | Reads `codestory://status`, uses allowed surfaces, cites sources |

Codex is the reference host for the managed MCP plugin path. See
[capability matrix](README.md#capability-matrix).

Surfaces and readiness: [Glossary](../glossary.md).

## Install

1. Open Codex in the repository you want to ground.
2. In the Codex chat input, type `/plugins` to open the plugin manager.
3. Install **TheGreenCedar -> codestory**.

**UI path:** `/plugins` opens the plugin picker in the current Codex session.
Search or browse for **codestory** under the **TheGreenCedar** marketplace
entry, then choose **Install**. Refresh or uninstall from the same `/plugins`
screen.

Optional: if your Codex build supports terminal marketplace management:

```bash
codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
```

Then install from `/plugins` as above. Marketplace catalog repo:
`TheGreenCedar/AgentPluginMarketplace`; plugin source lives in this repository
at `plugins/codestory`.

## Refresh or update

There are three separate steps:

1. `codex plugin marketplace upgrade TheGreenCedar` refreshes the marketplace
   snapshot only.
2. `/plugins` refresh or `codex plugin add codestory@TheGreenCedar` updates
   the installed plugin package.
3. A fresh Codex host session starts the new MCP adapter and lets it provision
   the matching managed CLI.

If terminal refresh fails on Windows with `Access is denied` while backing up
the plugin cache, quit stale Codex windows that may still be running the old
CodeStory MCP process, then retry the `/plugins` refresh or terminal install.
The marketplace may already show the new version before the running host has
reloaded the package; prove the active runtime with a fresh
`codestory://status` read.

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
Read codestory://status, ground this checkout if allowed, and tell me which CodeStory surfaces are ready before I edit.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready, says whether
broad search is available, and does not report a missing CLI or broken plugin.

The agent reports `allowed_surfaces`, [local navigation](../glossary.md#local-navigation-readiness) vs [packet/search](../glossary.md#agent-packetsearch-readiness) readiness, and any repair steps. You do not tag the plugin, install a CLI, or run the CLI yourself for normal setup.

## Example prompts

**Readiness before editing**

```text
Read codestory://status and check allowed_surfaces before I change [path/to/file].
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
| No `codestory://status` resource is visible | Reload only after plugin install or config changes, then start a fresh thread from the target repo |
| `codestory://status` is visible but `mcp__codestory` tools are hidden | Use the live resource path. If `status_resource_auto_repair` starts, reread status until repair finishes; otherwise report that tool actions are blocked by host visibility |
| Status shows `repair_setup` | Let the agent follow `recommended_next_calls` from status; restart host if binary was updated |
| Terminal refresh says `Access is denied` | Quit stale Codex windows running the old plugin, then refresh from `/plugins` or rerun `codex plugin add codestory@TheGreenCedar` |
| Packet/search blocked | Reread `codestory://status` after any `status_resource_auto_repair`; if tools are visible, follow `recommended_next_calls`; see [Troubleshooting](troubleshooting.md#packetsearch-degraded-or-blocked) |
| Status/grounding read times out | Restart stale CodeStory MCP processes, then read `codestory://status` in a fresh thread |

Shared repair lanes: [Troubleshooting](troubleshooting.md).

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
