# Repository Guidelines

## Project Structure & Module Organization
- Rust workspace is defined in `Cargo.toml`; crates live under `crates/`.
- Primary runtime surface is `crates/codestory-cli`; the repo-local skill lives under `.agents/skills/codestory-grounding`.
- Core crates: `codestory-core`, `codestory-events`, `codestory-storage`, `codestory-index`, `codestory-search`, `codestory-app`, `codestory-api`, `codestory-project`, `codestory-cli`.
- Runtime artifacts: user-cache SQLite grounding indexes keyed by repo path; build outputs in `target/`.

## Architecture Overview
- `codestory-app` is the headless orchestrator used by the CLI and skill scripts.
- `codestory-events::EventBus` decouples indexing/storage progress from API and UI updates.
- Indexing pipeline: `codestory-project` discovers files, `codestory-index` extracts symbols/edges via tree-sitter + semantic resolution, `codestory-storage` persists to SQLite.
- `codestory-api` holds DTOs shared across the CLI and any higher-level adapters.

## Build, Test, and Development Commands
- Backend build/test: `cargo build`, `cargo test`, `cargo check`, `cargo fmt`, `cargo clippy`.
- CLI runtime: `cargo run -p codestory-cli -- index --project .`.
- Skill-first grounding: `cargo run -p codestory-cli -- ground --project .`.
- On Windows, the Codex npm shim should be invoked as `codex.cmd` (typically under `%APPDATA%\\npm`); using the extensionless `codex` shim can fail with `os error 193`.
- In this PowerShell environment, large parallel file reads can truncate output; when investigating a single large file, prefer one direct read command (for example `Get-Content` or `cmd /c type`) before parallelizing.

## Coding Style & Naming Conventions
- Rust edition is `2024` across workspace crates.
- Rust naming: `snake_case` for functions/modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
- Tests live in `#[cfg(test)]` blocks or `*_tests.rs`; name them `test_*`.
- For indexer regressions, prefer targeted suites such as `cargo test -p codestory-index --test fidelity_regression`.
- For graph perf and fidelity checks, use Criterion benches in `crates/codestory-bench`.

## Commit & Pull Request Guidelines
- Commit messages are short, lowercase, imperative (e.g., `fix minimap`, `refactor graph style`).
- PRs should include a summary, tests run, linked issues, and relevant artifacts for behavior changes.

## Security & Configuration Tips
- Keep secrets out of the repo; pass credentials via environment variables.
