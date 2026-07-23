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

Configure an MCP server that runs
`plugins/codestory/scripts/codestory-mcp.cjs`. Use the same server block as the
[Cursor guide](cursor.md#2-configure-mcp), with a persistent
`CODESTORY_PLUGIN_DATA` directory.

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

1. Copy `.github/copilot-instructions.md` from this repository into the target
   repository.
2. If the editor supports MCP, configure the shipped CodeStory adapter as in
   the [Cursor guide](cursor.md#2-configure-mcp).
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
