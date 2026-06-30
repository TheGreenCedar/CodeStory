# CodeStory Agent Plugin

Thin plugin package that wires CodeStory into agent hosts. You install once;
the agent gets local grounding through MCP, hooks, or project rules depending on
the host.

## Host guides

| Host | Install surface | Guide |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json`, `.mcp.json`, `skills/` | [Codex](../../docs/users/codex.md) |
| Cursor | `.cursor/rules/codestory.mdc` | [Cursor](../../docs/users/cursor.md) |
| Claude Code | `.claude-plugin/plugin.json`, `hooks/claude-codex-hooks.json` | [Claude Code](../../docs/users/claude-code.md) |
| GitHub Copilot CLI | `.github/plugin/plugin.json`, `hooks/copilot-hooks.json` | [Copilot CLI](../../docs/users/copilot.md#copilot-cli) |
| GitHub Copilot editor | `.github/copilot-instructions.md` (repo root) | [Copilot editor](../../docs/users/copilot.md#copilot-editor) |

Start at the [user guides hub](../../docs/users/README.md) for capability
comparison and portable prompts.

## What runs

- `scripts/codestory-mcp.cjs` -- stdio MCP adapter; provisions a managed CLI when configured
- `hooks/` -- lifecycle activation for hook-capable hosts
- `skills/codestory-grounding/` -- canonical grounding skill (Codex and partial other hosts)

The adapter prefers a checksummed plugin-managed CLI. It can provision from
GitHub release `SHA256SUMS.txt`, honor `CODESTORY_CLI` as a local-dev override,
and stay up with diagnostic `codestory://status` when managed setup fails.
Ambient `PATH` binaries are reported as diagnostics only; installed plugin
runtime does not launch them.

## Codex install (summary)

1. Open Codex in the repository you want to ground.
2. Run `/plugins` and install **TheGreenCedar -> codestory**.
3. Start a fresh thread; follow [Codex guide](../../docs/users/codex.md).

Marketplace catalog: `TheGreenCedar/AgentPluginMarketplace` (display name
`TheGreenCedar`). Plugin source: this repo at `plugins/codestory`.

Refresh or uninstall from `/plugins` in the Codex UI. Terminal marketplace
commands (`codex plugin marketplace add|upgrade|remove`) are optional when your
Codex build exposes them.

Marketplace refresh is not runtime reload: `codex plugin marketplace upgrade`
only refreshes the catalog snapshot. Refresh the installed package from
`/plugins` or `codex plugin add codestory@TheGreenCedar`, then start a fresh
Codex host session before trusting the active MCP runtime. On Windows, an
`Access is denied` cache-backup error usually means an old host still has the
previous plugin files open; quit stale Codex windows and retry.

## Repair and CLI

Normal users repair through the agent and MCP (`codestory://status`,
`sidecar_setup`). Power-user CLI transcripts:
[CLI reference](../../docs/users/cli-reference.md).

Blocked session steps: [Troubleshooting](../../docs/users/troubleshooting.md).

## Maintainer checks

```powershell
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```

`plugin-static` validates adapter/skill structure and runtime wiring only — it
does not assert documentation copy or prose phrases.

Agent portability reference (maintainer): [agent-portability.md](docs/agent-portability.md).
