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

const CARD_WIDTH_MIN = 228;
const CARD_WIDTH_MAX = 432;
const CARD_CHROME_WIDTH = 112;
const PILL_WIDTH_MIN = 96;
const PILL_WIDTH_MAX = 272;
const PILL_CHROME_WIDTH = 58;
const APPROX_CHAR_WIDTH = 7.25;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function estimateNodeDimensions(node: LayoutElements["nodes"][number]): {
  width: number;
  height: number;
} {
  if (node.nodeStyle === "bundle") {
    return { width: 6, height: 6 };
  }

  if (node.nodeStyle === "pill") {
    const width = clamp(
      PILL_CHROME_WIDTH + node.label.length * APPROX_CHAR_WIDTH,
      PILL_WIDTH_MIN,
      PILL_WIDTH_MAX,
    );
    return { width, height: 34 };
  }

  const longestLabel = Math.max(
    node.label.length,
    ...node.members.map((member) => member.label.length),
  );
  const width = clamp(
    CARD_CHROME_WIDTH + longestLabel * APPROX_CHAR_WIDTH,
    CARD_WIDTH_MIN,
    CARD_WIDTH_MAX,
  );
  const publicCount = node.members.filter((member) => member.visibility === "public").length;
  const privateCount = node.members.length - publicCount;
  const sectionCount = (publicCount > 0 ? 1 : 0) + (privateCount > 0 ? 1 : 0);
  const effectiveSections = sectionCount === 0 ? 1 : sectionCount;
  const height = clamp(
    74 + effectiveSections * 28 + Math.max(1, node.members.length) * 21,
    110,
    560,
  );
  return { width, height };
}

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
    ...estimateNodeDimensions(node),
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
    const targetIsBundleNode = edge.target.startsWith("bundle:");
    const hierarchyEdge = edge.family === "hierarchy";
    const markerSize = hierarchyEdge ? 14 : 13;
    const markerEnd = targetIsBundleNode
      ? undefined
      : {
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
