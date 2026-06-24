# Agent Portability

CodeStory ships one grounding skill and thin host adapters. The user-facing
path is one flow: run agent preflight, install the plugin or adapter, then let
MCP plus hooks keep CodeStory ambient.

```sh
codestory-cli agent preflight --project <repo> --format json
```

The skill owns the runtime rules; adapters make the rules ambient in hosts that
support lifecycle hooks or project instruction files. Where the host exposes
prompt input, the hook attempts request-aware packet grounding before the agent
opens source files.

| Host | Files | Behavior |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json`, `.mcp.json`, `skills/` | Starts the stdio MCP server and ships the grounding skill. |
| Claude Code | `.claude-plugin/plugin.json`, `hooks/claude-codex-hooks.json` | Attempts strict session grounding and request-aware packet grounding through lifecycle hooks. |
| GitHub Copilot CLI | `.github/plugin/`, `hooks/copilot-hooks.json`, `skills/` | Injects status-first grounding rules at session start. |
| GitHub Copilot editor | `.github/copilot-instructions.md` | Repository instruction fallback. |
| Cursor | `.cursor/rules/codestory.mdc` | Always-on project rule fallback. |

Keep adapters thin. When a host supports hooks or skills, point it at the
existing `hooks/` and `skills/` files. When a host only supports project
instructions, keep the copied rule text aligned with the root instruction file.

Every adapter should preserve the same first check: read `codestory://status`
when MCP is live, then trust only the surfaces allowed by status. Broad
`packet`, `search`, and `context` use still requires `retrieval_mode=full`.
Hooks ground on session start, resume, clear, and compact handoff; attempt
request-aware grounding on user prompts; and fail open with the next CodeStory
check instead of blocking the host.
