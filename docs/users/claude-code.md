# Claude Code

Use CodeStory in Claude Code with one session hook and the CodeStory MCP server.
MCP setup is manual unless your Claude Code plugin flow wires it.

## What you get

| You | Agent |
| --- | --- |
| Install the CodeStory Claude plugin | A bounded session hook makes the grounding contract available |
| Open your repo | The hook records the project without running tools or injecting source claims |
| Ask concrete questions | Calls the matching CodeStory tool; managed preparation happens automatically |

The hook fails open: missing Node or MCP does not block the host. Normal use does
not begin with a status check and never asks you to configure CodeStory. If a
first call is still preparing, the agent retries that same call after its delay.

## Install

Plugin manifest: `plugins/codestory/.claude-plugin/plugin.json`  
Hooks: `plugins/codestory/hooks/claude-codex-hooks.json`

### From the marketplace (recommended)

From a terminal, with Claude Code on PATH:

```bash
claude plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
claude plugin install codestory@TheGreenCedar --scope project
```

Or inside a Claude Code session:

```text
/plugin marketplace add TheGreenCedar/AgentPluginMarketplace
/plugin install codestory@TheGreenCedar --scope project
```

### From this repository checkout

When you already have the CodeStory repo open locally:

```bash
claude plugin install ./plugins/codestory --scope project
```

For a one-session test without installing:

```bash
claude --plugin-dir plugins/codestory
```

Re-run `claude plugin install ./plugins/codestory --scope project` after you
change plugin files; Claude Code caches installed plugins.

### MCP (manual)

Configure MCP separately if your Claude Code setup does not inherit the Codex
`.mcp.json` launch path. Point MCP at
`plugins/codestory/scripts/codestory-mcp.cjs` (same shape as
[Cursor MCP config](cursor.md#2-mcp-server-copy-shipped-config)).

Use `CODESTORY_PLUGIN_DATA` in the MCP server env block to give Claude Code a
persistent managed-runtime data directory:

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

Open the repository you want to ground. Managed CLI bootstrap depends on MCP:
the adapter provisions the runtime when the MCP server starts successfully.

## Install verification

Run these two checks before your first real task:

1. **Adapter present** — `claude plugin list` shows **codestory**, or you
   started Claude with `--plugin-dir plugins/codestory`. Confirm the manifest
   at `plugins/codestory/.claude-plugin/plugin.json` and hooks at
   `plugins/codestory/hooks/claude-codex-hooks.json`.
2. **Hooks live** — Start a new session. The session hook should make the
   CodeStory grounding contract available (and fails open if Node is missing).

## First session

Start a new Claude Code session in the repository and ask a real repository
question:

```text
Where is [Feature] implemented, who calls it, and which tests cover it?
```

**Expected wait:** On a large repository, the first index build can take several
minutes. The agent should retry the same tool while CodeStory prepares.

**Success looks like:** The agent answers with repository-specific files and
symbols without asking you to run setup, approve a background service, or poll
readiness.

Hooks inject routing, not grounding evidence. Follow the selected live MCP
surface and its evidence gaps before making source claims.

## Example prompts

**Find ownership**

```text
Where is [Feature] defined and who calls it?
```

**Plan a change**

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Broad question**

```text
How does [subsystem] interact with [other area]? Cite the owning files and symbols.
```

More pairs and anti-patterns: [Prompt patterns](prompt-patterns.md).

## Troubleshooting

| Symptom | What to try |
| --- | --- |
| Hooks silent | Confirm `node` on PATH; check `CLAUDE_PLUGIN_ROOT` resolves to plugin dir |
| No MCP tools | Add MCP server config; see [Cursor MCP section](cursor.md#2-mcp-server-copy-shipped-config) |
| Broad search unavailable | [Troubleshooting - packet/search](troubleshooting.md#packetsearch-degraded-or-blocked) |
| Hook timeout | Session continues; ask the repository question directly |

Shared repair: [Troubleshooting](troubleshooting.md).

## Limitations vs Codex

| Feature | Claude Code | Codex |
| --- | --- | --- |
| MCP auto-start | Manual | Yes |
| Hooks | Session | Session |
| Skill | Partial (host-dependent) | Full `@CodeStory` skill |
| Managed CLI | Yes when MCP sets `CODESTORY_PLUGIN_DATA` | Yes via plugin |

Claude Code matches Codex on hooks when the plugin is installed, but MCP and
managed bootstrap are not automatic unless you configure them.

Compare: [capability matrix](README.md#capability-matrix).
