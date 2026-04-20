# CLI Subsystem

`codestory-cli` is the thin adapter for indexing, grounding reads, DB-first ask packets, graph-query helpers, local exploration, health checks, and lightweight serving.

## Ownership

- parse command-line arguments
- resolve project and cache paths
- call runtime services
- render text or JSON

## Entry Points

- `crates/codestory-cli/src/main.rs`
- `crates/codestory-cli/src/args.rs`
- `crates/codestory-cli/src/runtime.rs`
- `crates/codestory-cli/src/output.rs`

## Extension Points

- add commands in `args.rs` and `main.rs`
- add renderers in `output.rs`
- keep business logic in runtime, not here

## `index` Command Options

`codestory-cli index` is the cache-building command. It parses options in `crates/codestory-cli/src/args.rs`, delegates behavior to runtime, and renders the returned summary through `crates/codestory-cli/src/output.rs`.

| Option | Default | Runtime effect |
| --- | --- | --- |
| `--project <PROJECT>` | `.` | Selects the repository root. `--path` is accepted as an alias. Runtime opens this root, loads workspace configuration, and uses it to derive the default cache key. |
| `--cache-dir <CACHE_DIR>` | system cache root plus project hash | Overrides the cache root exactly. The SQLite store and persisted search directory live under this directory, which makes it useful for isolated repros and cold-start benchmarks. |
| `--refresh <auto\|full\|incremental\|none>` | `auto` | Selects the refresh strategy. `auto` resolves to `full` for an empty cache and `incremental` once indexed files already exist. |
| `--format <markdown\|json>` | `markdown` | Chooses human-readable output or machine-readable output. JSON includes the same retrieval metadata and phase timings. |
| `--output-file <PATH>` | stdout | Writes Markdown or JSON to a file whose parent directory already exists. |
| `--dry-run` | off | Computes discovery and refresh-plan counts without parsing files or writing storage. |
| `--summarize` | off | Generates cached one-sentence symbol summaries after indexing. |
| `--progress` | off | Prints progress updates to stderr while preserving stdout for structured output. |
| `--watch` | off | Runs once, then watches the project root and triggers incremental refreshes. `--output-file` is rejected when it points inside the watched project tree. |

Refresh behavior belongs to runtime, not the CLI adapter:

- `auto`: inspect stored inventory and choose `full` or `incremental`.
- `full`: build a staged database, run the indexer for all discovered files, finalize and publish snapshots, then synchronize semantic docs before returning.
- `incremental`: update the live database for changed/new/removed files, refresh live snapshots, and limit semantic invalidation to touched files.
- `none`: open the current cache and return a summary without graph or semantic indexing.

Semantic indexing is not a separate CLI flag. The default `index` path syncs semantic docs when embedding assets are available. Runtime-level environment variables control retrieval behavior and tuning, including `CODESTORY_HYBRID_RETRIEVAL_ENABLED`, `CODESTORY_SEMANTIC_DOC_SCOPE`, `CODESTORY_SEMANTIC_DOC_ALIAS_MODE`, `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE`, and `CODESTORY_EMBED_*`.

Index output should expose:

- project and storage paths
- resolved refresh mode
- graph stats and retrieval state
- graph phase timings
- semantic timings and doc counts when semantic sync was considered
- resolution diagnostics when the indexer reports them

## Read And Query Output

Read commands default to `--refresh none` so they query the current cache unless the caller asks for a refresh. `ground`, `search`, `symbol`, `trail`, `snippet`, `query`, and `explore` all support `--format markdown|json` and `--output-file <PATH>`; `trail` additionally supports Graphviz DOT via `--format dot`, while `symbol` and `trail` support Mermaid via `--mermaid`.

`query` is intentionally small. It parses source operations (`search`, `symbol`, `trail`) followed by stream refinements (`filter`, `limit`) and rejects malformed or unknown named arguments rather than silently ignoring typos.

`ask` is the first higher-level retrieval packet. It delegates to `codestory-runtime` agent orchestration, includes citations and retrieval traces, and defaults to DB-first synthesis without launching an external agent. `--with-local-agent` opts into the configured Codex or Claude command, and `--bundle <DIR>` writes Markdown, JSON, and Mermaid artifacts for handoff.

`doctor` is a read-only health report for project path resolution, cache presence, index counts, retrieval state, relevant embedding environment variables, and next commands. It should stay diagnostic; it should not mutate caches or fetch model assets.

## `search` And `ask` Research Options

`codestory-cli search` and `codestory-cli ask` keep production behavior on the runtime defaults unless a caller explicitly passes hybrid research weights:

| Option | Default | Runtime effect |
| --- | --- | --- |
| `--hybrid-lexical <WEIGHT>` | runtime default | Overrides the lexical component weight for this search request. |
| `--hybrid-semantic <WEIGHT>` | runtime default | Overrides the semantic embedding component weight for this search request. |
| `--hybrid-graph <WEIGHT>` | runtime default | Overrides the graph-neighborhood component weight for this search request. |
| `--why` | off | Search-only Markdown explanations for fallback state, origin, and lexical/semantic/graph score contributions when present. JSON includes the structured score breakdown when runtime has scored hybrid hits. |

The runtime clamps and normalizes supplied weights before ranking. These flags exist so benchmark runs can sweep retrieval settings without changing global environment variables or production defaults.

## Serving And Integration Surface

HTTP serving keeps the current small GET/query-string shape. The stable routes are `/health`, `/search`, `/symbol`, `/definition`, `/references`, `/symbols`, and `/trail`. Definition and references accept either `q` or `id`, so agents can resolve from a query first and then reuse exact node ids.

`serve --stdio` is MCP-style JSON lines. It exposes tools for search, symbol, trail, definition, references, symbols, snippet, and ask; resources for project, grounding, and root symbols; resource templates for node-specific symbol/reference/snippet/trail reads; and prompts for explain-symbol, callflow tracing, and impact analysis.

## Failure Signatures

- CLI depends directly on `codestory-store` or `codestory-indexer`
- output helpers start opening files or stores on their own
- command-specific orchestration is copied instead of delegated
