# CodeStory

CodeStory is a Rust-first code understanding system with a web UI. It indexes a repository into a local SQLite graph, then uses symbol search, graph exploration, and agent responses to help you inspect unfamiliar code quickly.

The project is actively evolving. Current architecture is centered on a headless app core (`codestory-app`), an HTTP server (`codestory-server`), and a React frontend (`codestory-ui`).

## Highlights

- Graph-grounded exploration around symbols
- Trail controls (mode, direction, depth, edge/node filters, target symbol pathing)
- Full-text and fuzzy search over indexed symbols
- Editable code pane with incremental re-index flow
- Event-streamed status updates from backend to UI

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
- Node.js + npm (for `codestory-ui`)
- A C/C++ build toolchain may be required on some platforms for native dependencies

### Run Full Stack In Dev Mode

From the workspace root:

```powershell
cd codestory-ui
npm install
npm run dev:all
```

This starts:

- Vite UI at `http://127.0.0.1:5173`
- API server at `http://127.0.0.1:7878`

### Run Backend And Frontend Separately

Terminal 1 (from repo root):

```powershell
cargo run -p codestory-server -- --project .
```

Terminal 2 (from repo root):

```powershell
cd codestory-ui
npm run dev
```

### Build For Local Production-Style Serving

```powershell
cd codestory-ui
npm run build
cd ..
cargo run -p codestory-server -- --project . --frontend-dist codestory-ui/dist
```

## CLI Indexer

For direct indexing workflows:

```powershell
cargo run -p codestory-cli -- --path .
```

Use `--db <path>` to write to a custom SQLite file.

## Common Development Commands

Backend (repo root):

```powershell
cargo build
cargo test
cargo check
cargo fmt
cargo clippy
```

Frontend (`codestory-ui`):

```powershell
npm run check
```

Regenerate shared TypeScript API bindings (repo root):

```powershell
cargo run -p codestory-server -- --types-only --types-out codestory-ui/src/generated/api.ts
```

## Repository Layout

- Workspace manifest: `Cargo.toml`
- Rust crates: `crates/`
  - `codestory-core`: shared graph/index domain types
  - `codestory-events`: event types and event bus
  - `codestory-project`: workspace discovery/refresh metadata
  - `codestory-index`: tree-sitter + semantic resolution indexing pipeline
  - `codestory-storage`: SQLite schema and query layer
  - `codestory-search`: search primitives over indexed data
  - `codestory-api`: API DTOs and identifiers shared with frontend
  - `codestory-app`: headless orchestrator
  - `codestory-server`: Axum API + SSE + optional static file serving
  - `codestory-cli`: standalone indexing CLI
  - `codestory-bench`: Criterion benchmark suite
- Web UI: `codestory-ui/` (Vite + React + TypeScript)

## Generated Runtime Files

Running CodeStory locally creates artifacts such as:

- `codestory.db` (SQLite index)
- `codestory_ui.json` (persisted UI layout/state)

These files are local runtime state and are ignored via `.gitignore`.

## License

MIT. See `LICENSE`.
