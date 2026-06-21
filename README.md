<h1 align="center">CodeStory</h1>

<p align="center">
Local codebase grounding for coding agents, with local evidence, readiness
checks, and source citations the human operator can inspect.
</p>

<p align="center">
<a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue"></a>
<a href="Cargo.toml"><img alt="Rust 2024" src="https://img.shields.io/badge/rust-2024-orange"></a>
</p>

CodeStory is for the human supervising a coding agent in a real repository.
Install it when you want the agent to start from local source evidence instead
of chat memory, fuzzy search, or whatever file it noticed first.

It runs on your machine, reads the codebase, reports whether its evidence is
ready, and gives the agent cited source paths to inspect. Use it when the next
question is concrete:

- What is in this repo?
- Which files changed, and what else might be affected?
- Where is this behavior implemented?
- Which symbols, references, snippets, and source trails support the answer?
- Is broad packet/search ready enough to trust?

CodeStory does not replace source review. It makes the agent read the right code
first, and it tells the human when broad packet/search output is only a hint.

## Use It With An Agent

Most humans should use CodeStory through the agent plugin, not by memorizing CLI
commands. For normal Codex use, install the plugin in the workspace you want to
ground:

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

The marketplace catalog repo is `TheGreenCedar/AgentPluginMarketplace`. This
repo owns the CodeStory plugin source under `plugins/codestory`; it does not
contain the marketplace catalog.

Start a new Codex thread after install or refresh so the MCP process gets the
current environment. A good first prompt is:

```text
@CodeStory check whether this repository is ready for local navigation and packet/search, then ground it before planning changes.
```

The plugin launches `codestory-cli serve --stdio --refresh none`. The local MCP
server is read-only: it gives the agent grounding, inventory, graph, snippet,
packet, and search tools; it does not edit your repository.

The skill owns binary setup. It checks `codestory-cli --version`, compares the
installed binary with the latest GitHub release, installs a matching release
asset when practical, and checks `SHA256SUMS.txt` when the host can.

Restart the Codex host/app before starting a new agent thread if setup changed
`PATH`.

CodeStory publishes cross-platform CLI assets for Windows, macOS, and Linux.
Source fallback is still available when a release asset does not fit the host.

## What Your Agent Gets

| Human question | What the agent gets |
| --- | --- |
| Is this repository ready to use? | Status and doctor output that separate local navigation from broad packet/search readiness. |
| What is in this repo? | A compact grounding view and indexed file inventory. |
| What changed, and what might be affected? | Changed-file impact hints for review and planning. |
| Where should we inspect next? | Symbol, reference, definition, trail, snippet, and context tools that point back to source. |
| Can we ask a broad codebase question? | Packet/search only when full retrieval is ready; otherwise the output is navigation help, not proof. |

Common agent surfaces:

| Need | Surface |
| --- | --- |
| Read status | `codestory://status`, `doctor` |
| Ground the repo | `codestory://grounding`, `ground` |
| Inspect file inventory | `files` |
| Map changed-file impact | `affected` |
| Follow concrete source evidence | `symbol`, `trail`, `definition`, `references`, `symbols`, `snippet`, `context` |
| Search broadly with citations | `search`, only with `retrieval_mode=full` |
| Build a bounded task packet | `packet`, only with `retrieval_mode=full` |

The canonical plugin skill is
[plugins/codestory/skills/codestory-grounding/SKILL.md](plugins/codestory/skills/codestory-grounding/SKILL.md).

## Trust And Readiness

Keep the two trust lanes separate:

| Lane | Use it for | Trust it when |
| --- | --- | --- |
| Local navigation | Grounding, file inventory, changed-file impact, graph/source follow-up. | `local_navigation` is ready. |
| Packet/search | Broad repo questions and candidate discovery. | `agent_packet_search` is ready and `retrieval_mode=full`. |

Do not treat packet/search as proof unless `retrieval_mode=full`. Degraded,
partial, stale, or missing sidecar output can help the agent navigate; it is not
product-grade evidence.

Every answer still needs cited source. CodeStory can point at the evidence; the
agent still has to use it honestly.

## When To Use The CLI

Most humans should start with the plugin. Use the CLI when you need a direct
setup, repair, or debug transcript.

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
