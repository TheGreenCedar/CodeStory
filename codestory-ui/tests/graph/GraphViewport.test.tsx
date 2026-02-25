import { render, screen } from "@testing-library/react";
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
    ReactFlow: ({ children, onInit }: { children: React.ReactNode; onInit?: unknown }) => {
      React.useEffect(() => {
        if (typeof onInit === "function") {
          onInit({
            fitView: () => Promise.resolve(),
          });
        }
      }, [onInit]);
      return <div data-testid="reactflow">{children}</div>;
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
      },
      {
        id: "run_incremental",
        label: "WorkspaceIndexer::run_incremental",
        kind: "METHOD",
        depth: 0,
      },
      {
        id: "merge",
        label: "IntermediateStorage::merge",
        kind: "METHOD",
        depth: 1,
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
    expect(screen.getByTestId("panel")).toHaveAttribute("data-position", "bottom-right");
  });

  it("hides legend when disabled", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: false };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.queryByText("Legend")).not.toBeInTheDocument();
    expect(screen.queryByTestId("panel")).not.toBeInTheDocument();
  });

  it("renders minimap toggle state", () => {
    const config = { ...defaultTrailUiConfig(), showLegend: false, showMiniMap: true };
    render(<GraphViewport graph={GRAPH_FIXTURE} onSelectNode={vi.fn()} trailConfig={config} />);

    expect(screen.getByTestId("minimap")).toHaveAttribute("data-position", "bottom-left");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-class", "graph-minimap");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-bg-color");
    expect(screen.getByTestId("minimap")).toHaveAttribute("data-mask-color");
  });
});
