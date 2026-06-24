# Sidecar retrieval — architecture and promotion guide

Sidecar-primary packet retrieval (Zoekt lexical, optional Qdrant dense anchors, SCIP graph) orchestrated by
`codestory-retrieval` and integrated in `codestory-runtime`. Production packet paths use
generic symbol/path roles; benchmark-only probe catalogs remain behind test-only eval harness hooks.
Sidecar retrieval is mandatory for current evidence; `CODESTORY_RETRIEVAL=0` is treated as a
configuration error, not a diagnostic route.

**Related:** [`../ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md#agent-readiness-repair) (setup,
agent readiness repair, env vars, CI smoke), [`../architecture/retrieval-design.md`](../architecture/retrieval-design.md)
(mode definitions, cost envelopes, promotion guards).

---

## Role-Bearing Packet Sufficiency Contract

Packet sufficiency is claim-role based, not citation-count based. Packet
citations may expose `evidence_tier`, `evidence_producer`,
`resolution_status`, `coverage_role`, and `eligible_for_sufficiency`; only
role-aligned citations can close the matching coverage role.

Proof-bearing packet roles require resolved or role-aligned source, graph,
lexical, symbol-doc, or component evidence. Dense semantic hits, generated
summaries, repo-text, and generic synthetic source scans are diagnostic unless a
runtime policy admits a specific structural/source-shape role. Generic `source
evidence` is not enough to mark a packet sufficient.

`retrieval_mode=full` is mandatory infrastructure readiness for product packet
paths. It does not promote answer quality, agent usefulness, or public language
quality without the packet-runtime or drill evidence required by the proof tier.
`retrieval status --format json` may include `manifest_contract`; that contract
proves the sidecar manifest's source root, input hash, generation, schema,
counts, degraded modes, and lane provenance. It is readiness/freshness evidence
only. Packet-quality promotion still needs role-bearing packet sufficiency plus
the matching drill or benchmark tier.

## Implemented Stack

| Layer | Location | Role |
|-------|----------|------|
| Sidecar clients | `crates/codestory-retrieval/` (`zoekt_client`, `qdrant_client`, `scip_client`, `health`) | HTTP probes, staged search, timeouts |
| Planner / executor / ranker | `codestory-retrieval` (`planner`, `executor`, `ranker`, `query_features`, `mode`) | Repo-agnostic staged plan, deadlines, degraded modes |
| Index manifest | `codestory-store` `retrieval_index_manifest` + `codestory-retrieval::index` | Version pins, sidecar input hash, generation id, symbol-doc count, dense-anchor count, semantic policy version, graph artifact hash, dense reason counts, mandatory real sidecar artifact paths, and derived status `manifest_contract` provenance |
| CLI lifecycle | `codestory-cli` `retrieval up\|down\|status\|index\|query` | Local data dirs, health JSON, standalone query |
| Packet integration | `codestory-runtime/src/agent/retrieval_primary.rs` | Primary sidecar path, diagnostic traces, promotion warnings |
| Nucleo policy | `codestory-runtime/src/agent/nucleo_policy.rs` | Suppresses Nucleo O(n) scan on sidecar primary; disabled sidecars are not valid product evidence |
| Generalization lint | `scripts/lint-retrieval-generalization.mjs` | Bans repo literals in Rust production retrieval trees (CI via Rust guard test); benchmark/eval harness scripts and `codestory-runtime/src/agent/eval_probes.rs` may name holdout repos only inside their manifest/eval boundary |

**Modes:** See the canonical
[mode matrix](../architecture/retrieval-design.md#mode-matrix). Only `full` may
serve primary packet/search results.

**Benchmark manifests:** `benchmarks/tasks/local-real/` is the realistic local
product corpus; `benchmarks/tasks/holdout-retrieval/` is the public
generalization corpus. Holdout rows are promotion evidence only, not a tuning
loop.

## Proof tiers and claims

Do not describe a branch as generalized or useful for agents until the matching
proof tier has run cleanly on the current branch. Docs and PRs must state only
the highest tier actually reached:

| Tier | Proof | Claim allowed |
|------|-------|---------------|
| 1. CodeStory self-e2e | Generalization lint, targeted runtime/indexer tests, release CLI build, `doctor`, and repo-scale e2e stats | CodeStory still works on itself and production code has no banned holdout literals |
| 2. Local-real drill suite | Tier 1 plus local-real packet/drill rows with no skip allowances | Product tuning survived realistic local repos |
| 3. Holdout-retrieval drill suite | Tier 2 plus holdout-retrieval materialized repos, no skip allowances, required recall/quality thresholds, and forbidden-claim checks | Retrieval behavior is generalized enough for the public holdout suite |
| 4. Promotion-grade paired benchmark | Tier 3 plus repeated paired CodeStory/no-CodeStory rows, quality gates, timing/cost accounting, and source-read avoidance checks | Promotion language about agent usefulness, speed, or savings |

`packet` status is evidence sufficiency, not final answer quality. Only
`drill`/`drill-suite` rows with ledger classifications can promote answer
quality. Packet-first runs count as agent-useful only when packets marked
`sufficient` avoid post-packet source reads, or when those reads are explicitly
classified as source-truth follow-up rather than hidden grounding.

## Environment and setup

Version pins, env vars, bootstrap commands, troubleshooting, and CI smoke
sequences are owned by
[`retrieval-sidecars.md`](../ops/retrieval-sidecars.md#agent-readiness-repair). AST-first policy gates
and dense-anchor promotion fields are summarized there and in
[`retrieval-design.md`](../architecture/retrieval-design.md#ast-first-semantic-contract).

Benchmark-only flag: `CODESTORY_EVAL_PROBES` is ignored in production runtime
and must stay test-only.

---

## Local test workflows

Local-real repo manifests live under `benchmarks/tasks/local-real/`.

```sh
node scripts/codestory-agent-ab-benchmark.mjs \
  --packet-runtime --packet-runtime-mode cold-cli \
  --task-suite local-real --repeats 1 \
  --out-dir target/agent-benchmark/packet-runtime-sidecar-promotion \
  --codestory-cli target/release/codestory-cli \
  --timeout-ms 300000
```

Local-real rows are product-development evidence, not public savings claims by
themselves. They need repeated quality-gated runs against clean pinned checkouts
before promotion language.

### holdout-retrieval (generalization)

```sh
node scripts/fetch-holdout-repos.mjs
# or:
node scripts/codestory-agent-ab-benchmark.mjs \
  --list --task-suite holdout-retrieval --materialize-repos

node scripts/codestory-agent-ab-benchmark.mjs \
  --packet-runtime --packet-runtime-mode cold-cli \
  --task-suite holdout-retrieval --materialize-repos \
  --repeats 1 \
  --out-dir target/agent-benchmark/holdout-retrieval-smoke \
  --codestory-cli target/release/codestory-cli \
  --timeout-ms 180000
```

Holdout failures should block promotion or trigger diagnosis; do not add
repo-name/path literals or tune planner/ranker heuristics against holdout rows.
The generalization lint currently fails production Rust on holdout names and
anchors such as repository names, specific source paths, and manifest-specific
symbols. Keep those strings in manifests, tests, benchmark harnesses, or the
test-only eval probe module.

## Required Checks

```sh
cargo test -p codestory-retrieval
cargo test -p codestory-runtime --test retrieval_generalization_guard
node --test scripts/tests/codestory-agent-ab-analyzer.test.mjs
```

Optional broader lane:

```sh
cargo test -p codestory-runtime
node --test scripts/tests/codestory-agent-ab-analyzer.test.mjs
```

---

## Promotion Gates

This page defines the gates; dated pass/fail rows live in
[`benchmark-ledger.md`](benchmark-ledger.md) and contributor verification lanes
live in [`testing-matrix.md`](../contributors/testing-matrix.md).

Support claims must be backed by committed benchmark manifests, generated
artifacts, or tests in the branch. Do not infer support for languages without
direct benchmark artifacts.

A branch may claim only the highest gate that has current evidence:

| Gate | Required evidence |
|------|-------------------|
| Stack shape | Implemented sidecar stack, design doc, sidecar runbook, manifests, warning config, and CI smoke contract exist and are linked from this repo. |
| Self-e2e | Generalization lint, guard test, release CLI build, `doctor`, and repo-scale e2e stats pass on the branch. |
| Local-real | Local-real packet/drill rows run against pinned repos with sidecars and no skip allowances. |
| Holdout generalization | Holdout-retrieval suite runs against materialized OSS repos with required recall/quality thresholds and no forbidden-claim failures. |
| Promotion-grade | Repeated paired CodeStory/no-CodeStory rows include quality gates, timing/cost accounting, and source-read avoidance checks. |

### North-Star SLOs

| Metric | Target |
|--------|--------|
| Cold CodeStory product index | under 180 s |
| Cold semantic embedding time | at least 70% lower than same-run baseline |
| Dense embedded docs | at least 65% lower than same-run baseline |
| Repeat full refresh | 0 unchanged dense docs embedded and under 25 s |
| Holdout MRR@10 | no more than 1 percentage-point drop versus same-run baseline |
| Hit@10 / exact-symbol Hit@1 | no regression |
| Retrieval p50 | ≤ 250 ms |
| Retrieval p90 | ≤ 600 ms |
| Retrieval p99 | ≤ 1,000 ms |
| Worst-case packet wall | ≤ 1,500 ms |
| local-real quality pass | ≥ 3/4 repos |
| `agent_value_gap` | &lt; 0.20 |
| holdout generalization | Required manifest thresholds across the full holdout-retrieval suite |
| Sidecar planner/ranker repo literals | 0 (lint clean) |

---

## Promotion Guard Evidence

After promotion runs, verify sidecar evidence and guard thresholds from the
benchmark artifacts:

1. Record `retrieval status --format json` and `doctor --format json` output from the promoted index.
2. Confirm packet/search evidence reports `retrieval_mode=full`.
3. Compare the run against the guard thresholds in `docs/architecture/retrieval-rollback.json`; the file stores promotion guard thresholds, not runtime rollback behavior.

**Promotion note:** Local `retrieval status` can report `full` after Qdrant
re-index. Sidecar-primary is the intended product path, but product promotion
still requires fresh benchmark evidence.

---

## Spec and design references

| Doc | Path |
|-----|------|
| Design | [`docs/architecture/retrieval-design.md`](../architecture/retrieval-design.md) |
| Operations | [`docs/ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md) |
| Promotion guard config | [`docs/architecture/retrieval-rollback.json`](../architecture/retrieval-rollback.json) |
