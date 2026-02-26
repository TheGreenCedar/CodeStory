import { MarkerType, Position, type Edge, type Node } from "@xyflow/react";

import type { EdgeKind, GraphResponse } from "../../generated/api";
import type {
  FlowNodeData,
  LayoutElements,
  LegendRow,
  RoutePoint,
  RouteKind,
  SemanticEdgeFamily,
} from "./types";

type EdgePalette = {
  stroke: string;
  width: number;
};

export type SemanticEdgeData = {
  edgeKind: EdgeKind;
  sourceEdgeIds: string[];
  routeKind: RouteKind;
  family: SemanticEdgeFamily;
  bundleCount: number;
  routePoints: RoutePoint[];
  trunkCoord?: number;
  channelId?: string;
  channelWeight?: number;
};

export const EDGE_STYLE: Record<EdgeKind, EdgePalette> = {
  MEMBER: { stroke: "#adb1b8", width: 2.0 },
  TYPE_USAGE: { stroke: "#7d8a99", width: 2.4 },
  USAGE: { stroke: "#1f84d6", width: 2.8 },
  CALL: { stroke: "#dfa72e", width: 2.8 },
  INHERITANCE: { stroke: "#7f7f86", width: 2.4 },
  OVERRIDE: { stroke: "#ad86c8", width: 2.4 },
  TYPE_ARGUMENT: { stroke: "#d37b93", width: 2.4 },
  TEMPLATE_SPECIALIZATION: { stroke: "#bc8fa3", width: 2.4 },
  INCLUDE: { stroke: "#87a988", width: 2.4 },
  IMPORT: { stroke: "#87a988", width: 2.4 },
  MACRO_USAGE: { stroke: "#b88b66", width: 2.4 },
  ANNOTATION_USAGE: { stroke: "#8f96b2", width: 2.4 },
  UNKNOWN: { stroke: "#8b8f96", width: 2.4 },
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

function certaintyStroke(
  certainty: string | null | undefined,
  family: SemanticEdgeFamily,
): {
  dash?: string;
  opacity: number;
} {
  const hierarchyOpacityBias = family === "hierarchy" ? 0.14 : 0;
  const normalized = certainty?.toLowerCase();
  if (normalized === "uncertain") {
    return { dash: "6 5", opacity: Math.min(1, 0.85 + hierarchyOpacityBias) };
  }
  if (normalized === "probable") {
    return { opacity: Math.min(1, 0.95 + hierarchyOpacityBias) };
  }
  return { opacity: 1 };
}

function edgeWidth(baseWidth: number, multiplicity: number, family: SemanticEdgeFamily): number {
  const multiplicityBoost = Math.min(0.72, Math.max(0, multiplicity - 1) * 0.15);
  const hierarchyBoost = family === "hierarchy" ? 0.2 : 0;
  return baseWidth + multiplicityBoost + hierarchyBoost;
}

export function buildLegendRows(graph: GraphResponse): LegendRow[] {
  const byKind = new Map<EdgeKind, LegendRow>();
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
    width: node.width,
    height: node.height,
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
      isNonIndexed: node.isNonIndexed,
      duplicateCount: node.duplicateCount,
      mergedSymbolIds: node.mergedSymbolIds,
      memberCount: node.memberCount,
      badgeVisibleMembers: node.badgeVisibleMembers,
      badgeTotalMembers: node.badgeTotalMembers,
      members: node.members,
      isVirtualBundle: node.isVirtualBundle,
    },
  }));

  const edges: Edge<SemanticEdgeData>[] = layout.edges.map((edge) => {
    const palette = EDGE_STYLE[edge.kind] ?? EDGE_STYLE.UNKNOWN;
    const certainty = certaintyStroke(edge.certainty, edge.family);
    const hierarchyEdge = edge.family === "hierarchy";
    const markerSize = hierarchyEdge ? 14 : 13;
    const markerEnd = {
      type: markerTypeFor(edge.kind),
      width: markerSize,
      height: markerSize,
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
        strokeWidth: edgeWidth(palette.width, edge.multiplicity, edge.family),
        strokeLinecap: "butt",
        strokeDasharray: certainty.dash,
        opacity: certainty.opacity,
      },
      interactionWidth: edge.routeKind === "flow-trunk" ? 24 : hierarchyEdge ? 20 : 18,
      data: {
        edgeKind: edge.kind,
        sourceEdgeIds: edge.sourceEdgeIds,
        routeKind: edge.routeKind,
        family: edge.family,
        bundleCount: edge.bundleCount,
        routePoints: edge.routePoints,
        trunkCoord: edge.trunkCoord,
        channelId: edge.channelId,
        channelWeight: edge.channelWeight,
      },
    };
  });

  return {
    nodes,
    edges,
    centerNodeId: layout.centerNodeId,
  };
}
