# CodeStory

CodeStory is a local codebase grounding engine. It indexes a repository into a SQLite-backed graph, keeps grounding-oriented read models up to date, and exposes grounding, graph-query, visualization, and local-serving workflows through `codestory-cli`.

## System Map

```mermaid
flowchart LR
    User["Human or tool"] --> CLI["codestory-cli"]
    CLI["codestory-cli"] --> Runtime["codestory-runtime"]
    Runtime --> Workspace["codestory-workspace"]
    Runtime --> Indexer["codestory-indexer"]
    Runtime --> Store["codestory-store"]
    Indexer --> Store
```

## Use CodeStory

Use this path if you want to run the tool against a repository.

1. Build the CLI.
   ```powershell
   cargo build --release -p codestory-cli
   ```
2. Use the built binary from this repo checkout.
   ```powershell
   .\target\release\codestory-cli.exe --help
   ```
3. Create or refresh the local index.
   ```powershell
   .\target\release\codestory-cli.exe index --project . --refresh auto
   ```
4. Run the grounding workflows against the existing cache.
   ```text
   .\target\release\codestory-cli.exe ground --project <path>
   .\target\release\codestory-cli.exe search --project <path> --query <query> --why
   .\target\release\codestory-cli.exe ask --project <path> "How does this repo fit together?"
   .\target\release\codestory-cli.exe symbol --project <path> (--id <node-id> | --query <query>)
   .\target\release\codestory-cli.exe trail --project <path> (--id <node-id> | --query <query>)
   .\target\release\codestory-cli.exe snippet --project <path> (--id <node-id> | --query <query>)
   .\target\release\codestory-cli.exe query --project <path> "trail(symbol: 'Foo') | filter(kind: function)"
   .\target\release\codestory-cli.exe explore --project <path> --query <query>
   .\target\release\codestory-cli.exe serve --project <path>
   .\target\release\codestory-cli.exe doctor --project <path>
   ```

Read commands default to `--refresh none`. They query the current cache unless you explicitly ask them to refresh.

### `index` Options

`codestory-cli index` accepts these CLI options:

| Option | Default | How it works |
| --- | --- | --- |
| `--project <PROJECT>` | `.` | Repository root to index. `--path` is an alias. Paths are resolved before CodeStory chooses the cache key. |
| `--cache-dir <CACHE_DIR>` | user cache root plus a per-project hash | Uses the exact directory passed for the SQLite database and sibling search directory. Use this for temp-cache benchmarks or isolated repros. |
| `--refresh <auto\|full\|incremental\|none>` | `auto` | Controls whether indexing work runs before the summary is returned. See the refresh table below. |
| `--format <markdown\|json>` | `markdown` | Markdown is for humans. JSON exposes the same summary, retrieval state, and phase timings for tests and automation. |
| `--dry-run` | off | Computes the refresh plan and reports files that would be indexed or removed without parsing, resolving, or writing storage. |
| `--summarize` | off | After indexing, generates cached one-sentence symbol summaries. Requires `CODESTORY_SUMMARY_ENDPOINT`, unless set to `local` or `mock` for deterministic local summaries. |
| `--progress` | off | Prints an incremental text progress bar to stderr so stdout stays parseable. |
| `--watch` | off | Keeps running after the first index and triggers incremental refreshes when files change. |

Refresh modes:

| Mode | Behavior |
| --- | --- |
| `auto` | Chooses `full` when the cache has no indexed files; chooses `incremental` once stored inventory exists. |
| `full` | Builds a staged SQLite database from the full workspace, copies reusable semantic docs forward from the previous live DB when present, finalizes snapshots, publishes the staged DB, and syncs semantic docs before returning. |
| `incremental` | Opens the live DB, asks `codestory-workspace` for changed/new/removed files, reindexes only that refresh scope, refreshes live snapshots, and rebuilds semantic docs only for touched files. |
| `none` | Opens the existing cache and returns a summary without running graph or semantic indexing. This is mainly for inspecting a known-good cache. |

There is intentionally no `index --semantic off` option in the current CLI. Default `index` completes semantic docs when embedding assets are available. Semantic behavior is controlled by retrieval environment settings such as `CODESTORY_HYBRID_RETRIEVAL_ENABLED=false`, `CODESTORY_SEMANTIC_DOC_SCOPE=all`, and the `CODESTORY_EMBED_*` variables documented below.

If you are using an agent in this repo, point it at the available `codestory-grounding` skill in `.agents/skills/codestory-grounding/SKILL.md` so it can use the indexed grounding workflows directly.

Start here when you are using the tool:

- [Runtime execution path](docs/architecture/runtime-execution-path.md)
- [CLI subsystem](docs/architecture/subsystems/cli.md)
- [Glossary](docs/glossary.md)

## Hack on CodeStory

Use this path if you want to change the codebase.

1. Read the architecture overview, runtime execution path, and indexing pipeline before you jump into crate-specific details.
2. Run Cargo verification serially because the workspace shares build locks.
3. Make changes in the owning crate instead of threading behavior through the CLI.
4. Use the contributor docs as a short path through architecture, debugging, and test coverage.

Start here when you are contributing:

- [Architecture overview](docs/architecture/overview.md)
- [Contributor setup](docs/contributors/getting-started.md)
- [Research handbook](docs/research.md)
- [Indexing pipeline](docs/architecture/indexing-pipeline.md)
- [Debugging guide](docs/contributors/debugging.md)
- [Testing matrix](docs/contributors/testing-matrix.md)
- [Architecture history](docs/decision-log.md)
- [Contracts subsystem](docs/architecture/subsystems/contracts.md)
- [Workspace subsystem](docs/architecture/subsystems/workspace.md)
- [Indexer subsystem](docs/architecture/subsystems/indexer.md)
- [Store subsystem](docs/architecture/subsystems/store.md)
- [Runtime subsystem](docs/architecture/subsystems/runtime.md)
- [CLI subsystem](docs/architecture/subsystems/cli.md)

## Grounding Workflows

The product surface starts with core grounding workflows and adds higher-level ask, graph, explorer, serving, health, and shell-integration commands:

```mermaid
flowchart LR
    Project["Repository"] --> Index["index"]
    Index["index"] --> LocalState["SQLite graph + snapshots"]
    LocalState --> Ground["ground"]
    LocalState --> Search["search"]
    LocalState --> Ask["ask"]
    LocalState --> Symbol["symbol"]
    LocalState --> Trail["trail"]
    LocalState --> Snippet["snippet"]
    LocalState --> Query["query"]
    LocalState --> Explore["explore"]
    LocalState --> Serve["serve"]
    LocalState --> Doctor["doctor"]
```

- `index`: discover files, parse supported languages, resolve graph edges, persist search projections, and complete semantic docs before returning
- `ground`: build grounded context from indexed symbols, snippets, graph traversal, and search results; `--why` explains retrieval mode, coverage, and query hints
- `search`: find symbols, files, and query matches; semantic-only near misses appear under `did_you_mean`, and `--why` includes lexical/semantic/graph score breakdowns when available
- `ask`: run DB-first agentic retrieval across search, graph, snippets, traces, and citations; by default it does not launch an external agent, while `--with-local-agent` opts into local Codex/Claude synthesis
- `symbol`: inspect one symbol and its indexed relationships
- `trail`: walk caller/callee and usage neighborhoods through the graph; `--mermaid` emits a Mermaid flowchart and `--format dot` emits Graphviz DOT
- `snippet`: fetch focused source context for a symbol or file location; Markdown snippets use ANSI syntax highlighting when stdout is an interactive terminal
- `query`: run a small graph query pipeline such as `trail(symbol: 'Foo', depth: 2) | filter(kind: function) | limit(10)`
- `explore`: open an interactive terminal explorer when stdout is a terminal, or emit Markdown/JSON with definition and reference navigation metadata when piped or passed `--no-tui`
- `serve`: expose local `/health`, `/search`, `/symbol`, `/definition`, `/references`, `/symbols`, and `/trail` JSON endpoints, or use `--stdio` for MCP-style tools, resources, resource templates, and prompts
- `doctor`: report project/cache/index/retrieval health, relevant environment settings, and the next useful commands for the workspace
- `generate-completions`: emit bash, zsh, fish, or PowerShell completions generated from the clap command model

Hybrid retrieval is the intended default when local embedding assets are available. `index`, `ground`, `search`, `ask`, and `doctor` now report retrieval mode, semantic doc counts, and explicit fallback reasons when the runtime drops back to symbolic ranking.

## Workspace And Config Files

CodeStory supports an optional `codestory_workspace.json` file at the repo root for monorepo-style sessions:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

When the manifest is present, `index --project .` discovers all listed member roots and reports per-member refresh counts in index output. Repos without the manifest keep the single-root behavior. OpenAPI JSON/YAML schemas are treated as lightweight endpoint sources, and literal client calls such as `fetch("/api/users")` or `axios.post("/api/users")` create speculative graph edges to matching endpoint refs.

Team or user defaults can live in `.codestory.toml` at the project root or in the user home directory. Project settings override home settings, and explicit environment variables still win. Supported keys include `cache_dir`, `embedding_model`, `hybrid_retrieval_enabled`, `semantic_doc_scope`, `semantic_doc_alias_mode`, `summary_endpoint`, and `summary_model`.

## Retrieval Defaults

`index`, `ground`, `search`, `ask`, and `doctor` report the active retrieval mode when they have retrieval state available. Hybrid retrieval is the default when local embedding assets are available; otherwise CodeStory falls back to symbolic or lexical ranking and reports why.

The default `index` path is a full semantic sync, not a deferred background task. When embedding assets are available, the command returns after graph state, snapshots, lexical search state, and durable semantic docs are all ready. The index summary reports semantic timing and reuse counts so cold-start and repeated-refresh costs stay visible.

Hybrid retrieval setup:

- fast local-dev semantic mode: set `CODESTORY_EMBED_RUNTIME_MODE=hash`
- backend and profile selection: set `CODESTORY_EMBED_BACKEND=llamacpp` or `hash`; default profile is `bge-base-en-v1.5`; explicit profiles include `minilm`, `bge-small-en-v1.5`, `bge-base-en-v1.5`, `qwen3-embedding-0.6b`, `embeddinggemma-300m`, `nomic-embed-text-v1.5`, `nomic-embed-text-v2-moe`, or `custom`
- llama.cpp GGUF server: run `llama-server --embedding` and set `CODESTORY_EMBED_LLAMACPP_URL` if it is not listening at `http://127.0.0.1:8080/v1/embeddings`; tune concurrent embedding requests with `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT`
- durable semantic docs are the default; set `CODESTORY_SEMANTIC_DOC_SCOPE=all` to include lower-signal local/member/module symbols for investigation
- embedding batch size defaults to `128`; override with `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE` only while profiling
- search and ask research can override hybrid ranking weights with `--hybrid-lexical <WEIGHT> --hybrid-semantic <WEIGHT> --hybrid-graph <WEIGHT>`; omit these flags for the runtime defaults
- handoff bundles: `ask --bundle <DIR>` writes `answer.md`, `answer.json`, and generated Mermaid artifacts for sharing or review
- lexical-only mode: set `CODESTORY_HYBRID_RETRIEVAL_ENABLED=false`
- verification: `index`, `ground`, `search`, `ask`, and `doctor` will report the retrieval mode plus any fallback reason when relevant

Measured backend tradeoffs and current model recommendations are summarized in
the [research handbook](docs/research.md), with the decision matrix in
[embedding-backend-benchmarks.md](docs/testing/embedding-backend-benchmarks.md).

Refresh behavior:

- `index --refresh auto`: full on an empty cache, incremental once indexed files already exist
- `ground`, `search`, `ask`, `symbol`, `trail`, `snippet`, `query`, `explore`, and `serve`: default to `--refresh none`
- use `--refresh incremental` when you want a read command to refresh an existing cache first
- use `--refresh full` after a cache reset, schema change, or suspected stale-state incident

## Cache Hygiene

By default, `codestory-cli` stores per-project caches under the user cache root using a hash of the project path. If you pass `--cache-dir`, that directory is used exactly as written.

Typical recovery flow:

```powershell
.\target\release\codestory-cli.exe index --project . --refresh full
.\target\release\codestory-cli.exe search --project . --query WorkspaceIndexer
```

If the cache itself is suspect, remove the project cache directory and rebuild:

```powershell
Remove-Item -LiteralPath <cache-dir> -Recurse -Force
.\target\release\codestory-cli.exe index --project . --refresh full
```

Low-memory guidance:

- prefer `index --refresh incremental` over repeated full refreshes
- avoid running multiple cargo commands at once in this repo
- if semantic retrieval assets are unavailable or too heavy for the current machine, symbolic retrieval remains supported and is reported explicitly
- if a cold index is slow, inspect `semantic_ms` and `semantic_docs` in the index output before changing parser or graph code
- if the repo-scale runtime integration gate exceeds local memory, stop there and fall back to the smaller runtime lanes before escalating to a larger machine

## Workspace Shape

The workspace is organized into seven durable crates:

- `codestory-contracts`: shared graph, API, grounding, trail, and event types
- `codestory-workspace`: manifest loading, file discovery, and refresh-plan computation
- `codestory-store`: SQLite persistence, snapshots, trails, bookmarks, and search docs
- `codestory-indexer`: parsing, extraction, resolution, batching, and indexing tests
- `codestory-runtime`: orchestration, grounding, search, trail, and agent flows
- `codestory-cli`: thin adapter and renderer for grounding, ask, navigation, health, and serving workflows
- `codestory-bench`: criterion benches for indexing, grounding, resolution, and cleanup work

## Build And Verification

Run Cargo commands serially in this repo:

```powershell
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Release-blocking fidelity suites:

```powershell
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime --test retrieval_eval
```

Runtime-backed CLI fixture flows are an explicit heavier lane now:

```powershell
cargo test -p codestory-cli --test runtime_backed_flows -- --ignored
```

The repo-scale runtime integration smoke test is ignored by default because it indexes the full
`codestory` workspace and can exhaust memory. Run it only as an explicit heavy lane:

```powershell
$env:CODESTORY_RUN_REPO_SCALE_TEST = "1"
cargo test -p codestory-runtime --test integration test_repo_scale_call_resolution -- --ignored --nocapture
```

## Runtime Artifacts

CodeStory writes user-cache SQLite indexes keyed by the target project path. Build outputs live under `target/`.

## License

Apache-2.0. See `LICENSE`.
