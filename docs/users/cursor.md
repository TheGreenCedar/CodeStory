# Cursor

Use CodeStory in Cursor with an always-on project rule and a manually configured
MCP server pointing at the plugin adapter.

## What you get

| You | Agent |
| --- | --- |
| Copy the project rule and add MCP config | Rule tells the agent to read `codestory://status` first |
| Open your repo in Cursor | Uses MCP tools when the server is connected |
| Ask repo questions | Local graph and packet/search when [allowed](../glossary.md#allowed-surfaces) |

Cursor provides the rule only; you configure MCP yourself. When MCP is
connected, `scripts/codestory-mcp.cjs` can bootstrap a managed CLI like the
Codex plugin.

## Install

### 1. Project rule

Ensure the repository includes the CodeStory rule (from the plugin package):

```text
plugins/codestory/.cursor/rules/codestory.mdc
```

For your own projects, copy that file to `.cursor/rules/codestory.mdc` at the
project root, or symlink it from the plugin checkout.

### 2. MCP server (copy shipped config)

Copy the shipped MCP config into your project:

```text
plugins/codestory/.cursor/mcp.json  →  .cursor/mcp.json
```

The shipped file looks like this:

```json
{
  "mcpServers": {
    "codestory": {
      "command": "node",
      "args": ["./plugins/codestory/scripts/codestory-mcp.cjs"],
      "tool_timeout_sec": 300
    }
  }
}
```

**Path adjustment:** The `./plugins/codestory/...` path assumes the plugin
checkout lives inside your workspace root. If the plugin is elsewhere, change
`args` to an absolute path to `codestory-mcp.cjs`. On Windows, ensure `node` is
on PATH.

Set `CODESTORY_CLI` only for local development overrides. The adapter
provisions a managed CLI when possible.

Alternatively, add the same server block in Cursor user settings instead of a
project `.cursor/mcp.json`.

### 3. Reload

Restart Cursor or reload the MCP server after config changes. Open the
repository root as the workspace folder.

## Install verification

Run these three checks before your first real task:

1. **Adapter present** — Confirm `.cursor/rules/codestory.mdc` (or the plugin
   copy at `plugins/codestory/.cursor/rules/codestory.mdc`) exists and
   `.cursor/mcp.json` points at `codestory-mcp.cjs`.
2. **MCP live** — In Cursor, the CodeStory MCP server shows as connected after
   reload.
3. **First status read succeeds** — Use the readiness prompt in [First
   session](#first-session). The agent should answer in plain English whether
   your repo map is ready and whether broad search is available.

## First session

1. Confirm the CodeStory MCP server shows connected in Cursor.
2. Start a new agent chat in the repository.
3. Ask:

```text
Read codestory://status, ground this checkout if allowed, and tell me which CodeStory surfaces are ready before I edit.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready, says whether
broad search is available, and does not report a missing CLI or broken MCP
connection.

Without MCP, the rule points the agent to repair fallbacks -- see
[Troubleshooting](troubleshooting.md).

## Example prompts

**Readiness**

```text
Check CodeStory status and allowed_surfaces before I change [path/to/file].
```

**Find ownership**

```text
Where is [Feature] defined and who calls it? Use CodeStory and cite files.
```

**Plan a change**

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Subsystem**

```text
How does [subsystem] work? Start from CodeStory and cite concrete paths.
```

More pairs and anti-patterns: [Prompt patterns](prompt-patterns.md).

## Troubleshooting

| Symptom | What to try |
| --- | --- |
| MCP server fails to start | Verify `node` and the path to `codestory-mcp.cjs`; use absolute path |
| Tools missing in chat | Reload MCP; confirm workspace root contains the repo to ground |
| Rule present but no grounding | MCP not connected -- configure server per Install above |
| Stale runtime after update | Reload MCP; read fresh `codestory://status` |

Shared steps: [Troubleshooting](troubleshooting.md).

## Limitations vs Codex

| Feature | Cursor | Codex |
| --- | --- | --- |
| MCP auto-start | Manual config | Yes |
| Lifecycle hooks | No (rule only) | Session start + prompt hooks |
| Grounding skill | Via rule text | Full `@CodeStory` skill |
| Managed CLI bootstrap | When MCP adapter runs | Always via plugin |

Request-aware packet injection on every prompt requires hook-capable hosts
(Codex, Claude Code). Cursor relies on the rule and your prompts.

Compare hosts: [capability matrix](README.md#capability-matrix).
