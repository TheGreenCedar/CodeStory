# CodeStory Agent Plugin

Every agent question often restarts repository discovery: hunt files, read
snippets, chase imports, rebuild the map. That repeats on every turn and burns
wall time plus context on work you already paid for.

CodeStory gives a coding agent a local, read-only grounding surface before it
plans, reviews, or edits a repository. Index once; answer from evidence with
citations instead of re-exploring the tree on every prompt.

The human job is simple: install the plugin, start a fresh thread in the repo,
and ask the agent to check readiness before it makes source claims. The agent
uses CodeStory for status, grounding, file inventory, graph trails, snippets,
packet, and search. The CLI is still there, but it is the escape hatch and repair surface, not the main user experience.

## What The Agent Gets

| Agent need | CodeStory surface | Human reading |
| --- | --- | --- |
| Is this repo indexed and safe to use? | `codestory://status` | Check readiness before trusting claims. |
| What should I do next? | `codestory://agent-guide` | Let the skill route normal setup and repair. |
| Give me a compact repo map. | `codestory://grounding` | Start from current local files. |
| Inspect indexed file inventory and coverage. | `files` tool | Use for scope, language mix, and missing coverage. |
| Map changed files to likely impact. | `affected` tool | Use for review planning and focused test choice. |
| Find candidate symbols, paths, or behavior terms. | `search` tool, only with full sidecars | Navigation only unless packet/search is full. |
| Answer a broad repo question with evidence. | `packet` tool, only with full sidecars | Proof only when strict sidecars are ready. |
| Follow a concrete target. | `symbol`, `trail`, `references`, `snippet`, `context` | Source anchors still matter. |

The status resource is the contract. Local navigation is ready only when
`local_navigation` is ready. Agent packet/search is eligible only when strict
sidecar status reports `retrieval_mode=full`; answer quality still needs the
matching packet-runtime, drill, or benchmark proof.

## How It Runs

This package stays thin:

- `.codex-plugin/plugin.json` describes the Codex plugin package.
- `.mcp.json` launches `codestory-cli serve --stdio --refresh none` from the
  agent host `PATH`.
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
the plugin source at `https://github.com/TheGreenCedar/CodeStory.git`, with source path `plugins/codestory`. The CodeStory repo does not contain the marketplace catalog.

Some workspace plugin settings are managed from the Codex Apps/Plugins UI
rather than the terminal. Use the UI path when the CLI marketplace command is
unavailable.

Start a new Codex thread after installation or refresh. The installed package
launches `codestory-cli serve --stdio --refresh none` from `PATH`.

### After install

Open Codex in the repo you want to ground and ask the agent to check readiness
before planning or editing:

```text
@CodeStory check local_navigation and agent_packet_search on this checkout, ground the repo, and tell me whether sidecars need repair before I use packet.
```

The first run should be agent-owned. The skill checks whether `codestory-cli` is
present and current, compares `codestory-cli --version` with the latest GitHub
release, installs the latest matching release asset when needed, verifies
`SHA256SUMS.txt` when the host can, and uses source fallback only when no release asset fits the host.

## What To Ask

Use concrete repo terms. These examples are written for the CodeStory
repository; adapt paths and symbols to your project:

**For checking readiness before editing:**

- `@CodeStory check local_navigation and agent_packet_search on this checkout before I edit codestory-indexer.`

**For finding ownership:**

- `@CodeStory Where is RefreshMode defined and which codestory-cli commands accept --refresh?`

**For planning changes with impact hints:**

- `@CodeStory I am editing crates/codestory-indexer/src/resolution/mod.rs. What symbols are affected and what tests should I run first?`

**For understanding sidecar readiness:**

- `@CodeStory Explain where strict_sidecar_status decides retrieval_mode=full.`

**Generalizable prompt templates for any repository:**

**For checking readiness before editing any crate:**

```text
@CodeStory check local_navigation and agent_packet_search on this checkout before I edit [TARGET_CRATE].
```

**For finding ownership of a feature:**

```text
@CodeStory Where is [TARGET_FEATURE] defined and which codestory-cli commands accept --refresh?
```

**For planning changes with impact hints:**

```text
@CodeStory I am editing [PATH_TO_FILE]. What symbols are affected by changes in this file, and what tests should I run first?
```

**For understanding sidecar readiness:**

```text
@CodeStory Explain where strict_sidecar_status decides retrieval_mode=full.
```

Avoid prompts that erase the trust boundary:

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

If `@CodeStory` loads the skill but no `mcp__codestory` tools or
`codestory://status` resource are exposed, treat that as plugin MCP registration
failure. The CLI can collect health evidence, but it does not prove the installed
MCP surface is live.

Packet/search evidence needs sidecars:

```console
codestory-cli retrieval bootstrap --project <repo> --format json
codestory-cli retrieval index --project <repo> --refresh full
codestory-cli retrieval status --project <repo> --format json
```

Do not treat `ground`, `symbol`, `trail`, or `snippet` readiness as proof that
agent packet/search is ready.

### Agent runtime bootstrap

The plugin does not bundle the binary. The agent-owned skill verifies
`codestory-cli --version`, compares it with the latest GitHub release, installs
the matching release asset when practical, and checks `SHA256SUMS.txt` when the
host can. If `PATH` changed, the skill tells the human that a Codex host/app restart may be needed before a fresh agent thread can see it.
If a running `codestory-cli serve --stdio --refresh none` process locks the old
binary, install the current release into a versioned directory and put that
directory before stale entries on `PATH`; verify the command that MCP will
launch with `codestory-cli --version` before starting a fresh Codex thread.

Use source fallback only when no release asset fits the host:

```sh
cargo build --release -p codestory-cli
```

Then put `target/release` on the agent host `PATH` for installed MCP runtime
use. Set `CODESTORY_CLI` only for manual CLI fallback commands or source-build
debugging; `.mcp.json` does not launch through that variable.

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
python <path-to-plugin-creator>\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```

The plugin validator path is maintainer-local. The committed repo check for
plugin docs/static contracts is the Node test above.
