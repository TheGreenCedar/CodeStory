# CodeStory Project Context

## Project Overview

**CodeStory** is a modern, cross-platform source code explorer written in Rust. It is a spiritual successor and migration of the original **Sourcetrail** project, designed to help developers navigate and understand unfamiliar codebases through interactive dependency graphs, code snippets, and powerful search capabilities.

## Architecture

The project is structured as a **Cargo Workspace** containing modular crates:

| Crate | Description |
|-------|-------------|
| **`codestory-gui`** | The main application entry point. Built with `egui` and `eframe`. Handles the UI, tab management, and visual components. |
| **`codestory-core`** | Contains shared domain types (`Node`, `Edge`, `Location`), error definitions, and the IDE protocol. |
| **`codestory-index`** | The indexing engine. Uses `tree-sitter` and `tree-sitter-graph` to parse code and extract semantic relationships in parallel. |
| **`codestory-storage`** | Manages the SQLite database, handling batch insertions, schema management, and caching. |
| **`codestory-graph`** | Responsible for graph layout algorithms and the in-memory graph model. |
| **`codestory-search`** | Implements fuzzy search (via `nucleo-matcher`) and full-text search (via `tantivy`). |
| **`codestory-events`** | A central `EventBus` that decouples the GUI, indexer, and other systems. |
| **`codestory-project`** | Manages project lifecycle, file discovery (glob patterns), and incremental updates. |
| **`codestory-cli`** | A command-line interface for the application (alternative entry point). |
| **`codestory-bench`** | Benchmarking suite for performance testing. |

## Key Technical Patterns

*   **Event-Driven Architecture:** Components communicate asynchronously via the `EventBus` (in `codestory-events`) to avoid tight coupling.
*   **Immediate Mode GUI:** The UI is purely functional and stateless where possible, relying on `egui`. State is persisted in `CodeStoryApp` or specialized component structs.
*   **Hybrid Indexing:** Indexing involves a high-performance parallel extraction phase (tree-sitter) followed by a transactional commit phase to SQLite.
*   **IDE Integration:** Listens on TCP port `6667` for JSON-based messages (`IdeMessage`) to sync selection with external IDEs.

## Development Workflow

### Prerequisites
*   **Rust Toolchain:** Ensure you have the latest stable Rust installed (`rustup update`).
*   **Dependencies:** Standard build tools for your OS (e.g., C++ build tools for `tree-sitter` bindings).

### Common Commands

**Build the GUI:**
```bash
cargo build -p codestory-gui
```

**Run the Application:**
```bash
cargo run -p codestory-gui
```

**Run with Logging:**
```bash
# Enable info-level logs
RUST_LOG=info cargo run -p codestory-gui
```

**Run Tests:**
```bash
cargo test
```

### Directory Structure Key
*   `crates/`: Source code for all workspace members.
*   `docs/`: detailed implementation docs and phase guides.
*   `codestory.db`: The SQLite database file (generated).
*   `codestory_ui.json`: Persisted UI state.

## Contribution Guidelines
*   **Code Style:** Follow standard Rust idioms (`cargo fmt`, `cargo clippy`).
*   **Testing:** Write unit tests for new logic. Integration tests in a `tests/` subfolder of the crate to test.
*   **UI Changes:** When modifying the GUI, ensure responsiveness. Large lists should be virtualized (`ScrollArea::show_rows`).
