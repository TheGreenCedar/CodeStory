# Design Document

## Overview
The implementation extends the existing React + TypeScript UI to expose trail-query controls on top of already available backend APIs (`/api/graph/trail`). The design minimizes risk by keeping current graph rendering primitives and adding a small orchestration layer for trail state, request execution, and legend metadata.

## Design Principles
- Preserve existing symbol-focus and neighborhood workflows.
- Reuse existing DTOs and API routes; avoid backend contract changes.
- Keep controls keyboard-accessible and status-transparent.
- Keep graph rendering deterministic (same config => same payload => stable tab behavior).

## Data Model
```ts
// Implements Req 1.1, 2.1, 5.1, 6.1
export type TrailUiConfig = {
  mode: "Neighborhood" | "AllReferenced" | "AllReferencing" | "ToTargetSymbol";
  targetId: string | null;
  targetLabel: string;
  depth: number;        // 0 means infinite
  direction: "Incoming" | "Outgoing" | "Both";
  edgeFilter: string[]; // EdgeKind[] values
  nodeFilter: string[]; // NodeKind[] values
  maxNodes: number;
};
```

## API Mapping
| UI Field | `TrailConfigDto` Field | Rule |
|---|---|---|
| `activeSymbolId` | `root_id` | Required for submit |
| `mode` | `mode` | Direct enum mapping |
| `targetId` | `target_id` | Required only for `ToTargetSymbol`, else `null` |
| `depth` | `depth` | `0` allowed for infinite |
| `direction` | `direction` | Direct enum mapping |
| `edgeFilter` | `edge_filter` | Must be non-empty for submit |
| `nodeFilter` | `node_filter` | Optional/empty allowed |
| `maxNodes` | `max_nodes` | Clamp in UI (10-100000) to match backend |

## Component Specifications

#### Component: GraphTrailControls
**Purpose**: Capture and validate trail inputs.
**Location**: `codestory-ui/src/components/GraphTrailControls.tsx`
**Interface**:
```ts
// Implements Req 1.1, 1.2, 1.3, 5.2, 7.3
type GraphTrailControlsProps = {
  config: TrailUiConfig;
  disabledReason: string | null;
  loading: boolean;
  availableEdgeKinds: string[];
  onConfigChange: (patch: Partial<TrailUiConfig>) => void;
  onRunTrail: () => void;
  onResetDefaults: () => void;
};
```

#### Component: TrailQueryController
**Purpose**: Own trail config state, execute trail requests, and create/update graph artifacts.
**Location**: `codestory-ui/src/App.tsx` (initial), optional extraction to `codestory-ui/src/graph/useTrailQueryController.ts`
**Interface**:
```ts
// Implements Req 2.1, 2.2, 2.3, 4.1, 5.1, 6.1, 6.2, 6.3, 7.1, 7.2
async function runTrail(config: TrailUiConfig, rootId: string): Promise<void>;
function toTrailConfigDto(config: TrailUiConfig, rootId: string): TrailConfigDto;
function makeTrailGraphId(rootId: string): string; // e.g. trail-${rootId}-${timestamp}
```

#### Component: TrailTargetSearchCombobox
**Purpose**: Provide ARIA-conformant symbol search for target selection.
**Location**: `codestory-ui/src/components/TrailTargetSearchCombobox.tsx`
**Interface**:
```ts
// Implements Req 3.1, 3.2, 3.3
type TrailTargetSearchComboboxProps = {
  query: string;
  selectedTargetId: string | null;
  selectedTargetLabel: string;
  disabled: boolean;
  onSelectTarget: (id: string, label: string) => void;
  onClearTarget: () => void;
};
```

#### Component: GraphViewportRenderer
**Purpose**: Render trail graphs and compute legend metadata from rendered edges.
**Location**: `codestory-ui/src/graph/GraphViewport.tsx`
**Interface**:
```ts
// Implements Req 4.1, 4.2, 4.3
type LegendRow = {
  edgeKind: string;
  color: string;
  usesDash: boolean;
  usesOpacityCue: boolean;
  visibleCount: number;
};

function buildLegendRows(graph: GraphResponse): LegendRow[];
```

#### Component: GraphLegendPanel
**Purpose**: Display edge visual semantics and active counts in an on-canvas overlay.
**Location**: `codestory-ui/src/graph/GraphLegendPanel.tsx`
**Interface**:
```ts
// Implements Req 4.2, 4.3
type GraphLegendPanelProps = {
  rows: LegendRow[];
  graphTitle: string;
};
```

## Interaction Flow
1. User focuses a root symbol from explorer/search (existing behavior).
2. User modifies trail controls.
3. `TrailQueryController` validates config and calls `api.graphTrail`.
4. On success, result is inserted in `graphMap`/`graphOrder` and activated as the current graph.
5. `GraphViewportRenderer` renders graph and emits legend rows to `GraphLegendPanel`.
6. Trail config is persisted through existing UI layout save path.

## Error and Loading States
- Disable submit if root symbol missing, target missing for `ToTargetSymbol`, or `edgeFilter` empty.
- Show in-control loading state during request; ignore duplicate clicks.
- Preserve currently active graph on request failure and emit status message.

## Styling and Layout
- Extend `codestory-ui/src/styles.css` with `.graph-trail-controls*` and `.graph-legend*` classes.
- Keep current mobile behavior by stacking controls under `@media (max-width: 1200px)`.
- Render legend in a `Panel` anchored top-right to avoid overlap with default React Flow controls.

## Test Design
- Unit tests for payload construction (`toTrailConfigDto`) and validation gates.
- Component tests for combobox keyboard interaction and disabled states.
- Integration test for run-trail success/failure paths and graph tab insertion.
