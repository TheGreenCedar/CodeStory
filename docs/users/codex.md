# Codex

Codex is CodeStory's reference host. The plugin supplies the MCP adapter,
grounding skill, lightweight repository hooks, and a version-matched native
CLI. Repository questions route through one multi-project MCP process; every
tool call still names its repository explicitly.

## Requirements

- One of the [supported platform and accelerator combinations](README.md#platform-summary);
- Node.js available to the plugin adapter; and
- a fresh Codex session after installing or replacing the plugin package.

The packaged CLI contains its model and embedding engine, so normal use needs
no Docker, external embedding endpoint, model download, or Xcode toolchain.
Building CodeStory from source has separate [contributor
prerequisites](../contributors/getting-started.md#prerequisites).

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

## Source context after ordinary searches

Source context is optional. When the executor explicitly reports a
non-interactive `/bin/zsh` login-shell request, the Codex plugin can append
complete Python and Rust declarations, plus Go functions and methods, around
numbered `rg` matches after the original command finishes. Existing pipelines and redirections receive the original
search bytes. This uses the already installed matching CLI; the hook does not
provision a runtime, build an index, or call a model.

If the host omits shell or login metadata, the hook leaves input unchanged.
An operator may set `CODESTORY_SOURCE_CONTEXT_ZSH_LOGIN=1` in the hook's startup
environment only when the executor has fixed `/bin/zsh` and login-enabled
defaults with no per-call shell/login overrides. This declaration does not
configure the executor. Leave it unset when that contract is unknown;
explicit conflicting fields always retain the ordinary path.

Context is bounded to 12 KiB of literal source and 2 KiB of framing. It describes
rechecked current files, with gaps and limits, rather than an atomic repository
snapshot or an exhaustive search. Unsupported commands, changed source,
unsupported file languages, unavailable CLI, and exhausted bounds retain the raw output.
Rust context covers functions, trait signatures, and enclosing `impl`, trait and
module items with attached outer attributes or docs. It does not expand macros
or establish type or runtime behavior. Go context retains named functions and
methods with immediately attached comments. Function literals inside a named
owner stay within that owner; package declarations, interfaces and top-level
initializers do not become callable context. A single Rust or Go file does not
need filename-prefixed search output.
Other shells, interactive/TTY requests and other hosts keep their normal path.

### Post-tool source hints

For hosts that support a trusted PostToolUse MCP hook, `source_context_hook`
can instead interpret complete `path:line:text` rows from finished ordinary-tool
output. It uses an explicit repository and local address base, re-reads the
source, and returns bounded source data with hashes and physical ranges. It
does not rewrite or rerun the original command, prepare an index, or call a model.
Original-command provenance, relevance and search completeness remain unknown;
the returned repository text is untrusted data, not instructions.

This optional endpoint uses the existing CodeStory MCP connection. Register and
trust its exact definition through the host's normal native hook settings;
the endpoint does not enable hooks for a portable plugin that the host excludes.
For Codex, add this definition to your user `hooks.json`, then review it with
`/hooks`. Set `server` to the name of your enabled CodeStory connection shown
by `/mcp`.

```json
{
  "hooks": {
    "PostToolUse": [{
      "matcher": "^Bash$",
      "hooks": [{
        "type": "mcp_tool",
        "server": "codestory",
        "tool": "source_context_hook",
        "input": {
          "project": "${cwd}",
          "cwd": "${cwd}",
          "output": "${tool_response}"
        },
        "timeout": 2
      }]
    }]
  }
}
```

Unsupported rows, unavailable source and exhausted bounds add no context and
leave the ordinary operation unchanged. The same source and display limits
above apply.

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
converge. Maintainer backend evidence is in the
[CLI reference](cli-reference.md), not a normal session.

## Host boundary

Codex supplies the complete managed path. Cursor, Claude Code, and Copilot may
need manual MCP configuration and do not all provide the same hook or skill
coverage. See the [host comparison](README.md#pick-your-host).

Plugin package details: [CodeStory plugin](../../plugins/codestory/README.md).
