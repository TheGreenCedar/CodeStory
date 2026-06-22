# Mandatory Sidecar Retrieval Design

CodeStory packet/search evidence is sidecar-primary. A product response may be
served only when the current project has a manifest-backed sidecar generation
with `retrieval_mode=full`.

`full` means all of the following are true for the same generation:

- Zoekt lexical shard exists, matches the current lexical input hash, and
  answers smoke queries against source files plus generated graph-native symbol
  docs and component-report virtual docs.
- Qdrant collection exists, has at least the manifest dense-anchor projection
  count, matches the product-compatible BGE-base embedding contract, and answers
  semantic smoke queries from the live llama.cpp sidecar when the active
  semantic policy selects one or more dense anchors. If the active policy
  selects zero dense anchors, Qdrant is explicitly not required for that
  generation.
- SCIP graph artifacts exist and are not stub markers.
- The SQLite `retrieval_index_manifest` has the current schema version,
  sidecar input hash, sidecar generation, Qdrant collection, embedding backend,
  embedding dimension, symbol-doc count, dense-anchor count, semantic policy
  version, graph artifact hash, and dense reason counts.

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

| Zoekt | Qdrant | SCIP | Dense anchors | Mode | Product behavior |
|-------|--------|------|---------------|------|------------------|
| up | up | up | >0 | `full` | Serve packet/search evidence |
| up | skipped by policy | up | 0 | `full` | Serve graph/lexical packet/search evidence; dense stage is explicitly skipped |
| up | up | down | any | `no_scip` | Fail closed |
| up | down | up | >0 | `no_semantic` | Fail closed |
| up | down | down | >0 | `lexical_only` | Fail closed |
| down | * | * | any | `unavailable` | Fail closed |

Runtime rules:

- Only `full` can serve primary packet/search results.
- Non-`full` modes must expose `retrieval_mode` and `degraded_reason`.
- Guard checks may warn or block promotion, but never switch product behavior to
  an older retrieval path.
- Repo-text, hash, stub, and old local search surfaces may be used only as
  explicitly labeled diagnostics.

## Generation And Reuse

Sidecar generation is content-addressed by project id and sidecar input hash.
The hash includes local lexical input, graph-native `symbol_search_doc` rows,
dense-anchor rows, semantic file-role metadata, sidecar schema version, Zoekt
version pin, embedding backend, embedding dimension, semantic policy version,
dense reason counts, and SCIP artifact contract inputs.

`retrieval index --refresh auto` should reuse an unchanged healthy generation.
If inputs match but health is not `full`, CodeStory rebuilds the unhealthy
component and persists the manifest only after the full stack is healthy.

## AST-First Semantic Contract

Code structure is graph-native first. Runtime writes a deterministic
`symbol_search_doc` for every durable AST symbol. These docs contain symbol name,
kind, file, signature, comments, aliases, related symbols, edge digest, hash,
policy version, extracted provenance, and file/node provenance. They are indexed
lexically and used for candidate generation and graph expansion; they are not
embedded by default.

Dense vectors are reserved for `graph_first_v1` anchors. Allowed reasons are
`public_api`, `entrypoint`, `documented_nontrivial`, `central_graph_node`,
`component_report`, and `unstructured_doc`. Rejected private trivial helpers,
generated/vendor code, test-only helpers, and local implementation details must
still be discoverable through symbol docs, source lexical search, exact symbol
lookup, and graph expansion. There is no anonymous foreground cap: every dense
or skipped symbol must be explainable through policy counters.

Component reports are deterministic extracted graph artifacts. They group symbols
by crate/module/directory ownership and summarize central "god node" symbols
using import/call/reference shape. Reports are virtual docs in the lexical shard
and may be dense anchors with reason `component_report`.

## Evidence Rules

- Exact symbol and path evidence remains the precision floor.
- Candidate generation order is exact symbol/AST lookup, lexical source and
  virtual-doc search, graph expansion, then dense-anchor augmentation.
- Dense search must never be the only recall path for code symbols.
- Served search evidence should expose provenance labels such as `exact`,
  `lexical_source`, `symbol_doc`, `graph_neighbor`, `component_report`, and
  `dense_anchor`.
- Broad prompt retrieval should let lexical/source evidence compete with
  semantic evidence and should downrank tests, generated files, benchmarks, and
  vendor paths unless the query explicitly asks for those roles.
- Broad packet/search results must preserve provenance and mark weak evidence.
- Search plans and repo-text diagnostics are discovery aids, not final proof.
- Promotion metrics must come from one coherent fresh artifact run.
- `retrieval_mode=full` is necessary infrastructure readiness. It is not enough
  to promote answer quality or language quality without packet-runtime or drill
  evidence at the matching proof tier.

Packet citations may expose optional JSON fields: `evidence_tier`,
`evidence_producer`, `resolution_status`, `coverage_role`, and
`eligible_for_sufficiency`. Sufficiency is role-bearing: a citation can help
prove a packet claim only when the evidence tier, resolved/source-range status,
and coverage role match the claim being covered.

| Evidence tier | Proof role | Sufficiency rule |
| --- | --- | --- |
| `exact_source` | Source line/range that directly covers the claim role. | Proof-bearing when source-range or resolved metadata is role-aligned. |
| `resolved_graph` | Typed graph symbol, edge, route, receiver, or dependency evidence. | Proof-bearing when resolved and aligned to the required role. |
| `lexical_source` | Lexical source hit from the manifest generation. | Proof-bearing only when the source range and role identify the needed behavior or shape. |
| `symbol_doc` | Generated graph-native symbol document. | Proof-bearing when it points back to resolved symbol/source provenance for the required role. |
| `component_report` | Deterministic graph component artifact. | Proof-bearing for ownership or component-shape claims when backed by graph/source provenance. |
| `dense_semantic` | Dense-anchor recall signal. | Diagnostic unless another proof-bearing citation covers the role. |
| `generated_summary` | Generated explanation or summary. | Diagnostic only. |
| `synthetic_source_scan` | Generic source scan, repo-text, or synthetic probe. | Diagnostic unless runtime policy admits a specific structural/source-shape role. |

Generic `source evidence` is not proof by itself. Proof-bearing roles need
resolved or role-aligned source, graph, lexical, symbol-doc, or component
evidence. Dense semantic hits, generated summaries, repo-text, and generic
synthetic source scans can explain where to look, but they must not carry
sufficiency unless a runtime policy explicitly admits their structural/source
shape for that role.

## Cost Envelope

| Tier | Example repos | Cold index budget | Sidecar disk budget | Query process budget |
|------|---------------|-------------------|---------------------|----------------------|
| S | `codestory`, `axios` | 8 min | 4 GB | 1.5 GB |
| M | `ripgrep`, `rootandruntime`, `axios` | 15 min | 8 GB | 3 GB |
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

Local-real tuning repos are declared in `benchmarks/tasks/local-real/`. Holdout
repos should be fetched into ignored target directories and must not influence
ranker/planner tuning. Dogfood results on `codestory` are fast regression
evidence, not generalization proof.

Promotion requires at least:

- fresh coherent six-lane artifacts,
- served packet/search rows reporting `retrieval_mode=full`,
- local-real quality that beats the prior accepted baseline,
- no diagnostic/stub/hash product evidence,
- docs and runbooks aligned with the current mandatory sidecar contract.

Proof tiers, promotion checklist, and north-star SLOs:
[`retrieval-architecture.md`](../testing/retrieval-architecture.md). Setup,
version pins, env vars, and CI smoke:
[`retrieval-sidecars.md`](../ops/retrieval-sidecars.md).
