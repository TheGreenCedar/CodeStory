# Repository Guidelines

## Project Structure & Module Organization
- Rust workspace in `Cargo.toml`; crates live under `crates/`.
- Core crates: `codestory-core`, `codestory-events`, `codestory-storage`, `codestory-index`, `codestory-search`, `codestory-graph`, `codestory-app`, `codestory-api`, `codestory-cli`.
- Runtime artifacts: `codestory.db`, `codestory_ui.json`; build outputs in `target/`.

## Architecture Overview
- Event-driven flow via `codestory-events` `EventBus` decouples app orchestration, indexer, and storage.
- Indexing: `codestory-project` discovers files, `codestory-index` extracts via tree-sitter, `codestory-storage` batch-writes SQLite.

## Build, Test, and Development Commands
- Build/run: `cargo build`, `cargo run -p codestory-cli -- --help` (add `RUST_LOG=info` for logs).
- Quality: `cargo test` or `cargo test -p <crate>`, `cargo check`, `cargo fmt`, `cargo clippy`.
- Playwright skill wrapper (`~/.codex/skills/playwright/scripts/playwright_cli.sh`) is Bash-based; on this Windows setup use `C:\Program Files\Git\bin\bash.exe -lc ...` rather than WSL `bash` when WSL has no distro configured.
- When iterating over a specific component with playwright, crop only the parts of the page that you need to save context.
- For tight Playwright crops on this Windows setup, prefer element screenshots then crop with PowerShell (`System.Drawing`) instead of `run-code` snippets with nested shell quoting.
- With playwright, make sure you start the browser maximized

## Coding Style & Naming Conventions
- Toolchain is nightly (`rust-toolchain.toml`).
- Rust naming: `snake_case` for functions/modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
- Tests live in `#[cfg(test)]` or `*_tests.rs`; name them `test_*`.

## Commit & Pull Request Guidelines
- Commit messages are short, lowercase, imperative (e.g., `fix minimap`, `refactor graph style`).
- PRs should include a summary, tests run, linked issues, and relevant artifacts for behavior changes.

## Security & Configuration Tips
- IDE integration uses TCP port `6667`; keep secrets out of the repo and use environment variables.
