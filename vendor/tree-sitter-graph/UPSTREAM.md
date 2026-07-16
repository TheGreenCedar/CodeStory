# Vendored tree-sitter-graph

This directory contains `tree-sitter-graph` 0.12.0 from upstream commit
`b930fb59c2177a90b3a6a68e1feeca6918ceb58b`.

CodeStory carries this narrow source copy because the published 0.12.0 crate
pins `tree-sitter` 0.24, whose runtime cannot load current ABI 15 grammars such
as `tree-sitter-rust` 0.24.2. The local changes are limited to:

- using `tree-sitter` 0.26.11;
- retaining only the library surface CodeStory links, without the upstream CLI,
  development dependencies, or optional terminal coloring;
- explicit elided lifetimes required by the current lint surface; and
- disabling publication of the vendored package.

Remove this copy and return to the crates.io dependency after upstream ships a
release compatible with `tree-sitter` 0.26 or newer.
