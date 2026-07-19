# Vector backend evidence runner

This is the evidence harness for #1202. It admits one real, complete CodeStory
retrieval publication before it freezes a vector-free source-truth catalog with
the linked CodeRank query embedder, then compares `sqlite-vec` and USearch in
separate processes. It does not route, package, or select a production backend.
The catalog is accepted only when its full `corpus_commit` matches the clean
Git `HEAD` of the supplied project root; the frozen fixture records that commit
and tree identity.

## Current disposition

No candidate measurement has been recorded. The original large-corpus indexing
blocker tracked in [#1237](https://github.com/TheGreenCedar/CodeStory/issues/1237)
is repaired, but its retained proof established the core publication only. It
did not create the full-retrieval `vectors.sqlite3` input required here. Until
an exact current source publication and both clean paired evidence roots exist,
#1202 is **inconclusive** and #1196 must not choose a backend from this work.

The runner uses six counterbalanced paired blocks for each declared nested
real-anchor workload in two fresh evidence roots. It snapshots the admitted
`vectors.sqlite3`, its `vector-generation-manifest.json`, and the fixture into
the evidence root together with the reviewed catalog. Before any candidate
starts, the same digest-bound binary re-resolves every catalog file/symbol and
document hash against the frozen source, re-embeds every query through the
product transport, and rechecks the exact clean corpus and publication
identities. That verification is frozen into the input manifest. Every oracle,
candidate, and fresh memory-reader child checks the manifest, binary, host, and
fixture-verification evidence before and after its work. Its journal is fsynced
after each child result. Before either completion marker is written, the runner
replays the declared counterbalanced matrix and validates every result artifact,
digest, frozen-input binding, and required fault result. Each candidate
validates a staged generation before atomically replacing the current/rollback
pointer. It attempts corrupt, incomplete, and mid-build-cancelled publications;
all must fail after real backend work while the previous pointer remains
readable. Barrier-held readers open the old generation before publication and
query only after it, while a new reader must match exact incremental source
truth. The bundle accepts ordinary files only; symlinked input paths fail
closed.

Create a fixture only after retrieval indexing produced an immutable
publication. Keep query embedding outside candidate timing:

```sh
mkdir -m 700 /Volumes/CodeStoryVectorEvidence/runs/fixture-embedding-authority
CODESTORY_CACHE_ROOT=/path/to/cache-that-owns-the-publication \
CODESTORY_EMBED_QUALIFICATION_DIR=/Volumes/CodeStoryVectorEvidence/runs/fixture-embedding-authority \
CODESTORY_EMBED_QUALIFICATION_NONCE=vector-spike-fixture-<sha> \
target/release/vector_backend_spike prepare \
  --project-root /path/to/linux \
  --storage /path/to/codestory.db \
  --catalog benchmarks/vector-backend-spike/query-catalog-linux-37e2f878.json \
  --output /Volumes/CodeStoryVectorEvidence/runs/linux-fixture.json
```

Run two clean evidence roots:

```sh
CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE=/path/to/vectors.sqlite3 \
CODESTORY_VECTOR_SPIKE_FIXTURE_JSON=/Volumes/CodeStoryVectorEvidence/runs/linux-fixture.json \
CODESTORY_VECTOR_SPIKE_PROJECT_ROOT=/path/to/linux \
CODESTORY_VECTOR_SPIKE_STORAGE=/path/to/codestory.db \
CODESTORY_VECTOR_SPIKE_CATALOG_JSON=benchmarks/vector-backend-spike/query-catalog-linux-37e2f878.json \
CODESTORY_VECTOR_SPIKE_OUTPUT_ROOT=/Volumes/CodeStoryVectorEvidence/runs/measurement-<sha> \
CODESTORY_CACHE_ROOT=/path/to/cache-that-owns-the-publication \
node scripts/run-vector-backend-spike.mjs
```

Keep `CODESTORY_CACHE_ROOT` on the cache that owns the admitted publication.
The runner creates a private qualification namespace for its verifier's native
embedding server. Preparation requires an explicit private namespace as shown,
so the evidence binary cannot share or replace the ordinary per-user authority.

Raw observations stay in the external evidence root. A completed run is still
not a selection: the criteria require review of the source-truth catalog and
the remaining package, license, native-dependency, integration, and fallback
evidence before #1196 can advance.

The approved profile is native macOS arm64. The orchestrator rejects another
host, Rosetta, or a non-arm64 Mach-O binary, and records the host, binary,
Cargo lock, and observed Rust, Cargo, and Node toolchain evidence in
`host-evidence.json`. Memory
is reported as current resident memory from a fresh candidate-reader process
after loading and warming the published candidate generation. Its baseline is
taken after frozen-input verification, and the output keeps that baseline plus
the post-load and post-warm values so it cannot be mistaken for startup memory
or a process-lifetime peak.

`first_query_after_load_ms` is deliberately a first-query-after-load measure,
not a cold-cache claim: building and the source-truth work may already have
touched filesystem pages. The runner keeps warm-query percentiles separately.
