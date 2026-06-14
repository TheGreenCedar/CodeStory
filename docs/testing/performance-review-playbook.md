# Performance Review Playbook

This playbook covers CLI-first performance review, targeted parallelization, and
search-quality promotion for the navigation workflow. It is not a server, MCP,
watch, or transport playbook.

## Workflow Boundary

Use this when a change affects one of these CLI paths:

- `index`: graph phase, semantic phase, semantic-doc reuse, search-doc writes.
- `ground`, `search`, `explore`, `context`, `files`, or `affected`: warm read
  latency, repo-text fallback, JSON/Markdown rendering, graph traversal, and
  route/coverage notes.
- route coverage and search evals: route discovery, handler query ranking,
  fallback source, recall, MRR, and latency.

Do not start with a concurrency change. Start with a baseline that proves which
path is slow.

## Baseline Capture

Before proposing an optimization, record:

| Field | Required evidence |
| --- | --- |
| Command | Exact command line, including `--project`, `--refresh`, `--format`, and relevant environment variables. |
| Commit | Current commit or working-tree label. If the tree is dirty, say so. |
| Cache state | Cold cache, warm cache, incremental refresh, full sidecar, lexical-only diagnostic, hash semantic diagnostic, ONNX diagnostic, or external embedding backend. |
| Sample size | Number of runs and whether the first run was discarded. |
| Headline metric | Index seconds, graph phase seconds, semantic phase seconds, per-command seconds, p95/max latency, or benchmark score. |
| Dominant cost | Measured cost center: graph phase, semantic phase, store reads/writes, repo-text scan, source reads, graph traversal, search scoring, CLI rendering, lock contention, or memory pressure. |
| Quality guard | Search recall/MRR, expected anchors, route coverage status, semantic-doc reuse, or output-golden checks that must not regress. |

For `index` changes, split the comparison into graph and semantic subphases
before drawing conclusions:

| Phase field | Use |
| --- | --- |
| Graph phase | File discovery, parse/extract, graph writes, and snapshot/store refresh. |
| Search projection | Search projection rebuild and symbol-index write time. |
| Semantic doc build | Semantic document text construction, file text cache, and graph-context shaping. |
| Semantic embedding | Embedding backend wall time, batch/request shape, and request concurrency setting. |
| Semantic persistence | Semantic-doc upsert, reload, prune, reuse, pending, embedded, and stale counts. |

For llama.cpp, the default request count remains serial. Compare explicit
`CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=<n>` values or the opt-in
`CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT=auto` path only after the baseline
shows embedding backend saturation rather than graph/store contention.

Do not collapse these into one "index got faster/slower" claim unless the
repo-scale e2e row shows the same project, cache state, semantic backend, and
command flags before and after.

Prefer existing gates before adding a new harness:

```sh
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-runtime --test retrieval_eval
cargo check -p codestory-bench --benches
```

`retrieval_eval` needs `CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1` for full sidecar quality assertions;
without it, the suite checks that non-full retrieval fails closed.

Use Criterion benches from `crates/codestory-bench` only when the measured hot
path is narrower than the repo-scale e2e test can explain.

## Promotion Record

For every accepted performance change, record:

| Item | Rule |
| --- | --- |
| Before/after | Use the same project, cache state, semantic backend, command flags, and sample shape. |
| No-regression threshold | Define the threshold before measuring the candidate. Examples: no lost expected search anchors, no lower MRR unless documented, no higher max latency beyond fixture cap, no worse semantic-doc reuse for the same cache state. |
| Failure result | If the candidate misses the threshold, mark it rejected and keep the measured regression in the validation record or PR notes. |
| Scope | Tie the result to one path. Do not promote a search-speed win as an indexing win, or an indexing win as a route-quality win. |

Append repo-scale timing rows to
[codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) when default indexing,
semantic-doc persistence, embedding reuse, or cold-start behavior changes.
Before/after rows in that log require a serialized full ignored e2e run. If the
branch cannot run it yet, leave the log unchanged and put this exact deferred
verification plan in the PR or final notes:

```sh
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

For the CLI navigation branch baseline, see
[cli-navigation-next-wave-performance-review.md](cli-navigation-next-wave-performance-review.md).

## Parallelization Candidate Gate

Parallel or async work is allowed only after the baseline shows the exact path
is CPU-bound or I/O-bound and safely isolated.

Use this template before implementation:

| Field | Required answer |
| --- | --- |
| Candidate path | Exact crate/module/function or CLI command path being changed. |
| Bottleneck evidence | Measurement proving this path dominates user-visible time. |
| Work unit boundary | The smallest independent unit, such as file parse, source read, route fixture case, search query, or graph traversal shard. |
| Maximum concurrency | Fixed cap or clear derivation. Avoid unbounded task fan-out. |
| Ordering requirement | How output order, ranking ties, diagnostics, and JSON arrays remain deterministic. |
| Resource risk | Build locks, SQLite writer locks, search-index writer contention, memory pressure, embedding backend saturation, or filesystem contention. |
| Serial fallback | The current serial path must remain available or trivially recoverable. |
| Validation | Micro/bench result plus at least one CLI integration run with unchanged result quality. |

Rejected by default unless fresh evidence overturns prior regressions:

- broad semantic score parallelization
- broad async runtime migration
- cargo-wide concurrency in this repo
- parallel Cargo verification while measuring CodeStory performance

## Failure Path

Stop the optimization and diagnose the failing layer when:

- faster output loses expected anchors, route hits, or handler evidence
- MRR drops below the agreed threshold
- max or p95 latency worsens beyond the fixture cap
- semantic-doc reuse changes unexpectedly for the same cache state
- result ordering becomes nondeterministic
- build/cache/store locks dominate the timing
- memory pressure invalidates the benchmark

When this happens, record the rejected candidate with the command, metric, and
stop condition. The rejected row is useful evidence; do not bury it as a failed
attempt.
