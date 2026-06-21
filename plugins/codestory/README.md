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
| Inspect indexed file inventory and coverage. | `files` tool |
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

If your Codex build exposes terminal marketplace management for source
marketplaces, add or refresh this marketplace first:

```bash
codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace
```

The marketplace catalog repo is `TheGreenCedar/AgentPluginMarketplace`; its
marketplace display/name concept is `TheGreenCedar`. This repository remains
the plugin source at `https://github.com/TheGreenCedar/CodeStory.git`, with
source path `plugins/codestory`. The CodeStory repo does not contain the marketplace catalog.

Some workspace plugin settings are managed from the Codex Apps/Plugins UI
rather than the terminal. Use the UI path when the CLI marketplace command is
unavailable.

Start a new Codex thread after installation or refresh. The installed package
launches `codestory-cli serve --stdio --refresh none` directly.

### After install

Open Codex in the repo you want to ground and ask the agent to check readiness
before planning or editing:

```text
@CodeStory check whether this repository is ready for local navigation and packet/search, then ground it before planning changes.
```

The first run should be agent-owned. The skill checks whether `codestory-cli` is
present and current, installs the latest matching release asset when needed,
and uses source fallback only when no release asset fits the host.

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

### Agent runtime bootstrap

The plugin does not bundle the binary. The agent-owned skill verifies
`codestory-cli --version`, compares it with the latest GitHub release, installs
the matching release asset when practical, checks `SHA256SUMS.txt` when the host
can, and restarts the Codex host/app before starting a new agent thread if the
MCP process needs a fresh `PATH`.

Use source fallback only when no release asset fits the host:

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
| Marketplace catalog repo | `TheGreenCedar/AgentPluginMarketplace` |
| Marketplace display/name | `TheGreenCedar` |
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
