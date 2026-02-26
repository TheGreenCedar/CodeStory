import dagre from "@dagrejs/dagre";

import type { LayoutDirection } from "../../generated/api";
import type { LayoutElements, RoutedEdgeSpec } from "./types";

const RASTER_STEP = 8;

function snapToRaster(value: number): number {
  return Math.round(value / RASTER_STEP) * RASTER_STEP;
}

function directionToRankdir(layoutDirection: LayoutDirection): "LR" | "TB" {
  return layoutDirection === "Vertical" ? "TB" : "LR";
}

function edgeLayoutHints(edge: RoutedEdgeSpec): { weight: number; minlen: number } {
  if (edge.family === "hierarchy") {
    if (edge.kind === "INHERITANCE") {
      return { weight: 10, minlen: 3 };
    }
    return { weight: 8, minlen: 2 };
  }
  return { weight: Math.max(1, edge.multiplicity), minlen: 1 };
}

export function buildDagreLayout(
  seed: LayoutElements,
  layoutDirection: LayoutDirection = "Horizontal",
): LayoutElements {
  const graph = new dagre.graphlib.Graph({ multigraph: true });
  graph.setDefaultEdgeLabel(() => ({}));
  graph.setGraph({
    rankdir: directionToRankdir(layoutDirection),
    ranker: "network-simplex",
    acyclicer: "greedy",
    nodesep: 96,
    ranksep: 128,
    marginx: 24,
    marginy: 24,
  });

  const sortedNodes = [...seed.nodes].sort((left, right) => left.id.localeCompare(right.id));
  for (const node of sortedNodes) {
    graph.setNode(node.id, {
      width: node.width,
      height: node.height,
    });
  }

  const sortedEdges = [...seed.edges].sort((left, right) => left.id.localeCompare(right.id));
  for (const edge of sortedEdges) {
    graph.setEdge(edge.source, edge.target, edgeLayoutHints(edge), edge.id);
  }

  dagre.layout(graph);

  const positionedNodes = seed.nodes
    .map((node) => {
      const position = graph.node(node.id) as { x: number; y: number } | undefined;
      if (!position) {
        return node;
      }
      return {
        ...node,
        x: snapToRaster(position.x - node.width / 2),
        y: snapToRaster(position.y - node.height / 2),
      };
    })
    .sort((left, right) => left.x - right.x || left.y - right.y || left.id.localeCompare(right.id));

  const routedEdges = seed.edges.map((edge) => {
    const dagreEdge = graph.edge({ v: edge.source, w: edge.target, name: edge.id }) as
      | { points?: Array<{ x: number; y: number }> }
      | undefined;

    const routePoints =
      dagreEdge?.points?.map((point) => ({
        x: snapToRaster(point.x),
        y: snapToRaster(point.y),
      })) ?? [];

    return {
      ...edge,
      routePoints,
    };
  });

  return {
    ...seed,
    nodes: positionedNodes,
    edges: routedEdges,
  };
}
