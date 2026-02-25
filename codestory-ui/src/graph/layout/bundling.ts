import type { EdgeKind } from "../../generated/api";
import type { LayoutElements, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

const MIN_EDGES_FOR_BUNDLING = 12;
const TRACE_MIN_EDGES_FOR_BUNDLING = 6;
const BUNDLE_THRESHOLD = 3;
const MAX_BUNDLE_NODES = 240;
const LANE_BAND_HEIGHT = 52;
const MIN_CENTER_EDGE_SPAN_X = 110;
const MAX_CENTER_BUNDLE_SPAN_X = 1600;
const NON_CENTER_MIN_EDGE_SPAN_X = 120;
const NON_CENTER_SOURCE_BUNDLE_MIN = 2;
const NON_CENTER_BUNDLE_THRESHOLD = 3;
const MIN_OUTWARD_GAP_X = 56;
const NON_CENTER_MIN_OUTWARD_GAP_X = 72;
const TRACE_MIN_EDGE_SPAN_X = 84;
const TRACE_NON_CENTER_CANDIDATE_MIN = 2;
const TRACE_NON_CENTER_BUNDLE_THRESHOLD = 3;
const BUNDLE_CORRIDOR_PADDING_X = 48;
const CARD_WIDTH_MIN = 228;
const CARD_WIDTH_MAX = 800;
const CARD_CHROME_WIDTH = 112;
const PILL_WIDTH_MIN = 96;
const PILL_WIDTH_MAX = 600;
const PILL_CHROME_WIDTH = 58;
const APPROX_CHAR_WIDTH = 7.25;
const OUTWARD_GAP_X = 72;
const LANE_SWAY_STEP_X = 8;
const LANE_SWAY_MAX_X = 32;
const SIBLING_SWAY_STEP_X = 10;
const SIBLING_SWAY_MAX_X = 40;

type BundleSide = "left" | "right";

type BundleGroup = {
  kind: EdgeKind;
  anchorId: string;
  anchorHandle: string;
  side: BundleSide;
  laneBand: number;
  edges: RoutedEdgeSpec[];
};

type GroupCandidate = {
  key: string;
  anchorId: string;
  anchorHandle: string;
  side: BundleSide;
  laneBand: number;
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

// removed mergeCertainty, makeBundleNode, bundleNodeId

function oppositeSide(side: BundleSide): BundleSide {
  return side === "right" ? "left" : "right";
}

function normalizeLaneBand(rawBand: number, anchorId: string, centerNodeId: string): number {
  if (anchorId === centerNodeId) {
    return 0;
  }
  return rawBand;
}

function candidateForEdge(
  edge: RoutedEdgeSpec,
  sourceNode: SemanticNodePlacement,
  targetNode: SemanticNodePlacement,
  anchor: "source" | "target",
  centerNodeId: string,
): GroupCandidate {
  const sourceToTargetSide: BundleSide = targetNode.x >= sourceNode.x ? "right" : "left";
  const anchorId = anchor === "source" ? edge.source : edge.target;
  const anchorHandle = anchor === "source" ? edge.sourceHandle : edge.targetHandle;
  const side = anchor === "source" ? sourceToTargetSide : oppositeSide(sourceToTargetSide);
  const rawLaneBand =
    anchor === "source"
      ? Math.floor((targetNode.y - sourceNode.y) / LANE_BAND_HEIGHT)
      : Math.floor((sourceNode.y - targetNode.y) / LANE_BAND_HEIGHT);
  let laneBand = normalizeLaneBand(rawLaneBand, anchorId, centerNodeId);
  if (anchorId === centerNodeId && side === "left") {
    laneBand = 0;
  }
  return {
    key: `${edge.kind}:${anchorId}:${anchorHandle}:${side}:${laneBand}`,
    anchorId,
    anchorHandle,
    side,
    laneBand,
  };
}

function textWidth(label: string): number {
  return label.length * APPROX_CHAR_WIDTH;
}

function estimatedNodeWidth(node: SemanticNodePlacement): number {
  if (node.nodeStyle === "card") {
    const longestLabel = Math.max(
      node.label.length,
      ...node.members.map((member) => member.label.length),
    );
    return clamp(
      CARD_CHROME_WIDTH + textWidth("x".repeat(longestLabel)),
      CARD_WIDTH_MIN,
      CARD_WIDTH_MAX,
    );
  }
  if (node.nodeStyle === "pill") {
    const base = PILL_CHROME_WIDTH + textWidth(node.label);
    return clamp(base, PILL_WIDTH_MIN, PILL_WIDTH_MAX);
  }
  return 0;
}

function anchorHandleX(anchor: SemanticNodePlacement, side: BundleSide): number {
  if (side === "left") {
    return anchor.x;
  }
  return anchor.x + estimatedNodeWidth(anchor);
}

function edgeSpanX(sourceNode: SemanticNodePlacement, targetNode: SemanticNodePlacement): number {
  return Math.abs(targetNode.x - sourceNode.x);
}

function isCenterBundleCandidate(
  edge: RoutedEdgeSpec,
  sourceNode: SemanticNodePlacement,
  targetNode: SemanticNodePlacement,
  centerNodeId: string,
): boolean {
  if (edge.source !== centerNodeId && edge.target !== centerNodeId) {
    return false;
  }

  const spanX = edgeSpanX(sourceNode, targetNode);
  return spanX >= MIN_CENTER_EDGE_SPAN_X && spanX <= MAX_CENTER_BUNDLE_SPAN_X;
}

function nonCenterOutwardSourceCandidate(
  edge: RoutedEdgeSpec,
  sourceNode: SemanticNodePlacement,
  targetNode: SemanticNodePlacement,
  centerNodeId: string,
): GroupCandidate | null {
  if (edge.source === centerNodeId || edge.target === centerNodeId) {
    return null;
  }

  // Mini-trunking for non-center fanout: group rightward calls at the source.
  if (targetNode.x <= sourceNode.x + 24) {
    return null;
  }

  const spanX = edgeSpanX(sourceNode, targetNode);
  if (spanX < NON_CENTER_MIN_EDGE_SPAN_X || spanX > MAX_CENTER_BUNDLE_SPAN_X) {
    return null;
  }

  return {
    key: `mini:${edge.kind}:${edge.source}:${edge.sourceHandle}:right`,
    anchorId: edge.source,
    anchorHandle: edge.sourceHandle,
    side: "right",
    laneBand: 0,
  };
}

function isTraceAggressiveCandidate(
  edge: RoutedEdgeSpec,
  sourceNode: SemanticNodePlacement,
  targetNode: SemanticNodePlacement,
  centerNodeId: string,
): boolean {
  if (edge.source === centerNodeId || edge.target === centerNodeId) {
    return false;
  }

  const spanX = edgeSpanX(sourceNode, targetNode);
  return spanX >= TRACE_MIN_EDGE_SPAN_X && spanX <= MAX_CENTER_BUNDLE_SPAN_X;
}

function selectTraceAggressiveCandidate(
  sourceCandidate: GroupCandidate,
  targetCandidate: GroupCandidate,
  candidateCounts: Map<string, number>,
  nodeById: Map<string, SemanticNodePlacement>,
): GroupCandidate | null {
  const sourceCount = candidateCounts.get(sourceCandidate.key) ?? 0;
  const targetCount = candidateCounts.get(targetCandidate.key) ?? 0;
  const strongest = Math.max(sourceCount, targetCount);
  if (strongest < TRACE_NON_CENTER_CANDIDATE_MIN) {
    return null;
  }
  if (sourceCount !== targetCount) {
    return sourceCount > targetCount ? sourceCandidate : targetCandidate;
  }

  const sourceAnchorRank = Math.abs(nodeById.get(sourceCandidate.anchorId)?.xRank ?? 0);
  const targetAnchorRank = Math.abs(nodeById.get(targetCandidate.anchorId)?.xRank ?? 0);
  if (sourceAnchorRank !== targetAnchorRank) {
    return sourceAnchorRank >= targetAnchorRank ? sourceCandidate : targetCandidate;
  }
  return sourceCandidate;
}

function groupBundleThreshold(
  mode: "off" | "overview" | "trace",
  anchorId: string,
  centerNodeId: string,
): number {
  if (anchorId === centerNodeId) {
    return BUNDLE_THRESHOLD;
  }
  if (mode === "trace") {
    return TRACE_NON_CENTER_BUNDLE_THRESHOLD;
  }
  return NON_CENTER_BUNDLE_THRESHOLD;
}

function incrementCount(map: Map<string, number>, key: string): void {
  map.set(key, (map.get(key) ?? 0) + 1);
}

function clampCoordWithinCorridor(
  startX: number,
  endX: number,
  desiredX: number,
  minGapFromStart: number,
  corridorPadding: number,
): number {
  const direction = endX >= startX ? 1 : -1;
  const nearStart = startX + direction * minGapFromStart;
  const nearEnd = endX - direction * corridorPadding;
  const min = Math.min(nearStart, nearEnd);
  const max = Math.max(nearStart, nearEnd);
  if (min > max) {
    return nearStart;
  }
  return clamp(desiredX, min, max);
}

// removed isSourceHandle, isTargetHandle

export function applySharedTrunkBundling(
  layout: LayoutElements,
  mode: "off" | "overview" | "trace" = "overview",
): LayoutElements {
  if (mode === "off") {
    return layout;
  }

  const traceMode = mode === "trace";
  const minEdgesForBundling = traceMode ? TRACE_MIN_EDGES_FOR_BUNDLING : MIN_EDGES_FOR_BUNDLING;
  if (layout.edges.length < minEdgesForBundling) {
    return layout;
  }

  const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
  const candidateCounts = new Map<string, number>();

  for (const edge of layout.edges) {
    if (edge.family !== "flow" || edge.routeKind === "hierarchy") {
      continue;
    }

    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode) {
      continue;
    }
    if (isCenterBundleCandidate(edge, sourceNode, targetNode, layout.centerNodeId)) {
      const sourceCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "source",
        layout.centerNodeId,
      );
      const targetCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "target",
        layout.centerNodeId,
      );
      incrementCount(candidateCounts, sourceCandidate.key);
      incrementCount(candidateCounts, targetCandidate.key);
      continue;
    }

    const miniCandidate = nonCenterOutwardSourceCandidate(
      edge,
      sourceNode,
      targetNode,
      layout.centerNodeId,
    );
    if (miniCandidate) {
      incrementCount(candidateCounts, miniCandidate.key);
    }

    if (
      traceMode &&
      isTraceAggressiveCandidate(edge, sourceNode, targetNode, layout.centerNodeId)
    ) {
      const sourceCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "source",
        layout.centerNodeId,
      );
      const targetCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "target",
        layout.centerNodeId,
      );
      incrementCount(candidateCounts, sourceCandidate.key);
      incrementCount(candidateCounts, targetCandidate.key);
    }
  }

  const passthroughEdges: RoutedEdgeSpec[] = [];
  const groups = new Map<string, BundleGroup>();

  for (const edge of layout.edges) {
    if (edge.family !== "flow" || edge.routeKind === "hierarchy") {
      passthroughEdges.push(edge);
      continue;
    }

    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode) {
      passthroughEdges.push(edge);
      continue;
    }
    let selected: GroupCandidate | null = null;
    if (isCenterBundleCandidate(edge, sourceNode, targetNode, layout.centerNodeId)) {
      const sourceCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "source",
        layout.centerNodeId,
      );
      const targetCandidate = candidateForEdge(
        edge,
        sourceNode,
        targetNode,
        "target",
        layout.centerNodeId,
      );
      const sourceCount = candidateCounts.get(sourceCandidate.key) ?? 0;
      const targetCount = candidateCounts.get(targetCandidate.key) ?? 0;

      if (edge.source === layout.centerNodeId) {
        selected = sourceCandidate;
      } else if (edge.target === layout.centerNodeId) {
        selected = targetCandidate;
      } else if (targetCount !== sourceCount) {
        selected = targetCount > sourceCount ? targetCandidate : sourceCandidate;
      } else {
        const sourceAnchorRank = Math.abs(nodeById.get(sourceCandidate.anchorId)?.xRank ?? 0);
        const targetAnchorRank = Math.abs(nodeById.get(targetCandidate.anchorId)?.xRank ?? 0);
        selected =
          targetAnchorRank > sourceAnchorRank
            ? targetCandidate
            : sourceAnchorRank > targetAnchorRank
              ? sourceCandidate
              : sourceCandidate;
      }
    } else {
      const miniCandidate = nonCenterOutwardSourceCandidate(
        edge,
        sourceNode,
        targetNode,
        layout.centerNodeId,
      );
      if (miniCandidate) {
        const miniCount = candidateCounts.get(miniCandidate.key) ?? 0;
        if (miniCount >= NON_CENTER_SOURCE_BUNDLE_MIN) {
          selected = miniCandidate;
        }
      }

      if (
        traceMode &&
        !selected &&
        isTraceAggressiveCandidate(edge, sourceNode, targetNode, layout.centerNodeId)
      ) {
        const sourceCandidate = candidateForEdge(
          edge,
          sourceNode,
          targetNode,
          "source",
          layout.centerNodeId,
        );
        const targetCandidate = candidateForEdge(
          edge,
          sourceNode,
          targetNode,
          "target",
          layout.centerNodeId,
        );
        selected = selectTraceAggressiveCandidate(
          sourceCandidate,
          targetCandidate,
          candidateCounts,
          nodeById,
        );
      }
    }

    if (!selected) {
      passthroughEdges.push(edge);
      continue;
    }

    const key = selected.key;
    const group = groups.get(key) ?? {
      kind: edge.kind,
      anchorId: selected.anchorId,
      anchorHandle: selected.anchorHandle,
      side: selected.side,
      laneBand: selected.laneBand,
      edges: [],
    };
    group.edges.push(edge);
    groups.set(key, group);
  }

  if (
    ![...groups.values()].some((group) => {
      const threshold = groupBundleThreshold(mode, group.anchorId, layout.centerNodeId);
      return group.edges.length >= threshold;
    })
  ) {
    return layout;
  }

  const newNodes = [...layout.nodes];
  const newEdges = [...passthroughEdges];
  let bundleNodeCount = 0;

  const sortedGroups = [...groups.values()].sort((left, right) => {
    const keyLeft = `${left.kind}:${left.anchorId}:${left.side}:${left.laneBand}`;
    const keyRight = `${right.kind}:${right.anchorId}:${right.side}:${right.laneBand}`;
    return keyLeft.localeCompare(keyRight);
  });
  const siblingCountByAnchor = new Map<string, number>();
  for (const group of sortedGroups) {
    const siblingKey = `${group.kind}:${group.anchorId}:${group.side}`;
    siblingCountByAnchor.set(siblingKey, (siblingCountByAnchor.get(siblingKey) ?? 0) + 1);
  }
  const siblingSeenByAnchor = new Map<string, number>();

  for (const group of sortedGroups) {
    const threshold = groupBundleThreshold(mode, group.anchorId, layout.centerNodeId);
    if (group.edges.length < threshold || bundleNodeCount >= MAX_BUNDLE_NODES) {
      newEdges.push(...group.edges);
      continue;
    }

    const anchor = nodeById.get(group.anchorId);
    if (!anchor) {
      newEdges.push(...group.edges);
      continue;
    }

    const siblingKey = `${group.kind}:${group.anchorId}:${group.side}`;
    const siblingCount = siblingCountByAnchor.get(siblingKey) ?? 1;
    const siblingIndex = siblingSeenByAnchor.get(siblingKey) ?? 0;
    siblingSeenByAnchor.set(siblingKey, siblingIndex + 1);
    const siblingCenterOffset = siblingIndex - (siblingCount - 1) / 2;
    const laneSway = clamp(group.laneBand * LANE_SWAY_STEP_X, -LANE_SWAY_MAX_X, LANE_SWAY_MAX_X);
    const siblingSway = clamp(
      siblingCenterOffset * SIBLING_SWAY_STEP_X,
      -SIBLING_SWAY_MAX_X,
      SIBLING_SWAY_MAX_X,
    );
    const direction = group.side === "right" ? 1 : -1;
    const anchorX = anchorHandleX(anchor, group.side);
    const counterpartHandleXs = group.edges.map((edge) => {
      const counterpartId = group.side === "right" ? edge.target : edge.source;
      const counterpartNode = nodeById.get(counterpartId);
      if (!counterpartNode) {
        return anchorX + direction * OUTWARD_GAP_X;
      }
      return anchorHandleX(counterpartNode, group.side === "right" ? "left" : "right");
    });
    const counterpartMedianX = median(counterpartHandleXs);
    const counterpartDistance = Math.abs(counterpartMedianX - anchorX);
    const minOutwardGap =
      group.anchorId === layout.centerNodeId ? MIN_OUTWARD_GAP_X : NON_CENTER_MIN_OUTWARD_GAP_X;
    const baseGap = clamp(counterpartDistance * 0.35, minOutwardGap, OUTWARD_GAP_X + 52);
    const desiredBundleX = anchorX + direction * (baseGap + laneSway + siblingSway);
    const fallbackCounterpartX = anchorX + direction * (minOutwardGap + BUNDLE_CORRIDOR_PADDING_X);
    const corridorEndX =
      counterpartDistance >= minOutwardGap + BUNDLE_CORRIDOR_PADDING_X
        ? counterpartMedianX
        : fallbackCounterpartX;
    const bundleX = clampCoordWithinCorridor(
      anchorX,
      corridorEndX,
      desiredBundleX,
      minOutwardGap,
      BUNDLE_CORRIDOR_PADDING_X,
    );
    const bundleCount = group.edges.reduce((sum, edge) => sum + Math.max(1, edge.multiplicity), 0);

    // Instead of creating a bundle node, we just route all original edges
    // through the calculated trunk coordinate, effectively creating a
    // unified visual trunk line without synthetic waypoint nodes.
    group.edges.forEach((edge) => {
      newEdges.push({
        ...edge,
        routeKind: "flow-trunk",
        trunkCoord: bundleX,
        bundleCount,
      });
    });
  }

  const sortedNodes = [...newNodes].sort((left, right) => {
    if (left.x !== right.x) {
      return left.x - right.x;
    }
    if (left.y !== right.y) {
      return left.y - right.y;
    }
    return left.id.localeCompare(right.id);
  });
  const sortedEdges = [...newEdges].sort((left, right) => left.id.localeCompare(right.id));

  return {
    nodes: sortedNodes,
    edges: sortedEdges,
    centerNodeId: layout.centerNodeId,
  };
}
