import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { api } from "../api/client";
import type {
  EdgeKind,
  LayoutDirection,
  NodeKind,
  SearchHit,
  TrailCallerScope,
  TrailDirection,
  TrailMode,
} from "../generated/api";
import {
  EDGE_KIND_OPTIONS,
  NODE_KIND_OPTIONS,
  TRAIL_PERSPECTIVE_PRESETS,
  type GroupingMode,
  type TrailPerspectivePreset,
  type TrailUiConfig,
  trailConfigFromPerspectivePreset,
  trailPerspectivePresetForConfig,
} from "../graph/trailConfig";

type GraphTrailControlsProps = {
  config: TrailUiConfig;
  projectOpen: boolean;
  projectRevision: number;
  hasRootSymbol: boolean;
  rootSymbolLabel: string | null;
  disabledReason: string | null;
  isRunning: boolean;
  dialogOpen: boolean;
  onDialogOpenChange: (open: boolean) => void;
  onOpenBookmarkManager?: () => void;
  onConfigChange: (patch: Partial<TrailUiConfig>) => void;
  onRunTrail: () => void;
  onResetDefaults: () => void;
};

function titleCase(value: string): string {
  return value
    .toLowerCase()
    .split("_")
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function modeLabel(mode: TrailMode): string {
  switch (mode) {
    case "Neighborhood":
      return "Neighborhood";
    case "AllReferenced":
      return "All Referenced";
    case "AllReferencing":
      return "All Referencing";
    case "ToTargetSymbol":
      return "To Target Symbol";
  }
}

function directionLabel(direction: TrailDirection): string {
  switch (direction) {
    case "Incoming":
      return "Incoming";
    case "Outgoing":
      return "Outgoing";
    case "Both":
      return "Both";
  }
}

function callerScopeLabel(scope: TrailCallerScope): string {
  switch (scope) {
    case "ProductionOnly":
      return "Production Only";
    case "IncludeTestsAndBenches":
      return "Include Tests/Benches";
  }
}

function groupingModeLabel(mode: GroupingMode): string {
  switch (mode) {
    case "none":
      return "No Group";
    case "namespace":
      return "Namespace";
    case "file":
      return "File";
  }
}

function perspectivePresetLabel(preset: TrailPerspectivePreset): string {
  switch (preset) {
    case "Architecture":
      return "Architecture";
    case "CallFlow":
      return "Call Flow";
    case "Impact":
      return "Impact";
    case "Ownership":
      return "Ownership";
  }
}

export function GraphTrailControls({
  config,
  projectOpen,
  projectRevision,
  hasRootSymbol,
  rootSymbolLabel,
  disabledReason,
  isRunning,
  dialogOpen,
  onDialogOpenChange,
  onOpenBookmarkManager,
  onConfigChange,
  onRunTrail,
  onResetDefaults,
}: GraphTrailControlsProps) {
  const [targetQuery, setTargetQuery] = useState<string>(config.targetLabel);
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchOpen, setSearchOpen] = useState<boolean>(false);
  const [searchIndex, setSearchIndex] = useState<number>(0);
  const [searching, setSearching] = useState<boolean>(false);
  const [nodeOptions, setNodeOptions] = useState<NodeKind[]>(NODE_KIND_OPTIONS);
  const [edgeOptions, setEdgeOptions] = useState<EdgeKind[]>(EDGE_KIND_OPTIONS);
  const [showQuickHelp, setShowQuickHelp] = useState<boolean>(false);
  const [advancedFilterQuery, setAdvancedFilterQuery] = useState<string>("");
  const searchSeqRef = useRef<number>(0);
  const isSubmitDisabled = disabledReason !== null || isRunning;

  useEffect(() => {
    if (!projectOpen) {
      setNodeOptions(NODE_KIND_OPTIONS);
      setEdgeOptions(EDGE_KIND_OPTIONS);
      return;
    }
    void api
      .graphTrailFilterOptions()
      .then((options) => {
        if (options.node_kinds.length > 0) {
          setNodeOptions(options.node_kinds);
        }
        if (options.edge_kinds.length > 0) {
          setEdgeOptions(options.edge_kinds);
        }
      })
      .catch(() => {
        setNodeOptions(NODE_KIND_OPTIONS);
        setEdgeOptions(EDGE_KIND_OPTIONS);
      });
  }, [projectOpen, projectRevision]);

  useEffect(() => {
    setTargetQuery(config.targetLabel);
  }, [config.targetLabel]);

  useEffect(() => {
    if (!dialogOpen) {
      setAdvancedFilterQuery("");
    }
  }, [dialogOpen]);

  useEffect(() => {
    if (config.mode !== "ToTargetSymbol") {
      setSearchOpen(false);
      setSearchHits([]);
      setSearchIndex(0);
      setSearching(false);
      return;
    }

    const query = targetQuery.trim();
    if (!projectOpen || query.length < 2) {
      searchSeqRef.current += 1;
      setSearchOpen(false);
      setSearchHits([]);
      setSearchIndex(0);
      setSearching(false);
      return;
    }

    const sequence = searchSeqRef.current + 1;
    searchSeqRef.current = sequence;
    setSearching(true);

    const timer = window.setTimeout(() => {
      void api
        .search({ query })
        .then((hits) => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits(hits.slice(0, 8));
          setSearchOpen(true);
          setSearchIndex(0);
        })
        .catch(() => {
          if (sequence !== searchSeqRef.current) {
            return;
          }
          setSearchHits([]);
          setSearchOpen(false);
        })
        .finally(() => {
          if (sequence === searchSeqRef.current) {
            setSearching(false);
          }
        });
    }, 220);

    return () => {
      window.clearTimeout(timer);
    };
  }, [config.mode, projectOpen, targetQuery]);

  const activeEdgeKinds = useMemo(() => new Set(config.edgeFilter), [config.edgeFilter]);
  const activeNodeKinds = useMemo(() => new Set(config.nodeFilter), [config.nodeFilter]);
  const activePerspectivePreset = useMemo(() => trailPerspectivePresetForConfig(config), [config]);
  const normalizedAdvancedFilter = advancedFilterQuery.trim().toLowerCase();
  const matchesAdvancedFilter = (label: string): boolean =>
    normalizedAdvancedFilter.length === 0 || label.toLowerCase().includes(normalizedAdvancedFilter);

  const filteredNodeOptions = nodeOptions.filter((kind) =>
    matchesAdvancedFilter(`node ${titleCase(kind)}`),
  );
  const filteredEdgeOptions = edgeOptions.filter((kind) =>
    matchesAdvancedFilter(`edge ${titleCase(kind)}`),
  );
  const showTrailMode = matchesAdvancedFilter("trail mode neighborhood target all referenced");
  const showDepth = matchesAdvancedFilter("max depth");
  const showLayoutDirection = matchesAdvancedFilter("layout direction horizontal vertical");
  const showDirection = matchesAdvancedFilter("direction incoming outgoing both");
  const showCallerScope = matchesAdvancedFilter("caller scope tests benches");
  const showGrouping = matchesAdvancedFilter("grouping namespace file");
  const showEdgeBundling = matchesAdvancedFilter("edge bundling bundled separate");
  const showMaxNodes = matchesAdvancedFilter("max nodes limit");
  const showMiniMap = matchesAdvancedFilter("minimap map");
  const showUtilityCalls = matchesAdvancedFilter("utility calls");
  const showNodeFilters = matchesAdvancedFilter("node filters") || filteredNodeOptions.length > 0;
  const showEdgeFilters = matchesAdvancedFilter("edge filters") || filteredEdgeOptions.length > 0;
  const hasAdvancedMatches =
    showTrailMode ||
    showDepth ||
    showLayoutDirection ||
    showDirection ||
    showCallerScope ||
    showGrouping ||
    showEdgeBundling ||
    showMaxNodes ||
    showMiniMap ||
    showUtilityCalls ||
    showNodeFilters ||
    showEdgeFilters;

  const toggleEdgeKind = (kind: EdgeKind) => {
    if (activeEdgeKinds.has(kind)) {
      onConfigChange({ edgeFilter: config.edgeFilter.filter((item) => item !== kind) });
      return;
    }
    onConfigChange({ edgeFilter: [...config.edgeFilter, kind] });
  };

  const toggleNodeKind = (kind: NodeKind) => {
    if (activeNodeKinds.has(kind)) {
      onConfigChange({ nodeFilter: config.nodeFilter.filter((item) => item !== kind) });
      return;
    }
    onConfigChange({ nodeFilter: [...config.nodeFilter, kind] });
  };

  const selectTargetHit = (hit: SearchHit) => {
    setTargetQuery(hit.display_name);
    onConfigChange({
      targetId: hit.node_id,
      targetLabel: hit.display_name,
    });
    setSearchOpen(false);
    setSearchHits([]);
  };

  const handleTargetKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (searchHits.length > 0) {
        setSearchOpen(true);
        setSearchIndex((prev) => Math.min(prev + 1, searchHits.length - 1));
      }
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (searchHits.length > 0) {
        setSearchOpen(true);
        setSearchIndex((prev) => Math.max(prev - 1, 0));
      }
      return;
    }

    if (event.key === "Enter") {
      if (searchHits.length > 0) {
        event.preventDefault();
        const selected = searchHits[Math.min(searchIndex, searchHits.length - 1)] ?? searchHits[0];
        if (selected) {
          selectTargetHit(selected);
        }
      }
      return;
    }

    if (event.key === "Escape") {
      setSearchOpen(false);
    }
  };

  const runAndCloseDialog = () => {
    onRunTrail();
    onDialogOpenChange(false);
  };

  return (
    <div className="graph-trail-controls" aria-label="Trail controls">
      <div className="graph-trail-toolbar">
        <button type="button" onClick={onRunTrail} disabled={isSubmitDisabled}>
          {isRunning ? "Running..." : "Run Trail"}
        </button>
        <div className="trail-mode-row" role="radiogroup" aria-label="Perspective preset">
          {TRAIL_PERSPECTIVE_PRESETS.map((preset) => (
            <button
              key={preset}
              type="button"
              className={
                activePerspectivePreset === preset ? "graph-chip graph-chip-active" : "graph-chip"
              }
              onClick={() => onConfigChange(trailConfigFromPerspectivePreset(preset))}
            >
              {perspectivePresetLabel(preset)}
            </button>
          ))}
        </div>
        <button
          type="button"
          className={config.showLegend ? "graph-chip graph-chip-active" : "graph-chip"}
          onClick={() => onConfigChange({ showLegend: !config.showLegend })}
        >
          Legend
        </button>
        <button
          type="button"
          className={showQuickHelp ? "graph-chip graph-chip-active" : "graph-chip"}
          onClick={() => setShowQuickHelp((previous) => !previous)}
        >
          Help
        </button>
      </div>

      {showQuickHelp ? (
        <div className="graph-trail-hint" role="status">
          Pick a preset, then run trail. Press `Ctrl+U` for advanced settings.
        </div>
      ) : null}
      <button type="button" onClick={() => onDialogOpenChange(true)} disabled={!projectOpen}>
        Advanced Settings
      </button>

      {disabledReason ? <div className="graph-trail-reason">{disabledReason}</div> : null}
      {!projectOpen || !hasRootSymbol ? (
        <div className="graph-trail-hint">Select a symbol, then run trail.</div>
      ) : null}

      {dialogOpen ? (
        <div className="trail-dialog-backdrop" role="presentation">
          <div className="trail-dialog" role="dialog" aria-modal="true" aria-label="Custom trail">
            <div className="trail-dialog-header">
              <h3>Trail Settings</h3>
            </div>

            <div className="trail-dialog-grid">
              <label className="graph-control-field">
                <span>From</span>
                <input
                  value={rootSymbolLabel ?? ""}
                  readOnly
                  placeholder="Start symbol"
                  aria-label="Start symbol"
                />
              </label>

              {config.mode === "ToTargetSymbol" ? (
                <label className="graph-control-field trail-target-field">
                  <span>To</span>
                  <input
                    role="combobox"
                    aria-expanded={searchOpen}
                    aria-controls="trail-target-results"
                    value={targetQuery}
                    placeholder="Target symbol"
                    onChange={(event) => {
                      const nextValue = event.target.value;
                      setTargetQuery(nextValue);
                      onConfigChange({
                        targetId: null,
                        targetLabel: nextValue,
                      });
                    }}
                    onFocus={() => {
                      if (searchHits.length > 0) {
                        setSearchOpen(true);
                      }
                    }}
                    onBlur={() => {
                      window.setTimeout(() => setSearchOpen(false), 120);
                    }}
                    onKeyDown={handleTargetKeyDown}
                  />
                  {searching ? <span className="trail-target-state">Searching...</span> : null}
                  {searchOpen && searchHits.length > 0 ? (
                    <div id="trail-target-results" className="search-dropdown" role="listbox">
                      {searchHits.map((hit, idx) => (
                        <button
                          key={`${hit.node_id}-${hit.score}`}
                          className={
                            idx === searchIndex ? "search-hit search-hit-active" : "search-hit"
                          }
                          onMouseEnter={() => setSearchIndex(idx)}
                          onClick={() => selectTargetHit(hit)}
                        >
                          <span className="search-hit-name">{hit.display_name}</span>
                          <span className="search-hit-kind">{hit.kind}</span>
                        </button>
                      ))}
                    </div>
                  ) : null}
                </label>
              ) : (
                <div className="trail-target-placeholder">
                  <span>To</span>
                  <div className="trail-target-placeholder-value">None</div>
                </div>
              )}
            </div>

            <label className="graph-control-field">
              <span>Find advanced option</span>
              <input
                value={advancedFilterQuery}
                onChange={(event) => setAdvancedFilterQuery(event.target.value)}
                placeholder="Search settings"
                aria-label="Find advanced option"
              />
            </label>

            {showTrailMode ? (
              <div className="trail-mode-row" role="radiogroup" aria-label="Trail mode">
                {(
                  [
                    "Neighborhood",
                    "ToTargetSymbol",
                    "AllReferenced",
                    "AllReferencing",
                  ] as TrailMode[]
                ).map((mode) => (
                  <label key={mode} className="trail-mode-option">
                    <input
                      type="radio"
                      checked={config.mode === mode}
                      onChange={() =>
                        onConfigChange({
                          mode,
                          ...(mode !== "ToTargetSymbol" ? { targetId: null, targetLabel: "" } : {}),
                        })
                      }
                    />
                    <span>{modeLabel(mode)}</span>
                  </label>
                ))}
              </div>
            ) : null}

            <div className="trail-dialog-grid trail-dialog-grid-secondary">
              {showDepth ? (
                <label className="graph-control-field">
                  <span>Max Depth</span>
                  <input
                    type="range"
                    min={0}
                    max={12}
                    value={Math.max(0, Math.min(12, config.depth))}
                    onChange={(event) => {
                      const depth = Number(event.target.value);
                      onConfigChange({ depth: Number.isFinite(depth) ? depth : 1 });
                    }}
                  />
                  <span className="trail-range-value">
                    {config.depth === 0 ? "0 (infinite)" : String(config.depth)}
                  </span>
                </label>
              ) : null}

              {showLayoutDirection ? (
                <div className="graph-control-field">
                  <span>Layout Direction</span>
                  <div
                    className="trail-inline-options"
                    role="radiogroup"
                    aria-label="Layout direction"
                  >
                    {(["Horizontal", "Vertical"] as LayoutDirection[]).map((direction) => (
                      <label key={direction} className="trail-mode-option">
                        <input
                          type="radio"
                          checked={config.layoutDirection === direction}
                          onChange={() => onConfigChange({ layoutDirection: direction })}
                        />
                        <span>{direction}</span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : null}

              {showDirection ? (
                <label className="graph-control-field">
                  <span>Direction</span>
                  <select
                    value={config.direction}
                    onChange={(event) =>
                      onConfigChange({ direction: event.target.value as TrailDirection })
                    }
                  >
                    {(["Incoming", "Outgoing", "Both"] as TrailDirection[]).map((direction) => (
                      <option key={direction} value={direction}>
                        {directionLabel(direction)}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}

              {showCallerScope ? (
                <label className="graph-control-field">
                  <span>Caller Scope</span>
                  <select
                    value={config.callerScope}
                    onChange={(event) =>
                      onConfigChange({ callerScope: event.target.value as TrailCallerScope })
                    }
                  >
                    {(["ProductionOnly", "IncludeTestsAndBenches"] as TrailCallerScope[]).map(
                      (scope) => (
                        <option key={scope} value={scope}>
                          {callerScopeLabel(scope)}
                        </option>
                      ),
                    )}
                  </select>
                </label>
              ) : null}

              {showGrouping ? (
                <label className="graph-control-field">
                  <span>Grouping</span>
                  <select
                    value={config.groupingMode}
                    onChange={(event) =>
                      onConfigChange({ groupingMode: event.target.value as GroupingMode })
                    }
                  >
                    {(["none", "namespace", "file"] as GroupingMode[]).map((mode) => (
                      <option key={mode} value={mode}>
                        {groupingModeLabel(mode)}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}

              {showEdgeBundling ? (
                <label className="graph-control-field">
                  <span>Edge Bundling</span>
                  <select
                    value={config.bundleEdges ? "bundled" : "separate"}
                    onChange={(event) =>
                      onConfigChange({ bundleEdges: event.target.value === "bundled" })
                    }
                  >
                    <option value="bundled">Bundled</option>
                    <option value="separate">Separate</option>
                  </select>
                </label>
              ) : null}

              {showMaxNodes ? (
                <label className="graph-control-field">
                  <span>Max Nodes</span>
                  <input
                    type="number"
                    min={10}
                    max={100000}
                    value={config.maxNodes}
                    onChange={(event) => {
                      const parsed = Number(event.target.value);
                      onConfigChange({
                        maxNodes: Number.isFinite(parsed)
                          ? Math.max(10, Math.min(100000, parsed))
                          : 500,
                      });
                    }}
                  />
                </label>
              ) : null}

              {showMiniMap ? (
                <label className="graph-control-field">
                  <span>MiniMap</span>
                  <select
                    value={config.showMiniMap ? "show" : "hide"}
                    onChange={(event) =>
                      onConfigChange({ showMiniMap: event.target.value === "show" })
                    }
                  >
                    <option value="show">Show</option>
                    <option value="hide">Hide</option>
                  </select>
                </label>
              ) : null}

              {showUtilityCalls ? (
                <label className="graph-control-field">
                  <span>Utility Calls</span>
                  <select
                    value={config.showUtilityCalls ? "show" : "hide"}
                    onChange={(event) =>
                      onConfigChange({ showUtilityCalls: event.target.value === "show" })
                    }
                  >
                    <option value="show">Show</option>
                    <option value="hide">Hide</option>
                  </select>
                </label>
              ) : null}
            </div>

            {showNodeFilters || showEdgeFilters ? (
              <div className="trail-filter-columns">
                {showNodeFilters ? (
                  <div className="trail-filter-column">
                    <div className="trail-filter-header">
                      <strong>Nodes</strong>
                      <div className="trail-filter-actions">
                        <button
                          type="button"
                          onClick={() => onConfigChange({ nodeFilter: [...nodeOptions] })}
                        >
                          Check All
                        </button>
                        <button type="button" onClick={() => onConfigChange({ nodeFilter: [] })}>
                          Uncheck All
                        </button>
                      </div>
                    </div>
                    <div className="trail-filter-list">
                      {filteredNodeOptions.map((kind) => (
                        <label key={kind} className="trail-filter-item">
                          <input
                            type="checkbox"
                            checked={activeNodeKinds.has(kind)}
                            onChange={() => toggleNodeKind(kind)}
                          />
                          <span>{titleCase(kind)}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                ) : null}

                {showEdgeFilters ? (
                  <div className="trail-filter-column">
                    <div className="trail-filter-header">
                      <strong>Edges</strong>
                      <div className="trail-filter-actions">
                        <button
                          type="button"
                          onClick={() => onConfigChange({ edgeFilter: [...edgeOptions] })}
                        >
                          Check All
                        </button>
                        <button type="button" onClick={() => onConfigChange({ edgeFilter: [] })}>
                          Uncheck All
                        </button>
                      </div>
                    </div>
                    <div className="trail-filter-list">
                      {filteredEdgeOptions.map((kind) => (
                        <label key={kind} className="trail-filter-item">
                          <input
                            type="checkbox"
                            checked={activeEdgeKinds.has(kind)}
                            onChange={() => toggleEdgeKind(kind)}
                          />
                          <span>{titleCase(kind)}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                ) : null}
              </div>
            ) : null}

            {!hasAdvancedMatches ? (
              <div className="graph-trail-hint" role="status">
                No match for "{advancedFilterQuery.trim()}".
              </div>
            ) : null}

            <div className="trail-dialog-actions">
              {onOpenBookmarkManager ? (
                <button type="button" onClick={onOpenBookmarkManager}>
                  Bookmarks
                </button>
              ) : null}
              <button type="button" onClick={() => onDialogOpenChange(false)}>
                Cancel
              </button>
              <button type="button" onClick={onResetDefaults}>
                Reset
              </button>
              <button type="button" disabled={isSubmitDisabled} onClick={runAndCloseDialog}>
                Run Trail
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
