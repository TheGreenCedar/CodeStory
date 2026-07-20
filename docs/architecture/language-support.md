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
| Structural source-proof | Dedicated extractor emits exact source anchors and publishes `structural_text` / `source_range_only` result metadata. | Parser-backed graph extraction, semantic code navigation, or packet semantic proof. |
| Parser compatibility record | A parser crate/version was checked for future use. | Runtime support. |
| Packet proof gate | A packet-runtime artifact proves the current packet citation and sufficiency contract for the measured tasks. | Public product-grade language quality. |
| Publishable packet-runtime pass | Success, quality, sufficiency, and cold-SLA gates all pass in one coherent run. | A change to parser-backed or structural language coverage. |
| Development comparison | A reused-baseline or local-real artifact informs tuning and diagnosis. | Fresh publishable promotion proof. |

A parser-backed file can be publishable even when the selected grammar reports
a partial tree. Runtime treats it as `parser_partial` only when the indexed bytes
have a verified content hash, the file carries no file-level error, and storage
does not require a retry. That row remains `complete=false` so diagnostics do
not overstate parser coverage, but it does not block an otherwise atomic core
publication. Malformed, binary/non-UTF-8, unreadable, oversized, changed,
incompletely discovered, or collector-failed source remains a publication
blocker.

## Current Runtime Claims

| Runtime claim | Languages | Evidence floor | Safe claim |
| --- | --- | --- | --- |
| Parser-backed graph, fidelity-gated | Python, Java, Rust, JavaScript, TypeScript/TSX, C++, C, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, Bash | fidelity lab, tictactoe coverage, raw graph contracts, targeted rule/resolution suites, opt-in OSS corpus | daily graph navigation on typical code, with caveats |
| Structural source-proof | HTML, CSS, SQL, Markdown/MDX, generic YAML/TOML/JSON, non-parser shell, PowerShell, path-scoped GitHub Actions workflows, path-scoped Docker Compose manifests, basename-scoped Cargo manifests, dedicated OpenAPI/Swagger endpoint schema anchors | structural collector and OpenAPI schema-anchor tests | structural-text/schema anchors |

Agent-facing packet/search quality is separate. Run-specific A/B artifacts are
not blanket promotion proof for every parser-backed language.

## Claim Ladder

The source-derived ladder in `language_support.rs` maps current profiles only to
the tiers they prove. Parser-backed profiles currently claim filename routing,
grammar parse, and source graph extraction. Structural collectors claim filename
routing and structural source-proof only.

| Tier | Allowed proof role | Provenance expectation | Does not mean |
| --- | --- | --- | --- |
| `filename_route` | `extension_routing` | `LANGUAGE_SUPPORT_PROFILES` extension registry | Parser availability. |
| `grammar_parse` | `parser_smoke` | Live tree-sitter parser config and parse smoke | Graph fidelity. |
| `source_graph_extraction` | `graph_fixture` | Fidelity or tictactoe graph fixture | Typed semantic resolution. |
| `structural_source_proof` | `structural_collector_fixture` | Structural collector fixture with exact source spans | Parser-backed graph extraction or semantic proof. |
| `typed_semantic_edges` | `semantic_resolver_fixture` | Targeted resolver regression | Broad semantic parity. |
| `packet_sufficient_answer_quality` | `packet_runtime_artifact` | Publishable packet-runtime artifact | Runtime language support. |

No current language profile claims `typed_semantic_edges` or
`packet_sufficient_answer_quality` from the profile registry alone.

## Agent-Facing Evidence

GitHub Actions workflow support is path-scoped to `.github/workflows/*.{yml,yaml}`.
The pilot emits workflow, job, and step anchors with `structural_text` /
`source_range_only` evidence and collector provenance. Docker Compose support
is path-scoped to
`compose*.{yml,yaml}`, `docker-compose*.{yml,yaml}`, and
`docker/*-compose.{yml,yaml}` style manifests; it emits stack, service, image or
build, ports, environment key, and volume anchors with the same structural-text
boundary. HTML, CSS, SQL, and Cargo manifest collector anchors use that result
tier too. OpenAPI/Swagger endpoint schemas stay on the dedicated OpenAPI
indexing path and emit `openapi:endpoint:*` anchors as `exact_source` /
`source_range_only` diagnostic evidence only; they do not make generic YAML an
OpenAPI surface. Unsupported shapes stay explicit: YAML anchors and merge keys are not
interpreted, matrix expansion and expressions are not resolved, reusable
workflows and shell bodies are not semantically traced, Compose interpolation,
profiles, health checks, dependency order, and runtime container behavior are
not interpreted, and schema endpoint anchors do not prove handler
implementation, auth behavior, request validation, response semantics, runtime
route behavior, or generated-client correctness. Neither collector validates
execution semantics.
Cargo manifest support is basename-scoped to `Cargo.toml`. It emits `[workspace]`
member, `[package]` name, and direct dependency-key anchors from `[dependencies]`,
`[dev-dependencies]`, and `[build-dependencies]` with the same `structural_text` /
`source_range_only` boundary. Its dedicated producer does not handle generic
TOML, `Cargo.lock`, target-scoped dependency tables, `[workspace.dependencies]`,
dependency subtables, feature tables, patch or replace tables, dependency
resolution, feature activation, workspace inheritance, build-script behavior,
or lockfile proof. Packet evidence treats these anchors as diagnostic unless a
future structural role explicitly admits them; they must not satisfy semantic
proof roles or semantic dependency proof.

Generic Markdown/MDX emits heading, link/reference-definition, and fenced-block
labels. Generic YAML emits conservative mapping keys; generic TOML emits table
and key labels; generic JSON emits object keys in source order. The shell
fallback emits function and import anchors only for `.zsh`, `.ksh`, and
`.command`; `.sh` and `.bash` remain parser-backed Bash. PowerShell `.ps1` and
`.psm1` emit function and module/dot-source anchors. These collectors do not
interpret references, substitutions, imports, execution behavior, or typed
targets.

Dedicated routing wins before generic collection: workflow and Compose paths
keep their YAML producers, `Cargo.toml` keeps its manifest producer, and
OpenAPI/Swagger JSON or YAML keeps its `exact_source` endpoint path. Structural
admission rejects generated/vendor, secret-bearing, lockfile, minified, and
declared high-noise paths before reading. A structural file is accepted only
as a complete UTF-8 projection within the 1 MiB and 2,048-unit bounds;
malformed, binary, unreadable, source-drifted, cancelled, or failed collection
does not publish reusable units.

Safe wording: structural-text anchors prove only that their collector found the
cited source span; their `source_range_only` status and non-sufficient result
flag must not be upgraded into graph or semantic proof. OpenAPI endpoint anchors
prove only that a schema declares the method/path at the cited source range.
Packet-runtime is implemented and
can complete measured suites, but publishable agent-facing packet quality is not
promoted until one coherent run has all quality, sufficiency, and cold-SLA gates
green. Run-specific scorecards belong in PRs, issues, release notes, or ignored
`target/` artifacts; this page records the durable claim boundaries. HTML, CSS,
SQL, Markdown/MDX, generic YAML/TOML/JSON, non-parser shell, PowerShell, GitHub
Actions workflows, Docker Compose manifests, and Cargo manifests remain
structural source-proof collectors; OpenAPI schemas remain a dedicated
schema-anchor path.

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

- `tree-sitter = "0.26.11"`
- `tree-sitter-rust = "0.24.2"`
- `tree-sitter-graph = "0.12.0"`, vendored from upstream commit
  `b930fb59c2177a90b3a6a68e1feeca6918ceb58b` with only the Tree-sitter 0.26
  compatibility and current lint adjustments recorded in
  `vendor/tree-sitter-graph/UPSTREAM.md`

Validation: each listed candidate passed an isolated `cargo check` probe with
the policy pins; wired parser rows also passed a parse smoke. HTML, CSS, SQL,
GitHub Actions workflows, Docker Compose manifests, Markdown/MDX, generic
YAML/TOML/JSON, non-parser shell, and PowerShell remain structural runtime
paths, not parser-backed runtime claims.

| Language | Candidate crate | Version checked | Decision |
| --- | --- | ---: | --- |
| Go | `tree-sitter-go` | `0.25.0` | wired |
| Ruby | `tree-sitter-ruby` | `0.23.1` | wired |
| PHP | `tree-sitter-php` | `0.24.2` | wired |
| C# | `tree-sitter-c-sharp` | `0.23.5` | wired |
| Kotlin | `tree-sitter-kotlin-ng` | `1.1.0` | wired |
| Swift | `tree-sitter-swift` | `0.7.3` | wired |
| Dart | `tree-sitter-dart-orchard` | `0.4.0` | wired |
| Bash | `tree-sitter-bash` | `0.25.1` | wired |
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
language parser. FastAPI decorator routes are parser-backed only when a
tree-sitter query captures a static string path and decorated handler on a
single-target module-scope receiver whose latest preceding binding constructs
an imported `FastAPI` or `APIRouter`. Later assignments, imports, functions,
and classes invalidate shadowed ownership. Chained or multi-target constructor
assignments are conservatively not promoted; unmatched, injected,
factory-returned, and nested-scope receivers are not labeled as FastAPI.
Error-local line-scan recovery remains structural evidence. Dynamic paths and
ordinary escaped literals do not become exact route claims.

Express registration calls are parser-backed only when a JavaScript,
TypeScript, or TSX tree-sitter query captures a static path on a module-scope
receiver whose latest source-ordered binding constructs an app or router from
an explicit `express` import or `require("express")`. Reassignment and shadowing
invalidate ownership. Substitution-free template literals are static; dynamic
or escaped paths are not exact claims. Handler edges remain probable and are
limited to direct names that graph resolution can match. Mounted prefixes,
nested or injected receivers, factory returns, and runtime middleware behavior
remain outside this claim tier; malformed-file line recovery is structural.

Fastify direct verb calls, including `TRACE`, and
`route({ method, url, handler })` registrations use the same JavaScript,
TypeScript, and TSX parser-backed boundary when the
module-scope receiver was constructed from an explicit `fastify` ESM or
CommonJS binding. Source-ordered reassignment, shadowing, or unsupported
construction invalidates receiver ownership. Exact claims require one static
method and path; dynamic or escaped strings, method arrays, nested builders,
and nested, injected, or factory-returned receivers are excluded. Direct
identifier and member handlers can retain probable edges, while wrapped and
inline handlers do not gain name-based edges. Plugin prefixes, schema behavior,
and runtime middleware semantics remain heuristic; malformed-file recovery is
structural and error-local.

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
   cargo test -p codestory-indexer --locked --test fidelity_regression
   cargo test -p codestory-indexer --locked --test tictactoe_language_coverage
   cargo test -p codestory-indexer --locked --test call_resolution_common_methods
   cargo test -p codestory-indexer --locked --test import_resolution
   cargo test -p codestory-indexer --locked --test query_rule_regressions
   cargo test -p codestory-indexer --locked --test trait_interface_resolution
   ```

6. For broader real-project smoke evidence, run either the OSS corpus dry-run
   manifest check or the relevant language subset:

   ```sh
   CODESTORY_OSS_CORPUS_DRY_RUN=1 cargo test -p codestory-indexer --locked --test oss_language_corpus -- --ignored --nocapture
   CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 CODESTORY_OSS_CORPUS_LANGUAGES=python cargo test -p codestory-indexer --locked --test oss_language_corpus -- --ignored --nocapture
   ```

7. For agent-facing evidence, run at least the targeted language task from the
   A/B suite. Reuse the fixed no-CodeStory control only when
   `--reuse-baseline-from` accepts the baseline fingerprints; otherwise treat the
   reused comparison as diagnostic or create a new approved control artifact:

   ```sh
   node scripts/codestory-agent-ab-benchmark.mjs \
     --task-suite language-expansion-holdout \
     --arms without_codestory,with_codestory \
     --repeats 3 --materialize-repos --prepare-codestory-cache \
     --reuse-baseline-from target/agent-benchmark/<compatible-baseline-run> \
     --out-dir target/agent-benchmark/language-expansion-holdout \
     --timeout-ms 600000
   ```
