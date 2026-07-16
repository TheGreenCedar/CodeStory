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
      "query_text": "Where is the published vector generation validated?",
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
768 values. `query_text` is required for decision fixtures; older synthetic
smoke fixtures may omit it.

Create the source publication under an isolated cache root. The retrieval index
command is the existing product publication path; the spike does not export or
rewrite production state:

```powershell
$env:CODESTORY_CACHE_ROOT = 'C:\evidence\codestory-cache'
cargo run --release --locked -p codestory-cli -- index --project 'C:\source\representative-repo' --refresh full --format json
cargo run --release --locked -p codestory-cli -- retrieval index --project 'C:\source\representative-repo' --refresh full --format json
Get-ChildItem -LiteralPath $env:CODESTORY_CACHE_ROOT -Recurse -Filter vector-generation-manifest.json
```

Select the intended `vector-generation-manifest.json` from that isolated root;
its adjacent `vectors.sqlite3` is the source. An independently reviewed query
catalog supplies only source-truth labels, never vectors or model neighbors:

```json
{
  "schema_version": 1,
  "queries": [
    {
      "query_id": "symbol-query-001",
      "kind": "symbol",
      "query_text": "Where is the published vector generation validated?",
      "expected_node_ids": ["..."]
    }
  ]
}
```

The entry above is illustrative; the decision catalog must contain the 30
predeclared representative and symbol queries.

Freeze the fixture with the linked production query embedder. The source must
contain the requested base rows plus the real ordered tail rows; the generator
fails instead of synthesizing missing increments or expected identities:

```powershell
$env:CODESTORY_VECTOR_SPIKE_SOURCE_SQLITE = 'C:\evidence\codestory-cache\semantic\collections\<generation>\vectors.sqlite3'
$env:CODESTORY_VECTOR_SPIKE_QUERY_CATALOG_JSON = 'C:\evidence\vector-spike-query-catalog.json'
$env:CODESTORY_VECTOR_SPIKE_FIXTURE_JSON = 'C:\evidence\vector-spike-fixture.json'
$env:CODESTORY_VECTOR_SPIKE_VECTOR_COUNT = '100000'
$env:CODESTORY_VECTOR_SPIKE_INCREMENTAL_COUNT = '100'
cargo test --release --locked --no-default-features --features fixture-generator -p codestory-bench --test vector_backend_spike prepare_vector_backend_fixture -- --ignored --exact --nocapture
```

`CODESTORY_VECTOR_SPIKE_FIXTURE_JSON` must name a new file whose parent already
exists outside both the source generation and `CODESTORY_CACHE_ROOT`. Fixture
publication never replaces an existing destination.

Run each predeclared vector count from a clean, exact, release-built Git tree on
the approved Windows x64 production host:

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
deep-validation time, the existing-scan regression baseline, Windows offline
build and archive-size implications, license and native-dependency review, and
reversible fallback proof remain required Windows x64 decision evidence rather
than inferred values.

Linux and macOS quality/publication proof and their offline/native packaging do
not block this comparison PR. If the Windows x64 decision selects a candidate,
track those platform items in the later adoption implementation. If neither
candidate clears the Windows x64 criteria, keep the existing embedded scan.
