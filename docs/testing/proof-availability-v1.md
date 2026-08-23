# Proof availability qualification v1

This qualification measures whether CodeStory's dark, exact call-path kernel can
prove or explain the frozen source paths in the proof-availability corpus. It is
benchmark infrastructure. It does not register a product tool or make proof
public.

## Inputs and lifecycle

Build one locked release `codestory-proof-availability` binary with the closed
command below, then use that same binary for materialization, the run, and
verification. It embeds Cargo's selected `rustc -vV` and build profile. Indexed
materialization rejects a non-release or dirty-source build. The embedded source
commit and tree are authoritative: the live checkout must remain clean and at
that exact identity. Public environment evidence records the build source,
compiler identity, prescribed build command, and explicit qualification ID.
Materialization checks out the exact frozen commits,
verifies every oracle range and each receipt step's independently frozen
full-source-file SHA-256, builds one fresh core index per project, and writes a
private local environment descriptor:

Before Q2, Q1 must prove that the evidence-only v3 surface is separable from
proof activation. The sealed conformance probe builds and validates packet,
context, and search results for every supported MCP revision while omitting the
proof tool, schema, and route. The companion feature-matrix build proves that
this surface compiles without the proof-qualification feature:

The sealed feature edge runs from the agent's v3 evidence planner through the
runtime's real `PacketExecutionRecordV3` and packet/context/search projection
builders to the CLI transport validator. The conformance probe serializes those
typed outputs; hand-built substitute JSON does not qualify.

```sh
cargo check --locked -p codestory-cli --lib --no-default-features \
  --features v3-evidence-separation-support
cargo test --locked -p codestory-cli --lib --no-default-features \
  --features v3-evidence-separation-support \
  stdio_v3::evidence_separation_tests::sealed_evidence_only_conformance_covers_all_revisions \
  -- --exact
```

Failure is a Q1 inseparability blocker: record plan-level Outcome D and do not
run Q2. A successful Q1 gate constrains Q2 to Outcomes A, B, or C. The
qualification binary never accepts caller-supplied dependency evidence.

```sh
cargo build --release --locked -p codestory-bench --bin codestory-proof-availability
qualification_bin=target/release/codestory-proof-availability
qualification_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=12 HEAD)"
run_root="target/proof-availability/$qualification_id"
results_root="$run_root/results"
mkdir -p "$run_root" "$results_root"

"$qualification_bin" materialize \
  --qualification-id "$qualification_id" \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace "$run_root/workspace" \
  --cache-root "$run_root/cache" \
  --out "$run_root/environment.json"
```

The ID is exactly `YYYYMMDDTHHMMSSZ-<12 lowercase commit hex>`, and its suffix
must match the qualification source commit. `environment_id` remains a separate
identity derived from measured environment and binary evidence.

The source-only audit form is separate. It accepts no qualification ID, does not
require a release build, and neither creates the cache nor indexes or executes
proof:

```sh
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- \
  materialize \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace target/proof-availability/source-audit \
  --cache-root target/proof-availability/unused-cache \
  --out target/proof-availability/source-environment.json \
  --verify-only
```

Run qualification only against the private descriptor produced by the same
clean source head and binary:

```sh
"$qualification_bin" run \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --environment "$run_root/environment.json" \
  --out "$results_root/$qualification_id"
```

The command is the complete Q2 contract for Outcomes A, B, and C.

All destinations are no-replace. Choose new paths for a rerun. Failed
materialization or publication keeps its owner-marked staging path for manual
inspection; the command never recursively removes a path as rollback.

## What the run does

The run rechecks the binary, source commit and tree, oracle bytes, database hash,
store schema, and pinned core publication. A receipt matches its oracle source
only when both the indexed and observed runtime file hashes equal the oracle's
full-file SHA-256; equality between the two runtime hashes is not sufficient. It
opens each store observationally,
derives inventory and edge-distinct trail counts, then sends every positive case
and its two frozen mutations through the accepted validator and the single
Runtime-owned core-pinned proof operation. No semantic retrieval publication is
created or required.

For every positive case, the runner immediately compares the product result's
contract digest with the digest produced by validating that frozen oracle path.
Read-only verification recomputes the same expected digest from the frozen
oracle, so a different well-formed SHA-256 fails before metrics are evaluated.

Public report schema `codestory.proof-availability-report/v4` does not expose
resolved runtime canonical IDs because those identities may contain host paths.
Each resolved source and target instead carries
`canonical_id_binding_sha256`, computed over RFC 8785 canonical JSON containing
the complete pinned node identity and raw canonical ID with the domain
`codestory.proof-availability-resolved-canonical-id/v1\0`. Frozen oracle
selectors may still use raw canonical IDs; verification recomputes the same
contextual binding from the selector and receipt pin. Exact graph IDs remain
signed 64-bit integers inside the runner but are encoded as strict canonical
signed-decimal JSON strings in public reports. This keeps RFC 8785
canonicalization from rounding IDs through IEEE-754 numbers. The v4
results-evidence digest binds these commitments under
`codestory.proof-availability-results-evidence/v4\0`.

The runner also authenticates the core-bound exact-resolution publication for
each cohort. `resolution-funnel.json` reports the installed adapter roster and
publication receipt, then partitions parser-derived facts by language, callee
form, and primary evidence kind. Later stages are deduplicated by fact identity:
proof-shape admission comes from admitted exact edges, authoritative receipts
come from the product result, and complete-proof participation requires an
evidence-supported `ContractProven` result.

A product finalization failure remains an `Invalid` case row rather than
aborting the report. It retains the actual trace and both completed mutation
rows, records every oracle step as missing, and carries no projection or receipt
claim.

The public result directory contains exactly:

```text
environment.json
inventory.json
trails.json
cases.json
failure-funnel.json
resolution-funnel.json
summary.json
decision.json
findings.md
```

JSON files use canonical compact JSON followed by one newline. The private
environment descriptor, absolute paths, environment variables, source logs, and
unbounded source text are excluded. The decision is recomputed from the complete
case rows and the explicitly supplied thresholds; it is not an input to the run.
`decision.json` embeds raw numerators and denominators, unrounded Wilson bounds
plus presentation values, per-cohort rows, step/partial/actionable rates,
latency and size percentiles, and hard-gate counts. Its domain-separated digest
binds those observations to the result and threshold hashes. Verification
recomputes the wrapper rather than trusting reported aggregates.
`findings.md` separates reproduced measurements from labeled inferences, records
the frozen hard gates and role thresholds used by the evaluator, and presents the
decision with its failed gates, provenance hashes, and explicit nonclaims.

An actionable gap retains its exact selector, step, or finalization coordinate
and its cause must match the trace at that coordinate. Only a gap at the first
unproven prefix boundary counts. An output-budget gap is actionable only after
every attempted step has an admitted trace and finalization names the matching
receipt/projection budget state. A product disposition whose reported gap cause
contradicts that trace remains measurable and fails the product-disposition hard
gate; the harness does not repair it into an actionable gap.

## Read-only verification

Verification reads the exact nine-file directory, recomputes corpus, path,
threshold, results, aggregate, and decision bindings, including the same
three-way oracle/indexed/observed file-hash comparison, and writes nothing:

```sh
"$qualification_bin" verify \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --results "$results_root/$qualification_id"
```

This is the complete A/B/C verification contract. There is no runtime input for
Outcome D. The result directory basename, public environment, summary, and
verification identity must all equal the same qualification ID. The directory
still contains exactly the nine artifacts listed above.

## Interpretation

The qualification covers an ordered, direct, outgoing, indexed source-level
call trail over the frozen corpus. It does not prove runtime execution, temporal
order, arbitrary reachability, ownership, data flow, extraction completeness, or
subsystem non-participation. A successful benchmark run is source-built evidence
for the dark kernel. It is not installed-host qualification, a public v3 switch,
or release evidence.
