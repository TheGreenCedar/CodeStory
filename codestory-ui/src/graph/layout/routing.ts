import { MarkerType, Position, type Edge, type Node } from "@xyflow/react";

import type { EdgeKind, GraphResponse } from "../../generated/api";
import type {
  FlowNodeData,
  LayoutElements,
  LegendRow,
  RouteKind,
  SemanticEdgeFamily,
} from "./types";

type EdgePalette = {
  stroke: string;
  width: number;
};

export type SemanticEdgeData = {
  routeKind: RouteKind;
  family: SemanticEdgeFamily;
  bundleCount: number;
  trunkCoord?: number;
};

export const EDGE_STYLE: Record<EdgeKind, EdgePalette> = {
  MEMBER: { stroke: "#adb1b8", width: 1.8 },
  TYPE_USAGE: { stroke: "#7d8a99", width: 2.0 },
  USAGE: { stroke: "#4d95be", width: 2.2 },
  CALL: { stroke: "#dfa72e", width: 2.45 },
  INHERITANCE: { stroke: "#7f7f86", width: 2.1 },
  OVERRIDE: { stroke: "#ad86c8", width: 2.1 },
  TYPE_ARGUMENT: { stroke: "#d37b93", width: 2.0 },
  TEMPLATE_SPECIALIZATION: { stroke: "#bc8fa3", width: 2.0 },
  INCLUDE: { stroke: "#87a988", width: 2.0 },
  IMPORT: { stroke: "#87a988", width: 2.0 },
  MACRO_USAGE: { stroke: "#b88b66", width: 2.0 },
  ANNOTATION_USAGE: { stroke: "#8f96b2", width: 2.0 },
  UNKNOWN: { stroke: "#8b8f96", width: 2.0 },
};

const OPEN_ARROW_KINDS = new Set<EdgeKind>([
  "INHERITANCE",
  "OVERRIDE",
  "TYPE_ARGUMENT",
  "TEMPLATE_SPECIALIZATION",
]);

function markerTypeFor(kind: EdgeKind): MarkerType {
  return OPEN_ARROW_KINDS.has(kind) ? MarkerType.Arrow : MarkerType.ArrowClosed;
}

function certaintyStroke(certainty: string | null | undefined): {
  dash?: string;
  opacity: number;
} {
  const normalized = certainty?.toLowerCase();
  if (normalized === "uncertain") {
    return { dash: "6 5", opacity: 0.74 };
  }
  if (normalized === "probable") {
    return { opacity: 0.88 };
  }
  return { opacity: 1 };
}

function edgeWidth(
  baseWidth: number,
  multiplicity: number,
  bundleCount: number,
  routeKind: RouteKind,
): number {
  if (routeKind === "flow-trunk") {
    const scaled = baseWidth + Math.log2(Math.max(1, bundleCount)) * 0.8;
    return Math.min(5.2, Math.max(baseWidth, scaled));
  }
  return baseWidth + Math.min(0.72, Math.max(0, multiplicity - 1) * 0.15);
}

export function buildLegendRows(graph: GraphResponse): LegendRow[] {
  const byKind = new Map<string, LegendRow>();
  for (const edge of graph.edges) {
    if (edge.kind === "MEMBER") {
      continue;
    }

    const certainty = edge.certainty?.toLowerCase();
    const existing = byKind.get(edge.kind) ?? {
      kind: edge.kind,
      stroke: EDGE_STYLE[edge.kind]?.stroke ?? EDGE_STYLE.UNKNOWN.stroke,
      count: 0,
      hasUncertain: false,
      hasProbable: false,
    };

    existing.count += 1;
    if (certainty === "uncertain") {
      existing.hasUncertain = true;
    } else if (certainty === "probable") {
      existing.hasProbable = true;
    }
    byKind.set(edge.kind, existing);
  }

  return [...byKind.values()].sort(
    (left, right) => right.count - left.count || left.kind.localeCompare(right.kind),
  );
}

export function toReactFlowElements(layout: LayoutElements): {
  nodes: Node<FlowNodeData>[];
  edges: Edge<SemanticEdgeData>[];
  centerNodeId: string;
} {
  const nodes: Node<FlowNodeData>[] = layout.nodes.map((node) => ({
    id: node.id,
    type: "sourcetrail",
    position: { x: node.x, y: node.y },
    sourcePosition: Position.Right,
    targetPosition: Position.Left,
    draggable: false,
    selectable: !node.isVirtualBundle,
    focusable: !node.isVirtualBundle,
    data: {
      kind: node.kind,
      label: node.label,
      center: node.center,
      nodeStyle: node.nodeStyle,
      duplicateCount: node.duplicateCount,
      memberCount: node.memberCount,
      members: node.members,
      isVirtualBundle: node.isVirtualBundle,
    },
  }));

  const edges: Edge<SemanticEdgeData>[] = layout.edges.map((edge) => {
    const palette = EDGE_STYLE[edge.kind] ?? EDGE_STYLE.UNKNOWN;
    const certainty = certaintyStroke(edge.certainty);
    const targetIsBundleNode = edge.target.startsWith("bundle:");
    const markerEnd = targetIsBundleNode
      ? undefined
      : {
          type: markerTypeFor(edge.kind),
          width: 13,
          height: 13,
          color: palette.stroke,
        };
    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
      type: "semantic",
      animated: false,
      markerEnd,
      style: {
        stroke: palette.stroke,
        strokeWidth: edgeWidth(palette.width, edge.multiplicity, edge.bundleCount, edge.routeKind),
        strokeLinecap: "round",
        strokeDasharray: certainty.dash,
        opacity: certainty.opacity,
      },
      interactionWidth: edge.routeKind === "flow-trunk" ? 24 : 18,
      data: {
        routeKind: edge.routeKind,
        family: edge.family,
        bundleCount: edge.bundleCount,
        trunkCoord: edge.trunkCoord,
      },
    };
  });

  return {
    nodes,
    edges,
    centerNodeId: layout.centerNodeId,
  };
}
