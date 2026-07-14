# CodeStory

**Local code intelligence for coding agents** -- graph-backed context, source citations, and explicit uncertainty.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/rust-2024-orange)](Cargo.toml)

Give your coding agent a local, read-only map of your repository -- files,
symbols, call paths, snippets, and bounded answer packets with citations -- so
it can plan, review, and edit from deterministic evidence instead of guessing from partial
file reads.

## Pick your host

| Host | Guide |
| --- | --- |
| Codex | [Codex guide](docs/users/codex.md) — recommended first install |
| Cursor | [Cursor guide](docs/users/cursor.md) |
| Claude Code | [Claude Code guide](docs/users/claude-code.md) |
| GitHub Copilot | [Copilot guide](docs/users/copilot.md) |

Capability comparison, day-1 checklist, and shared prompts: [User guides](docs/users/README.md).

## Quick start

1. Open the [guide for your host](docs/users/README.md#pick-your-host).
2. Install the plugin or adapter once.
3. Approve hooks when your host prompts for them.
4. Open the repository you want grounded and start a **fresh** agent session there.
5. Ask a repository question from your host guide.

The agent calls the matching CodeStory tool directly. CodeStory grounds the
checkout and prepares managed search automatically; you do not need CLI setup
commands for normal use.

**Something blocked?** [Troubleshooting](docs/users/troubleshooting.md).

## Example prompts

Use your project's symbols and paths:

**Find ownership**

```text
Where is [Feature] defined, who calls it, and which files should I read first?
```

**Plan a change**

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

**Understand a subsystem**

```text
How does [subsystem] work? Cite concrete files and flag gaps if coverage is incomplete.
```

More shapes and host-specific invocation: [User guides](docs/users/README.md#portable-prompt-shapes).

Surfaces, capability matrix, and readiness lanes: [User guides](docs/users/README.md).

## Documentation

| If you want to... | Read |
| --- | --- |
| Install and use CodeStory | [User guides](docs/users/README.md) |
| Know when to trust agent output | [Trust and readiness](docs/users/trust-and-readiness.md) |
| Repair a blocked session | [Troubleshooting](docs/users/troubleshooting.md) |
| Run CLI repair or debug | [CLI reference](docs/users/cli-reference.md) |
| Change CodeStory itself | [Contributor setup](docs/contributors/getting-started.md) |
| Verify a claim or PR | [Testing matrix](docs/contributors/testing-matrix.md) |

Full routing: [docs/README.md](docs/README.md).

## Evaluation

> **Scope:** The language-expansion holdout proves **token and wall-time reduction**
> on 18 pinned public OSS tasks when agents use CodeStory instead of re-reading
> the tree. It does **not** prove equal quality for every language, every repo
> size, or your private checkout. For day-to-day limits, see
> [What to expect](docs/users/what-to-expect.md).

### Language expansion holdout (18 tasks)

Broader public-repo evidence uses the
[`language-support-ab`](benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json)
manifest across 18 pinned OSS packages. Latest recorded suite totals:

| Metric | Without | With | Change |
| --- | ---: | ---: | --- |
| Context tokens | 9,692,559 | 5,514,580 | -43% |
| Repeat-task wall time | 7,943s | 4,343s | -45% |
| Tool calls | 475 | 60 | -87% |
| Direct source reads | 417 | 0 | -100% |

Per-task medians, ranges, reproduction commands, and boundary notes:
[language-expansion holdout stats](docs/testing/language-expansion-holdout-stats.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
