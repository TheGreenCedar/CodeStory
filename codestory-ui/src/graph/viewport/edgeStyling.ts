import { Position, getSmoothStepPath, type Edge, type MarkerType, type Node } from "@xyflow/react";

import type { EdgeKind } from "../../generated/api";
import type { SemanticEdgeData } from "../layout/routing";
import type { FlowNodeData } from "../layout/types";

const EDGE_BUS_MIN_EDGES = 2;
const EDGE_BUS_SOURCE_GUTTER = 22;
const EDGE_BUS_TARGET_GUTTER = 34;
const EDGE_SOURCE_OUTSET = 2;
const EDGE_TARGET_INSET_BASE = 8;

type EdgePoint = { x: number; y: number };

function isUncertainEdge(certainty: string | null | undefined): boolean {
  return certainty?.toLowerCase() === "uncertain";
}

export function formatKindLabel(kind: string): string {
  return kind
    .toLowerCase()
    .split("_")
    .map((segment) => `${segment.slice(0, 1).toUpperCase()}${segment.slice(1)}`)
    .join(" ");
}

export function edgeTooltipLabel(
  data: SemanticEdgeData | undefined,
  bundledCount: number,
): string | undefined {
  if (!data?.edgeKind) {
    return undefined;
  }
  let label = formatKindLabel(data.edgeKind);
  if (bundledCount > 1) {
    label = `${label} (${bundledCount} edges)`;
  }
  if (isUncertainEdge(data.certainty)) {
    label = `ambiguous ${label}`;
  }
  return label;
}

export function measuredWidth(node: Node<FlowNodeData>): number {
  if (typeof node.width === "number") {
    return node.width;
  }
  if (node.data.nodeStyle === "card") {
    return 240;
  }
  return 132;
}

export function measuredHeight(node: Node<FlowNodeData>): number {
  if (typeof node.height === "number") {
    return node.height;
  }
  if (node.data.nodeStyle === "card") {
    return 160;
  }
  return 42;
}

function approxHandlePoint(
  node: Node<FlowNodeData> | undefined,
  handleId: string | null | undefined,
  isSource: boolean,
  layoutDirection: "Horizontal" | "Vertical",
): EdgePoint {
  if (!node) {
    return { x: 0, y: 0 };
  }
  const width = measuredWidth(node);
  const height = measuredHeight(node);
  const centerX = node.position.x + width / 2;
  const centerY = node.position.y + height / 2;
  const normalized = (handleId ?? "").toLowerCase();

  if (normalized.includes("top")) {
    return { x: centerX, y: node.position.y };
  }
  if (normalized.includes("bottom")) {
    return { x: centerX, y: node.position.y + height };
  }
  if (normalized.includes("left")) {
    return { x: node.position.x, y: centerY };
  }
  if (normalized.includes("right")) {
    return { x: node.position.x + width, y: centerY };
  }

  if (layoutDirection === "Vertical") {
    return isSource
      ? { x: centerX, y: node.position.y + height }
      : { x: centerX, y: node.position.y };
  }

  return isSource ? { x: node.position.x + width, y: centerY } : { x: node.position.x, y: centerY };
}

function polylinePath(points: EdgePoint[]): string {
  if (points.length === 0) {
    return "";
  }
  const deduped: EdgePoint[] = [];
  for (const point of points) {
    const previous = deduped.at(-1);
    if (previous && Math.abs(previous.x - point.x) < 0.5 && Math.abs(previous.y - point.y) < 0.5) {
      continue;
    }
    deduped.push(point);
  }
  if (deduped.length < 2) {
    return "";
  }
  const [first, ...rest] = deduped;
  const segments = rest.map((point) => `L ${point.x} ${point.y}`);
  return `M ${first!.x} ${first!.y} ${segments.join(" ")}`;
}

function midpointOnPolyline(points: EdgePoint[]): EdgePoint {
  if (points.length === 0) {
    return { x: 0, y: 0 };
  }
  if (points.length === 1) {
    return points[0]!;
  }

  let totalLength = 0;
  for (let index = 1; index < points.length; index += 1) {
    const previous = points[index - 1]!;
    const current = points[index]!;
    totalLength += Math.hypot(current.x - previous.x, current.y - previous.y);
  }
  if (totalLength < 1e-4) {
    return points[Math.floor(points.length / 2)]!;
  }

  const midpointLength = totalLength / 2;
  let traversed = 0;
  for (let index = 1; index < points.length; index += 1) {
    const previous = points[index - 1]!;
    const current = points[index]!;
    const segmentLength = Math.hypot(current.x - previous.x, current.y - previous.y);
    if (traversed + segmentLength >= midpointLength) {
      const remaining = midpointLength - traversed;
      const t = segmentLength < 1e-4 ? 0 : remaining / segmentLength;
      return {
        x: previous.x + (current.x - previous.x) * t,
        y: previous.y + (current.y - previous.y) * t,
      };
    }
    traversed += segmentLength;
  }
  return points.at(-1)!;
}

function offsetPointByPosition(point: EdgePoint, position: Position, distance: number): EdgePoint {
  if (position === Position.Left) {
    return { x: point.x - distance, y: point.y };
  }
  if (position === Position.Right) {
    return { x: point.x + distance, y: point.y };
  }
  if (position === Position.Top) {
    return { x: point.x, y: point.y - distance };
  }
  return { x: point.x, y: point.y + distance };
}

export function applyEdgeBusRouting(
  edges: Edge<SemanticEdgeData>[],
  nodes: Node<FlowNodeData>[],
  layoutDirection: "Horizontal" | "Vertical",
): Edge<SemanticEdgeData>[] {
  if (layoutDirection !== "Horizontal" || edges.length < EDGE_BUS_MIN_EDGES) {
    return edges;
  }

  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const endpointByEdgeId = new Map<string, { source: EdgePoint; target: EdgePoint }>();
  const sourceGroups = new Map<string, string[]>();
  const targetGroups = new Map<string, string[]>();

  for (const edge of edges) {
    const data = edge.data;
    if (!data || data.family !== "flow") {
      continue;
    }

    const sourcePoint = approxHandlePoint(
      nodeById.get(edge.source),
      edge.sourceHandle,
      true,
      layoutDirection,
    );
    const targetPoint = approxHandlePoint(
      nodeById.get(edge.target),
      edge.targetHandle,
      false,
      layoutDirection,
    );

    endpointByEdgeId.set(edge.id, { source: sourcePoint, target: targetPoint });
    const sourceGroupKey = `S:${data.edgeKind}:${edge.source}`;
    const targetGroupKey = `T:${data.edgeKind}:${edge.target}`;
    sourceGroups.set(sourceGroupKey, [...(sourceGroups.get(sourceGroupKey) ?? []), edge.id]);
    targetGroups.set(targetGroupKey, [...(targetGroups.get(targetGroupKey) ?? []), edge.id]);
  }

  const groupById = new Map<
    string,
    { id: string; size: number; trunkX: number; span: number; edgeIds: Set<string> }
  >();

  const registerGroups = (groups: Map<string, string[]>) => {
    for (const [groupId, edgeIds] of groups) {
      if (edgeIds.length < EDGE_BUS_MIN_EDGES) {
        continue;
      }

      const endpoints = edgeIds
        .map((edgeId) => endpointByEdgeId.get(edgeId))
        .filter((value): value is NonNullable<typeof value> => value !== undefined);
      if (endpoints.length < EDGE_BUS_MIN_EDGES) {
        continue;
      }

      const sourceXs = endpoints.map((endpoint) => endpoint.source.x);
      const targetXs = endpoints.map((endpoint) => endpoint.target.x);
      const avgDirection =
        endpoints.reduce((sum, endpoint) => sum + (endpoint.target.x - endpoint.source.x), 0) /
        endpoints.length;

      let trunkX: number | null = null;
      let span = 0;
      if (avgDirection >= 0) {
        const farthestSourceX = Math.max(...sourceXs);
        const nearestTargetX = Math.min(...targetXs);
        const minTrunkX = farthestSourceX + EDGE_BUS_SOURCE_GUTTER;
        const maxTrunkX = nearestTargetX - 8;
        if (maxTrunkX > minTrunkX) {
          trunkX = Math.max(
            minTrunkX,
            Math.min(maxTrunkX, nearestTargetX - EDGE_BUS_TARGET_GUTTER),
          );
          span = nearestTargetX - farthestSourceX;
        }
      } else {
        const farthestSourceX = Math.min(...sourceXs);
        const nearestTargetX = Math.max(...targetXs);
        const minTrunkX = nearestTargetX + 8;
        const maxTrunkX = farthestSourceX - EDGE_BUS_SOURCE_GUTTER;
        if (maxTrunkX > minTrunkX) {
          trunkX = Math.min(
            maxTrunkX,
            Math.max(minTrunkX, nearestTargetX + EDGE_BUS_TARGET_GUTTER),
          );
          span = farthestSourceX - nearestTargetX;
        }
      }

      if (trunkX === null) {
        continue;
      }

      groupById.set(groupId, {
        id: groupId,
        size: edgeIds.length,
        trunkX,
        span,
        edgeIds: new Set(edgeIds),
      });
    }
  };

  registerGroups(sourceGroups);
  registerGroups(targetGroups);

  if (groupById.size === 0) {
    return edges;
  }

  const sourceGroupByEdge = new Map<string, string>();
  const targetGroupByEdge = new Map<string, string>();
  for (const [groupId, edgeIds] of sourceGroups) {
    if (!groupById.has(groupId)) {
      continue;
    }
    for (const edgeId of edgeIds) {
      sourceGroupByEdge.set(edgeId, groupId);
    }
  }
  for (const [groupId, edgeIds] of targetGroups) {
    if (!groupById.has(groupId)) {
      continue;
    }
    for (const edgeId of edgeIds) {
      targetGroupByEdge.set(edgeId, groupId);
    }
  }

  const assignedGroupByEdge = new Map<string, string>();
  for (const edge of edges) {
    const sourceGroupId = sourceGroupByEdge.get(edge.id);
    const targetGroupId = targetGroupByEdge.get(edge.id);
    if (!sourceGroupId && !targetGroupId) {
      continue;
    }
    if (sourceGroupId && !targetGroupId) {
      assignedGroupByEdge.set(edge.id, sourceGroupId);
      continue;
    }
    if (!sourceGroupId && targetGroupId) {
      assignedGroupByEdge.set(edge.id, targetGroupId);
      continue;
    }

    const sourceGroup = groupById.get(sourceGroupId!);
    const targetGroup = groupById.get(targetGroupId!);
    if (!sourceGroup || !targetGroup) {
      continue;
    }

    if (sourceGroup.size !== targetGroup.size) {
      assignedGroupByEdge.set(
        edge.id,
        sourceGroup.size > targetGroup.size ? sourceGroup.id : targetGroup.id,
      );
      continue;
    }

    assignedGroupByEdge.set(
      edge.id,
      sourceGroup.span <= targetGroup.span ? sourceGroup.id : targetGroup.id,
    );
  }

  const assignedCountByGroup = new Map<string, number>();
  for (const groupId of assignedGroupByEdge.values()) {
    assignedCountByGroup.set(groupId, (assignedCountByGroup.get(groupId) ?? 0) + 1);
  }
  for (const [edgeId, groupId] of assignedGroupByEdge) {
    if ((assignedCountByGroup.get(groupId) ?? 0) < EDGE_BUS_MIN_EDGES) {
      assignedGroupByEdge.delete(edgeId);
    }
  }

  return edges.map((edge) => {
    const edgeData = edge.data;
    if (!edgeData) {
      return edge;
    }
    const groupId = assignedGroupByEdge.get(edge.id);
    if (!groupId) {
      return {
        ...edge,
        data: {
          ...edgeData,
          bundleTrunkX: undefined,
        },
      };
    }
    const group = groupById.get(groupId);
    const endpoints = endpointByEdgeId.get(edge.id);
    if (!group || !endpoints) {
      return edge;
    }
    return {
      ...edge,
      data: {
        ...edgeData,
        bundleTrunkX: group.trunkX,
        routePoints: [
          { x: group.trunkX, y: endpoints.source.y },
          { x: group.trunkX, y: endpoints.target.y },
        ],
      },
    };
  });
}

export function isDenseGraph(depth: number, nodeCount: number, edgeCount: number): boolean {
  if (nodeCount <= 48) {
    return false;
  }
  if (depth >= 4) {
    return nodeCount > 90 || edgeCount > 180;
  }
  if (depth >= 3) {
    return nodeCount > 120 || edgeCount > 240;
  }
  return nodeCount > 180 || edgeCount > 360;
}

type StyleSemanticEdgesArgs = {
  edges: Edge<SemanticEdgeData>[];
  centerNodeId: string;
  nodeCount: number;
  depth: number;
  selectedEdgeId: string | null;
  hoveredEdgeId: string | null;
  legendFilterKinds: Set<EdgeKind> | null;
};

export function styleSemanticEdges({
  edges,
  centerNodeId,
  nodeCount,
  depth,
  selectedEdgeId,
  hoveredEdgeId,
  legendFilterKinds,
}: StyleSemanticEdgesArgs): Edge<SemanticEdgeData>[] {
  const denseFocusActive = isDenseGraph(depth, nodeCount, edges.length);
  return edges.map((edge) => {
    const edgeData = edge.data;
    if (!edgeData) {
      return edge;
    }
    const touchesCenter = edge.source === centerNodeId || edge.target === centerNodeId;
    const hasSelectedEdge = selectedEdgeId !== null;
    const isSelectedEdge = selectedEdgeId === edge.id;
    const isHoveredEdge = hoveredEdgeId === edge.id;
    const isFilteredOut = legendFilterKinds !== null && !legendFilterKinds.has(edgeData.edgeKind);
    const certaintyOpacity = Number(edge.style?.opacity ?? 1);
    const baseStroke = Number(edge.style?.strokeWidth ?? 2);
    const hasHoveredEdge = hoveredEdgeId !== null;
    const deemphasized = denseFocusActive && !touchesCenter;
    let interactionOpacity: number;
    let strokeWidth: number;
    if (isFilteredOut) {
      interactionOpacity = hasHoveredEdge ? 0.08 : 0.09;
      strokeWidth = Math.max(1, baseStroke - 0.8);
    } else if (hasHoveredEdge) {
      const dimmed = !isHoveredEdge;
      interactionOpacity = dimmed ? 0.18 : 1;
      strokeWidth = dimmed ? Math.max(1, baseStroke - 0.6) : baseStroke + 0.45;
    } else if (hasSelectedEdge) {
      interactionOpacity = isSelectedEdge ? 1 : 0.2;
      strokeWidth = isSelectedEdge ? baseStroke + 0.45 : Math.max(1, baseStroke - 0.55);
    } else {
      interactionOpacity = deemphasized ? 0.42 : 0.94;
      strokeWidth = deemphasized ? Math.max(1, baseStroke - 0.3) : baseStroke + 0.1;
    }

    const finalOpacity = Math.max(0.04, Math.min(1, interactionOpacity * certaintyOpacity));
    const isFocusHighlighted = !isFilteredOut && (hasHoveredEdge ? isHoveredEdge : isSelectedEdge);
    const baseStrokeColor = String(edge.style?.stroke ?? "currentColor");
    const strokeColor = isFocusHighlighted ? "var(--focus)" : baseStrokeColor;
    const baseMarkerEnd = edge.markerEnd;
    const markerEnd =
      baseMarkerEnd && typeof baseMarkerEnd === "object"
        ? {
            ...baseMarkerEnd,
            color: strokeColor,
          }
        : baseMarkerEnd;
    const sourceEdgeIds = edgeData.sourceEdgeIds.length > 0 ? edgeData.sourceEdgeIds : [edge.id];
    const groupedCount = sourceEdgeIds.length;
    return {
      ...edge,
      markerEnd,
      style: {
        ...edge.style,
        stroke: strokeColor,
        opacity: finalOpacity,
        strokeWidth,
      },
      data: {
        ...edgeData,
        bundleTrunkX: edgeData.bundleTrunkX,
        tooltipLabel: edgeTooltipLabel(edgeData, groupedCount),
        isFocused: isSelectedEdge,
        isHovered: isHoveredEdge,
      },
    };
  });
}

type SemanticEdgePathArgs = {
  sourceX: number;
  sourceY: number;
  targetX: number;
  targetY: number;
  sourcePosition: Position;
  targetPosition: Position;
  markerEnd: string | MarkerType | undefined;
  busTrunkX: number | undefined;
};

export function computeSemanticEdgePath({
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  busTrunkX,
}: SemanticEdgePathArgs): { path: string; labelX: number; labelY: number } {
  const markerWidthCandidate =
    markerEnd && typeof markerEnd === "object"
      ? (markerEnd as { width?: unknown }).width
      : undefined;
  const markerWidth =
    typeof markerWidthCandidate === "number" && Number.isFinite(markerWidthCandidate)
      ? markerWidthCandidate
      : 12;
  const sourcePoint = offsetPointByPosition(
    { x: sourceX, y: sourceY },
    sourcePosition,
    EDGE_SOURCE_OUTSET,
  );
  const targetPoint = offsetPointByPosition(
    { x: targetX, y: targetY },
    targetPosition,
    Math.max(EDGE_TARGET_INSET_BASE, markerWidth * 0.65),
  );

  let path = "";
  let labelX = (sourcePoint.x + targetPoint.x) / 2;
  let labelY = (sourcePoint.y + targetPoint.y) / 2;

  if (typeof busTrunkX === "number" && Number.isFinite(busTrunkX)) {
    const busPoints: EdgePoint[] = [
      sourcePoint,
      { x: busTrunkX, y: sourcePoint.y },
      { x: busTrunkX, y: targetPoint.y },
      targetPoint,
    ];
    const busPath = polylinePath(busPoints);
    if (busPath.length > 0) {
      path = busPath;
      const midpoint = midpointOnPolyline(busPoints);
      labelX = midpoint.x;
      labelY = midpoint.y;
    }
  }

  if (path.length === 0) {
    const [smoothPath, smoothLabelX, smoothLabelY] = getSmoothStepPath({
      sourceX: sourcePoint.x,
      sourceY: sourcePoint.y,
      targetX: targetPoint.x,
      targetY: targetPoint.y,
      sourcePosition,
      targetPosition,
      borderRadius: 18,
      offset: 24,
    });
    path = smoothPath;
    labelX = smoothLabelX;
    labelY = smoothLabelY;
  }

  return { path, labelX, labelY };
}
