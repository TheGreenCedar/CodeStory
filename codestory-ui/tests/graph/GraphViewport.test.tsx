import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { GraphArtifactDto } from "../../src/generated/api";
import { GraphViewport } from "../../src/graph/GraphViewport";
import * as dagreLayout from "../../src/graph/layout/dagreLayout";
import { defaultTrailUiConfig } from "../../src/graph/trailConfig";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(),
  },
}));

vi.mock("@xyflow/react", async () => {
  const React = await import("react");
  return {
    ReactFlow: ({
      children,
      onInit,
      nodes,
      edges,
      onEdgeClick,
      onEdgeContextMenu,
      onEdgeMouseEnter,
      onEdgeMouseLeave,
      edgeTypes,
    }: {
      children: React.ReactNode;
      onInit?: unknown;
      nodes?: Array<{ id: string }>;
      edges?: Array<{
        id: string;
        source: string;
        target: string;
        type?: string;
        markerEnd?: unknown;
        style?: unknown;
        data?: unknown;
      }>;
      onEdgeClick?: unknown;
      onEdgeContextMenu?: unknown;
      onEdgeMouseEnter?: unknown;
      onEdgeMouseLeave?: unknown;
      edgeTypes?: Record<string, React.ComponentType<unknown>>;
    }) => {
      React.useEffect(() => {
        if (typeof onInit === "function") {
          onInit({
            fitView: () => Promise.resolve(),
          });
        }
      }, [onInit]);

      return (
        <div data-testid="reactflow">
          <div data-testid="node-ids">{nodes?.map((node) => node.id).join(",")}</div>
          <button
            type="button"
            data-testid="mock-edge-click"
            onClick={() => {
              const edge = edges?.[0];
              if (edge && typeof onEdgeClick === "function") {
                onEdgeClick({ button: 0, altKey: false }, edge);
              }
            }}
          >
            edge
          </button>
          <button
            type="button"
            data-testid="mock-edge-alt-click"
            onClick={() => {
              const edge = edges?.[0];
              if (edge && typeof onEdgeClick === "function") {
                onEdgeClick({ button: 0, altKey: true }, edge);
              }
            }}
          >
            edge-alt
          </button>
          <button
            type="button"
            data-testid="mock-edge-context-menu"
            onClick={() => {
              const edge = edges?.[0];
              if (edge && typeof onEdgeContextMenu === "function") {
                onEdgeContextMenu(
                  {
                    clientX: 120,
                    clientY: 80,
                    preventDefault: () => undefined,
                    stopPropagation: () => undefined,
                  },
                  edge,
                );
              }
            }}
          >
            edge-context
          </button>
          <button
            type="button"
            data-testid="mock-edge-enter"
            onClick={() => {
              const edge = edges?.[0];
              if (edge && typeof onEdgeMouseEnter === "function") {
                onEdgeMouseEnter({}, edge);
              }
            }}
          >
            edge-enter
          </button>
          <button
            type="button"
            data-testid="mock-edge-leave"
            onClick={() => {
              const edge = edges?.[0];
              if (edge && typeof onEdgeMouseLeave === "function") {
                onEdgeMouseLeave({}, edge);
              }
            }}
          >
            edge-leave
          </button>
          <div data-testid="edge-layer">
            {(edges ?? []).map((edge) => {
              const edgeType = edge.type ?? "semantic";
              const EdgeRenderer = edgeTypes?.[edgeType];
              if (!EdgeRenderer) {
                return null;
              }
              return (
                <EdgeRenderer
                  key={`edge-${edge.id}`}
                  id={edge.id}
                  source={edge.source}
                  target={edge.target}
                  sourceX={8}
                  sourceY={12}
                  targetX={120}
                  targetY={12}
                  markerEnd={edge.markerEnd}
                  style={edge.style}
                  data={edge.data}
                />
              );
            })}
          </div>
          {children}
        </div>
      );
    },
    Controls: () => <div data-testid="controls" />,
    MiniMap: ({
      position,
      bgColor,
      maskColor,
      className,
    }: {
      position: string;
      bgColor?: string;
      maskColor?: string;
      className?: string;
    }) => (
      <div
        data-testid="minimap"
        data-position={position}
        data-bg-color={bgColor}
        data-mask-color={maskColor}
        data-class={className}
      />
    ),
    Panel: ({
      children,
      className,
      position,
    }: {
      children: React.ReactNode;
      className?: string;
      position: string;
    }) => (
      <div data-testid="panel" data-position={position} className={className}>
        {children}
      </div>
    ),
    Handle: () => <span data-testid="handle" />,
    BaseEdge: ({ id, path }: { id?: string; path?: string }) => (
      <path data-testid={`base-edge-${id ?? "unknown"}`} d={path} />
    ),
    EdgeLabelRenderer: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    getSmoothStepPath: ({
      sourceX,
      sourceY,
      targetX,
      targetY,
    }: {
      sourceX: number;
      sourceY: number;
      targetX: number;
      targetY: number;
    }) => {
      const midX = (sourceX + targetX) / 2;
      const midY = (sourceY + targetY) / 2;
      return [`M ${sourceX} ${sourceY} L ${targetX} ${targetY}`, midX, midY];
    },
    Position: {
      Left: "left",
      Right: "right",
      Top: "top",
      Bottom: "bottom",
    },
    MarkerType: {
      Arrow: "arrow",
      ArrowClosed: "arrow-closed",
    },
  };
});

const STRUCTURAL_NODE_KINDS = new Set([
  "CLASS",
  "STRUCT",
  "INTERFACE",
  "UNION",
  "ENUM",
  "NAMESPACE",
  "MODULE",
  "PACKAGE",
  "FILE",
]);

const HIERARCHY_EDGE_KINDS = new Set([
  "INHERITANCE",
  "OVERRIDE",
  "TYPE_ARGUMENT",
  "TEMPLATE_SPECIALIZATION",
]);

function withCanonicalLayout(graph: GraphArtifactDto): GraphArtifactDto {
  if (graph.kind !== "uml") {
    return graph;
  }

  const orderedNodes = [...graph.graph.nodes].sort((left, right) => {
    const depthDiff = left.depth - right.depth;
    if (depthDiff !== 0) {
      return depthDiff;
    }
    const labelDiff = left.label.localeCompare(right.label);
    if (labelDiff !== 0) {
      return labelDiff;
    }
    return left.id.localeCompare(right.id);
  });

  const rowByDepth = new Map<number, number>();
  const canonicalNodes = orderedNodes.map((node) => {
    const yRank = rowByDepth.get(node.depth) ?? 0;
    rowByDepth.set(node.depth, yRank + 1);
    const nodeStyle = STRUCTURAL_NODE_KINDS.has(node.kind) ? "card" : "pill";
    return {
      id: node.id,
      kind: node.kind,
      label: node.label,
      center: node.id === graph.graph.center_id,
      node_style: nodeStyle,
      is_non_indexed: node.kind === "UNKNOWN" || node.kind === "BUILTIN_TYPE",
      duplicate_count: 1,
      merged_symbol_ids: [node.id],
      member_count: node.badge_visible_members ?? 0,
      badge_visible_members: node.badge_visible_members ?? null,
      badge_total_members: node.badge_total_members ?? null,
      members: [],
      x_rank: node.depth,
      y_rank: yRank,
      width: nodeStyle === "card" ? 260 : 220,
      height: nodeStyle === "card" ? 140 : 34,
      is_virtual_bundle: false,
    };
  });

  const canonicalEdges = graph.graph.edges
    .filter((edge) => edge.kind !== "MEMBER")
    .map((edge) => {
      const isHierarchy = HIERARCHY_EDGE_KINDS.has(edge.kind);
      return {
        id: edge.id,
        source_edge_ids: [edge.id],
        source: edge.source,
        target: edge.target,
        source_handle: "source-node",
        target_handle: "target-node",
        kind: edge.kind,
        certainty: edge.certainty ?? null,
        multiplicity: 1,
        family: isHierarchy ? ("hierarchy" as const) : ("flow" as const),
        route_kind: isHierarchy ? ("hierarchy" as const) : ("direct" as const),
      };
    });

  return {
    ...graph,
    graph: {
      ...graph.graph,
      canonical_layout: {
        schema_version: 1,
        center_node_id: graph.graph.center_id,
        nodes: canonicalNodes,
        edges: canonicalEdges,
      },
    },
  };
}

const GRAPH_FIXTURE: GraphArtifactDto = withCanonicalLayout({
  kind: "uml",
  id: "graph-1",
  title: "Graph",
  graph: {
    center_id: "run_incremental",
    truncated: false,
    nodes: [
      {
        id: "workspace",
        label: "WorkspaceIndexer",
        kind: "CLASS",
        depth: 0,
        badge_visible_members: 1,
        badge_total_members: 2,
        file_path: "src/workspace/WorkspaceIndexer.cpp",
        qualified_name: "codestory::workspace::WorkspaceIndexer",
      },
      {
        id: "run_incremental",
        label: "WorkspaceIndexer::run_incremental",
        kind: "METHOD",
        depth: 0,
        file_path: "src/workspace/WorkspaceIndexer.cpp",
        qualified_name: "codestory::workspace::WorkspaceIndexer::run_incremental",
      },
      {
        id: "merge",
        label: "IntermediateStorage::merge",
        kind: "METHOD",
        depth: 1,
        file_path: "src/storage/IntermediateStorage.cpp",
        qualified_name: "codestory::storage::IntermediateStorage::merge",
      },
    ],
    edges: [
      { id: "member-1", source: "workspace", target: "run_incremental", kind: "MEMBER" },
      { id: "call-1", source: "run_incremental", target: "merge", kind: "CALL" },
      { id: "usage-1", source: "workspace", target: "merge", kind: "USAGE" },
    ],
  },
});

const TOOLTIP_GRAPH_FIXTURE: GraphArtifactDto = withCanonicalLayout({
  kind: "uml",
  id: "graph-tooltip",
  title: "Tooltip Graph",
  graph: {
    center_id: "runner",
    truncated: false,
    nodes: [
      {
        id: "runner",
        label: "Runner::run",
        kind: "METHOD",
        depth: 0,
        file_path: "src/runner.cpp",
        qualified_name: "Runner::run",
      },
      {
        id: "worker",
        label: "Worker::execute",
        kind: "METHOD",
        depth: 1,
        file_path: "src/worker.cpp",
        qualified_name: "Worker::execute",
      },
    ],
    edges: [{ id: "tooltip-call", source: "runner", target: "worker", kind: "CALL" }],
  },
});

describe("GraphViewport", () => {
  it("renders legend docked to bottom-right when enabled", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: true, showMiniMap: false };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.getByText("Legend")).toBeInTheDocument();
    expect(document.querySelector(".graph-legend-panel")).toBeInTheDocument();
  });

  it("hides legend when disabled", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.queryByText("Legend")).not.toBeInTheDocument();
  });

  it("renders minimap toggle state", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: true };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.getByTestId("minimap")).toHaveAttribute("data-position", "bottom-left");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-class", "graph-minimap");
  });

  it("emits edge selections from edge clicks", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    const onSelectEdge = vi.fn();
    render(
      <GraphViewport
        graph={GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        onSelectEdge={onSelectEdge}
        trailConfig={config}
      />,
    );

    fireEvent.click(screen.getByTestId("mock-edge-click"));
    expect(onSelectEdge).toHaveBeenCalledTimes(1);
    expect(onSelectEdge.mock.calls[0]?.[0]).toMatchObject({
      id: expect.any(String),
      edgeIds: expect.arrayContaining([expect.any(String)]),
      sourceNodeId: expect.any(String),
      targetNodeId: expect.any(String),
      kind: expect.any(String),
    });
  });

  it("shows edge tooltip with edge type on hover", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport graph={TOOLTIP_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );

    expect(screen.queryByText(/call/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("mock-edge-enter"));
    expect(screen.getByText(/call/i)).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("mock-edge-leave"));
    expect(screen.queryByText(/call/i)).not.toBeInTheDocument();
  });

  it("renders smoothstep svg paths for semantic edges", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport graph={TOOLTIP_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );

    const firstPath = document.querySelector(
      'path[data-testid^="base-edge-"]',
    ) as SVGPathElement | null;
    const d = firstPath?.getAttribute("d") ?? "";
    expect(d.startsWith("M ")).toBe(true);
    expect(d.includes("L")).toBe(true);
  });

  it("hides edges on alt+left-click without activating selection", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    const onStatusMessage = vi.fn();
    const onSelectEdge = vi.fn();
    render(
      <GraphViewport
        graph={TOOLTIP_GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        onSelectEdge={onSelectEdge}
        onStatusMessage={onStatusMessage}
        trailConfig={config}
      />,
    );

    fireEvent.click(screen.getByTestId("mock-edge-alt-click"));
    expect(onSelectEdge).not.toHaveBeenCalled();
    expect(onStatusMessage).toHaveBeenCalledWith(
      "Edge hidden. Use Reset Hidden in the context menu to restore.",
    );
  });

  it("adds grouping container nodes for file grouping mode", () => {
    const config = {
      ...defaultTrailUiConfig(),
      showLegend: false,
      showMiniMap: false,
      groupingMode: "file" as const,
    };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    const renderedNodeIds = screen.getByTestId("node-ids").textContent ?? "";
    expect(renderedNodeIds).toContain("group:file:");
  });

  it("opens custom trail dialog with ctrl+u", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    const onRequestOpenTrailDialog = vi.fn();
    render(
      <GraphViewport
        graph={GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        trailConfig={config}
        onRequestOpenTrailDialog={onRequestOpenTrailDialog}
      />,
    );

    fireEvent.keyDown(window, { key: "u", ctrlKey: true });
    expect(onRequestOpenTrailDialog).toHaveBeenCalledTimes(1);
  });

  it("toggles legend with question-mark shortcut", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: true, showMiniMap: false };
    const onToggleLegend = vi.fn();
    render(
      <GraphViewport
        graph={GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        trailConfig={config}
        onToggleLegend={onToggleLegend}
      />,
    );

    fireEvent.keyDown(window, { key: "?", shiftKey: true });
    expect(onToggleLegend).toHaveBeenCalledTimes(1);
  });

  it("does not recompute dagre for hover/selection-only interactions", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    const dagreSpy = vi.spyOn(dagreLayout, "buildDagreLayout");
    const onSelectEdge = vi.fn();
    render(
      <GraphViewport
        graph={TOOLTIP_GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        onSelectEdge={onSelectEdge}
        trailConfig={config}
      />,
    );

    const baseline = dagreSpy.mock.calls.length;
    fireEvent.click(screen.getByTestId("mock-edge-enter"));
    fireEvent.click(screen.getByTestId("mock-edge-click"));
    fireEvent.click(screen.getByTestId("mock-edge-leave"));

    expect(dagreSpy.mock.calls.length).toBe(baseline);
    dagreSpy.mockRestore();
  });

  it("hard-fails rendering when canonical payload is missing", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport
        graph={{
          ...GRAPH_FIXTURE,
          id: "graph-missing-canonical",
          graph: {
            ...GRAPH_FIXTURE.graph,
            canonical_layout: null,
          },
        }}
        onSelectNode={vi.fn()}
        trailConfig={config}
      />,
    );

    expect(screen.getByText(/Unable to render UML graph:/i)).toBeInTheDocument();
    expect(screen.queryByTestId("reactflow")).not.toBeInTheDocument();
  });
});
