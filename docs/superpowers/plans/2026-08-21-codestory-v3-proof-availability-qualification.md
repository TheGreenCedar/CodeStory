# CodeStory v3 Proof Availability Qualification and Conditional Activation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve the sound, fail-closed call-path proof kernel, measure whether it is useful on independently established source paths, remove two unrelated packet-ranking defects, and make the public v3 proof surface conditional on frozen qualification evidence.

**Architecture:** Keep proof soundness in the existing dark `codestory-agent` kernel and pinned `codestory-runtime` adapter. Add a sealed, benchmark-only observation boundary that calls those exact code paths and emits typed first-failure data. `codestory-bench` owns corpus materialization, inventory, reports, statistics, and the activation decision; it does not define proof behavior. Packet/context/search still become evidence-only in v3 regardless of whether proof is public, experimental, or dark.

**Tech Stack:** Rust 2024, Cargo feature gates, SQLite/`rusqlite`, `serde`/`serde_json`, RFC 8785 canonical JSON, SHA-256, JSON Schema 2020-12, Clap, existing runtime/store/indexer APIs, Node-based repository checks, GitHub Issues/PRs.

**Spec:** [CodeStory v3 retrieval/proof separation](../codestory-v3-retrieval-proof-separation.md). This plan preserves that document's proof semantics but supersedes its fixed five-PR sequence and unconditional public `prove_call_path` registration.

**2026-08-21 operational amendment:** The verified-state section below records the historical planning snapshot and remains useful as provenance. All remaining branches and integration PRs target `dev/codestory-0.18`, not `dev/codestory-next`. The concurrent 0.17.4 release lane is outside this program. Q1 remains production-dark and produces neither an availability result nor an activation decision; Q2 alone may select Outcome A, B, C, or D. Current `run` invocations must pass the frozen thresholds explicitly, and current `verify` invocations must pass the frozen corpus explicitly.

## Global Constraints

- Preserve PR 2's strict positive-proof semantics: stored `Certain`, exact resolved target, no retained candidate alternatives, canonical callsite identity, callable/file/containment agreement, hash-bound source, and edge-distinct receipts.
- Production has no extractor-completeness provider; missing relations remain `Unknown`, and no production path may emit `CertifiedAbsence`.
- `source_text` is available only to clause validation, hashing, diagnostics, and rendering; it never enters retrieval, ranking, selector resolution, graph traversal, or source matching.
- PR 3, PR 4, and Q1 remain production-unreachable. They do not change the current publication constants, dispatchers, public DTO serialization, generated catalog, CLI command surface, HTTP routes, or plugin routes.
- Do not register MCP or stable CLI proof before Q2 selects Outcome A. Outcome B is CLI-only and explicitly experimental. Outcome C keeps proof dark.
- Packet, context, and search become evidence products in every public-v3 outcome. Legacy `Supported` cannot reach a public v3 response.
- Cut CodeStory publication schema 3 exactly once in the final integration commit; keep MCP protocol revision negotiation separate from the CodeStory publication schema.
- No version bump, release, tag, marketplace publication, or production-compatibility claim is part of this program.
- Use fresh full indexes built from pinned clean checkouts for qualification. The July schema-22 database is diagnostic only.
- Freeze corpus and thresholds before the first result run. A later kernel, indexer, corpus, threshold, or qualification-calculation change invalidates Q2.

---

---

## 1. Executive verdict

Claude likely found a real availability ceiling in the graph feeding the proof kernel. The existing database reproduces the reported raw ratio closely: 30,367 rows satisfy a SQL approximation of the strict predicate out of 92,682 stored `CALL` rows, or 32.7647%. That is evidence of limited exact-resolution inventory, not evidence that the predicate is unsound.

What remains unproven is the product question: how often the actual pinned runtime can prove an independently source-audited true path, how useful its exact partial receipts and gaps are when it abstains, and whether latency and payload cost fit an agent workflow. The generated one-edge and six-edge fixtures prove non-vacuity only.

Therefore:

- do not weaken or revert PR 2;
- do not register proof publicly yet;
- finish PR 3 and PR 4 while they remain production-unreachable;
- fix the packet-ranking defects in a separate PR;
- add two qualification children before public integration;
- choose public, experimental, or dark proof from frozen evidence;
- delay the entire v3 cut only if code proves packet projection and proof activation are inseparable.

## 2. Verified current state

All facts in this section were rechecked on 2026-08-21.

### Git and ownership

- `dev/codestory-next`, local and origin: `74753c1766c80f8cf27873943409bd509bc30350`
- tree: `446d15bbb8dc3e13818c81c951a8f6167e981f50`
- PR 1: merged as `d9e20f045ef4bce2a704c7b6182cb71e52576b68` by #1979
- PR 2: merged as `74753c1766c80f8cf27873943409bd509bc30350` by #1980
- open PRs: none
- open program issues: #1968, #1973, #1974, #1977, #1978; #1949 is unrelated release/archive-pin work
- active PR-3 worktree: `.worktrees/1974-dark-packet-projections`
- PR-3 branch/head/tree: `codex/1974-dark-packet-projections` / `220995d6e7a0e227292d8b5b38edb201be0340fa` / `f5b12e98e0d12f37b14c981468787bc2cf43b6d3`
- PR-3 commits: `8ca090c8`, `40d91eaa`, `220995d6`; tracked files are clean and only `.superpowers/` is untracked

Recheck command:

```sh
git fetch origin
git rev-parse HEAD^{commit} HEAD^{tree} origin/dev/codestory-next
git worktree list --porcelain
gh pr list --state open --json number,title,headRefName,baseRefName,url
gh issue list --state open --limit 100 --json number,title,url
```

### Kernel and runtime

- The strict persisted-edge predicate is `admit_raw_call_edge` in `crates/codestory-agent/src/indexed_source_call_path_v1.rs:106-160`.
- Selector resolution, pinned identity, exact path matching, source/file checks, hash-before-UTF-8 binding, unique-smallest containment, receipt construction, and the current early selector failure are in `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:46-328`.
- Edge-distinct connected checking is in `crates/codestory-agent/src/indexed_source_call_path_v1.rs:1432-1830`.
- Checked receipt integration and authoritative-only projection are in the same file at `:1834-2478`.
- The core-only public operation wrapper is `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:337-363`.
- The real `WorkspaceIndexer` census fixtures are at `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:1126-1344`; their exact 1/1 and 6/6 counts establish reachability, not repository recall.
- Production darkness is enforced at `crates/codestory-cli/tests/architecture_contracts.rs:611-770`.

No Critical or Important soundness defect was found in the accepted PR-2 review. The qualification gap is observability and recall: raw admission is currently `Admitted | Rejected`, containment is `Option`, several source failures collapse to `SourceNotBoundToPublication`, and a selector failure returns before any step facts are built.

### Existing database

Artifact:

```text
target/embedding-model-study/cache/codestory/codestory.db
sha256 e6395172655b98183b872d12177bbcca1d0d6fd5c2d067621edae6c96b742746
270,839,808 bytes
mtime 2026-07-15T08:55:51-0400
```

Observed inventory:

| Field | Value |
| --- | ---: |
| SQLite `user_version` | 22 |
| current source schema | 31 |
| generation | 1 |
| generation ID | `28a6f09a-84b1-4db1-a452-1581a2855c34` |
| run ID | `110d8ab6-dafd-4ca6-a80d-9aebfa3c10ff` |
| files / complete / hashed | 296 / 288 / 286 |
| nodes / edges / `CALL` rows | 128,793 / 106,463 / 92,682 |
| certainty null, unresolved | 55,563 |
| certain, resolved | 30,368 |
| probable, resolved | 6,711 |
| uncertain, unresolved | 40 |

The source checkout represented by this database is absent. The database has no attested indexer commit or freshness proof and is nine schema versions old. It is useful only for reproducing the hypothesis. It cannot decide activation.

The 30,367/92,682 raw-admission approximation is a reproduced measurement. Claude's reported 17.5% two-step and 14.0% three-step figures remain unverified because the original endpoint model, SQL, and trail-enumeration rules were not supplied. Q1 replaces those numbers with checked-in definitions and actual-kernel counts; it does not bless the old percentages after the fact.

### Evidence labels used by this plan

| Label | Current example |
| --- | --- |
| Verified fact | exact integration head/tree, merged PRs, dark kernel paths, and production-unreachability tests |
| Reproduced measurement | 30,367 SQL-approximated admissible rows out of 92,682 stored `CALL` rows in the named schema-22 database |
| Inference | low exact-resolution inventory is likely an upstream availability constraint rather than a proof-soundness defect |
| Unverified claim | Claude's connected-trail percentages and any real-world yield inferred from the stale database |
| Proposed gate | the role thresholds in Section 8, frozen before Q2 and not yet qualification evidence |

## 3. Root-cause analysis

### Kernel soundness

The kernel deliberately demands stored `Certain`, exact resolved target, no candidate alternatives, canonical callsite identity, callable/file/containment agreement, publication-bound source bytes, and edge-distinct receipts. Those requirements are the reason a positive result is defensible. They must remain unchanged unless a concrete implementation bug is independently reproduced.

### Graph-resolution coverage

Most stored `CALL` rows in the old database lack a resolved target. Another 6,711 are only `probable`. Both populations are intentionally ineligible for proof. This points upstream to parser/resolution coverage rather than downstream to the checker.

### Product-availability gap

Raw row ratios and stored trail ratios describe inventory. They do not answer whether exact source paths users care about resolve, whether failures occur early or late, or whether partial receipts are useful. The load-bearing metric is an independently source-audited manifest executed through the actual runtime builder, checker, projection, and budget path.

### Actual PR-2 defects

No correctness defect is currently established. Add typed qualification diagnostics without changing dispositions. If qualification exposes a mismatch between the diagnostic path and the existing result, treat that as a PR-2 bug and stop the run; never let the benchmark silently reinterpret product behavior.

## 4. Revised architecture

### Unchanged

- `search` is discovery leads.
- `context` is evidence for one target.
- `packet` is broad evidence, gaps, continuation, and retrieval state.
- Only the exact call-path domain may ever emit proof dispositions.
- `ContractProven` remains strict and receipt-backed.
- Missing edges remain `Unknown`; production has no certified-absence provider.
- Source text remains translation/hashing/rendering input, never retrieval input.
- Runtime owns publication pins, selector resolution, source reads, and retry.
- Agent owns the pure checker.
- Adapters render; they do not decide truth.

### Added

Introduce two sealed features:

```toml
# crates/codestory-agent/Cargo.toml
proof-qualification-support = ["dep:serde_json_canonicalizer", "dep:sha2"]

# crates/codestory-runtime/Cargo.toml
proof-qualification-support = ["codestory-agent/proof-qualification-support"]
```

Only `codestory-bench` may enable them. Product crates, CLI production features, the plugin, and generated catalogs may not. The features expose:

1. a reason-bearing view of the same raw admission leaf;
2. a runtime trace of selector, edge, containment, source, receipt, and projection gates;
3. the same checked integration and projection used by the dark tool;
4. no new public product DTO, route, tool, or readiness path.

The existing proof function delegates to the reason-bearing leaf:

```rust
pub fn admit_raw_call_edge(
    edge: &Edge,
    expected_source: NodeId,
    expected_target: NodeId,
) -> RawCallEdgeAdmission {
    match diagnose_raw_call_edge(edge, expected_source, expected_target) {
        Ok(admitted) => RawCallEdgeAdmission::Admitted(admitted),
        Err(_) => RawCallEdgeAdmission::Rejected,
    }
}
```

That delegation is the anti-drift boundary. The benchmark must never reimplement admission in SQL.

## 5. Qualification design

### Repositories and fixed corpus size

| Cohort | Repository | Commit | Workspace | Primary language role |
| --- | --- | --- | --- | --- |
| `codestory-rust` | TheGreenCedar/CodeStory | `74753c1766c80f8cf27873943409bd509bc30350` | `.` | Rust plus mixed-repo boundary |
| `vite-ts-js` | vitejs/vite | `80a333a23103ced0442d4463d1191433d90f5e19` | `packages/vite` | TypeScript/JavaScript |
| `flask-python` | pallets/flask | `7fff56f5172c48b6f3aedf17ee14ef5c2533dfd1` | `.` | Python |
| `gin-go` | gin-gonic/gin | `d75fcd4c9ab260e5225de590f1f0f8c0e0e12d11` | `.` | Go |

Each cohort contains 30 independently source-audited positive paths:

| Path length | Paths per cohort | Positive step attempts per cohort |
| ---: | ---: | ---: |
| 1 | 10 | 10 |
| 2 | 7 | 14 |
| 3 | 5 | 15 |
| 4 | 3 | 12 |
| 5 | 3 | 15 |
| 6 | 2 | 12 |
| **Total** | **30** | **78** |

The full corpus is 120 positive requests and 312 positive steps. Every positive has two checked-in, source-audited negative mutations, for 240 negative requests. Negative mutations assert only “must not be `ContractProven`”; missing static relations remain `Unknown` without completeness receipts.

Corpus selection must be independent of CodeStory output. Before the first benchmark run:

- record exact caller and target declaration path/range/hash;
- record exact callsite byte range/hash for every step;
- verify the callsite from pinned source bytes;
- spread cases across at least five source areas per repository where available;
- cap one source file at 20% of a cohort;
- reject duplicate caller-target pairs;
- record curator and independent reviewer;
- freeze the corpus and threshold hashes;
- make the runner refuse a corpus without the freeze record.

### Checked-in artifacts

```text
benchmarks/proof-availability/
  methodology.md
  thresholds-v1.json
  corpus-v1.json
  schemas/
    corpus.schema.json
    path.schema.json
    report.schema.json
    thresholds.schema.json
  paths/
    codestory-rust.json
    vite-ts-js.json
    flask-python.json
    gin-go.json
  sql/
    raw-call-inventory.sql
    connected-trails.sql
  results/{qualification_id}/
    environment.json
    inventory.json
    trails.json
    cases.json
    failure-funnel.json
    summary.json
    decision.json
    findings.md
docs/testing/proof-availability-v1.md
```

Target checkouts, databases, binaries, and logs stay under `target/proof-availability/` and are not committed.

### Oracle path shape

The benchmark owns a closed serializable DTO and converts it into the existing unvalidated proof contract:

```rust
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OraclePathV1 {
    case_id: String,
    repository_id: String,
    language: String,
    source_text: String,
    clauses: Vec<ClauseAnchorV1>,
    spec: CallPathSpecV1,
    oracle_steps: Vec<OracleStepV1>,
    negative_mutations: [NegativeMutationV1; 2],
    audit: OracleAuditV1,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OracleStepV1 {
    caller: OracleDeclarationV1,
    callsite: OracleSourceRangeV1,
    target: OracleDeclarationV1,
}
```

`materialize` rehashes every declaration and callsite range against the pinned checkout before indexing. A mismatch stops the entire run.

### Separate denominators

For each repository and path length 1 through 6, report raw counts and ratios for:

1. **Raw graph admission:** actual-kernel admitted `CALL` rows / all stored `CALL` rows.
2. **Effective-endpoint trails:** edge-distinct stored `CALL` trails using `resolved_source.unwrap_or(source)` and `resolved_target.unwrap_or(target)` / all such trails.
3. **Exact-resolved trails:** edge-distinct trails restricted to rows with an exact stored resolved target / all effective-endpoint trails.
4. **Strictly admissible trails:** edge-distinct trails composed only of actual-kernel admitted edges / effective and exact-resolved denominators.
5. **Known-positive yield:** actual runtime `ContractProven` paths / independently audited positives.

Vertices may repeat; an edge ID may appear at most once in a trail. Unresolved placeholder endpoints remain in the effective-endpoint inventory and are counted explicitly. Use checked `u128` accumulation and fail on overflow. SQL files are diagnostic definitions; Rust calls the Store and kernel and is authoritative.

### First-failure funnel

Add closed reason types rather than free-form strings:

```rust
enum SelectorFailure {
    Missing,
    Ambiguous,
    NonCallable,
}

enum RawAdmissionFailure {
    WrongKind,
    CertaintyAbsent,
    CertaintyProbable,
    CertaintyUncertain,
    WrongEffectiveSource,
    WrongEffectiveTarget,
    MissingExactResolvedTarget,
    CandidateAlternativesRetained,
    MissingFileNode,
    MissingLine,
    InvalidOrLegacyCallsiteIdentity,
    CallsiteFileMismatch,
    CallsiteLineMismatch,
    CallsiteRawTargetMismatch,
}

enum ContainmentFailure {
    EdgeSourceFileMismatch,
    Missing,
    Ambiguous,
}

enum SourceBindingFailure {
    FileIncomplete,
    StoredHashAbsent,
    WorkingTreeReadFailed,
    WorkingTreeHashMismatch,
    InvalidUtf8,
    LineMissing,
    LineOverLimit,
}

enum FinalizationFailure {
    ReceiptIntegration,
    ReceiptBudget,
    ProjectionBudget,
}
```

Candidate edges are ordered by edge ID and advanced through the exact predicate order. The step's first failure is the first gate that leaves zero survivors. Preserve the complete histogram of reasons that removed candidates at that gate. A successful step is `Admitted`. This produces one deterministic step-level funnel without hiding alternative candidate failures.

Every attempted positive step must end in exactly one first-failure or `Admitted` bucket. The sum must reconcile to 312. Unknown or uncategorized output is a hard qualification failure.

### End-to-end and partial-value metrics

For every positive request report:

- full `ContractProven`;
- authoritative receipt count and exact oracle match;
- proven-step precision and recall;
- at least one authoritative receipt;
- mean proven prefix length;
- exact product disposition and gaps;
- exact first-failure trace;
- diagnostic-only candidate evidence versus authoritative receipt evidence;
- warm end-to-end latency and per-stage durations;
- complete projection bytes;
- revision-specific complete `ToolResult` bytes for all four MCP profiles after PR 4;
- false `ContractProven` across both negative mutations.

A useful partial result has at least one oracle-matching authoritative receipt and an exact next-step gap. Candidate diagnostics without an authoritative receipt are reported separately and never counted as partial proof value.

### Reproducible executable

Add an explicit binary:

```text
crates/codestory-bench/src/bin/codestory_proof_availability.rs
binary name: codestory-proof-availability
```

Commands:

```sh
./target/release/codestory-proof-availability materialize \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace target/proof-availability/workspaces \
  --cache-root target/proof-availability/cache \
  --out target/proof-availability/run/environment.json

./target/release/codestory-proof-availability run \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --environment target/proof-availability/run/environment.json \
  --out target/proof-availability/run

./target/release/codestory-proof-availability verify \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --results target/proof-availability/run
```

`materialize` creates detached pinned checkouts, verifies oracle hashes, builds fresh full indexes through the source-linked runtime/indexer, and records core generation/run plus database SHA. `run` is local and does not fetch. `verify` recomputes every aggregate and decision from case rows.

The exact commands above remain the A/B/C contract. Outcome D additionally requires the same optional `--source-dependency <EVIDENCE_JSON>` on `run` and `verify`. That closed evidence is bound to the qualification source commit/tree, full-file and range hashes for both the dependency and its architecture test, and a supported dependency/test pairing. Present-but-invalid evidence fails closed; omitted evidence cannot select D.

## 6. Revised PR and issue sequence

| Order | Owner | Purpose | Blocks public cut? |
| ---: | --- | --- | --- |
| 1 | PR 1 / #1975 | compatibility harness; merged | complete |
| 2 | PR 2 / #1976 | dark proof kernel; merged | complete |
| 3 | PR 3 / #1974 | dark packet projections | yes, but may proceed now |
| 4 | new packet-scoring child | remove global Python and `/collections/` rank authority | yes; independent of proof qualification |
| 5 | PR 4 / #1978 | dark revision-native MCP machinery | yes, but may proceed after PR 3 |
| 6 | new qualification child Q1 | benchmark boundary, frozen corpus, thresholds, executable | yes |
| 7 | new qualification child Q2 | exact-head results, independent review, activation decision | yes |
| 8 | optional measured remediation | only for a demonstrated dominant gate | only if opened |
| 9 | PR 5 / #1977 | one public v3 cut following Outcome A, B, or C | final |

Issue changes to make during execution:

- Add Q1 and Q2 as blocking children of #1973. Remove fixed “five PRs” wording without rewriting the history of merged PRs.
- Mark #1977 blocked by Q2 and the packet-scoring child. Add explicit Outcome A/B/C branches.
- Keep #1974 and #1978 authorized because both remain dark.
- Keep #1976 closed; its issue required non-vacuity, not product-availability qualification.
- Do not reopen #1200.
- Record Claude's figures in #1973 as the motivating unverified hypothesis and link the old DB identity. Replace them as decision evidence only with Q2's checked-in report.
- Create an upstream exact-call-resolution epic only if Q2 shows certainty/resolution gates account for at least 50% of failed expected steps in at least two repository/language cohorts.

## 7. Decision table

| Outcome | Required evidence | Public behavior |
| --- | --- | --- |
| **A — public exact verifier** | all hard gates; stable-explicit thresholds; automatic thresholds decide only the workflow posture | register MCP + stable CLI. If automatic thresholds pass, skill may invoke for matching exact-call-path work; otherwise skill documents explicit verification only. |
| **B — experimental/manual verifier** | all hard gates; experimental thresholds; stable thresholds missed | expose `codestory-cli experimental prove-call-path`; no MCP tool, catalog entry, or agent-skill recommendation. |
| **C — keep proof dark** | hard gates pass but experimental usefulness fails, or actionable partial value is too low | ship evidence-only packet/context/search v3; retain kernel and qualification executable behind sealed features. |
| **D — delay full v3 cut** | a source-level dependency test demonstrates packet v3 cannot ship without activating proof, or v3 transport cannot truthfully represent Outcome C | delay the public cut and file the exact dependency as a blocker. Metrics alone do not select D. |

Under A, #1968 and #1973 may close after installed acceptance. Under B or C, close #1968 when the packet cut ships but keep #1973 open as the stable-proof parent. Under D, close neither.

## 8. Acceptance gates

Thresholds are frozen in `thresholds-v1.json` before the first benchmark run. Use a two-sided 95% Wilson score interval and report raw numerator/denominator beside every percentage.

### Hard gates for every public or experimental outcome

- 0 false `ContractProven` across 240 negative mutations.
- 100% of authoritative receipts match the oracle caller, callsite line/window, edge identity, and target.
- 0 production `CertifiedAbsence` results.
- 0 unclassified positive-step failures; funnel total equals 312.
- complete repository, source, index, core publication, binary, corpus, threshold, and result provenance.
- no invalid or over-cap result; proof maximum 64 KiB.
- no aggregate hides a failed repository cohort.
- any benchmark/product disposition mismatch stops qualification as an implementation bug.

### Role thresholds

| Metric | Automatic workflow | Stable explicit verifier | Experimental CLI |
| --- | ---: | ---: | ---: |
| Full proof, overall | ≥96/120 (80%); Wilson lower ≥72% | ≥60/120 (50%); lower ≥41% | ≥24/120 (20%); lower ≥14% |
| Full proof, each cohort | ≥21/30 (70%); lower ≥50% | ≥12/30 (40%); lower ≥24% | at least one cohort ≥12/30; others shown, never averaged away |
| Positive-step recall | ≥90% | ≥75% | ≥50% |
| Full or useful receipt-backed partial | ≥95% | ≥80% | ≥60% |
| Incomplete requests with actionable exact gap | ≥95% | ≥90% | ≥80% |
| Warm runtime `Unknown` p95 | ≤500 ms | ≤1 s | ≤2 s |
| Installed/transport p95 after integration | ≤1.5 s | ≤2 s | ≤3 s CLI |
| Complete response p95 | ≤32 KiB | ≤32 KiB | ≤48 KiB |
| `Unknown` response p95 | ≤16 KiB | ≤16 KiB | ≤24 KiB |
| Absolute maximum | 64 KiB | 64 KiB | 64 KiB |

The overall and cohort cutoffs are sized for 120 and 30 observations respectively; the Wilson lower bounds prevent a small cohort from passing on a brittle point estimate. Step recall measures the 312 audited relations. Partial-value thresholds stop a low full-chain rate from looking useful merely because diagnostics mention candidates. Latency and size gates reflect the role: automatic use must be cheap, while an explicit manual verifier can tolerate more abstention and cost.

“Actionable exact gap” is closed, coordinate-bearing, and not prose-scored: selector missing/ambiguous retains its selector index; relation/recursion and source-binding gaps retain their exact step; finalization budget retains the completed step count. Only a gap matching the first unproven prefix boundary counts. Projection budget is actionable only when every attempted step has an admitted trace, no step is unclassified, and finalization records the matching receipt/projection budget state. A later or global gap does not count merely because it appears first in a product list.

No threshold is tuned after viewing results. Changing a corpus, threshold, kernel, indexer, or qualification calculation invalidates Q2 and requires a new qualification ID.

## 9. Packet scoring defect disposition

| Code | Classification | Ownership | Blocking effect |
| --- | --- | --- | --- |
| `/collections/` +4 at `crates/codestory-agent/src/packet_scoring.rs:149-155` | retrieval ranking and context-noise defect; path spelling is standing in for role | `codestory-agent` scoring plus runtime production-path tests | does not block dark PR 3; blocks public packet integration |
| production-only Python -100 at `crates/codestory-agent/src/packet_scoring.rs:188-200` and `:1188-1197` | cross-language false-negative ranking defect | `codestory-agent`; production ordering is called by `crates/codestory-runtime/src/agent/orchestrator.rs:2064-2094` | does not block proof or dark PR 3; blocks public packet integration |

The existing `packet_drop_unrequested_python_siblings` is already limited to runtime-formatting questions and keeps Python when no non-Python replacement exists. Preserve that narrow behavior. Remove the global rank penalty. Do not infer collection authority solely from `/collections/`; typed evidence roles may still identify a collection.

## 10. Risks and non-goals

### Risks

- Corpus curation can bias results. Freeze it before output and require independent source review.
- Qualification features can accidentally ship. Architecture tests must permit them only on the `codestory-bench` edge.
- A diagnostic fork can drift from product behavior. Existing admission/check/projection functions must delegate to or be called by the observed path.
- Trail enumeration may be large. Use edge-indexed adjacency, checked `u128`, deterministic order, and fail rather than sample.
- Exact source hashes can drift after checkout. Materialization rechecks before index, run rechecks before each case, and the runtime post-operation fence still applies.
- A low aggregate can hide one useful language. Preserve every cohort and the role-specific per-cohort rules.
- A high aggregate can hide a broken language. Automatic and stable roles require every advertised cohort to pass.

### Non-goals

- weakening `Certain` or exact-target admission;
- proving runtime execution or arbitrary reachability;
- production negative proof;
- evaluating unrestricted English translation in Q1/Q2;
- using the old database as qualification evidence;
- improving indexer resolution before measuring the failing gates;
- changing versions, releasing, tagging, or publishing a marketplace package;
- folding packet relevance into proof availability.

## 11. Changes from the previous plan

Added:

- a frozen, multi-repository proof-availability corpus;
- exact row/trail/known-positive denominators;
- typed first-failure funnel;
- partial-receipt usefulness metrics;
- Wilson-backed role thresholds;
- two blocking qualification children;
- public/experimental/dark decision branches;
- a separate packet-scoring blocker.

Removed:

- the assumption that five PRs are sufficient;
- unconditional public `prove_call_path` registration;
- the claim that generated positive fixtures qualify real-world availability.

Delayed:

- public proof activation;
- any upstream call-resolution program;
- final installed-host proof until Q2 selects a surface.

Reclassified:

- Claude's percentages are reproduced diagnostic evidence from a stale database, not qualification;
- low yield is an availability problem unless a correctness counterexample appears;
- PR 2 remains correctly closed for its actual dark-kernel acceptance.

---

# Implementation tasks

For every remaining new PR below, create a dedicated `codex/` branch and linked worktree from the then-current `origin/dev/codestory-0.18`; never implement directly on the primary checkout or the concurrent 0.17.4 release lane. After creating it, run:

```sh
node scripts/codex-worktree-setup.mjs \
  --project "$(pwd)" \
  --intended-base-ref origin/dev/codestory-0.18 \
  --resolve-cli-only
```

Treat the script's printed base, branch head, and proof target as authoritative. Keep Cargo commands serial within each worktree.

## Task 1: Reconcile the design and GitHub control plane

**Files:**

- Modify: GitHub issues #1973 and #1977
- Create: one packet-scoring child, Q1 qualification child, and Q2 decision child

**Interfaces:**

- Consumes: the verified issue/branch state in Section 2 and the A/B/C/D decision contract in Section 7.
- Produces: three PR-sized child issue numbers, Q1 → Q2 → #1977 dependency links, and parent text that no longer assumes unconditional proof activation.

- [ ] **Step 1: Recheck exact state before external mutation**

Run:

```sh
git fetch origin
test "$(git rev-parse origin/dev/codestory-next)" = "74753c1766c80f8cf27873943409bd509bc30350"
gh pr list --state open
gh issue view 1973 --json body,state,title
gh issue view 1977 --json body,state,title
```

Expected: no open PR; #1973 and #1977 still contain fixed five-PR/unconditional-registration wording. If origin moved, stop and rebase this plan's state section before creating issues.

- [ ] **Step 2: Create exact child issues**

Create these titles and acceptance boundaries:

```text
[v3 blocker] Remove repository-shaped packet ranking authority
[v3 qualification 1/2] Freeze exact call-path availability harness and oracle corpus
[v3 qualification 2/2] Qualify exact call-path availability and choose activation
```

The packet issue references #1968 and blocks #1977. Q1 and Q2 reference #1973; Q2 blocks #1977 and depends on Q1.

- [ ] **Step 3: Update the parents without selecting an outcome**

Replace “five PR-sized children” in #1973 with the sequence in Section 6. Add the A/B/C/D table to #1977. Keep #1976 closed and #1200 untouched.

- [ ] **Step 4: Verify the control plane**

Run:

```sh
gh issue view 1973 --json body,state,title
gh issue view 1977 --json body,state,title
gh issue list --state open --limit 100 --json number,title,url
node .github/scripts/check-doc-links.mjs
```

Expected: three new children exist, #1977 names both blockers, and dark PRs remain authorized. No repository file changes in this task.

## Task 2: Review and land the existing dark packet projection PR

**Files already changed in `.worktrees/1974-dark-packet-projections`:**

- `crates/codestory-contracts/src/packet_projection_v3.rs`
- `crates/codestory-agent/src/packet_execution_plan_v3.rs`
- `crates/codestory-runtime/src/agent/packet_execution_record_v3.rs`
- `crates/codestory-runtime/src/agent/packet_projection_v3.rs`
- `crates/codestory-cli/tests/architecture_contracts.rs`
- narrow module/test wiring listed by `git diff --stat 74753c17..220995d6`

**Interfaces:**

- Consumes: exact head `220995d6e7a0e227292d8b5b38edb201be0340fa` and the accepted Task-3A/3B/3C reports already present in that worktree.
- Produces: a merged, still-unregistered v3 packet execution record, explicit packet/context/search projection builders, and bounded diagnostic artifact bytes for PR 5.

- [ ] **Step 1: Freeze the current candidate**

```sh
cd .worktrees/1974-dark-packet-projections
git status --short
git rev-parse HEAD^{commit} HEAD^{tree}
shasum -a 256 plugins/codestory/generated-mcp-catalog.json
```

Expected: tracked clean, head `220995d6e7a0e227292d8b5b38edb201be0340fa`, tree `f5b12e98e0d12f37b14c981468787bc2cf43b6d3`, catalog `e96a7049922552198cc65270aeb7d4ee1c8d3d8b6974224f3c8305398bb6b7b6`.

- [ ] **Step 2: Run the focused accepted lanes once**

```sh
cargo test --locked -p codestory-runtime packet_projection_v3
cargo test --locked -p codestory-runtime packet_execution_record_v3
cargo test --locked -p codestory-agent packet_execution_plan_v3
cargo test --locked -p codestory-contracts packet_projection_v3
cargo test --locked -p codestory-cli v2_packet_context_search_projection_bytes
cargo test --locked -p codestory-cli --test architecture_contracts dark_packet_v3_preparation_stays_inert_and_unshipped
cargo check --locked -p codestory-runtime --features test-support
cargo clippy --locked -p codestory-runtime --all-targets --features test-support -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected focused counts from the accepted reports: projector 11, record 9, planner 7, contracts 2, frozen-v2 1, architecture 1; no v2 catalog or production route change.

- [ ] **Step 3: Obtain independent review, then push/open the guarded PR**

The PR targets `dev/codestory-next`, closes #1974, references #1968/#1973, and explicitly states that proof availability is unqualified and no public route exists.

- [ ] **Step 4: Merge only after exact-head review and focused CI**

Do not add qualification, packet scoring, or PR-4 work to this branch.

## Task 3: Remove repository-shaped packet ranking authority

**Files:**

- Modify: `crates/codestory-agent/src/packet_scoring.rs`
- Modify: `crates/codestory-runtime/src/agent/orchestrator.rs` tests only unless a production call-site bug is found

**Interfaces:**

- Consumes: the existing production `rank_packet_evidence` ordering path and the narrowly scoped `packet_drop_unrequested_python_siblings` formatting rule.
- Produces: repository- and language-neutral ranking behavior with unchanged typed-role scoring; PR 5 may switch packet projections only after this child merges.

- [ ] **Step 1: Add failing agent ranking tests**

Add tests named `python_sources_are_not_globally_demoted_without_a_language_term` and `collections_path_alone_has_no_rank_authority`.

The first compares equivalent Python and non-Python citations under a neutral request-flow question. The second compares equivalent typed evidence under `src/collections/` and another path.

Run:

```sh
cargo test --locked -p codestory-agent python_sources_are_not_globally_demoted_without_a_language_term -- --exact
cargo test --locked -p codestory-agent collections_path_alone_has_no_rank_authority -- --exact
```

Expected RED: Python loses by 100 and the collections path gains 4.

- [ ] **Step 2: Remove only the two global bonuses**

Delete `packet_unrequested_python_source_rank_bonus` and its production call. Delete the `/collections/` path bonus. Preserve `packet_drop_unrequested_python_siblings` and its runtime-formatting/replacement guard.

- [ ] **Step 3: Add production-ordering regressions**

In `orchestrator.rs` add tests named `flask_shaped_request_path_survives_production_packet_ranking` and `mixed_language_formatting_suppression_requires_a_replacement`.

Call `rank_packet_evidence`, not a test-only scoring substitute. Use test fixtures only; add no repository or expected-answer literal to production code.

- [ ] **Step 4: Run focused GREEN**

```sh
cargo test --locked -p codestory-agent packet_scoring
cargo test --locked -p codestory-runtime --lib agent::orchestrator::tests::flask_shaped_request_path_survives_production_packet_ranking -- --exact
cargo test --locked -p codestory-runtime --lib agent::orchestrator::tests::mixed_language_formatting_suppression_requires_a_replacement -- --exact
cargo clippy --locked -p codestory-agent --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

- [ ] **Step 5: Commit and open the scoring PR**

```sh
git add crates/codestory-agent/src/packet_scoring.rs crates/codestory-runtime/src/agent/orchestrator.rs
git commit -m "remove repository-shaped packet rank bias"
```

Close only the packet-scoring child; reference #1968 and #1977. Do not claim proof improvement.

## Task 4: Complete dark MCP machinery with a qualification measurement seam

**Files:**

- Modify: `crates/codestory-cli/src/stdio_catalog.rs`
- Modify: `crates/codestory-cli/src/stdio_transport.rs`
- Modify: `crates/codestory-cli/src/lib.rs`
- Modify: `plugins/codestory/scripts/codestory-mcp.cjs`
- Modify: `crates/codestory-cli/tests/architecture_contracts.rs`
- Add: revision-native modules chosen by #1978 under `crates/codestory-cli/src/stdio_v3/`

**Interfaces:**

- Consumes: PR 3's dark packet/context/search projections, PR 2's dark proof JSON projection, and the four MCP revision contracts from the original v3 spec.
- Produces: the unregistered `measure_revision_native_proof_result_v3(root: &serde_json::Value) -> Result<Vec<RevisionNativeToolResultMeasurementV3>, StdioV3InternalError>` seam, where each measurement owns one negotiated revision, exact `CallToolResult` bytes, and byte length. Task 5 is the only cross-crate exposure path.

- [ ] **Step 1: Execute #1978's RED/GREEN profile, batch, schema, error, registry, and digest matrix**

Keep every new entry point behind an unselected v3 facade. The current v2 initialize, lists, tool results, constants, and generated catalog remain byte-stable.

- [ ] **Step 2: Add the dark measurement function**

Expose a crate-private function under the existing dark/test-support v3 facade. It receives the already-built proof projection and returns exact serialized `CallToolResult` bytes for each of the four revision profiles. It must call the same PR-4 result builder and budget code; it may not approximate JSON wrapping. Q1 will expose this function across a sealed feature after the runtime qualification feature exists.

- [ ] **Step 3: Add a failing architecture test before exposing it**

Test name:

```text
proof_qualification_transport_measurement_is_bench_only_and_unregistered
```

Expected RED: no feature or architecture rule exists. GREEN requires that no production dependency, tool catalog, command enum, HTTP route, launcher route, or generated file enables/references the seam.

- [ ] **Step 4: Run PR-4 focused verification**

```sh
cargo test --locked -p codestory-cli --test stdio_protocol_contracts
cargo test --locked -p codestory-cli --test architecture_contracts
node --test plugins/codestory/tests/plugin-static.test.mjs
node scripts/generate-codestory-skill-syntax.mjs --check --cli target/debug/codestory-cli
cargo fmt --all -- --check
git diff --check
```

- [ ] **Step 5: Commit review-sized PR-4 commits and close #1978**

The PR remains public-v2 neutral and references Q1 as the only consumer of its sealed measurement seam.

## Task 5: Add the benchmark-only proof qualification feature boundary

**Files:**

- Modify: `crates/codestory-agent/Cargo.toml`
- Modify: `crates/codestory-agent/src/lib.rs`
- Modify: `crates/codestory-runtime/Cargo.toml`
- Modify: `crates/codestory-runtime/src/lib.rs`
- Add: `crates/codestory-runtime/src/proof_qualification_support.rs`
- Modify: `crates/codestory-cli/Cargo.toml`
- Modify: `crates/codestory-cli/src/lib.rs`
- Modify: `crates/codestory-bench/Cargo.toml`
- Modify: `crates/codestory-cli/tests/architecture_contracts.rs`

**Interfaces:**

- Consumes: the existing dark agent/runtime modules and PR 4's crate-private `measure_revision_native_proof_result_v3` seam.
- Produces: feature-gated `codestory_agent::proof_qualification_support`, `codestory_runtime::proof_qualification_support`, and `codestory_cli::proof_qualification_support` facades that only `codestory-bench` can enable. No facade is present in a default product build.

- [ ] **Step 1: Write the architecture RED**

Add a test named `proof_qualification_support_is_bench_only_and_never_a_product_feature`.

It requires the two exact feature names, permits them only on `codestory-bench` dependencies, and rejects them from CLI default/product features, plugin files, generated catalogs, and current public routes.

Run:

```sh
cargo test --locked -p codestory-cli --test architecture_contracts proof_qualification_support_is_bench_only_and_never_a_product_feature -- --exact
```

Expected RED: required feature declarations and sealed facade are missing.

- [ ] **Step 2: Add the minimal features and facade**

Gate the existing dark modules with:

```rust
#[cfg(any(
    test,
    feature = "test-support",
    feature = "proof-qualification-support"
))]
```

Expose only qualification-owned request/observation functions from `proof_qualification_support.rs`; do not make `indexed_source_call_path_v1` public.

In `crates/codestory-cli/Cargo.toml`, add:

```toml
proof-qualification-support = ["codestory-runtime/proof-qualification-support"]
```

The gated CLI wrapper calls PR 4's crate-private revision-native measurement function. It does not register a tool or transport route.

- [ ] **Step 3: Wire `codestory-bench`**

Move `codestory-agent`, `codestory-runtime`, `codestory-cli`, `codestory-store`, `codestory-indexer`, and `codestory-contracts` needed by the binary into normal benchmark dependencies. Enable only `proof-qualification-support` and existing `benchmark-support`; never `test-support`.

- [ ] **Step 4: Prove the feature graph**

```sh
cargo check --locked -p codestory-agent --features proof-qualification-support
cargo check --locked -p codestory-runtime --features proof-qualification-support
cargo check --locked -p codestory-bench
cargo tree -p codestory-cli -e features | rg 'proof-qualification-support' && exit 1 || true
cargo test --locked -p codestory-cli --test architecture_contracts proof_qualification_support_is_bench_only_and_never_a_product_feature -- --exact
```

Expected: the feature appears only under the benchmark package graph.

- [ ] **Step 5: Commit**

```sh
git add Cargo.toml Cargo.lock crates/codestory-agent crates/codestory-runtime crates/codestory-bench crates/codestory-cli/tests/architecture_contracts.rs
git commit -m "add sealed proof qualification boundary"
```

## Task 6: Make proof gate failures observable without changing behavior

**Files:**

- Modify: `crates/codestory-agent/src/indexed_source_call_path_v1.rs`
- Modify: `crates/codestory-runtime/src/indexed_source_call_path_v1.rs`
- Modify: `crates/codestory-runtime/src/proof_qualification_support.rs`

**Interfaces:**

- Consumes: `admit_raw_call_edge`, `build_indexed_source_call_path_facts`, `check_built_call_path_integration`, and `project_internal_call_path_result` without changing their result semantics.
- Produces: `diagnose_raw_call_edge(&Edge, NodeId, NodeId) -> Result<AdmittedRawCallEdge, RawAdmissionFailure>` and an observed runtime builder returning `ObservedBuiltCallPathFacts { built: BuiltCallPathFacts, trace: ProofQualificationTrace }` through the sealed qualification facade.

- [ ] **Step 1: Add raw-admission reason RED tests**

Create a table test with one lawful edge and every `RawAdmissionFailure` variant. For each mutation assert both:

```rust
let diagnostic = diagnose_raw_call_edge(&edge, expected_source, expected_target);
assert_eq!(diagnostic, Err(expected_reason));
let admission = admit_raw_call_edge(&edge, expected_source, expected_target);
assert_eq!(admission, RawCallEdgeAdmission::Rejected);
```

Run:

```sh
cargo test --locked -p codestory-agent raw_admission_diagnostics_share_the_product_leaf -- --exact
```

Expected RED: `diagnose_raw_call_edge` and the reason enum do not exist.

- [ ] **Step 2: Refactor the existing predicate into one reason-bearing leaf**

Preserve conjunct order and all accepted/rejected behavior. `admit_raw_call_edge` delegates exactly as shown in Section 4.

- [ ] **Step 3: Add runtime gate-trace RED tests**

Add source-built and hostile Store fixtures for selector, containment, source binding, line, receipt integration, and projection failures. Assert:

- product `BuiltCallPathFacts`/`ProofDisposition` remain byte-for-byte/equality identical before and after observation;
- the trace chooses the first zero-survivor gate;
- candidate histograms are edge-ID deterministic;
- a proven prefix remains authoritative where current product behavior retains it;
- the existing all-selector early return is reported honestly rather than simulated away.

Expected RED: trace types and observed builder are missing.

- [ ] **Step 4: Add the observed internal builder**

Have one internal function return:

```rust
struct ObservedBuiltCallPathFacts {
    built: BuiltCallPathFacts,
    trace: ProofQualificationTrace,
}
```

The existing builder discards `trace`; the sealed qualification facade returns both. The checker and projector still consume `built` through `check_built_call_path_integration` and `project_internal_call_path_result`.

- [ ] **Step 5: Run GREEN and mutation coverage**

```sh
cargo test --locked -p codestory-agent indexed_source_call_path_v1
cargo test --locked -p codestory-runtime indexed_source_call_path_v1
cargo check --locked -p codestory-runtime --features proof-qualification-support
cargo clippy --locked -p codestory-agent --all-targets --features proof-qualification-support -- -D warnings
cargo clippy --locked -p codestory-runtime --all-targets --features proof-qualification-support -- -D warnings
```

- [ ] **Step 6: Commit**

```sh
git add crates/codestory-agent/src/indexed_source_call_path_v1.rs crates/codestory-runtime/src/indexed_source_call_path_v1.rs crates/codestory-runtime/src/proof_qualification_support.rs
git commit -m "diagnose proof availability gates"
```

## Task 7: Add closed qualification artifact contracts and CLI skeleton

**Files:**

- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/codestory-bench/Cargo.toml`
- Add: `crates/codestory-bench/src/bin/codestory_proof_availability.rs`
- Add: `crates/codestory-bench/src/bin/codestory_proof_availability/{mod.rs,cli.rs,contracts.rs,corpus.rs,util.rs}`
- Add: `crates/codestory-bench/tests/proof_availability_contracts.rs`
- Add: `benchmarks/proof-availability/schemas/{corpus,path,report,thresholds}.schema.json`

**Interfaces:**

- Consumes: the typed failure and measurement types produced by Tasks 4-6.
- Produces: closed `CorpusV1`, `OraclePathV1`, `ThresholdsV1`, `EnvironmentReportV1`, `InventoryReportV1`, `TrailReportV1`, `CaseReportV1`, `FailureFunnelReportV1`, `QualificationSummaryV1`, and `ActivationDecisionV1` DTOs; canonical schema files; and a `codestory-proof-availability` binary with `materialize`, `run`, and `verify` command shapes.

- [ ] **Step 1: Add failing closed-contract tests**

Test valid maximal fixtures plus unknown fields, duplicate IDs, non-40-hex commits, missing source hashes, wrong path counts, wrong two-mutation count, and source ranges outside file bytes.

Run:

```sh
cargo test --locked -p codestory-bench --test proof_availability_contracts
```

Expected RED: binary contracts and schema files are absent.

- [ ] **Step 2: Implement closed DTOs**

Use `serde(deny_unknown_fields)`, bounded constructors, closed enums, and exact schema strings such as `codestory.proof-availability-corpus/v1`. Add workspace-pinned `schemars = "1"` with derive support and generate the four checked-in root-object schemas from the DTOs; commit the resolved version in `Cargo.lock`.

- [ ] **Step 3: Add CLI parsing**

Clap subcommands are exactly `materialize`, `run`, and `verify`. `materialize --verify-only` may create or refresh detached source checkouts and validate every oracle range/hash, but it must skip indexing and proof execution. Reject relative corpus/output paths only where repository policy requires absolute paths; always refuse output-directory overwrite unless `verify` is read-only.

- [ ] **Step 4: Add schema parity tests**

Generate schemas in memory and compare canonical JSON to checked-in files. Populate every optional field and every enum/tagged-union variant at least once.

- [ ] **Step 5: Run GREEN and help smoke**

```sh
cargo test --locked -p codestory-bench --test proof_availability_contracts
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- --help
```

Expected: three subcommands, no network or indexing during `--help`.

- [ ] **Step 6: Commit**

```sh
git add Cargo.toml Cargo.lock crates/codestory-bench benchmarks/proof-availability/schemas
git commit -m "add proof availability artifact contracts"
```

## Task 8: Implement graph inventory and exact trail denominators

**Files:**

- Add: `crates/codestory-bench/src/bin/codestory_proof_availability/inventory.rs`
- Add: `crates/codestory-bench/src/bin/codestory_proof_availability/trails.rs`
- Add: `benchmarks/proof-availability/sql/raw-call-inventory.sql`
- Add: `benchmarks/proof-availability/sql/connected-trails.sql`

**Interfaces:**

- Consumes: a freshly materialized `Store` plus Task 6's actual reason-bearing admission leaf.
- Produces: deterministic `InventoryReportV1` and `TrailReportV1` values with effective, exact-resolved, and strictly admitted counts for lengths 1 through 6; Task 11 serializes them unchanged.

- [ ] **Step 1: Add synthetic graph RED tests**

Cover unresolved targets, probable/certain edges, repeated vertices, self edges, parallel edge IDs, and forbidden edge reuse. Hand-calculate lengths 1 through 6.

Expected RED: no inventory/trail counter.

- [ ] **Step 2: Implement inventory through Store + actual kernel**

Call `Store::get_edges()` and `diagnose_raw_call_edge(edge, edge.effective_source(), edge.effective_target())`. Reconcile all certainty/resolution/admission buckets to total `CALL` rows.

- [ ] **Step 3: Implement edge-distinct trail counting**

Build sorted adjacency for effective, exact-resolved, and admitted edge sets. DFS to depth 6 with a six-ID used-edge stack, repeated vertices allowed, checked `u128` counts, and deterministic source/edge order.

- [ ] **Step 4: Add SQL diagnostic definitions**

The SQL files name schema assumptions, endpoint formulas, unresolved handling, edge-distinct encoding, and lengths 1-6. Tests compare SQL and Rust on a bounded synthetic database; the Rust/kernal result remains authoritative.

- [ ] **Step 5: Run GREEN**

```sh
cargo test --locked -p codestory-bench proof_availability::inventory
cargo test --locked -p codestory-bench proof_availability::trails
```

- [ ] **Step 6: Commit**

```sh
git add crates/codestory-bench/src/bin/codestory_proof_availability benchmarks/proof-availability/sql
git commit -m "count proof inventory and connected trails"
```

## Task 9: Freeze role thresholds and decision logic before results

**Files:**

- Add: `crates/codestory-bench/src/bin/codestory_proof_availability/thresholds.rs`
- Add: `benchmarks/proof-availability/thresholds-v1.json`
- Add: `benchmarks/proof-availability/methodology.md`

**Interfaces:**

- Consumes: `QualificationSummaryV1`, recomputed hard-gate and role observations, the frozen role thresholds in Section 8, and optional closed source-dependency evidence.
- Produces: a decision report containing the selected A/B/C/D outcome, every failed gate, raw numerators/denominators, unrounded and presentation Wilson values, cohort rows, latency/size observations, hard-gate counts, and domain-separated observation/evidence digests. Task 13 records this report verbatim in `decision.json`; no ninth artifact is added.

- [ ] **Step 1: Add threshold boundary RED tests**

Fixtures cover 95/120 versus 96/120, 11/30 versus 12/30, one false proof, 311/312 classified steps, one invalid payload, and every A/B/C/D decision branch.

Expected RED: evaluator and frozen threshold document are absent.

- [ ] **Step 2: Implement Wilson bounds and hard-gate precedence**

Use `z = 1.959963984540054`. Hard failures select C with `hard_gate_failed` unless a demonstrated integration dependency independently selects D. Then evaluate automatic, stable, experimental, dark in that order.

- [ ] **Step 3: Bind threshold identity**

Every result records the canonical SHA-256 of `thresholds-v1.json`. `run` refuses a threshold file changed after corpus freeze.

- [ ] **Step 4: Run GREEN**

```sh
cargo test --locked -p codestory-bench proof_availability::thresholds
```

- [ ] **Step 5: Commit before any corpus execution**

```sh
git add crates/codestory-bench/src/bin/codestory_proof_availability/thresholds.rs benchmarks/proof-availability/thresholds-v1.json benchmarks/proof-availability/methodology.md
git commit -m "freeze proof activation thresholds"
```

## Task 10: Curate and independently freeze the four-cohort oracle corpus

**Files:**

- Add: `benchmarks/proof-availability/corpus-v1.json`
- Add: `benchmarks/proof-availability/paths/{codestory-rust,vite-ts-js,flask-python,gin-go}.json`
- Modify: `benchmarks/proof-availability/methodology.md`

**Interfaces:**

- Consumes: the four pinned repositories/commits in Section 5 and the closed Task-7 corpus schemas.
- Produces: one frozen `corpus-v1.json` referencing four independently reviewed 30-path oracle files, their exact canonical hashes, and the already frozen threshold hash. It contains no CodeStory result data.

- [ ] **Step 1: Add repository declarations and source materializer**

Pin the exact URLs/commits/workspaces from Section 5. `materialize` clones to a new `target/proof-availability/workspaces` directory, detaches at the SHA, rejects submodules or setup drift not declared by the corpus, and records a deterministic `git ls-tree` digest.

- [ ] **Step 2: Curate CodeStory's 30 paths without running CodeStory proof**

Use source reading only. Record the 10/7/5/3/3/2 length distribution, hashes, ranges, typed contract, and two absent-relation mutations per positive.

- [ ] **Step 3: Repeat for Vite, Flask, and Gin**

Commit each cohort separately after local schema/source validation:

```sh
git commit -m "add codestory proof oracle paths"
git commit -m "add vite proof oracle paths"
git commit -m "add flask proof oracle paths"
git commit -m "add gin proof oracle paths"
```

- [ ] **Step 4: Obtain independent source review**

The reviewer receives pinned source plus oracle files, not CodeStory benchmark output. Every step must have exact caller/callsite/target agreement. Correct the oracle before freeze; never after results.

- [ ] **Step 5: Freeze corpus identity**

Write curator/reviewer IDs, review date, path counts, source-tree digests, and canonical hashes for all four path files and thresholds into `corpus-v1.json`.

- [ ] **Step 6: Prove the runner has not been used**

The Q1 diff must contain no `benchmarks/proof-availability/results/` directory and no result-derived edits to paths or thresholds.

- [ ] **Step 7: Validate**

```sh
cargo run --locked -p codestory-bench --bin codestory-proof-availability -- materialize \
  --verify-only \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace target/proof-availability/oracle-workspaces \
  --out target/proof-availability/oracle-environment.json
git diff --check
```

Expected: all pinned source ranges and hashes validate; no database, proof result, or `benchmarks/proof-availability/results/` artifact is created.

## Task 11: Implement materialization and actual-kernel case execution

**Files:**

- Add: `crates/codestory-bench/src/bin/codestory_proof_availability/{materialize.rs,runner.rs,report.rs}`
- Modify: `crates/codestory-bench/src/bin/codestory_proof_availability/mod.rs`
- Add: `docs/testing/proof-availability-v1.md`

**Interfaces:**

- Consumes: the frozen corpus/threshold DTOs, Task 8 inventory/trail counters, Task 6 observed runtime facade, and Task 4 revision-native measurement facade.
- Produces: atomic `environment.json`, `inventory.json`, `trails.json`, `cases.json`, `failure-funnel.json`, `summary.json`, `decision.json`, and `findings.md` artifacts. `decision.json` embeds recomputable derived observations and optional closed source-dependency evidence while preserving this exact eight-artifact set. The executable never substitutes SQL or benchmark logic for a product disposition.

- [ ] **Step 1: Add materialization RED tests**

Use local fixture repositories to test wrong commit, dirty checkout, source-range mismatch, stale/missing index, schema mismatch, mixed generation, and output overwrite refusal.

- [ ] **Step 2: Implement fresh full indexing**

Construct explicit `SidecarProcessDefaults` with the harness cache root, use Local profile, bind the project through `Runtime::project_service`, and run one full core index through `IndexService`. Record source head/tree, qualification binary SHA, store schema, DB SHA, file/node/edge counts, core generation/run, and freshness. Do not initialize semantic retrieval.

- [ ] **Step 3: Add actual-kernel execution RED tests**

Run one source-built positive and both negative mutations through the sealed runtime API. Assert exact product disposition, authoritative receipts, trace, projection bytes, and no retrieval publication/activation.

- [ ] **Step 4: Implement case execution**

For each case:

1. revalidate source oracle bytes;
2. convert the closed benchmark DTO to the existing unvalidated contract;
3. call existing contract validation;
4. enter the existing core-only public operation;
5. build observed facts;
6. call checked integration and projection;
7. ask PR 4's sealed transport builder for all four complete result sizes;
8. record stage durations with one monotonic clock;
9. compare receipts to oracle;
10. write one immutable case row.

- [ ] **Step 5: Implement atomic reports**

Write environment, inventory, trails, cases, funnel, summary, decision, and findings to a new directory through temp-file + rename. Omit absolute local paths, environment variables, logs, source text beyond bounded contract fields, and secrets.

- [ ] **Step 6: Add deterministic report tests**

Shuffle Store rows and input case order; canonical report bytes and aggregates must remain identical. Timestamps belong only to `environment.json` and do not enter decision hashes.

- [ ] **Step 7: Run GREEN**

```sh
cargo test --locked -p codestory-bench proof_availability
cargo check --locked -p codestory-bench
cargo clippy --locked -p codestory-bench --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 8: Commit**

```sh
git add crates/codestory-bench/src/bin/codestory_proof_availability docs/testing/proof-availability-v1.md
git commit -m "run exact proof availability qualification"
```

## Task 12: Close Q1 with focused source proof

**Files:** all Q1 changes from Tasks 5-11, plus `docs/superpowers/codestory-v3-retrieval-proof-separation.md`.

**Interfaces:**

- Consumes: every independently reviewable Q1 commit from Tasks 5-11.
- Produces: one merged, production-dark qualification harness and a dated spec amendment. It does not produce a qualification result or select an activation outcome.

- [ ] **Step 1: Amend the original design before closing Q1**

Add a dated “Proof availability amendment” that links this plan, preserves the proof contract, and replaces only the fixed delivery/activation sections. Keep the historical PR-1/PR-2 descriptions intact.

- [ ] **Step 2: Run focused Q1 gates once on the accepted head**

```sh
cargo fmt --all -- --check
cargo check --locked -p codestory-agent --features proof-qualification-support
cargo check --locked -p codestory-runtime --features proof-qualification-support
cargo check --locked -p codestory-bench
cargo clippy --locked -p codestory-agent --all-targets --features proof-qualification-support -- -D warnings
cargo clippy --locked -p codestory-runtime --all-targets --features proof-qualification-support -- -D warnings
cargo clippy --locked -p codestory-bench --all-targets -- -D warnings
cargo test --locked -p codestory-agent indexed_source_call_path_v1
cargo test --locked -p codestory-runtime indexed_source_call_path_v1
cargo test --locked -p codestory-bench proof_availability
cargo test --locked -p codestory-cli --test architecture_contracts
node .github/scripts/check-doc-links.mjs
git diff --check
```

- [ ] **Step 3: Verify non-shipping boundaries**

```sh
cargo test --locked -p codestory-cli --test architecture_contracts proof_qualification_support_is_bench_only_and_never_a_product_feature -- --exact
shasum -a 256 plugins/codestory/generated-mcp-catalog.json
```

Expected: the architecture feature-graph scan passes; catalog remains the accepted v2/v3-dark baseline for the current integration head.

- [ ] **Step 4: Independent adversarial review**

The reviewer checks corpus independence, diagnostic/product identity, denominator definitions, threshold freeze, negative mutations, and feature darkness. Review does not run the real corpus.

- [ ] **Step 5: Push/open Q1 PR**

Close Q1 only. Reference #1973 and #1977. State that no availability result or activation decision exists yet.

## Task 13: Run Q2 on one exact clean head and commit the evidence

**Files generated:**

- Add: `benchmarks/proof-availability/results/{qualification_id}/*.json`
- Add: `benchmarks/proof-availability/results/{qualification_id}/findings.md`

**Interfaces:**

- Consumes: one exact clean post-PR3/scoring/PR4/Q1 source head, the frozen corpus and thresholds, and one locked release build of `codestory-proof-availability`.
- Produces: an independently reviewed immutable result directory and a machine-selected A/B/C/D decision for PR 5. No code, corpus, or threshold file changes in this task.

- [ ] **Step 1: Freeze the source candidate**

After PR 3, scoring, PR 4, and Q1 are merged, create a clean dedicated worktree at the exact live `origin/dev/codestory-0.18`. Record commit/tree and confirm no known source change is pending. Do not source the candidate from, merge it into, or otherwise mutate the concurrent 0.17.4 release lane.

- [ ] **Step 2: Build once**

```sh
cargo build --release --locked -p codestory-bench --bin codestory-proof-availability
qualification_bin=target/release/codestory-proof-availability
shasum -a 256 "$qualification_bin"
git status --short
git rev-parse HEAD^{commit} HEAD^{tree}
```

Expected: clean tracked tree. Record the command, binary SHA, Rust host/toolchain, OS/architecture, and source commit/tree in `environment.json`.

- [ ] **Step 3: Create one immutable qualification ID**

```sh
qualification_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=12 HEAD)"
run_root="target/proof-availability/$qualification_id"
test ! -e "$run_root"
mkdir -p "$run_root"
```

- [ ] **Step 4: Materialize, run, and verify once**

```sh
"$qualification_bin" materialize \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace "$run_root/workspaces" \
  --cache-root "$run_root/cache" \
  --out "$run_root/environment.json"

"$qualification_bin" run \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --environment "$run_root/environment.json" \
  --out "$run_root/results"

"$qualification_bin" verify \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --results "$run_root/results"
```

If and only if independent source review supplies valid outcome-D evidence, add the same `--source-dependency <EVIDENCE_JSON>` argument to both `run` and `verify`. The default Q2 command remains exactly as shown.

Do not rerun unchanged failures. A source/oracle/logic failure invalidates the run and returns to Q1; a product result is evidence even when it selects C.

- [ ] **Step 5: Review raw reconciliation before copying results**

Require:

- 120 positives, 312 positive steps, 240 negatives;
- exact repository counts and publication identities;
- inventory bucket sum equals all `CALL` rows;
- first-failure sum equals 312;
- every receipt comparison recorded;
- all four profile sizes recorded;
- no local absolute path or secret in output.

- [ ] **Step 6: Copy machine results and generate findings**

Copy only `results` artifacts to `benchmarks/proof-availability/results/$qualification_id`. `findings.md` is generated from the same case rows and names reproduced measurements, inferences, and selected thresholds separately.

- [ ] **Step 7: Independent exact-artifact review**

The reviewer receives the source head/tree, binary SHA, frozen corpus/threshold hashes, result directory, and source checkouts. It independently samples at least five positive and five negative cases per cohort and recomputes the decision from machine rows.

- [ ] **Step 8: Commit result artifacts without changing code/corpus/thresholds**

```sh
git add "benchmarks/proof-availability/results/$qualification_id"
git diff --cached --name-only | rg -v "^benchmarks/proof-availability/results/$qualification_id/" && exit 1 || true
git commit -m "record exact proof availability decision"
```

- [ ] **Step 9: Open Q2 PR and update #1977 with the selected branch**

Close Q2 only after artifact review. Do not close #1973 at this stage.

## Task 14: Handle a measured dominant failure without moving the goalposts

This task is conditional.

**Files:**

- Create only when the trigger passes: one GitHub owner epic plus a separate focused implementation plan under `docs/superpowers/plans/`
- Modify: no product source, corpus, threshold, or Q2 result file in this task

**Interfaces:**

- Consumes: Q2's exact first-failure rows and per-cohort aggregates.
- Produces: either no new issue, or one owner-specific remediation issue/plan tied to exact case IDs and the 50%-in-two-cohorts trigger. Any implemented remediation invalidates the prior Q2 result.

- [ ] **Step 1: Test the exact trigger**

Create an upstream exact-call-resolution epic only when certainty absent/probable/uncertain plus missing exact resolved target account for at least 50% of failed expected steps in at least two cohorts.

- [ ] **Step 2: If the trigger is false, do not create the epic**

Route any other dominant failure to its owner (selector, containment, source publication, receipt integration, or budget) with the exact case IDs. Write a new focused plan before changing product code.

- [ ] **Step 3: If remediation changes kernel/indexer/corpus/thresholds, invalidate Q2**

Land the focused PR, keep strict admission unchanged unless a soundness bug was proved, then issue a new qualification ID and rerun Task 13. Do not compare a changed candidate against the old decision as though it were the same run.

- [ ] **Step 4: Stop after two failed revisions of the same shape**

Redesign the failing seam rather than adding repository-specific heuristics.

## Task 15: Implement the one public v3 cut selected by Q2

**Files:** the existing PR-5 files from the original design spec, adjusted by outcome.

- Modify: public DTO/projection wiring in `crates/codestory-contracts`, `codestory-runtime`, and `codestory-cli`
- Modify: `crates/codestory-cli/src/{args,app,stdio_catalog,stdio_transport,output}.rs`
- Modify: `plugins/codestory/scripts/codestory-mcp.cjs`
- Modify: generated catalog/skill/help/docs and `CHANGELOG.md`
- Modify: all CodeStory publication schema constants/mirrors exactly once

**Interfaces:**

- Consumes: merged dark PR 3/PR 4 machinery, the scoring fix, Q2's immutable decision, and unchanged Q2 proof/kernel/corpus/threshold hashes.
- Produces: exactly one public CodeStory schema-3 contract: A registers stable CLI+MCP proof, B registers experimental CLI proof only, C registers no proof surface, and D produces no public cut.

- [ ] **Step 1: Write the outcome-specific registration RED**

Common RED: packet/context/search v3 projections are not registered and legacy `Supported` is still public.

Outcome A RED: stable CLI/MCP proof route missing.

Outcome B RED: `codestory-cli experimental prove-call-path` missing while MCP/catalog remains proof-free.

Outcome C RED: any proof route/catalog/skill reference is a failure; only evidence surfaces register.

- [ ] **Step 2: Switch packet/context/search in every outcome**

Remove packet truth authority, packet-only `include_evidence`, and public proof-like search/context vocabulary. Enable diagnostics, revision-native MCP behavior, output validation, discovery digests, and CodeStory schema 3 once.

- [ ] **Step 3: Activate only the selected proof surface**

**Outcome A:** register `prove_call_path` in CLI and MCP. If only stable-explicit thresholds passed, the skill says explicit verification only. If automatic thresholds passed, the skill may recommend it for the exact supported domain.

**Outcome B:** add only `codestory-cli experimental prove-call-path --project <ROOT> --spec <PATH|->`; no MCP tool, schema, resource, prompt, skill instruction, or launcher route.

**Outcome C:** keep both proof modules and qualification support dark. Preserve their focused CI tests and document the current Q2 decision.

**Outcome D:** do not implement this task; file the demonstrated inseparability blocker.

- [ ] **Step 4: Add route exclusivity tests**

Assert every selected v3 route is registered once, unselected proof surfaces are absent, legacy `Supported` cannot serialize publicly, and no benchmark/corpus/threshold name appears in production behavior.

- [ ] **Step 5: Run focused integration gates**

```sh
cargo test --locked -p codestory-cli --test stdio_protocol_contracts
cargo test --locked -p codestory-cli --test architecture_contracts
cargo test --locked -p codestory-runtime --lib agent::packet_compiler::tests
cargo test --locked -p codestory-runtime --lib agent::packet_batch::
cargo test --locked -p codestory-agent
node --test scripts/tests/install-codestory-dev-plugin.test.mjs
node --test plugins/codestory/tests/plugin-static.test.mjs
node scripts/generate-codestory-skill-syntax.mjs --check --cli target/debug/codestory-cli
```

- [ ] **Step 6: Commit the atomic public switch**

Use one final integration commit after all preparatory commits are present:

```sh
git commit -m "publish evidence-only codestory v3"
```

No version bump, tag, release, or marketplace publication.

## Task 16: Final exact-head verification and issue closure

**Files:**

- Read-only verification: final workspace source, generated catalog, built CLI, installed CodeStoryDev package, and fresh host result
- Modify: GitHub issues #1968, #1973, and #1977 only as allowed by the selected outcome
- Add: no release, version, tag, or marketplace artifact

**Interfaces:**

- Consumes: the final PR-5 source head/tree and Q2 lineage hashes.
- Produces: one exact-source build/install/live-host acceptance record for the selected outcome, plus issue closure that matches what actually shipped. It performs no version, release, tag, or marketplace mutation.

- [ ] **Step 1: Freeze the final PR-5 head/tree**

Confirm clean/pushed and no known source change. Any later source or wire edit invalidates this task.

- [ ] **Step 2: Run the final source gates once**

```sh
cargo fmt --all -- --check
cargo check --locked --workspace
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo nextest run --locked --workspace --no-fail-fast
cargo test --locked --workspace --doc
cargo test --locked -p codestory-cli --test stdio_protocol_contracts
node scripts/generate-codestory-skill-syntax.mjs --check --cli target/debug/codestory-cli
node --test scripts/tests/install-codestory-dev-plugin.test.mjs
node --test plugins/codestory/tests/plugin-static.test.mjs
node .github/scripts/check-doc-links.mjs
git diff --check
```

- [ ] **Step 3: Revalidate Q2 lineage**

The final proof kernel, runtime fact builder, qualification logic, corpus, and thresholds must match the hashes in Q2. If any changed, rerun Q2. Adapter-only changes require fresh transport size/schema checks but not a new availability corpus run.

- [ ] **Step 4: Build/install one exact CodeStoryDev candidate**

Record clean head/tree, locked build command, CLI SHA, installed receipt SHA, live host CLI SHA, schema 3 stamp, negotiated revision, and discovery digest. A receipt alone is not source-build provenance.

- [ ] **Step 5: Run surface-specific acceptance**

- Outcome A: run the 16-prompt translator conformance set and require zero false `ContractProven`, zero silent material omissions, and installed p95 within the selected role.
- Outcome B: run direct experimental CLI DTO-equivalence and latency cases; no MCP/host translation claim.
- Outcome C: prove packet/context/search v3 in a fresh host and prove the MCP catalog contains no proof tool.

- [ ] **Step 6: Close issues honestly**

- Outcome A: close #1968, #1977, and #1973 after installed acceptance.
- Outcome B/C: close #1968 and #1977; keep #1973 open with the Q2 decision and next requalification trigger.
- Outcome D: keep all public-cut parents open.

- [ ] **Step 7: Stop**

Do not bump a version, publish a release, tag, edit the marketplace, or claim production compatibility.
