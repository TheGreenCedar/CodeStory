# Proof availability qualification v1

This qualification measures whether CodeStory's dark, exact call-path kernel can
prove or explain the frozen source paths in the proof-availability corpus. It is
benchmark infrastructure. It does not register a product tool or make proof
public.

## Inputs and lifecycle

Use one locked `codestory-proof-availability` binary for both materialization and
the run. Materialization checks out the exact frozen commits, verifies every
oracle range, builds one fresh core index per project, and writes a private local
environment descriptor:

Before Q2, Q1 must prove that the evidence-only v3 surface is separable from
proof activation. The sealed conformance probe builds and validates packet,
context, and search results for every supported MCP revision while omitting the
proof tool, schema, and route. The companion feature-matrix build proves that
this surface compiles without the proof-qualification feature:

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
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- \
  materialize \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace target/proof-availability/workspace \
  --cache-root target/proof-availability/cache \
  --out target/proof-availability/environment.json
```

The source-only audit form is separate. It neither creates the cache nor indexes
or executes proof:

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
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- \
  run \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --environment target/proof-availability/environment.json \
  --out target/proof-availability/results
```

The command is the complete Q2 contract for Outcomes A, B, and C.

All destinations are no-replace. Choose new paths for a rerun. Failed
materialization or publication keeps its owner-marked staging path for manual
inspection; the command never recursively removes a path as rollback.

## What the run does

The run rechecks the binary, source commit and tree, oracle bytes, database hash,
store schema, and pinned core publication. It opens each store observationally,
derives inventory and edge-distinct trail counts, then sends every positive case
and its two frozen mutations through the accepted validator and the single
Runtime-owned core-pinned proof operation. No semantic retrieval publication is
created or required.

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

Verification reads the exact eight-file directory, recomputes corpus, path,
threshold, results, aggregate, and decision bindings, and writes nothing:

```sh
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- \
  verify \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --results target/proof-availability/results
```

This is the complete A/B/C verification contract. There is no runtime input for
Outcome D.

## Interpretation

The qualification covers an ordered, direct, outgoing, indexed source-level
call trail over the frozen corpus. It does not prove runtime execution, temporal
order, arbitrary reachability, ownership, data flow, extraction completeness, or
subsystem non-participation. A successful benchmark run is source-built evidence
for the dark kernel. It is not installed-host qualification, a public v3 switch,
or release evidence.
