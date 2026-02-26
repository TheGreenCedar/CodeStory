import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { GraphArtifactDto } from "../../src/generated/api";
import { GraphViewport } from "../../src/graph/GraphViewport";
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
    }: {
      children: React.ReactNode;
      onInit?: unknown;
      nodes?: Array<{ id: string }>;
      edges?: Array<{ id: string; source: string; target: string; data?: unknown }>;
      onEdgeClick?: unknown;
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
                onEdgeClick({}, edge);
              }
            }}
          >
            edge
          </button>
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
    BaseEdge: () => <path />,
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
});
