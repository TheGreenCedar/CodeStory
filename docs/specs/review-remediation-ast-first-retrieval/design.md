# Design Document

## Overview

This design describes how to convert the review findings into a mergeable branch. It does not introduce a new subsystem. It tightens existing boundaries in tests, sidecar runtime behavior, benchmark evidence handling, CLI security, packet sufficiency output, readiness reporting, performance budgets, language support, and documentation.

## Principles

- Default gates must be deterministic and safe without live services.
- Live-service checks must be explicit and named.
- Publishable evidence must not consume expected answers as inputs.
- Runtime failures must be visible and actionable.
- Local CLI features must not read or copy outside trusted roots.
- Structured JSON fields must stay machine-readable, not prose-shaped.
- Documentation must describe what the code and workflow actually do.

## Component Specifications

### Component: TestGateHygiene

**Purpose**: Keep default Rust and Node verification deterministic, offline-safe, and green.

**Locations**:

- `crates/codestory-retrieval/src/query.rs`
- `crates/codestory-retrieval/tests/*`
- `scripts/tests/*`
- `docs/ops/retrieval-sidecars.md`

**Interface**:

```text
Implements Req 1.1, 1.2, 1.3, 1.4

Default command:
  cargo test -p codestory-retrieval

Live command:
  cargo test -p codestory-retrieval -- --ignored --nocapture
  or CODESTORY_LIVE_SIDECAR_TESTS=1 cargo test -p codestory-retrieval <test-filter> -- --nocapture
```

**Design Notes**:

- Move `integration_query_against_fixture_manifest` behind `#[ignore = "..."]` or an env guard.
- Replace shallow reachability skip with either a full preflight or an explicit live-only failure message.
- Add a mock executor or fixture-level test that exercises retrieval query behavior without real Qdrant/Zoekt.
- Avoid `expect("index")` in live tests where sidecar failure is expected environmental behavior.

### Component: ReleaseProofLedger

**Purpose**: Ensure branch-head release proof is fresh, recorded, and clearly scoped.

**Locations**:

- `docs/testing/codestory-e2e-stats-log.md`
- `crates/codestory-cli/tests/codestory_repo_e2e_stats.rs`
- `AGENTS.md`

**Interface**:

```text
Implements Req 2.1, 2.2, 2.3, 2.4

Required commands:
  cargo build --release -p codestory-cli
  cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

**Design Notes**:

- The stats row must cite the current commit short hash.
- If the row is stats-only or uses skip allowances, state that explicitly.
- If docs were changed after sidecar hashing, rerun `ready` or `doctor` before claiming current full-sidecar readiness.

### Component: SidecarErrorBoundary

**Purpose**: Propagate sidecar candidate-resolution and search failures as explicit unavailable states.

**Locations**:

- `crates/codestory-runtime/src/agent/retrieval_primary.rs`
- `crates/codestory-runtime/src/lib.rs`
- `crates/codestory-runtime/tests/retrieval_primary_rejection.rs`
- `crates/codestory-runtime/src/agent/packet_batch.rs`

**Interface**:

```rust
// Implements Req 3.1, 3.2, 3.3, 3.4

fn try_sidecar_primary_search(...) -> Option<SidecarPrimarySearchOutcome>;

fn search_results_sidecar_primary(...) -> Result<SearchResponse, ApiError>;

// Error mapping must include:
// "candidate resolution failed"
```

**Design Notes**:

- Replace both `unwrap_or_default()` calls around candidate resolution.
- Mirror packet batch's existing `sidecar_retrieval_unavailable_error` behavior.
- Tests should simulate candidate-resolution failure independent of sidecar HTTP availability.

### Component: BenchmarkEvidenceBoundary

**Purpose**: Separate diagnostic/oracle-assisted benchmark rows from publishable product evidence.

**Locations**:

- `scripts/codestory-agent-ab-benchmark.mjs`
- `scripts/codestory-agent-ab-score.mjs`
- `scripts/tests/codestory-agent-ab-analyzer.test.mjs`
- `benchmarks/tasks/README.md`
- `docs/testing/agent-benchmark-harness-verification.md`
- `docs/testing/benchmark-ledger.md`

**Interface**:

```text
Implements Req 4.1, 4.2, 4.3, 4.4, 4.5

New or clarified options:
  --diagnostic-extra-probes-from-manifest
  --allow-empty-packet-gate
  --max-source-reads-after-packet <n>

Publishable blockers:
  manifest_extra_probe_strategy != null
  max_source_reads_after_packet == null for agent A/B publishable rows
  packet_gate_selected_tasks == 0 unless allow-empty flag is present
```

**Design Notes**:

- `packetManifestExtraProbes(task)` should not be called by default publishable packet prelude.
- Keep manifest-derived probes available for diagnostics, but mark rows with an explicit `evidence_mode`.
- `agentPublishableBlockers` should reject oracle-assisted and ambiguous source-read-policy rows.
- Reuse-baseline copy logic must canonicalize paths under `sourceRunDir`, reject absolute paths, and cap file size.

### Component: LocalFileBoundary

**Purpose**: Prevent CodeStory CLI and scripts from reading or copying paths outside trusted roots.

**Locations**:

- `crates/codestory-cli/src/main.rs`
- `crates/codestory-cli/tests/*`
- `scripts/codestory-agent-ab-benchmark.mjs`
- `scripts/tests/*`

**Interface**:

```rust
// Implements Req 5.1, 5.2, 5.4
fn project_contained_path(project_root: &Path, candidate: &Path) -> Option<PathBuf>;
```

```js
// Implements Req 5.3
function resolveRunArtifactPath(sourceRunDir, artifactPath) {
  // returns canonical contained path or null/error
}
```

**Design Notes**:

- Use canonical project root plus canonical candidate paths.
- Reject absolute endpoint paths unless they canonicalize inside project root.
- Reject import candidates that escape via `..`.
- For benchmark artifacts, permit only known artifact basenames or files inside the source run directory.

### Component: ModelArtifactIntegrity

**Purpose**: Verify downloaded retrieval model artifacts before storing or using them.

**Locations**:

- `scripts/setup-retrieval-env.mjs`
- `docs/ops/retrieval-sidecars.md`
- `docs/contributors/getting-started.md`
- `.agents/skills/codestory-grounding/references/setup.md`

**Interface**:

```js
// Implements Req 6.1, 6.2, 6.3, 6.4
const BGE_GGUF_SHA256 = "...";

async function fetchEmbedModel() {
  // download to temp, hash, compare, rename
}
```

**Design Notes**:

- Write to `dest + ".tmp"` or a unique temp path.
- Hash the full buffer or streaming download before rename.
- Treat fallback mirrors as explicit opt-in unless the mirror is verified by the same checksum.
- Do not leave failed partial downloads in the final path.

### Component: PacketSufficiencyContract

**Purpose**: Emit deterministic, typed, and semantically honest packet sufficiency fields.

**Locations**:

- `crates/codestory-contracts/src/api/dto.rs`
- `crates/codestory-runtime/src/agent/packet_sufficiency.rs`
- `scripts/codestory-agent-ab-benchmark.mjs`
- `crates/codestory-runtime/tests/*`
- `scripts/tests/*`

**Interface**:

```rust
// Implements Req 7.1, 7.3, 7.4
struct PacketAvoidOpeningDto {
    file_path: String,
    reason: String,
}

struct PacketSufficiencyDto {
    covered_claims: Vec<PacketClaimDto>,
    display_claims: Vec<PacketClaimDto>, // optional if needed
    avoid_opening: Vec<PacketAvoidOpeningDto>,
}
```

```js
// Implements Req 7.2
const avoidOpeningPaths = packet.sufficiency.avoid_opening.map((entry) => entry.file_path);
```

**Design Notes**:

- Sort deduped paths before truncating.
- Keep fallback summaries outside proof-bearing `covered_claims`.
- Maintain backward-compatible aliases only if external JSON consumers need them.

### Component: ReadinessContract

**Purpose**: Exercise and expose degraded index, sidecar, and cache-busy readiness states.

**Locations**:

- `crates/codestory-cli/src/readiness.rs`
- `crates/codestory-cli/src/runtime.rs`
- `crates/codestory-cli/tests/ready_command.rs`
- `crates/codestory-contracts/src/api/dto.rs`
- `docs/usage.md`

**Interface**:

```rust
// Implements Req 8.1, 8.2, 8.3, 8.4
enum ReadinessStatusDto {
    Ready,
    RepairIndex,
    CheckIndex,
    RepairRetrieval,
    CacheBusy,
}
```

**Design Notes**:

- Add tests for unchecked index, stale index, missing index, unavailable sidecar, and non-full sidecar.
- Decide whether `CacheBusy` is a real structured verdict. If yes, return it in `ready`/`doctor`; if no, remove it from the DTO.
- Validate command strings in tests so docs can safely quote them.

### Component: PerformanceBudgetContract

**Purpose**: Keep interactive paths bounded and isolate deep-quality work behind explicit modes.

**Locations**:

- `crates/codestory-runtime/src/agent/retrieval_primary.rs`
- `crates/codestory-runtime/src/agent/packet_batch.rs`
- `crates/codestory-retrieval/src/sidecar.rs`
- `crates/codestory-retrieval/src/zoekt_index.rs`
- `crates/codestory-indexer/src/lib.rs`
- `crates/codestory-bench/*`
- `docs/testing/language-expansion-ab-report.md`

**Interface**:

```text
Implements Req 9.1, 9.2, 9.3, 9.4

Packet modes:
  compact: interactive budget
  standard: normal quality budget
  deep: long-running repair/diagnostic budget

Packet runtime summary:
  packet_sla_missed_runs must be 0 for smoke pass, unless exceptions are listed.
```

**Design Notes**:

- Keep 18s+ sidecar batch budgets behind `standard` or `deep`, not default compact.
- Stream lexical fingerprint hashing or cache fingerprint components keyed by DB revision/generation.
- Build per-file lookup maps for manual parser passes before adding more language heuristics.
- Add stress fixtures for large single files with many declarations/calls.

### Component: LanguageSupportContract

**Purpose**: Align registry, workspace discovery, parser routing, docs, and tests.

**Locations**:

- `crates/codestory-contracts/src/language_support.rs`
- `crates/codestory-indexer/src/lib.rs`
- `crates/codestory-indexer/src/languages/*`
- `crates/codestory-workspace/src/lib.rs`
- `docs/architecture/language-support.md`
- `crates/codestory-indexer/tests/*`

**Interface**:

```rust
// Implements Req 10.1, 10.2, 10.3, 10.4
trait LanguageParserProvider {
    fn profile(&self) -> LanguageSupportProfile;
    fn config(&self) -> LanguageConfig;
}
```

**Design Notes**:

- Keep the registry as the public support-claim source.
- Move language-specific tree-sitter configuration and ruleset selection out of the giant indexer `lib.rs`.
- Add alignment tests that fail if registry extensions are not routable by parser/workspace layers.
- Keep OSS corpus docs honest: raw-file-list indexer evidence is not persisted CLI/runtime proof.

### Component: ProductSemanticsContract

**Purpose**: Keep production packet claims general, source-derived, and separate from benchmark fixtures.

**Locations**:

- `crates/codestory-runtime/src/agent/packet_claim_profiles.rs`
- `crates/codestory-runtime/src/agent/eval_probes.rs`
- `crates/codestory-runtime/tests/retrieval_generalization_guard.rs`
- `scripts/lint-retrieval-generalization.mjs`
- `docs/testing/language-expansion-ab-report.md`

**Interface**:

```text
Implements Req 11.1, 11.2, 11.3, 11.4

Production claim profile:
  source pattern -> evidence role -> cautious claim candidate

Diagnostic claim profile:
  manifest/eval-only probe -> row-specific expected claim
```

**Design Notes**:

- Remove or generalize library-name-specific production claims that only serve benchmark rows.
- Keep exact row probes in manifests or eval-only code.
- Fix docs that imply `CODESTORY_EVAL_PROBES` changes an integration test path when the test only runs lint/fixture checks.

### Component: DocumentationContract

**Purpose**: Keep runbooks, branch action plans, and test docs consistent with actual commands.

**Locations**:

- `docs/review-action-plan.md`
- `docs/ops/retrieval-sidecars.md`
- `docs/contributors/retrieval-sidecar-smoke-ci.md`
- `.github/workflows/retrieval-sidecar-smoke.yml`
- `docs/testing/*`
- `.agents/skills/codestory-grounding/references/*`

**Interface**:

```text
Implements Req 12.1, 12.2, 12.3, 12.4

Final verification bundle:
  passed commands
  failed commands
  skipped live gates
  artifact paths
  e2e stats row hash
```

**Design Notes**:

- Fix clippy warnings directly.
- Align the ops runbook with the workflow or add the missing workflow step.
- Update nearest docs whenever command surface, benchmark meaning, or release proof changes.
