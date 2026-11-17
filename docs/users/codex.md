# Codex

## Set up shell lookups

Install `codestory-cli` from the 0.18 [release archives](https://github.com/TheGreenCedar/CodeStory/releases)
for your [platform](README.md#platform-summary). If the executable is outside
your `PATH`, use its full path to run setup.

```sh
codestory-cli setup codex
```

Setup puts a marked note in your Codex `AGENTS.md` and installs a shared
lookup launcher in your Codex configuration directory. The note gives Codex
the full invocation for `cs slice`, `cs refs`, `cs find`, `cs read`, and
`cs edit`. The commands use your existing shell with no `PATH` edits or MCP
connection. Use the launcher path printed by setup if you want to run them
yourself, or run Codex through `codestory-cli run -- codex` to bound and
deduplicate oversized tool outputs in the loopback proxy.

Start a fresh Codex session in your repository and ask a question about the
code, such as this one.

```text
Where is configuration loaded, which modules consume it, and what tests cover that path?
```

Lookups return live source before indexing. They add declarations, 1-hop
callees, and co-change siblings from an existing index when the indexed file
matches the current file. To build a fast core-only lookup index yourself, run
`codestory-cli index --project . --core-only` from the repository.

Setup applies to your Codex user by default. To put the note at a repository's
Git root, run `codestory-cli setup codex --repo` from inside it. Remove the user
note with `codestory-cli setup codex --remove`. To remove the repository note,
run `codestory-cli setup codex --repo --remove` from that repository. Removal
preserves your other instructions and keeps the shared launcher available to
other repositories. If you move the CLI executable, run setup again.

## Install the optional plugin

The plugin adds MCP tools for the repository map and broad search, a grounding
skill, and lightweight hooks. It uses a version-matched native CLI and needs
Node.js for its adapter. See [supported platforms](README.md#platform-summary)
for accelerator requirements.

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

### Shell lookups from the plugin

The installed CodeStory skill points to the plugin's bundled `bin/cs`.
From a repository, run it directly or use `node` with its absolute installed
path. Try `cs slice QUESTION` to pack declarations and 1-hop callees,
`cs refs NAME` for definitions and mentions, `cs find TERM` for literal search,
`cs read PATH::NAME` for numbered source, or `cs edit PATH::NAME --stdin` for
syntax-checked edits. The command discovers the Git root from the current
directory and uses the same cache as the native CLI.

Once the native runtime is installed, a missing index still gives live text.
When the host allows cache writes, the launcher requests a background
`index --core-only` refresh after a missing-index reply or a hit in a changed
file. Lookups return before the background work finishes.

### Verify the plugin

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

## Update the plugin

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
| Codex misses shell lookups | Run setup for the intended scope and open a fresh session |
| No CodeStory MCP tools | Confirm the plugin is installed, then start a fresh host session |
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

The Codex plugin manages its MCP connection. Other hosts may need manual MCP
configuration. See the [host comparison](README.md#pick-your-host).

Plugin package details: [CodeStory plugin](../../plugins/codestory/README.md).
