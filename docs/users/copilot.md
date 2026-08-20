# GitHub Copilot

CodeStory has two Copilot adapters: session hooks for Copilot CLI and repository
instructions for editor chat. Neither adapter auto-starts MCP. Hooks and
instructions teach the agent when to use CodeStory; only a connected MCP server
or an explicit CLI command can produce CodeStory evidence.

## Copilot CLI

### Install the hook package

From GitHub:

```bash
copilot plugin install TheGreenCedar/CodeStory:plugins/codestory
```

From a local checkout:

```bash
copilot plugin install plugins/codestory
```

Verify that `copilot plugin list` shows **codestory**, then start a fresh
session. Reinstall after changing plugin files because Copilot caches installed
plugins.

### Connect MCP

Neither Copilot adapter auto-starts MCP. After the plugin is installed, point
the host at the adapter inside that installed plugin directory, not at a
CodeStory git checkout.

The portable plugin `mcp.json` uses `${PLUGIN_ROOT}`:

```json
{
  "mcpServers": {
    "codestory": {
      "command": "node",
      "args": ["./scripts/codestory-mcp.cjs"],
      "cwd": "${PLUGIN_ROOT}",
      "env": {
        "CODESTORY_PLUGIN_DATA": "/absolute/path/to/codestory-plugin-data"
      }
    }
  }
}
```

If the host does not expand `${PLUGIN_ROOT}`, set `cwd` to the directory
`copilot plugin list` reports for **codestory**, and keep `args` as
`./scripts/codestory-mcp.cjs`. Use a persistent per-user data directory outside
the repository you are grounding.

The session hook is useful without MCP because it preserves the grounding
contract, but it cannot query the CodeStory index. Without MCP, the agent should
say CodeStory is unavailable and inspect source normally.

### Verify the install

1. `copilot plugin list` shows **codestory**.
2. A fresh session loads the `sessionStart` hook.
3. The CodeStory MCP server is connected.
4. A repository question returns cited files and symbols.

```text
Where is authorization enforced, who calls it, and which tests cover it?
```

The first MCP request may prepare the repository and retry. Shared behavior:
[First use](README.md#first-use).

### Troubleshooting

| Symptom | Action |
| --- | --- |
| Hook does not load | Confirm the plugin is installed and `node` is on `PATH` |
| No CodeStory tools | Configure MCP; the hook does not start it |
| Installed changes are stale | Reinstall the cached plugin and start a fresh session |
| Tool remains blocked | Follow [shared troubleshooting](troubleshooting.md) |

## Copilot editor

Repository instructions can tell editor chat to prefer CodeStory when MCP is
available. They do not install the CLI, start MCP, or create an index.

### Install

1. Add `.github/copilot-instructions.md` at the target repository root with
   this contract:

   ```markdown
   # CodeStory Grounding

   Use CodeStory proactively for repository questions. Do not wait for the user
   to mention it by name.

   Before making source claims, planning edits, choosing tests, or reviewing
   changes in this repository:

   1. Call the CodeStory tool that matches the task and pass the repository's
      absolute root as `project`.
   2. If it reports `preparing` or `updating`, retry that same tool after its
      reported delay. Do not poll status.
   3. Use `status` or `codestory://status` only to diagnose a failed or
      unexpectedly slow call.
   4. If MCP is missing, inspect source normally and report that CodeStory was
      unavailable for the task.
   ```

2. If the editor supports MCP, configure the host-neutral CodeStory server
   block above.
3. Open the repository root and start a fresh chat.

### Verify the install

Confirm the instruction file is present. If MCP is configured, confirm the
CodeStory server is connected and ask:

```text
What owns src/auth/session.ts, which symbols depend on it, and which tests should I run first?
```

With MCP, a healthy result cites repository-specific evidence. Without MCP,
there is no CodeStory readiness or first-index wait: the editor is following
instructions and using its ordinary source tools.

### Troubleshooting

| Symptom | Action |
| --- | --- |
| Instructions are ignored | Confirm `.github/copilot-instructions.md` is at the repository root and start a fresh chat |
| No CodeStory evidence | Connect MCP; instructions alone cannot query CodeStory |
| Wrong repository scope | Open the intended repository root and pass that project to MCP |

## Differences from Codex

Copilot provides no CodeStory MCP auto-start. Copilot CLI has a session hook;
editor chat has repository instructions only. Both need manual MCP setup for
live CodeStory grounding.
