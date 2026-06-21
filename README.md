<h1 align="center">CodeStory</h1>

<p align="center">
Local codebase grounding for coding agents, with cited repository evidence and
readiness checks the human operator can inspect.
</p>

<p align="center">
<a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue"></a>
<a href="Cargo.toml"><img alt="Rust 2024" src="https://img.shields.io/badge/rust-2024-orange"></a>
</p>

CodeStory is for a human who is about to let a coding agent work in a real
repository.

Its promise is narrow on purpose: the agent should begin from current local
source evidence, cite the files and trails behind its claims, and tell you when
broad packet/search evidence is not ready enough to trust.

Use CodeStory when the next agent answer needs to survive review:

- What is in this repo?
- Which files changed, and what else might be affected?
- Where is this behavior implemented?
- Which symbols, references, snippets, and trails support the answer?
- Is broad packet/search ready enough to use as evidence?

CodeStory does not replace source review, tests, or human judgment. It changes
the starting point: local cited evidence first, confident-sounding guesses last.

## The Trust Loop

```mermaid
flowchart LR
    Human["Human operator"] --> Prompt["Ask the agent to ground the repo"]
    Prompt --> Plugin["CodeStory plugin"]
    Plugin --> Runtime["codestory-cli serve --stdio --refresh none"]
    Runtime --> Evidence["Local graph, files, snippets, trails"]
    Runtime --> Readiness["doctor/readiness status"]
    Evidence --> Agent["Agent plans, reviews, or edits"]
    Readiness --> Agent
    Agent --> Human
    Readiness -. "packet/search only when retrieval_mode=full" .-> Packet["packet/search evidence"]
```

The normal path is plugin-first. The CLI exists for setup, repair, debugging,
and transcripts.

## Use It With An Agent

Most humans should install CodeStory through the agent plugin, not memorize CLI
commands. Open Codex in the workspace you want to ground and use:

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

The marketplace catalog repo is `TheGreenCedar/AgentPluginMarketplace`. Its
marketplace display/name concept is `TheGreenCedar`. This repository is the
plugin source at `https://github.com/TheGreenCedar/CodeStory.git`, with source path `plugins/codestory`. The CodeStory repo does not contain the marketplace catalog.

Start a new Codex thread after install or refresh. A useful first prompt is:

```text
@CodeStory check whether this repository is ready for local navigation and packet/search, then ground it before planning changes.
```

The plugin launches `codestory-cli serve --stdio --refresh none` directly. The
local MCP server is read-only: it gives the agent grounding, inventory, graph,
snippet, packet, and search tools; it does not edit your repository.

The skill owns binary setup. It checks `codestory-cli --version`, compares the
installed binary with the latest GitHub release, installs a matching release
asset when practical, and checks `SHA256SUMS.txt` when the host can. It also
restarts the Codex host/app before starting a new agent thread if `PATH` changed.

CodeStory publishes cross-platform CLI assets for Windows, macOS, and Linux.
Source fallback is available when a release asset does not fit the host.

## What Your Agent Gets

| Human question | CodeStory surface | Trust boundary |
| --- | --- | --- |
| Is this repository ready to use? | `codestory://status`, `doctor` | Separates local navigation from packet/search readiness. |
| What is in this repo? | `codestory://grounding`, `ground`, `files` | Source-backed orientation, not a proof of every behavior. |
| What changed, and what might be affected? | `affected` | Review planning help; still run the relevant tests. |
| Where should we inspect next? | `symbol`, `trail`, `definition`, `references`, `symbols`, `snippet`, `context` | Follow concrete source anchors before claiming facts. |
| Can we ask a broad codebase question? | `packet`, `search` | Proof only with `agent_packet_search` ready and `retrieval_mode=full`. |

The canonical plugin skill is
[plugins/codestory/skills/codestory-grounding/SKILL.md](plugins/codestory/skills/codestory-grounding/SKILL.md).

## Trust And Readiness

| Lane | Use it for | Trust it when | If not ready |
| --- | --- | --- | --- |
| Local navigation | Grounding, file inventory, changed-file impact, graph/source follow-up. | `local_navigation` is ready. | Refresh or rebuild the local cache, then source-read the named files. |
| Packet/search | Broad repo questions and candidate discovery. | `agent_packet_search` is ready and `retrieval_mode=full`. | Treat output as navigation help only, then repair sidecars or fall back to direct source reads. |

Non-`full` packet/search output is not proof. Degraded, partial, stale, fallback,
or missing sidecar output may help the agent choose files to inspect; it cannot
carry a product-grade claim.

## When To Use The CLI

Use the CLI when you need a direct setup, repair, or debug transcript.

Setup and local navigation:

```sh
codestory-cli doctor --project <repo>
codestory-cli index --project <repo> --refresh auto
codestory-cli ground --project <repo> --why
codestory-cli files --project <repo> --limit 80
codestory-cli affected --project <repo> --format markdown
```

Repair a stale local cache:

```sh
codestory-cli doctor --project <repo>
codestory-cli index --project <repo> --refresh full
codestory-cli doctor --project <repo>
```

Debug packet/search readiness:

```sh
codestory-cli retrieval status --project <repo> --format json
```

Repair packet/search sidecars:

```sh
codestory-cli retrieval bootstrap --project <repo> --format json
codestory-cli retrieval index --project <repo> --refresh full
codestory-cli retrieval status --project <repo> --format json
```

For source checkout work:

```sh
cargo build --release -p codestory-cli
```

On Windows PowerShell, use `.\target\release\codestory-cli.exe` and normal
Windows paths. The release-binary installer path is:

```powershell
.\scripts\install-codestory.ps1 -Project C:\path\to\repo
```

See [docs/usage.md](docs/usage.md) for task-shaped flows and
[docs/ops/retrieval-sidecars.md](docs/ops/retrieval-sidecars.md) for
packet/search setup and repair.

## Docs For Operators And Contributors

- [docs/usage.md](docs/usage.md)
- [docs/concepts/how-codestory-works.md](docs/concepts/how-codestory-works.md)
- [docs/architecture/overview.md](docs/architecture/overview.md)
- [docs/architecture/language-support.md](docs/architecture/language-support.md)
- [docs/contributors/getting-started.md](docs/contributors/getting-started.md)
- [docs/contributors/debugging.md](docs/contributors/debugging.md)
- [docs/contributors/testing-matrix.md](docs/contributors/testing-matrix.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
