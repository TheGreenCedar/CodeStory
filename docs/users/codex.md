# Codex

Codex is CodeStory's reference host. The plugin supplies the MCP adapter,
grounding skill, lightweight repository hooks, and a version-matched native
CLI. Repository questions route through one multi-project MCP process; every
tool call still names its repository explicitly.

## Requirements

- macOS 15 or later, Windows, or Linux on a packaged architecture;
- Node.js available to the plugin adapter; and
- a fresh Codex session after installing or replacing the plugin package.

Apple Silicon broad search uses Metal. Intel Macs keep the local repository map
available, but do not claim Metal; explicit CPU operation is reserved for CI or
maintainer diagnostics. The packaged CLI contains its model and embedding
engine, so normal use needs no Docker, external embedding endpoint, model
download, or Xcode toolchain. Building CodeStory from source has separate
[contributor prerequisites](../contributors/getting-started.md#prerequisites).

## Install

1. Open Codex in the repository you want to use.
2. Type `/plugins` in the chat input.
3. Install **TheGreenCedar -> codestory**.
4. Start a fresh Codex session in that repository.

On Windows, Codex builds that expose terminal plugin management can add the
marketplace with:

```powershell
codex.cmd plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
```

The `/plugins` UI remains the normal install and refresh path.

## Verify the install

Ask a normal code question in the fresh session:

```text
Where is configuration loaded, which modules consume it, and what tests cover that path?
```

The first request may report that CodeStory is preparing and retry the same
tool. A healthy result names repository-specific files and symbols with source
citations. You should not be asked to configure retrieval infrastructure or run
a setup command.

The shared first-use timeline, platform boundaries, and multi-repository model
are in the [user guide](README.md#first-use).

## Update

Marketplace refresh, package refresh, and host reload are separate:

1. Refresh **TheGreenCedar** in `/plugins`.
2. Refresh or reinstall **codestory** from the same screen.
3. Start a fresh Codex host session so it launches the updated adapter.

On Windows, the equivalent terminal commands are available in some builds:

```powershell
codex.cmd plugin marketplace upgrade TheGreenCedar
codex.cmd plugin add codestory@TheGreenCedar
```

The first command refreshes the catalog only. The second replaces the installed
package. A running host can continue using the previous adapter until restart.
The adapter can provision its matching signed CLI in the background and hand a
later request to that CLI without another restart.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| No CodeStory tools | Confirm the plugin is installed, then start a fresh host session |
| Resources exist but CodeStory tools are hidden | Report the Codex tool-visibility problem; an unscoped resource cannot safely choose a repository |
| First request is preparing | Let the agent wait for the returned delay and retry the same tool |
| Wrong repository appears | Retry with the target repository's absolute `project` path; hooks do not route requests |
| `runtime_update.state=available` | Keep working if surfaces remain allowed; restart when `restart_recommended=true` |
| `repair_setup` | The managed CLI failed to launch or prove compatibility; follow the returned next call |
| Windows update says `Access is denied` | Close stale Codex windows using the old plugin, replace the package, and start a fresh host |

Use [shared troubleshooting](troubleshooting.md) when a tool still does not
converge. Backend, model, and adapter evidence belongs in the
[retrieval operations guide](../ops/retrieval-engine.md), not normal sessions.

## Host boundary

Codex supplies the complete managed path. Cursor, Claude Code, and Copilot may
need manual MCP configuration and do not all provide the same hook or skill
coverage. See the [host comparison](README.md#pick-your-host).

Plugin package details: [CodeStory plugin](../../plugins/codestory/README.md).
