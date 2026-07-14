# Claude Code

Use CodeStory in Claude Code with lifecycle hooks for session and prompt
grounding. MCP setup is manual unless your Claude Code plugin flow wires it.

## What you get

| You | Agent |
| --- | --- |
| Install the CodeStory Claude plugin | Hooks run `codestory-activate.cjs` on session start and prompts |
| Open your repo | Receives a compact task router; the hook does not run MCP or inject source claims |
| Ask concrete questions | Uses the routed MCP surface when configured and obeys `allowed_surfaces` from status |

Hooks fail open: missing Node, MCP, or degraded sidecars do not block the host.
The agent reads project-scoped status once, reuses it while repository/runtime
state is unchanged, and follows the routed local graph when deep retrieval is
blocked. Sidecar repair is reserved for tasks that actually need packet/search.

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

Run these three checks before your first real task:

1. **Adapter present** — `claude plugin list` shows **codestory**, or you
   started Claude with `--plugin-dir plugins/codestory`. Confirm the manifest
   at `plugins/codestory/.claude-plugin/plugin.json` and hooks at
   `plugins/codestory/hooks/claude-codex-hooks.json`.
2. **Hooks live** — Start a new session; you should see CodeStory hook status
   messages on session start or prompt submit (hooks fail open if Node is
   missing).
3. **First status read succeeds** — Use the readiness prompt in [First
   session](#first-session). The agent should answer in plain English whether
   your repo map is ready and whether broad search is available.

## First session

1. Start a new Claude Code session in the repository (startup, resume, clear, or
   compact triggers session hooks).
2. Ask:

```text
Read codestory://status, ground this checkout if allowed, and tell me which CodeStory surfaces are ready before I edit.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready, says whether
broad search is available, and does not report a missing CLI or broken MCP
connection.

Hooks inject routing, not grounding evidence. Follow the selected live MCP
surface and its evidence gaps before making source claims.

## Example prompts

**Readiness**

```text
Check CodeStory status and allowed_surfaces before I edit [path/to/file].
```

**Find ownership**

```text
Where is [Feature] defined and who calls it?
```

**Plan a change**

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Broad question (when packet/search ready)**

```text
How does [subsystem] interact with [other area]? Use packet or search only if retrieval_mode is full.
```

More pairs and anti-patterns: [Prompt patterns](prompt-patterns.md).

## Troubleshooting

| Symptom | What to try |
| --- | --- |
| Hooks silent | Confirm `node` on PATH; check `CLAUDE_PLUGIN_ROOT` resolves to plugin dir |
| No MCP tools | Add MCP server config; see [Cursor MCP section](cursor.md#2-mcp-server-copy-shipped-config) |
| Packet/search blocked | [Troubleshooting - packet/search](troubleshooting.md#packetsearch-degraded-or-blocked) |
| Hook timeout | Session continues; use explicit status prompt |

Shared repair: [Troubleshooting](troubleshooting.md).

## Limitations vs Codex

| Feature | Claude Code | Codex |
| --- | --- | --- |
| MCP auto-start | Manual | Yes |
| Hooks | Session + prompt | Session + prompt |
| Skill | Partial (host-dependent) | Full `@CodeStory` skill |
| Managed CLI | Yes when MCP sets `CODESTORY_PLUGIN_DATA` | Yes via plugin |

Claude Code matches Codex on hooks when the plugin is installed, but MCP and
managed bootstrap are not automatic unless you configure them.

Compare: [capability matrix](README.md#capability-matrix).
