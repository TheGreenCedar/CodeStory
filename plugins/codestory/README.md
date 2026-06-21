# CodeStory Agent Plugin

CodeStory is the local grounding layer your coding agent should use before it
starts guessing through a repository.

The human job is simple: install the plugin, make sure `codestory-cli` is
available on the agent host, then ask the agent to ground the repo before it
plans, reviews, or edits. The agent should use CodeStory's MCP tools and
resources for status, grounding, search, graph trails, snippets, and broad task
packets. The CLI is still there, but it is the escape hatch and repair surface,
not the main user experience.

## What The Agent Gets

| Agent need | CodeStory surface |
| --- | --- |
| Is this repo indexed and safe to use? | `codestory://status` |
| What should I do next? | `codestory://agent-guide` |
| Give me a compact repo map. | `codestory://grounding` |
| Answer a broad repo question with evidence. | `packet` tool, only with full sidecars |
| Find candidate symbols, paths, or behavior terms. | `search` tool, only with full sidecars |
| Follow a concrete target. | `symbol`, `trail`, `references`, `snippet`, `context` |

The status resource is the contract. Local navigation is ready only when
`local_navigation` is ready. Agent packet/search is ready only when strict
sidecar status reports `retrieval_mode=full`.

## How It Runs

This package is intentionally thin:

- `.codex-plugin/plugin.json` describes the Codex plugin package.
- `.mcp.json` launches `codestory-cli serve --stdio --refresh none` directly.
- `skills/codestory-grounding` is the single canonical CodeStory grounding
  skill shipped by this repository.

There is no Node adapter and no marketplace catalog in this repository.
CodeStory owns the plugin package and the Rust CLI/MCP runtime.

## Install For Agent Use

For normal Codex use, install the plugin through the Codex plugin flow for your
workspace. Open Codex in the repo you want to ground, then use:

```text
/plugins
```

Choose:

```text
TheGreenCedar -> codestory -> Install plugin
```

If the `TheGreenCedar` catalog is not listed and your Codex build exposes
terminal marketplace management for source marketplaces, add or refresh the
external catalog first:

```bash
codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace
```

The marketplace source is `TheGreenCedar/AgentPluginMarketplace`.
This repository remains the plugin source. One marketplace can list multiple plugins.
CodeStory's entry points at `https://github.com/TheGreenCedar/CodeStory.git`
with source path `plugins/codestory`.

Then return to `/plugins` and install `TheGreenCedar -> codestory`. Some
workspace plugin settings are managed from the Codex Apps/Plugins UI rather
than the terminal, so use the UI path when the CLI marketplace command is
unavailable.

Start a new Codex thread after installation or refresh. The installed package
launches `codestory-cli serve --stdio --refresh none` directly.

### After install

Open Codex in the repo you want to ground and ask the agent to check readiness
before planning or editing:

```text
@CodeStory check whether this repository is ready for local navigation and packet/search, then ground it before planning changes.
```

## What To Ask

Good first prompts are agent-shaped:

- `@CodeStory check whether this repository is ready for local navigation and packet/search.`
- `@CodeStory ground this repository before you plan the change.`
- `@CodeStory find the code paths involved in request routing, then inspect the concrete snippets.`
- `@CodeStory answer where cache freshness is enforced, but only make packet/search claims if sidecars are full.`

Bad first prompts are usually CLI-shaped:

- `Run every CodeStory command.`
- `Search broadly and summarize whatever comes back.`
- `Trust packet/search even though status says sidecars are degraded.`

## Manual CLI Escape Hatch

Use the CLI when the agent needs to repair setup, produce a transcript, or debug
why the MCP server is not ready:

```console
codestory-cli --version
codestory-cli doctor --project <repo> --format markdown
codestory-cli index --project <repo> --refresh full
codestory-cli retrieval status --project <repo> --format json
codestory-cli serve --project <repo> --stdio --refresh none
```

Packet/search evidence needs sidecars:

```console
codestory-cli retrieval bootstrap --project <repo> --format json
codestory-cli retrieval index --project <repo> --refresh full
codestory-cli retrieval status --project <repo> --format json
```

Do not treat `ground`, `symbol`, `trail`, or `snippet` readiness as proof that
agent packet/search is ready. That mistake is how agents write confident
nonsense with a straight face.

### Runtime latest-release fallback

The plugin does not bundle the binary. The agent should run
`codestory-cli --version` first. If `codestory-cli` is missing or outdated
against the latest GitHub release, resolve the latest release, download and
unpack the matching host asset, verify it when practical, place it in a stable
user bin directory, and re-check `codestory-cli --version`. If the host `PATH`
changes, start a new agent thread so the MCP process can see the binary.

For latest tag `vX.Y.Z`, choose the host asset derived from that tag:

| Host OS | Binary setup |
| --- | --- |
| Windows x64 | Download `codestory-cli-vX.Y.Z-windows-x64.zip`, extract `codestory-cli.exe`, and put it on `PATH`. |
| Windows arm64 | Download `codestory-cli-vX.Y.Z-windows-arm64.zip`, extract `codestory-cli.exe`, and put it on `PATH`. |
| macOS arm64 | Download `codestory-cli-vX.Y.Z-macos-arm64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |
| macOS x64 | Use the source fallback until a matching release asset exists. |
| Linux x64 | Download `codestory-cli-vX.Y.Z-linux-x64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |
| Linux arm64 | Download `codestory-cli-vX.Y.Z-linux-arm64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |

Verify downloaded archives against `SHA256SUMS.txt` from the release when the
host has the tools to do it. Source fallback for any OS:

```sh
cargo build --release -p codestory-cli
```

Then put `target/release` on the agent host `PATH`, or set `CODESTORY_CLI` for
manual CLI fallback commands.

Source docs, marketplace source checkout/cache, and the active installed MCP
runtime can differ. Before claiming an installed behavior is live, verify the
active runtime surface in the target Codex thread.

## Update Or Remove

For normal Codex use, refresh or uninstall the plugin from the Codex plugin
surface:

```text
/plugins
```

Then choose the installed `codestory` plugin and use the available refresh or
uninstall action.

If your Codex build exposes terminal marketplace management for source
marketplaces, these commands may be available:

```bash
codex plugin marketplace upgrade TheGreenCedar
codex plugin marketplace remove TheGreenCedar
```

`marketplace remove` removes the source marketplace registration. It may not
uninstall an already installed workspace plugin. Prefer the plugin UI for
installed-plugin refresh/uninstall actions, and use terminal marketplace
commands only for source registration when your Codex build supports them.

### Marketplace maintainer details

The marketplace catalog is external. One marketplace can list multiple plugins.

| Field | Value |
| --- | --- |
| Catalog repo | `TheGreenCedar/AgentPluginMarketplace` |
| Catalog name | `TheGreenCedar` |
| Plugin entry | `codestory` |
| Source kind | `git-subdir` |
| Source repo | `https://github.com/TheGreenCedar/CodeStory.git` |
| Source path | `plugins/codestory` |

## Review Checks

```powershell
python C:\Users\alber\.codex\skills\.system\plugin-creator\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```
