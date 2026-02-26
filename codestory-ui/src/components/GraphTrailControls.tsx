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
  type GroupingMode,
  type TrailUiConfig,
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
        <button type="button" onClick={() => onDialogOpenChange(true)} disabled={!projectOpen}>
          Custom Trail
        </button>
        <button type="button" onClick={onRunTrail} disabled={isSubmitDisabled}>
          {isRunning ? "Running..." : "Run Trail"}
        </button>
        <button
          type="button"
          className={config.showLegend ? "graph-chip graph-chip-active" : "graph-chip"}
          onClick={() => onConfigChange({ showLegend: !config.showLegend })}
        >
          Legend
        </button>
        <button
          type="button"
          className={config.showMiniMap ? "graph-chip graph-chip-active" : "graph-chip"}
          onClick={() => onConfigChange({ showMiniMap: !config.showMiniMap })}
        >
          MiniMap
        </button>
        <button
          type="button"
          className={config.showUtilityCalls ? "graph-chip graph-chip-active" : "graph-chip"}
          onClick={() => onConfigChange({ showUtilityCalls: !config.showUtilityCalls })}
        >
          Utility Calls
        </button>
        {onOpenBookmarkManager ? (
          <button type="button" onClick={onOpenBookmarkManager}>
            Bookmarks
          </button>
        ) : null}
      </div>

      {disabledReason ? <div className="graph-trail-reason">{disabledReason}</div> : null}
      {!projectOpen || !hasRootSymbol ? (
        <div className="graph-trail-hint">Select a symbol to use as the trail root.</div>
      ) : null}

      {dialogOpen ? (
        <div className="trail-dialog-backdrop" role="presentation">
          <div className="trail-dialog" role="dialog" aria-modal="true" aria-label="Custom trail">
            <div className="trail-dialog-header">
              <h3>Custom Trail</h3>
            </div>

            <div className="trail-dialog-grid">
              <label className="graph-control-field">
                <span>From</span>
                <input
                  value={rootSymbolLabel ?? ""}
                  readOnly
                  placeholder="Start Symbol"
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
                    placeholder="Target Symbol"
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

            <div className="trail-mode-row" role="radiogroup" aria-label="Trail mode">
              {(
                ["Neighborhood", "ToTargetSymbol", "AllReferenced", "AllReferencing"] as TrailMode[]
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

            <div className="trail-dialog-grid trail-dialog-grid-secondary">
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
            </div>

            <div className="trail-filter-columns">
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
                  {nodeOptions.map((kind) => (
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
                  {edgeOptions.map((kind) => (
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
            </div>

            <div className="trail-dialog-actions">
              <button type="button" onClick={() => onDialogOpenChange(false)}>
                Cancel
              </button>
              <button type="button" onClick={onResetDefaults}>
                Reset
              </button>
              <button type="button" disabled={isSubmitDisabled} onClick={runAndCloseDialog}>
                Search
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
