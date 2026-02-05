# CodeStory

CodeStory is a modern, Rust-based source code explorer inspired by [Sourcetrail](https://github.com/CoatiSoftware/Sourcetrail). It helps you understand unfamiliar codebases by combining an interactive dependency graph with code snippets and fast search. It will eventually incorporate coding agents of your choice to explain the codebase like a story book, using the generated graphs as grounding context + interactive visuals for the user.

BIG NOTE: This project is at its infancy, I just got it to render graph nodes without dying on me. Contributions (bot or human) are welcome 😄

## What You Get

- Interactive graph visualization centered on the selected symbol
- Code view with snippet/full-file modes and clickable highlighted tokens
- Tabbed navigation with back/forward history
- High-performance, parallel indexing powered by tree-sitter
- Local persistence for UI state and an on-disk SQLite index

For a walkthrough of the UI, see `USER_GUIDE.md`. For architecture and contribution notes, see `DEVELOPER_GUIDE.md`.

## Quickstart

### Prerequisites

- Rust toolchain: this repo pins **nightly** via `rust-toolchain.toml`. I'll use stable releases once I'm happy enough with the feature set and can spend time stabilizing.
- A C/C++ toolchain may be required on some platforms because dependencies can include native components

### Run the GUI

```bash
cargo run -p codestory-gui
```

Optional logging:

```bash
RUST_LOG=info cargo run -p codestory-gui
```

### Build and Test

```bash
cargo build
cargo test
cargo fmt
cargo clippy
```

## Repository Layout

- Workspace manifest: `Cargo.toml`
- Crates: `crates/`
  - `codestory-core`: shared types (nodes/edges/locations)
  - `codestory-events`: event bus for decoupled communication
  - `codestory-project`: workspace discovery and file watching
  - `codestory-index`: tree-sitter based extraction pipeline
  - `codestory-storage`: SQLite schema + batch writes
  - `codestory-search`: fuzzy + full-text search
  - `codestory-graph`: graph model/layout
  - `codestory-gui`: egui/eframe application

## Generated Files (Not Committed)

Running CodeStory will generate local artifacts like:

- `codestory.db` (SQLite index)
- `codestory_ui.json` (UI state)

These are intentionally ignored via `.gitignore`.

## IDE Integration

CodeStory can listen for Sourcetrail-style IDE messages over TCP.

- Default port: `6667`

## License

MIT. See `LICENSE`.
