# Claude Code

## Set up shell lookups

Install `codestory-cli` from the 0.18 [release archives](https://github.com/TheGreenCedar/CodeStory/releases)
for your [platform](README.md#platform-summary). If the executable is outside
your `PATH`, use its full path to run setup.

```sh
codestory-cli setup claude-code
```

Setup puts a marked note in your Claude Code `CLAUDE.md` and installs a shared
lookup launcher in your Claude Code configuration directory. The note gives
Claude Code the full invocation for `cs slice`, `cs refs`, `cs find`, `cs read`,
and `cs edit`. The commands use your existing shell with no `PATH` edits or MCP
connection. Use the launcher path printed by setup if you want to run them
yourself.

Start a fresh Claude Code session in your repository and ask a question about
the code, such as this one.

```text
How does the configuration loader reach the runtime, and which tests exercise that path?
```

Lookups return live source before indexing. They add declarations, 1-hop
callees, and co-change siblings from an existing index when the indexed file
matches the current file. To build a fast core-only lookup index yourself, run
`codestory-cli index --project . --core-only` from the repository.

Setup applies to your Claude Code user by default. To put the note at a
repository's Git root, run `codestory-cli setup claude-code --repo` from inside
it. Remove the user note with `codestory-cli setup claude-code --remove`. To
remove the repository note, run `codestory-cli setup claude-code --repo --remove`
from that repository. Removal preserves your other instructions and keeps the
shared launcher available to other repositories. If you move the CLI
executable, run setup again.

## Install the optional plugin

The Claude Code plugin supplies a session hook and the CodeStory grounding
skill. Connect MCP for repository map and broad search tools if the plugin
setup has not registered the shipped adapter.

From a terminal:

```bash
claude plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
claude plugin install codestory@TheGreenCedar --scope project
```

Or from a Claude Code session:

```text
/plugin marketplace add TheGreenCedar/AgentPluginMarketplace
/plugin install codestory@TheGreenCedar --scope project
```

For local development from this checkout:

```bash
claude plugin install ./plugins/codestory --scope project
```

Use `claude --plugin-dir plugins/codestory` for a one-session test. Claude Code
caches installed plugins, so reinstall after changing plugin files.

### Shell lookups from the plugin

In Claude Code builds that put the enabled plugin's `bin/` on the shell tool's
PATH, use `cs slice QUESTION`, `cs refs NAME`, `cs find TERM`, `cs read PATH::NAME`,
or `cs edit PATH::NAME --stdin`. The installed CodeStory lookups skill also describes
how to run the bundled command by absolute path. Run it from the repository you want to inspect. The command
uses the matching native runtime prepared by the plugin's launcher.

### Configure MCP

If the plugin does not register MCP for you, use the portable adapter from the
installed plugin, not a CodeStory checkout path:

```json
{
  "mcpServers": {
    "codestory": {
      "command": "node",
      "args": ["./scripts/codestory-mcp.cjs"],
      "cwd": "${PLUGIN_ROOT}",
      "env": {
        "CODESTORY_PLUGIN_DATA": "/absolute/path/to/codestory-plugin-data"
      },
      "tool_timeout_sec": 300
    }
  }
}
```

If Claude does not expand `${PLUGIN_ROOT}`, set `cwd` to
`${CLAUDE_PLUGIN_ROOT}` (the installed plugin directory). Use a persistent
per-user plugin-data directory outside the repository. This server block is
host-neutral; the Claude plugin's `.mcp.json` keeps the legacy auto-discovery
shape for Claude and Codex clients that still require it.

### Verify the plugin

1. `claude plugin list` shows **codestory**.
2. A fresh session loads the session hook.
3. The CodeStory MCP server is connected.
4. A normal repository question returns cited files and symbols.

For example:

```text
How does the configuration loader reach the runtime, and which tests exercise that path?
```

The first request may prepare the repository and retry. Hooks fail open when
Node or MCP is missing, but hooks alone are not grounding evidence. If the MCP
connection is unavailable, standalone shell lookups can still read current
source.

Shared first-use behavior: [User guide](README.md#first-use).

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Plugin hook is silent | Confirm `node` is on `PATH` and `CLAUDE_PLUGIN_ROOT` resolves to the plugin directory |
| Claude Code misses shell lookups | Run setup for the intended scope and open a fresh session |
| No CodeStory MCP tools | Add or repair the MCP configuration before using repository map or broad search tools |
| First tool is preparing | Retry the same tool after its returned delay |
| Installed changes do not appear | Reinstall the cached plugin and start a fresh session |

See [shared troubleshooting](troubleshooting.md) for readiness and cache
problems.

## Plugin differences from Codex

Claude Code provides the session hook, but MCP setup is normally manual and
skill exposure depends on the host. Once MCP is connected, repository
preparation and the per-user retrieval server behave the same as in Codex.
