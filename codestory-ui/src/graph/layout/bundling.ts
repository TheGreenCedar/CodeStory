import type { EdgeKind, LayoutDirection } from "../../generated/api";
import { PARITY_CONSTANTS } from "./parityConstants";
import type { LayoutElements, RoutePoint, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

type BundleCandidate = {
  key: string;
  kind: EdgeKind;
  pairId: string;
  laneBand: number;
};

type BundleGroup = {
  key: string;
  kind: EdgeKind;
  pairId: string;
  laneBand: number;
  edges: RoutedEdgeSpec[];
};

type VirtualNode = Pick<SemanticNodePlacement, "id" | "x" | "y" | "width" | "height">;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function median(values: number[]): number {
  if (values.length === 0) {
    return 0;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 0) {
    const left = sorted[middle - 1] ?? 0;
    const right = sorted[middle] ?? 0;
    return (left + right) / 2;
  }
  return sorted[middle] ?? 0;
}

function compareDeterministicString(left: string, right: string): number {
  if (left === right) {
    return 0;
  }
  return left < right ? -1 : 1;
}

function nodeLeft(node: VirtualNode): number {
  return node.x;
}

function nodeRight(node: VirtualNode): number {
  return node.x + node.width;
}

function pairIdFor(edge: RoutedEdgeSpec): string {
  const [left, right] = [edge.source, edge.target].sort(compareDeterministicString);
  return `${left}<->${right}`;
}

function groupKeyFor(kind: EdgeKind, pairId: string, laneBand: number): string {
  return `${kind}:${pairId}:band:${laneBand}`;
}

function sideBetween(source: VirtualNode, target: VirtualNode): "left" | "right" {
  return target.x >= source.x ? "right" : "left";
}

function laneBandFor(source: VirtualNode, target: VirtualNode, bandHeight: number): number {
  const deltaY = target.y - source.y;
  const normalized = Math.round(deltaY / bandHeight);
  return clamp(Math.abs(normalized), 0, 12);
}

function toVirtualNode(node: SemanticNodePlacement, vertical: boolean): VirtualNode {
  if (!vertical) {
    return {
      id: node.id,
      x: node.x,
      y: node.y,
      width: node.width,
      height: node.height,
    };
  }
  return {
    id: node.id,
    x: node.y,
    y: node.x,
    width: node.height,
    height: node.width,
  };
}

function fromVirtualPoint(point: RoutePoint, vertical: boolean): RoutePoint {
  return vertical ? { x: point.y, y: point.x } : point;
}

function buildCandidate(
  edge: RoutedEdgeSpec,
  source: VirtualNode,
  target: VirtualNode,
  laneBandHeight: number,
): BundleCandidate {
  const pairId = pairIdFor(edge);
  const laneBand = laneBandFor(source, target, laneBandHeight);
  return {
    key: groupKeyFor(edge.kind, pairId, laneBand),
    kind: edge.kind,
    pairId,
    laneBand,
  };
}

function incrementCount(map: Map<string, number>, key: string): void {
  map.set(key, (map.get(key) ?? 0) + 1);
}

function adaptiveMinGroupSize(depth: number, densityScore: number): number {
  const thresholds = [...PARITY_CONSTANTS.bundling.minGroupSizeThresholds].sort(
    (left, right) => right.minDensity - left.minDensity || right.minDepth - left.minDepth,
  );
  for (const threshold of thresholds) {
    if (depth >= threshold.minDepth || densityScore >= threshold.minDensity) {
      return threshold.minGroupSize;
    }
  }
  return 4;
}

function shouldBundleEdge(edge: RoutedEdgeSpec): boolean {
  if (edge.family !== "flow") {
    return false;
  }
  if (edge.routeKind === "hierarchy") {
    return false;
  }
  return true;
}

function trunkCoordForGroup(
  group: BundleGroup,
  nodeById: Map<string, VirtualNode>,
  densityScore: number,
): number | null {
  const bundling = PARITY_CONSTANTS.bundling;
  const entries: number[] = [];
  const exits: number[] = [];

  for (const edge of group.edges) {
    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) {
      continue;
    }
    const side = sideBetween(source, target);
    entries.push(side === "right" ? nodeRight(source) : nodeLeft(source));
    exits.push(side === "right" ? nodeLeft(target) : nodeRight(target));
  }

  if (entries.length === 0 || exits.length === 0) {
    return null;
  }

  const anchorX = median(entries);
  const counterpartMedianX = median(exits);
  const direction = counterpartMedianX >= anchorX ? 1 : -1;
  const counterpartDistance = Math.abs(counterpartMedianX - anchorX);
  const densityGapBoost = densityScore >= 2.4 ? 22 : densityScore >= 1.8 ? 12 : 0;
  const desiredGap = clamp(
    counterpartDistance * 0.34 + densityGapBoost,
    bundling.minTrunkGap,
    bundling.maxTrunkGap,
  );
  const desiredX = anchorX + direction * desiredGap;

  const corridorMin = Math.min(anchorX, counterpartMedianX) + bundling.corridorPadding;
  const corridorMax = Math.max(anchorX, counterpartMedianX) - bundling.corridorPadding;
  if (corridorMin > corridorMax) {
    return anchorX + direction * bundling.trunkGutter;
  }
  return clamp(desiredX, corridorMin, corridorMax);
}

function sharedTrunkPointsForGroup(
  group: BundleGroup,
  nodeById: Map<string, VirtualNode>,
  trunkCoord: number,
  vertical: boolean,
): RoutePoint[] {
  const bundling = PARITY_CONSTANTS.bundling;
  const ySamples: number[] = [];
  for (const edge of group.edges) {
    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) {
      continue;
    }
    ySamples.push(source.y + source.height / 2);
    ySamples.push(target.y + target.height / 2);
  }

  const fallbackY = 0;
  const minY = Math.min(...(ySamples.length > 0 ? ySamples : [fallbackY]));
  const maxY = Math.max(...(ySamples.length > 0 ? ySamples : [fallbackY]));
  const startY = minY - bundling.sharedTrunkPadding;
  const endY = maxY + bundling.sharedTrunkPadding;

  const virtualPoints = [
    { x: trunkCoord, y: startY },
    { x: trunkCoord, y: endY },
  ];
  return virtualPoints.map((point) => fromVirtualPoint(point, vertical));
}

function rankHandleIds(edges: RoutedEdgeSpec[], role: "source" | "target"): Map<string, number> {
  const uniqueHandles = [
    ...new Set(edges.map((edge) => (role === "source" ? edge.sourceHandle : edge.targetHandle))),
  ].sort(compareDeterministicString);
  return new Map(uniqueHandles.map((handleId, index) => [handleId, index]));
}

export function applyAdaptiveBundling(
  layout: LayoutElements,
  depth: number,
  nodeCount: number,
  edgeCount: number,
  layoutDirection: LayoutDirection = "Horizontal",
): LayoutElements {
  const bundling = PARITY_CONSTANTS.bundling;
  if (layout.edges.length < bundling.minEdgesForBundling) {
    return layout;
  }

  const vertical = layoutDirection === "Vertical";
  const densityScore = depth * 0.45 + nodeCount / 90 + edgeCount / 180;
  const laneBandHeight =
    densityScore >= bundling.laneBandDenseThreshold
      ? bundling.laneBandDenseHeight
      : bundling.laneBandBaseHeight;
  const minGroupSize = adaptiveMinGroupSize(depth, densityScore);

  const nodeById = new Map(layout.nodes.map((node) => [node.id, toVirtualNode(node, vertical)]));
  const candidateCounts = new Map<string, number>();

  for (const edge of layout.edges) {
    if (!shouldBundleEdge(edge)) {
      continue;
    }
    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) {
      continue;
    }
    const candidate = buildCandidate(edge, source, target, laneBandHeight);
    incrementCount(candidateCounts, candidate.key);
  }

  const groups = new Map<string, BundleGroup>();
  const passthrough: RoutedEdgeSpec[] = [];

  for (const edge of layout.edges) {
    if (!shouldBundleEdge(edge)) {
      passthrough.push(edge);
      continue;
    }

    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    if (!source || !target) {
      passthrough.push(edge);
      continue;
    }

    const candidate = buildCandidate(edge, source, target, laneBandHeight);
    const count = candidateCounts.get(candidate.key) ?? 0;
    if (count < 2) {
      passthrough.push(edge);
      continue;
    }

    const group = groups.get(candidate.key) ?? {
      key: candidate.key,
      kind: candidate.kind,
      pairId: candidate.pairId,
      laneBand: candidate.laneBand,
      edges: [],
    };
    group.edges.push(edge);
    groups.set(candidate.key, group);
  }

  if (groups.size === 0) {
    return layout;
  }

  const bundledEdges: RoutedEdgeSpec[] = [];
  for (const group of [...groups.values()].sort((left, right) =>
    compareDeterministicString(left.key, right.key),
  )) {
    if (group.edges.length < minGroupSize) {
      bundledEdges.push(...group.edges);
      continue;
    }

    const trunkCoord = trunkCoordForGroup(group, nodeById, densityScore);
    if (trunkCoord === null) {
      bundledEdges.push(...group.edges);
      continue;
    }

    const channelWeight = group.edges.reduce(
      (sum, edge) => sum + Math.max(1, edge.multiplicity),
      0,
    );
    const channelId = `channel:${group.kind}:${group.pairId}:${group.laneBand}`;
    const sharedTrunkPoints = sharedTrunkPointsForGroup(group, nodeById, trunkCoord, vertical);
    const sourceHandleOrder = rankHandleIds(group.edges, "source");
    const targetHandleOrder = rankHandleIds(group.edges, "target");

    for (const edge of group.edges) {
      bundledEdges.push({
        ...edge,
        routeKind: "flow-trunk",
        trunkCoord,
        bundleCount: channelWeight,
        channelId,
        channelPairId: group.pairId,
        channelWeight,
        sharedTrunkPoints,
        sourceMemberOrder: sourceHandleOrder.get(edge.sourceHandle),
        targetMemberOrder: targetHandleOrder.get(edge.targetHandle),
      });
    }
  }

  const edges = [...passthrough, ...bundledEdges].sort((left, right) =>
    compareDeterministicString(left.id, right.id),
  );
  return {
    ...layout,
    edges,
  };
}

// Backward-compatible alias used by older tests/callers.
export function applySharedTrunkBundling(layout: LayoutElements): LayoutElements {
  return applyAdaptiveBundling(layout, 1, layout.nodes.length, layout.edges.length);
}
