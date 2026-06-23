# CodeStory Agent Plugin

Every agent question often restarts repository discovery: hunt files, read
snippets, chase imports, rebuild the map. That repeats on every turn and burns
wall time plus context on work you already paid for.

CodeStory gives a coding agent a local, read-only grounding surface before it
plans, reviews, or edits a repository. Index once; answer from evidence with
citations instead of re-exploring the tree on every prompt.

The human job is simple: install the plugin and start a fresh thread in the
repo. Hosts with lifecycle-hook adapters inject CodeStory's status-first
grounding rules at session start, so the agent can check readiness before it
makes source claims without waiting for a special prompt. The agent uses
CodeStory for status, grounding, file inventory, graph trails, snippets, packet,
and search. The CLI is still there, but it is the escape hatch and repair
surface, not the main user experience.

## What The Agent Gets

| Agent need | CodeStory surface | Human reading |
| --- | --- | --- |
| Is this repo indexed and safe to use? | `codestory://status` | Read first; status is the runtime truth. |
| What should I do next? | `codestory://agent-guide` | Let the skill route normal setup and repair. |
| Give me a compact repo map. | `codestory://grounding` | Start from current local files. |
| Inspect indexed file inventory and coverage. | `files` tool | Use for scope, language mix, and missing coverage. |
| Map changed files to likely impact. | `affected` tool | Use for review planning and focused test choice. |
| Follow a local graph target. | `symbol`, `definition`, `trail`, `references`, `snippet`, `symbols`, `get_node`, `neighbors`, `shortest_path`, `query_subgraph` | Check each surface's own allowed bit. |
| Find candidate symbols, paths, or behavior terms. | `search` tool, only when status allows search | Requires `retrieval_mode=full`. |
| Answer a broad repo question or build an evidence context. | `packet` or `context`, only when status allows that surface | Requires `retrieval_mode=full`. |

The status resource is the contract. When MCP is live, read
`codestory://status` first and obey `allowed_surfaces`. Treat
`server_version` and `server_executable` from status as the active runtime
evidence; source docs, marketplace cache contents, and local build outputs can
all differ from the running server.

| Status lane | Allows | Does not allow |
| --- | --- | --- |
| `allowed_surfaces.<surface>.allowed` for local graph surfaces | The named local surface only: `ground`, `files`, `symbol`, `definition`, `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, or `query_subgraph`. | Other local surfaces, `packet`, `search`, or `context`. |
| `allowed_surfaces.packet.allowed`, `allowed_surfaces.search.allowed`, or `allowed_surfaces.context.allowed` with `retrieval_mode=full` | `packet`, `search`, or `context` for broad candidate discovery and evidence packets. | Answer-quality claims without packet-runtime, drill, benchmark, or source evidence. |
| `codestory://status` fields | Current `server_version`, `server_executable`, and `allowed_surfaces`. | Guessing active runtime from source checkout or PATH alone. |

## How It Runs

This package stays thin:

- `.codex-plugin/plugin.json` describes the Codex plugin package.
- `.mcp.json` launches `codestory-cli serve --stdio --refresh none` from the
  agent host `PATH`.
- `hooks/` injects status-first grounding rules at session start for host
  adapters that support lifecycle hooks.
- `skills/codestory-grounding` is the single canonical CodeStory grounding
  skill shipped by this repository.

There is a tiny Node hook adapter, but no Node runtime server and no marketplace
catalog in this repository. CodeStory owns the plugin package and the Rust
CLI/MCP runtime.

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
codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
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

Open the agent host in the repo you want to ground and ask normal repository
questions. With lifecycle hooks enabled, the agent should first check CodeStory
status and allowed surfaces before planning or editing.

If the host does not expose lifecycle hooks yet, use the explicit prompt:

```text
@CodeStory read codestory://status, report allowed_surfaces for this checkout, ground the repo if allowed, and tell me whether packet/search/context need sidecar repair before I use them.
```

The first run should be agent-owned. The skill checks whether `codestory-cli` is
live through MCP by reading `codestory://status`. If MCP is live, the agent uses
`server_version`, `server_executable`, and `allowed_surfaces` from status
instead of rechecking PATH or release metadata.

Use `where.exe codestory-cli` and `codestory-cli --version` only when MCP is
missing, status reports a suspect runtime, or you are debugging/repairing the
installed CLI. If PATH changed during repair, start a fresh Codex host/app
session before treating a new MCP runtime as live.

## What To Ask

Use concrete repo terms. These examples are written for the CodeStory
repository; adapt paths and symbols to your project:

**For checking readiness before editing:**

- `@CodeStory read codestory://status and check allowed_surfaces before I edit codestory-indexer.`

**For finding ownership:**

- `@CodeStory Where is RefreshMode defined and which codestory-cli commands accept --refresh?`

**For planning changes with impact hints:**

- `@CodeStory I am editing crates/codestory-indexer/src/resolution/mod.rs. What symbols are affected and what tests should I run first?`

**For understanding sidecar readiness:**

- `@CodeStory Explain where strict_sidecar_status decides retrieval_mode=full.`

**Generalizable prompt templates for any repository:**

**For checking readiness before editing any crate:**

```text
@CodeStory read codestory://status and check allowed_surfaces before I edit [TARGET_CRATE].
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
- `Trust packet/search/context even though status says sidecars are degraded.`

## Manual CLI Escape Hatch

Use the CLI when the agent needs to repair setup, produce a transcript, or debug
why the MCP server is not ready:

```console
where.exe codestory-cli
codestory-cli --version
codestory-cli ready --goal local --repair --project <repo> --format json
codestory-cli ready --goal agent --repair --project <repo> --format json
codestory-cli doctor --project <repo> --format markdown
codestory-cli retrieval status --project <repo> --format json
codestory-cli serve --project <repo> --stdio --refresh none
```

If `@CodeStory` loads the skill but no `mcp__codestory` tools or
`codestory://status` resource are exposed, treat that as plugin MCP registration
failure. The CLI can collect health evidence, but it does not prove the installed
MCP surface is live.

Packet/search evidence needs sidecars:

```console
codestory-cli ready --goal agent --repair --project <repo> --format json
codestory-cli retrieval status --project <repo> --format json
```

Do not treat `ground`, `symbol`, `trail`, `snippet`, or local graph readiness as
proof that `packet`, `search`, or `context` is ready.

### Agent runtime repair

The plugin does not bundle the binary. The installed MCP runtime launches
`codestory-cli serve --stdio --refresh none` from the agent host `PATH`. Once
MCP is live, `codestory://status` is the runtime proof. Use
`where.exe codestory-cli`, `codestory-cli --version`, and release repair checks
only when MCP is missing or the status fields show stale binary drift.

If status reports `repair_setup`, the active CLI is older than the latest
release. The agent runs the installer command from `recommended_next_calls`
before using local navigation, packet, search, or context.
If a running `codestory-cli serve --stdio --refresh none` process locks the old
binary, install the current release into a versioned directory and put that
directory before stale entries on `PATH`; verify the command that MCP will
launch with `codestory-cli --version`. If `PATH` changed, mention restart only
after the current binary is installed and verified; a Codex host/app restart may
be needed before a fresh agent thread sees the new `PATH`.

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
codex plugin marketplace add TheGreenCedar/AgentPluginMarketplace --ref main
codex plugin marketplace upgrade TheGreenCedar
codex plugin marketplace remove TheGreenCedar
```

If TheGreenCedar was added from a local path, remove it and add the Git marketplace source again before using upgrade.

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

## Agent Portability

CodeStory also ships thin adapters for hosts that do not install the Codex
plugin package directly: Claude Code, GitHub Copilot CLI, GitHub Copilot editor,
and Cursor. See [Agent Portability](docs/agent-portability.md).

## Review Checks

```powershell
python <path-to-plugin-creator>\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```

The plugin validator path is maintainer-local. The committed repo check for
plugin docs/static contracts is the Node test above.
