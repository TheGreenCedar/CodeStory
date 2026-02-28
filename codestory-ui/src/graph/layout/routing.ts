import { MarkerType, Position, type Edge, type Node } from "@xyflow/react";

import type { EdgeKind, GraphResponse, LayoutDirection } from "../../generated/api";
import { EDGE_KIND_COLORS } from "../../theme/tokens";
import { PARITY_CONSTANTS } from "./parityConstants";
import type {
  FlowNodeData,
  LayoutElements,
  LegendRow,
  RoutePoint,
  SemanticEdgeFamily,
} from "./types";

type EdgePalette = {
  stroke: string;
  width: number;
};

export type SemanticEdgeData = {
  edgeKind: EdgeKind;
  sourceEdgeIds: string[];
  certainty: string | null | undefined;
  family: SemanticEdgeFamily;
  multiplicity: number;
  routePoints: RoutePoint[];
  bundleTrunkX?: number;
  layoutDirection?: LayoutDirection;
  tooltipLabel?: string;
  isFocused?: boolean;
  isHovered?: boolean;
};

export const EDGE_STYLE: Record<EdgeKind, EdgePalette> = {
  MEMBER: { stroke: EDGE_KIND_COLORS.MEMBER, width: 2.0 },
  TYPE_USAGE: { stroke: EDGE_KIND_COLORS.TYPE_USAGE, width: 2.4 },
  USAGE: { stroke: EDGE_KIND_COLORS.USAGE, width: 2.8 },
  CALL: { stroke: EDGE_KIND_COLORS.CALL, width: 2.8 },
  INHERITANCE: { stroke: EDGE_KIND_COLORS.INHERITANCE, width: 2.4 },
  OVERRIDE: { stroke: EDGE_KIND_COLORS.OVERRIDE, width: 2.4 },
  TYPE_ARGUMENT: { stroke: EDGE_KIND_COLORS.TYPE_ARGUMENT, width: 2.4 },
  TEMPLATE_SPECIALIZATION: { stroke: EDGE_KIND_COLORS.TEMPLATE_SPECIALIZATION, width: 2.4 },
  INCLUDE: { stroke: EDGE_KIND_COLORS.INCLUDE, width: 2.4 },
  IMPORT: { stroke: EDGE_KIND_COLORS.IMPORT, width: 2.4 },
  MACRO_USAGE: { stroke: EDGE_KIND_COLORS.MACRO_USAGE, width: 2.4 },
  ANNOTATION_USAGE: { stroke: EDGE_KIND_COLORS.ANNOTATION_USAGE, width: 2.4 },
  UNKNOWN: { stroke: EDGE_KIND_COLORS.UNKNOWN, width: 2.4 },
};

const CLOSED_TRIANGLE_KINDS = new Set<EdgeKind>([
  "INHERITANCE",
  "TYPE_ARGUMENT",
  "TEMPLATE_SPECIALIZATION",
]);

function markerTypeFor(kind: EdgeKind): MarkerType {
  return CLOSED_TRIANGLE_KINDS.has(kind) ? MarkerType.ArrowClosed : MarkerType.Arrow;
}

function markerSizeFor(edge: { kind: EdgeKind; multiplicity: number }): {
  width: number;
  height: number;
} {
  if (edge.kind === "INHERITANCE") {
    return PARITY_CONSTANTS.markers.inheritance;
  }
  if (edge.kind === "TEMPLATE_SPECIALIZATION") {
    return PARITY_CONSTANTS.markers.templateSpecialization;
  }
  if (edge.multiplicity > 1) {
    return PARITY_CONSTANTS.markers.bundled;
  }
  return PARITY_CONSTANTS.markers.default;
}

function certaintyStroke(
  certainty: string | null | undefined,
  family: SemanticEdgeFamily,
): {
  dash?: string;
  opacity: number;
} {
  const certaintyProfile = PARITY_CONSTANTS.rendering.certainty;
  const hierarchyOpacityBias = family === "hierarchy" ? certaintyProfile.hierarchyOpacityBias : 0;
  const normalized = certainty?.toLowerCase();
  if (normalized === "uncertain") {
    return {
      dash: certaintyProfile.uncertainDash,
      opacity: Math.min(1, certaintyProfile.uncertainOpacity + hierarchyOpacityBias),
    };
  }
  if (normalized === "probable") {
    return { opacity: Math.min(1, certaintyProfile.probableOpacity + hierarchyOpacityBias) };
  }
  return { opacity: 1 };
}

function edgeWidth(baseWidth: number, multiplicity: number, family: SemanticEdgeFamily): number {
  const strokeAmplification = PARITY_CONSTANTS.rendering.strokeAmplification;
  const multiplicityBoost = Math.min(
    strokeAmplification.multiplicityMaxBoost,
    Math.max(0, multiplicity - 1) * strokeAmplification.multiplicityStep,
  );
  const hierarchyBoost = family === "hierarchy" ? strokeAmplification.hierarchyBoost : 0;
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

export function toReactFlowElements(
  layout: LayoutElements,
  layoutDirection: LayoutDirection = "Horizontal",
): {
  nodes: Node<FlowNodeData>[];
  edges: Edge<SemanticEdgeData>[];
  centerNodeId: string;
} {
  const horizontal = layoutDirection !== "Vertical";
  const nodes: Node<FlowNodeData>[] = layout.nodes.map((node) => ({
    width: node.width,
    height: node.height,
    id: node.id,
    type: "sourcetrail",
    position: { x: node.x, y: node.y },
    sourcePosition: horizontal ? Position.Right : Position.Bottom,
    targetPosition: horizontal ? Position.Left : Position.Top,
    draggable: false,
    selectable: !node.isVirtualBundle,
    focusable: !node.isVirtualBundle,
    data: {
      kind: node.kind,
      label: node.label,
      center: node.center,
      nodeStyle: node.nodeStyle,
      layoutDirection,
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
    const markerSize = markerSizeFor(edge);
    const markerEnd = {
      type: markerTypeFor(edge.kind),
      width: markerSize.width,
      height: markerSize.height,
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
        strokeDasharray: certainty.dash,
        opacity: certainty.opacity,
      },
      interactionWidth:
        edge.family === "hierarchy"
          ? PARITY_CONSTANTS.rendering.interactionWidth.hierarchy
          : PARITY_CONSTANTS.rendering.interactionWidth.default +
            Math.min(6, Math.max(0, edge.multiplicity - 1) * 1.1),
      data: {
        edgeKind: edge.kind,
        sourceEdgeIds: edge.sourceEdgeIds,
        certainty: edge.certainty,
        family: edge.family,
        multiplicity: edge.multiplicity,
        routePoints: edge.routePoints,
        layoutDirection,
      },
    };
  });

  return {
    nodes,
    edges,
    centerNodeId: layout.centerNodeId,
  };
}
