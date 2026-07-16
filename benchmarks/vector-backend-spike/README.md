# Vector backend comparison spike

This directory owns the predeclared decision criteria and the reproducible
comparison harness for issue #1202. It deliberately does not select or adopt a
backend. The current decision remains blocked until the approved Windows x64
production evidence in `criteria.json` is present. For this non-adopting
comparison, Windows x64 is the only blocking platform.

## Harness

The ignored `vector_backend_spike` integration test builds sqlite-vec and
USearch indexes from the same identified vectors and runs the same identified
queries against both. Both candidates and the Rust oracle use cosine distance.
It measures build, load, query latency, disk use, exact-oracle recall, frozen
expected-identity Hit@20, incremental reuse, concurrent readers, immutable
generation publication, pointer rollback, and corrupt-candidate rejection.

Each backend publishes a complete, validated generation directory under
`generations/` and atomically swaps one `publication.json` record containing
the current and rollback pointer pair. Readers that already opened a generation
retain it; new readers resolve the new pointer. The harness hashes the unchanged
old generation before and after publication and exercises corrupt rejection and
rollback through the same paired-pointer protocol. Each generation manifest
binds the exact candidate index SHA-256 and a canonical digest of every
non-manifest file in the generation. A post-publication index tamper must make a
new reader fail closed while an already-open reader remains usable.

The artifact hashes the criteria, complete publication identity, selected
node/document/vector records, fixture, queries, incremental records, Git head
and tree, dirty worktree, toolchain, build profile, target, CPU, RAM, and ISA
envelope. Production source rows are selected deterministically with
`ORDER BY node_id` before applying the declared vector-count limit.

The default command is a fast synthetic harness check. Its output validates the
measurement path only and is not decision evidence:

```powershell
$env:CODESTORY_VECTOR_SPIKE_OUTPUT = 'target/vector-backend-spike/windows-x86_64-smoke.json'
cargo test --locked --no-default-features -p codestory-bench --test vector_backend_spike compare_vector_backends -- --ignored --nocapture
```

A decision-input run must use a complete CodeStory `vectors.sqlite3`
publication and a frozen JSON fixture. Before any rows are sampled, the harness
requires the fixture's predeclared production attestation, compares its exact
database SHA-256, runs SQLite `quick_check`, compares the complete metadata
identity, and recomputes the production `codestory-vector-digest-v1` digest and
row coverage across the entire database. Validation and sampling share one
pinned read transaction; the database is rehashed after sampling before records
are accepted. WAL, shared-memory, or rollback-journal sidecars are rejected
because their bytes are not covered by `database_sha256`. The fixture also
carries representative and symbol query records with expected node/document
identities and identified incremental records. Every vector must be finite,
nonzero, and L2-normalized within `1e-3`. Its shape is:

```json
{
  "schema_version": 2,
  "source_attestation": {
    "schema_version": 1,
    "generation": "...",
    "input_hash": "...",
    "embedding_backend": "...",
    "embedding_dim": 768,
    "point_count": 100000,
    "producer_identity": "...",
    "evidence_contract_identity": "...",
    "vector_digest": "...",
    "database_sha256": "..."
  },
  "incremental_set_id": "frozen-delta-001",
  "queries": [
    {
      "query_id": "symbol-query-001",
      "kind": "symbol",
      "vector": [1.0],
      "expected": [{ "node_id": "...", "document_hash": "..." }]
    }
  ],
  "incremental_records": [
    { "node_id": "...", "document_hash": "...", "vector": [1.0] }
  ]
}
```

The abbreviated vectors above illustrate the schema only; real vectors contain
768 values. Run each predeclared vector count from a clean, exact, release-built
Git tree on the approved Windows x64 production host:

```powershell
$env:CODESTORY_VECTOR_SPIKE_PROFILE = 'decision'
$env:CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE = 'C:\evidence\vectors.sqlite3'
$env:CODESTORY_VECTOR_SPIKE_FIXTURE_JSON = 'C:\evidence\vector-spike-fixture.json'
$env:CODESTORY_VECTOR_SPIKE_VECTOR_COUNT = '25000'
$env:CODESTORY_VECTOR_SPIKE_OUTPUT = 'target/vector-backend-spike/windows-x86_64-25000.json'
cargo test --release --locked --no-default-features -p codestory-bench --test vector_backend_spike compare_vector_backends -- --ignored --nocapture
```

The decision profile rejects synthetic or mismatched inputs, a missing or
mismatched database attestation, incomplete canonical row coverage, missing
evidence identity, missing query expectations or query kinds, synthetic increments,
non-normalized vectors, dirty Git state, debug builds, or unavailable host
identity. The smoke repeats the candidates in reversed order, but its artifact
sets `timing_comparable` to `false`: same-process timings and
`first_query_after_open_ms` are diagnostics, not cold-cache or candidate-choice
evidence. Isolated clean-host timing remains required.

USearch's own lower-bound memory estimate is reported, while sqlite-vec memory
is marked unmeasured. Cold-cache latency, isolated RSS, cancellation,
deep-validation time, and the existing-scan regression baseline remain required
Windows x64 decision evidence rather than inferred values.

Linux and macOS proof, cross-platform offline builds and native packaging,
archive-size measurement, license review, and implementation fallback proof do
not block this comparison PR. If the Windows x64 decision selects a candidate,
track those items in the later adoption implementation. If neither candidate
clears the Windows x64 criteria, keep the existing embedded scan.
