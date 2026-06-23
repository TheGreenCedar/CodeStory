# Agent Portability

CodeStory ships one grounding skill and thin host adapters. The skill owns the
runtime rules; adapters only make the rules ambient in hosts that support
lifecycle hooks or project instruction files.

| Host | Files | Behavior |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json`, `.mcp.json`, `skills/` | Starts the stdio MCP server and ships the grounding skill. |
| Claude Code | `.claude-plugin/plugin.json`, `hooks/claude-codex-hooks.json` | Injects status-first grounding rules at session start. |
| GitHub Copilot CLI | `.github/plugin/`, `hooks/copilot-hooks.json`, `skills/` | Injects status-first grounding rules at session start. |
| GitHub Copilot editor | `.github/copilot-instructions.md` | Repository instruction fallback. |
| Cursor | `.cursor/rules/codestory.mdc` | Always-on project rule fallback. |

Keep adapters thin. When a host supports hooks or skills, point it at the
existing `hooks/` and `skills/` files. When a host only supports project
instructions, keep the copied rule text aligned with the root instruction file.

Every adapter should preserve the same first check: read `codestory://status`
when MCP is live, then trust only the surfaces allowed by status. Broad
`packet`, `search`, and `context` use still requires `retrieval_mode=full`.
