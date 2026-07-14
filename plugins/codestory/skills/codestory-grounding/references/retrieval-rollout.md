# Retrieval Rollout Verification

Use this when a change touches retrieval coverage, sidecars, runtime integration,
CLI retrieval/search output, benchmarks, or the `retrieval-sidecar-smoke` CI
workflow. The goal is to choose the proof that establishes each layer is
trustworthy; running retrieval alone is not enough.

## Decision Table

| Rollout layer | Trustworthy proof | Run when | Does not prove |
| --- | --- | --- | --- |
| Indexer coverage | `cargo test --locked -p codestory-indexer --test fidelity_regression`; `cargo test --locked -p codestory-indexer --test tictactoe_language_coverage`; targeted `files` or `affected` checks for changed paths | Parser, tree-sitter, semantic-resolution, symbol, edge, file-role, or coverage changes | Sidecar readiness, runtime packet behavior, or CLI search contract |
| Retrieval sidecar crate | `cargo test --locked -p codestory-retrieval`; then live `retrieval bootstrap`, `retrieval index --project <repo> --refresh full`, and `retrieval status --project <repo> --format json` reporting `retrieval_mode="full"` plus current `symbol_doc_count`, `dense_projection_count`, `semantic_policy_version`, `graph_artifact_hash`, and dense reason counts | SQLite lexical, Qdrant, SCIP, manifest generation, sidecar status, symbol-doc virtual docs, dense-anchor policy, embedding backend/dim, or Qdrant client changes | Runtime admission, stdio cache invalidation, or full CLI output shape |
| Runtime integration | `cargo test --locked -p codestory-runtime --lib`; `cargo test --locked -p codestory-runtime --test retrieval_generalization_guard`; `cargo test --locked -p codestory-runtime --test retrieval_eval`; set `CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1` only after real sidecars are prepared | Packet/search orchestration, fail-closed modes, retrieval shadow traces, rollback-warning logic, or runtime use of sidecar results | CLI argument/output behavior or GitHub smoke workflow behavior |
| CLI surface | `cargo test --locked -p codestory-cli --test retrieval_bootstrap_contracts`; `cargo test --locked -p codestory-cli --test stdio_protocol_contracts`; `cargo test --locked -p codestory-cli --test search_json_output`; with real sidecars, run the ignored full-mode search JSON test explicitly | `retrieval bootstrap/status/index` contracts, stdio protocol/cache fingerprints, fail-closed search JSON, or user-facing command shape | Full product readiness unless `retrieval status` is `full` after live sidecar indexing |
| Benchmark harness | `cargo check --locked -p codestory-bench --benches`; the relevant Criterion bench only when it isolates the hot path; release e2e stats for real-repo timing; for AST-first retrieval, include same-run baseline/candidate rows for cold total index time, `semantic_embedding_ms`, dense doc count reduction, repeat refresh embedded-doc count, holdout MRR@10/Hit@10/exact-symbol Hit@1, packet lazy-search source reads, and peak descendant working set | New benchmark code, latency/timing claims, rollback baseline updates, dense-policy changes, or performance-sensitive retrieval/index changes | Promotion by itself; synthetic or narrow benches are scouts until real-repo evidence exists |
| Smoke CI | `.github/workflows/retrieval-sidecar-smoke.yml` plus `docs/ops/retrieval-sidecars.md#preflight-smoke-contract` pass criteria | PRs touching retrieval crate, runtime/stdio/search wiring, indexer retrieval hooks, retrieval docs, scripts, Docker sidecar config, or the workflow | Full sidecar readiness. CI smoke uses `--skip-compose --wait-secs 0` and proves manifest-missing fail-closed shape only |

## Agent-Grounding Release Gates

Use the highest completed tier as the only claim level in docs, PRs, or final
handoffs:

| Tier | Required evidence | Claim boundary |
| --- | --- | --- |
| CodeStory self-e2e | Generalization lint, targeted runtime/indexer tests, release CLI build, `doctor`, and repo-scale e2e stats | This branch still works on CodeStory and product Rust has no banned holdout literals |
| Local-real drill suite | Self-e2e plus local-real packet/drill rows without skip allowances | Product tuning survived realistic local repos |
| Holdout-retrieval drill suite | Local-real plus materialized holdout-retrieval rows, required recall/quality thresholds, and forbidden-claim checks with no skip allowances | Retrieval behavior is generalized for the public holdout suite |
| Promotion-grade paired benchmark | Holdout plus repeated CodeStory/no-CodeStory rows, timing/cost accounting, answer-quality ledger classifications, and packet-first source-read avoidance checks | Useful-for-agents, speed, or savings claims |

Packet statuses (`sufficient`, `partial`, `blocked`) describe evidence coverage
only. Final answer quality is promoted only by `drill`/`drill-suite` ledger
classifications. Holdout literals belong in manifests, tests, benchmark
harnesses, or the `CODESTORY_EVAL_PROBES` eval module, not production
planner/ranker/runtime code.

## CI Smoke Triage

The Windows `retrieval-sidecar-smoke` workflow is intentionally reduced. It
should fail if the manifest-missing status shape, production-path lint,
runtime/stdio/search contracts, or retrieval crate contracts drift. It should
not be "fixed" to pretend that full retrieval is available on the runner.

| Failure | First triage step | Expected boundary |
| --- | --- | --- |
| Generalization lint fails | Check `scripts/lint-retrieval-generalization.mjs` and production retrieval paths for hard-coded fixture or repo-specific assumptions | Fix source/docs drift before changing tests |
| `retrieval_bootstrap_contracts` fails | Inspect the clean pre-index status JSON; it should report `degraded_reason="retrieval_manifest_missing"` and non-`full` mode | Passing smoke still means no real sidecars, no GGUF fetch, and no manifest |
| Runtime or stdio tests fail | Localize to runtime admission, rollback warning, stdio cache fingerprint, or search JSON fail-closed behavior | Do not use repo-text fallback as success evidence |
| A reviewer asks for full-mode proof | Prepare real sidecars, fetch the GGUF model, run `retrieval index`, then require `retrieval status` to report `retrieval_mode="full"` | The CI smoke job is not the full-mode proof |

## Qdrant And Indexing Symptoms

Treat non-`full` retrieval modes as diagnostic only. Product packet/search
evidence is trustworthy only after live sidecars are indexed and status is full.

| Symptom | Likely layer | Action |
| --- | --- | --- |
| `retrieval_manifest_missing` | Bootstrap/state exists but no project manifest was finalized | In CI smoke this is expected. For product proof, run live `retrieval index --refresh full` and recheck status |
| `sidecar_manifest_stale`, input-hash drift, policy-version drift, graph-artifact-hash drift, dense-reason drift, or embedding-backend drift | Source, SQLite projection, `symbol_search_doc`, dense anchors, backend, dimension, policy, or schema changed after the manifest | Rerun `retrieval index --refresh full`; `--refresh auto` may repair stale stored symbol-doc or dense-anchor contracts once, but explicit failures still fail closed |
| `no_semantic`, `lexical_only`, or `unavailable` with Qdrant errors while dense anchors are expected | Qdrant, embedding endpoint, or semantic smoke failed | Run bootstrap, then use status to confirm the profile-selected Qdrant and embedding endpoints. Local defaults use Qdrant ports `6333`/`6334`; managed agent profiles allocate and persist dynamic ports. Rebuild sidecar indexes after endpoint health is restored. |
| Qdrant skipped while manifest dense-anchor count is `0` | Expected `graph_first_v1` graph/lexical full mode | Verify the SQLite lexical shard and SCIP are healthy and manifest symbol-doc count, policy version, graph hash, and dense reason counts match |
| Qdrant collection exists but point count is below the dense-anchor projection count, is one-point, or has a stub marker | Partial or obsolete collection | Rerun `retrieval index`; do not bless semantic smoke alone as full readiness |
| Qdrant response lacks `result.points[]` | Qdrant client/API contract drift or wrong image | Verify the pinned Qdrant image and update the client/test contract deliberately |
| `storage_repair.scan_errors` appears during bootstrap | Cache protection scan was incomplete | Resolve unreadable cache roots or DBs before relying on retention pruning; do not treat suppressed pruning as readiness proof |

## Repo-Scale E2E Stats Log

On the promoted final merge-ready head, run the release CLI e2e stats lane and
append the emitted headline metrics to `docs/testing/codestory-e2e-stats-log.md`
only when the changed behavior requires that lane:

```powershell
cargo build --release --locked -p codestory-cli
cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

This log is required on that promoted head for retrieval rollout changes that affect
default indexing, symbol-doc persistence, dense-anchor persistence or reuse,
sidecar indexing/status, packet/search behavior, runtime grounding surfaces, CLI
command shape, or any performance/timing claim. A stats-only row with
`CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES=1` can record local timing, but it
is not real-drill release evidence.
