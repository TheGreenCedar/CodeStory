# tree-sitter-graph

[![DOI](https://zenodo.org/badge/368886913.svg)](https://zenodo.org/badge/latestdoi/368886913)

CodeStory vendors only the library surface. The upstream CLI, development
dependencies, and terminal-color feature are intentionally omitted; see
[UPSTREAM.md](UPSTREAM.md) for provenance and the complete local delta.

The `tree-sitter-graph` library defines a DSL for constructing arbitrary graph
structures from source code that has been parsed using [tree-sitter][].

[tree-sitter]: https://tree-sitter.github.io/

- [Language Reference](https://docs.rs/tree-sitter-graph/*/tree_sitter_graph/reference/)
- [API documentation](https://docs.rs/tree-sitter-graph/)
- [Release notes](https://github.com/tree-sitter/tree-sitter-graph/blob/main/CHANGELOG.md)
- [VS Code Extension](https://marketplace.visualstudio.com/items?itemName=tree-sitter.tree-sitter-graph)

## Usage

Use this copy as a library path dependency:

``` toml
[dependencies]
tree-sitter-graph = "0.12"
```

## Development

The project is written in Rust, and requires a recent version installed.
Rust can be installed and updated using [rustup][].

[rustup]: https://rustup.rs/

Build the project by running:

```
$ cargo build
```

Run the tests by running:

```
$ cargo test
```

This vendored package contains only the library used by CodeStory. Upstream
owns the CLI and its development surface.

Sources are formatted using the standard Rust formatted, which is applied by running:

```
$ cargo fmt
```
