# Performance Review Playbook

**Audience:** Evidence record — not an install guide.

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
| Cache state | Cold cache, warm cache, incremental refresh, full retrieval, or lexical-only diagnostic. |
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

The in-process batching contract is fixed for product comparisons. Profile
batch internals only after the baseline shows engine saturation rather than
graph/store contention, and do not promote a runtime tuning switch.

Do not collapse these into one "index got faster/slower" claim unless the
repo-scale e2e row shows the same project, cache state, semantic backend, and
command flags before and after.

Prefer existing gates before adding a new harness:

```sh
export CODESTORY_EMBED_MODEL_SOURCE="$(node scripts/prepare-embedded-model.mjs)"
cargo build --release --locked -p codestory-cli
cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
cargo test --locked -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo check --locked -p codestory-bench --bench <name>
```

Bench targets opt out of broad workspace test selection, so compile or run them
with an explicit `--bench <name>` target.

Use Criterion benches from `crates/codestory-bench` only when the measured hot
path is narrower than the repo-scale e2e test can explain.

## Current Ops Gates

Keep performance/scale/ops proof split by lane. A timing row can show trend or
regression risk, but it is not answer-quality proof.

| Gate | Current metric or threshold | Command that proves it | Source |
| --- | --- | --- | --- |
| Repeat refresh | Promoted stats require `repeat_semantic_docs_embedded == 0` and record wall-clock telemetry with living-baseline warnings. Release evidence separately requires repeat graph `< 20s`, repeat semantic reuse `< 3s`, and full-refresh convergence within the approved machine-profile budget. | Set `CODESTORY_EMBED_MODEL_SOURCE` to the output of `node scripts/prepare-embedded-model.mjs`, run `cargo build --release --locked -p codestory-cli`, then run `cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture` for correctness and telemetry; use `scripts/codestory-release-evidence-gate.mjs` for hardware-bound timing proof. | `crates/codestory-cli/tests/codestory_repo_e2e_stats.rs`, `scripts/codestory-release-evidence-gate.mjs`, `benchmarks/release-evidence/repo-stats-contract.json`, `benchmarks/release-evidence/approved-baselines.json` |
| Retrieval status | After retrieval indexing, `retrieval_mode == "full"` and `retrieval status --format json` reports current manifest provenance: source root, input hash, generation, schema, graph hash, symbol-doc count, dense-anchor count, degraded modes, and engine identity. Non-`full` status is diagnostic only. | `codestory-cli retrieval index --project <repo> --refresh full --format json`; `codestory-cli retrieval status --project <repo> --format json` | `docs/ops/retrieval-engine.md`, `crates/codestory-retrieval/src/sidecar.rs`, `crates/codestory-runtime/src/agent/retrieval_primary.rs` |
| Packet runtime | Product retrieval query budget defaults to `1,500ms`; packet batch budget defaults to `18,000ms` and is capped at `120,000ms`; packet runs must report `packet_latency.sla_missed == false` for product evidence. North-star targets are retrieval p50 `<= 250ms`, p90 `<= 600ms`, p99 `<= 1,000ms`, and worst-case packet wall `<= 1,500ms`, but those targets become promotion proof only inside a quality-gated benchmark run. | `node scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite local-real --repeats 1 --codestory-cli target/release/codestory-cli --timeout-ms 300000` | `crates/codestory-runtime/src/agent/retrieval_primary.rs`, `crates/codestory-retrieval/src/planner.rs`, `scripts/codestory-agent-ab-benchmark.mjs`, `docs/testing/retrieval-architecture.md` |
| Benchmark promotion | `--publishable` requires at least 3 repeats, full retrieval, no diagnostic extra probes, no failed rows, token usage, clean preludes, manifest quality gates when present, packet-first compliance, sufficient packets with no unresolved diagnostics, and the explicit `--max-source-reads-after-packet` budget. Holdout/local task quality thresholds live in the task manifests; stats-log timing rows do not promote answer quality. | `node scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --packet-runtime-mode cold-cli --task-suite holdout-retrieval --materialize-repos --repeats 3 --publishable --max-source-reads-after-packet 0 --codestory-cli target/release/codestory-cli --timeout-ms 180000` | `scripts/codestory-agent-ab-benchmark.mjs`, `scripts/codestory-benchmark-contract.mjs`, `benchmarks/tasks/`, `docs/testing/retrieval-architecture.md` |

Current telemetry snapshot from `docs/testing/codestory-e2e-stats-log.md`
(2026-06-18 `d8d59e9e+wt`, #41 hardening row): `retrieval_mode full`,
`retrieval_index_seconds 4.34`, `retrieval_status_seconds 0.39`, repeat full
refresh `29.45s` with `750` reused and `0` embedded, index `75.36s`, semantic
phase `49.45s`. This row is useful regression telemetry; it does not prove
answer quality because the real drill was intentionally skipped.

## Release Evidence Gate

The stats log is append-only telemetry, not a baseline selector. Release
candidates must use a named machine profile from
`benchmarks/release-evidence/approved-baselines.json`; the profile pins its
accepted commit, approval rationale, source artifacts, and a separate budget
for status, local grounding, convergence, packet, search, indexing, and
same-corpus storage growth. This avoids applying one timing to dissimilar
machines while preventing a slower appended row from becoming its own
reference.

Do not hand-author candidate metrics. The provisioned
`release-candidate-evidence.yml` workflow runs the full repo-scale and
publishable packet producers on the same clean SHA, records the corpus, cache,
and machine fingerprint, hashes both non-empty raw artifacts, and derives the
candidate. It refuses contract-only profiles during release runs. The release
workflow calls this gate after preflight and does not start packaged proof until
the evidence decision passes.

Provision, verify, recover, and unregister the dedicated Linux host using the
[release-evidence runner runbook](../contributors/release-evidence-runner.md).
Its fingerprint combines a stable profile ID with the checked-in machine
contract hash. The workflow accepts that fingerprint only after the live guest
matches the current-boot host attestation and pinned package/toolchain state.

For maintainer reproduction, produce and evaluate from real raw artifacts:

```sh
node scripts/codestory-release-evidence-gate.mjs produce \
  --baseline benchmarks/release-evidence/approved-baselines.json \
  --profile <approved-release-profile> \
  --stats target/release-evidence/stats.json \
  --packet target/release-evidence/packet/packet-runtime-summary.json \
  --out target/release-evidence/candidate.json \
  --expected-sha <full-40-character-sha> --mode release --repo .
node scripts/codestory-release-evidence-gate.mjs evaluate \
  --baseline benchmarks/release-evidence/approved-baselines.json \
  --candidate target/release-evidence/candidate.json \
  --out target/release-evidence/decision.json \
  --expected-sha <full-40-character-sha> --mode release --repo .
```

Production and evaluation both reject missing, empty, changed, or all-zero raw
artifacts; short or dirty Git identities; self-baselining; identity drift; and
unit or aggregation changes. The normalized report records the candidate and
baseline hashes, full commits, artifact hashes and sizes, and every metric's
status and decision. A regression exits nonzero. Each exception is bound to the
exact candidate hash, baseline id/hash, full commit, profile, metric, measured
value, threshold, owner, ISO approval date, rationale, unexpired date, and
rollback evidence under approval schema v3. Accepted exceptions remain
`pass_with_exception` in the evidence row, claim, evaluation, and report.
Approval never updates the pinned baseline or converts the regression to plain
pass.

Packet provenance is finalized only after publishable blockers are calculated.
The evaluator does not trust its status label: it requires an empty blocker
list and independently checks every embedded row for pass/quality/sufficiency,
full retrieval shadow, SLA, the benchmark's shared pinned-repository and
local-cache provenance validators, and distinct exact `1..N` repeats matching
the top-level modes/repeat contract. It also
rechecks the raw stats object's `full_retrieval` tier, index/ground/search modes,
manifest counts/hash/policy, zero index errors, and zero repeat embeddings.
The promoted Rust stats run treats wall-clock variation as telemetry and emits
living-baseline warnings while retaining those correctness assertions. The
hardware-bound release-evidence gate separately re-evaluates repeat graph,
semantic, and full-refresh limits from `repo-stats-contract.json` before an
artifact can become release-eligible.

On rejection, the workflow uploads provisioning, raw, candidate, approval (when
provided), and report files with `if: always()`. Author an exception against the
reported hashes and values in the short-lived repository Actions secret
`CODESTORY_RELEASE_EVIDENCE_APPROVAL_JSON`. The release coordinator passes only
that named secret into the reusable job behind the protected
`release-evidence` environment gate; PR packaging receives none. Dispatch the
Release workflow on the same SHA and version with
`source_run_id=<rejected-run-id>`. That path downloads and re-evaluates the
exact candidate without re-running measurements; any SHA, hash, value, profile,
baseline, threshold, date, or expiry drift still fails.

The checked-in `ci-contract-v1` fixture and report exercise this trust chain in
ordinary CI but are explicitly `release_eligible: false`. The release path uses
only a product profile created from provisioned raw evidence and explicitly
approved in the baseline document.

The corpus boundary is deliberately precise. The generalization lint scans Rust
production files in all non-benchmark crates for copied corpus content, direct
paths, and adjacent/split literal construction. It also scans the following
repository-controlled non-Rust surfaces for direct and adjacent/split
dependencies on every inventoried evaluation/query corpus:

The inventory is executable rather than documentation-only. Supported text and
configuration files under `scripts/`, `.github/scripts/`,
`.github/workflows/`, the shipped plugin, and native backend metadata enter the
protected scan by default. The tracked Codex
environment definition is a required protected file. Only the explicit
corpus/proof harness list, the named release-detector unit-test harness, and
test/fixture directories are excluded; there are no blanket test-filename
exceptions. Missing required protected or harness paths fail the lint so moves
require review.

| Classification | Protected or allowed surface | Static contract |
| --- | --- | --- |
| Product launch and setup | Plugin manifests, hooks, MCP launcher, grounding skill guidance/setup scripts, Codex environment/worktree setup, and the Windows installer | Protected; the configured directories are scanned recursively, while test and fixture directories are excluded. |
| Runtime configuration | Native retrieval backend and model metadata | Protected; new supported text/config files under those directories enter the scan automatically. |
| Release control | Release/auto-release workflows, version detection/checking, package assembly, the release-evidence evaluator, and shared provenance validation | Protected; required files must exist, and protected non-Rust modules cannot import an explicitly classified corpus/proof harness module. |
| Explicit corpus and test harnesses | Task manifests, packet/repo benchmark drivers and scorers, holdout provisioning, release-candidate measurement, the retrieval-engine contract workflow, the release-detector unit test, and test fixtures | Allowed to load evaluation corpora or exercise protected code; these paths are evidence producers/tests, not product or release-decision logic. Other scripts and workflows remain protected by default. |
| Environment and generated inputs | Environment values, generated evidence, and workflow artifacts | Static code paths are protected where listed above. Runtime values and generated bytes require the existing schema, hash, identity, and provenance checks. |
| External processes | Git and agent executables | Static command construction is protected where it is part of a listed surface. The external executable's internal behavior is outside this repository scan. |

The release evaluator now imports repository/cache provenance checks from
`scripts/codestory-evidence-provenance.mjs`, a corpus-neutral module shared with
the benchmark harness. It no longer loads the packet benchmark module—and its
query catalog—as a transitive release-control dependency.

This guard does not prove arbitrary runtime string generation, environment
values, generated content, or external-process behavior. Those boundaries stay
fail closed through the release-evidence attestation/runtime checks where a
machine-verifiable contract exists; otherwise they remain explicit non-claims.

Do not promote importable or rebuildable graph/retrieval artifacts in this slice.
A follow-up PR for that idea must require provenance before reuse: source root,
commit or dirty-tree label, CodeStory CLI version, retrieval schema, retrieval input
hash, graph artifact hash, semantic policy version, embedding backend/dim,
symbol-doc count, dense-anchor count, dense reason counts, lane artifact paths,
the exact rebuild command, and a fresh `retrieval status --format json` proof
showing the imported/rebuilt artifact is still `full`.

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
export CODESTORY_EMBED_MODEL_SOURCE="$(node scripts/prepare-embedded-model.mjs)"
cargo build --release --locked -p codestory-cli
cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Appending those rows is never a release decision. Assemble the candidate
artifact and run the release evidence gate after the raw producers complete.

### SQLite lexical validation baseline

The ignored lexical benchmark separates deep corpus validation from warm query
execution. On 2026-07-14, the macOS arm64 debug test binary reported:

| Documents | Deep validation median | SQLite warm query median | Legacy JSONL query median |
| ---: | ---: | ---: | ---: |
| 1,000 | 3,292 us | 1,497 us | 2,447 us |
| 10,000 | 31,193 us | 3,838 us | 24,447 us |

Command:

```sh
cargo test -p codestory-retrieval --lib --locked report_jsonl_to_sqlite_corpus_and_query_delta -- --ignored --nocapture
```

These numbers are diagnostic, not unit-test thresholds. Query tests instead
prove that the hot path performs only immutable metadata validation; build,
publication, readiness, and explicit health checks own deep row validation.

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
