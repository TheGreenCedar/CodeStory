# Repository Guidelines

## Project Structure & Module Organization
- Rust workspace in `Cargo.toml`; crates live under `crates/`.
- Core crates: `codestory-core`, `codestory-events`, `codestory-storage`, `codestory-index`, `codestory-search`, `codestory-graph`, `codestory-gui`.
- Runtime artifacts: `codestory.db`, `codestory_ui.json`; build outputs in `target/`.

## Architecture Overview
- Event-driven flow via `codestory-events` `EventBus` decouples GUI, indexer, and storage.
- Indexing: `codestory-project` discovers files, `codestory-index` extracts via tree-sitter, `codestory-storage` batch-writes SQLite.
- UI is immediate-mode `egui`; state lives in `codestory-gui` `CodeStoryApp` and components.

## Build, Test, and Development Commands
- Build/run: `cargo build -p codestory-gui`, `cargo run -p codestory-gui` (add `RUST_LOG=info` for logs).
- Quality: `cargo test` or `cargo test -p <crate>`, `cargo check`, `cargo fmt`, `cargo clippy`.

## Coding Style & Naming Conventions
- Toolchain is nightly (`rust-toolchain.toml`).
- Rust naming: `snake_case` for functions/modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
- Tests live in `#[cfg(test)]` or `*_tests.rs` (e.g., `crates/codestory-gui/src/components/file_watcher_tests.rs`); name them `test_*`.

## Commit & Pull Request Guidelines
- Commit messages are short, lowercase, imperative (e.g., `fix minimap`, `refactor graph style`).
- PRs should include a summary, tests run, linked issues, and screenshots for GUI changes.

## Security & Configuration Tips
- IDE integration uses TCP port `6667`; keep secrets out of the repo and use environment variables.
