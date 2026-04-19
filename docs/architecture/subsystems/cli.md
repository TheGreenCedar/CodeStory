# CLI Subsystem

`codestory-cli` is the thin adapter for the six grounding workflows.

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

## Failure Signatures

- CLI depends directly on `codestory-store` or `codestory-indexer`
- output helpers start opening files or stores on their own
- command-specific orchestration is copied instead of delegated
