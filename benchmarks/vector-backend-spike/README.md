# Vector backend evidence runner

This is the evidence harness for #1202. It reads a production-published
`vectors.sqlite3`, freezes a vector-free source-truth catalog with the linked
CodeRank query embedder, and compares `sqlite-vec` and USearch in separate
processes. It does not route, package, or select a production backend.

## Current disposition

No candidate measurement has been recorded. The approved Linux corpus cannot
yet create the required immutable source publication: its full index exceeds
SQLite's bind-variable limit in the file-identity lookup. The separate
current-source control passes, which makes this a large-project indexing
defect tracked in [#1237](https://github.com/TheGreenCedar/CodeStory/issues/1237),
not evidence for either vector backend. Until that publication exists, #1202
is **inconclusive** and #1196 must not choose a backend from this work.

The runner uses six counterbalanced paired blocks for each declared nested
real-anchor workload in two fresh evidence roots. Its journal is fsynced after
each child result, and each candidate publishes staged immutable generations
with a digest-bound manifest and atomic current/rollback pointer.

Create a fixture only after retrieval indexing produced an immutable
publication. Keep query embedding outside candidate timing:

```sh
CODESTORY_CACHE_ROOT=/Volumes/CodeStoryVectorEvidence/runs/fixture-cache \
target/release/vector_backend_spike prepare \
  --source /path/to/vectors.sqlite3 \
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
