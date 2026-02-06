# Requirements: Graph Parity (Sourcetrail -> CodeStory)

## Glossary
- **Sourcetrail reference**: `..\Sourcetrail\DOCUMENTATION.md` + images in `..\Sourcetrail\docs\documentation\`.
- **Graph View UI (NodeGraphView)**: `crates/codestory-gui/src/components/node_graph/viewer.rs`
- **Graph Canvas (GraphCanvas)**: `crates/codestory-gui/src/components/node_graph/graph_canvas.rs`
- **Custom Trail Dialog**: `crates/codestory-gui/src/components/custom_trail_dialog.rs`
- **Trail Query Engine**: `crates/codestory-storage/src/lib.rs`

## Requirements
### Requirement 1: Toolbar Placement Parity
#### Acceptance Criteria
1. WHEN Graph View is visible, THEN **Graph View UI (NodeGraphView)** SHALL render a top-left overlay toolbar cluster positioned within the graph viewport matching Sourcetrail placement semantics.
2. WHEN the toolbar is collapsed/expanded, THEN **Graph View UI (NodeGraphView)** SHALL show/hide the custom trail controls via a caret toggle without affecting grouping toggles.
3. WHEN grouping toggles are clicked, THEN **Graph View UI (NodeGraphView)** SHALL toggle grouping-by-namespace and grouping-by-file.

### Requirement 2: Zoom Controls Parity
#### Acceptance Criteria
1. WHEN the user clicks zoom-in/zoom-out, THEN **Graph View UI (NodeGraphView)** SHALL publish zoom events that update the viewport.
2. WHEN the user presses `0`, THEN **Graph View UI (NodeGraphView)** SHALL reset zoom to 100%.
3. WHEN the user presses `Shift+W` / `Shift+S`, THEN **Graph View UI (NodeGraphView)** SHALL zoom in/out.
4. WHEN the user presses `Ctrl/Cmd+Shift+Up` / `Ctrl/Cmd+Shift+Down`, THEN **Graph View UI (NodeGraphView)** SHALL zoom in/out.

### Requirement 3: Panning Parity (WASD)
#### Acceptance Criteria
1. WHEN Graph View is active and keyboard focus is not in a text field, THEN **Graph View UI (NodeGraphView)** SHALL pan using `W/A/S/D`.

### Requirement 4: Graph Wheel Behavior Preference (Sourcetrail “Graph Zoom”)
#### Acceptance Criteria
1. WHEN `GraphWheelBehavior=ScrollPan`, THEN **Graph Canvas (GraphCanvas)** SHALL pan using mouse wheel scroll deltas and Ctrl/Cmd+wheel SHALL still zoom.
2. WHEN `GraphWheelBehavior=Zoom`, THEN **Graph Canvas (GraphCanvas)** SHALL zoom using mouse wheel scroll deltas.
3. WHEN the preference is changed in Preferences, THEN **Settings + Preferences** SHALL persist and apply it.

### Requirement 5: Legend Parity
#### Acceptance Criteria
1. WHEN the user clicks the bottom-right `?` button, THEN **Graph View UI (NodeGraphView)** SHALL toggle the legend overlay.
2. WHEN the user types `legend` into the main search field and submits, THEN **CodeStoryApp** SHALL open the Graph tab and show the legend.

### Requirement 6: Custom Trail Modes Parity
#### Acceptance Criteria
1. WHEN Custom Trail dialog is opened, THEN **Custom Trail Dialog** SHALL provide mode selection: `All Referenced`, `All Referencing`, `To Target Symbol`.
2. WHEN mode is `To Target Symbol`, THEN **Custom Trail Dialog** SHALL require both From and To nodes before enabling Start.
3. WHEN depth is set to `∞`, THEN **Trail Query Engine (Storage)** SHALL treat the query as unbounded depth but bounded by node caps.
4. WHEN layout direction is selected in the dialog, THEN **Custom Trail Dialog** SHALL publish `SetLayoutDirection`.
5. WHEN node/edge filters are selected, THEN **Custom Trail Dialog** SHALL publish them in `TrailConfigChange`.

### Requirement 7: To Target Symbol Query Semantics
#### Acceptance Criteria
1. WHEN `TrailMode=ToTargetSymbol` with From+To and a max depth, THEN **Trail Query Engine (Storage)** SHALL return a subgraph containing nodes/edges on at least one path from From to To within the max depth (or within bounds when infinite depth).
2. WHEN the query would exceed safety caps, THEN **Trail Query Engine (Storage)** SHALL set `TrailResult.truncated=true`.

### Requirement 8: Image Export Parity (partial)
#### Acceptance Criteria
1. WHEN the user exports the graph image, THEN **Graph View UI (NodeGraphView)** SHALL support PNG, JPEG, and BMP formats based on the chosen filename extension.

