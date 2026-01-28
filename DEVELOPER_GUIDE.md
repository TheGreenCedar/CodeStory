# CodeStory Developer Guide

This guide provides an architectural overview and contribution guidelines for CodeStory.

## Architecture Overview

CodeStory is a modular Rust workspace consisting of several specialized crates:

| Crate | Responsibility |
|-------|----------------|
| `codestory-core` | Shared types (`Node`, `Edge`, `Location`), Protocol definitions. |
| `codestory-events` | Central event bus and event definitions for decoupled communication. |
| `codestory-storage` | SQLite schema management, batch inserts, and in-memory caching. |
| `codestory-index` | Tree-sitter based indexing pipeline, parallel file processing. |
| `codestory-project` | Project lifecycle, glob-based file discovery, incremental refresh logic. |
| `codestory-search` | Fuzzy search (Nucleo) and full-text search (Tantivy). |
| `codestory-graph` | Graph layout algorithms and model management. |
| `codestory-gui` | `egui` based user interface, tab management, state persistence. |

## Key Patterns

### 1. Event-Driven Communication
Components communicate primarily through the `EventBus`. This prevents tight coupling between the GUI, the indexer, and the storage layer.
- **Publish**: `event_bus.publish(Event::ActivateNode { id, origin })`
- **Listen**: Implement the `EventListener` trait and register with the bus.

### 2. Immediate Mode UI (egui)
The GUI is built with `egui`. Most state is kept in the `CodeStoryApp` struct or delegated to specialized component structs.
- **Virtualized Scrolling**: Large files in `CodeView` use `ScrollArea::show_rows` for performance.
- **Conditional Rendering**: UI responds to global `AppSettings`.

### 3. High-Performance Indexing
Indexing is split into two phases:
1. **Extraction**: Parallel tree-sitter traversal producing intermediate data.
2. **Commit**: Batch insertion into SQLite within a transaction.

## Adding a New Language

1. Add the corresponding `tree-sitter-<lang>` crate to `Cargo.toml`.
2. Implement semantic queries in `codestory-index/src/languages/`.
3. Update `codestory-gui` `CodeView` syntax highlighting configuration.

## IDE Protocol

CodeStory listens on TCP port `6667`. Messages are JSON-encoded `IdeMessage` objects.
- **Format**: `{"type": "SetActiveLocation", "file_path": "...", "line": 10, "column": 1}`

## Build & Test

```bash
# Build the application
cargo build -p codestory-gui

# Run all tests
cargo test

# Run with logging
RUST_LOG=info cargo run -p codestory-gui
```
