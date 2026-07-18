# CodeStory agent plugin

The plugin connects agent hosts to the native CodeStory CLI. It contains no
indexing or retrieval implementation of its own: hooks teach routing, the MCP
adapter selects a verified CLI, and every live tool request names its repository
explicitly.

## Host surfaces

| Host | Plugin surface | User guide |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json`, `.mcp.json`, hooks, skill | [Codex](../../docs/users/codex.md) |
| Cursor | `.cursor/rules/codestory.mdc`, `.cursor/mcp.json` | [Cursor](../../docs/users/cursor.md) |
| Claude Code | `.claude-plugin/plugin.json`, session hooks | [Claude Code](../../docs/users/claude-code.md) |
| Copilot CLI | `.github/plugin/plugin.json`, session hooks | [Copilot](../../docs/users/copilot.md#copilot-cli) |
| Copilot editor | Repository instructions | [Copilot editor](../../docs/users/copilot.md#copilot-editor) |

The [user guide](../../docs/users/README.md) owns shared first-use, platform,
privacy, and readiness behavior.

## Package anatomy

- `scripts/codestory-mcp.cjs` is the stdio adapter and managed CLI launcher.
- `hooks/` records bounded lifecycle state for hosts that support hooks.
- `skills/codestory-grounding/` defines the canonical direct-tool and evidence
  contract.
- host manifests and rules point those pieces at Codex, Cursor, Claude Code,
  and Copilot.

Hooks do not inject source claims or route a request through an ambient active
project. They tell the agent to use the live MCP tool with an absolute `project`
root. If MCP is unavailable, the agent reports the gap and uses ordinary source
inspection.

## Runtime handoff

The adapter starts one projectless, multi-repository MCP runtime. It prefers the
exact checksummed CLI version declared by the plugin. If that CLI is missing,
one installer publishes it while other requests wait or receive a bounded
preparing response. `CODESTORY_CLI` is an explicit local-development override;
ambient `PATH` binaries are diagnostic only and are not launched by an installed
plugin.

The managed installer verifies the release checksum manifest, archive,
executable, plugin version, `--version`, and MCP initialization before
publication. Archive extraction is bounded, publication is atomic, concurrent
installers share one owner, unsafe replacement fails closed, and a corrupt
target is quarantined before one reprovision attempt. Status reports retained
versions and any terminal provisioning error.

This network activity installs or updates the CodeStory CLI package. It is not
an embedding-runtime download: the verified CLI already contains its model and
linked backend. Once installed, repository indexing and retrieval require no
model download, separate helper executable, TCP endpoint, port, or user
approval. The same verified CLI automatically runs its hidden per-user server
over private local IPC.

## Codex install

1. Open `/plugins` in Codex.
2. Install **TheGreenCedar -> codestory**.
3. Start a fresh Codex host session.

Marketplace catalog: `TheGreenCedar/AgentPluginMarketplace`. Refresh or remove
the package from the same `/plugins` screen. Some Windows Codex builds also
expose `codex.cmd plugin marketplace ...` and `codex.cmd plugin add ...`.

Marketplace refresh updates the catalog only. Package refresh replaces the
installed plugin, and a fresh host session loads that replacement. See the
[Codex update guide](../../docs/users/codex.md#update).

## Diagnostics

Normal calls prepare the repository automatically. Agents call the intended
tool first and retry it while preparation runs. `codestory://status` and the
[CLI reference](../../docs/users/cli-reference.md) are diagnostic surfaces for
failed convergence, not first-use steps.

Blocked session steps: [Troubleshooting](../../docs/users/troubleshooting.md).

## Maintainer checks

```sh
node scripts/generate-codestory-skill-syntax.mjs --check
node --test plugins/codestory/tests/plugin-static.test.mjs
node .github/scripts/check-doc-links.mjs
git diff --check
```

Build `codestory-cli` before checking generated syntax. When Clap syntax
changes, run the generator with `--rewrite-references` to refresh the compact
index and remove copied option matrices from the skill references.

`plugin-static` checks adapter, manifest, skill, and runtime wiring. It does not
assert prose.

Host-adapter boundary: [Agent portability](docs/agent-portability.md).
