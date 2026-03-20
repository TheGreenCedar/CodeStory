# Indexing Pipeline

This page explains how `codestory-cli index` turns a repository into SQLite-backed graph state, projection rows, and grounding snapshots.

Read this page when you need the implementation mental model. Use the CLI grounding workflows after that if you want live evidence from an indexed workspace.

## End-To-End Command Path

```mermaid
sequenceDiagram
    participant CLI as codestory-cli
    participant Runtime as codestory-runtime
    participant Workspace as codestory-workspace
    participant Indexer as codestory-indexer
    participant Store as codestory-store

    CLI->>Runtime: parse `index` command and open project
    Runtime->>Workspace: full refresh plan or diff-based refresh plan
    Workspace-->>Runtime: RefreshExecutionPlan
    Runtime->>Store: open staged store for full refresh or live store for incremental
    Runtime->>Indexer: WorkspaceIndexer::run(plan, store)
    Indexer->>Store: flush files, nodes, edges, occurrences, component access, callable projection state
    Indexer->>Store: run post-flush resolution updates
    Runtime->>Store: finalize staged snapshot or refresh live snapshots
    Runtime-->>CLI: indexing summary and phase timings
```

## Who Owns What

- `codestory-cli` parses the command and renders the indexing summary.
- `codestory-runtime` chooses full versus incremental flow and staged versus live store behavior.
- `codestory-workspace` discovers source files and computes the refresh plan.
- `codestory-indexer` turns the plan into projection writes and post-flush resolution.
- `codestory-store` persists rows, invalidates or refreshes snapshots, and publishes staged builds.

That split is intentional: the runtime orchestrates the run, the indexer performs indexing work, and the store owns persistence mechanics.

## Indexer Phases

```mermaid
flowchart TD
    plan["Refresh plan from codestory-workspace"] --> prep["Normalize paths and load compile_commands metadata"]
    prep --> supported{"Supported language?"}
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
- `build_refresh_plan` compares discovered files against stored inventory

For incremental work, a file is reindexed when:

- it is new
- its modification time is newer than the stored row
- it exists in the store but is marked as not indexed

Files that disappeared from discovery are collected into `files_to_remove`.

### 4. The indexer prepares file work

`WorkspaceIndexer::run` in `crates/codestory-indexer/src/lib.rs` starts by preparing state for the whole run:

- it seeds the symbol table from existing stored node kinds for incremental runs
- it chunks `files_to_index` using batch settings
- it loads parsed compilation metadata from `compile_commands.json` when available
- it picks a language configuration for each file and skips unsupported files before any parse work

Compilation metadata matters mostly for native-language parsing and is part of the artifact-cache key, so changes to compiler flags or include paths can invalidate cached artifacts.

### 5. Artifact cache decides parse versus reuse

`prepare_index_work` checks the index artifact cache before reparsing a file.

The cache key includes:

- the file path
- file bytes
- language queries
- feature-flag values that affect graph shape
- compilation metadata when present

A cache hit can reuse the serialized indexing artifact and turn it back into `IntermediateStorage`. A cache miss sends the file through parse and extract work.

### 6. Parse and extract run in parallel

Cache misses become `PreparedIndexInput` values and are parsed in parallel. Each file produces `IntermediateStorage`, which is the in-memory shape of a future store flush:

- file metadata
- nodes
- edges
- occurrences
- component access
- callable projection state
- impl anchors
- errors

This phase is where the indexer builds unresolved edges and other graph artifacts. Resolution does not happen yet.

### 7. The indexer flushes projection batches

As file results are merged, `WorkspaceIndexer::run` flushes batches once file, node, edge, or occurrence counts cross the configured thresholds.

Projection flushes write more than the core graph:

- files
- nodes
- edges
- occurrences
- component access tuples
- callable projection state

The store flush path invalidates grounding snapshots as part of persistence. That is why the docs should treat projection flush as both a write boundary and a derived-state invalidation boundary.

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

## Mental Model

### How files are selected for refresh

`codestory-workspace` is the source of truth for file discovery and diffing. Incremental runs only reindex files whose stored inventory is missing, stale, or marked unindexed.

### When files are skipped

The indexer skips files before parsing when it cannot select a supported language configuration for the path plus compilation metadata.

### How `compile_commands.json` participates

`WorkspaceIndexer::new` looks for a compilation database near the workspace root. When present, parsed compilation info informs language configuration and becomes part of the artifact-cache key.

### Where artifact caching is used

Artifact caching sits inside the indexer before parsing. Cache hits can reuse a file's serialized projection payload; cache misses fall back to parse and extract work.

### What gets written before resolution

Files, nodes, edges, occurrences, component access, and callable projection state are flushed before `ResolutionPass` runs. Resolution then updates unresolved edges using the stored graph state.

### What full refresh publishes that incremental refresh does not

Full refresh builds a staged database and publishes it only after staged finalization succeeds. Incremental refresh never publishes a staged build; it updates the live store and refreshes live snapshots in place.

## How To Debug Indexing

Start with static docs first:

1. [Architecture overview](overview.md)
2. [Runtime execution path](runtime-execution-path.md)
3. [Indexer subsystem](subsystems/indexer.md)
4. [Debugging guide](../contributors/debugging.md)

Then use live tooling if you need workspace-specific evidence:

- `codestory-cli index --project .`
- `codestory-cli search --project . --query <symbol>`
- the repo-local `codestory-grounding` skill in `.agents/skills/codestory-grounding/SKILL.md`

Treat the grounding workflows as follow-up evidence, not the primary explanation. Local grounding and search-state rebuilds can depend on semantic retrieval assets and current machine health, so the architecture docs should remain the first stop when you are learning the pipeline.

## Verification Targets

If you change indexing behavior, review or run the suites that guard it:

- `cargo test -p codestory-indexer --test fidelity_regression`
- `cargo test -p codestory-indexer --test tictactoe_language_coverage`
- `cargo test -p codestory-indexer --test integration`
- targeted resolution suites under `crates/codestory-indexer/tests/`
