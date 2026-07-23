# Cursor

Cursor uses a project rule plus a manually configured CodeStory MCP server. The
rule teaches the agent when to use CodeStory; MCP supplies the actual repository
evidence.

## Install

### 1. Add the project rule

Copy the shipped rule into the target repository:

```text
plugins/codestory/.cursor/rules/codestory.mdc -> .cursor/rules/codestory.mdc
```

### 2. Configure MCP

Copy `plugins/codestory/.cursor/mcp.json` to `.cursor/mcp.json`, or add the same
server in Cursor user settings:

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

Use an absolute adapter path unless the plugin checkout is inside the Cursor
workspace. `CODESTORY_PLUGIN_DATA` must be a persistent per-user directory
outside the repository. Set `CODESTORY_CLI` only when testing a local build.

### 3. Reload

Reload the MCP server or restart Cursor after changing its configuration, then
open the repository root as the workspace folder.

## Verify the install

Confirm the CodeStory MCP server is connected, then ask:

```text
Where is request validation implemented, who calls it, and which tests cover it?
```

The first request may prepare the repository and retry. A healthy answer cites
real files and symbols. The rule alone cannot provide CodeStory evidence; if MCP
is disconnected, the agent must fall back to ordinary source inspection and say
so.

Shared first-use behavior: [User guide](README.md#first-use).

## Troubleshooting

| Symptom | Action |
| --- | --- |
| MCP fails to start | Check that `node` is on `PATH` and use an absolute adapter path |
| Tools are missing | Reload MCP and confirm the workspace root is the repository being queried |
| Rule is present but no CodeStory evidence appears | The rule is instructions only; connect MCP |
| Runtime is stale after an update | Replace the plugin checkout or package, then reload MCP |
| A tool remains preparing | Retry that same tool after its returned delay |

See [shared troubleshooting](troubleshooting.md) for readiness and cache
problems.

## Differences from Codex

Cursor does not auto-start CodeStory MCP or install lifecycle hooks. Once MCP is
connected, it uses the same project-scoped runtime and automatic repository
preparation as Codex.
