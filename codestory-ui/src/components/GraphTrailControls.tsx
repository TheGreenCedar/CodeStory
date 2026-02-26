import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { api } from "../api/client";
import type {
  EdgeKind,
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
  hasRootSymbol: boolean;
  disabledReason: string | null;
  isRunning: boolean;
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
  hasRootSymbol,
  disabledReason,
  isRunning,
  onConfigChange,
  onRunTrail,
  onResetDefaults,
}: GraphTrailControlsProps) {
  const [targetQuery, setTargetQuery] = useState<string>(config.targetLabel);
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchOpen, setSearchOpen] = useState<boolean>(false);
  const [searchIndex, setSearchIndex] = useState<number>(0);
  const [searching, setSearching] = useState<boolean>(false);
  const searchSeqRef = useRef<number>(0);

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

  const isSubmitDisabled = disabledReason !== null || isRunning;

  const hasAnyNodeFilter = config.nodeFilter.length > 0;
  const nodeFilterSummary = hasAnyNodeFilter
    ? `${config.nodeFilter.length} node kind${config.nodeFilter.length === 1 ? "" : "s"}`
    : "All node kinds";

  const activeEdgeKinds = useMemo(() => new Set(config.edgeFilter), [config.edgeFilter]);

  const toggleEdgeKind = (kind: EdgeKind) => {
    if (activeEdgeKinds.has(kind)) {
      onConfigChange({ edgeFilter: config.edgeFilter.filter((item) => item !== kind) });
      return;
    }

    onConfigChange({ edgeFilter: [...config.edgeFilter, kind] });
  };

  const toggleNodeKind = (kind: NodeKind) => {
    if (config.nodeFilter.includes(kind)) {
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

  return (
    <div className="graph-trail-controls" aria-label="Trail controls">
      <div className="graph-trail-grid">
        <label className="graph-control-field">
          <span>Mode</span>
          <select
            value={config.mode}
            onChange={(event) => {
              const mode = event.target.value as TrailMode;
              onConfigChange({
                mode,
                ...(mode !== "ToTargetSymbol" ? { targetId: null, targetLabel: "" } : {}),
              });
            }}
          >
            {(
              ["Neighborhood", "AllReferenced", "AllReferencing", "ToTargetSymbol"] as TrailMode[]
            ).map((mode) => (
              <option key={mode} value={mode}>
                {modeLabel(mode)}
              </option>
            ))}
          </select>
        </label>

        <label className="graph-control-field">
          <span>Depth</span>
          <input
            type="number"
            min={0}
            max={64}
            value={config.depth}
            onChange={(event) => {
              const parsed = Number(event.target.value);
              onConfigChange({ depth: Number.isFinite(parsed) ? Math.max(0, parsed) : 0 });
            }}
          />
        </label>

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
            {(["ProductionOnly", "IncludeTestsAndBenches"] as TrailCallerScope[]).map((scope) => (
              <option key={scope} value={scope}>
                {callerScopeLabel(scope)}
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
                maxNodes: Number.isFinite(parsed) ? Math.max(10, Math.min(100000, parsed)) : 500,
              });
            }}
          />
        </label>
      </div>

      <div className="graph-filter-row">
        <span className="graph-filter-label">View Options</span>
        <div className="graph-chip-row">
          <button
            type="button"
            className={config.showUtilityCalls ? "graph-chip graph-chip-active" : "graph-chip"}
            onClick={() => onConfigChange({ showUtilityCalls: !config.showUtilityCalls })}
          >
            Utility Calls
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
        </div>
        <div
          className="graph-chip-row graph-chip-row-grouping"
          role="group"
          aria-label="Node grouping"
        >
          {(["none", "namespace", "file"] as GroupingMode[]).map((mode) => {
            const active = config.groupingMode === mode;
            return (
              <button
                key={mode}
                type="button"
                className={active ? "graph-chip graph-chip-active" : "graph-chip"}
                aria-pressed={active}
                onClick={() => onConfigChange({ groupingMode: mode })}
              >
                {groupingModeLabel(mode)}
              </button>
            );
          })}
        </div>
      </div>

      {config.mode === "ToTargetSymbol" ? (
        <div className="graph-target-wrap">
          <label
            className="graph-control-field graph-control-field-target"
            aria-label="Target symbol search"
          >
            <span>Target Symbol</span>
            <input
              role="combobox"
              aria-expanded={searchOpen}
              aria-controls="trail-target-results"
              value={targetQuery}
              placeholder="Search target symbol"
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
          </label>
          {searching && <span className="trail-target-state">Searching...</span>}
          {searchOpen && searchHits.length > 0 && (
            <div id="trail-target-results" className="search-dropdown" role="listbox">
              {searchHits.map((hit, idx) => (
                <button
                  key={`${hit.node_id}-${hit.score}`}
                  className={idx === searchIndex ? "search-hit search-hit-active" : "search-hit"}
                  onMouseEnter={() => setSearchIndex(idx)}
                  onClick={() => selectTargetHit(hit)}
                >
                  <span className="search-hit-name">{hit.display_name}</span>
                  <span className="search-hit-kind">{hit.kind}</span>
                </button>
              ))}
            </div>
          )}
          {config.targetId ? (
            <button
              type="button"
              className="graph-target-clear"
              onClick={() => {
                setTargetQuery("");
                onConfigChange({ targetId: null, targetLabel: "" });
              }}
            >
              Clear Target
            </button>
          ) : null}
        </div>
      ) : null}

      <div className="graph-filter-row">
        <span className="graph-filter-label">Edge Filter</span>
        <div className="graph-chip-row">
          {EDGE_KIND_OPTIONS.map((kind) => {
            const active = activeEdgeKinds.has(kind);
            return (
              <button
                key={kind}
                type="button"
                className={active ? "graph-chip graph-chip-active" : "graph-chip"}
                onClick={() => toggleEdgeKind(kind)}
              >
                {titleCase(kind)}
              </button>
            );
          })}
        </div>
      </div>

      <details className="graph-filter-details">
        <summary>Node Filter: {nodeFilterSummary}</summary>
        <div className="graph-chip-row graph-chip-row-node">
          {NODE_KIND_OPTIONS.map((kind) => {
            const active = config.nodeFilter.includes(kind);
            return (
              <button
                key={kind}
                type="button"
                className={active ? "graph-chip graph-chip-active" : "graph-chip"}
                onClick={() => toggleNodeKind(kind)}
              >
                {titleCase(kind)}
              </button>
            );
          })}
        </div>
      </details>

      <div className="graph-trail-actions">
        <button type="button" onClick={onRunTrail} disabled={isSubmitDisabled}>
          {isRunning ? "Running..." : "Run Trail"}
        </button>
        <button type="button" onClick={onResetDefaults} disabled={isRunning}>
          Reset
        </button>
        {disabledReason && <span className="graph-trail-reason">{disabledReason}</span>}
      </div>

      {!projectOpen || !hasRootSymbol ? (
        <div className="graph-trail-hint">Select a symbol to use as the trail root.</div>
      ) : null}
    </div>
  );
}
