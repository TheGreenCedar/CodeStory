# Retrieval Subsystem

`codestory-retrieval` owns project-local retrieval artifacts and the
fail-closed query boundary. It uses `codestory-llama-sys` for embeddings and
exposes typed services to runtime and CLI; it does not own packet assembly or
adapter rendering.

## Inputs and outputs

Inputs are a selected project runtime config, a current core store publication,
a complete source inventory, graph-native search documents, and the immutable
process embedding policy. Outputs are:

- immutable lexical `lexical-index.sqlite3`, semantic `vectors.sqlite3`, and
  SCIP generations;
- a manifest binding those artifacts to source, core, schema, and producer
  identity;
- health/readiness reports;
- query hits carrying `RetrievalPublicationIdentity`.

## Main paths

- `index.rs` and `generation.rs`: candidate construction, validation, and
  atomic publication
- `lexical_index.rs` and `embedded_vector.rs`: lexical and vector persistence
- `scip_index.rs`: SCIP artifact generation
- `query.rs`, `executor.rs`, `candidate.rs`, and `ranker.rs`: coherent query
  execution and result identity
- `health.rs` and `mode.rs`: artifact classification and live readiness
- `in_process_embedding.rs`: process-wide engine selection and reuse
- `retention.rs`: generation leases and owned cleanup
- `config.rs`: frozen process defaults and per-project runtime config

## Concurrency and publication

Writers stage a whole generation, deep-validate it, rescan source at the commit
fence, and publish the manifest only when every identity still matches.
Readers carry one publication identity through candidate resolution, retain
generation leases, and revalidate before returning. Query-time checks stay
cheap; deep corpus validation is a build, promotion, readiness, or health
operation.

The embedding engine is process-wide, while manifests and artifacts are
project-local. An incompatible cache-root or CPU-policy request fails rather
than replacing an already initialized engine.

Retrieval finalization holds an embedding residency lease across candidate
build, validation, and publication. Manifest compatibility uses the stable
producer contract; the lease's owner/load generation is a live publication
fence and is not persisted as vector compatibility.

## Extension rules

- add artifact formats and identity fields here before teaching runtime about
  them;
- keep packet sufficiency, evidence assembly, and bounded product retry in
  `codestory-runtime`;
- keep model execution mechanics in `codestory-llama-sys`;
- preserve stable `sidecar_*` DTO fields only as compatibility vocabulary, not
  as an external-service abstraction.

## Failure signatures

- a query resolves numeric IDs against a different core publication;
- `retrieval_mode=full` is treated as live engine proof;
- deep row validation runs on every query;
- cleanup recurses from a previously validated pathname;
- a project silently reconfigures the process-wide engine.

See [retrieval design](../retrieval-design.md) and
[retrieval verification](../../testing/retrieval-architecture.md).
