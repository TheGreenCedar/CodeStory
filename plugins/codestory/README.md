# CodeStory agent plugin

The plugin connects agent hosts to the native CodeStory CLI. It contains no
indexing or retrieval implementation of its own: hooks teach routing, the MCP
adapter selects a verified CLI, and every live tool request names its repository
explicitly.

## Host surfaces

The portable [Agent Plugins v1](https://agent-plugins.org/specification) core is
`plugin.json`, `skills/`, and `mcp.json`. Host manifests add only the rules,
hooks, and compatibility wiring their clients require.

| Host | Plugin surface | User guide |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json`, legacy `.mcp.json`, hooks, skill | [Codex](../../docs/users/codex.md) |
| Cursor | Portable core plus `.cursor-plugin/plugin.json`, `rules/`, and `hooks/cursor-hooks.json` | [Cursor](../../docs/users/cursor.md) |
| Claude Code | `.claude-plugin/plugin.json`, legacy `.mcp.json`, session hooks | [Claude Code](../../docs/users/claude-code.md) |
| Copilot CLI | `.github/plugin/plugin.json`, session hooks | [Copilot](../../docs/users/copilot.md#copilot-cli) |
| Copilot editor | Repository instructions | [Copilot editor](../../docs/users/copilot.md#copilot-editor) |

The [user guide](../../docs/users/README.md) owns shared first-use, platform,
privacy, and readiness behavior.

## Package anatomy

- `scripts/codestory-mcp.cjs` is the stdio adapter and managed CLI launcher.
- `plugin.json`, `skills/`, and `mcp.json` are the portable plugin core.
- `hooks/` records bounded lifecycle state for hosts that support hooks.
- `rules/codestory.mdc` is Cursor's always-on grounding rule.
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
the launcher fetches and publishes it while other requests wait or receive a
bounded preparing response. `CODESTORY_CLI` is an explicit local-development
override; ambient `PATH` binaries are diagnostic only and are not launched by
an installed plugin.

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

<!-- codestory-public-support:start -->
| Platform | Release support |
| --- | --- |
| macOS 15+ on Apple Silicon | Supported with Metal |
| Windows x64 | Supported with Vulkan |
| Linux x64 | Supported with Vulkan |
| CPU-only Windows and Linux | Unsupported |
| Intel Mac | Unsupported |
| Windows ARM | Unsupported |
<!-- codestory-public-support:end -->

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

## Cursor install

Open **Customize → Plugins**, install **codestory**, enable its MCP server once,
and reload the Cursor window. The package supplies the rule, grounding skill,
session-start context, and managed CLI launcher. On the first MCP call, the
launcher fetches the matching managed runtime if it is not already installed.
The MCP toggle is a Cursor platform setting, so installing the plugin cannot
enable it on your behalf.

For Teams or Enterprise distribution, an administrator uses **Dashboard →
Plugins → Team Marketplaces → Add Marketplace → Import from Repo** and selects
this repository. Cursor's access settings must grant the workspace access to
the repository, including the required organization or repository permission
for a private repository. The import reads
[`.cursor-plugin/marketplace.json`](../../.cursor-plugin/marketplace.json).
Enable **Auto Refresh** or use **Refresh** from the Team Marketplaces dashboard
after an update. That refresh updates the team catalog; each user still refreshes
the installed plugin from Customize and reloads Cursor.

## CodeStoryDev / Cursor refresh

### Codex CodeStoryDev

Maintainers dogfood an unpublished head through the local `CodeStoryDev`
marketplace. Build the exact CLI, commit the plugin package, then run:

```sh
node scripts/install-codestory-dev-plugin.mjs \
  --cli "$(pwd)/target/release/codestory-cli"
```

The installer stages the clean committed `plugins/codestory` package, the
platform-native CLI, and `.codestory-dev-cli.json`, then refreshes only
`codestory@CodeStoryDev`. The receipt binds the source-package digest, plugin
ID/version, platform, direct executable name/path, bytes, SHA-256, and reported
CLI version. It preserves `~/.codex/plugins/data/codestory-CodeStoryDev`.

The installed launcher validates the cached receipt again with an empty
`PATH`. If the receipt, package, cache copy, or CLI changed—or if
`CODESTORY_CLI` is also set—it reports the receipt failure and does not try the
production release installer. Start a fresh Codex host after a successful
refresh to load the new adapter.

### Cursor local package

After committing the plugin package, link the checkout into Cursor with:

```sh
node scripts/install-codestory-cursor-plugin.mjs
```

Pass `--cli "$(pwd)/target/release/codestory-cli"` to use an exact local CLI
build. The installer writes only that path to Cursor's private CodeStory data
directory; no repository or process environment is copied. Reload Cursor,
enable **codestory** MCP in Customize, and start a fresh agent session.

## Diagnostics

Normal calls prepare the repository automatically. Agents call the intended
tool first and retry it while preparation runs. Project-scoped resources use
the advertised `{?project}` templates; for example, status binds the caller's
percent-encoded absolute root in `codestory://status?project=...`.
`codestory://agent-guide` stays static and project-free. Status and the
[CLI reference](../../docs/users/cli-reference.md) are diagnostic surfaces for
failed convergence, not first-use steps.

Blocked session steps: [Troubleshooting](../../docs/users/troubleshooting.md).

## Maintainer checks

```sh
node scripts/generate-codestory-skill-syntax.mjs --check
node --test scripts/tests/install-codestory-dev-plugin.test.mjs
node --test scripts/tests/install-codestory-cursor-plugin.test.mjs
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
