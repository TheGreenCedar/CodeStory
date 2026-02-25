# Requirements Document

## Introduction
This document defines the functional and interaction requirements for adding trail exploration controls to the Graph Workspace UI.

## Glossary
- **Trail Graph**: A graph returned from `/api/graph/trail` based on a root symbol and traversal config.
- **Control Surface**: The UI region containing mode, depth, direction, filter, and submit controls.
- **Target Symbol**: The destination node used when mode is `ToTargetSymbol`.

## Requirements

### Requirement 1: Trail Control Surface
#### Acceptance Criteria
1. WHEN a UML graph tab is active, THE **GraphTrailControls** SHALL render controls for trail mode, depth, direction, max nodes, and edge-kind filtering.
2. WHEN trail mode is set to `ToTargetSymbol`, THE **GraphTrailControls** SHALL require a target symbol before enabling trail execution.
3. WHEN no project is open or no root symbol is selected, THE **GraphTrailControls** SHALL disable trail execution and display a clear reason.

### Requirement 2: Trail Payload Construction
#### Acceptance Criteria
1. WHEN the user executes a trail query, THE **TrailQueryController** SHALL send `root_id`, `mode`, `target_id`, `depth`, `direction`, `edge_filter`, `node_filter`, and `max_nodes` in the `TrailConfigDto` payload.
2. WHEN mode is not `ToTargetSymbol`, THE **TrailQueryController** SHALL send `target_id` as `null`.
3. WHEN trail mode is `Neighborhood`, `AllReferenced`, or `AllReferencing`, THE **TrailQueryController** SHALL still allow explicit direction selection but preserve backend authority for mode-imposed direction semantics.

### Requirement 3: Trail Search Interaction
#### Acceptance Criteria
1. WHEN the target search query length is at least two characters, THE **TrailTargetSearchCombobox** SHALL query `/api/search` with debounce and display the top results.
2. WHEN the combobox popup is open, THE **TrailTargetSearchCombobox** SHALL support Arrow Up/Down selection, Enter commit, and Escape dismissal.
3. WHEN a search result is selected, THE **TrailTargetSearchCombobox** SHALL propagate both `target_id` and `target_label` to the **TrailQueryController**.

### Requirement 4: Graph Rendering and Legend
#### Acceptance Criteria
1. WHEN a trail response is returned, THE **GraphViewportRenderer** SHALL render it as an activatable graph tab without breaking existing neighborhood/agent graphs.
2. WHEN a trail graph is rendered, THE **GraphLegendPanel** SHALL display edge kind, stroke color/style cue, and visible edge count for each legend row.
3. WHEN uncertain/probable edges are present, THE **GraphLegendPanel** SHALL explain dashed/opacity semantics used by the renderer.

### Requirement 5: Filtering Behavior
#### Acceptance Criteria
1. WHEN edge-kind filters are changed and a trail is rerun, THE **TrailQueryController** SHALL submit only selected kinds in `edge_filter`.
2. WHEN no edge kind is selected, THE **GraphTrailControls** SHALL block execution and show validation feedback.
3. WHEN node-kind filtering is enabled by the user, THE **TrailQueryController** SHALL include selected node kinds in `node_filter`.

### Requirement 6: State Persistence and Defaults
#### Acceptance Criteria
1. WHEN trail controls change, THE **TrailQueryController** SHALL persist trail UI preferences in saved UI layout.
2. WHEN the project is reopened, THE **TrailQueryController** SHALL restore persisted trail UI preferences.
3. WHEN no saved preference exists, THE **TrailQueryController** SHALL initialize defaults aligned with core trail defaults (mode `Neighborhood`, depth `2`, direction `Both`, empty filters, max nodes `500`).

### Requirement 7: Error and Status Handling
#### Acceptance Criteria
1. WHEN `/api/graph/trail` fails, THE **TrailQueryController** SHALL surface a user-visible status message and keep prior graph state intact.
2. WHEN a trail response is truncated, THE **GraphTrailControls** or parent pane SHALL display the existing truncation indicator for that active graph.
3. WHEN a trail request is in flight, THE **GraphTrailControls** SHALL provide a loading state and prevent duplicate submissions.
