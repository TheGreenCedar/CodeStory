# Claude Code

The Claude Code plugin supplies a session hook and the CodeStory grounding
contract. Configure MCP separately unless your plugin setup already wires the
shipped adapter. Hooks record repository state and teach routing; MCP supplies
the repository evidence.

## Install the plugin

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

## Configure MCP

If the plugin does not register MCP for you, point a server at the adapter:

```json
{
  "mcpServers": {
    "codestory": {
      "command": "node",
      "args": ["/absolute/path/to/plugins/codestory/scripts/codestory-mcp.cjs"],
      "env": {
        "CODESTORY_PLUGIN_DATA": "/absolute/path/to/codestory-plugin-data"
      },
      "tool_timeout_sec": 300
    }
  }
}
```

Use a persistent per-user plugin-data directory outside the repository. The
same adapter shape is explained in the [Cursor guide](cursor.md#2-configure-mcp).

## Verify the install

1. `claude plugin list` shows **codestory**.
2. A fresh session loads the session hook.
3. The CodeStory MCP server is connected.
4. A normal repository question returns cited files and symbols.

For example:

```text
How does the configuration loader reach the runtime, and which tests exercise that path?
```

The first request may prepare the repository and retry. Hooks fail open when
Node or MCP is missing, but hooks alone are not grounding evidence. Without MCP,
the agent must use ordinary source inspection and report that CodeStory was
unavailable.

Shared first-use behavior: [User guide](README.md#first-use).

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Plugin hook is silent | Confirm `node` is on `PATH` and `CLAUDE_PLUGIN_ROOT` resolves to the plugin directory |
| No CodeStory tools | Add or repair the MCP configuration; the hook does not start MCP by itself |
| First tool is preparing | Retry the same tool after its returned delay |
| Installed changes do not appear | Reinstall the cached plugin and start a fresh session |

See [shared troubleshooting](troubleshooting.md) for readiness and cache
problems.

## Differences from Codex

Claude Code provides the session hook, but MCP setup is normally manual and
skill exposure depends on the host. Once MCP is connected, repository
preparation and the in-process retrieval engine behave the same as in Codex.
