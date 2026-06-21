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

There is no Node adapter and no marketplace catalog in this repository. The
catalog lives in `TheGreenCedar/AgentPluginMarketplace` with catalog name
`TheGreenCedar`. CodeStory owns the plugin package and the Rust CLI/MCP runtime.

## Install For Agent Use

Install the plugin from the external marketplace, then make `codestory-cli`
available on the same host that runs the agent.

The plugin does not bundle the binary. If `codestory-cli` is missing, the agent
should download and unpack the matching release asset, verify it when practical,
place it in a stable user bin directory, and re-check `codestory-cli --version`.
If the host `PATH` changes, start a new agent thread so the MCP process can see
the binary.

The archive names below are release-bound to CodeStory `v0.11.1`.

| Host OS | Binary setup |
| --- | --- |
| Windows x64 | Download `codestory-cli-v0.11.1-windows-x64.zip`, extract `codestory-cli.exe`, and put it on `PATH`. |
| Windows arm64 | Download `codestory-cli-v0.11.1-windows-arm64.zip`, extract `codestory-cli.exe`, and put it on `PATH`. |
| macOS arm64 | Download `codestory-cli-v0.11.1-macos-arm64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |
| macOS x64 | Use the source fallback until a matching release asset exists. |
| Linux x64 | Download `codestory-cli-v0.11.1-linux-x64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |
| Linux arm64 | Download `codestory-cli-v0.11.1-linux-arm64.tar.gz`, extract `codestory-cli`, put it on `PATH`, and run `chmod +x codestory-cli` if needed. |

Verify downloaded archives against `SHA256SUMS.txt` from the release when the
host has the tools to do it. Source fallback for any OS:

```sh
cargo build --release -p codestory-cli
```

Then put `target/release` on the agent host `PATH`, or set `CODESTORY_CLI` for
manual CLI fallback commands.

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

## Review Checks

```powershell
python C:\Users\alber\.codex\skills\.system\plugin-creator\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```
