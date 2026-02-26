import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { GraphArtifactDto } from "../../src/generated/api";
import { GraphViewport } from "../../src/graph/GraphViewport";
import { defaultTrailUiConfig } from "../../src/graph/trailConfig";

const bundlingControl = { forceChannelizedBundling: false };

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(),
  },
}));

vi.mock("../../src/graph/layout/bundling", async () => {
  const actual = await vi.importActual<typeof import("../../src/graph/layout/bundling")>(
    "../../src/graph/layout/bundling",
  );
  return {
    ...actual,
    applyAdaptiveBundling: (...args: Parameters<typeof actual.applyAdaptiveBundling>) => {
      const bundled = actual.applyAdaptiveBundling(...args);
      if (!bundlingControl.forceChannelizedBundling) {
        return bundled;
      }
      return {
        ...bundled,
        edges: bundled.edges.map((edge) =>
          edge.kind === "CALL"
            ? {
                ...edge,
                routeKind: "flow-trunk",
                channelId: "channel:test:forced",
                channelWeight: 6,
                bundleCount: 6,
              }
            : edge,
        ),
      };
    },
  };
});

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

afterEach(() => {
  bundlingControl.forceChannelizedBundling = false;
});

const GRAPH_FIXTURE: GraphArtifactDto = {
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
};

const TOOLTIP_GRAPH_FIXTURE: GraphArtifactDto = {
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
};

const BUNDLED_GRAPH_FIXTURE: GraphArtifactDto = {
  kind: "uml",
  id: "graph-bundled",
  title: "Bundled Graph",
  graph: {
    center_id: "hub",
    truncated: false,
    nodes: [
      {
        id: "hub",
        label: "Hub::run",
        kind: "METHOD",
        depth: 0,
        file_path: "src/hub.cpp",
        qualified_name: "Hub::run",
      },
      {
        id: "leaf-1",
        label: "Leaf::one",
        kind: "METHOD",
        depth: 1,
        file_path: "src/leaf1.cpp",
        qualified_name: "Leaf::one",
      },
      {
        id: "leaf-2",
        label: "Leaf::two",
        kind: "METHOD",
        depth: 1,
        file_path: "src/leaf2.cpp",
        qualified_name: "Leaf::two",
      },
      {
        id: "leaf-3",
        label: "Leaf::three",
        kind: "METHOD",
        depth: 1,
        file_path: "src/leaf3.cpp",
        qualified_name: "Leaf::three",
      },
    ],
    edges: [
      { id: "call-b-1", source: "hub", target: "leaf-1", kind: "CALL" },
      { id: "call-b-2", source: "hub", target: "leaf-2", kind: "CALL" },
      { id: "call-b-3", source: "hub", target: "leaf-3", kind: "CALL" },
    ],
  },
};

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
    expect(document.querySelector(".graph-legend-panel")).not.toBeInTheDocument();
  });

  it("renders minimap toggle state", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: true };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.getByTestId("minimap")).toHaveAttribute("data-position", "bottom-left");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-class", "graph-minimap");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-bg-color");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-mask-color");
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
      id: expect.stringMatching(/^(call|usage)-1$/),
      edgeIds: [expect.stringMatching(/^(call|usage)-1$/)],
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

  it("renders rounded orthogonal svg paths for semantic edges", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport graph={TOOLTIP_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );

    const firstPath = document.querySelector(
      'path[data-testid^="base-edge-"]',
    ) as SVGPathElement | null;
    const d = firstPath?.getAttribute("d") ?? "";
    expect(d.startsWith("M ")).toBe(true);
    expect(d.includes("A ")).toBe(true);
  });

  it("shows bundled edge count in tooltip on bundled hover", () => {
    bundlingControl.forceChannelizedBundling = true;
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport graph={BUNDLED_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );

    fireEvent.click(screen.getByTestId("mock-edge-enter"));
    expect(screen.getByText(/call \(3 edges\)/i)).toBeInTheDocument();
  });

  it("renders Sourcetrail-style rounded hook joins for bundled trunks", () => {
    bundlingControl.forceChannelizedBundling = true;
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(
      <GraphViewport graph={BUNDLED_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );

    const firstPath = document.querySelector(
      'path[data-testid^="base-edge-"]',
    ) as SVGPathElement | null;
    const d = firstPath?.getAttribute("d") ?? "";
    const roundedCornerCount = (d.match(/ A /g) ?? []).length;
    expect(roundedCornerCount).toBeGreaterThanOrEqual(4);
  });

  it("emits aggregated edgeIds when activating bundled channel edges", () => {
    bundlingControl.forceChannelizedBundling = true;
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    const onSelectEdge = vi.fn();
    render(
      <GraphViewport
        graph={BUNDLED_GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        onSelectEdge={onSelectEdge}
        trailConfig={config}
      />,
    );

    fireEvent.click(screen.getByTestId("mock-edge-click"));
    expect(onSelectEdge).toHaveBeenCalledTimes(1);
    expect(onSelectEdge.mock.calls[0]?.[0]).toMatchObject({
      edgeIds: expect.arrayContaining(["call-b-1", "call-b-2", "call-b-3"]),
      kind: "CALL",
    });
  });

  it("keeps bundled path geometry deterministic in vertical layout", () => {
    bundlingControl.forceChannelizedBundling = true;
    const config = {
      ...defaultTrailUiConfig(),
      showLegend: false,
      showMiniMap: false,
      layoutDirection: "Vertical" as const,
    };
    const { rerender } = render(
      <GraphViewport graph={BUNDLED_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );
    const firstRenderD = (
      document.querySelector('path[data-testid^="base-edge-"]') as SVGPathElement | null
    )?.getAttribute("d");

    rerender(
      <GraphViewport graph={BUNDLED_GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />,
    );
    const secondRenderD = (
      document.querySelector('path[data-testid^="base-edge-"]') as SVGPathElement | null
    )?.getAttribute("d");

    expect(firstRenderD).toBeTruthy();
    expect(firstRenderD).toBe(secondRenderD);
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

  it("keeps edge route contracts stable between grouped and non-grouped modes", () => {
    const baseConfig = {
      ...defaultTrailUiConfig(),
      showLegend: false,
      showMiniMap: false,
    };
    const { rerender } = render(
      <GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={baseConfig} />,
    );
    const baselinePath = (
      document.querySelector('path[data-testid^="base-edge-"]') as SVGPathElement | null
    )?.getAttribute("d");

    rerender(
      <GraphViewport
        graph={GRAPH_FIXTURE}
        onSelectNode={vi.fn()}
        trailConfig={{ ...baseConfig, groupingMode: "file" }}
      />,
    );
    const groupedPath = (
      document.querySelector('path[data-testid^="base-edge-"]') as SVGPathElement | null
    )?.getAttribute("d");

    expect(groupedPath).toBe(baselinePath);
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
});
