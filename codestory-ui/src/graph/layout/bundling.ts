import type { EdgeKind } from "../../generated/api";
import type { LayoutElements, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

const MIN_EDGES_FOR_ADAPTIVE_BUNDLING = 8;
const LANE_BAND_BASE_HEIGHT = 56;
const LANE_BAND_DENSE_HEIGHT = 74;
const MIN_TRUNK_GAP = 56;
const MAX_TRUNK_GAP = 176;
const CORRIDOR_PADDING = 42;
const TRUNK_GUTTER = 34;

type BundleSide = "left" | "right";

type BundleCandidate = {
  key: string;
  kind: EdgeKind;
  anchorId: string;
  side: BundleSide;
  laneBand: number;
  anchorHandle: string;
};

type BundleGroup = {
  key: string;
  kind: EdgeKind;
  anchorId: string;
  side: BundleSide;
  laneBand: number;
  edges: RoutedEdgeSpec[];
};

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

function nodeLeft(node: SemanticNodePlacement): number {
  return node.x;
}

function nodeRight(node: SemanticNodePlacement): number {
  return node.x + node.width;
}

function sideBetween(source: SemanticNodePlacement, target: SemanticNodePlacement): BundleSide {
  return target.x >= source.x ? "right" : "left";
}

function laneBandFor(
  source: SemanticNodePlacement,
  target: SemanticNodePlacement,
  bandHeight: number,
): number {
  const deltaY = target.y - source.y;
  return Math.round(deltaY / bandHeight);
}

function buildCandidate(
  edge: RoutedEdgeSpec,
  source: SemanticNodePlacement,
  target: SemanticNodePlacement,
  anchor: "source" | "target",
  preserveHandleDetail: boolean,
  laneBandHeight: number,
): BundleCandidate {
  const direction = sideBetween(source, target);
  const anchorId = anchor === "source" ? edge.source : edge.target;
  const anchorHandle = anchor === "source" ? edge.sourceHandle : edge.targetHandle;
  const side = anchor === "source" ? direction : direction === "right" ? "left" : "right";
  const rawBand =
    anchor === "source"
      ? laneBandFor(source, target, laneBandHeight)
      : laneBandFor(target, source, laneBandHeight);
  const laneBand = clamp(rawBand, -12, 12);
  const detailToken = preserveHandleDetail ? anchorHandle : "all";
  return {
    key: `${edge.kind}:${anchorId}:${side}:${laneBand}:${detailToken}`,
    kind: edge.kind,
    anchorId,
    side,
    laneBand,
    anchorHandle,
  };
}

function incrementCount(map: Map<string, number>, key: string): void {
  map.set(key, (map.get(key) ?? 0) + 1);
}

function trunkXForGroup(
  group: BundleGroup,
  nodeById: Map<string, SemanticNodePlacement>,
  densityScore: number,
): number | null {
  const anchor = nodeById.get(group.anchorId);
  if (!anchor) {
    return null;
  }

  const direction = group.side === "right" ? 1 : -1;
  const anchorX = group.side === "right" ? nodeRight(anchor) : nodeLeft(anchor);
  const counterpartXs = group.edges.map((edge) => {
    const counterpartId = group.side === "right" ? edge.target : edge.source;
    const counterpart = nodeById.get(counterpartId);
    if (!counterpart) {
      return anchorX + direction * MIN_TRUNK_GAP;
    }
    return group.side === "right" ? nodeLeft(counterpart) : nodeRight(counterpart);
  });
  const counterpartMedianX = median(counterpartXs);
  const counterpartDistance = Math.abs(counterpartMedianX - anchorX);
  const densityGapBoost = densityScore >= 2.4 ? 22 : densityScore >= 1.8 ? 12 : 0;
  const desiredGap = clamp(
    counterpartDistance * 0.34 + densityGapBoost,
    MIN_TRUNK_GAP,
    MAX_TRUNK_GAP,
  );
  const desiredX = anchorX + direction * desiredGap;

  const corridorMin = Math.min(anchorX, counterpartMedianX) + CORRIDOR_PADDING;
  const corridorMax = Math.max(anchorX, counterpartMedianX) - CORRIDOR_PADDING;
  if (corridorMin > corridorMax) {
    return anchorX + direction * TRUNK_GUTTER;
  }
  return clamp(desiredX, corridorMin, corridorMax);
}

function adaptiveMinGroupSize(depth: number, densityScore: number): number {
  if (depth >= 4 || densityScore >= 2.8) {
    return 2;
  }
  if (depth >= 3 || densityScore >= 2.0) {
    return 3;
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

export function applyAdaptiveBundling(
  layout: LayoutElements,
  depth: number,
  nodeCount: number,
  edgeCount: number,
): LayoutElements {
  if (layout.edges.length < MIN_EDGES_FOR_ADAPTIVE_BUNDLING) {
    return layout;
  }

  const densityScore = depth * 0.45 + nodeCount / 90 + edgeCount / 180;
  const preserveHandleDetail = depth <= 2 && nodeCount <= 84 && edgeCount <= 160;
  const laneBandHeight = densityScore >= 2.2 ? LANE_BAND_DENSE_HEIGHT : LANE_BAND_BASE_HEIGHT;
  const minGroupSize = adaptiveMinGroupSize(depth, densityScore);

  const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
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
    const sourceCandidate = buildCandidate(
      edge,
      source,
      target,
      "source",
      preserveHandleDetail,
      laneBandHeight,
    );
    const targetCandidate = buildCandidate(
      edge,
      source,
      target,
      "target",
      preserveHandleDetail,
      laneBandHeight,
    );
    incrementCount(candidateCounts, sourceCandidate.key);
    incrementCount(candidateCounts, targetCandidate.key);
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

    const sourceCandidate = buildCandidate(
      edge,
      source,
      target,
      "source",
      preserveHandleDetail,
      laneBandHeight,
    );
    const targetCandidate = buildCandidate(
      edge,
      source,
      target,
      "target",
      preserveHandleDetail,
      laneBandHeight,
    );

    const sourceCount = candidateCounts.get(sourceCandidate.key) ?? 0;
    const targetCount = candidateCounts.get(targetCandidate.key) ?? 0;
    const strongest = Math.max(sourceCount, targetCount);
    if (strongest < 2) {
      passthrough.push(edge);
      continue;
    }

    const selected =
      sourceCount > targetCount
        ? sourceCandidate
        : targetCount > sourceCount
          ? targetCandidate
          : Math.abs(source.xRank) >= Math.abs(target.xRank)
            ? sourceCandidate
            : targetCandidate;

    const group = groups.get(selected.key) ?? {
      key: selected.key,
      kind: selected.kind,
      anchorId: selected.anchorId,
      side: selected.side,
      laneBand: selected.laneBand,
      edges: [],
    };
    group.edges.push(edge);
    groups.set(selected.key, group);
  }

  if (groups.size === 0) {
    return layout;
  }

  const bundledEdges: RoutedEdgeSpec[] = [];
  for (const group of [...groups.values()].sort((left, right) =>
    left.key.localeCompare(right.key),
  )) {
    if (group.edges.length < minGroupSize) {
      bundledEdges.push(...group.edges);
      continue;
    }

    const trunkX = trunkXForGroup(group, nodeById, densityScore);
    if (trunkX === null) {
      bundledEdges.push(...group.edges);
      continue;
    }

    const channelWeight = group.edges.reduce(
      (sum, edge) => sum + Math.max(1, edge.multiplicity),
      0,
    );
    const channelId = `channel:${group.kind}:${group.anchorId}:${group.side}:${group.laneBand}`;

    for (const edge of group.edges) {
      bundledEdges.push({
        ...edge,
        routeKind: "flow-trunk",
        trunkCoord: trunkX,
        bundleCount: channelWeight,
        channelId,
        channelWeight,
      });
    }
  }

  const edges = [...passthrough, ...bundledEdges].sort((left, right) =>
    left.id.localeCompare(right.id),
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
