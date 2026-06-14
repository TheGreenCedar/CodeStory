# Requirements Document

## Introduction

This document converts the branch review findings into testable requirements. The component names match `blueprint.md` and must remain stable across design, tasks, and validation.

## Glossary

- **Default gate**: A command a contributor can run without live sidecars, private credentials, or benchmark cache state.
- **Live gate**: A command that intentionally requires local sidecars, real model assets, or prepared benchmark repositories.
- **Publishable evidence**: Benchmark or release evidence that can be used to justify merge, release, or product claims.
- **Diagnostic evidence**: Benchmark or probe output useful for debugging, but not valid as promotion evidence.
- **Oracle-assisted row**: A benchmark row where expected files, expected symbols, or expected claims are injected into the system under test.

## Requirements

### Requirement 1: Hermetic Default Retrieval Tests

#### Acceptance Criteria

1.1 WHEN `cargo test -p codestory-retrieval` runs with sidecars absent, down, or partially reachable, THE **TestGateHygiene** SHALL pass all default tests without attempting mandatory live Qdrant or Zoekt indexing.

1.2 WHEN a test requires live sidecars, THE **TestGateHygiene** SHALL mark it `#[ignore]` or guard it behind an explicit environment variable and document the exact live command.

1.3 WHEN the live sidecar query path is removed from the default suite, THE **TestGateHygiene** SHALL add or retain a hermetic mock/fixture test for successful query execution and sidecar-unavailable behavior.

1.4 WHEN a sidecar preflight succeeds shallowly but a later sidecar operation fails, THE **TestGateHygiene** SHALL return a controlled skip or failure message instead of panicking through `expect`.

### Requirement 2: Fresh Branch-Head Release Proof

#### Acceptance Criteria

2.1 WHEN remediation is complete, THE **ReleaseProofLedger** SHALL run `cargo build --release -p codestory-cli` at branch `HEAD`.

2.2 WHEN the release binary build passes, THE **ReleaseProofLedger** SHALL run `cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture` at the same `HEAD`.

2.3 WHEN the e2e stats test emits a row, THE **ReleaseProofLedger** SHALL append a `HEAD` row to `docs/testing/codestory-e2e-stats-log.md` in every relevant table.

2.4 WHEN docs change after a sidecar input hash was recorded, THE **ReleaseProofLedger** SHALL rerun `ready` or `doctor` as needed before treating full-sidecar proof as current.

### Requirement 3: Visible Sidecar Candidate-Resolution Failures

#### Acceptance Criteria

3.1 WHEN candidate resolution fails in sidecar primary search, THE **SidecarErrorBoundary** SHALL not convert the error to an empty result with `unwrap_or_default`.

3.2 WHEN `try_sidecar_primary_search` cannot resolve candidates, THE **SidecarErrorBoundary** SHALL return an unavailable outcome with a reason that includes candidate-resolution failure.

3.3 WHEN `search_results_sidecar_primary` cannot resolve candidates, THE **SidecarErrorBoundary** SHALL map the error through the existing sidecar unavailable error path.

3.4 WHEN these error boundaries change, THE **SidecarErrorBoundary** SHALL add regression tests for both runtime paths.

### Requirement 4: Benchmark Evidence Integrity

#### Acceptance Criteria

4.1 WHEN the benchmark harness runs publishable agent A/B rows, THE **BenchmarkEvidenceBoundary** SHALL not inject `expected_files`, `expected_symbols`, or `expected_symbol_probes` as packet `--extra-probe` values by default.

4.2 WHEN manifest-derived extra probes are used, THE **BenchmarkEvidenceBoundary** SHALL label the row as diagnostic or oracle-assisted and block it from publishable summaries unless an explicit diagnostic flag is selected.

4.3 WHEN `--publishable` is used for agent A/B rows, THE **BenchmarkEvidenceBoundary** SHALL require an explicit post-packet source-read policy and report whether the row is CodeStory-first or packet-only.

4.4 WHEN packet-gate mode selects zero nested A/B tasks, THE **BenchmarkEvidenceBoundary** SHALL exit non-zero unless the caller passes an explicit exploratory allow-empty flag.

4.5 WHEN `--reuse-baseline-from` copies artifacts, THE **BenchmarkEvidenceBoundary** SHALL canonicalize source paths, reject absolute or escaping paths, cap copied file size, and allow only known artifact names.

### Requirement 5: Local File and Artifact Boundaries

#### Acceptance Criteria

5.1 WHEN `drill` resolves endpoint files, search-hit files, or relative import candidates, THE **LocalFileBoundary** SHALL canonicalize them and reject paths outside the canonical project root.

5.2 WHEN a malicious repo contains absolute imports or `..` traversal imports, THE **LocalFileBoundary** SHALL prove through tests that no file outside the project root is read.

5.3 WHEN benchmark artifact reuse consumes untrusted JSON rows, THE **LocalFileBoundary** SHALL prevent copying local files outside the reusable benchmark run directory.

5.4 WHEN local file boundary checks reject a path, THE **LocalFileBoundary** SHALL keep the CLI output useful without exposing file contents from rejected paths.

### Requirement 6: Model Artifact Integrity

#### Acceptance Criteria

6.1 WHEN `scripts/setup-retrieval-env.mjs --fetch-embed-model` downloads a GGUF model, THE **ModelArtifactIntegrity** SHALL download to a temporary file, verify a pinned SHA-256, and only then rename it into the model directory.

6.2 WHEN a fallback mirror is configured, THE **ModelArtifactIntegrity** SHALL require explicit opt-in or prove the mirror uses the same checksum as the primary artifact.

6.3 WHEN checksum verification fails, THE **ModelArtifactIntegrity** SHALL delete the temporary file and exit with a clear error.

6.4 WHEN setup docs mention managed model download, THE **ModelArtifactIntegrity** SHALL document checksum and mirror behavior.

### Requirement 7: Deterministic Packet Sufficiency Contract

#### Acceptance Criteria

7.1 WHEN packet sufficiency emits avoid-opening guidance, THE **PacketSufficiencyContract** SHALL expose deterministic raw paths separately from human-readable reasons.

7.2 WHEN benchmark composition scores avoid-opening support, THE **PacketSufficiencyContract** SHALL score only raw path fields, not prose strings.

7.3 WHEN no supported claims are derived, THE **PacketSufficiencyContract** SHALL not insert a fallback answer summary into `covered_claims` after sufficiency status has already been computed.

7.4 WHEN packet output is serialized to JSON, THE **PacketSufficiencyContract** SHALL have golden or schema tests that catch shape drift for `covered_claims`, `avoid_opening`, `open_next`, `gaps`, and `follow_up_commands`.

### Requirement 8: Complete Readiness Contract

#### Acceptance Criteria

8.1 WHEN an index is unchecked, stale, or missing, THE **ReadinessContract** SHALL test the emitted status, reason, `minimum_next`, and `full_repair` commands.

8.2 WHEN agent packet/search readiness sees non-full sidecar retrieval, THE **ReadinessContract** SHALL test `repair_retrieval` output and the required retrieval repair commands.

8.3 WHEN cache access is busy, THE **ReadinessContract** SHALL either emit a structured `cache_busy` readiness verdict or remove `cache_busy` from the public readiness DTO.

8.4 WHEN readiness docs or examples describe repair commands, THE **ReadinessContract** SHALL keep them aligned with the tested command output.

### Requirement 9: Performance Budget and Scalability

#### Acceptance Criteria

9.1 WHEN users run compact/default packet search, THE **PerformanceBudgetContract** SHALL keep latency within an explicit interactive budget or require an explicit deep-quality mode for longer budgets.

9.2 WHEN packet runtime rows exceed the SLA, THE **PerformanceBudgetContract** SHALL fail a packet smoke gate or record an explicit exception with the reason.

9.3 WHEN strict sidecar status computes input fingerprints, THE **PerformanceBudgetContract** SHALL avoid materializing the full source corpus and all symbol docs into memory when a streaming or cached fingerprint can be used.

9.4 WHEN manual parser passes scan per-file nodes and edges, THE **PerformanceBudgetContract** SHALL add lookup maps or stress tests that bound large single-file behavior.

### Requirement 10: Unified Language Support Contract

#### Acceptance Criteria

10.1 WHEN a parser-backed language is added or changed, THE **LanguageSupportContract** SHALL verify alignment across the shared registry, parser routing, workspace source-group acceptance, docs, and tests.

10.2 WHEN parser routing grows, THE **LanguageSupportContract** SHALL move language-specific parser/ruleset construction toward per-language modules instead of expanding `crates/codestory-indexer/src/lib.rs`.

10.3 WHEN docs claim language support, THE **LanguageSupportContract** SHALL distinguish parser-backed graph coverage, structural collection, semantic resolution, route/framework coverage, and packet-quality evidence.

10.4 WHEN the OSS language corpus runs, THE **LanguageSupportContract** SHALL label it as indexer/raw-file-list evidence unless a persisted CLI/runtime smoke is added.

### Requirement 11: Product Semantics and Eval Probe Boundaries

#### Acceptance Criteria

11.1 WHEN production packet claim profiles emit framework or domain claims, THE **ProductSemanticsContract** SHALL keep them source-pattern-derived and general enough for real projects, not exact benchmark answer templates.

11.2 WHEN exact row-specific probes or expected claims are useful, THE **ProductSemanticsContract** SHALL keep them in benchmark manifests, eval-only tests, or explicit diagnostic extra probes.

11.3 WHEN docs describe `CODESTORY_EVAL_PROBES`, THE **ProductSemanticsContract** SHALL point to a test or harness that actually exercises eval-probe behavior.

11.4 WHEN generalization lint runs, THE **ProductSemanticsContract** SHALL continue to fail production paths that contain holdout-specific literals or benchmark-family steering.

### Requirement 12: Documentation and Quality Gate Completion

#### Acceptance Criteria

12.1 WHEN clippy reports warnings under `-D warnings`, THE **DocumentationContract** SHALL require code fixes rather than broad lint allows unless there is a documented false positive.

12.2 WHEN retrieval smoke docs describe CI behavior, THE **DocumentationContract** SHALL align the runbook with `.github/workflows/retrieval-sidecar-smoke.yml` or update the workflow.

12.3 WHEN remediation changes behavior, THE **DocumentationContract** SHALL update the nearest durable doc or repo-local skill reference.

12.4 WHEN all remediation tasks are complete, THE **DocumentationContract** SHALL produce a final verification bundle including pass/fail commands, skipped live gates, and remaining intentional follow-ups.
