import type { EdgeKind } from "../../generated/api";
import type { LayoutElements, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

const MIN_EDGES_FOR_BUNDLING = 12;
const BUNDLE_THRESHOLD = 1;
const MAX_BUNDLE_NODES = 240;
const LANE_BAND_HEIGHT = 76;
const MIN_CENTER_EDGE_SPAN_X = 110;
const MAX_CENTER_BUNDLE_SPAN_X = 1600;
const NON_CENTER_MIN_EDGE_SPAN_X = 120;
const NON_CENTER_SOURCE_BUNDLE_MIN = 2;
const NON_CENTER_BUNDLE_THRESHOLD = 2;
const BRANCH_SEPARATION_STEP_X = 0;
const BRANCH_SEPARATION_MAX_X = 0;
const INBOUND_BRANCH_SEPARATION_STEP_X = 0;
const INBOUND_BRANCH_SEPARATION_MAX_X = 0;
const MIN_OUTWARD_GAP_X = 52;
const NON_CENTER_MIN_OUTWARD_GAP_X = 112;
const BUNDLE_CORRIDOR_PADDING_X = 26;
const CARD_WIDTH_MIN = 228;
const CARD_WIDTH_MAX = 432;
const CARD_CHROME_WIDTH = 112;
const PILL_WIDTH_MIN = 96;
const PILL_WIDTH_MAX = 272;
const PILL_CHROME_WIDTH = 58;
const APPROX_CHAR_WIDTH = 7.25;
const OUTWARD_GAP_X = 124;
const LANE_SWAY_STEP_X = 14;
const LANE_SWAY_MAX_X = 68;
const SIBLING_SWAY_STEP_X = 18;
const SIBLING_SWAY_MAX_X = 88;

type BundleSide = "left" | "right";

type BundleGroup = {
  kind: EdgeKind;
  anchorId: string;
  side: BundleSide;
  laneBand: number;
  edges: RoutedEdgeSpec[];
};

type GroupCandidate = {
  key: string;
  anchorId: string;
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

function certaintyRank(certainty: string | null | undefined): number {
  const normalized = certainty?.toLowerCase();
  if (normalized === "uncertain") {
    return 2;
  }
  if (normalized === "probable") {
    return 1;
  }
  return 0;
}

function mergeCertainty(
  existing: string | null | undefined,
  next: string | null | undefined,
): string | null | undefined {
  return certaintyRank(next) > certaintyRank(existing) ? next : existing;
}

function makeBundleNode(
  id: string,
  x: number,
  y: number,
  xRank: number,
  yRank: number,
): SemanticNodePlacement {
  return {
    id,
    kind: "BUNDLE",
    label: "",
    center: false,
    nodeStyle: "bundle",
    duplicateCount: 1,
    memberCount: 0,
    members: [],
    xRank,
    yRank,
    x,
    y,
    isVirtualBundle: true,
  };
}

function bundleNodeId(
  kind: EdgeKind,
  anchorId: string,
  side: BundleSide,
  laneBand: number,
): string {
  return `bundle:${kind}:${anchorId}:${side}:${laneBand}`;
}

function bandForDelta(deltaY: number): number {
  if (deltaY >= 0) {
    return Math.floor(deltaY / LANE_BAND_HEIGHT);
  }
  return -Math.floor(Math.abs(deltaY) / LANE_BAND_HEIGHT);
}

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
  const side = anchor === "source" ? sourceToTargetSide : oppositeSide(sourceToTargetSide);
  const rawLaneBand =
    anchor === "source"
      ? bandForDelta(targetNode.y - sourceNode.y)
      : bandForDelta(sourceNode.y - targetNode.y);
  let laneBand = normalizeLaneBand(rawLaneBand, anchorId, centerNodeId);
  if (anchorId === centerNodeId && side === "left") {
    laneBand = 0;
  }
  const centerHandleKey =
    anchorId === centerNodeId ? (anchor === "source" ? edge.sourceHandle : edge.targetHandle) : "";
  return {
    key: `${edge.kind}:${anchorId}:${side}:${laneBand}:${centerHandleKey}`,
    anchorId,
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
    key: `mini:${edge.kind}:${edge.source}:right`,
    anchorId: edge.source,
    side: "right",
    laneBand: 0,
  };
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

export function applySharedTrunkBundling(layout: LayoutElements): LayoutElements {
  if (layout.edges.length < MIN_EDGES_FOR_BUNDLING) {
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
      candidateCounts.set(sourceCandidate.key, (candidateCounts.get(sourceCandidate.key) ?? 0) + 1);
      candidateCounts.set(targetCandidate.key, (candidateCounts.get(targetCandidate.key) ?? 0) + 1);
      continue;
    }

    const miniCandidate = nonCenterOutwardSourceCandidate(
      edge,
      sourceNode,
      targetNode,
      layout.centerNodeId,
    );
    if (miniCandidate) {
      candidateCounts.set(miniCandidate.key, (candidateCounts.get(miniCandidate.key) ?? 0) + 1);
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
    }

    if (!selected) {
      passthroughEdges.push(edge);
      continue;
    }

    const key = selected.key;
    const group = groups.get(key) ?? {
      kind: edge.kind,
      anchorId: selected.anchorId,
      side: selected.side,
      laneBand: selected.laneBand,
      edges: [],
    };
    group.edges.push(edge);
    groups.set(key, group);
  }

  if (
    ![...groups.values()].some((group) => {
      const threshold =
        group.anchorId === layout.centerNodeId ? BUNDLE_THRESHOLD : NON_CENTER_BUNDLE_THRESHOLD;
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
    const threshold =
      group.anchorId === layout.centerNodeId ? BUNDLE_THRESHOLD : NON_CENTER_BUNDLE_THRESHOLD;
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
    const counterpartYs = group.edges.map((edge) => {
      const counterpartId = group.side === "right" ? edge.target : edge.source;
      return nodeById.get(counterpartId)?.y ?? anchor.y;
    });
    const counterpartMedianY = median(counterpartYs);
    const minCounterpartY = Math.min(anchor.y, ...counterpartYs) - LANE_BAND_HEIGHT * 0.65;
    const maxCounterpartY = Math.max(anchor.y, ...counterpartYs) + LANE_BAND_HEIGHT * 0.65;
    const laneTargetY = anchor.y + group.laneBand * (LANE_BAND_HEIGHT * 0.55);
    const desiredBundleY = laneTargetY + siblingCenterOffset * 14;
    const bundleY = clamp(
      counterpartMedianY * 0.42 + desiredBundleY * 0.58,
      minCounterpartY,
      maxCounterpartY,
    );
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
    const bundleIdBase = bundleNodeId(group.kind, group.anchorId, group.side, group.laneBand);
    const bundleId = nodeById.has(bundleIdBase)
      ? `${bundleIdBase}:${bundleNodeCount}`
      : bundleIdBase;

    const bundleNode = makeBundleNode(
      bundleId,
      bundleX,
      bundleY,
      anchor.xRank + (group.side === "right" ? 0.25 : -0.25),
      anchor.yRank,
    );
    newNodes.push(bundleNode);
    nodeById.set(bundleId, bundleNode);
    bundleNodeCount += 1;

    const bundleCount = group.edges.reduce((sum, edge) => sum + Math.max(1, edge.multiplicity), 0);
    let mergedCertainty: string | null | undefined = undefined;
    for (const edge of group.edges) {
      mergedCertainty = mergeCertainty(mergedCertainty, edge.certainty);
    }

    const sortedGroupEdges = [...group.edges].sort((left, right) => {
      const leftCounterpartId = group.side === "right" ? left.target : left.source;
      const rightCounterpartId = group.side === "right" ? right.target : right.source;
      const leftY = nodeById.get(leftCounterpartId)?.y ?? bundleY;
      const rightY = nodeById.get(rightCounterpartId)?.y ?? bundleY;
      return leftY - rightY || left.id.localeCompare(right.id);
    });

    if (group.side === "right") {
      newEdges.push({
        id: `trunk:${bundleId}`,
        source: group.anchorId,
        target: bundleId,
        sourceHandle: "source-node",
        targetHandle: "target-node-left",
        kind: group.kind,
        certainty: mergedCertainty,
        multiplicity: 1,
        family: "flow",
        routeKind: "flow-trunk",
        bundleCount,
        trunkCoord: bundleX,
      });

      const branchCenter = (sortedGroupEdges.length - 1) / 2;
      const spreadBranches = group.anchorId === layout.centerNodeId;
      sortedGroupEdges.forEach((edge, idx) => {
        const branchOffset = spreadBranches
          ? clamp(
              (idx - branchCenter) * BRANCH_SEPARATION_STEP_X,
              -BRANCH_SEPARATION_MAX_X,
              BRANCH_SEPARATION_MAX_X,
            )
          : 0;
        newEdges.push({
          ...edge,
          id: `branch:${bundleId}:${edge.id}`,
          source: bundleId,
          target: edge.target,
          sourceHandle: "source-node-right",
          targetHandle: edge.targetHandle,
          routeKind: "flow-branch",
          bundleCount: Math.max(1, edge.multiplicity),
          trunkCoord: bundleX + branchOffset,
        });
      });
    } else {
      newEdges.push({
        id: `trunk:${bundleId}`,
        source: bundleId,
        target: group.anchorId,
        sourceHandle: "source-node-right",
        targetHandle: "target-node",
        kind: group.kind,
        certainty: mergedCertainty,
        multiplicity: 1,
        family: "flow",
        routeKind: "flow-trunk",
        bundleCount,
        trunkCoord: bundleX,
      });

      const branchCenter = (sortedGroupEdges.length - 1) / 2;
      sortedGroupEdges.forEach((edge, idx) => {
        const branchOffset = clamp(
          (idx - branchCenter) * INBOUND_BRANCH_SEPARATION_STEP_X,
          -INBOUND_BRANCH_SEPARATION_MAX_X,
          INBOUND_BRANCH_SEPARATION_MAX_X,
        );
        newEdges.push({
          ...edge,
          id: `branch:${bundleId}:${edge.id}`,
          source: edge.source,
          target: bundleId,
          sourceHandle: edge.sourceHandle,
          targetHandle: "target-node-left",
          routeKind: "flow-branch",
          bundleCount: Math.max(1, edge.multiplicity),
          trunkCoord: bundleX + branchOffset,
        });
      });
    }
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
