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

## Interpretation

The qualification covers an ordered, direct, outgoing, indexed source-level
call trail over the frozen corpus. It does not prove runtime execution, temporal
order, arbitrary reachability, ownership, data flow, extraction completeness, or
subsystem non-participation. A successful benchmark run is source-built evidence
for the dark kernel. It is not installed-host qualification, a public v3 switch,
or release evidence.
