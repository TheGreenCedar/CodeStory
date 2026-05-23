# CodeStory

Local codebase grounding for coding agents.

CodeStory turns a repository into a local SQLite graph, search index, semantic
docs, and evidence packets so coding agents can understand the codebase before
they start spending context on ad hoc file reads. The globally installed skill is
the intended front door: set up the CodeStory source/binary artifact once, then
use that skill to index explicit target workspaces and run grounded commands for
orientation, search, context, trails, snippets, and impact checks.

## Why CodeStory

Ungrounded agents burn time and context rediscovering the same code paths with
glob, grep, and file reads. CodeStory gives them a local, auditable grounding
loop instead:

```text
doctor -> index -> ground -> search -> symbol/trail/snippet/explore -> context
```

Everything stays local. The cache is SQLite plus local search/semantic state,
and command output carries freshness, fallback, citation, gap, and next-command
signals so the agent can tell evidence from guesswork.

## Benchmark Results

Current public evidence is CodeStory's local index/read latency and
retrieval-quality gates. Agent A/B savings are deliberately unpublished until
the controlled with/without-CodeStory harness produces measured wall-time,
token, cost, and tool-call rows.

| Lane | Current result | Why it matters |
| --- | ---: | --- |
| With/without-agent savings | Pending | Run the agent A/B harness before claiming token, cost, time, or tool-call savings |
| CodeStory repo cold index | `9.23s` | Full repo-scale gate on the Rust workspace with hash semantic mode |
| Warm stdio agent loop smoke | `53.50ms` per `search -> symbol -> trail -> snippet` loop | Persistent read surface stays fast once an index exists |
| Warm stdio search p95 smoke | `25.96ms` | Low-latency search budget with protocol-clean stdout |
| Historical cross-repo retrieval gate | Hit@10 `1.0`, MRR@10 `0.826831`, search p95 `84.7ms` | Expected anchors found across `4` projects and `225` queries |

See [benchmark results](docs/testing/benchmark-results.md),
[repo-scale E2E stats](docs/testing/codestory-e2e-stats-log.md), and
[stdio warm-loop stats](docs/testing/codestory-stdio-warm-loop-stats.md) for
methodology, raw-source links, commands, and caveats. Use
[`scripts/codestory-agent-ab-benchmark.mjs`](scripts/codestory-agent-ab-benchmark.mjs)
to generate publishable with/without-agent rows; until then, README claims stay
on measured local indexing, warm reads, protocol hygiene, and retrieval quality.

## Global Skill Setup

Use this path when the skill is the product surface and the CodeStory repository
is the backing artifact it sets up once.

1. Install the skill into your agent's global skill directory.
   ```powershell
   $SkillHome = "<agent-global-skill-directory>"
   New-Item -ItemType Directory -Force -Path $SkillHome | Out-Null
   Copy-Item -Recurse -Force .\.agents\skills\codestory-grounding "$SkillHome\codestory-grounding"
   ```
   The source skill package lives at
   [.agents/skills/codestory-grounding/SKILL.md](.agents/skills/codestory-grounding/SKILL.md).
2. Run the one-time setup script from the installed skill. The script clones or
   refreshes the CodeStory source artifact, builds `codestory-cli`, and prints
   the resolved executable path.
   ```powershell
   & "$SkillHome\codestory-grounding\scripts\setup.ps1"
   ```
   On Unix-like systems:
   ```sh
   sh "<agent-global-skill-directory>/codestory-grounding/scripts/setup.sh"
   ```
3. Optionally persist the printed CLI path for future global-skill runs.
   ```powershell
   setx CODESTORY_CLI "C:\Users\you\AppData\Local\CodeStory\bin\codestory-cli.exe"
   ```
4. If you need a different source artifact, set `CODESTORY_REPO_URL` and
   `CODESTORY_REPO_REF` explicitly before setup; otherwise setup uses the pinned
   `CODESTORY_REF` bundled with the skill.
5. Choose the target workspace explicitly.
   ```powershell
   $TargetWorkspace = "C:\path\to\repo"
   ```
6. Check health, build the target index, and gather orientation.
   ```powershell
   $CodeStoryCli = $env:CODESTORY_CLI
   & $CodeStoryCli doctor --project $TargetWorkspace
   & $CodeStoryCli index --project $TargetWorkspace --refresh full
   & $CodeStoryCli ground --project $TargetWorkspace --why
   ```
7. In future agent sessions, invoke the global `$codestory-grounding` skill and
   point it at the target workspace. The source checkout stays the tool artifact,
   not the assumed working directory.

## Agent Loop

| Need | Command |
| --- | --- |
| Health and cache readiness | `codestory-cli doctor --project <target-workspace>` |
| Broad orientation | `codestory-cli ground --project <target-workspace> --why` |
| Candidate discovery | `codestory-cli search --project <target-workspace> --query "<term>" --why` |
| Exact symbol evidence | `codestory-cli symbol --project <target-workspace> --id <node-id>` |
| Flow and dependency evidence | `codestory-cli trail --project <target-workspace> --id <node-id> --story --hide-speculative` |
| Source excerpt | `codestory-cli snippet --project <target-workspace> --id <node-id>` |
| Bundled navigation packet | `codestory-cli explore --project <target-workspace> --id <node-id> --no-tui` |
| Deep context bundle | `codestory-cli context --project <target-workspace> --id <node-id>` |
| Change impact before edits | `codestory-cli affected --project <target-workspace> --format markdown` |
| Persistent agent read surface | `codestory-cli serve --project <target-workspace> --stdio` |

Use `serve --stdio` when an agent needs repeated warm reads against an existing
index without paying one process startup per command.

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

## Use CodeStory From Source

Use this path if you are running the tool from this source checkout. For normal
agent work, keep the executable and target workspace separate.

1. Build the CLI.
   ```powershell
   cargo build --release -p codestory-cli
   $CodeStoryCli = ".\target\release\codestory-cli.exe"
   ```
2. Use the built binary from this repo checkout.
   ```powershell
   & $CodeStoryCli --help
   ```
3. Choose the repository you want to ground.
   ```powershell
   $TargetWorkspace = "C:\path\to\repo"
   ```
4. Create or refresh the local index.
   ```powershell
   & $CodeStoryCli index --project $TargetWorkspace --refresh auto
   ```
5. Run the CLI workflows against the existing cache.
   ```text
   codestory-cli ground --project <target-workspace> --why
   codestory-cli search --project <target-workspace> --query <query> --why
   codestory-cli files --project <target-workspace> --format markdown
   codestory-cli explore --project <target-workspace> --query <query> --no-tui
   codestory-cli context --project <target-workspace> --query AppController
   codestory-cli context --project <target-workspace> --id <node-id>
   codestory-cli affected --project <target-workspace> --format markdown
   codestory-cli symbol --project <target-workspace> (--id <node-id> | --query <query>)
   codestory-cli trail --project <target-workspace> (--id <node-id> | --query <query>)
   codestory-cli snippet --project <target-workspace> (--id <node-id> | --query <query>)
   codestory-cli query --project <target-workspace> "trail(symbol: 'Foo') | filter(kind: function)"
   codestory-cli doctor --project <target-workspace>
   ```

Read commands default to `--refresh none`. They query the current cache unless you explicitly request a refresh.

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

The product surface starts with core CLI grounding workflows and adds deeper context, file inventory, impact analysis, graph queries, explorer packets, health checks, and shell-integration commands:

```mermaid
flowchart LR
    Project["Repository"] --> Index["index"]
    Index["index"] --> LocalState["SQLite graph + snapshots"]
    LocalState --> Ground["ground"]
    LocalState --> Search["search"]
    LocalState --> Context["context"]
    LocalState --> Symbol["symbol"]
    LocalState --> Trail["trail"]
    LocalState --> Snippet["snippet"]
    LocalState --> Query["query"]
    LocalState --> Explore["explore"]
    LocalState --> Files["files"]
    LocalState --> Affected["affected"]
    LocalState --> Drill["drill"]
    LocalState --> Doctor["doctor"]
```

- `doctor`: read-only health check for project/cache/index/retrieval readiness.
- `index`: build or refresh the SQLite graph/search/semantic cache.
- `ground`: broad repo-level orientation snapshot; `--why` explains retrieval mode, coverage, gaps, and next commands.
- `search`: lightweight candidate discovery for symbols, files, literals, API paths, modules, and specific behavior terms; use `--why` for ranking reasons and `kind:`, `path:`, `name:`, or `lang:` filters for ambiguous result sets.
- `context`: deep evidence/context bundle for one concrete target selected by `--id`, `--query`, or `--bookmark`; it is not question answering, chatting, or prompt interpretation.
- `symbol`: inspect one exact symbol and relationships.
- `trail`: follow caller/callee/reference graph around a symbol; `--story --hide-speculative` gives a readable flow with uncertainty.
- `snippet`: fetch source context around a symbol; Markdown snippets use ANSI syntax highlighting when stdout is an interactive terminal.
- `query`: run structured graph-query pipelines such as `trail(symbol: 'Foo', depth: 2) | filter(kind: function) | limit(10)`.
- `explore`: interactive or bundled navigation view around a target; use `--no-tui` or `--format json` for stable agent output.
- `files`: inspect indexed file inventory, language counts, inferred roles, and framework route coverage notes.
- `affected`: map changed files to impacted symbols, route evidence when present, likely tests, blind spots, and next commands.
- `drill`: write a deterministic report bundle for selected anchors and an optional architecture question, including search/symbol/trail/explore/snippet artifacts, cross-anchor bridge evidence, and the CodeStory-only/source-truth answer-quality contract plus claim-ledger template; defaults to `--refresh full`.
- `bookmark`: save, list, or remove investigation focus nodes.
- `setup embeddings`: install and validate managed embedding assets.
- `generate-completions`: emit bash, zsh, fish, or PowerShell completions generated from the clap command model.

Use `ground --why` for broad orientation, `search --why` for candidate discovery, `explore` for a bundled navigation packet, and `context` when you already have one concrete target and want the deeper evidence bundle: target resolution metadata, symbol details, related hits, trail/story evidence, snippets/source context, retrieval/freshness health, citations/evidence ids, gaps/uncertainty, and optional bundle artifacts. Use `files` before making coverage claims, and use `affected` before choosing focused regression checks.

Do not pass broad natural-language questions to `context`. For broad repo/product questions, use `ground --why`, run one or more concrete `search --repo-text on --why` queries, select anchors, then run `context --id <node-id>` for each anchor.

Hybrid retrieval is the intended default when local embedding assets are available. `index`, `ground`, `search`, `context`, and `doctor` now report retrieval mode, semantic doc counts, and explicit fallback reasons when the runtime drops back to symbolic ranking.

Search accepts field-qualified filters when you already know part of the target: `kind:function name:listUsers`, `path:routes.ts /api/users`, and `lang:typescript /api/users` keep the original query in output while using the unqualified terms for ranking.

## Template Workflows

Fresh repo orientation:

```powershell
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli ground --project <target-workspace> --why
codestory-cli search --project <target-workspace> --query "<architecture term>" --why
codestory-cli files --project <target-workspace> --format markdown
```

Candidate-to-context workflow:

```powershell
codestory-cli search --project <target-workspace> --query "<symbol/file/literal/API path>" --why
# choose a concrete node_id
codestory-cli explore --project <target-workspace> --id <node-id> --no-tui
codestory-cli context --project <target-workspace> --id <node-id>
```

Exact symbol investigation:

```powershell
codestory-cli symbol --project <target-workspace> --id <node-id>
codestory-cli explore --project <target-workspace> --id <node-id> --no-tui
codestory-cli trail --project <target-workspace> --id <node-id> --story --hide-speculative
codestory-cli snippet --project <target-workspace> --id <node-id> --context 40
codestory-cli context --project <target-workspace> --id <node-id> --bundle out/context-<name>
```

Changed-file impact workflow:

```powershell
codestory-cli index --project <target-workspace> --refresh incremental
codestory-cli affected --project <target-workspace> --format markdown
git diff --name-only HEAD | codestory-cli affected --project <target-workspace> --stdin --format json
```

Route coverage workflow:

```powershell
codestory-cli files --project <target-workspace> --format json
cargo test -p codestory-indexer --lib framework_route
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
```

Evaluation workflow:

```powershell
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-runtime --test retrieval_eval
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Broad repo/product question workflow:

```powershell
# do not pass the question to context
codestory-cli ground --project <target-workspace> --why
codestory-cli search --project <target-workspace> --repo-text on --query "<concrete term>" --why
codestory-cli search --project <target-workspace> --repo-text on --query "<another concrete term>" --why
# select anchors
codestory-cli context --project <target-workspace> --id <node-id>
```

Stale or unhealthy semantic retrieval:

```powershell
codestory-cli doctor --project <target-workspace>
codestory-cli setup embeddings --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
```

If retrieval is still partial, stale, or failed, use `search --repo-text on --why`, `symbol`, `trail`, and `snippet`; treat `context` output as incomplete when it reports gaps.

Status words in CLI/docs output are deliberately conservative:

- `supported`: fixture-backed behavior is passing and the documented coverage floor is met.
- `heuristic`: evidence came from a pattern or convention that needs source review before a full support claim.
- `partial`: some cases are covered, but known patterns, handler links, languages, or fixtures are missing.
- `unsupported`: no support claim is made for that syntax, framework, language, or path.
- `stale`: the cache or semantic evidence may not match the current workspace; run `doctor` or `index --refresh full`.
- `non-promotable`: required fixtures, coverage notes, or validation evidence are missing or failing.
- `ambiguous`: a query matched multiple plausible targets; rerun `search --why`, then use `--id` or `--file`.
- `unmatched`: a changed path was not found in the persisted index; confirm the path with `files --path <fragment>` or refresh the index.

## Workspace And Config Files

CodeStory supports an optional `codestory_workspace.json` file at the repo root for monorepo-style sessions:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

When the manifest is present, `index --project <target-workspace>` discovers all listed member roots and reports per-member refresh counts in index output. Repos without the manifest keep the single-root behavior. OpenAPI JSON/YAML schemas are treated as lightweight endpoint sources, and literal client calls such as `fetch("/api/users")` or `axios.post("/api/users")` create speculative graph edges to matching endpoint refs.

Team or user defaults can live in `.codestory.toml` at the project root or in the user home directory. CodeStory loads the home file first, then the project file, so project settings override home settings. Explicit environment variables still win over config defaults.

Supported keys include `cache_dir`, `embedding_profile`, `embedding_model_id`, `hybrid_retrieval_enabled`, `semantic_doc_scope`, `semantic_doc_alias_mode`, `summary_endpoint`, and `summary_model`. The legacy `embedding_model` key is still accepted as a deprecated alias for `embedding_model_id`.

Example:

```toml
embedding_profile = "bge-base-en-v1.5"
embedding_model_id = "BAAI/bge-base-en-v1.5-local"
hybrid_retrieval_enabled = true
semantic_doc_scope = "durable"
```

`embedding_profile` maps to `CODESTORY_EMBED_PROFILE`, and `embedding_model_id` maps to `CODESTORY_EMBED_MODEL_ID`. If those environment variables are already set before the CLI starts, the CLI leaves them unchanged.

## Retrieval Defaults

`index`, `ground`, `search`, `context`, and `doctor` report the active retrieval mode when they have retrieval state available. Hybrid retrieval is the default when local embedding assets are available; otherwise CodeStory falls back to symbolic or lexical ranking and reports why.

The default `index` path is a full semantic sync, not a deferred background task. When embedding assets are available, the command returns after graph state, snapshots, lexical search state, and durable semantic docs are all ready. The index summary reports semantic timing and reuse counts so cold-start and repeated-refresh costs stay visible.

Hybrid retrieval setup:

- managed real-model setup: run `codestory-cli setup embeddings --project <target-workspace>` to download the pinned Qdrant BGE-base ONNX graph plus tokenizer files into the user cache. Setup derives `model_optimized_cls_pool.onnx` from the downloaded graph so runtime receives `sentence_embedding` directly instead of the full token hidden state. The CLI seeds the managed local defaults of semantic doc window `512`, doc batch `2048`, ONNX provider `directml` on Windows or `cpu` elsewhere, ONNX per-call token budget `32768`, and in-memory stored vectors `int8` unless explicit environment variables override them.
- fast local-dev semantic mode: set `CODESTORY_EMBED_RUNTIME_MODE=hash`
- backend and profile selection: set `CODESTORY_EMBED_BACKEND=onnx`, `llamacpp`, or `hash`; default profile is `bge-base-en-v1.5`; explicit profiles include `minilm`, `bge-small-en-v1.5`, `bge-base-en-v1.5`, `qwen3-embedding-0.6b`, `embeddinggemma-300m`, `nomic-embed-text-v1.5`, `nomic-embed-text-v2-moe`, or `custom`
- managed ONNX paths: `setup embeddings` sets `CODESTORY_EMBED_ONNX_MODEL`, `CODESTORY_EMBED_ONNX_TOKENIZER`, `CODESTORY_EMBED_ONNX_PROVIDER`, and `CODESTORY_EMBED_ONNX_BATCH_TOKENS`; set them manually only for custom ONNX assets or profiling
- external legacy llama.cpp GGUF server: run `llama-server --embedding` yourself, set `CODESTORY_EMBED_BACKEND=llamacpp`, and set `CODESTORY_EMBED_LLAMACPP_URL` if it is not listening at `http://127.0.0.1:8080/v1/embeddings`; tune concurrent embedding requests with `CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT`
- durable semantic docs are the default; set `CODESTORY_SEMANTIC_DOC_SCOPE=all` to include lower-signal local/member/module symbols for investigation
- embedding batch size defaults to `128` for unmanaged runtimes and `2048` for the managed ONNX path; override with `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE` only while profiling
- search and context research can override hybrid ranking weights with hidden `--hybrid-lexical <WEIGHT> --hybrid-semantic <WEIGHT> --hybrid-graph <WEIGHT>` tuning flags; omit these flags for the runtime defaults
- context bundles: `context --bundle <DIR>` writes `context.md`, `context.json`, generated graph artifacts, and a bundle manifest for sharing or review
- lexical-only mode: set `CODESTORY_HYBRID_RETRIEVAL_ENABLED=false`
- verification: `index`, `ground`, `search`, `context`, and `doctor` will report the retrieval mode plus any fallback reason when relevant

Measured backend tradeoffs and current model recommendations are summarized in
the [research handbook](docs/research.md), with the decision matrix in
[embedding-backend-benchmarks.md](docs/testing/embedding-backend-benchmarks.md).

Refresh behavior:

- `index --refresh auto`: full on an empty cache, incremental once indexed files already exist
- `ground`, `search`, `context`, `symbol`, `trail`, `snippet`, `query`, `explore`, `files`, and `affected`: default to `--refresh none`
- `drill`: defaults to `--refresh full` so each report is mechanically fresh; use `--refresh none` only after a fresh index
- use `--refresh incremental` when you want a read command to refresh an existing cache first
- use `--refresh full` after a cache reset, schema change, or suspected stale-state incident

## Cache Hygiene

By default, `codestory-cli` stores per-project caches under the user cache root using a hash of the project path. If you pass `--cache-dir`, that directory is used exactly as written.

Typical recovery flow:

```powershell
codestory-cli index --project <target-workspace> --refresh full
codestory-cli search --project <target-workspace> --query WorkspaceIndexer
```

If the cache itself is suspect, remove the project cache directory and rebuild:

```powershell
Remove-Item -LiteralPath <cache-dir> -Recurse -Force
codestory-cli index --project <target-workspace> --refresh full
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
- `codestory-cli`: thin adapter and renderer for grounding, context packets, navigation, health, and serving workflows
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

Navigation and quality gates:

```powershell
cargo test -p codestory-indexer --lib framework_route
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-runtime --test retrieval_eval
```

Performance branches must capture a baseline before tuning and record the comparison in the testing docs. Use [performance-review-playbook.md](docs/testing/performance-review-playbook.md) for the required baseline fields, parallelization candidate gate, and rejection rules.

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
