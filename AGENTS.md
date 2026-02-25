# Repository Guidelines

## Project Structure & Module Organization
- Rust workspace is defined in `Cargo.toml`; crates live under `crates/`.
- Runtime stack: `crates/codestory-server` (Axum API + SSE + static SPA hosting) and `codestory-ui` (Vite + React + TypeScript).
- Core crates: `codestory-core`, `codestory-events`, `codestory-storage`, `codestory-index`, `codestory-search`, `codestory-graph`, `codestory-app`, `codestory-api`, `codestory-project`, `codestory-cli`.
- Runtime artifacts: `codestory.db`, `codestory_ui.json`; build outputs in `target/` and `codestory-ui/dist/`.

## Architecture Overview
- `codestory-app` is the headless orchestrator used by the server.
- `codestory-events::EventBus` decouples indexing/storage progress from API and UI updates.
- Indexing pipeline: `codestory-project` discovers files, `codestory-index` extracts symbols/edges via tree-sitter + semantic resolution, `codestory-storage` persists to SQLite.
- `codestory-api` holds DTOs shared between Rust and generated TypeScript (`codestory-ui/src/generated/api.ts`).

## Build, Test, and Development Commands
- Backend build/test: `cargo build`, `cargo test`, `cargo check`, `cargo fmt`, `cargo clippy`.
- Server (API + optional static UI): `cargo run -p codestory-server -- --project .`.
- CLI indexer: `cargo run -p codestory-cli -- --path .`.
- Frontend dev (from `codestory-ui`): `npm install`, `npm run dev` (UI only), `npm run dev:all` (UI + server).
- Frontend quality (from `codestory-ui`): `npm run check` (`tsgo`, `oxlint`, `oxfmt --check`).
- Type generation: `cargo run -p codestory-server -- --types-only --types-out codestory-ui/src/generated/api.ts`.
- Playwright skill wrapper (`~/.codex/skills/playwright/scripts/playwright_cli.sh`) is Bash-based; on this Windows setup use `C:\Program Files\Git\bin\bash.exe -lc ...` rather than WSL `bash` when WSL has no distro configured.
- When iterating over a specific component with Playwright, capture only the relevant crop and start the browser maximized.

## Coding Style & Naming Conventions
- Rust edition is `2024` across workspace crates.
- Rust naming: `snake_case` for functions/modules, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Frontend uses `oxfmt`/`oxlint`; avoid introducing conflicting formatter/linter patterns.

## Testing Guidelines
- Tests live in `#[cfg(test)]` blocks or `*_tests.rs`; name them `test_*`.
- For indexer regressions, prefer targeted suites such as `cargo test -p codestory-index --test fidelity_regression`.
- For graph perf and fidelity checks, use Criterion benches in `crates/codestory-bench`.

## Commit & Pull Request Guidelines
- Commit messages are short, lowercase, imperative (e.g., `fix minimap`, `refactor graph style`).
- PRs should include a summary, tests run, linked issues, and relevant artifacts for behavior changes.

## Security & Configuration Tips
- Server defaults to `127.0.0.1:7878`; avoid exposing it publicly unless intentional.
- Keep secrets out of the repo; pass credentials via environment variables.
