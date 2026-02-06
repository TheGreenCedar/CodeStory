# Graph Parity Runbook (Sourcetrail vs CodeStory)

This runbook documents a repeatable process for checking CodeStory's graph UI/UX against Sourcetrail's reference docs and screenshots.

## Reference Baseline (Sourcetrail)

Use these as the canonical UI reference images:

- `..\Sourcetrail\docs\documentation\graph_view.png`
- `..\Sourcetrail\docs\documentation\graph_view_graph.png`
- `..\Sourcetrail\docs\documentation\grouping_buttons.png`
- `..\Sourcetrail\docs\documentation\call_graph.png`
- `..\Sourcetrail\docs\documentation\custom_trail.png`
- `..\Sourcetrail\docs\documentation\graph_legend.png`

See the written spec for behavior and shortcuts:

- `..\Sourcetrail\DOCUMENTATION.md` (Graph View, Custom Trail, Graph Legend, zoom/export)

## Standardize The Test Setup

1. Run CodeStory:
   - `cargo run -p codestory-gui`
2. Use a consistent window size and position for screenshots (example: `1280x720`).
3. Use a consistent theme and UI scale (keep defaults unless comparing theme parity).
4. Ensure you are on the **Graph** tab and the graph has a known active symbol.

## Parity Checklist (Workflow Parity)

### Controls Placement
- Top-left trail toolbar is visible in the graph viewport.
- Bottom-left zoom cluster is visible and shows a zoom percentage.
- Bottom-right `?` button toggles the legend overlay.

### Core Interactions
- Back/Forward works from the graph toolbar.
- Custom Trail toolbar can be collapsed/expanded via caret toggle.
- Depth control supports `1..=20` and `∞` (infinite until node cap).
- Trail direction toggles (Outgoing / Both / Incoming) change the displayed subgraph.
- Presets (All / Call / Inh / Inc) change the displayed subgraph (edge filter).
- Zoom in/out, zoom-to-fit, and reset zoom (`0`) work.
- Keyboard: `Shift+W/S` zoom, `W/A/S/D` pan.
- Mouse wheel behavior matches Sourcetrail “Graph Zoom” preference:
  - Default: wheel pans, Ctrl/Cmd+wheel zooms.
  - Optional: wheel zooms (preference).
- Export: graph image export supports PNG/JPEG/BMP by file extension.

### Overlays
- Legend opens/closes predictably and does not overlap controls badly (especially with minimap on).
- Minimap toggle still works (left rail).
- Search keyword: typing `legend` in the main search field opens the graph legend.

### Custom Trail Dialog
- Modes are present: `All Referenced`, `All Referencing`, `To Target Symbol`.
- `To Target Symbol` requires both From + To selections.
- Layout direction (Horizontal/Vertical) is settable and affects the graph layout.

## Automated Capture (Codex / Windows UI Automation)

When running under Codex, use the `windows-native-ui-automation` workflow:

1. Snapshot the desktop to find the CodeStory window handle and coordinates:
   - `mcp__windows-mcp__Snapshot` with `use_vision=true`
2. Bring CodeStory to foreground and standardize window size:
   - `mcp__windows-mcp__App` with `mode=switch`
   - `mcp__windows-mcp__App` with `mode=resize`
3. Re-snapshot after resizing.
4. Click toolbar controls by coordinates to set a known state.
5. Capture a screenshot for comparison:
   - Prefer UIA screenshot: `mcp__uiautomation__take_screenshot`
   - Or use the repo-local capture script:
     - `.\scripts\graph_parity\capture_window.ps1`

## Image Diff (Optional)

Once you have a CodeStory capture, generate a heatmap diff against the Sourcetrail baseline:

```powershell
powershell -File .\\scripts\\graph_parity\\diff_images.ps1 `
  -ReferencePath ..\\Sourcetrail\\docs\\documentation\\graph_view.png `
  -CandidatePath .\\artifacts\\graph_parity\\candidate.png `
  -OutPath .\\artifacts\\graph_parity\\diff.png
```

## Repeatable Capture + Diff (Recommended)

Use the helper script to produce:
- a window capture
- standardized crops for top-left controls, depth slider, and grouping pill
- heatmap diffs + metrics text report

```powershell
powershell -File .\\scripts\\graph_parity\\capture_and_diff.ps1 -WindowHandle 123456
```

If you don't have a window handle, omit it and the script will match by `-Title` (less reliable):

```powershell
powershell -File .\\scripts\\graph_parity\\capture_and_diff.ps1 -Title "CodeStory"
```

## Recording Deltas

When a mismatch is found, record:

- **Category**: placement / control presence / shortcut / behavior / performance
- **Expected**: Sourcetrail reference (image + section in `DOCUMENTATION.md`)
- **Observed**: CodeStory state + screenshot name
- **Notes**: any constraints (egui limitations, performance caps)
