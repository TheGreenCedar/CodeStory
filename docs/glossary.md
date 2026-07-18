# Glossary

Plain-language definitions for terms used across user, architecture, and
verification docs.

## Readiness and activation

### local navigation readiness

The current core publication can serve database-backed graph tools such as
`ground`, `files`, `symbol`, `callers`, `trail`, and `snippet`. It does not
imply broad packet/search readiness.

### agent packet/search readiness

The project has a current complete retrieval publication, a matching live
per-user embedding server/engine identity, and an allowed packet/search/context surface.
`retrieval_mode=full` is necessary but is not sufficient on its own.

### retrieval mode

Stable status classification for persisted retrieval artifacts. `full` means
the lexical, vector, SCIP, manifest, and core identities form a complete
publication. Live serving still checks engine identity, policy, freshness, and
the requested surface.

### allowed surfaces

Per-operation readiness decisions reported in `codestory://status` and enforced
inside normal tool calls. They are diagnostic; agents should call the intended
tool and follow its structured result.

### managed activation

Automatic project refresh, embedded-engine initialization, or retrieval
finalization performed by the product call that needs it. It does not require
user consent or a user-managed service.

### preparing

A managed operation exceeded its foreground budget but is making progress. The
result names a delay and the same operation to retry. Local graph surfaces may
remain available.

## Identity and publication

### process scope

State captured once for a CodeStory process: startup defaults and its client
identity for the shared per-user embedding server.

### project scope

Repository identity, cache namespace, immutable configuration, and retained
runtime context selected by the absolute `project` on each request.

### core publication

One coherent `codestory.db` generation/run containing graph rows, snapshots,
search documents, component reports, and reusable dense-anchor inputs.

### retrieval publication

Immutable lexical `lexical-index.sqlite3`, semantic `vectors.sqlite3`, SCIP,
and manifest state bound to an exact core publication, source input, schema,
and embedding producer.

### retrieval publication identity

The core generation/run, retrieval generation/input hash, and semantic
generation carried from query through candidate resolution and final
revalidation.

### snapshot

Derived grounding view built from graph tables. A staged snapshot belongs to a
candidate core database that is not live until durable promotion completes.

### refresh baseline

Stored file inventory used to plan incremental refresh. Only complete source
discovery can prove that a previously indexed file disappeared.

## Evidence and search

### grounding

Indexed repository context tied to files and symbols. `ground` is the normal
starting map and can be useful with local navigation readiness alone.

### packet

Broad, retrieval-backed evidence assembled for an agent task, including
citations, sufficiency, gaps, and bounded follow-up commands.

### target context

Evidence for one concrete symbol, node, query-selected target, or bookmark. It
is not a synonym for broad packet discovery.

### trail

Focused graph walk from one symbol through callers, callees, imports, or
references.

### projection

Persisted derived state such as callable projection rows, ranked summaries, or
search documents.

### symbol doc

Deterministic per-symbol search text persisted in the core database.

### dense anchor

A symbol, component report, or unstructured document selected for embedding.
Reusable core rows feed the immutable semantic vector generation.

### repo-text hit

Raw file-content match. It is evidence to inspect, not an instruction or a
substitute for graph/publication coherence.

## System parts

### host launcher

The small plugin-packaged Node adapter that provisions and validates the exact
native CodeStory executable, exposes a fail-open catalog, and hands requests to
the native stdio process.

### runtime

`codestory-runtime`, the only product orchestration layer.

### retrieval engine

The llama.cpp/ggml model and accelerator context owned by the private per-user
embedding server and shared by compatible CodeStory client processes.

### workspace

Repository identity, source discovery, refresh planning, and shared filesystem
safety owned by `codestory-workspace`.

### cache root

Trusted root containing project-isolated core and retrieval state. See the
[CLI reference](users/cli-reference.md) for operator configuration.

### sidecar

Legacy compatibility vocabulary still present in some stable fields and
internal type names. In current architecture it refers to project-local
retrieval publication state, not an external server, daemon, port, or container.
