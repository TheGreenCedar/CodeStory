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

The adapter prefers a checksummed plugin-managed CLI and starts one projectless
MCP runtime. Every tool call carries its repository root, so concurrent Codex
tasks can use different projects without rebinding or restarting the server. It can provision from
GitHub release `SHA256SUMS.txt`, honor `CODESTORY_CLI` as a local-dev override,
and open diagnostic `codestory://status` immediately while a missing exact
version is provisioned in the background. The next request after verified
publication is handed to the real stdio runtime; terminal setup failures remain
available through the same diagnostic MCP.
Ambient `PATH` binaries are reported as diagnostics only; installed plugin
runtime does not launch them.

After a managed runtime passes archive checksum, executable checksum, manifest,
`--version`, and MCP stdio `initialize` verification, the adapter retains that
active version plus one verified pending upgrade or rollback. ZIP and tar.gz
release assets are extracted with Node platform APIs, without an external
archive command, under explicit archive-size, entry-count, per-entry, and total
output ceilings. Same-version launches elect one publisher
under an atomically owner-published PID/start-identity/token lock; acquisition
fails closed without a reliable process-start identity, and waiters reuse its
atomically renamed staging directory. Their wait bound covers both release
assets' absolute total download retry windows. The staging MCP probe bounds its
output and waits for child termination with forced-kill escalation. Stale initialization aliases are
revalidated by inode and owner token after rename before deletion. A corrupt target is quarantined (two
copies retained) and reprovisioned once. A live owner or unmovable Windows
executable is never deleted, and publication fails closed when safe quarantine
or replacement is not possible. Publisher, waiter, reclaimed-lock, quarantine,
reprovision, and terminal-failure states appear in `plugin_runtime.warnings`;
retained, removed, and reclaimable byte totals remain under
`managed_cli_retention`.

## Codex install (summary)

1. Open Codex in the repository you want to ground.
2. Run `/plugins` and install **TheGreenCedar -> codestory**.
3. Start a fresh thread; follow [Codex guide](../../docs/users/codex.md).

Marketplace catalog: `TheGreenCedar/AgentPluginMarketplace` (display name
`TheGreenCedar`). Plugin source: this repo at `plugins/codestory`.

Refresh or uninstall from `/plugins` in the Codex UI. On Windows, terminal
commands (`codex.cmd plugin marketplace add|upgrade|remove`) are optional when
your Codex build exposes them. In Unix shells, use `codex` instead of
`codex.cmd`.

Marketplace refresh is not runtime reload: the marketplace upgrade command only
refreshes the catalog snapshot. Refresh the installed package from `/plugins` or
the matching terminal plugin add command, then start a fresh Codex host session
before trusting the active MCP runtime.

## Repair and CLI

Normal users repair through the agent and MCP (`status` and `sidecar_setup`,
both with an explicit `project`). Power-user CLI transcripts:
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
