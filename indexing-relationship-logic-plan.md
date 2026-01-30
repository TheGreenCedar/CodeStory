# Plan: Indexing Relationship Logic Improvement

**Generated**: January 30, 2026
**Estimated Complexity**: High

## Overview
Modernize indexing to be fully tree-sitter-graph driven, switch NodeId hashing to canonical ids, add edge/node metadata columns, and implement a drift-style resolution pass using line ranges. This plan assumes no production DB migrations are needed and focuses on correctness, determinism, and improved call attribution.

## Prerequisites
- Rust nightly toolchain (already in repo)
- Tree-sitter-graph rules for each language under `crates/codestory-index/rules/*.scm`
- Canonical id and metadata decisions locked:
  - Canonical id for **every node**: `"{file}:{qualified}:{start_line}"` (1-based line)
  - Node metadata columns: include **everything** (file_node_id, qualified_name, start_line, end_line, etc.)
  - Edge metadata columns: include **resolved source** and target ids, plus confidence and line (1-based)

## Sprint 1: Core Types + Schema + Canonical IDs
**Goal**: Add node/edge metadata columns, update core types, and switch NodeId hashing to canonical ids.
**Demo/Validation**:
- `cargo test -p codestory-core`
- `cargo test -p codestory-storage`

### Task 1.1: Define canonical id format + hashing helper
- **Location**: `crates/codestory-index/src/lib.rs`, `crates/codestory-core/src/lib.rs` (if shared helper is needed)
- **Description**: Use `"{file}:{qualified}:{start_line}"` for **all** nodes (including FILE, CLASS, VARIABLE, etc.). Replace `generate_id(name)` with `generate_id(canonical)` and update all call sites to pass canonical ids (1-based line numbers).
- **Dependencies**: None
- **Acceptance Criteria**:
  - All NodeId generation uses canonical ids.
  - Hashing is deterministic and includes file context.
- **Validation**:
  - Run unit tests; verify no panics in indexing tests.

### Task 1.2: Add metadata fields to core Node/Edge
- **Location**: `crates/codestory-core/src/lib.rs`
- **Description**: Add **full metadata** fields to `Node` and `Edge` structs. For Nodes: `file_node_id`, `qualified_name`, `start_line`, `end_line`, and any other useful metadata from rules. For Edges: `line`, `resolved_source_node_id`, `resolved_target_node_id`, `confidence`, plus any rule-provided attributes. All line numbers are 1-based.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - Structs compile across workspace.
  - Default values are defined or optional fields used where appropriate.
- **Validation**:
  - `cargo check`

### Task 1.3: Update storage schema + CRUD
- **Location**: `crates/codestory-storage/src/lib.rs`
- **Description**: Extend `node` and `edge` tables with all new metadata columns. Update inserts/selects to read/write the new fields. Add indexes for `node(file_node_id)`, `node(qualified_name)`, `edge(resolved_source_node_id)`, `edge(resolved_target_node_id)`, and `edge(line)` as needed.
- **Dependencies**: Task 1.2
- **Acceptance Criteria**:
  - `Storage::init` creates tables with new columns.
  - Insert/read functions are updated to handle metadata.
- **Validation**:
  - `cargo test -p codestory-storage`

## Sprint 2: Tree-sitter-graph Rule Loading + Edge Extraction
**Goal**: Load `.scm` files per language, extract edges from graph output, and remove hardcoded Rust queries.
**Demo/Validation**:
- `cargo test -p codestory-index`

### Task 2.1: Load `.scm` rules per language
- **Location**: `crates/codestory-index/src/lib.rs`, `crates/codestory-index/rules/*.scm`
- **Description**: Replace inline graph strings in `get_language_for_ext` with `include_str!` for corresponding `.scm` files. Ensure each rule file defines node creation and edge creation.
- **Dependencies**: Sprint 1 complete
- **Acceptance Criteria**:
  - Graph queries are no longer embedded in Rust.
  - All supported languages have rule files used by the indexer.
- **Validation**:
  - `cargo test -p codestory-index`

### Task 2.2: Extract edges from tree-sitter-graph output
- **Location**: `crates/codestory-index/src/lib.rs`
- **Description**: Iterate graph edges (`graph[node].iter_edges()`) and read edge attributes (e.g., `kind`, `line`) to construct `Edge` entries. Remove `get_relationship_queries` and the current ad-hoc pass.
- **Dependencies**: Task 2.1
- **Acceptance Criteria**:
  - All edges come from `.scm` definitions.
  - No `get_relationship_queries` usage remains.
- **Validation**:
  - Run codestory-index tests; ensure edges are present.

### Task 2.3: Add function range + call-site metadata to `.scm`
- **Location**: `crates/codestory-index/rules/*.scm`
- **Description**: Add attributes on **all nodes** for `start_row`/`end_row` (1-based by adding `+ 1` in Rust) and add call-site `line` attributes. Ensure call edges include `kind = "CALL"` and carry `line` (1-based) attributes.
- **Dependencies**: Task 2.1
- **Acceptance Criteria**:
  - Function nodes carry line range attributes.
  - Call-site edges or nodes carry `line` attribute.
- **Validation**:
  - Add/extend unit tests to verify ranges and call-line metadata exist in extracted nodes/edges.

## Sprint 3: Line-Range Call Attribution + Resolution Pass
**Goal**: Attribute calls to callers by line range and resolve edges post-pass (drift-style).
**Demo/Validation**:
- `cargo test -p codestory-index`

### Task 3.1: Build function range map and call-site list
- **Location**: `crates/codestory-index/src/lib.rs`
- **Description**: Build a `HashMap<NodeId, (file_node_id, start_line, end_line)>` from node metadata and a `Vec<CallSite>` from edge metadata or call nodes.
- **Dependencies**: Sprint 2 complete
- **Acceptance Criteria**:
  - All functions/methods have ranges recorded.
  - Call sites have line numbers.
- **Validation**:
  - Unit test with nested functions to ensure line-range attribution works.

### Task 3.2: Replace parent-walking call attribution
- **Location**: `crates/codestory-index/src/lib.rs`
- **Description**: For each call site, find the enclosing function by `start_line <= line <= end_line` within the same file. Create CALL edge from caller to callee.
- **Dependencies**: Task 3.1
- **Acceptance Criteria**:
  - No tree-walking parent hack remains.
  - CALL edges link to enclosing function by line range.
- **Validation**:
  - Update or add tests to assert correct caller attribution.

### Task 3.3: Implement `ResolutionPass`
- **Location**: `crates/codestory-index/src/resolution.rs`, `crates/codestory-index/src/lib.rs`
- **Description**: Add SQL post-pass similar to drift:
  1) Same file match (highest confidence)
  2) Same module/namespace (medium)
  3) Global unique (medium)
  4) Fuzzy or first-candidate (low)
  Update edge columns `resolved_source_node_id`, `resolved_target_node_id`, and `confidence`.
- **Dependencies**: Task 3.2
- **Acceptance Criteria**:
  - Unresolved calls are stored with confidence scores.
  - SQL pass updates `resolved_target_node_id` for eligible edges.
- **Validation**:
  - Unit test for resolution ordering and confidence assignment.

### Task 3.4: Wire resolution pass into indexing flow
- **Location**: `crates/codestory-index/src/lib.rs`
- **Description**: Run `ResolutionPass` after inserts (or as a configurable optional step) to update edges.
- **Dependencies**: Task 3.3
- **Acceptance Criteria**:
  - Resolution executed after indexing completes.
- **Validation**:
  - Integration test verifies resolved edges for a small multi-file example.

## Sprint 4: Tests + Cleanup
**Goal**: Update tests for canonical IDs, metadata columns, and new call resolution behavior.
**Demo/Validation**:
- `cargo test -p codestory-index`
- `cargo test -p codestory-storage`

### Task 4.1: Update existing integration tests
- **Location**: `crates/codestory-index/tests/integration.rs`
- **Description**: Adjust expectations for CALL attribution, ensure MEMBER edges still exist, and validate new metadata columns are populated.
- **Dependencies**: Sprint 3 complete
- **Acceptance Criteria**:
  - Tests pass and assert resolved calls.
- **Validation**:
  - `cargo test -p codestory-index`

### Task 4.2: Add unit tests for line-range attribution
- **Location**: `crates/codestory-index/src/lib.rs` (tests) or new `*_tests.rs`
- **Description**: Add a Python or JS file with multiple functions, nested blocks, and calls. Verify call attribution uses line ranges, not parent walking.
- **Dependencies**: Task 3.2
- **Acceptance Criteria**:
  - Test fails if caller attribution is wrong.
- **Validation**:
  - `cargo test -p codestory-index`

### Task 4.3: Remove unused code paths
- **Location**: `crates/codestory-index/src/lib.rs`, `crates/codestory-index/src/post_processing.rs`
- **Description**: Remove `get_relationship_queries`, parent-walking call graph logic, and unused post-processing if superseded by `ResolutionPass`.
- **Dependencies**: Sprint 3 complete
- **Acceptance Criteria**:
  - No dead code remains.
- **Validation**:
  - `cargo check`

## Testing Strategy
- Unit tests for line-range attribution and resolution confidence.
- Integration tests for CALL edges and resolved target linking.
- Smoke run: `cargo run -p codestory-gui` and verify graph renders nodes/edges.

## Potential Risks & Gotchas
- NodeId hashing change will invalidate old IDs; full reindex required (acceptable per requirement).
- Missing node ranges in `.scm` rules can break line-range attribution.
- Node/edge metadata columns require updates across all query paths in storage and GUI.
- Resolution pass may resolve to wrong overloads without namespace/file scoping.

## Rollback Plan
- Revert NodeId hashing to name-only and remove metadata columns if needed.
- Restore `get_relationship_queries` + parent-walking logic.
