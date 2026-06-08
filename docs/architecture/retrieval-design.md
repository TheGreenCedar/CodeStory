# Mandatory Sidecar Retrieval Design

CodeStory packet/search evidence is sidecar-primary. A product response may be
served only when the current project has a manifest-backed sidecar generation
with `retrieval_mode=full`.

`full` means all of the following are true for the same generation:

- Zoekt lexical shard exists, matches the current lexical input hash, and
  answers smoke queries.
- Qdrant collection exists, has at least the manifest projection count, uses the
  product llama.cpp `bge-base-en-v1.5` embedding backend, and answers semantic
  smoke queries.
- SCIP graph artifacts exist and are not stub markers.
- The SQLite `retrieval_index_manifest` has the current schema version,
  sidecar input hash, sidecar generation, Qdrant collection, embedding backend,
  embedding dimension, and projection count.

Everything else is diagnostic only. `no_scip`, `no_semantic`, `lexical_only`,
`unavailable`, stale manifests, stub markers, disabled sidecars, hash vectors,
ONNX-only paths, old env aliases, and `CODESTORY_RETRIEVAL=0` fail closed for
agent-facing packet/search.

## Ownership

| Area | Owner | Supporting areas |
|------|-------|------------------|
| Sidecar clients, health, index generation, query execution | `codestory-retrieval` | `codestory-cli` |
| Manifest persistence and migrations | `codestory-store` | `codestory-contracts` |
| Packet/search routing and fail-closed behavior | `codestory-runtime` | `codestory-contracts` |
| CLI setup, status, index, query commands | `codestory-cli` | `codestory-retrieval` |
| Benchmarks and promotion gates | `scripts/` | docs |

## Mode Matrix

| Zoekt | Qdrant | SCIP | Mode | Product behavior |
|-------|--------|------|------|------------------|
| up | up | up | `full` | Serve packet/search evidence |
| up | up | down | `no_scip` | Fail closed |
| up | down | up | `no_semantic` | Fail closed |
| up | down | down | `lexical_only` | Fail closed |
| down | * | * | `unavailable` | Fail closed |

Runtime rules:

- Only `full` can serve primary packet/search results.
- Non-`full` modes must expose `retrieval_mode` and `degraded_reason`.
- Guard checks may warn or block promotion, but never switch product behavior to
  an older retrieval path.
- Repo-text, hash, stub, and old local search surfaces may be used only as
  explicitly labeled diagnostics.

## Generation And Reuse

Sidecar generation is content-addressed by project id and sidecar input hash.
The hash includes local lexical input, symbol projection rows, semantic file
role metadata, sidecar schema version, Zoekt version pin, embedding backend,
embedding dimension, and SCIP artifact contract inputs.

`retrieval index --refresh auto` should reuse an unchanged healthy generation.
If inputs match but health is not `full`, CodeStory rebuilds the unhealthy
component and persists the manifest only after the full stack is healthy.

## Evidence Rules

- Exact symbol and path evidence remains the precision floor.
- Semantic and graph evidence can expand or rank candidates, but cannot replace
  a missing exact sidecar contract.
- Mixed or symbol-shaped queries may run Qdrant semantic search with a
  candidate-path allowlist derived from prior Zoekt/SCIP evidence. This is a
  query-time resource optimization only: it never changes the sidecar
  projection count, never marks a partial semantic corpus as `full`, and must
  fall back to an unfiltered Qdrant search when the scoped pass underfills or
  errors.
- Broad prompt retrieval should let lexical/source evidence compete with
  semantic evidence and should downrank tests, generated files, benchmarks, and
  vendor paths unless the query explicitly asks for those roles.
- Broad packet/search results must preserve provenance and mark weak evidence.
- Search plans and repo-text diagnostics are discovery aids, not final proof.
- Promotion metrics must come from one coherent fresh artifact run.

## Cost Envelope

| Tier | Example repos | Cold index budget | Sidecar disk budget | Query process budget |
|------|---------------|-------------------|---------------------|----------------------|
| S | `codestory`, `axios` | 8 min | 4 GB | 1.5 GB |
| M | `ripgrep`, `rootandruntime`, `codex` | 15 min | 8 GB | 3 GB |
| L | `redis`, `sourcetrail`, `vscode` | 45 min | 25 GB | 6 GB |
| XL | `vscode` monolith | 60 min | 35 GB | 8 GB |

Promotion is blocked for a tier if cold index exceeds budget by more than 20%
without a documented exception.

## Promotion Guards

Guard warnings block promotion when consecutive full local-real runs show:

| Trigger | Threshold |
|---------|-----------|
| p95 packet wall regression | >25% versus current accepted sidecar baseline |
| retrieval p99 regression | >50% versus current accepted sidecar baseline |
| quality pass drop | at least one repo worse than prior promotion |
| sufficient-quality mismatch | any increase |
| degraded mode rate | >5% of runs |
| VS Code claim recall | <50% while packet says sufficient |

The file currently named `retrieval-rollback.json` stores these guard
thresholds. It is not a runtime rollback mechanism.

## Generalization

Local-real tuning repos are `codex`, `rootandruntime`, `sourcetrail`, and
`vscode`. Holdout repos should be fetched into ignored target directories and
must not influence ranker/planner tuning. Dogfood results on `codestory` are
fast regression evidence, not generalization proof.

Promotion requires at least:

- fresh coherent six-lane artifacts,
- served packet/search rows reporting `retrieval_mode=full`,
- local-real quality that beats the prior accepted baseline,
- no diagnostic/stub/hash product evidence,
- docs and runbooks aligned with the current mandatory sidecar contract.
