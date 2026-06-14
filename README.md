<h1 align="center">CodeStory</h1>

<p align="center">
Local codebase grounding for coding agents.
</p>

<p align="center">
<a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue"></a>
<a href="Cargo.toml"><img alt="Rust 2024" src="https://img.shields.io/badge/rust-2024-orange"></a>
<a href="docs/testing/benchmark-ledger.md"><img alt="Benchmarks" src="https://img.shields.io/badge/benchmarks-documented-blue"></a>
</p>

## Why CodeStory

Agents fail on real repos the same way humans do when they are new: they open the
wrong files, chase a plausible name, and call it done. CodeStory indexes the
repository first — who calls whom, where symbols live, the actual source — so
the next step is evidence, not guesswork.

It runs locally. Your code stays on your machine.

## How it works

**1. Index the repo.** CodeStory walks your tree (honoring normal ignore rules),
parses supported languages into symbols and edges — calls, imports, overrides —
and stores snippets plus a searchable graph in a per-project SQLite cache. One
full `index` up front; incremental refresh after edits.

**2. Find a foothold.** Ask where something lives: a handler name, a route, a
type, a string literal. `ground` summarizes a repo you have never seen; `search`
returns ranked candidates with file paths and graph ids.

**3. Follow the graph.** Pick one symbol. `trail` shows callers and callees;
`snippet` returns the surrounding source. You are walking relationships in the
index, not grepping random directories.

**4. Answer with proof.** `context` bundles trails, neighbors, and citations
around one target. For broad questions ("how does indexing persist state?"),
`packet` assembles a bounded evidence packet — but only when sidecar retrieval
is fully healthy (`retrieval_mode=full`).

**Embeddings are optional, not step one.** Most symbols are found through the
graph and lexical symbol docs. A separate `retrieval index` pass builds Zoekt,
Qdrant, and SCIP sidecars and embeds only policy-selected anchors (entry
points, public APIs, high-centrality nodes) when you need agent-grade
`packet`/`search`. Details: [docs/usage.md](docs/usage.md),
[docs/ops/retrieval-sidecars.md](docs/ops/retrieval-sidecars.md).

```mermaid
flowchart LR
    files[Source files] --> index[Parse into symbols and edges]
    index --> graph[(Local graph and snippets)]
    question[Question or symbol] --> graph
    graph --> match[Matching files and symbols]
    match --> walk[Caller and callee paths]
    walk --> source[Source at file and line]
    source --> cite[Answer with citations]
    graph --> sidecars[Optional sidecar indexes]
    sidecars --> cite
```

Example: *"Who calls `WorkspaceIndexer`?"* → search returns the symbol → trail
lists callers across crates → snippet shows the call sites → you edit with paths
already in hand.

More depth: [docs/concepts/how-codestory-works.md](docs/concepts/how-codestory-works.md).

## Try it

```sh
cargo build --release -p codestory-cli
export CODESTORY_CLI="./target/release/codestory-cli"
export TARGET_WORKSPACE="/path/to/repo"

"$CODESTORY_CLI" doctor --project "$TARGET_WORKSPACE"
"$CODESTORY_CLI" index --project "$TARGET_WORKSPACE" --refresh full
"$CODESTORY_CLI" ground --project "$TARGET_WORKSPACE" --why
"$CODESTORY_CLI" search --project "$TARGET_WORKSPACE" --query "WorkspaceIndexer" --why
```

On Windows use `.\target\release\codestory-cli.exe` and `$env:TARGET_WORKSPACE = "C:\path\to\repo"`.

That gets you a local graph you can browse with `trail`, `snippet`, `symbol`,
`explore`, `context`, and `report`. Add sidecars when you need `packet`; see
[docs/ops/retrieval-sidecars.md](docs/ops/retrieval-sidecars.md).

## Install as an agent skill

Copy [`.agents/skills/codestory-grounding`](.agents/skills/codestory-grounding) into
your agent skill directory and run `scripts/setup.sh` (or `setup.ps1` on
Windows). Skill source: [`.agents/skills/codestory-grounding/SKILL.md`](.agents/skills/codestory-grounding/SKILL.md).
The setup script prints `CODESTORY_CLI=` — point it at any workspace with
`--project`.

## Command cheat sheet

| When you need… | Command |
| --- | --- |
| Check cache health | `doctor --project <repo>` |
| Build or refresh the index | `index --project <repo> --refresh full` |
| Repo orientation | `ground --project <repo> --why` |
| Find a symbol or path | `search --project <repo> --query "…" --why` |
| Call graph around one symbol | `trail --project <repo> --id <node-id> --story` |
| Source around a symbol | `snippet --project <repo> --id <node-id>` |
| Deep bundle on one target | `context --project <repo> --id <node-id>` |
| Broad task question (sidecars required) | `packet --project <repo> --question "…"` |
| Warm agent read surface | `serve --project <repo> --stdio` |

Full operator guide: [docs/usage.md](docs/usage.md).

## Languages, evidence, contributing

Parser-backed graph indexing covers Python, Java, Rust, JavaScript,
TypeScript/TSX, C++, C, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, and Bash; HTML,
CSS, and SQL use structural collectors. Claim details:
[docs/architecture/language-support.md](docs/architecture/language-support.md).

Benchmark notes and caveats live in
[docs/testing/benchmark-ledger.md](docs/testing/benchmark-ledger.md). Timing
history: [docs/testing/codestory-e2e-stats-log.md](docs/testing/codestory-e2e-stats-log.md).

To hack on CodeStory: [docs/contributors/getting-started.md](docs/contributors/getting-started.md).
Architecture: [docs/architecture/overview.md](docs/architecture/overview.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
