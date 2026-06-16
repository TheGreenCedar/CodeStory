# Language Support Contract

CodeStory uses "support" only with a qualifier. Parser routing, graph fidelity,
semantic resolution, framework routes, and agent packet quality are different
claims.

The source of truth for public language labels is
`crates/codestory-contracts/src/language_support.rs`. The indexer maps those
profiles to parser and rule construction in `get_language_for_ext`.

## Claim Terms

| Term | Means | Does not mean |
| --- | --- | --- |
| Parser-backed graph | Extension routes to a parser and graph rules. | Full semantic navigation. |
| Fidelity-gated | Core symbol/import/call/member shapes pass fixture suites. | Every language feature is covered. |
| Semantic-resolution-backed | Targeted resolver tests prove the named behavior. | Broad cross-package or polymorphic dispatch. |
| Structural collector | Dedicated extractor emits structural entities. | Parser-backed code navigation. |
| Parser compatibility record | A parser crate/version was checked for future use. | Runtime support. |

## Current Runtime Claims

| Runtime claim | Languages | Evidence floor | Safe claim |
| --- | --- | --- | --- |
| Parser-backed graph, fidelity-gated | Python, Java, Rust, JavaScript, TypeScript/TSX, C++, C, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, Bash | fidelity lab, tictactoe coverage, raw graph contracts, targeted rule/resolution suites, opt-in OSS corpus | daily graph navigation on typical code, with caveats |
| Structural collector | HTML, CSS, SQL | structural collector tests | structural entity extraction |

Agent-facing packet/search quality is separate. The language-expansion A/B
report is not blanket promotion proof for every parser-backed language.

## Latest Agent-Facing Evidence

The latest full language-expansion paired A/B run was completed on 2026-06-16:
`target/agent-benchmark/language-expansion-holdout-20260616-0.8.0-retry/reanalyzed-summary.md`.

That run is useful operational evidence, not a broad answer-quality promotion:

| Measure | With CodeStory | Without CodeStory | Read |
| --- | ---: | ---: | --- |
| Runs attempted | `54` | `54` | Three repeats across 18 language tasks. |
| Run success | `54/54` | `51/54` | All baseline failures were the Ruby/Jekyll row. |
| Quality pass | `16/54` | `19/51` successful rows | Answer quality remains uneven. |
| All-in wall time | `6,411,835 ms` | `7,523,716 ms` | CodeStory ratio `0.852`. |
| Total tokens | `7,859,161` | `9,087,330` | CodeStory ratio `0.865`. |
| Commands | `54` | `471` | CodeStory kept exploration bounded. |
| Source reads | `0` | `417` | The CodeStory arm stayed packet-first. |

Safe wording: CodeStory completed the full 18-language A/B arm with lower
overall wall time, token use, command count, and direct source reads. Do not say
the run proves first-class agent-facing quality across every language; the
quality score is mixed and several rows still need better handoff semantics.

## Resolution Claims

Receiver and import resolution are fixture-backed. If a behavior is not covered
by `crates/codestory-indexer/tests/call_resolution_common_methods.rs` or another
targeted regression suite, do not claim it.

Use the tests for specifics. This page should state the contract, not repeat the
fixture catalogue.

Current boundaries:

- Typed receiver behavior is proven only for the languages and shapes covered by
  targeted tests.
- Framework handlers, broad scoped-import shadowing, inheritance-heavy target
  selection, polymorphic dispatch, declarative parameter extraction, and untyped
  factory-returned receivers need separate fixtures before they become claims.
- Header files keep the shared registry default of `.h` as C for path-only
  semantic detection. Any C++ header upgrade from compile/source signals is a
  parser-routing detail until semantic requests carry that resolved identity.

## Parser Compatibility Records

This table records parser-version compatibility only. A parser becomes runtime
support only after dependency wiring, rule assets, extension routing, and
fidelity coverage land.

Workspace parser policy:

- `tree-sitter = "0.24"`
- `tree-sitter-graph = "0.12"`

Validation: each listed candidate passed an isolated `cargo check` probe with
the policy pins; wired parser rows also passed a parse smoke. HTML, CSS, and SQL
remain structural runtime paths, not parser-backed runtime claims.

| Language | Candidate crate | Version checked | Decision |
| --- | --- | ---: | --- |
| Go | `tree-sitter-go` | `0.23.4` | wired |
| Ruby | `tree-sitter-ruby` | `0.23.1` | wired |
| PHP | `tree-sitter-php` | `0.23.11` | wired |
| C# | `tree-sitter-c-sharp` | `=0.23.0` | wired |
| Kotlin | `tree-sitter-kotlin-ng` | `1.1.0` | wired |
| Swift | `tree-sitter-swift` | `0.7.0` | wired |
| Dart | `tree-sitter-dart-orchard` | `0.3.2` | wired |
| Bash | `tree-sitter-bash` | `0.23.3` | wired |
| HTML | `tree-sitter-html` | `0.23.2` | candidate only |
| CSS | `tree-sitter-css` | `0.25.0` | candidate only |
| SQL | `tree-sitter-sequel` | `0.3.11` | candidate only |

Older or newer parser candidates that use an incompatible tree-sitter ABI are
not support claims. Re-check the candidate before upgrading.

## Route Coverage Is Separate

Framework route extraction has its own confidence labels in
[framework-route-coverage.md](../testing/framework-route-coverage.md). A
language can have parser-backed graph support while a framework remains partial
or heuristic. A route claim needs fixture or real-repo route evidence, not just a
language parser.

## Expansion Checklist

Before adding a parser-backed language or widening a public claim:

1. Update registry, parser construction, extension mapping, rules, and docs in
   one change.
2. Add tictactoe and fidelity-lab coverage for the represented language shapes.
3. Add targeted resolution tests for any receiver, import, framework, or
   polymorphic behavior being claimed.
4. Add or update the OSS corpus and A/B task manifest before making
   agent-facing savings or answer-quality claims.
5. Run the full binaries, not filtered test names:

   ```sh
   cargo test -p codestory-indexer --test fidelity_regression
   cargo test -p codestory-indexer --test tictactoe_language_coverage
   cargo test -p codestory-indexer --test call_resolution_common_methods
   cargo test -p codestory-indexer --test import_resolution
   cargo test -p codestory-indexer --test query_rule_regressions
   cargo test -p codestory-indexer --test trait_interface_resolution
   ```

6. For broader real-project smoke evidence, run either the OSS corpus dry-run
   manifest check or the relevant language subset:

   ```sh
   CODESTORY_OSS_CORPUS_DRY_RUN=1 cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
   CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 CODESTORY_OSS_CORPUS_LANGUAGES=python cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
   ```

7. For agent-facing evidence, run at least the targeted language task from the
   A/B suite, and run the full suite before making language-wide claims:

   ```sh
   node scripts/codestory-agent-ab-benchmark.mjs \
     --task-suite language-expansion-holdout \
     --arms without_codestory,with_codestory \
     --repeats 3 --materialize-repos --prepare-codestory-cache \
     --out-dir target/agent-benchmark/language-expansion-holdout \
     --timeout-ms 600000
   ```
