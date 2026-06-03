# Retrieval parser compatibility matrix (ws-a-parser-compat)

This records Step 2 parser compatibility decisions from `retrieval-language-support_038d3ae9.plan.md` against the workspace policy:

- `tree-sitter = "0.24"`
- `tree-sitter-graph = "0.12"`

## Validation method

Checked candidate parser crates in an isolated temporary probe crate (outside workspace members) with this dependency shape:

```toml
[dependencies]
tree-sitter = "0.24"
tree-sitter-graph = "0.12"
<language-parser-crate> = "=<pinned-version>"
```

For each language, ran `cargo check` after pinning exactly one parser crate/version.

## Decision matrix

| Language | Candidate crate | Version checked | `cargo check` with 0.24/0.12 | Decision | Notes |
|---|---|---:|---|---|---|
| Go | `tree-sitter-go` | `0.23.4` | pass (`cargo check` + parse smoke) | crates.io pin | `0.25.0` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| Ruby | `tree-sitter-ruby` | `0.23.1` | pass (`cargo check` + parse smoke) | crates.io pin | Wired in indexer with `rules/ruby.scm`. |
| PHP | `tree-sitter-php` | `0.23.11` | pass (`cargo check` + parse smoke) | crates.io pin | `0.24.2` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| C# | `tree-sitter-c-sharp` | `=0.23.0` | pass (`cargo check` + parse smoke) | crates.io pin | `0.23.5` compiles but fails at runtime with `LanguageError { version: 15 }` on tree-sitter `0.24`. |
| Kotlin | `tree-sitter-kotlin-ng` | `1.1.0` | pass | crates.io pin | Use `-ng` crate family for Kotlin parser wiring. |
| Swift | `tree-sitter-swift` | `0.7.2` | pass | crates.io pin | crates.io source compiles with policy pins. |
| Dart | `tree-sitter-dart` | `0.2.0` | pass | crates.io pin | crates.io source compiles with policy pins. |
| HTML | `tree-sitter-html` | `0.23.2` | pass | crates.io pin | Parser is available if structural extraction chooses parser-backed route. |
| CSS | `tree-sitter-css` | `0.25.0` | pass | crates.io pin | Parser is available if structural extraction chooses parser-backed route. |
| SQL | `tree-sitter-sequel` | `0.3.11` | pass | crates.io pin | SQL parser candidate compiles with policy pins. |
| Bash | `tree-sitter-bash` | `0.25.1` | pass | crates.io pin | Supports script-language parser path if/when enabled. |

## Current outcome

- No language in this matrix currently requires a git pin, custom fork, or forced text-only fallback for **parser-policy compatibility**.
- Go, Ruby, PHP, and C# have parser dependencies, rule assets, and extension
  routing wired in the current branch.
- HTML, CSS, and SQL have structural extraction paths, but they are not
  parser-backed rule assets from this matrix.
- Kotlin, Swift, Dart, and Bash remain compatibility decisions only. They still
  need dependency wiring, rule assets, language routing, and fidelity coverage
  before they should be described as parser-backed runtime support.
