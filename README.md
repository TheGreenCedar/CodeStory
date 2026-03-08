# CodeStory

CodeStory is a skill-first codebase grounding engine for local repositories. It builds a SQLite-backed symbol and relationship graph, then exposes grounding primitives that higher-level skills, agents, and runtimes can compose.

The canonical packaging target is `codestory-cli`, and the repo-local skill lives at `.agents/skills/codestory-grounding`.

## Grounding Workflows

The repo is organized around six grounding verbs:

- `index`: discover files, parse supported languages, and persist graph/search state locally
- `ground`: turn a prompt into grounded code context using indexed symbols, snippets, and graph traversal
- `search`: find likely symbols, files, and matches by name or text
- `symbol`: inspect one symbol and its indexed metadata
- `trail`: walk neighborhoods or focused paths through the code graph
- `snippet`: return focused source context for a symbol or location

## Supported Languages (Indexer)

- Python
- Java
- Rust
- JavaScript
- TypeScript/TSX
- C
- C++

## Quickstart

### Prerequisites

- Rust toolchain (edition 2024 crates in this workspace)
- A native build toolchain may be required on some platforms for parser or dependency builds

### Build The CLI Runtime

```powershell
cargo build --release -p codestory-cli
```

### Create Or Refresh A Local Index

```powershell
cargo run --release -p codestory-cli -- index --project .
```

This writes repo-local grounding data into the user cache by default, keyed by the target project path.

### Command Model

The docs and packaging now center on this runtime surface:

```text
codestory-cli index --project <path> [--refresh auto|full|incremental]
codestory-cli ground --project <path> [--budget strict|balanced|max]
codestory-cli search --project <path> --query <query>
codestory-cli symbol --project <path> (--id <node-id> | --query <query>)
codestory-cli trail --project <path> (--id <node-id> | --query <query>)
codestory-cli snippet --project <path> (--id <node-id> | --query <query>)
```

The bundled skill scripts in `.agents/skills/codestory-grounding/scripts/` are thin wrappers around these commands.

## Common Development Commands

From the workspace root:

```powershell
cargo check -p codestory-cli
cargo build
cargo test
cargo fmt
cargo clippy
```

Use `cargo check -p codestory-cli` for the fastest packaging-oriented validation pass when you are working on the grounding runtime surface.

## Repository Layout

- `Cargo.toml`: workspace manifest
- `crates/codestory-cli`: canonical CLI packaging target for grounding workflows
- `crates/codestory-app`: headless orchestrator used by higher-level runtimes
- `crates/codestory-project`: repository discovery and refresh metadata
- `crates/codestory-index`: tree-sitter plus semantic indexing pipeline
- `crates/codestory-storage`: SQLite schema, persistence, and trail queries
- `crates/codestory-search`: lexical and semantic retrieval primitives
- `crates/codestory-core`: shared graph and domain types
- `crates/codestory-events`: event types used by adapters and status flows
- `crates/codestory-api`: DTOs shared by the CLI, skill, and adapter layers
- `.agents/skills/codestory-grounding`: repo-local skill scripts and instructions
- `crates/codestory-bench`: Criterion benchmarks for performance and fidelity work

## Runtime Artifacts

Running CodeStory locally creates runtime state such as:

- user-cache SQLite grounding indexes keyed by project path

## License

MIT. See `LICENSE`.
