# Indexer Subsystem

`codestory-indexer` is the workspace indexing engine. It turns a refresh plan into graph rows, occurrences, callable projection state, and post-flush resolution updates in `codestory-store`.

## Ownership

- language detection and parser or query selection
- file-level parse and extract work
- parser artifact-cache lookup and reuse
- versioned structural text-unit collection, source revalidation, and
  structural-only cache reuse
- batching and projection flush timing
- post-flush call, import, and override resolution
- incremental cleanup for touched and removed files
- indexing regression, fidelity, and language-coverage suites

## Main Modules

- `crates/codestory-indexer/src/lib.rs`: `WorkspaceIndexer`, feature flags, indexing phases, flush timing, and incremental cleanup
- `crates/codestory-indexer/src/intermediate_storage.rs`: in-memory batch shape before a store flush
- `crates/codestory-indexer/src/cache.rs`: serialized parser and structural
  artifact-cache formats and cache-key construction
- `crates/codestory-indexer/src/structural/`: workflow, Compose, Cargo
  manifest, HTML, CSS, SQL, Markdown/MDX, generic YAML/TOML/JSON, non-parser
  shell, and PowerShell collectors plus structural unit finalization
- `crates/codestory-indexer/src/compilation_database.rs`: `compile_commands.json` discovery and parsed compilation metadata
- `crates/codestory-indexer/src/resolution/`: post-flush `ResolutionPass`, candidate selection, scoped resolution, and semantic fallback
- `crates/codestory-indexer/src/semantic/`: language-aware semantic helpers used by resolution

## Runtime Contract

The runtime owns orchestration and chooses the store shape for a run:

- full refresh: runtime opens a staged store, passes a full refresh plan to `WorkspaceIndexer`, then asks the store to finalize and publish the staged snapshot
- incremental refresh: runtime clones the live database into a durable staged store, passes a diff-based refresh plan to `WorkspaceIndexer`, refreshes summary and detail snapshots on the clone, then publishes the completed replacement

The indexer does not choose staged versus live storage. It only consumes a refresh plan plus a mutable store.

## Refresh Modes

`WorkspaceIndexer::run` handles both full and incremental work, but the plan changes the behavior:

- full refresh indexes every discovered source file, does not remove file rows, and runs unscoped resolution
- incremental refresh only touches files whose mtime changed, whose verified
  parser or structural source hash no longer matches, or that were previously
  not indexed, plus files whose durable extraction revision is outdated; it tracks removed file IDs, seeds the symbol table from existing
  rows, and scopes resolution to touched files

Incremental work also does more cleanup:

- before merging new results for a touched file, the indexer may delete stale callable projection state for that file
- after the resolution pass, the indexer removes files that disappeared from the workspace

## Pipeline

The core path inside `WorkspaceIndexer::run` is documented in the
[indexing pipeline](../indexing-pipeline.md). At a high level: discover files,
parse or reuse cached artifacts, flush projection batches, run `ResolutionPass`,
and clean up removed files on incremental runs.

Python callable inspection retains the exact outer `decorated_definition`
source range. Lexical call ownership, receiver mapping and proof inputs are
collected against the inner function before that displayed range is expanded.
Decorator callee references remain graph nodes but cannot act as lexical
callers for argument expressions; canonical declarations, including one-line
functions, remain eligible owners.
Rust struct and enum display ranges include their fields, variants and attached
outer attributes. These ranges expand after precise name occurrences, stable
type identities and call ownership have been collected; enum variants keep
their own name anchors. The same late stage retains Rust callable attributes,
while malformed syntax leaves the original anchors unchanged.
Parser body metadata joins the outer declaration coordinates to the proper
execution-body range. Successful verified extraction and current cache replay
record the Python or Rust extraction revision with the file's source hash.
Indexed, hash-verified parser-partial files with no file-level error retain this
producer identity while remaining incomplete; the identity certifies neither
complete coverage nor inherited-graph cache reuse. Missing or outdated producer
identity still selects the file for refresh. For complete projections, a revision
mismatch forces replacement before equality-based reuse.
Parser-backed invalid UTF-8 retains the existing lossy graph view only as
incomplete navigation. Raw-source/producer identity can remain current, but
complete cache reuse, Exact lookup inputs and exact source hydration stay
unavailable, including when cached replacement text parses without errors.

## What Gets Flushed

Projection flushes are broader than just graph rows. `IntermediateStorage` carries:

- file metadata
- nodes
- edges
- occurrences
- component access tuples
- callable projection state
- impl anchor node IDs
- verified structural source hashes
- collector-owned structural text units with separate content and placement
  identities
- one structural projection per admitted structural file, including zero-unit
  files
- dedicated structural artifact-cache writes
- indexing errors

`flush_projection_batch` writes the projection payload through
`codestory-store`. Source identity, graph rows, structural units and cache
writes, errors owned by refreshed file rows, and derived-snapshot invalidation
share one transaction. A cache-maintenance failure can produce a file-scoped
error without a replacement file row; that error retains the separate
file-outcome fallback and is not counted as projection persistence. Resolution
changes happen later, after those rows already exist. Indexer telemetry keeps
row-family work nested under the complete projection transaction wall and
reports attempted rows, estimated bound-input bytes, statement executions,
setup, and commit time without treating them as additive phases.

## Structural Unit Identity

The twelve structural unit collector families emit source-range-only evidence
without claiming parser-backed graph resolution. Finalization slices the exact
decoded UTF-8 source span and records the collector producer, evidence tier, resolution
status, language, kind, file role, descriptor version, source hash, and span.
The content identity is stable for an equivalent descriptor and exact source
slice; placement identity additionally includes file and node identity.

Only collector-marked nodes become structural units. Delegated parser nodes,
including HTML script or style descendants, remain parser-owned. Cache hits
must reproduce the complete unit and projection digests after a fresh source
read or the indexer recollects the file.

Format routing is ordered. GitHub Actions, Docker Compose, and Cargo manifests
keep their dedicated producers; OpenAPI JSON/YAML is checked on its dedicated
schema path before generic collection; `.sh` and `.bash` remain parser-backed
Bash. Generic shell structural fallback is limited to `.zsh`, `.ksh`, and
`.command`.

Structural admission applies one contracts-owned policy to workspace-relative
inventory paths before metadata or content reads. Generated and vendor trees,
secret-bearing conventions, lockfiles, minified/generated outputs, and
declared high-noise forms do not create file, unit, projection, or cache rows;
an excluded path already present in stored inventory is scheduled for normal
incremental deletion. Repository ancestors are never policy inputs. Admitted
files are capped at 1 MiB and 2,048 units, both carried on `SourceIndexPolicy`
rather than compiled into the collector, so an operator override reaches them and
an exclusion names the bound that refused it. A bound failure rejects the whole
file instead of publishing a truncated projection. Structural cache identity
remains v4: unchanged admitted UTF-8 artifacts remain reusable, and cache hits
enforce the same unit cap.
Structural text accepts UTF-8 and strict BOM-marked UTF-16LE/BE. UTF-16 BOMs
select byte order and are omitted from decoded coordinates; UTF-8 BOM handling
is unchanged. Native hashes and cache keys cover original bytes, so equal text
in different encodings is different source. Both raw and decoded bytes obey the
source cap. Unpaired surrogates, odd payloads, bare UTF-16 BOMs, decoded binary
controls, unknown encodings, malformed format syntax, and unreadable sources
retain distinct coverage reasons and do not create reusable structural cache
entries. A fully read, within-cap decoding rejection can retain a reverified raw
identity with an incomplete `binary` gap and no projection. It may coexist with
unrelated published source; it is never decoded by guessing or replacement.
Both the preparation and publication fences verify the original bytes, and
indexed text readers preserve the strict structural decoding contract.

## Resolution and Semantic Fallback

Resolution is a post-flush pass because it depends on the stored graph state:

- the indexer first persists unresolved edges and callable projection state
- `ResolutionPass` then loads unresolved call, import, and override edges from the store
- candidate selection prefers structural matches first and can use semantic candidate indexes as a fallback for supported languages
- incremental runs scope the resolution pass to touched files; full refresh resolves across the full workspace

Go package-call resolution consumes the stored hash of each required `go.mod`
control and a bounded module-declaration subset. It combines the exact local
module-relative package path with the indexed package clause, then resolves a
static imported selector only when one local package and one `FUNCTION` match.
Explicit aliases, implicit package names, malformed controls, duplicate local
module ownership, and local shadowing are handled before selection; ambiguous or
unverified ownership stays unresolved. A module-control change clears and
recomputes only this package-function resolution class so unchanged Go callers
cannot retain stale targets. This is local navigation evidence, not `go.work`,
`replace`, external dependency, return-type, or closure type-flow resolution.

Go package variable initializers retain ordinary `CALL` edges owned by the
existing `FILE` node, with each callee's physical callsite. Only AST-proven
initializer value expressions qualify; nested eager argument calls qualify,
while calls inside function literal bodies do not. Local variable initializers
keep their named callable owner. This adds navigation without declaring
initializer variables callable or granting exact-call proof. A Go-local
extraction revision invalidates unchanged older file projections and parser
artifacts through the existing producer compatibility checks.

This keeps parse and extract logic in the indexer while leaving persistence and snapshot ownership in the store.

## Extension Points

- add language detection or parser support in the indexer crate, not in runtime
- add new extraction or projection data in `IntermediateStorage` and the store projection flush path together
- add new resolution strategies inside `src/resolution/`
- add compilation-metadata-aware behavior through `compilation_database.rs`
- add regression fixtures or suites in `crates/codestory-indexer/tests/`

## Failure Signatures

- indexing behavior moves into runtime or CLI instead of staying in `codestory-indexer`
- unsupported files are treated as runtime concerns instead of being skipped by the indexer
- resolution logic is applied before graph data is flushed
- projection flushes change without matching updates to cleanup or snapshot invalidation expectations
- a structural-looking path is used to infer evidence provenance instead of
  reading the persisted unit descriptor
- new indexing behavior lands without a fidelity or regression test in `crates/codestory-indexer/tests/`

## Read Next

- [Indexing pipeline](../indexing-pipeline.md)
- [Runtime execution path](../runtime-execution-path.md)
- [Store subsystem](store.md)
