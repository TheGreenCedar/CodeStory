# Design: Graph Parity (Sourcetrail -> CodeStory)

## Overview
This design maps each parity requirement to concrete code locations and implementation notes. The approach is to keep CodeStory’s event-driven architecture intact (UI publishes events, app/controller updates settings + reloads from storage).

## Component Specifications

### Component: Graph View UI (NodeGraphView)
**Purpose**: Render graph viewport overlays and translate actions into events.  
**Location**: `crates/codestory-gui/src/components/node_graph/viewer.rs`

**Key behaviors**
- Top-left overlay toolbar rendered in an `egui::Area` (foreground), styled to match Sourcetrail.
- New: custom trail controls can be collapsed/expanded via caret toggle (`trail_toolbar_expanded`).
- Keyboard shortcuts:
  - `0` -> `Event::ZoomReset`
  - `Shift+W/S` -> `Event::ZoomIn/ZoomOut`
  - `Ctrl/Cmd+Shift+Up/Down` -> `Event::ZoomIn/ZoomOut`
  - `W/A/S/D` continuous panning (calls into `GraphCanvas::pan_by`)
- Legend:
  - `?` button toggles via `Event::SetShowLegend(bool)`
  - `Esc` closes legend when open

**Implements**
- Req 1.1-1.3, Req 2.1-2.4, Req 3.1, Req 5.1, Req 8.1

### Component: Graph Canvas (GraphCanvas)
**Purpose**: Draw nodes/edges and manage pan/zoom interactions.  
**Location**: `crates/codestory-gui/src/components/node_graph/graph_canvas.rs`

**Key behaviors**
- Mouse wheel behavior preference:
  - When `GraphWheelBehavior::ScrollPan`, consume `InputState.smooth_scroll_delta` and apply to `pan`.
  - When `GraphWheelBehavior::Zoom`, convert scroll delta to exponential zoom factor and zoom about pointer.
  - Always still supports Ctrl/Cmd+wheel zoom via `InputState.zoom_delta()`.
- Maintains `pan` and `zoom` synchronized with `GraphViewState`.

**Implements**
- Req 4.1-4.2

### Component: Custom Trail Dialog
**Purpose**: Collect custom trail intent and publish trail events.  
**Location**: `crates/codestory-gui/src/components/custom_trail_dialog.rs`

**Key behaviors**
- Modes:
  - All Referenced (outgoing)
  - All Referencing (incoming)
  - To Target Symbol (requires From+To)
- Depth supports `∞` (stored as `depth=0`).
- Layout direction publishes `Event::SetLayoutDirection(LayoutDirection)`.
- Publishes `Event::TrailConfigChange` including:
  - `mode`, `target_id`, `node_filter`, `edge_filter`, `depth`, `direction`

**Implements**
- Req 6.1-6.5

### Component: Trail Query Engine (Storage)
**Purpose**: Execute trail queries against SQLite storage.  
**Location**: `crates/codestory-storage/src/lib.rs`

**Key behaviors**
- `Storage::get_trail` dispatches on `TrailConfig.mode`:
  - Neighborhood / AllReferenced / AllReferencing -> BFS exploration
  - ToTargetSymbol -> two-sided distance discovery + subgraph construction
- ToTargetSymbol returns endpoints even when no path exists (bounded; may set truncated)
- Optional `TrailConfig.node_filter` filters final results while preserving endpoints.

**Implements**
- Req 6.3, Req 7.1-7.2

### Component: Settings + Preferences
**Purpose**: Persist and expose graph interaction preferences.  
**Locations**
- `crates/codestory-gui/src/settings.rs`
- `crates/codestory-gui/src/components/preferences.rs`

**Key behaviors**
- New `GraphWheelBehavior` persisted under `NodeGraphSettings.graph_wheel_behavior`.

**Implements**
- Req 4.3

### Component: CodeStoryApp Legend Keyword Trigger
**Purpose**: Map Sourcetrail “legend” keyword to the legend overlay.  
**Location**: `crates/codestory-gui/src/app.rs`

**Behavior**
- On `SearchAction::FullSearch("legend")`, focus Graph tab and publish `Event::SetShowLegend(true)`.

**Implements**
- Req 5.2

