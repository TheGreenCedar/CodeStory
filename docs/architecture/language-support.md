# Language Support Contract

CodeStory uses the word "support" only with a qualifier. Parser routing,
regression evidence, framework route coverage, and agent packet/search quality
are separate claims.

The source of truth for extension and stored-language claim tiers is
`language_support_profile_for_ext` and
`language_support_profile_for_language_name` in
`crates/codestory-indexer/src/lib.rs`. The live parser-backed graph map is still
`get_language_for_ext`; structural and parser-compatibility-only languages do
not route through that function. The `files` command exposes these tiers in
`summary.language_counts` so operators can see the claim level attached to the
current indexed inventory.

## Claim Terms

- `parser-backed graph`: the file extension routes to a tree-sitter parser and
  rule asset, and the indexer can emit graph nodes and edges for that language.
- `fidelity-gated`: parser-backed graph support has overlapping regression
  evidence, including the fidelity lab and targeted resolution suites.
- `beta fidelity`: parser-backed graph support has tictactoe coverage plus a
  basic fidelity-lab fixture for symbols, imports, and call edges, but does not
  yet have the same owner-qualified or polymorphic resolution gates as Tier A.
- `structural collector`: the language is indexed by dedicated structural
  collectors, not full tree-sitter graph rules.
- `parser compatibility only`: a parser crate/version was checked for future
  use, but the language is not wired into runtime indexing.

## Current Matrix

| Tier | Languages | Runtime path | Evidence floor | Safe claim |
| --- | --- | --- | --- | --- |
| A | Python, Java, Rust, JavaScript, TypeScript/TSX, C++, C | parser-backed graph | fidelity lab, tictactoe, and targeted rule/resolution suites | daily graph navigation on typical code, with language caveats |
| B | Go, Ruby, PHP, C# | parser-backed graph | tictactoe plus basic fidelity lab | beta graph indexing for straightforward symbols/imports/calls |
| C | HTML, CSS, SQL | structural collector | structural collector tests | structural entity extraction, not semantic code navigation |
| D | Kotlin, Swift, Dart, Bash | parser compatibility only | parser crate/version compatibility notes | future candidate only; no runtime support claim |

Tier A is not uniform. Rust, TypeScript/TSX, JavaScript, Java, and C++ have the
strongest owner-qualified call-resolution evidence. Python and C are useful for
symbols, imports, call skeletons, and local trails, but their resolution claims
are intentionally narrower.

Tier B languages are wired and now covered by a basic fidelity lab, but they are
not promoted until they gain targeted call/import-resolution suites comparable
to Tier A.

## Route Coverage Is Separate

Framework route extraction has its own confidence labels in
[framework-route-coverage.md](../testing/framework-route-coverage.md). A
language can have parser-backed graph support while a framework remains
partial or heuristic. A route claim needs fixture or real-repo route evidence,
not just a language parser.

## Promotion Checklist

Before promoting a language or framework claim:

1. Add or update the parser/rule path and extension mapping.
2. Add tictactoe coverage for symbol, import, call, member, and inheritance
   shapes that the language can reasonably represent.
3. Add or update fidelity-lab fixtures for symbols, imports, call edges, and
   any resolution behavior being claimed.
4. Add targeted resolution tests before claiming polymorphic, cross-package,
   framework-handler, or owner-qualified call trails.
5. Update `language_support_profile_for_ext`,
   `language_support_profile_for_language_name`, and this page in the same
   change.
6. Run the full test binaries, not filtered test names:

   ```sh
   cargo test -p codestory-indexer --test fidelity_regression
   cargo test -p codestory-indexer --test tictactoe_language_coverage
   cargo test -p codestory-indexer --test call_resolution_common_methods
   cargo test -p codestory-indexer --test import_resolution
   cargo test -p codestory-indexer --test query_rule_regressions
   ```
