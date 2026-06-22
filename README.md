# CodeStory

**Local code intelligence for coding agents** — graph-backed context, source citations, and explicit uncertainty.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/rust-2024-orange)](Cargo.toml)

CodeStory indexes your repository once and keeps a local, read-only map ready: files, symbols, call paths, snippets, and bounded answer packets with citations. The agent starts from evidence it can cite instead of re-exploring the tree from scratch.

## Quick start

The normal path is the **Codex plugin**. The CLI and MCP server are for setup, repair, and transcripts.

1. Open Codex in the repository you want to ground.
2. Run `/plugins`, then install **TheGreenCedar → codestory**.
3. Start a fresh thread and ask:

```text
@CodeStory check local_navigation and agent_packet_search on this checkout, ground the repo, and tell me whether sidecars need repair before I use packet.
```

The plugin launches `codestory-cli serve --stdio --refresh none` on your machine. It does not edit your repository. Install details, binary bootstrap, and uninstall notes live in the [plugin README](plugins/codestory/README.md).

**Verify without the agent:**

```sh
codestory-cli doctor --project <repo>
```

**If install or MCP fails:** marketplace registration, CLI bootstrap, and host restart notes live in the [plugin README](plugins/codestory/README.md).

**If a readiness lane fails:** follow [Usage - Operator Journey](docs/usage.md#operator-journey), [Stale Local Cache](docs/usage.md#stale-local-cache), and [Sidecar Repair](docs/usage.md#sidecar-repair).

Full operator flow, prompt catalog, and command cheat sheet: [docs/usage.md](docs/usage.md).

## Example prompts

Use concrete repo terms, not generic architecture words. When working in the CodeStory repository itself, these are good shapes:

**Find ownership**

```text
@CodeStory Where is RefreshMode defined, which codestory-cli commands accept --refresh, and what is the call path from index into codestory-store?
```

**Plan a change with impact hints**

```text
@CodeStory I am editing crates/codestory-indexer/src/resolution/mod.rs. What symbols are affected by changes in this file, and what tests should I run first?
```

**Broad question**

```text
@CodeStory Explain where strict_sidecar_status decides retrieval_mode=full.
```

Portable templates and adaptation guidance: [Usage - Example prompts](docs/usage.md#example-prompts).

## What your agent gets

| Need | CodeStory surface |
| --- | --- |
| Repo orientation | Grounding snapshot, file inventory, language coverage |
| Symbol lookup | Search, symbol, snippet |
| Behavior tracing | Trails across callers, callees, imports, and references |
| Change impact | Affected-file hints for review and test selection |
| Broad repo questions | Bounded `packet` output with citations and follow-up commands |
| Candidate discovery | `search` when `agent_packet_search` is ready and `retrieval_mode=full` |

Two readiness lanes matter:

- **Local navigation** — graph, trails, snippets, and impact hints from the SQLite index.
- **Agent packet/search** — broad discovery and task packets only when sidecar retrieval reports `retrieval_mode: full`.

Treat degraded packet/search output as a lead to inspect, not proof.

## CLI escape hatch

Use the CLI when you need a direct setup, repair, or debug transcript:

```sh
codestory-cli doctor --project <repo>
codestory-cli index --project <repo> --refresh auto
codestory-cli ground --project <repo> --why
codestory-cli files --project <repo> --limit 80
codestory-cli affected --project <repo> --format markdown
```

When packet/search readiness is the question:

```sh
codestory-cli retrieval status --project <repo> --format json
```

Repair commands and sidecar setup: [docs/usage.md](docs/usage.md) and [docs/ops/retrieval-sidecars.md](docs/ops/retrieval-sidecars.md).

### Build from source

```sh
cargo build --release -p codestory-cli
./target/release/codestory-cli doctor --project .
```

On Windows PowerShell, use `.\target\release\codestory-cli.exe`.

## With vs without CodeStory

The product comparison is with CodeStory versus ordinary local exploration on
the same repo task. Two recorded tiers live here; both are scoped A/B evidence,
not promises for every future question.

### Focused README task

The focused `readme-with-without` task
[`codestory-index-refresh-mode`](benchmarks/tasks/readme-with-without/codestory-index-refresh-mode.task.json)
asks the agent to identify the `RefreshMode` enum, cite the owning file, and
pass the manifest quality gate.

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs `
  --task-suite readme-with-without `
  --arms without_codestory,with_codestory `
  --repeats 1 `
  --codestory-cli .\target\release\codestory-cli.exe `
  --out-dir .\target\agent-benchmark\issue-352-readme-with-without-v4 `
  --timeout-ms 300000
```

Evidence artifact: `target/agent-benchmark/issue-352-readme-with-without-v4`.
Both arms passed the task quality gate (`1/1`), the CodeStory packet manifest
passed (`1/1`), packet/search readiness was `retrieval_mode full`, and the
CodeStory arm used zero post-packet source reads.

| Metric | Without CodeStory | With CodeStory | Result |
| --- | ---: | ---: | --- |
| Token/context spend | `198,109` tokens | `35,218` tokens | `82.2%` lower: `(198109 - 35218) / 198109` |
| Repeat-task wall time | `76.571s` | `63.399s` | `17.2%` saved: `(76.571 - 63.399) / 76.571` |
| Tool calls / commands | `11` | `1` | `90.9%` lower: `(11 - 1) / 11` |
| Direct source reads | `8` | `0` | `100%` lower |
| All-in wall time, including ready-cache check | `76.571s` | `76.793s` | `0.3%` slower; setup verification is not hidden |

First setup is separate from repeat-task savings. This run used an already
indexed repo with full retrieval sidecars; the ready-cache check still recorded
`13.394s`, including `7.839s` for retrieval index refresh. Cold setup requires
building or installing `codestory-cli`, running `index --refresh full`, and
preparing retrieval sidecars before packet/search numbers are comparable.

### Language expansion holdout (18 tasks)

Broader public-repo evidence uses the
[`language-support-ab`](benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json)
manifest across 18 pinned OSS packages. Latest recorded suite totals:

| Metric | Without | With | Change |
| --- | ---: | ---: | --- |
| Context tokens | 9,692,559 | 5,514,580 | −43% |
| Repeat-task wall time | 7,943s | 4,343s | −45% |
| Tool calls | 475 | 60 | −87% |
| Direct source reads | 417 | 0 | −100% |

Per-task medians, ranges, reproduction commands, and boundary notes:
[language-expansion holdout stats](docs/testing/language-expansion-holdout-stats.md).

Scope matters: these are manifest-backed benchmark rows, not broad
answer-quality or generalization proof. Do not rerun the no-CodeStory baseline
unless the harness contract changes.

## Documentation

Start from the job you need to do:

| If you want to… | Read |
| --- | --- |
| Install, ground a repo, and use the plugin | [Usage](docs/usage.md) |
| Repair local navigation or sidecar readiness | [Retrieval sidecars ops](docs/ops/retrieval-sidecars.md) |
| Change CodeStory itself | [Contributor setup](docs/contributors/getting-started.md) |
| Verify a claim or PR | [Testing matrix](docs/contributors/testing-matrix.md) |
| Understand retrieval architecture | [Retrieval design](docs/architecture/retrieval-design.md) |
| Review timing and benchmark records | [E2E stats log](docs/testing/codestory-e2e-stats-log.md) and [language-expansion holdout stats](docs/testing/language-expansion-holdout-stats.md) |

Full doc routing: [docs/README.md](docs/README.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
