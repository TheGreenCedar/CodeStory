# External Review Action Plan

> Current remediation note (2026-06-13): later review work on this branch closed
> the remaining AST-first retrieval cleanup items that this older action plan
> did not cover: production benchmark-family steering, semantic language-label
> drift, sidecar packet diagnostic gaps, `files` count ambiguity, and receiver
> resolution boundary documentation. The generated remediation spec artifacts
> were removed after implementation; the durable context now lives in this
> action plan, the changed code/docs, the e2e stats log, and the pull request
> summary.

This plan turns the recent architecture and language-support review into
traceable repo work. It focuses on changes that can be made true in this branch:
support-claim clarity, regression coverage, and durable follow-up ownership.

## Requirements

| ID | Requirement | Acceptance criteria | Status |
| --- | --- | --- | --- |
| R1 | Support claims must distinguish parser-backed graph support, regression evidence, product readiness, and framework-route claims. | Public docs define the terms and `files` exposes support claim metadata for indexed language counts. | Done |
| R2 | Parser-backed languages must not be split into public quality tiers or beta buckets. | Go, Ruby, PHP, and C# use the same fidelity-gated claim label as the existing parser-backed languages, with member ownership and resolved-owner fixtures enforcing the floor. | Done |
| R3 | Candidate languages must not look runtime-supported until they are wired and verified. | Kotlin, Swift, Dart, and Bash now route through `get_language_for_ext` only after dependency wiring, rule assets, fixtures, receiver/call tests, and docs were added. | Done |
| R4 | Structural languages must not be conflated with semantic code navigation. | HTML, CSS, and SQL are documented as structural collectors. | Done |
| R5 | Sidecar packet/search readiness must stay separate from local navigation. | Packet sufficiency requires cited planned-probe evidence, and local graph smoke tests no longer pretend sidecar search is available. | Done |
| R6 | Monolithic runtime/CLI files should be reduced without drive-by refactors. | Large-module decomposition remains a separate refactor campaign with tests around each extraction. | Follow-up |

## Completed Work

- Added language support profile APIs in the indexer so extension-level and
  stored-language runtime/evidence labels are explicit in code.
- Exposed support claim metadata from the `files` command in JSON and Markdown.
- Expanded `fidelity_regression` with Go, Ruby, PHP, and C# fixtures for
  symbols, imports, call edges, member ownership, and resolved owner calls.
- Added span-aware member ownership extraction for Go, Ruby, PHP, and C# so
  duplicate method names bind to their actual declaring type rather than the
  first name match.
- Added Go interface method extraction so interface-owned methods participate in
  the same graph and resolution evidence as receiver methods.
- Added receiver-owner resolution fixtures for Go, Ruby, PHP, and C# with decoy
  methods that previously exposed name-only false positives.
- Added local receiver-call resolution for simple typed parameters in Go, PHP,
  and C#, plus Ruby constructor-assigned locals, and remapped resolved edge IDs
  through node deduplication so the edges survive persistence.
- Added Ruby bare-call coverage for method calls without parentheses, including
  a negative regression so local variable reads are not presented as calls.
- Added parser-backed graph support for Kotlin, Swift, Dart, and Bash with
  ABI-compatible parser crate pins, rule assets, extension routing, raw graph
  contracts, tictactoe fixtures, and targeted call-resolution coverage.
- Added typed receiver-call resolution for Kotlin, Swift, and Dart and a
  Dart-specific call attribution path for its signature/body sibling grammar.
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
3. Add cross-package, polymorphic, inheritance-heavy, and framework-handler
   resolution suites before claiming those deeper trails are complete.
4. Add representative real-repo probes for Go, Ruby, PHP, C#, Kotlin, Swift,
   Dart, and Bash before making route or packet-quality claims for those
   ecosystems.

## Parser Implementation Audit

This audit records the implementation surface used to promote Kotlin, Swift,
Dart, and Bash from candidate parser records to parser-backed graph languages.
The crate pins below are the ABI-compatible versions verified against the
workspace's `tree-sitter = "0.24"` policy.

| Language | Crate | Runtime extensions | Implemented graph floor |
| --- | --- | --- | --- |
| Kotlin | `tree-sitter-kotlin-ng = "1.1.0"` | `.kt`, `.kts` | classes, interfaces, objects, functions, package/import modules, member edges, inheritance/conformance, direct calls, member calls, typed receiver calls |
| Swift | `tree-sitter-swift = "0.7.0"` | `.swift` | classes, protocols, functions, protocol functions, imports, member edges, inheritance/conformance, direct calls, member calls, typed receiver calls |
| Dart | `tree-sitter-dart-orchard = "0.3.2"` | `.dart` | classes, abstract interfaces, mixins, enums, extensions, top-level functions, methods, imports, member edges, inheritance/interfaces, direct calls, typed receiver calls |
| Bash | `tree-sitter-bash = "0.23.3"` | `.sh`, `.bash` | shell functions, variable assignments, command calls, and static `source`/`.` import edges |

## Validation

Validation run for this branch:

```sh
cargo test -p codestory-indexer test_language_support_profiles_separate_runtime_claims
cargo test -p codestory-indexer test_raw_graph_contracts_cover_supported_languages -- --nocapture
cargo test -p codestory-indexer test_live_rule_parsers_expose_key_node_kinds -- --nocapture
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-indexer --test trait_interface_resolution -- --nocapture
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
