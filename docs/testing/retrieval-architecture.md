# Sidecar retrieval — architecture and promotion guide

**Audience:** Evidence record — not an install guide.

Sidecar-primary packet retrieval (project-local SQLite FTS lexical, optional Qdrant dense anchors, SCIP graph) orchestrated by
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

## Published Schema and Evidence Examples

These are the safe public surfaces for agents, docs, and PR evidence. Prefer
these DTOs and status fields over internal SQL tables, cache paths, sidecar
payloads, dense vectors, or generated summaries.

| Need | Safe surface | Owner boundary |
|------|--------------|----------------|
| Local graph shape | `NodeId`, `EdgeId`, `NodeKind`, `EdgeKind`, `ResolutionCertainty` in `crates/codestory-contracts/src/graph.rs`; route DTOs in `crates/codestory-contracts/src/api/dto.rs` | Store-local ids and extracted graph facts. `NodeId` is not a cross-repo identity; use `canonical_id`, file path, or qualified name for durable references. |
| Project/index readiness | `ProjectSummary`, `StorageStatsDto`, `IndexFreshnessDto`, `ReadinessVerdictDto`, `ReadinessSidecarSnapshotDto` | Runtime truth for this workspace and this invocation. A fresh local graph does not imply packet/search readiness. |
| Support tier | `IndexedFileLanguageCountDto` plus `LANGUAGE_SUPPORT_PROFILES` in `language_support.rs` | Language claim tier only: parser-backed graph or structural source-proof. It does not claim typed semantic edges or packet answer quality. |
| Search and packet evidence | `SearchHit`, `AgentCitationDto`, `PacketEvidenceTierDto`, `PacketEvidenceResolutionDto`, `coverage_role`, `eligible_for_sufficiency` | Role-bearing evidence for packet sufficiency. Ranking fields explain why a hit appeared; they do not prove sufficiency alone. |
| Sidecar freshness | `retrieval status --format json` `retrieval_mode`, `degraded_reason`, and `manifest_contract`; persisted source is `RetrievalIndexManifest` | Full sidecar retrieval requires a current manifest contract for the same source root, input hash, schema, generation, graph hash, and counts. |
| Blocked retrieval | `agent preflight` `safe_surfaces`, `blocked_surfaces`, `repair_command`; API error `retrieval_unavailable` with recovery commands | Fail closed for `packet`, `search`, and `context`; continue only with allowed local graph surfaces. |

Local graph evidence example:

```json
{
  "node_id": "2754485059345548338",
  "node_ref": "crates/codestory-contracts/src/api/dto.rs:43:ProjectSummary",
  "display_name": "ProjectSummary",
  "kind": "STRUCT",
  "evidence_tier": "resolved_graph",
  "resolution_status": "resolved",
  "eligible_for_sufficiency": true
}
```

Support tier example:

```json
{
  "language": "rust",
  "file_count": 118,
  "support_mode": "parser_backed_graph",
  "evidence_tier": "graph_fidelity",
  "claim_label": "parser-backed graph, fidelity-gated"
}
```

Sidecar manifest example:

```json
{
  "retrieval_mode": "full",
  "degraded_reason": null,
  "manifest_contract": {
    "source_root": "/repo",
    "project_id": "1a2b3c",
    "input_hash": "sha256:...",
    "generation": "project-input",
    "schema_version": 3,
    "graph_hash": "sha256:...",
    "symbol_doc_count": 2213,
    "dense_anchor_count": 0,
    "degraded_modes": [],
    "retrieval_mode": "full",
    "degraded_reason": null,
    "lanes": [
      { "lane": "graph", "status": "ready", "count": 2213 }
    ]
  }
}
```

Packet evidence example:

```json
{
  "display_name": "sidecar_retrieval_primary_enabled",
  "file_path": "crates/codestory-runtime/src/agent/retrieval_primary.rs",
  "line": 74,
  "evidence_tier": "resolved_graph",
  "evidence_producer": "route_endpoint",
  "resolution_status": "resolved",
  "coverage_role": "runtime_orchestration",
  "eligible_for_sufficiency": true
}
```

Blocked retrieval example:

```json
{
  "usable": false,
  "local_graph": { "ready": true, "status": "ready" },
  "full_retrieval": {
    "ready": false,
    "status": "repair_retrieval",
    "summary": "Full retrieval is unavailable for agent packet/search."
  },
  "safe_surfaces": ["ground", "files", "symbol", "trail", "snippet", "affected"],
  "blocked_surfaces": ["packet_full", "search_full", "context_full"],
  "repair_command": "codestory-cli ready --goal agent --repair --project \"<repo>\" --format json"
}
```

Non-claims:

- `semantic_suggestion`, `dense_semantic`, repo-text, generated summary, and
  synthetic source-scan evidence can help navigation, but they are not source
  truth without an exact source or resolved graph follow-up.
- `retrieval_mode=full` proves infrastructure eligibility only. It is not
  answer-quality, agent-usefulness, language-support, or public savings proof.
- A support tier row proves the named language/profile boundary only. It does
  not upgrade structural collectors to parser-backed graphs or parser-backed
  graphs to typed semantic edge coverage.
- A blocked `packet`, `search`, or `context` surface must not be worked around
  by treating legacy semantic fallback or local graph readiness as sidecar
  packet/search evidence.

## Implemented Stack

| Layer | Location | Role |
|-------|----------|------|
| Retrieval clients | `crates/codestory-retrieval/` (`lexical_client`, `qdrant_client`, `scip_client`, `health`) | SQLite FTS candidate selection, HTTP probes, staged search, timeouts |
| Planner / executor / ranker | `codestory-retrieval` (`planner`, `executor`, `ranker`, `query_features`, `mode`) | Repo-agnostic staged plan, deadlines, degraded modes |
| Index manifest | `codestory-store` `retrieval_index_manifest` + `codestory-retrieval::index` | Version pins, sidecar input hash, generation id, symbol-doc count, dense-anchor count, semantic policy version, graph artifact hash, dense reason counts, mandatory real sidecar artifact paths, and derived status `manifest_contract` provenance |
| CLI lifecycle | `codestory-cli` `retrieval up\|down\|status\|index\|query` | Local data dirs, health JSON, standalone query |
| Packet integration | `codestory-runtime/src/agent/retrieval_primary.rs` | Primary sidecar path, diagnostic traces, promotion warnings |
| Nucleo policy | `codestory-runtime/src/agent/nucleo_policy.rs` | Suppresses Nucleo O(n) scan on sidecar primary; disabled sidecars are not valid product evidence |
| Generalization lint | `scripts/lint-retrieval-generalization.mjs` | Derives banned identities, prompts, claims, paths, and query/probe phrases from benchmark manifests, script prompt/query catalogs, and the eval-only probe manifest/source, then scans Rust production retrieval trees after masking test-only items (CI via Rust guard test); missing, malformed, or partially parsed corpora fail closed |

All planned retrieval stages use the same fixed-capacity worker pool, including
symbol-like and natural-language queries. Each job carries the request deadline
and cancellation flag into sidecar calls. Lexical and SCIP scans poll that
context while iterating. Live query embedding and Qdrant requests clamp their
transport timeout to the remaining deadline and use separate bounded reusable
HTTP capacity, allowing the stage worker to return promptly when synchronous
I/O cannot be interrupted in place. Stage traces report admission and queue
wait separately, report execution duration only after completion, and classify
completed, skipped, cancelled-before-start, pending-after-deadline, and
observed-late work. Post-return completions are logged and discarded. Any cancelled or late
result remains diagnostic and is never inserted into the retrieval cache.

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
[language-expansion holdout stats](language-expansion-holdout-stats.md) and
contributor verification lanes
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
