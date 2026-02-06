# Implementation Plan: Graph Parity (Sourcetrail -> CodeStory)

## Completed (Implemented)
- [x] 1. Add trail mode + target support in core config
  - [x] 1.1 Add `TrailMode` enum
  - [x] 1.2 Extend `TrailConfig` with `mode`, `target_id`, `node_filter` + serde defaults
  - _Requirements: 6.1, 7.1_

- [x] 2. Extend trail events to carry mode/target/filters
  - [x] 2.1 Extend `Event::TrailConfigChange` with `mode`, `target_id`, `node_filter`
  - _Requirements: 6.5_

- [x] 3. Implement `ToTargetSymbol` in storage
  - [x] 3.1 Add `Storage::get_trail_to_target` and distance discovery
  - [x] 3.2 Add node filter application on `TrailResult`
  - [x] 3.3 Add tests for ToTargetSymbol behavior
  - _Requirements: 7.1, 7.2_

- [x] 4. Update Custom Trail dialog to match Sourcetrail modes
  - [x] 4.1 Add mode selection and From/To requirements
  - [x] 4.2 Add infinite depth (`∞` => `depth=0`)
  - [x] 4.3 Publish layout direction change
  - _Requirements: 6.1-6.5_

- [x] 5. Implement Graph Zoom preference equivalent
  - [x] 5.1 Add `GraphWheelBehavior` to `NodeGraphSettings`
  - [x] 5.2 Wire to Preferences UI
  - [x] 5.3 Apply behavior in `GraphCanvas` (wheel pan vs wheel zoom)
  - _Requirements: 4.1-4.3_

- [x] 6. Implement keyboard parity shortcuts
  - [x] 6.1 `Shift+W/S` zoom, `Ctrl/Cmd+Shift+Up/Down` zoom
  - [x] 6.2 WASD pan (continuous)
  - _Requirements: 2.3, 2.4, 3.1_

- [x] 7. Implement “legend” keyword trigger
  - [x] 7.1 Intercept `legend` in main search and open legend on Graph tab
  - _Requirements: 5.2_

- [x] 8. Extend image export formats (partial parity)
  - [x] 8.1 Support PNG/JPEG/BMP based on extension
  - _Requirements: 8.1_

## Remaining (Parity Iteration + Automation)
- [x] 9. Pixel-precise toolbar styling + spacing
  - [x] 9.1 Compare against `..\Sourcetrail\docs\documentation\graph_view.png` and `grouping_buttons.png`
  - [x] 9.2 Adjust button sizes/margins/frames to match
  - _Requirements: 1.1-1.3_

- [x] 10. Visual parity capture loop (Windows UI Automation)
  - [x] 10.1 Standardize window size/theme/zoom for captures
  - [x] 10.2 Capture CodeStory screenshots via `windows-native-ui-automation`
  - [x] 10.3 Diff against Sourcetrail reference images; record deltas
  - _Requirements: 1.x, 2.x, 4.x, 5.x, 6.x_
