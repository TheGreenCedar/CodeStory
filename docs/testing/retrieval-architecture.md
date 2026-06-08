# Sidecar retrieval — architecture and promotion guide

Sidecar-primary packet retrieval (Zoekt lexical, Qdrant semantic, SCIP graph) orchestrated by
`codestory-retrieval` and integrated in `codestory-runtime`. Production packet paths use
generic symbol/path roles; benchmark-only probe catalogs remain behind test-only eval harness hooks.
Sidecar retrieval is mandatory for current evidence; `CODESTORY_RETRIEVAL=0` is treated as a
configuration error, not a diagnostic route.

**Related:** [`../ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md) (operator runbook),
[`../architecture/retrieval-design.md`](../architecture/retrieval-design.md) (module contracts).

---

## Implemented stack (Phases 0–5)

| Layer | Location | Role |
|-------|----------|------|
| Sidecar clients | `crates/codestory-retrieval/` (`zoekt_client`, `qdrant_client`, `scip_client`, `health`) | HTTP probes, staged search, timeouts |
| Planner / executor / ranker | `codestory-retrieval` (`planner`, `executor`, `ranker`, `query_features`, `mode`) | Repo-agnostic staged plan, deadlines, degraded modes |
| Index manifest | `codestory-store` `retrieval_index_manifest` + `codestory-retrieval::index` | Version pins, sidecar input hash, generation id, and mandatory real sidecar artifact paths |
| CLI lifecycle | `codestory-cli` `retrieval up\|down\|status\|index\|query` | Local data dirs, health JSON, standalone query |
| Packet integration | `codestory-runtime/src/agent/retrieval_primary.rs` | Primary sidecar path, diagnostic traces, promotion warnings |
| Nucleo policy | `codestory-runtime/src/agent/nucleo_policy.rs` | Suppresses Nucleo O(n) scan on sidecar primary; disabled sidecars are not valid product evidence |
| Generalization lint | `scripts/lint-retrieval-generalization.mjs` | Bans repo literals in Rust production retrieval trees (CI via Rust guard test); benchmark/eval harness scripts may name holdout repos only inside their manifest/eval boundary |

**Modes:** `full`, `no_scip`, `no_semantic`, `lexical_only`, `unavailable` — only
`full` may serve primary packet/search results. All non-`full` modes fail closed. See
[`retrieval-design.md`](../architecture/retrieval-design.md#mandatory-sidecar-mode-matrix).

**Benchmark manifests:** `benchmarks/tasks/local-real/` is the realistic local
product corpus; `benchmarks/tasks/holdout-retrieval/` is the public
generalization corpus. Holdout rows are promotion evidence only, not a tuning
loop.

## Environment flags

### Runtime variables

`CODESTORY_RETRIEVAL_V2` and `CODESTORY_RETRIEVAL_V2_SHADOW` are no longer migration aliases.
If either legacy variable is present, packet retrieval fails closed instead of silently mapping it
to the sidecar-primary contract.

| Variable | Default (production) | Purpose |
|----------|----------------------|---------|
| `CODESTORY_RETRIEVAL` | unset → sidecar primary when manifest + `full` mode (else fail closed) | `1` force sidecar primary attempt; `0` is unsupported and fails closed |
| `CODESTORY_RETRIEVAL_SHADOW` | unsupported for product benchmarks | Historical diagnostic switch; benchmark contract rejects it |
| `CODESTORY_ZOEKT_ENABLED` | on | `0` is unsupported for product retrieval |
| `CODESTORY_QDRANT_ENABLED` | on | `0` is unsupported for product retrieval |
| `CODESTORY_RETRIEVAL_REAL_EMBEDDINGS` | `1` | `0` is unsupported for product retrieval |
| `CODESTORY_RETRIEVAL_COMPOSE_PROFILE` | `real` | every other profile is unsupported for product bootstrap |
| `CODESTORY_EMBED_BACKEND` | `llamacpp` | product manifests require llama.cpp bge-base embeddings |
| `CODESTORY_EMBED_LLAMACPP_URL` | `http://127.0.0.1:8080/v1/embeddings` | local embedding sidecar endpoint |
| `CODESTORY_ZOEKT_PORT` | `6070` | Zoekt HTTP |
| `CODESTORY_QDRANT_HTTP_PORT` | `6333` | Qdrant HTTP |
| `CODESTORY_QDRANT_GRPC_PORT` | `6334` | Qdrant gRPC |

### Benchmark-only flags

Use these when running promotion harnesses. Do not enable in normal production packet runs.

| Variable | Default | Purpose |
|----------|---------|---------|
| `CODESTORY_EVAL_PROBES` | ignored in production runtime | Benchmark-shaped probe catalog (`eval_probes.rs`) is test-only; promotion bundles do not inject it. |

**Sidecar promotion candidate (typical):**

```powershell
Remove-Item Env:CODESTORY_RETRIEVAL -ErrorAction SilentlyContinue
Remove-Item Env:CODESTORY_EVAL_PROBES -ErrorAction SilentlyContinue
.\target\release\codestory-cli.exe retrieval up
.\target\release\codestory-cli.exe retrieval index --project . --refresh auto
```

---

## Local workflows

### One-command environment setup

From the CodeStory repository root:

```sh
cargo retrieval-setup
cargo retrieval-status
```

Optional Node wrapper (prerequisite report, optional holdout clone):
`node scripts/setup-retrieval-env.mjs`.
See [`../ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md#quick-start-one-command).

### Sidecars and index

```sh
cargo retrieval-setup
cargo run -p codestory-cli -- retrieval index --project <repo-root> --refresh auto
cargo run -p codestory-cli -- retrieval query "main" --project <repo-root>
```

`retrieval bootstrap` (alias `cargo retrieval-setup`) starts Docker Compose when Docker is installed.
`retrieval up` alone only prepares cache dirs and state (see runbook).

### local-real packet suite (in-scope tuning)

Repos: `codex`, `rootandruntime`, `sourcetrail`, `vscode` — manifests under
`benchmarks/tasks/local-real/`.

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --packet-runtime --packet-runtime-mode cold-cli `
  --task-suite local-real --repeats 1 `
  --out-dir target/agent-benchmark/packet-runtime-sidecar-promotion `
  --codestory-cli target/release/codestory-cli.exe `
  --timeout-ms 300000
```

Local-real rows are product-development evidence, not public savings claims by
themselves. They need repeated quality-gated runs against clean pinned checkouts
before promotion language.

### holdout-retrieval (generalization)

```powershell
node scripts/fetch-holdout-repos.mjs
# or:
node scripts/codestory-agent-ab-benchmark.mjs `
  --list --task-suite holdout-retrieval --materialize-repos

node scripts/codestory-agent-ab-benchmark.mjs `
  --packet-runtime --packet-runtime-mode cold-cli `
  --task-suite holdout-retrieval --materialize-repos `
  --repeats 1 `
  --out-dir target/agent-benchmark/holdout-retrieval-smoke `
  --codestory-cli target/release/codestory-cli.exe `
  --timeout-ms 180000
```

Holdout failures should block promotion or trigger diagnosis; do not add
repo-name/path literals or tune planner/ranker heuristics against holdout rows.

## Fast CI-style checks (automated in Phase 6)

```powershell
cargo test -p codestory-runtime --test retrieval_generalization_guard
node --test scripts/tests/codestory-agent-ab-analyzer.test.mjs
cargo test -p codestory-cli --test onboarding_contracts
```

Optional broader lane:

```powershell
cargo test -p codestory-retrieval
cargo test -p codestory-runtime
node --test scripts/tests/codestory-agent-ab-analyzer.test.mjs
```

`cargo test -p codestory-retrieval` includes the scoped semantic behavior gate:
mixed queries with strong lexical/graph candidates use a Qdrant path allowlist,
while underfilled allowlists fall back to the full semantic stage. Those tests
are the minimum guard against turning selective search into a silent recall cap.

---

## Promotion checklist

Status as of Phase 6 documentation pass. **Benchmark pass columns require a human run** with
repos, sidecars, and release CLI — not claimed here.

### Language support audit alignment

Support claims must be backed by committed benchmark manifests, generated artifacts, or
tests in the branch. Do not infer support for languages without direct benchmark artifacts.

| Item | Status | Notes |
|------|--------|-------|
| Phases 0–5 code landed | done | See implemented stack above |
| Architecture / design docs | done | `docs/architecture/retrieval-design.md` |
| Sidecar runbook | done | `docs/ops/retrieval-sidecars.md` |
| Local-real manifests | done | `benchmarks/tasks/local-real/` |
| Holdout manifests + fetch script | done | `benchmarks/tasks/holdout-retrieval/`, `scripts/fetch-holdout-repos.mjs` |
| `freelancer` / `traderotate` removed from default holdouts | done | OSS holdouts only |
| Generalization lint + guard test | done | `lint-retrieval-generalization.mjs`, `retrieval_generalization_guard` |
| Warning config | done | `docs/architecture/retrieval-rollback.json` |
| Markdown link contract (`onboarding_contracts`) | verify | `cargo test -p codestory-cli --test onboarding_contracts` |
| local-real cold packet + north-star SLOs | **human** | p99 retrieval, quality 3/4, wall targets |
| holdout-retrieval 2/3 pass | **human** | Requires materialized OSS repos + index |
| `agent_value_gap` &lt; 0.20 | **human** | Measure from a fresh coherent bundle |
| Windows `retrieval-sidecar-smoke` CI job | fail-closed sidecar smoke | [`retrieval-sidecar-smoke-ci.md`](../contributors/retrieval-sidecar-smoke-ci.md) |
| Ragas/Phoenix nightly eval | optional | Not configured |

### North-star SLOs (targets — measure before claiming pass)

| Metric | Target |
|--------|--------|
| Retrieval p50 | ≤ 250 ms |
| Retrieval p90 | ≤ 600 ms |
| Retrieval p99 | ≤ 1,000 ms |
| Worst-case packet wall | ≤ 1,500 ms |
| local-real quality pass | ≥ 3/4 repos |
| `agent_value_gap` | &lt; 0.20 |
| holdout generalization | 2/3 of `ripgrep`, `axios`, `redis` |
| Sidecar planner/ranker repo literals | 0 (lint clean) |

---

## Rollback drill (REQ-RES-005)

After promotion runs, verify rollback warnings:

1. Point `retrieval_rollback` at a baseline `packet-runtime-summary.json` with thresholds that will trip on the current summary (or use unit test `rollback_drill_warns_without_setting_legacy_env` in `retrieval_rollback.rs`).
2. Confirm `check_and_log_rollback_warnings` logs trigger ids without setting `CODESTORY_RETRIEVAL=0`.
3. File a one-line incident note in this doc with date and trigger id if rollback fires in production promotion.

**One-shot operator drill (after each promotion run):**

```powershell
cargo test -p codestory-runtime retrieval_rollback::tests::rollback_drill_warns_without_setting_legacy_env -- --nocapture
```

Expect rollback warnings only when configured thresholds fire (see `docs/architecture/retrieval-rollback.json`). Sidecar retrieval remains mandatory.

**Closure status (2026-05-27, semantic promotion pass):** Phase A shipped (bge-base 768-d, llama.cpp `embed` compose service, manifest `embedding_backend`/`embedding_dim`, Qdrant collection migration, llamacpp dim hard-fail). Local `retrieval status` reaches `full` with default 768-d vectors after Qdrant re-index. Sidecar-primary is the intended product path, but product promotion remains gated until fresh benchmark evidence passes.

---

## Spec and design references

| Doc | Path |
|-----|------|
| Design | [`docs/architecture/retrieval-design.md`](../architecture/retrieval-design.md) |
| Operations | [`docs/ops/retrieval-sidecars.md`](../ops/retrieval-sidecars.md) |
| Rollback config | [`docs/architecture/retrieval-rollback.json`](../architecture/retrieval-rollback.json) |
