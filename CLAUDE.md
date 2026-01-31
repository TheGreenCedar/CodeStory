# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CodeStory is a modern source code explorer written in Rust, a spiritual successor to Sourcetrail. It helps developers navigate unfamiliar codebases through interactive dependency graphs, code snippets, and fuzzy search. The GUI is built with `egui`/`eframe`.

## Build & Development Commands

```bash
# Build the GUI application
cargo build -p codestory-gui

# Run the GUI application
cargo run -p codestory-gui

# Run with logging enabled
RUST_LOG=info cargo run -p codestory-gui

# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p codestory-index

# Run a single test by name
cargo test -p codestory-storage test_trail_query

# Check for warnings/errors without building
cargo check

# Format code
cargo fmt

# Lint
cargo clippy

# Run benchmarks
cargo bench -p codestory-bench
```

## Architecture

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `codestory-core` | Shared domain types: `Node`, `Edge`, `NodeId`, `EdgeId`, `SourceLocation`, `TrailConfig`. All enums use `#[repr(i32)]` for SQLite compatibility. |
| `codestory-events` | Central `EventBus` for decoupled communication. Components publish/subscribe to typed `Event` variants. Implement `EventListener` trait to receive events. |
| `codestory-storage` | SQLite persistence via `rusqlite`. Handles batch inserts, caching (`StorageCache`), and trail queries (BFS subgraph exploration). |
| `codestory-index` | Tree-sitter based indexing. `WorkspaceIndexer::run_incremental()` does parallel file processing via `rayon`, then batch commits to storage. |
| `codestory-project` | Project lifecycle, glob-based file discovery, `RefreshInfo` for incremental updates. |
| `codestory-search` | Fuzzy search via `nucleo-matcher`, full-text via `tantivy`. |
| `codestory-graph` | Graph layout algorithms (`ForceDirectedLayouter`, `RadialLayouter`, `NestingLayouter`, `GridLayouter`), `GraphModel` for in-memory representation, `NodeBundler` for edge bundling. Uses `oak-visualize` for some layouts. |
| `codestory-gui` | Main application. `CodeStoryApp` holds all state. Components in `src/components/`. Uses `egui_dock` for docking and a custom graph canvas for node graph visualization. |

### Key Patterns

**Event-Driven Communication:**
```rust
// Publishing
event_bus.publish(Event::ActivateNode { id, origin: ActivationOrigin::Search });

// Listening (implement EventListener)
fn handle_event(&mut self, event: &Event) {
    match event {
        Event::ActivateNode { id, .. } => self.select_node(*id),
        _ => {}
    }
}
```

**Indexing Pipeline:**
1. `codestory-project::Project::full_refresh()` discovers files to index
2. `WorkspaceIndexer::run_incremental()` parses files in parallel with tree-sitter
3. Results collected into `IntermediateStorage`, then batch-committed to SQLite
4. Events published: `IndexingStarted` -> `IndexingProgress` -> `IndexingComplete`

**Storage Pattern:**
- All writes use batch operations with transactions for performance
- `StorageCache` (with `parking_lot::RwLock`) caches nodes in memory
- Trail queries use BFS traversal with configurable depth, direction, and edge filters

**GUI State:**
- `CodeStoryApp` is the main state holder, implements `eframe::App`
- UI components are structs with `ui(&mut self, ui: &mut egui::Ui)` methods
- `DockState` manages panel layout (persisted to `codestory_ui.json`)
- Settings persisted via `AppSettings::save()`/`load()`

### Data Flow

```
User Input -> Event published to EventBus
           -> CodeStoryApp::handle_event() processes event
           -> Updates relevant component state
           -> UI re-renders on next frame (immediate mode)
```

### Important Types

- `NodeId(i64)` / `EdgeId(i64)` - Database primary keys, used everywhere
- `NodeKind` / `EdgeKind` - Enums with `TryFrom<i32>` for DB storage
- `SourceLocation` - 1-based line/column positions (matches Sourcetrail format)
- `TrailConfig` / `TrailResult` - For subgraph exploration queries

## File Structure Notes

- `crates/*/src/lib.rs` - Main entry point for each crate
- `crates/codestory-gui/src/app.rs` - Main application state (~1500 lines)
- `crates/codestory-gui/src/components/` - UI components (sidebar, code_view, node_graph, etc.)
- `crates/codestory-index/src/lib.rs` - Contains tree-sitter graph queries for each language
- Generated files: `codestory.db` (SQLite), `codestory_ui.json` (UI state)

## IDE Integration

CodeStory listens on TCP port 6667 for JSON-encoded `IdeMessage` objects. Format:
```json
{"type": "SetActiveLocation", "file_path": "...", "line": 10, "column": 1}
```

## Adding Language Support

1. Add `tree-sitter-<lang>` to workspace dependencies in root `Cargo.toml`
2. Add language detection in `codestory-index/src/lib.rs::get_language_for_ext()`
3. Add tree-sitter-graph queries for node extraction
4. Add relationship queries in `get_relationship_queries()`
