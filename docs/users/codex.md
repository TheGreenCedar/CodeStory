# Codex

Use CodeStory in Codex with the full plugin path: MCP auto-start, lifecycle
hooks, the grounding skill, and managed CLI bootstrap.

## What you get

| You | Agent |
| --- | --- |
| Install the plugin once from `/plugins` | MCP starts via `scripts/codestory-mcp.cjs` |
| Open your repo and start a fresh thread | Hooks ground on session start; request-aware packets on prompts |
| Ask repo questions with concrete terms | Reads `codestory://status`, uses allowed surfaces, cites sources |

Codex is the reference host: MCP, hooks, skill, and managed CLI bootstrap all
ship together. See [capability matrix](README.md#capability-matrix).

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
2. **MCP and hooks live** — Start a **new** Codex thread in the grounded repo.
   CodeStory MCP should start automatically and session hooks should run without
   blocking the host.
3. **First status read succeeds** — Use the readiness prompt in [First
   session](#first-session). The agent should answer in plain English whether
   your repo map is ready and whether broad search is available.

## First session

1. Start a **new** Codex thread in the grounded repository (not an old thread
   from before install).
2. Ask:

```text
@CodeStory read codestory://status, ground this checkout if allowed, and tell me which CodeStory surfaces are ready before I edit.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready, says whether
broad search is available, and does not report a missing CLI or broken plugin.

The agent reports `allowed_surfaces`, [local navigation](../glossary.md#local-navigation-readiness) vs [packet/search](../glossary.md#agent-packetsearch-readiness) readiness, and any repair steps. You do not install or run the CLI yourself for normal setup.

## Example prompts

**Readiness before editing**

```text
@CodeStory read codestory://status and check allowed_surfaces before I change [path/to/file].
```

**Find ownership**

```text
@CodeStory Where is [Feature] defined and who calls it?
```

**Plan a change**

```text
@CodeStory I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Subsystem overview**

```text
@CodeStory How does [subsystem] work? Cite concrete files and note any coverage gaps.
```

## Prompt patterns

**Bad** — tree walk before grounding:

```text
@CodeStory Grep the repo for [SYMBOL] and read every matching file.
```

**Good** — repo question with concrete terms:

```text
@CodeStory Where is [SYMBOL] defined, who calls it, and which tests cover that path?
```

More pairs, anti-patterns, and language-flavored examples:
[Prompt patterns](prompt-patterns.md).

## Troubleshooting

| Symptom | What to try |
| --- | --- |
| `@CodeStory` loads but no MCP tools | Start a fresh Codex host session after install; confirm plugin shows in `/plugins` |
| Status shows `repair_setup` | Let the agent follow `recommended_next_calls` from status; restart host if binary was updated |
| Terminal refresh says `Access is denied` | Quit stale Codex windows running the old plugin, then refresh from `/plugins` or rerun `codex plugin add codestory@TheGreenCedar` |
| Packet/search blocked | Agent can call `sidecar_setup`; see [Troubleshooting](troubleshooting.md#packetsearch-degraded-or-blocked) |
| Hooks time out | Hooks fail open; ask the explicit status prompt above |

Shared repair lanes: [Troubleshooting](troubleshooting.md).

Optional Git dirty-marker hooks (local graph freshness after checkout/merge):

```bash
node plugins/codestory/hooks/codestory-dirty-hook.cjs install --project <repo> --plugin-data <plugin-data-dir>
```

## Limitations

None relative to other hosts -- Codex ships the full adapter set. Other hosts
may lack auto MCP start, full hooks, or managed CLI bootstrap; compare
[capability matrix](README.md#capability-matrix).

Plugin package details: [Plugin README](../../plugins/codestory/README.md).
