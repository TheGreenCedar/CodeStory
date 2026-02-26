import type { LayoutDirection } from "../../generated/api";
import { PARITY_CONSTANTS } from "./parityConstants";
import type { LayoutElements, RoutePoint, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

type Rect = {
  id: string;
  left: number;
  right: number;
  top: number;
  bottom: number;
};

type AnchorSide = "left" | "right" | "top" | "bottom";
type RouteOrientation = "horizontal" | "vertical";

type EdgeRouteStyle = {
  originOffsetX: number;
  targetOffsetX: number;
  originOffsetY: number;
  targetOffsetY: number;
  verticalOffset: number;
};

export type RouteIntersection = {
  obstacleId: string;
  segmentIndex: number;
  from: RoutePoint;
  to: RoutePoint;
};

export type RouteIntersectionDiagnostics = {
  edgeId: string;
  channelId?: string;
  intersections: RouteIntersection[];
  collisionCount: number;
};

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function snap(value: number): number {
  return Math.round(value / PARITY_CONSTANTS.rasterStep) * PARITY_CONSTANTS.rasterStep;
}

function toVirtualPoint(point: RoutePoint, vertical: boolean): RoutePoint {
  return vertical ? { x: point.y, y: point.x } : point;
}

function fromVirtualPoint(point: RoutePoint, vertical: boolean): RoutePoint {
  return vertical ? { x: point.y, y: point.x } : point;
}

function nodeRect(node: SemanticNodePlacement, padding = 0): Rect {
  return {
    id: node.id,
    left: node.x - padding,
    right: node.x + node.width + padding,
    top: node.y - padding,
    bottom: node.y + node.height + padding,
  };
}

function toVirtualRect(rect: Rect, vertical: boolean): Rect {
  if (!vertical) {
    return rect;
  }
  return {
    id: rect.id,
    left: rect.top,
    right: rect.bottom,
    top: rect.left,
    bottom: rect.right,
  };
}

function memberIdFromHandle(handleId: string): string | null {
  const idx = handleId.indexOf("member-");
  if (idx < 0) {
    return null;
  }
  return handleId.slice(idx + "member-".length);
}

function memberIndex(
  node: SemanticNodePlacement,
  handleId: string,
): { index: number; count: number } | null {
  const memberId = memberIdFromHandle(handleId);
  if (!memberId) {
    return null;
  }
  const idx = node.members.findIndex((member) => member.id === memberId);
  if (idx < 0) {
    return null;
  }
  return { index: idx, count: Math.max(1, node.members.length) };
}

function evenMemberOffset(size: number, index: number, count: number): number {
  if (count <= 1) {
    return size / 2;
  }
  const padding = 18;
  const usable = Math.max(0, size - padding * 2);
  return padding + (index / (count - 1)) * usable;
}

function cardMemberYOffset(node: SemanticNodePlacement, memberId: string): number | null {
  const sectionOrder = [
    {
      visibility: "public" as const,
      members: node.members.filter((m) => m.visibility === "public"),
    },
    {
      visibility: "protected" as const,
      members: node.members.filter((m) => m.visibility === "protected"),
    },
    {
      visibility: "private" as const,
      members: node.members.filter((m) => m.visibility === "private"),
    },
    {
      visibility: "default" as const,
      members: node.members.filter((m) => m.visibility === "default"),
    },
  ];

  let cursor = 74;
  for (const section of sectionOrder) {
    if (section.members.length === 0) {
      continue;
    }
    cursor += 28;
    const idx = section.members.findIndex((member) => member.id === memberId);
    if (idx >= 0) {
      return cursor + idx * 21 + 10;
    }
    cursor += section.members.length * 21;
  }
  return null;
}

function memberAxisOffset(node: SemanticNodePlacement, handleId: string, axis: "x" | "y"): number {
  const ordering = memberIndex(node, handleId);
  if (!ordering) {
    return axis === "x" ? node.width / 2 : node.height / 2;
  }

  const memberId = memberIdFromHandle(handleId);
  if (axis === "y" && memberId && node.nodeStyle === "card") {
    const cardOffset = cardMemberYOffset(node, memberId);
    if (typeof cardOffset === "number") {
      return cardOffset;
    }
  }

  const size = axis === "x" ? node.width : node.height;
  return evenMemberOffset(size, ordering.index, ordering.count);
}

function resolveHandleSide(
  handleId: string,
  role: "source" | "target",
  layoutDirection: LayoutDirection,
): AnchorSide {
  const vertical = layoutDirection === "Vertical";
  const lower = handleId.toLowerCase();

  if (lower.startsWith("source-member-")) {
    return vertical ? "bottom" : "right";
  }
  if (lower.startsWith("target-member-")) {
    return vertical ? "top" : "left";
  }
  if (lower === "source-node") {
    return vertical ? "bottom" : "right";
  }
  if (lower === "target-node") {
    return vertical ? "top" : "left";
  }
  if (lower === "source-node-top") {
    return vertical ? "right" : "top";
  }
  if (lower === "target-node-bottom") {
    return vertical ? "left" : "bottom";
  }

  if (lower.includes("top")) {
    return "top";
  }
  if (lower.includes("bottom")) {
    return "bottom";
  }
  if (lower.includes("left")) {
    return "left";
  }
  if (lower.includes("right")) {
    return "right";
  }

  if (role === "source") {
    return vertical ? "bottom" : "right";
  }
  return vertical ? "top" : "left";
}

function toVirtualSide(side: AnchorSide, vertical: boolean): AnchorSide {
  if (!vertical) {
    return side;
  }
  if (side === "top") {
    return "left";
  }
  if (side === "bottom") {
    return "right";
  }
  if (side === "left") {
    return "top";
  }
  return "bottom";
}

function sideIndex(side: AnchorSide): number {
  if (side === "top") {
    return 0;
  }
  if (side === "right") {
    return 1;
  }
  if (side === "bottom") {
    return 2;
  }
  return 3;
}

function anchorPoint(
  node: SemanticNodePlacement,
  handleId: string,
  role: "source" | "target",
  layoutDirection: LayoutDirection,
): RoutePoint {
  const side = resolveHandleSide(handleId, role, layoutDirection);
  if (side === "top") {
    return { x: snap(node.x + memberAxisOffset(node, handleId, "x")), y: snap(node.y) };
  }
  if (side === "bottom") {
    return {
      x: snap(node.x + memberAxisOffset(node, handleId, "x")),
      y: snap(node.y + node.height),
    };
  }
  if (side === "left") {
    return { x: snap(node.x), y: snap(node.y + memberAxisOffset(node, handleId, "y")) };
  }
  return {
    x: snap(node.x + node.width),
    y: snap(node.y + memberAxisOffset(node, handleId, "y")),
  };
}

function segmentIntersectsRect(a: RoutePoint, b: RoutePoint, rect: Rect): boolean {
  const minX = Math.min(a.x, b.x);
  const maxX = Math.max(a.x, b.x);
  const minY = Math.min(a.y, b.y);
  const maxY = Math.max(a.y, b.y);

  if (a.x === b.x) {
    const intersectsInteriorX = a.x > rect.left && a.x < rect.right;
    const overlapsInteriorY = maxY > rect.top && minY < rect.bottom;
    return intersectsInteriorX && overlapsInteriorY;
  }
  if (a.y === b.y) {
    const intersectsInteriorY = a.y > rect.top && a.y < rect.bottom;
    const overlapsInteriorX = maxX > rect.left && minX < rect.right;
    return intersectsInteriorY && overlapsInteriorX;
  }

  if (maxX <= rect.left || minX >= rect.right || maxY <= rect.top || minY >= rect.bottom) {
    return false;
  }
  const midX = (a.x + b.x) / 2;
  const midY = (a.y + b.y) / 2;
  return midX > rect.left && midX < rect.right && midY > rect.top && midY < rect.bottom;
}

function simplify(points: RoutePoint[]): RoutePoint[] {
  if (points.length <= 2) {
    return points;
  }

  const deduped: RoutePoint[] = [];
  for (const point of points) {
    const prev = deduped[deduped.length - 1];
    if (prev && prev.x === point.x && prev.y === point.y) {
      continue;
    }
    deduped.push({ x: snap(point.x), y: snap(point.y) });
  }

  const simplified: RoutePoint[] = [];
  for (const point of deduped) {
    const prev = simplified[simplified.length - 1];
    const prevPrev = simplified[simplified.length - 2];
    if (prev && prevPrev) {
      const collinear =
        (prevPrev.x === prev.x && prev.x === point.x) ||
        (prevPrev.y === prev.y && prev.y === point.y);
      if (collinear) {
        simplified[simplified.length - 1] = point;
        continue;
      }
    }
    simplified.push(point);
  }
  return simplified;
}

function edgeRouteStyle(edge: RoutedEdgeSpec): EdgeRouteStyle {
  const routing = PARITY_CONSTANTS.routing;
  const baseStyle =
    edge.routeKind === "flow-trunk" || edge.bundleCount > 1
      ? routing.edgeOffsets.bundled
      : routing.edgeOffsets.default;
  const overrides = routing.edgeOffsets.kindOverrides[edge.kind] ?? {};
  return {
    ...baseStyle,
    ...overrides,
  };
}

function routeOrientation(edge: RoutedEdgeSpec): RouteOrientation {
  return edge.family === "hierarchy" ? "vertical" : "horizontal";
}

function pointDistance(left: RoutePoint, right: RoutePoint): number {
  const dx = left.x - right.x;
  const dy = left.y - right.y;
  return Math.hypot(dx, dy);
}

function getPivotPoints(inRect: Rect, outRect: Rect, offset: number): RoutePoint[] {
  const centerX = inRect.left + (inRect.right - inRect.left) / 2 + offset;
  const centerY = inRect.top + (inRect.bottom - inRect.top) / 2 + offset;
  return [
    { x: centerX, y: outRect.top },
    { x: outRect.right, y: centerY },
    { x: centerX, y: outRect.bottom },
    { x: outRect.left, y: centerY },
  ];
}

function offsetSide(point: RoutePoint, sideIdx: number, amount: number): RoutePoint {
  if (sideIdx === 0) {
    return { x: point.x, y: point.y - amount };
  }
  if (sideIdx === 1) {
    return { x: point.x + amount, y: point.y };
  }
  if (sideIdx === 2) {
    return { x: point.x, y: point.y + amount };
  }
  return { x: point.x - amount, y: point.y };
}

function distanceForCombo(dists: Map<string, number>, io: number, it: number): number {
  return dists.get(`${io}:${it}`) ?? Number.POSITIVE_INFINITY;
}

function selectPivotIndices(
  sourcePivots: RoutePoint[],
  targetPivots: RoutePoint[],
  orientation: RouteOrientation,
  forcedSourceSideIdx?: number,
  forcedTargetSideIdx?: number,
): {
  sourceSideIdx: number;
  targetSideIdx: number;
  dists: Map<string, number>;
} {
  let sourceSideIdx = -1;
  let targetSideIdx = -1;
  let bestDistance = Number.POSITIVE_INFINITY;
  const dists = new Map<string, number>();

  for (let sourceIdx = 0; sourceIdx < 4; sourceIdx += 1) {
    for (let targetIdx = 0; targetIdx < 4; targetIdx += 1) {
      if (sourceIdx % 2 !== targetIdx % 2) {
        continue;
      }
      if (orientation === "horizontal" && (sourceIdx % 2 === 0 || targetIdx % 2 === 0)) {
        continue;
      }
      if (orientation === "vertical" && (sourceIdx % 2 === 1 || targetIdx % 2 === 1)) {
        continue;
      }
      if (typeof forcedSourceSideIdx === "number" && sourceIdx !== forcedSourceSideIdx) {
        continue;
      }
      if (typeof forcedTargetSideIdx === "number" && targetIdx !== forcedTargetSideIdx) {
        continue;
      }

      const distance = pointDistance(sourcePivots[sourceIdx]!, targetPivots[targetIdx]!);
      dists.set(`${sourceIdx}:${targetIdx}`, distance);
      if (distance < bestDistance) {
        bestDistance = distance;
        sourceSideIdx = sourceIdx;
        targetSideIdx = targetIdx;
      }
    }
  }

  if (sourceSideIdx >= 0 && targetSideIdx >= 0) {
    return { sourceSideIdx, targetSideIdx, dists };
  }

  const fallbackSource =
    typeof forcedSourceSideIdx === "number"
      ? forcedSourceSideIdx
      : orientation === "horizontal"
        ? 1
        : 2;
  const fallbackTarget =
    typeof forcedTargetSideIdx === "number"
      ? forcedTargetSideIdx
      : orientation === "horizontal"
        ? 3
        : 0;
  return {
    sourceSideIdx: fallbackSource,
    targetSideIdx: fallbackTarget,
    dists,
  };
}

function clampTrunkCoord(sourceX: number, targetX: number, trunkX: number): number {
  const routing = PARITY_CONSTANTS.routing;
  const direction = targetX >= sourceX ? 1 : -1;
  const sourceGate = sourceX + direction * routing.sourceExit;
  const targetGate = targetX - direction * routing.targetEntry;
  const lower = Math.min(sourceGate, targetGate);
  const upper = Math.max(sourceGate, targetGate);
  return snap(clamp(trunkX, lower, upper));
}

function sourcetrailStyledCandidate(
  sourceRect: Rect,
  targetRect: Rect,
  sourceParentRect: Rect,
  targetParentRect: Rect,
  style: EdgeRouteStyle,
  orientation: RouteOrientation,
  sourceSideIdx?: number,
  targetSideIdx?: number,
  preferredTrunkCoord?: number,
): RoutePoint[] {
  const sourceOuterPivots = getPivotPoints(sourceRect, sourceParentRect, style.originOffsetY);
  const targetOuterPivots = getPivotPoints(targetRect, targetParentRect, style.targetOffsetY);
  const {
    sourceSideIdx: selectedSourceIdx,
    targetSideIdx: selectedTargetIdx,
    dists,
  } = selectPivotIndices(
    sourceOuterPivots,
    targetOuterPivots,
    orientation,
    sourceSideIdx,
    targetSideIdx,
  );

  const sourceInnerPivots = getPivotPoints(sourceRect, sourceRect, style.originOffsetY);
  const targetInnerPivots = getPivotPoints(targetRect, targetRect, style.targetOffsetY);

  let sourceIdx = selectedSourceIdx;
  let targetIdx = selectedTargetIdx;

  let targetPoint = { ...targetInnerPivots[targetIdx]! };
  let targetOuter = offsetSide(
    { ...targetOuterPivots[targetIdx]! },
    targetIdx,
    style.targetOffsetX,
  );
  let sourcePoint = { ...sourceInnerPivots[sourceIdx]! };
  let sourceOuter = offsetSide(
    { ...sourceOuterPivots[sourceIdx]! },
    sourceIdx,
    style.originOffsetX,
  );

  const setTargetPoints = (index: number): void => {
    targetPoint = { ...targetInnerPivots[index]! };
    targetOuter = offsetSide({ ...targetOuterPivots[index]! }, index, style.targetOffsetX);
  };

  const setSourcePoints = (index: number): void => {
    sourcePoint = { ...sourceInnerPivots[index]! };
    sourceOuter = offsetSide({ ...sourceOuterPivots[index]! }, index, style.originOffsetX);
  };

  if (targetIdx !== sourceIdx) {
    if (
      (targetIdx === 1 && targetOuter.x < sourceOuter.x) ||
      (sourceIdx === 1 && targetOuter.x > sourceOuter.x)
    ) {
      sourceOuter.x = targetOuter.x;
    } else if (
      (targetIdx === 2 && targetOuter.y < sourceOuter.y) ||
      (sourceIdx === 2 && targetOuter.y > sourceOuter.y)
    ) {
      sourceOuter.y = targetOuter.y;
    } else if (
      (targetIdx === 3 && targetOuter.x < sourceOuter.x) ||
      (sourceIdx === 3 && targetOuter.x > sourceOuter.x) ||
      (targetIdx === 0 && targetOuter.y < sourceOuter.y) ||
      (sourceIdx === 0 && targetOuter.y > sourceOuter.y)
    ) {
      const distSwitchTarget = distanceForCombo(dists, sourceIdx, (targetIdx + 2) % 4);
      const distSwitchSource = distanceForCombo(dists, (sourceIdx + 2) % 4, targetIdx);

      if (distSwitchTarget < distSwitchSource) {
        targetIdx = (targetIdx + 2) % 4;
        setTargetPoints(targetIdx);
      } else {
        sourceIdx = (sourceIdx + 2) % 4;
        setSourcePoints(sourceIdx);
      }
    }
  }

  if (targetIdx % 2 === 1) {
    if (
      targetIdx === sourceIdx &&
      ((targetIdx === 1 && targetOuter.x < sourceOuter.x) ||
        (targetIdx === 3 && targetOuter.x > sourceOuter.x))
    ) {
      targetOuter.x = sourceOuter.x;
    } else {
      sourceOuter.x = targetOuter.x;
    }
  } else if (
    targetIdx === sourceIdx &&
    ((targetIdx === 0 && targetOuter.y > sourceOuter.y) ||
      (targetIdx === 2 && targetOuter.y < sourceOuter.y))
  ) {
    targetOuter.y = sourceOuter.y;
  } else {
    sourceOuter.y = targetOuter.y;
  }

  // Apply styling offset on the axis perpendicular to the connected side.
  if (targetIdx % 2 === 0) {
    let verticalOffset = style.verticalOffset;
    if (sourceOuter.x > targetOuter.x) {
      verticalOffset *= -1;
    }
    targetOuter.x += verticalOffset;
    sourceOuter.x += verticalOffset;
  } else {
    let verticalOffset = style.verticalOffset;
    if (sourceOuter.y > targetOuter.y) {
      verticalOffset *= -1;
    }
    targetOuter.y += verticalOffset;
    sourceOuter.y += verticalOffset;
  }

  const path = [sourcePoint, sourceOuter, targetOuter, targetPoint];

  if (typeof preferredTrunkCoord === "number" && orientation === "horizontal") {
    const trunkX = clampTrunkCoord(sourcePoint.x, targetPoint.x, preferredTrunkCoord);
    if (path[1]) {
      path[1].x = trunkX;
    }
    if (path[2]) {
      path[2].x = trunkX;
    }
  }

  return simplify(path.map((point) => ({ x: snap(point.x), y: snap(point.y) })));
}

function collectPathIntersections(points: RoutePoint[], obstacles: Rect[]): RouteIntersection[] {
  const intersections: RouteIntersection[] = [];
  for (let idx = 1; idx < points.length; idx += 1) {
    const from = points[idx - 1];
    const to = points[idx];
    if (!from || !to) {
      continue;
    }
    for (const obstacle of obstacles) {
      if (segmentIntersectsRect(from, to, obstacle)) {
        intersections.push({
          obstacleId: obstacle.id,
          segmentIndex: idx - 1,
          from: { ...from },
          to: { ...to },
        });
      }
    }
  }
  return intersections;
}

function pathIntersections(points: RoutePoint[], obstacles: Rect[]): number {
  return collectPathIntersections(points, obstacles).length;
}

function routeLength(points: RoutePoint[]): number {
  let total = 0;
  for (let idx = 1; idx < points.length; idx += 1) {
    const from = points[idx - 1];
    const to = points[idx];
    if (!from || !to) {
      continue;
    }
    total += Math.abs(to.x - from.x) + Math.abs(to.y - from.y);
  }
  return total;
}

function trunkPenalty(points: RoutePoint[], trunkCoord: number | undefined): number {
  if (typeof trunkCoord !== "number") {
    return 0;
  }
  const distances = points.slice(1, -1).map((point) => Math.abs(point.x - trunkCoord));
  if (distances.length === 0) {
    return 0;
  }
  return Math.min(...distances) * PARITY_CONSTANTS.routing.trunkPenaltyWeight;
}

function flowCandidates(
  source: RoutePoint,
  target: RoutePoint,
  preferredX: number,
): RoutePoint[][] {
  const routing = PARITY_CONSTANTS.routing;
  const direction = target.x >= source.x ? 1 : -1;
  const midX = (source.x + target.x) / 2;
  const xCandidates = [
    ...new Set(
      [preferredX, midX, preferredX + routing.xDetourStep, preferredX - routing.xDetourStep].map(
        snap,
      ),
    ),
  ];
  const yMid = snap((source.y + target.y) / 2);
  const yCandidates = [
    ...new Set([yMid, yMid + routing.yDetourStep, yMid - routing.yDetourStep].map(snap)),
  ];
  const sourceExit = snap(source.x + direction * routing.sourceExit);
  const targetEntry = snap(target.x - direction * routing.targetEntry);
  const sourceStub = snap(source.x + direction * routing.branchStub);
  const targetStub = snap(target.x - direction * routing.branchStub);

  const candidates: RoutePoint[][] = [];
  candidates.push([
    source,
    { x: sourceStub, y: source.y },
    { x: preferredX, y: source.y },
    { x: preferredX, y: target.y },
    { x: targetStub, y: target.y },
    target,
  ]);

  for (const x of xCandidates) {
    candidates.push([source, { x, y: source.y }, { x, y: target.y }, target]);
  }

  for (const corridorY of yCandidates) {
    candidates.push([
      source,
      { x: sourceExit, y: source.y },
      { x: sourceExit, y: corridorY },
      { x: targetEntry, y: corridorY },
      { x: targetEntry, y: target.y },
      target,
    ]);
  }

  return candidates;
}

function hierarchyCandidates(
  source: RoutePoint,
  target: RoutePoint,
  preferredY: number,
): RoutePoint[][] {
  const yCandidates = [
    ...new Set([preferredY, (source.y + target.y) / 2, preferredY + 64, preferredY - 64].map(snap)),
  ];
  return yCandidates.map((y) => [source, { x: source.x, y }, { x: target.x, y }, target]);
}

function selectBestPath(
  edge: RoutedEdgeSpec,
  candidates: RoutePoint[][],
  obstacles: Rect[],
  trunkCoord: number | undefined,
): RoutePoint[] {
  const scoreWeights = PARITY_CONSTANTS.routing.scoreWeights;
  let bestPath = simplify(candidates[0] ?? []);
  let bestScore = Number.POSITIVE_INFINITY;

  for (let idx = 0; idx < candidates.length; idx += 1) {
    const candidate = candidates[idx];
    if (!candidate) {
      continue;
    }
    const path = simplify(candidate);
    if (path.length < 2) {
      continue;
    }
    const collisions = pathIntersections(path, obstacles);
    const turns = Math.max(0, path.length - 2);
    const length = routeLength(path);
    const weightBias = Math.max(1, edge.channelWeight ?? edge.bundleCount ?? 1);
    const score =
      collisions * scoreWeights.collision +
      turns *
        (scoreWeights.turnBase +
          Math.min(scoreWeights.turnBundleCap, weightBias * scoreWeights.turnBundleScale)) +
      length * scoreWeights.length +
      trunkPenalty(path, trunkCoord) +
      idx * scoreWeights.candidateOrder;
    if (score < bestScore) {
      bestScore = score;
      bestPath = path;
    }
  }

  return bestPath;
}

function edgeObstacles(allObstacles: Rect[], edge: RoutedEdgeSpec): Rect[] {
  return allObstacles.filter(
    (obstacle) => obstacle.id !== edge.source && obstacle.id !== edge.target,
  );
}

export function routeEdgesWithObstacles(
  layout: LayoutElements,
  layoutDirection: LayoutDirection = "Horizontal",
): LayoutElements {
  const routing = PARITY_CONSTANTS.routing;
  const vertical = layoutDirection === "Vertical";
  const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
  const obstacles = layout.nodes.map((node) => nodeRect(node, routing.obstaclePadding));
  const virtualObstacles = obstacles.map((obstacle) => toVirtualRect(obstacle, vertical));

  const edges = layout.edges.map((edge) => {
    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode) {
      return edge;
    }

    const source = anchorPoint(sourceNode, edge.sourceHandle, "source", layoutDirection);
    const target = anchorPoint(targetNode, edge.targetHandle, "target", layoutDirection);
    const sourceVirtual = toVirtualPoint(source, vertical);
    const targetVirtual = toVirtualPoint(target, vertical);
    const sourceVirtualRect = toVirtualRect(nodeRect(sourceNode, 0), vertical);
    const targetVirtualRect = toVirtualRect(nodeRect(targetNode, 0), vertical);
    const sourceVirtualSide = toVirtualSide(
      resolveHandleSide(edge.sourceHandle, "source", layoutDirection),
      vertical,
    );
    const targetVirtualSide = toVirtualSide(
      resolveHandleSide(edge.targetHandle, "target", layoutDirection),
      vertical,
    );
    const preferredX =
      typeof edge.trunkCoord === "number"
        ? snap(edge.trunkCoord)
        : snap((sourceVirtual.x + targetVirtual.x) / 2);
    const preferredY = snap((sourceVirtual.y + targetVirtual.y) / 2);
    const styledCandidate = sourcetrailStyledCandidate(
      sourceVirtualRect,
      targetVirtualRect,
      sourceVirtualRect,
      targetVirtualRect,
      edgeRouteStyle(edge),
      routeOrientation(edge),
      sideIndex(sourceVirtualSide),
      sideIndex(targetVirtualSide),
      edge.routeKind === "flow-trunk" ? preferredX : undefined,
    );
    const fallbackCandidates =
      edge.family === "hierarchy"
        ? hierarchyCandidates(sourceVirtual, targetVirtual, preferredY)
        : flowCandidates(sourceVirtual, targetVirtual, preferredX);
    const candidates = [styledCandidate, ...fallbackCandidates];
    const bestPath = selectBestPath(
      edge,
      candidates,
      edgeObstacles(virtualObstacles, edge),
      edge.routeKind === "flow-trunk" || edge.bundleCount > 1 ? preferredX : undefined,
    );
    if (bestPath.length >= 2) {
      bestPath[0] = sourceVirtual;
      bestPath[bestPath.length - 1] = targetVirtual;
    }

    return {
      ...edge,
      routePoints: bestPath.map((point) => fromVirtualPoint(point, vertical)),
    };
  });

  return {
    ...layout,
    edges,
  };
}

export function routeIntersectsNonEndpointNode(
  edge: RoutedEdgeSpec,
  nodes: SemanticNodePlacement[],
): boolean {
  return routeIntersectionDiagnostics(edge, nodes).collisionCount > 0;
}

export function routeIntersectionDiagnostics(
  edge: RoutedEdgeSpec,
  nodes: SemanticNodePlacement[],
): RouteIntersectionDiagnostics {
  if (edge.routePoints.length < 2) {
    return {
      edgeId: edge.id,
      channelId: edge.channelId,
      intersections: [],
      collisionCount: 0,
    };
  }
  const blocked = nodes
    .filter((node) => node.id !== edge.source && node.id !== edge.target)
    .map((node) => nodeRect(node, 0));
  const intersections = collectPathIntersections(edge.routePoints, blocked);
  return {
    edgeId: edge.id,
    channelId: edge.channelId,
    intersections,
    collisionCount: intersections.length,
  };
}
