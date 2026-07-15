# Runtime Execution Path

CodeStory routes CLI, HTTP, and MCP calls through the same product boundaries.
The adapter validates and renders; `codestory-runtime` orchestrates; the owning
workspace, store, indexer, and retrieval crates enforce their state contracts.

## Request state machine

```mermaid
flowchart TD
    Input["CLI, HTTP, tool, or resource request"] --> Validate["validate name, URI, and arguments"]
    Validate --> Project["select explicit project and immutable config"]
    Project --> Class{"surface class"}
    Class -->|"observational"| Observe["read status or diagnostics"]
    Class -->|"local graph"| Refresh["bounded local freshness check/refresh"]
    Class -->|"broad retrieval"| Prepare["bounded local refresh + managed retrieval preparation"]
    Class -->|"exact target"| Target["resolve target, then apply its graph/retrieval requirement"]
    Refresh --> Execute["runtime operation"]
    Prepare --> Ready{"publication and engine ready?"}
    Ready -->|"still working"| Retry["preparing + retry same call"]
    Ready -->|"unavailable"| Gap["fail closed with actionable gap"]
    Ready -->|"ready"| Pin["pin coherent retrieval identity"]
    Pin --> Execute
    Target --> Execute
    Execute --> Revalidate{"publication changed?"}
    Revalidate -->|"yes"| Bounded["one bounded retry"]
    Revalidate -->|"no"| Render["redact diagnostics and render DTO"]
    Bounded --> Execute
```

Validation precedes activation. An unknown resource or malformed request cannot
refresh a project, initialize the engine, or mutate status state.

## Surface classes

| Class | Examples | May activate work | Required state |
| --- | --- | --- | --- |
| Observational | `status`, `doctor`, retrieval-engine diagnostics | No | readable current state |
| Local graph | `ground`, `files`, `symbol`, `callers`, `trail`, `snippet` | bounded local refresh | current core publication for the requested surface |
| Broad retrieval | `packet`, `search`, broad query-based `context` | local refresh, engine init, retrieval finalization | coherent retrieval publication and live policy-compliant engine |
| Exact target | definition/reference/node and focused context calls | only what the selected operation needs | resolved target plus its declared readiness |

`ground` is the normal first call. It can return a useful local repository map
after the bounded graph refresh while managed broad-retrieval preparation
continues. A later `packet` or `search` call either completes that preparation,
returns `preparing` for a bounded retry, or reports an environment gap. There is
no user-facing sidecar setup or repair decision.

## Core indexing

An explicit `index` request delegates to runtime, which asks
`codestory-workspace` for a complete or incremental refresh plan,
`codestory-indexer` to parse and resolve projections, and `codestory-store` to
publish or refresh the core database. Runtime then synchronizes graph-native
symbol documents and reusable dense-anchor rows.

This publishes core state, not the immutable retrieval generation. Normal
agent activation or an explicit retrieval index operation may next finalize
lexical, vector, and SCIP artifacts against that core publication. See the
[indexing pipeline](indexing-pipeline.md).

## Broad retrieval read

```mermaid
sequenceDiagram
    participant Adapter as CLI or stdio adapter
    participant Runtime as codestory-runtime
    participant Retrieval as codestory-retrieval
    participant Store as codestory-store

    Adapter->>Runtime: packet/search request
    Runtime->>Retrieval: ensure current publication and live engine
    Retrieval->>Store: derive core publication identity
    Retrieval-->>Runtime: query hits + RetrievalPublicationIdentity
    Runtime->>Store: open matching read transaction
    Runtime->>Retrieval: retain lexical/semantic generation leases
    Runtime->>Runtime: resolve candidates and assemble evidence
    Runtime->>Retrieval: revalidate publication and engine identity
    alt publication changed
        Runtime->>Runtime: retry once
    else coherent
        Runtime-->>Adapter: cited result DTO
    end
```

Query execution and candidate resolution use separately pinned reads tied by
one publication identity; they are not one indefinitely held database
snapshot. Numeric node IDs are accepted only through the matching core
publication. A concurrent publish returns `cache_busy` to the bounded retry
instead of resolving an old candidate against a new graph.

`retrieval_mode=full` records the artifact classification. Serving additionally
requires a current manifest, matching producer identity, live embedded engine,
and allowed surface. The mode string alone cannot bless dead or mismatched
infrastructure.

## Local and exact-target reads

Runtime reads graph rows, occurrences, trails, search documents, or grounding
snapshots from `codestory-store` and assembles contract DTOs. `explore` and
`serve` reuse those services; adapters do not open SQLite or invent product
fallbacks. When broad retrieval is unavailable, local graph tools can remain
usable, but their output must not be presented as a full packet/search result.

## Failure boundaries

- adapters never choose a global active project;
- status and diagnostics never activate managed work;
- project switching never rereads ambient process defaults;
- core success never implies retrieval publication success;
- stale, partial, ambiguous, non-`full`, or engine-mismatched broad evidence
  fails closed;
- CLI rendering does not reimplement runtime orchestration.

Read [host integration](host-integration.md) for plugin lifecycle and
[retrieval design](retrieval-design.md) for publication mechanics.
