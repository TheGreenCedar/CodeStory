import type { KeyboardEvent } from "react";

import { GraphTrailControls } from "./GraphTrailControls";
import { GraphViewport } from "../graph/GraphViewport";
import type { GraphArtifactDto, SearchHit } from "../generated/api";
import type { TrailUiConfig } from "../graph/trailConfig";

type GraphPaneProps = {
  activeGraph: GraphArtifactDto | null;
  isTruncated: boolean;
  searchQuery: string;
  onSearchQueryChange: (query: string) => void;
  onSearchKeyDown: (event: KeyboardEvent<HTMLInputElement>) => void;
  onSearchFocus: () => void;
  onSearchBlur: () => void;
  isSearching: boolean;
  searchOpen: boolean;
  searchHits: SearchHit[];
  searchIndex: number;
  onSearchHitHover: (index: number) => void;
  onSearchHitActivate: (hit: SearchHit) => void;
  projectOpen: boolean;
  graphOrder: string[];
  activeGraphId: string | null;
  graphMap: Record<string, GraphArtifactDto>;
  onActivateGraph: (graphId: string) => void;
  onSelectNode: (nodeId: string, label: string) => void;
  trailConfig: TrailUiConfig;
  trailRunning: boolean;
  trailDisabledReason: string | null;
  hasActiveRoot: boolean;
  onTrailConfigChange: (patch: Partial<TrailUiConfig>) => void;
  onRunTrail: () => void;
  onResetTrailDefaults: () => void;
};

export function GraphPane({
  activeGraph,
  isTruncated,
  searchQuery,
  onSearchQueryChange,
  onSearchKeyDown,
  onSearchFocus,
  onSearchBlur,
  isSearching,
  searchOpen,
  searchHits,
  searchIndex,
  onSearchHitHover,
  onSearchHitActivate,
  projectOpen,
  graphOrder,
  activeGraphId,
  graphMap,
  onActivateGraph,
  onSelectNode,
  trailConfig,
  trailRunning,
  trailDisabledReason,
  hasActiveRoot,
  onTrailConfigChange,
  onRunTrail,
  onResetTrailDefaults,
}: GraphPaneProps) {
  return (
    <section className="pane pane-graph">
      <div className="pane-header graph-header">
        <div className="graph-header-title">
          <h2>Graph Workspace</h2>
          {isTruncated && <span className="truncation-pill">Truncated</span>}
        </div>
        <div className="graph-search-wrap">
          <input
            className="graph-search-input"
            value={searchQuery}
            onChange={(event) => onSearchQueryChange(event.target.value)}
            onKeyDown={onSearchKeyDown}
            onFocus={onSearchFocus}
            onBlur={onSearchBlur}
            placeholder="Search symbols"
            disabled={!projectOpen}
            aria-label="Search symbols"
          />
          {isSearching && <span className="search-state">Searching...</span>}
          {searchOpen && searchHits.length > 0 && (
            <div className="search-dropdown" role="listbox" aria-label="Search hits">
              {searchHits.map((hit, idx) => (
                <button
                  key={`${hit.node_id}-${hit.score}`}
                  className={idx === searchIndex ? "search-hit search-hit-active" : "search-hit"}
                  onMouseEnter={() => onSearchHitHover(idx)}
                  onClick={() => onSearchHitActivate(hit)}
                >
                  <span className="search-hit-name">{hit.display_name}</span>
                  <span className="search-hit-kind">{hit.kind}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="graph-tabs">
          {graphOrder.slice(0, 8).map((graphId) => (
            <button
              key={graphId}
              className={activeGraphId === graphId ? "tab-active" : ""}
              onClick={() => onActivateGraph(graphId)}
            >
              {graphMap[graphId]?.title ?? graphId}
            </button>
          ))}
        </div>
      </div>
      <GraphTrailControls
        config={trailConfig}
        projectOpen={projectOpen}
        hasRootSymbol={hasActiveRoot}
        disabledReason={trailDisabledReason}
        isRunning={trailRunning}
        onConfigChange={onTrailConfigChange}
        onRunTrail={onRunTrail}
        onResetDefaults={onResetTrailDefaults}
      />
      <div className="graph-canvas">
        <GraphViewport graph={activeGraph} onSelectNode={onSelectNode} />
      </div>
    </section>
  );
}
