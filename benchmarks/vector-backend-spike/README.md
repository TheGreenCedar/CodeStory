# Vector backend evidence runner

This is the evidence harness for #1202. It admits one real, complete CodeStory
retrieval publication before it freezes a vector-free source-truth catalog with
the linked CodeRank query embedder, then compares `sqlite-vec` and USearch in
separate processes. It does not route, package, or select a production backend.

## Current disposition

No candidate measurement has been recorded. The approved Linux corpus cannot
yet create the required immutable source publication: its full index exceeds
SQLite's bind-variable limit in the file-identity lookup. The separate
current-source control passes, which makes this a large-project indexing
defect tracked in [#1237](https://github.com/TheGreenCedar/CodeStory/issues/1237),
not evidence for either vector backend. Until that publication exists, #1202
is **inconclusive** and #1196 must not choose a backend from this work.

The runner uses six counterbalanced paired blocks for each declared nested
real-anchor workload in two fresh evidence roots. It snapshots the admitted
`vectors.sqlite3`, its `vector-generation-manifest.json`, and the fixture into
the evidence root before any child starts. Every oracle, candidate, and fresh
memory-reader child checks that frozen input manifest, the binary digest, and
the recorded host evidence before and after its work. Its journal is fsynced
after each child result, and each candidate publishes staged immutable
generations with a digest-bound manifest and atomic current/rollback pointer.
The bundle accepts ordinary files only; symlinked input paths fail closed.

Create a fixture only after retrieval indexing produced an immutable
publication. Keep query embedding outside candidate timing:

```sh
CODESTORY_CACHE_ROOT=/Volumes/CodeStoryVectorEvidence/runs/fixture-cache \
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
CODESTORY_VECTOR_SPIKE_OUTPUT_ROOT=/Volumes/CodeStoryVectorEvidence/runs/measurement-<sha> \
node scripts/run-vector-backend-spike.mjs
```

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
