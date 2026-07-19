# Indexing Pipeline

This page explains how `codestory-cli index` turns a repository into SQLite-backed graph state, projection rows, and grounding snapshots.

Read this page when you need the implementation mental model. For live evidence
from an indexed workspace, use contributor CLI commands in
[getting-started.md](../contributors/getting-started.md) or operator repair
paths in [users/troubleshooting.md](../users/troubleshooting.md).

Default `index` includes graph-native symbol docs and selected dense anchors. A
successful run returns only after graph indexing, snapshots, lexical search
projection, deterministic `symbol_search_doc` rows, component reports, and
persisted dense-anchor inputs are synchronized. This is the core publication
contract. Immutable retrieval generation finalization is a separate fence.

## End-To-End Command Path

```mermaid
sequenceDiagram
    participant CLI as codestory-cli
    participant Runtime as codestory-runtime
    participant Workspace as codestory-workspace
    participant Indexer as codestory-indexer
    participant Store as codestory-store
    participant Search as runtime search

    CLI->>Runtime: parse `index` command and open project
    Runtime->>Workspace: full refresh plan or diff-based refresh plan
    Workspace-->>Runtime: RefreshExecutionPlan + verified policy exclusions
    Runtime->>Store: open staged store for full refresh or live store for incremental
    Runtime->>Indexer: WorkspaceIndexer::run(plan, store)
    Indexer->>Store: flush files, nodes, edges, occurrences, component access, callable projection state
    Indexer->>Store: run post-flush resolution updates
    Runtime->>Store: finalize staged snapshot or refresh live snapshots
    Runtime->>Search: rebuild lexical projection, symbol docs, component reports, and dense anchors
    Search->>Store: reuse unchanged anchor metadata or upsert selected embedding-free inputs
    Runtime->>Store: revalidate and publish the bound exclusion manifest at the identity fence
    Runtime-->>CLI: indexing summary and phase timings
```

## Who Owns What

- `codestory-cli` parses the command and renders the indexing summary.
- `codestory-runtime` chooses full versus incremental flow and staged versus live store behavior.
- `codestory-workspace` discovers source files and computes the refresh plan.
- `codestory-indexer` turns the plan into projection writes and post-flush resolution.
- `codestory-store` persists rows, invalidates or refreshes snapshots, publishes staged builds, and stores symbol docs plus embedding-free dense-anchor inputs.
- `codestory-runtime` owns the runtime search engine, deterministic symbol/dense input construction, retrieval readiness, and timing surface.
- `codestory-retrieval` owns model execution, immutable vector generations, producer evidence, validation, and query generation leases.

That split is intentional: the runtime orchestrates the run, the indexer performs indexing work, and the store owns persistence mechanics.

## Two publication fences

```mermaid
flowchart LR
    Files["source inventory"] --> CoreBuild["index and resolve graph"]
    CoreBuild --> Core["publish codestory.db"]
    Core --> Inputs["symbol docs, reports, reusable dense inputs"]
    Files --> RetrievalBuild["build immutable retrieval generation"]
    Inputs --> RetrievalBuild
    Engine["per-user embedding server"] --> RetrievalBuild
    RetrievalBuild --> Retrieval["publish lexical + vectors + SCIP + manifest"]
```

Core indexing and retrieval publication deliberately do not share one commit:

1. runtime and store publish a coherent core graph generation;
2. runtime synchronizes deterministic search documents and reusable dense inputs
   in that core generation;
3. a broad activating call or explicit retrieval-index operation builds and
   validates immutable `lexical-index.sqlite3`, `vectors.sqlite3`, and SCIP
   artifacts;
4. retrieval publishes a manifest that binds those artifacts to the exact core
   generation/run, source input, and embedding producer.

Local graph navigation can use a current core publication while retrieval is
still preparing. Packet/search requires both publications plus a live
policy-compliant engine. See [retrieval design](retrieval-design.md).

## Indexer Phases

```mermaid
flowchart TD
    plan["Refresh plan from codestory-workspace"] --> prep["Normalize paths and load compile_commands metadata"]
    prep --> supported{"Parser-backed or structural support path?"}
    supported -->|"No"| skip["Skip file with no parse work"]
    supported -->|"Yes"| cache{"Artifact cache hit?"}
    cache -->|"Yes"| reuse["Reuse cached intermediate artifacts or refresh file metadata"]
    cache -->|"No"| parse["Parse and extract file in parallel"]
    reuse --> merge["Merge into IntermediateStorage batch"]
    parse --> merge
    merge --> flush{"Batch threshold reached?"}
    flush -->|"Yes"| write["Flush files, nodes, edges, occurrences, component access, callable projection state"]
    flush -->|"No"| more["Continue collecting file results"]
    more --> prep
    write --> prep
    prep --> done["All files prepared"]
    done --> finalflush["Flush remaining batch"]
    finalflush --> resolve["Run ResolutionPass on stored unresolved edges"]
    resolve --> errors["Flush indexing errors"]
    errors --> cleanup["Incremental cleanup for removed files"]
    cleanup --> snapshots["Runtime refreshes or publishes snapshots"]
    snapshots --> semantic["Runtime syncs lexical search projection, symbol docs, component reports, and dense anchors"]
    semantic --> summary["CLI receives retrieval state and phase timings"]
```

## Step By Step

### 1. CLI dispatches the `index` workflow

`crates/codestory-cli/src/main.rs` routes `Command::Index` into `run_index`. The CLI does not index files directly. It builds a runtime context, asks runtime to open the project with the requested refresh mode, and then renders the returned summary.

### 2. Runtime chooses full or incremental indexing

`crates/codestory-runtime/src/lib.rs` owns the orchestration split:

- `index_full` opens a staged store with `SnapshotStore::open_staged`, asks the workspace for a full refresh plan, runs the indexer against the staged store, finalizes the staged snapshot, and then publishes it to the live path
- `index_incremental` opens the live store, collects refresh inputs from stored inventory, builds a diff-based execution plan, runs the same indexer against the live store, and then refreshes live summary and detail snapshots

The indexer does not know whether the store is staged or live.

### 3. Workspace computes the refresh plan

`crates/codestory-workspace/src/lib.rs` decides which files belong in the run:

- `source_files` walks the configured source groups from the workspace manifest, follows directories, applies exclude globs, sorts the result, and removes duplicates
- `source_inventory` retains whether that walk was complete, partial, unreadable, or stopped by its candidate bound, plus any traversal failures
- `source_inventory_with_oversized_policy` hashes stable source bytes above the
  shared parser cap and classifies them before parser scheduling, independent
  of whether they are generated, vendored, or ordinary project files
- `build_refresh_plan` compares discovered files against stored inventory and only plans stored-file deletion from a complete inventory. Stored parser-backed rows carry the verified SHA-256 identity of the bytes that produced their projection, so matching millisecond mtimes do not hide changed content

For incremental work, a file is reindexed when:

- it is new
- its modification time differs from the stored row
- its modification time matches but its verified parser content hash differs
- it exists in the store but is marked as not indexed

Legacy rows and collectors without a verified parser hash retain the metadata
fallback. Content comparison reads only rows that already carry a verified
source hash; it does not add hashing for structural, text-only, or oversized
files that did not produce one.

Verified oversized candidates are removed from parser scheduling and published
as a complete exclusion manifest bound to the project, workspace, candidate
core generation/run, policy version, byte cap, row count, and digest. The rows
remain source inventory only: they make no parser graph or semantic coverage
claim. A content or policy change forces reclassification.

Files that disappeared from a complete discovery are collected into
`files_to_remove`. Partial, unreadable, bounded, or concurrently changing
inventories retain observed files for safe reindexing but never infer deletion
or a complete exclusion set from absence. Runtime treats them as source
coverage blockers and preserves the previous complete publication.

### 4. The indexer prepares file work

`WorkspaceIndexer::run` in `crates/codestory-indexer/src/lib.rs` starts by preparing state for the whole run:

- it seeds the symbol table from existing stored node kinds for incremental runs
- it chunks `files_to_index` using batch settings
- it loads parsed compilation metadata from `compile_commands.json` when available
- it picks a parser-backed language configuration or structural collector for
  each file and skips unsupported files before any parse work

Compilation metadata matters mostly for native-language parsing and is part of the artifact-cache key, so changes to compiler flags or include paths can invalidate cached artifacts. Artifact-cache identity uses the root-relative file path so compatible clean worktrees can reuse copied artifact rows.

### 5. Artifact cache decides parse versus reuse

`prepare_index_work` checks the index artifact cache before reparsing a file.

The cache key includes:

- the root-relative file path
- file bytes
- language queries
- feature-flag values that affect graph shape
- compilation metadata when present and root-relative enough to be portable

A cache hit can reuse the serialized indexing artifact and turn it back into
`IntermediateStorage`. A cache miss sends the file through parse and extract
work. Before either result is accepted, the indexer re-reads the source and
compares its SHA-256 hash with the bytes used to build the cache key or parser
input. A mismatch, including one hidden by a restored timestamp, becomes an
incomplete file-level retry error; stale graph output and artifact-cache writes
are discarded.

File diagnostics retain a typed coverage reason: `parser_partial`,
`source_changed`, `unreadable`, `oversized`, `discovery_incomplete`, or
`collector_failure`. Each entry reports whether the condition is retryable and
whether verified source and projection data exist. A full candidate may publish
a stable parser-partial projection; the other reasons remain source-integrity
gaps and preserve the previous complete publication.

### 6. Parse and extract run in parallel

Cache misses become `PreparedIndexInput` values and are parsed in parallel. Each file produces `IntermediateStorage`, which is the in-memory shape of a future store flush:

- file metadata
- verified parser source hashes
- nodes
- edges
- occurrences
- component access
- callable projection state
- impl anchors
- errors

This phase is where the indexer builds unresolved edges and other graph artifacts. Resolution does not happen yet.

Parsed artifact-cache writes retain Rayon result order and commit once per
existing file chunk. Duplicate paths therefore keep ordered last-write
semantics, while a row failure rolls back the whole cache chunk. Cancellation
is observed at the next chunk boundary: an in-flight cache transaction either
commits completely or rolls back, and a staged full refresh still cannot
become live before the runtime publication fence.

### 7. The indexer flushes projection batches

As file results are merged, `WorkspaceIndexer::run` flushes batches once file, node, edge, or occurrence counts cross the configured thresholds.

Projection flushes write more than the core graph:

- files
- nullable verified parser source hashes
- nodes
- edges
- occurrences
- component access tuples
- callable projection state

The store flush path writes the verified hash beside the file row and clears it
when a refreshed row has no verified parser content identity. Modification time
is captured only after the verification read matches. The same flush invalidates
grounding snapshots as part of persistence. Projection flush is both a write
boundary and a derived-state invalidation boundary.

### 8. Resolution happens after flushes

Once all batched projection data has been flushed, the indexer runs `ResolutionPass`.

That pass:

- loads unresolved call, import, and override edges from the store
- builds candidate indexes
- applies structural strategies first
- uses semantic candidate lookup as a fallback when enabled and supported

Resolution is scoped differently by refresh mode:

- full refresh resolves without a touched-file scope
- incremental refresh limits the pass to touched files

This is why unresolved edges are visible in storage before resolution completes.

### 9. Incremental cleanup removes stale state

Cleanup is split into two pieces for incremental runs:

- before merging new results for a touched file, the indexer may delete stale callable projection rows for that file
- after the resolution pass, the indexer removes files that no longer exist in the workspace

That makes incremental indexing more than just "parse changed files." It also reconciles stale projection state.

### 10. Runtime refreshes or publishes snapshots

The last step belongs to runtime plus store:

- full refresh finalizes a staged build, creates deferred indexes, refreshes the summary snapshot, and publishes the staged database
- incremental refresh stays on the live database and refreshes both summary and detail snapshots in place

Full and incremental snapshot behavior are intentionally not symmetric.

### 11. Runtime synchronizes core search inputs

After graph and snapshot work, runtime rebuilds the search-symbol projection, opens or refreshes the persisted Tantivy search directory, writes graph-native symbol docs, writes deterministic component reports, and synchronizes selected dense-anchor docs. This is part of the default `index` contract.

A new persisted symbol generation uses one 20 MB Tantivy writer across the
existing 8,192-document checkpoints. Runtime checks cancellation after each
checkpoint and immediately before the single commit/reload. Dropping an
unfinished writer restores its in-memory fuzzy projection; the generation is
not admitted until document-count validation and the completion marker both
succeed. This protects process failure and cancellation. The completion-marker
rename does not claim power-loss durability before its parent directory is
synced.

Semantic sync does these pieces of work:

- build deterministic generated text for durable AST symbols and store it in `symbol_search_doc`
- build deterministic component/community report docs with extracted provenance
- classify each symbol under `graph_first_v1`
- reuse unchanged dense-anchor input metadata when generated text hash, selection reason, and policy version still match
- publish selected anchor text, source provenance, source range, content hash, policy, and exact core generation/run identity without loading the embedding engine
- prune stale symbol docs or dense inputs that no longer correspond to the refreshed graph and policy

Full refresh has an extra copy-forward path: if a previous live database exists, unchanged symbol docs, retrieval artifact nodes, and dense-anchor inputs are copied into the staged database before publish. The later sync can retain content-level reuse while rebinding every selected input to the candidate core publication.

Incremental refresh scopes symbol-doc and dense-anchor invalidation to changed or removed files plus files connected to them through the graph before or after the refresh. Related-symbol text, edge digests, centrality, and component reports can change at either endpoint, so those graph-dependent files are rebuilt even when their source bytes did not change. Unrelated untouched files keep their existing inputs. Vector reuse and rebuild happen only when retrieval finalizes the next immutable generation.

The default symbol-doc scope is durable symbols: classes, structs, interfaces, annotations, unions, enums, typedefs, functions, methods, macros, global variables, constants, and enum constants. Lower-signal module, namespace, package, field, local variable, and type-parameter docs stay out of dense retrieval by default while remaining present in graph and lexical search. Set `CODESTORY_SEMANTIC_DOC_SCOPE=all` only for investigations.

The dense-anchor policy version is `graph_first_v1`. Dense reasons are `public_api`, `entrypoint`, `documented_nontrivial`, `central_graph_node`, `component_report`, and `unstructured_doc`. Private trivial helpers, generated/vendor code, and test-only implementation details are skipped for dense embedding unless they are structurally central; they remain discoverable through exact lookup, `symbol_search_doc`, source lexical search, and graph expansion.

The default semantic text alias policy is `CODESTORY_SEMANTIC_DOC_ALIAS_MODE=alias_variant`. It keeps compact language, terminal-name, owner-name, and symbol-role hints, but leaves out the noisier full name-alias and path-alias lists from the earlier `current_alias` research variant. Use `no_alias` for baseline research rows and `current_alias` only when reproducing older alias-enriched runs.

Dense input generation remains part of core indexing. Model execution,
query-versus-bulk scheduling, platform policy, and immutable vector publication
belong to [retrieval design](retrieval-design.md) and the
[llama-sys subsystem](subsystems/llama-sys.md).

Keep measured repo-scale timings in [codestory-e2e-stats-log.md](../testing/codestory-e2e-stats-log.md). Architecture explains the lifecycle; the testing log owns time-specific numbers because caches, backends, and workstation state drift.

## Mental Model

### How files are selected for refresh

`codestory-workspace` is the source of truth for file discovery and diffing. Incremental runs only reindex files whose stored inventory is missing, stale, or marked unindexed.

### When files are skipped

The indexer skips files before parsing when it cannot select a parser-backed
language configuration or structural collector for the path plus compilation
metadata. See [language-support.md](language-support.md) for the distinction
between parser-backed graph support, structural collectors, and candidate parser
compatibility records.

### How `compile_commands.json` participates

`WorkspaceIndexer::new` looks for a compilation database near the workspace root. When present, parsed compilation info informs language configuration and becomes part of the artifact-cache key. If compile-command paths cannot be made relative to the workspace root, the indexer skips artifact-cache lookup/write for that file and rebuilds instead.

### Where artifact caching is used

Artifact caching sits inside the indexer before parsing. Cache hits can reuse a file's serialized projection payload; cache misses fall back to parse and extract work.

### What gets written before resolution

Files, nodes, edges, occurrences, component access, and callable projection state are flushed before `ResolutionPass` runs. Resolution then updates unresolved edges using the stored graph state.

### What full refresh publishes that incremental refresh does not

Full refresh builds a staged database and publishes it only after staged finalization succeeds. Incremental refresh never publishes a staged build; it updates the live store and refreshes live snapshots in place.

Neither core path alone publishes the immutable retrieval generation.
Retrieval finalization binds its candidate to the resulting core generation and
leaves the prior retrieval publication active if source or core identity drifts.

### How symbol docs and dense anchors are kept fast

Symbol docs are deterministic graph artifacts persisted in SQLite with generated-text metadata and extracted provenance. Dense anchors are persisted separately as embedding-free inputs. Core reuse is keyed by generated text hash, selection reason, and semantic policy version; the stored source identity still changes to the exact candidate core generation/run before publication. Core publishes the complete anchor count, content digest, policy, migration state, and source identity as one manifest with the graph generation. On full refresh, runtime copies prior retrieval artifact nodes, symbol docs, and dense inputs forward into the staged database before checking them. On incremental refresh, runtime rebuilds inputs for changed and removed files plus files connected to them through the previous or refreshed graph, then rebinds the complete carried-forward set before publication.

Retrieval fingerprints content, provenance, the complete core generation/run,
and the stable model/engine/device compatibility identity before selecting a
generation. Core can reuse unchanged embedding-free anchor inputs, but each
core or producer-compatibility publication receives a distinct immutable vector
generation and producer-evidence identity. Retrieval embeds the selected inputs
in bounded batches, validates exact anchor/hash coverage and vector properties,
and publishes the attested generation. Core indexing never loads the model.

### What timing output means

The index summary reports graph and semantic work separately:

- `timings_ms.cache_refresh`: wrapper time for search projection, search indexing, semantic sync, and runtime publication
- `cache_ms.search_projection`: SQLite search-symbol projection rebuild from persisted nodes
- `cache_ms.search_index`: runtime search index construction for symbol names
- `symbol_index`: documents written plus Tantivy writer, commit, and reader-reload counts; completed-generation reuse reports zero for each count
- `cache_ms.runtime_publish`: publishing the rebuilt search state into the live runtime
- `semantic_ms.doc_build`: generated semantic text and hashes
- `semantic_ms.embedding`: always zero for core indexing; retained as a compatibility field
- `semantic_ms.db_upsert`: SQLite writes for symbol docs and embedding-free dense inputs
- `semantic_ms.reload`: compatibility timing for the core search state
- `semantic_ms.prune`: removing stale dense inputs after the refreshed symbol set is known
- `symbol_search_docs_written`: graph-native symbol docs and component reports written for lexical/graph recall
- `semantic_docs.reused`: dense-anchor inputs whose content/policy metadata was reusable
- `semantic_docs.embedded`: always zero for core indexing; vector production is retrieval-owned
- `semantic_docs.pending`: changed dense-anchor inputs that require the next retrieval-generation decision
- `semantic_docs.stale`: persisted dense-anchor inputs pruned because they no longer match the refreshed symbol set
- `semantic_dense_docs_skipped` and `semantic_dense_*`: policy skip and dense-reason counters for `graph_first_v1`

Use these fields before changing parser, graph, or SQLite code for a slow
`index` run.

## How To Debug Indexing

Start with static docs first:

1. [Architecture overview](overview.md)
2. [Runtime execution path](runtime-execution-path.md)
3. [Indexer subsystem](subsystems/indexer.md)
4. [Debugging guide](../contributors/debugging.md)

Then use live tooling if you need workspace-specific evidence:

- `codestory-cli index --project .`
- `codestory-cli search --project . --query <symbol>`
- the canonical plugin skill in `plugins/codestory/skills/codestory-grounding/SKILL.md`

Treat the grounding workflows as follow-up evidence, not the primary
explanation. Local grounding and search-state rebuilds can depend on semantic
retrieval assets and current machine health, so the architecture docs should
remain the primary reference when you are learning the pipeline.

## Verification Targets

If you change indexing behavior, review or run the suites that guard it:

- `cargo test -p codestory-indexer --locked --test fidelity_regression`
- `cargo test -p codestory-indexer --locked parser_result_changed_with_restored_mtime_is_incomplete_and_not_cached`
- `cargo test -p codestory-indexer --locked artifact_cache_result_changed_with_restored_mtime_is_rejected`
- `cargo test -p codestory-store --locked projection_batch_round_trips_and_clears_file_content_hash`
- `cargo test -p codestory-indexer --locked --test tictactoe_language_coverage`
- `cargo test -p codestory-indexer --locked --test integration`
- targeted resolution suites under `crates/codestory-indexer/tests/`
