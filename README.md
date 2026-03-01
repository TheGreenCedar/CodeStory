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

## LLM Retrieval Configuration (Local-Only)

`/api/agent/ask` now expects a local semantic embedding setup by default.

- `CODESTORY_EMBED_MODEL_PATH` (required for hybrid retrieval): absolute path to a local embedding model artifact.
- `CODESTORY_EMBED_MODEL_ID` (optional): identifier recorded with stored embeddings.
- `CODESTORY_EMBED_TOKENIZER_PATH` (optional): tokenizer JSON path. Defaults to `tokenizer.json` next to the model.
- `CODESTORY_EMBED_RUNTIME_MODE` (optional, default `onnx`): set to `hash` for deterministic local dev/benchmark embeddings.
- `CODESTORY_HYBRID_RETRIEVAL_ENABLED` (optional, default `true`): set to `false`/`0` for lexical rollback mode.
- `CODESTORY_CORS_ALLOW_ANY` (optional, default `false`): set to `true` only when you intentionally need permissive cross-origin access.

Default CORS policy is local-first and explicit:
- `http://127.0.0.1:<server-port>`
- `http://localhost:<server-port>`
- `http://127.0.0.1:5173`
- `http://localhost:5173`

The server still defaults to `127.0.0.1:7878`. If you bind `--host` to a non-loopback address, startup logs a warning.

Examples:

```powershell
$env:CODESTORY_EMBED_MODEL_PATH = "C:\models\all-minilm-l6-v2.onnx"
$env:CODESTORY_EMBED_MODEL_ID = "sentence-transformers/all-MiniLM-L6-v2"
cargo run -p codestory-server -- --project .
```

Rollback example:

```powershell
$env:CODESTORY_HYBRID_RETRIEVAL_ENABLED = "false"
cargo run -p codestory-server -- --project .
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
