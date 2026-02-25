# Implementation Plan

- [x] 1. Build reusable trail control state model and defaults in the UI layer
  - [x] 1.1 Add `TrailUiConfig` type and default factory aligned to core defaults.
  - [x] 1.2 Add validation helpers for required root symbol, required target in `ToTargetSymbol`, and non-empty edge filter.
  - [x] 1.3 Add DTO mapping helper for `TrailUiConfig -> TrailConfigDto`.
  - _Requirements: 1.1, 1.2, 1.3, 2.1, 2.2, 5.2, 6.3_

- [x] 2. Implement GraphTrailControls component and wire control callbacks
  - [x] 2.1 Create mode/depth/direction/max-nodes controls with accessible labels.
  - [x] 2.2 Add edge-kind multi-select chips and optional node-kind filter section.
  - [x] 2.3 Add submit/reset buttons with loading and disabled reason rendering.
  - _Requirements: 1.1, 1.2, 1.3, 5.1, 5.2, 7.3_

- [x] 3. Implement TrailTargetSearchCombobox with keyboard support
  - [x] 3.1 Debounce `/api/search` calls and render result popup.
  - [x] 3.2 Implement Arrow Up/Down, Enter, and Escape behavior.
  - [x] 3.3 Emit `target_id` and `target_label` on commit and support clear action.
  - _Requirements: 3.1, 3.2, 3.3_

- [x] 4. Extend App-level orchestration for trail execution
  - [x] 4.1 Add `runTrail` workflow in `App.tsx` and call `api.graphTrail`.
  - [x] 4.2 Create trail graph artifact IDs/titles and insert into graph tabs.
  - [x] 4.3 Keep previous graph active on errors and emit status updates.
  - _Requirements: 2.1, 2.2, 2.3, 4.1, 7.1, 7.2_

- [x] 5. Add legend generation and rendering in graph viewport
  - [x] 5.1 Build `buildLegendRows` from rendered edge data and style cues.
  - [x] 5.2 Add `GraphLegendPanel` overlay with color/style/count rows.
  - [x] 5.3 Document uncertain/probable semantics in legend copy.
  - _Requirements: 4.2, 4.3_

- [x] 6. Integrate filtering behavior end-to-end
  - [x] 6.1 Ensure selected edge kinds are submitted in `edge_filter`.
  - [x] 6.2 Block execution and show validation when edge filter is empty.
  - [x] 6.3 Submit optional node kind selection in `node_filter`.
  - _Requirements: 5.1, 5.2, 5.3_

- [x] 7. Persist and restore trail preferences with UI layout
  - [x] 7.1 Extend persisted layout schema with trail control preferences.
  - [x] 7.2 Save trail config alongside existing layout saves.
  - [x] 7.3 Restore trail config on project open and apply defaults if absent.
  - _Requirements: 6.1, 6.2, 6.3_

- [ ] 8. Verify behavior with automated and manual tests
  - [ ] 8.1 Add unit tests for payload mapping and validation.
  - [ ] 8.2 Add component tests for combobox keyboard handling and disabled states.
  - [ ] 8.3 Perform manual QA passes for trail mode changes, truncation handling, and duplicate-submit protection.
  - _Requirements: 3.2, 7.1, 7.2, 7.3_
