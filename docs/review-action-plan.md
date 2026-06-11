# External Review Action Plan

This plan turns the recent architecture and language-support review into
traceable repo work. It focuses on changes that can be made true in this branch:
support-claim clarity, regression coverage, and durable follow-up ownership.

## Requirements

| ID | Requirement | Acceptance criteria | Status |
| --- | --- | --- | --- |
| R1 | Support claims must distinguish parser-backed graph support, regression evidence, product readiness, and framework-route claims. | Public docs define the terms and `files` exposes support tier metadata for indexed language counts. | Done |
| R2 | Thinly tested parser-backed languages must not be described like Tier A languages. | Go, Ruby, PHP, and C# are labeled beta and have basic fidelity-lab coverage without owner-qualified resolution promotion. | Done |
| R3 | Parser-compatibility-only languages must not look runtime-supported. | Kotlin, Swift, Dart, and Bash are documented as future candidates and do not route through `get_language_for_ext`. | Done |
| R4 | Structural languages must not be conflated with semantic code navigation. | HTML, CSS, and SQL are documented as structural collectors. | Done |
| R5 | Sidecar packet/search readiness must stay separate from local navigation. | Packet sufficiency requires cited planned-probe evidence, and local graph smoke tests no longer pretend sidecar search is available. | Done |
| R6 | Monolithic runtime/CLI files should be reduced without drive-by refactors. | Large-module decomposition remains a separate refactor campaign with tests around each extraction. | Follow-up |

## Completed Work

- Added language support profile APIs in the indexer so extension-level and
  stored-language claim tiers are explicit in code.
- Exposed support tier metadata from the `files` command in JSON and Markdown.
- Expanded `fidelity_regression` with basic Go, Ruby, PHP, and C# fixtures for
  symbols, imports, and call edges.
- Added [language-support.md](architecture/language-support.md) as the public
  support taxonomy and promotion checklist.
- Linked language support from README and architecture docs.
- Added doc drift checks so the README and language support contract keep the
  support terminology visible.
- Tightened packet sufficiency so supported-claim prose cannot satisfy missing
  planned flow probes without a matching citation.
- Updated stale regression tests that were hiding current runtime contracts: the
  resolution support snapshot test now uses the exported snapshot version, and
  the runtime lifecycle smoke uses graph symbol listing instead of mandatory
  sidecar search.

## Follow-Up Backlog

1. Decompose `crates/codestory-runtime/src/lib.rs` by extracting one orchestration
   subsystem at a time behind existing integration tests.
2. Decompose `crates/codestory-cli/src/main.rs` only after each command path has
   enough focused CLI tests to prove no behavior drift.
3. Add Tier B targeted resolution suites before changing their claim label from
   beta fidelity to fidelity-gated.
4. Add representative real-repo probes for Go, Ruby, PHP, and C# before making
   route or packet-quality claims for those ecosystems.

## Validation

Validation run for this branch:

```sh
cargo test -p codestory-indexer test_language_support_profiles_separate_claim_tiers
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-indexer
cargo test -p codestory-runtime packet_sufficiency -- --nocapture
cargo test -p codestory-runtime --test integration test_cli_app_indexer_smoke -- --nocapture
cargo test -p codestory-runtime
cargo test -p codestory-cli
cargo check -p codestory-indexer -p codestory-runtime -p codestory-cli
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats codestory_repo_release_e2e_emits_stats -- --ignored --nocapture
cargo fmt --check
git diff --check
```

The broad ignored-test command also invokes
`real_repo_agent_grounding_drill_emits_verification_packets`; that separate
drill was not run because `CODESTORY_REAL_REPO_DRILL_CASES` was not set.
