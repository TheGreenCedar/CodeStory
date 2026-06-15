# Language Support Contract

CodeStory uses the word "support" only with a qualifier. Parser routing,
regression evidence, framework route coverage, and agent packet/search quality
are separate claims.

The source of truth for extension ownership, stored-language names, support
modes, evidence tiers, and claim labels is
`crates/codestory-contracts/src/language_support.rs`. The indexer maps those
shared support profiles to parser/rule construction in `get_language_for_ext`.
The shared registry owns public support claims. Workspace discovery also carries
compatibility-only filters for file types that can be scanned or grouped without
being claimed as parser-backed language support.

## Claim Terms

- `parser-backed graph`: the file extension routes to a tree-sitter parser and
  rule asset, and the indexer can emit graph nodes and edges for that language.
- `fidelity-gated`: parser-backed graph support has overlapping regression
  evidence for symbols, imports, calls, member ownership, representable
  inheritance, and resolved-call behavior covered by the fixture suites.
- `semantic-resolution-backed`: the language has explicit semantic resolver
  dispatch and tests for the resolution behavior being claimed. This is a
  narrower claim than parser-backed graph support.
- `structural collector`: the language is indexed by dedicated structural
  collectors, not full tree-sitter graph rules.
- `candidate parser compatibility record`: a parser crate/version was checked
  for possible future use, but that record is not a runtime support claim until
  the language has dependency wiring, rule assets, routing, and fidelity tests.

## Current Matrix

| Runtime claim | Languages | Runtime path | Evidence floor | Safe claim |
| --- | --- | --- | --- | --- |
| Parser-backed graph, fidelity-gated | Python, Java, Rust, JavaScript, TypeScript/TSX, C++, C, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, Bash | tree-sitter parser plus graph rules | fidelity lab, tictactoe coverage, raw graph contracts, targeted rule/resolution suites, and the opt-in OSS language corpus; agent-facing A/B evidence is separate and currently mixed | daily graph navigation on typical code, with language-specific caveats |
| Structural collector | HTML, CSS, SQL | dedicated structural collectors | structural collector tests | structural entity extraction, not semantic code navigation |

Agent-facing packet/search quality is a separate claim from parser-backed graph
support. The current language-expansion A/B report records a mixed full
18-language result and a stronger packet-eligible slice; do not use that report
as blanket promotion proof for every parser-backed language.

The parser-backed graph claim is not a promise that every language has identical
dispatch or semantic-resolution semantics. Typed receiver-call support is
claimed only for the fixture-backed cases named in the indexer regression
suites. Current support covers simple local owner-qualified calls where tests
prove the behavior, plus Python `from module import Type` annotations and
constructor locals, and TypeScript named or aliased relative type imports when
the imported module file and owner method are both indexed. Python receiver
fixtures cover direct imports, `Type as Alias` imports, parenthesized multiline
import lists, same-file constructor locals such as
`workflow = Workflow(); workflow.run()`, and imported constructor locals through
direct and aliased `from module import Type` bindings. Python instance-property
receiver fixtures cover direct assignments such as
`self.workflow = Workflow(); self.workflow.run()`, plus imported direct and
aliased constructor owners assigned to `self.<field>`. Python constructor-local
and instance-property fixtures also cover future-binding guards, factory-return
shadows, mixed property owners, nested assignment guards, static/classmethod
assignment guards, local constructor-name shadows, missing or duplicate imported
owners, and function-local class and import shadows. These cases fail closed or
resolve locally instead of leaking to imported or parameter receiver owners. Python import fixtures fail
closed when a top-level class or assignment shadows the imported annotation name.
TypeScript receiver fixtures cover `import type { Type as Alias } from "./file"`,
inline `import { type Type }` specifiers, plain named relative imports,
namespace type imports such as `import type * as ns`, NodeNext style `.js`
relative specifiers resolving to indexed `.ts` sources, same-line calls to
same-named methods on different imported receiver types, missing imported files,
duplicate imported local names, and local type/interface shadows. TypeScript and
TSX receiver fixtures also cover same-file class `this.method()` calls and
explicit `this.<field>` calls through typed class properties such as
`private repository: Repository; this.repository.save()`, plus ECMAScript
private-field receivers such as
`#privateRepository: PrivateRepository; this.#privateRepository.persist()`.
TypeScript property receiver fixtures cover same-file owners, aliased named
imports, inline `type` imports, namespace imports, missing imported files,
duplicate imported local names, local type/interface shadows, `any`/unknown
property owners, missing namespace aliases, namespace alias collisions, and
unimported cross-file property owners. TypeScript and
TSX receiver fixtures also cover visible-before-call constructor bindings such
as `const workflow = new Workflow<T>(); workflow.run()`, and TypeScript/TSX
imported constructor bindings through named, aliased, and namespace imports.
Future constructor bindings, function-local class shadows, and block-local
factory shadows stay fail-closed or local instead of leaking to imported or
parameter receiver owners. Bare class-field calls without `this`, broader
scoped import shadowing, polymorphic dispatch, inheritance-heavy target
selection, framework-handler resolution, and declarative parameter extraction
require separate fixtures and cannot be used as product claims until those
fixtures pass. JavaScript receiver
fixtures cover same-file class `this.method()` calls and visible-before-call
constructor bindings such as `const workflow = new Workflow(); workflow.run()`,
plus imported default, named, and aliased constructor bindings such as `import
Workflow from "./workflow.js"; const workflow = new Workflow(); workflow.run()`.
JavaScript constructor receiver fixtures also cover future-binding guards and
function-local class shadows. JavaScript property receiver fixtures cover direct
instance assignments such as
`this.workflow = new Workflow(); this.workflow.run()`, ECMAScript private
instance assignments such as
`this.#workflow = new Workflow(); this.#workflow.run()`, private field
initializers such as `#workflow = new Workflow()`, plus imported default, named,
and aliased constructor owners assigned to `this.<field>`. Factory-returned
receivers, parameter receivers, static-only property setup, mixed property
owners, qualified constructors/namespace imports, missing or duplicate imported
constructor aliases, and duplicate ambiguous owners stay fail-closed unless a
same-file owner can be resolved. Rust receiver fixtures
cover trait-bound parameter calls and same-function unit-struct bindings such as
`let workflow = Workflow; workflow.run(...)`. Java receiver fixtures
cover enclosing-type `this.decorate(...)` calls, same-file typed method
parameters such as `EventListener listener`, including same-line calls to
same-named methods on different receiver types, plus explicit non-static imports
such as `import com.example.Notifier` when the imported package-qualified owner
method is indexed. Java ordinary local-variable receivers such as
`Workflow workflow = new Workflow(); workflow.run(...)`,
`Workflow workflow = makeWorkflow(); workflow.run(...)`, and
`var workflow = new Workflow(); workflow.run(...)` also resolve through the
local static type, direct constructor temporaries such as
`new Workflow().run(...)` resolve to the constructor owner, and class-qualified
calls such as `Entry.makeWorkflow()` remain resolvable. Typed same-file field
receivers such as `notifier.notifyEvent(...)` and
`this.repository.save(...)` resolve when the field has an explicit local or
imported type. Enhanced-for variables, try-with-resources resources, and catch
parameters resolve through their scoped static types. Wildcard imports, static
imports, missing packages, duplicate imported local names, erased `Object`
receivers, and unimported cross-file field owners stay fail-closed unless a
same-file owner can be resolved. Untyped `var` factory receivers stay
fail-closed. Go receiver
fixtures cover qualified imported interface parameters such as `mail.Notifier`
when the imported package directory and owner method are indexed, including a
same-name local interface guard. Go visible same-file composite literal bindings
such as
`workflow := Workflow{...}; workflow.Run(...)` also resolve to the declared owner
method after the binding declaration, and qualified imported composite bindings
such as `workflow := mail.Workflow{...}; workflow.Run(...)` resolve through the
same import guards. Go method receiver fields such as `w.notifier.Notify(...)`
resolve through same-file struct field types or through qualified imported field
types such as `notifier mail.Notifier` when the imported package directory and
owner method are indexed. Missing or duplicate import aliases, unqualified
cross-file composite bindings, and unknown factory-returned receivers stay
fail-closed unless a same-file owner can be resolved. Ruby receiver fixtures
cover visible same-file constructor chains such as
`Workflow.new(...).run(...)` and local constructor assignments such as
`workflow = Workflow.new; workflow.run(...)`. Same-file instance-variable
receivers such as `@workflow.run(...)` resolve only when all same-class ordinary
instance-method assignments for that instance variable are direct same-file
constructor owners of the same type. Factory-returned receivers, operator
assignments such as `||=`, class-body or singleton-method instance-variable
assignments, mixed instance-variable constructor owners, and multiple or missing
require-relative owner paths stay fail-closed unless a same-file owner can be
resolved. Exact single-file `require_relative` constructor owners can resolve
direct constructor chains, local constructor assignments, and instance-variable
receivers when the required `.rb` file and owner method are indexed. C++ receiver fixtures cover same-file typed parameters such as
`const Notifier& notifier`,
`Repository<std::string>& repository`, and `Notifier* pointer`, including
same-named methods on different local receiver owners, pointer member calls with
`->`, and explicit single local declarations such as
`Workflow workflow; workflow.run(...)`. C++ visible same-file receiver coverage
also includes direct-constructor `auto` locals such as
`auto workflow = Workflow{}; workflow.run(...)`, `const auto workflow =
Workflow(); workflow.run(...)`, and `auto workflow = new Workflow();
workflow->run(...)`. C++ visible same-file receiver coverage also includes
explicit `this->decorate(...)` calls and typed class-field
receivers such as `repository.save(...)`, `this->repository.save(...)`,
`pointer->save(...)`, and `this->pointer->save(...)`. Bare implicit self calls
such as `decorate(...)` are not part of the C++ receiver claim. `auto` factory
receivers, qualified-constructor `auto` locals, smart-pointer factory
receivers, multi-declarator local or field declarations, function-style
constructor declarations, and cross-file local or field owner lookup stay
fail-closed unless a same-file owner can be resolved. Dart receiver fixtures
cover prefixed relative imports such as
`import "./file.dart" as ns` with `ns.Type` parameter annotations, including
missing imported files, duplicate alias guards, and ambiguous unprefixed-import
guards. Dart visible same-file constructor bindings such as
`final workflow = Workflow(); workflow.run(...)` also resolve to the declared
owner method after the binding declaration, and prefixed imported constructor
bindings such as `final workflow = ns.Workflow(); workflow.run(...)` resolve
through the same import-alias guards. Dart visible same-file receiver coverage
also includes class `this.decorate(...)` calls and typed class-field receivers
such as `notifier.notifyEvent(...)` and `this.repository.save(...)`, with
imported field types resolved only through prefixed relative imports such as
`ns.Type`. Missing or duplicate imported constructor aliases, unprefixed
cross-file receiver annotations, unprefixed cross-file field or constructor
owners, erased `dynamic` receivers, and local factory or dynamic shadows stay
fail-closed unless a same-file owner can be resolved. Kotlin receiver fixtures
cover explicit imports such
as `import com.example.Notifier`, including `as` aliases, when the
package-qualified owner method is indexed. Kotlin visible same-file constructor
bindings such as `val workflow = Workflow(); workflow.run(...)` also resolve to
the declared owner method after the binding declaration, and exact imported
constructor bindings such as
`import other.Workflow; val workflow = Workflow(); workflow.run(...)` resolve
through the same import and alias guards. Kotlin visible
same-file receiver coverage also includes enclosing-type `this.decorate(...)`
calls and typed class-property receivers such as `notifier.notifyEvent(...)`,
`this.repository.save(...)`, and primary-constructor properties such as
`class Workflow(private val notifier: Notifier)`, with imported or aliased
property types resolved only through exact imports. Kotlin wildcard imports,
missing packages, duplicate imported local names, unimported cross-file
constructor or property owners, erased `Any` receivers, and local type or factory
shadows stay fail-closed unless a same-file owner can be resolved. Swift
receiver fixtures cover SwiftPM module imports such as `import MailKit` with
`Notifier` parameters when exactly one imported module is visible, scoped type
imports such as `import class MailKit.Notifier`, plus module-qualified receiver
annotations such as `MailKit.Notifier`, when the owner method is indexed under
`Sources/MailKit`. Module-qualified receiver annotations prefer the imported
module owner over a same-file type with the same terminal name. Scoped Swift
imports authorize only the named imported owner and do not imply whole-module
receiver lookup for unrelated types. Swift visible same-file constructor
bindings such as `let workflow = Workflow(); workflow.run(...)` also resolve to
the declared owner method after the binding declaration, and imported
constructor bindings resolve through the same SwiftPM module, module-qualified,
and scoped-type import rules as parameter annotations. Swift visible same-file
receiver coverage also includes class `self.decorate(...)` calls and typed class
property receivers such as `notifier.notifyEvent(...)` and
`self.repository.save(...)`, with imported property types resolved through the
same SwiftPM module rules as parameter annotations. Missing modules, ambiguous
multiple module imports, duplicate same-module owners, unimported cross-file
property owners, erased `Any` receivers, and local type or factory shadows stay
fail-closed unless a same-file owner can be resolved. C# receiver fixtures cover
explicit using-alias imports such as
`using Mailer = Acme.Mail.Notifier` when the namespace-qualified owner method is
indexed. C# visible same-file receiver coverage also includes enclosing-type
`this.Decorate(...)` calls, class field receivers such as
`notifier.Notify(...)`, `this.repository.Save(...)`, ordinary local declarations
such as `Workflow workflow = makeWorkflow(); workflow.Run(...)`, direct
constructor temporaries such as `new Workflow(...).Run(...)`,
`var workflow = new Workflow(...)` direct-constructor bindings, and visible
type/static receivers such as `Program.MakeWorkflow()`. Plain namespace `using`
directives, missing alias targets, duplicate alias local names, local type
shadows, parameter-name shadows, `var` factory-returned receivers,
erased/dynamic receivers, base-class dispatch, and cross-file local owner lookup
stay fail-closed unless a same-file owner can be resolved. C# using-alias
receiver fixtures also cover alias-typed class fields and alias-typed local
declarations. PHP receiver fixtures cover non-grouped explicit use-alias imports
such as `use Acme\Mail\Notifier as Mailer` when the namespace-qualified owner
method is indexed. PHP visible same-file receiver coverage also includes
enclosing-type `$this->decorate(...)` calls, direct constructor temporaries such
as `(new Workflow())->run(...)`, and local constructor assignments such as
`$workflow = new Workflow(); $workflow->run(...)`. Typed same-file property
receivers such as `$this->notifier->notify(...)` also resolve when the property
type comes from an explicit property declaration or constructor property
promotion, and use-alias property receiver fixtures resolve through the same
namespace-qualified exact-owner rule as use-alias parameter annotations.
Use-alias constructor receivers such as `$workflow = new RemoteWorkflow()` and
`(new RemoteWorkflow())->run(...)` resolve through the same alias rule. Plain
`use` imports, missing alias targets, duplicate alias local names, local type
shadows, grouped use-alias imports, factory-returned receivers, untyped property
receivers, and cross-file local owner lookup stay fail-closed unless a same-file
owner can be resolved.

All parser-backed graph languages route through semantic call/import candidate
dispatch, but that only proves simple candidate resolution. Treat
language-specific typed receiver behavior, framework/domain handoffs, and
agent packet sufficiency as separate claims until their targeted fixtures and
packet evidence pass. The generic semantic resolver provides a low-confidence
floor for parser-backed languages that do not yet have custom resolvers:
same-language direct calls and straightforward package/path import tails can
produce probable candidates when integration fixtures cover the language shape.
That does not imply generalized cross-package typed receiver lookup,
framework-aware target selection, or import-alias support beyond these
fixture-backed claims: JavaScript same-file receiver ownership plus imported
constructor locals and property receivers, Python imported receiver annotations, TypeScript same-file
receiver ownership and relative imported receiver annotations plus TypeScript/TSX imported constructor locals,
Java explicit imported receiver parameters, Kotlin explicit imported receiver
parameters plus exact imported constructor locals, Swift SwiftPM module imported receiver
parameters plus imported constructor locals, C++ same-file typed receiver
parameters, C# using-alias imported receiver parameters plus same-file
field/local/constructor receiver ownership, Go qualified imported receiver
parameters plus qualified imported composite locals, Ruby same-file constructor
receiver ownership plus exact single-file `require_relative` constructor owners, PHP use-alias
imported receiver parameters and property receivers plus same-file
self/property/constructor receiver ownership plus use-alias constructor
receivers, and Dart prefixed relative imported receiver parameters plus
prefixed imported constructor locals. Header
files keep the shared registry default of
`.h` as C for path-only semantic detection;
index-time C++ header upgrades from compile/source signals are a parser-routing
detail until semantic requests carry that resolved language identity explicitly.

## Parser Compatibility Matrix

This table is a parser-version compatibility record, not a runtime support
claim. Candidate parser crates are judged against the workspace parser-version
policy before they become durable language-support evidence:

- `tree-sitter = "0.24"`
- `tree-sitter-graph = "0.12"`

Validation method: checked candidate parser crates in an isolated temporary probe
crate (outside workspace members) with `tree-sitter = "0.24"`,
`tree-sitter-graph = "0.12"`, and exactly one pinned `<language-parser-crate>`
dependency, then ran `cargo check` for each language.

| Language | Candidate crate | Version checked | `cargo check` with 0.24/0.12 | Decision | Notes |
|---|---|---:|---|---|---|
| Go | `tree-sitter-go` | `0.23.4` | pass (`cargo check` + parse smoke) | crates.io pin | `0.25.0` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| Ruby | `tree-sitter-ruby` | `0.23.1` | pass (`cargo check` + parse smoke) | crates.io pin | Wired in indexer with `rules/ruby.scm`. |
| PHP | `tree-sitter-php` | `0.23.11` | pass (`cargo check` + parse smoke) | crates.io pin | `0.24.2` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| C# | `tree-sitter-c-sharp` | `=0.23.0` | pass (`cargo check` + parse smoke) | crates.io pin | `0.23.5` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| Kotlin | `tree-sitter-kotlin-ng` | `1.1.0` | pass (`cargo check` + parse smoke) | crates.io pin | Wired in indexer with `rules/kotlin.scm`. |
| Swift | `tree-sitter-swift` | `0.7.0` | pass (`cargo check` + parse smoke) | crates.io pin | `0.7.1` and newer tested candidates use ABI 15 and fail at runtime on tree-sitter `0.24`. |
| Dart | `tree-sitter-dart-orchard` | `0.3.2` | pass (`cargo check` + parse smoke) | crates.io pin | Replaces `tree-sitter-dart = 0.2.0`, whose language export uses ABI 15 with tree-sitter `0.24`. |
| HTML | `tree-sitter-html` | `0.23.2` | pass | crates.io pin | Parser is available if structural extraction chooses parser-backed route. |
| CSS | `tree-sitter-css` | `0.25.0` | pass | crates.io pin | Parser is available if structural extraction chooses parser-backed route. |
| SQL | `tree-sitter-sequel` | `0.3.11` | pass | crates.io pin | SQL parser candidate compiles with policy pins. |
| Bash | `tree-sitter-bash` | `0.23.3` | pass (`cargo check` + parse smoke) | crates.io pin | `0.25.x` uses ABI 15 and fails at runtime on tree-sitter `0.24`. |

Current outcome:

- No language in this matrix currently requires a git pin, custom fork, or forced
  text-only fallback for parser-policy compatibility.
- Go, Ruby, PHP, C#, Kotlin, Swift, Dart, and Bash have parser dependencies,
  rule assets, and extension routing wired in the current branch.
- HTML, CSS, and SQL have structural extraction paths, but they are not
  parser-backed rule assets from this matrix.
- New parser candidates should stay on this page as compatibility records until
  they also have dependency wiring, rule assets, language routing, and fidelity
  coverage.

## Route Coverage Is Separate

Framework route extraction has its own confidence labels in
[framework-route-coverage.md](../testing/framework-route-coverage.md). A
language can have parser-backed graph support while a framework remains
partial or heuristic. A route claim needs fixture or real-repo route evidence,
not just a language parser.

## Expansion Checklist

Before adding a new parser-backed language or broader framework claim:

1. Add or update the parser/rule path and extension mapping.
2. Add tictactoe coverage for symbol, import, call, member, and inheritance
   shapes that the language can reasonably represent.
3. Add or update fidelity-lab fixtures for symbols, imports, call edges, and
   any resolution behavior being claimed.
4. Add targeted resolution tests before claiming local receiver-aware,
   polymorphic, cross-package, framework-handler, or owner-qualified call trails.
5. Update `crates/codestory-contracts/src/language_support.rs`, including
   `language_support_profile_for_ext` and
   `language_support_profile_for_language_name`, parser construction such as
   `get_language_for_ext`, and this page in the same change.
6. Add or update the
   [OSS language corpus](../testing/oss-language-corpus.md) entry so the new
   public language-support profile has a pinned medium-sized open source project and
   a raw-without-CodeStory indexing comparison lane.
7. Add or update the `language-expansion-holdout` task manifest so the language
   also has a strict `without_codestory` versus `with_codestory` agent A/B task
   that measures elapsed time, tokens, tool calls, command counts, source reads,
   post-packet source reads, and answer quality.
8. Run the full test binaries, not filtered test names:

   ```sh
   cargo test -p codestory-indexer --test fidelity_regression
   cargo test -p codestory-indexer --test tictactoe_language_coverage
   cargo test -p codestory-indexer --test call_resolution_common_methods
   cargo test -p codestory-indexer --test import_resolution
   cargo test -p codestory-indexer --test query_rule_regressions
   cargo test -p codestory-indexer --test trait_interface_resolution
   ```

9. For broader real-project smoke evidence, run either the OSS corpus dry-run
   manifest check or the relevant full corpus language subset:

   ```sh
   CODESTORY_OSS_CORPUS_DRY_RUN=1 cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
   CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 CODESTORY_OSS_CORPUS_LANGUAGES=python cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
   ```

10. For agent-facing evidence, run at least the targeted language task from the
    A/B suite, and run the full suite before making language-wide savings or
    answer-quality claims:

    ```sh
    node scripts/codestory-agent-ab-benchmark.mjs \
      --task-suite language-expansion-holdout \
      --arms without_codestory,with_codestory \
      --repeats 3 --materialize-repos --prepare-codestory-cache \
      --out-dir target/agent-benchmark/language-expansion-holdout \
      --timeout-ms 600000
    ```

11. Before widening typed receiver-call claims, add same-file and cross-file
    fixtures for the target language. If implementation still uses signature
    string slicing, document that as a transitional boundary; prefer a
    tree-sitter-query or global-resolution-backed implementation for new
    claims.
