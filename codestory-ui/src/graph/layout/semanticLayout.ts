import type { EdgeKind, GraphResponse } from "../../generated/api";
import {
  CARD_NODE_KINDS,
  PRIVATE_MEMBER_KINDS,
  PUBLIC_MEMBER_KINDS,
  STRUCTURAL_KINDS,
  edgeFamilyForKind,
  type FlowMemberData,
  type LayoutElements,
  type RoutedEdgeSpec,
  type SemanticEdgeFamily,
} from "./types";

export const FLOW_COL_SPACING = 380;
export const HIER_ROW_SPACING = 100;
export const LANE_JITTER_SPACING = 58;

const ROOT_TARGET_Y = 200;
const DENSE_LAYER_ROW_TARGET = 9;
const DENSE_LAYER_MAX_COLUMNS = 4;
const DENSE_LAYER_COL_SPACING_MIN = 300;
const RANK_ONE_CENTER_CLEARANCE_X = 80;
const DEPTH_TWO_ROW_TARGET = 7;
const DEPTH_TWO_MIN_COLUMNS = 2;
const DEPTH_TWO_OUTWARD_OFFSET_X = 32;
const OUTER_LAYER_OUTWARD_STEP_X = 16;
const DEPTH_TWO_ROW_SPACING_BOOST = 8;
const DENSE_LAYER_COL_GAP = 48;
const DENSE_LAYER_STAGGER_Y = 28;
const MIN_ROW_SPACING = 56;
const CARD_WIDTH_MIN = 228;
const CARD_WIDTH_MAX = 432;
const CARD_CHROME_WIDTH = 112;
const CARD_HEIGHT_MIN = 110;
const CARD_HEIGHT_MAX = 560;
const PILL_WIDTH_MIN = 96;
const PILL_WIDTH_MAX = 560;
const PILL_CHROME_WIDTH = 72;
const PILL_HEIGHT = 34;
const APPROX_CHAR_WIDTH = 7.25;
const RANK_MIN_VERTICAL_GAP = 26;
const RASTER_STEP = 8;

type FoldedEdge = {
  id: string;
  sourceEdgeIds: string[];
  source: string;
  target: string;
  kind: EdgeKind;
  certainty: string | null | undefined;
  multiplicity: number;
  sourceHandle: string;
  targetHandle: string;
  family: SemanticEdgeFamily;
};

type GraphNodeLike = GraphResponse["nodes"][number];
type GraphEdgeLike = GraphResponse["edges"][number];

function inferMemberVisibility(
  kind: string,
  label: string,
  explicitAccess?: GraphNodeLike["member_access"] | null,
): "public" | "protected" | "private" | "default" {
  if (explicitAccess === "Public") {
    return "public";
  }
  if (explicitAccess === "Protected") {
    return "protected";
  }
  if (explicitAccess === "Private") {
    return "private";
  }
  if (explicitAccess === "Default") {
    return "default";
  }
  if (PRIVATE_MEMBER_KINDS.has(kind)) {
    return "private";
  }
  if (PUBLIC_MEMBER_KINDS.has(kind)) {
    return "public";
  }
  if (/^_|_$|^m_[A-Za-z0-9]/.test(label)) {
    return "private";
  }
  return "public";
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

function dedupeKeyForNode(
  kind: string,
  label: string,
  depth: number,
  isCenter: boolean,
): string | null {
  if (isCenter) {
    return null;
  }
  if (CARD_NODE_KINDS.has(kind)) {
    return `${kind}:${label.toLowerCase()}`;
  }
  return `${kind}:${label.toLowerCase()}:${depth}`;
}

function median(values: number[]): number {
  if (values.length === 0) {
    return 0;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const mid = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 0) {
    const left = sorted[mid - 1] ?? 0;
    const right = sorted[mid] ?? 0;
    return (left + right) / 2;
  }
  return sorted[mid] ?? 0;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function textWidth(label: string): number {
  return label.length * APPROX_CHAR_WIDTH;
}

function bfsDistances(adjacency: Map<string, Set<string>>, start: string): Map<string, number> {
  const dist = new Map<string, number>();
  const queue: string[] = [start];
  dist.set(start, 0);

  for (let idx = 0; idx < queue.length; idx += 1) {
    const current = queue[idx];
    if (!current) {
      continue;
    }
    const currentDepth = dist.get(current) ?? 0;
    for (const next of adjacency.get(current) ?? []) {
      if (dist.has(next)) {
        continue;
      }
      dist.set(next, currentDepth + 1);
      queue.push(next);
    }
  }

  return dist;
}

function nodeOrderIndex(layerNodeIds: string[]): Map<string, number> {
  return new Map(layerNodeIds.map((nodeId, index) => [nodeId, index]));
}

function sortLayerByNeighborMedian(
  layerNodeIds: string[],
  referenceOrder: Map<string, number>,
  neighborsByNode: Map<string, Set<string>>,
  labelsByNode: Map<string, string>,
): string[] {
  const fallbackOrder = nodeOrderIndex(layerNodeIds);
  const medianByNode = new Map<string, number | null>();

  for (const nodeId of layerNodeIds) {
    const neighborIndexes = [...(neighborsByNode.get(nodeId) ?? [])]
      .map((neighborId) => referenceOrder.get(neighborId))
      .filter((index): index is number => typeof index === "number")
      .sort((left, right) => left - right);

    if (neighborIndexes.length === 0) {
      medianByNode.set(nodeId, null);
      continue;
    }

    medianByNode.set(nodeId, median(neighborIndexes));
  }

  return [...layerNodeIds].sort((left, right) => {
    const leftMedian = medianByNode.get(left) ?? null;
    const rightMedian = medianByNode.get(right) ?? null;

    if (leftMedian === null && rightMedian === null) {
      return (
        (fallbackOrder.get(left) ?? 0) - (fallbackOrder.get(right) ?? 0) ||
        (labelsByNode.get(left) ?? left).localeCompare(labelsByNode.get(right) ?? right)
      );
    }
    if (leftMedian === null) {
      return 1;
    }
    if (rightMedian === null) {
      return -1;
    }
    if (Math.abs(leftMedian - rightMedian) > 0.0001) {
      return leftMedian - rightMedian;
    }
    return (
      (fallbackOrder.get(left) ?? 0) - (fallbackOrder.get(right) ?? 0) ||
      (labelsByNode.get(left) ?? left).localeCompare(labelsByNode.get(right) ?? right)
    );
  });
}

function columnCountForLayer(layerSize: number, rank: number): number {
  if (rank === 0) {
    return 1;
  }

  const absRank = Math.abs(rank);
  if (absRank === 1) {
    return 1;
  }

  if (absRank === 2) {
    if (layerSize <= DEPTH_TWO_ROW_TARGET + 1) {
      return 1;
    }

    const preferred = Math.ceil(layerSize / DEPTH_TWO_ROW_TARGET);
    return Math.min(DENSE_LAYER_MAX_COLUMNS, Math.max(DEPTH_TWO_MIN_COLUMNS, preferred));
  }

  if (layerSize <= DENSE_LAYER_ROW_TARGET) {
    return 1;
  }

  return Math.min(DENSE_LAYER_MAX_COLUMNS, Math.ceil(layerSize / DENSE_LAYER_ROW_TARGET));
}

function centerOutColumnOrder(columnCount: number): number[] {
  const center = (columnCount - 1) / 2;
  return Array.from({ length: columnCount }, (_, idx) => idx).sort((left, right) => {
    const leftDist = Math.abs(left - center);
    const rightDist = Math.abs(right - center);
    if (leftDist !== rightDist) {
      return leftDist - rightDist;
    }
    return left - right;
  });
}

function bucketLayerNodes(layerNodeIds: string[], columnCount: number, rank: number): string[][] {
  if (columnCount <= 1) {
    return [layerNodeIds];
  }

  const columns = Array.from({ length: columnCount }, () => [] as string[]);
  if (Math.abs(rank) === 2) {
    const visitOrder = centerOutColumnOrder(columnCount);
    for (let idx = 0; idx < layerNodeIds.length; idx += 1) {
      const nodeId = layerNodeIds[idx];
      if (!nodeId) {
        continue;
      }
      const columnIdx = visitOrder[idx % visitOrder.length] ?? 0;
      columns[columnIdx]?.push(nodeId);
    }
    return columns.filter((columnNodes) => columnNodes.length > 0);
  }

  for (let idx = 0; idx < layerNodeIds.length; idx += 1) {
    const nodeId = layerNodeIds[idx];
    if (!nodeId) {
      continue;
    }

    const row = Math.floor(idx / columnCount);
    const offset = idx % columnCount;
    const columnIdx = row % 2 === 0 ? offset : columnCount - 1 - offset;
    columns[columnIdx]?.push(nodeId);
  }

  return columns.filter((columnNodes) => columnNodes.length > 0);
}

function estimatedNodeWidth(kind: string, label: string, members: FlowMemberData[]): number {
  if (CARD_NODE_KINDS.has(kind)) {
    const longestLabel = Math.max(label.length, ...members.map((member) => member.label.length));
    return clamp(
      CARD_CHROME_WIDTH + textWidth("x".repeat(longestLabel)),
      CARD_WIDTH_MIN,
      CARD_WIDTH_MAX,
    );
  }

  const pillWidth = PILL_CHROME_WIDTH + textWidth(label);
  return clamp(pillWidth, PILL_WIDTH_MIN, PILL_WIDTH_MAX);
}

function estimatedNodeHeight(kind: string, members: FlowMemberData[]): number {
  if (!CARD_NODE_KINDS.has(kind)) {
    return PILL_HEIGHT;
  }

  const publicCount = members.filter((member) => member.visibility === "public").length;
  const protectedCount = members.filter((member) => member.visibility === "protected").length;
  const privateCount = members.filter((member) => member.visibility === "private").length;
  const defaultCount = members.filter((member) => member.visibility === "default").length;
  const sectionCount = [publicCount, protectedCount, privateCount, defaultCount].filter(
    (count) => count > 0,
  ).length;
  const effectiveSections = sectionCount === 0 ? 1 : sectionCount;
  return clamp(
    74 + effectiveSections * 28 + Math.max(1, members.length) * 21,
    CARD_HEIGHT_MIN,
    CARD_HEIGHT_MAX,
  );
}

function snapToRaster(value: number, step = RASTER_STEP): number {
  return Math.round(value / step) * step;
}

type MemberExtraction = {
  memberHostById: Map<string, string>;
  membersByHost: Map<string, FlowMemberData[]>;
  syntheticHosts: GraphNodeLike[];
};

function syntheticHostId(hostLabel: string): string {
  const slug = hostLabel
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `__synthetic_host__${slug.length > 0 ? slug : "anonymous"}`;
}

function extractMembers(graph: GraphResponse): MemberExtraction {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const memberHostById = new Map<string, string>();
  const membersByHost = new Map<string, FlowMemberData[]>();
  const syntheticHostsById = new Map<string, GraphNodeLike>();

  for (const edge of graph.edges) {
    if (edge.kind !== "MEMBER") {
      continue;
    }

    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode) {
      continue;
    }

    const sourceIsStructural = STRUCTURAL_KINDS.has(sourceNode.kind);
    const targetIsStructural = STRUCTURAL_KINDS.has(targetNode.kind);

    let memberId: string | null = null;
    let hostId: string | null = null;
    if (sourceIsStructural && !targetIsStructural) {
      memberId = targetNode.id;
      hostId = sourceNode.id;
    } else if (!sourceIsStructural && targetIsStructural) {
      memberId = sourceNode.id;
      hostId = targetNode.id;
    }

    if (!memberId || !hostId) {
      continue;
    }

    memberHostById.set(memberId, hostId);
    const hostMembers = membersByHost.get(hostId) ?? [];
    if (!hostMembers.some((member) => member.id === memberId)) {
      const memberNode = nodeById.get(memberId);
      const memberLabel = memberNode?.label ?? memberId;
      const memberKind = memberNode?.kind ?? "UNKNOWN";
      hostMembers.push({
        id: memberId,
        label: memberLabel,
        kind: memberKind,
        visibility: inferMemberVisibility(memberKind, memberLabel, memberNode?.member_access),
      });
      membersByHost.set(hostId, hostMembers);
    }
  }

  // Fallback grouping: attach detached `Type::member` symbols to visible structural hosts.
  const hostIdsByLabel = new Map<string, string>();
  for (const node of graph.nodes) {
    if (STRUCTURAL_KINDS.has(node.kind)) {
      hostIdsByLabel.set(node.label, node.id);
    }
  }

  for (const node of graph.nodes) {
    if (STRUCTURAL_KINDS.has(node.kind) || memberHostById.has(node.id)) {
      continue;
    }
    const separatorIdx = node.label.indexOf("::");
    if (separatorIdx <= 0) {
      continue;
    }
    const hostLabel = node.label.slice(0, separatorIdx);
    let hostId = hostIdsByLabel.get(hostLabel);
    if (!hostId) {
      hostId = syntheticHostId(hostLabel);
      hostIdsByLabel.set(hostLabel, hostId);
      if (!syntheticHostsById.has(hostId)) {
        syntheticHostsById.set(hostId, {
          id: hostId,
          label: hostLabel,
          kind: "CLASS",
          depth: Math.max(1, node.depth - 1),
          badge_visible_members: null,
          badge_total_members: null,
          merged_symbol_examples: [],
        });
      }
    }

    memberHostById.set(node.id, hostId);
    const hostMembers = membersByHost.get(hostId) ?? [];
    if (!hostMembers.some((member) => member.id === node.id)) {
      hostMembers.push({
        id: node.id,
        label: node.label,
        kind: node.kind,
        visibility: inferMemberVisibility(node.kind, node.label, node.member_access),
      });
      membersByHost.set(hostId, hostMembers);
    }
  }

  return {
    memberHostById,
    membersByHost,
    syntheticHosts: [...syntheticHostsById.values()],
  };
}

function foldEdges(
  nodes: GraphNodeLike[],
  edges: GraphEdgeLike[],
  centerHostNodeId: string,
  memberHostById: Map<string, string>,
  signedDepthByNode: Map<string, number>,
): {
  foldedEdges: FoldedEdge[];
  canonicalNodeById: Map<string, string>;
  duplicateCountByCanonical: Map<string, number>;
  mergedIdsByCanonical: Map<string, string[]>;
} {
  const canonicalNodeById = new Map<string, string>();
  const canonicalNodeByKey = new Map<string, string>();
  const duplicateCountByCanonical = new Map<string, number>();
  const mergedIdsByCanonical = new Map<string, string[]>();

  for (const node of nodes) {
    if (memberHostById.has(node.id)) {
      continue;
    }

    const depth = signedDepthByNode.get(node.id) ?? Math.max(1, node.depth);
    const key = dedupeKeyForNode(node.kind, node.label, depth, node.id === centerHostNodeId);
    const canonicalId = key ? (canonicalNodeByKey.get(key) ?? node.id) : node.id;
    if (key && !canonicalNodeByKey.has(key)) {
      canonicalNodeByKey.set(key, canonicalId);
    }

    canonicalNodeById.set(node.id, canonicalId);
    duplicateCountByCanonical.set(
      canonicalId,
      (duplicateCountByCanonical.get(canonicalId) ?? 0) + 1,
    );
    const mergedIds = mergedIdsByCanonical.get(canonicalId) ?? [];
    mergedIds.push(node.id);
    mergedIdsByCanonical.set(canonicalId, mergedIds);
  }

  const folded = new Map<string, FoldedEdge>();
  for (const edge of edges) {
    if (edge.kind === "MEMBER") {
      continue;
    }

    const family = edgeFamilyForKind(edge.kind);
    const sourceHost = memberHostById.get(edge.source);
    const targetHost = memberHostById.get(edge.target);
    const sourceNodeId = sourceHost ?? edge.source;
    const targetNodeId = targetHost ?? edge.target;
    const source = canonicalNodeById.get(sourceNodeId) ?? sourceNodeId;
    const target = canonicalNodeById.get(targetNodeId) ?? targetNodeId;

    if (source === target) {
      continue;
    }

    const sourceHandle = sourceHost
      ? `source-member-${edge.source}`
      : family === "hierarchy"
        ? "source-node-top"
        : "source-node";
    const targetHandle = targetHost
      ? `target-member-${edge.target}`
      : family === "hierarchy"
        ? "target-node-bottom"
        : "target-node";

    const key = `${edge.kind}:${source}:${sourceHandle}:${target}:${targetHandle}`;
    const existing = folded.get(key);
    if (!existing) {
      folded.set(key, {
        id: key,
        sourceEdgeIds: [edge.id],
        source,
        target,
        kind: edge.kind,
        certainty: edge.certainty,
        multiplicity: 1,
        sourceHandle,
        targetHandle,
        family,
      });
      continue;
    }

    existing.multiplicity += 1;
    existing.sourceEdgeIds.push(edge.id);
    existing.certainty = mergeCertainty(existing.certainty, edge.certainty);
  }

  return {
    foldedEdges: [...folded.values()],
    canonicalNodeById,
    duplicateCountByCanonical,
    mergedIdsByCanonical,
  };
}

function computeSignedDepthByNode(
  nodes: GraphNodeLike[],
  edges: GraphEdgeLike[],
  centerHostNodeId: string,
): Map<string, number> {
  const directionBiasByNode = new Map<string, number>();
  for (const edge of edges) {
    if (edge.kind === "MEMBER") {
      continue;
    }

    if (edge.source === centerHostNodeId && edge.target !== centerHostNodeId) {
      directionBiasByNode.set(edge.target, (directionBiasByNode.get(edge.target) ?? 0) + 1);
    }
    if (edge.target === centerHostNodeId && edge.source !== centerHostNodeId) {
      directionBiasByNode.set(edge.source, (directionBiasByNode.get(edge.source) ?? 0) - 1);
    }
  }

  const signedDepthByNode = new Map<string, number>();
  for (const node of nodes) {
    if (node.id === centerHostNodeId) {
      signedDepthByNode.set(node.id, 0);
      continue;
    }

    const baseDepth = Math.max(1, node.depth);
    const bias = directionBiasByNode.get(node.id) ?? 0;
    signedDepthByNode.set(node.id, bias < 0 ? -baseDepth : baseDepth);
  }
  return signedDepthByNode;
}

function applyVirtualRankHints(
  yRank: Map<string, number>,
  xRank: Map<string, number>,
  edges: FoldedEdge[],
): void {
  const longEdges = edges.filter((edge) => {
    if (edge.family !== "flow") {
      return false;
    }
    return Math.abs((xRank.get(edge.target) ?? 0) - (xRank.get(edge.source) ?? 0)) > 1;
  });
  if (longEdges.length === 0) {
    return;
  }

  for (let pass = 0; pass < 3; pass += 1) {
    for (const edge of longEdges) {
      const sourceY = yRank.get(edge.source) ?? 0;
      const targetY = yRank.get(edge.target) ?? 0;
      const blended = targetY * 0.65 + sourceY * 0.35;
      yRank.set(edge.target, blended);
    }
  }
}

function compactRankPositions(
  positionsByNode: Map<string, { x: number; y: number }>,
  xRank: Map<string, number>,
  heightByNode: Map<string, number>,
): void {
  const byRank = new Map<number, string[]>();
  for (const nodeId of positionsByNode.keys()) {
    const rank = xRank.get(nodeId) ?? 0;
    const bucket = byRank.get(rank) ?? [];
    bucket.push(nodeId);
    byRank.set(rank, bucket);
  }

  for (const [rank, nodeIds] of byRank) {
    nodeIds.sort((left, right) => {
      const leftY = positionsByNode.get(left)?.y ?? 0;
      const rightY = positionsByNode.get(right)?.y ?? 0;
      return leftY - rightY;
    });

    const minGap = Math.abs(rank) >= 2 ? RANK_MIN_VERTICAL_GAP + 4 : RANK_MIN_VERTICAL_GAP;
    const originalTops = nodeIds.map((nodeId) => positionsByNode.get(nodeId)?.y ?? 0);
    const originalTop = originalTops[0] ?? 0;
    const originalBottom = originalTops.at(-1) ?? originalTop;
    const originalCenter = originalTops.length > 0 ? (originalTop + originalBottom) / 2 : 0;

    let lastBottom = Number.NEGATIVE_INFINITY;
    for (const nodeId of nodeIds) {
      const position = positionsByNode.get(nodeId);
      if (!position) {
        continue;
      }
      const height = heightByNode.get(nodeId) ?? PILL_HEIGHT;
      const minTop = lastBottom + minGap;
      if (position.y < minTop) {
        position.y = minTop;
        positionsByNode.set(nodeId, position);
      }
      lastBottom = position.y + height;
    }

    const newTops = nodeIds.map((nodeId) => positionsByNode.get(nodeId)?.y ?? 0);
    const newTop = newTops[0] ?? 0;
    const newBottom = newTops.at(-1) ?? newTop;
    const newCenter = newTops.length > 0 ? (newTop + newBottom) / 2 : 0;
    const correction = originalCenter - newCenter;
    for (const nodeId of nodeIds) {
      const position = positionsByNode.get(nodeId);
      if (!position) {
        continue;
      }
      position.y += correction;
      positionsByNode.set(nodeId, position);
    }
  }
}

function smoothRankPositions(
  layers: string[][],
  sortedXRanks: number[],
  positionsByNode: Map<string, { x: number; y: number }>,
  xRank: Map<string, number>,
  neighborsByNode: Map<string, Set<string>>,
  heightByNode: Map<string, number>,
): void {
  const layerByRank = new Map<number, string[]>();
  for (let idx = 0; idx < sortedXRanks.length; idx += 1) {
    const rank = sortedXRanks[idx];
    const layer = layers[idx];
    if (typeof rank !== "number" || !layer) {
      continue;
    }
    layerByRank.set(rank, layer);
  }

  const smoothDirection = (direction: "forward" | "backward") => {
    const rankList = [...sortedXRanks];
    if (direction === "backward") {
      rankList.reverse();
    }

    for (const rank of rankList) {
      const layer = layerByRank.get(rank);
      if (!layer || layer.length <= 1) {
        continue;
      }

      const refRank = direction === "forward" ? rank - 1 : rank + 1;
      const refLayer = layerByRank.get(refRank);
      if (!refLayer || refLayer.length === 0) {
        continue;
      }
      const refSet = new Set(refLayer);

      for (const nodeId of layer) {
        const position = positionsByNode.get(nodeId);
        if (!position) {
          continue;
        }
        const neighborYs = [...(neighborsByNode.get(nodeId) ?? [])]
          .filter((neighborId) => refSet.has(neighborId))
          .map((neighborId) => positionsByNode.get(neighborId)?.y ?? 0);
        if (neighborYs.length === 0) {
          continue;
        }
        const desiredY = median(neighborYs);
        position.y = position.y * 0.68 + desiredY * 0.32;
        positionsByNode.set(nodeId, position);
      }
    }

    compactRankPositions(positionsByNode, xRank, heightByNode);
  };

  smoothDirection("forward");
  smoothDirection("backward");
}

function toRoutedEdgeSpecs(edges: FoldedEdge[]): RoutedEdgeSpec[] {
  return edges
    .map((edge) => {
      const routeKind: RoutedEdgeSpec["routeKind"] =
        edge.family === "hierarchy" ? "hierarchy" : "direct";

      return {
        id: edge.id,
        sourceEdgeIds: [...edge.sourceEdgeIds],
        source: edge.source,
        target: edge.target,
        sourceHandle: edge.sourceHandle,
        targetHandle: edge.targetHandle,
        kind: edge.kind,
        certainty: edge.certainty,
        multiplicity: edge.multiplicity,
        family: edge.family,
        routeKind,
        bundleCount: edge.multiplicity,
        routePoints: [],
      };
    })
    .sort((left, right) => left.id.localeCompare(right.id));
}

export function buildSemanticLayout(graph: GraphResponse): LayoutElements {
  const { memberHostById, membersByHost, syntheticHosts } = extractMembers(graph);
  const allNodes = [...graph.nodes, ...syntheticHosts];
  const nodeById = new Map(allNodes.map((node) => [node.id, node]));
  const labelsByNode = new Map(allNodes.map((node) => [node.id, node.label]));
  const centerHostNodeId = memberHostById.get(graph.center_id) ?? graph.center_id;
  const signedDepthByNode = computeSignedDepthByNode(allNodes, graph.edges, centerHostNodeId);

  const { foldedEdges, canonicalNodeById, duplicateCountByCanonical, mergedIdsByCanonical } =
    foldEdges(allNodes, graph.edges, centerHostNodeId, memberHostById, signedDepthByNode);
  const membersByCanonical = new Map<string, FlowMemberData[]>();
  for (const [nodeId, canonicalId] of canonicalNodeById) {
    const members = membersByHost.get(nodeId);
    if (!members || members.length === 0) {
      continue;
    }
    const merged = membersByCanonical.get(canonicalId) ?? [];
    const seen = new Set(merged.map((member) => member.id));
    for (const member of members) {
      if (!seen.has(member.id)) {
        merged.push(member);
        seen.add(member.id);
      }
    }
    membersByCanonical.set(canonicalId, merged);
  }

  const canonicalNodeIds = [...new Set(canonicalNodeById.values())];
  const centerNodeId = canonicalNodeById.get(centerHostNodeId) ?? centerHostNodeId;
  const estimatedWidthByNode = new Map<string, number>();
  const estimatedHeightByNode = new Map<string, number>();
  for (const nodeId of canonicalNodeIds) {
    const node = nodeById.get(nodeId);
    if (!node) {
      continue;
    }
    const members = membersByCanonical.get(nodeId) ?? [];
    estimatedWidthByNode.set(nodeId, estimatedNodeWidth(node.kind, node.label, members));
    estimatedHeightByNode.set(nodeId, estimatedNodeHeight(node.kind, members));
  }

  const outgoingAdj = new Map<string, Set<string>>();
  const incomingAdj = new Map<string, Set<string>>();
  for (const edge of foldedEdges) {
    if (edge.family !== "flow") {
      continue;
    }
    if (!outgoingAdj.has(edge.source)) {
      outgoingAdj.set(edge.source, new Set());
    }
    if (!incomingAdj.has(edge.target)) {
      incomingAdj.set(edge.target, new Set());
    }
    outgoingAdj.get(edge.source)?.add(edge.target);
    incomingAdj.get(edge.target)?.add(edge.source);
  }

  const distOut = bfsDistances(outgoingAdj, centerNodeId);
  const distIn = bfsDistances(incomingAdj, centerNodeId);
  const xRank = new Map<string, number>();
  for (const nodeId of canonicalNodeIds) {
    if (nodeId === centerNodeId) {
      xRank.set(nodeId, 0);
      continue;
    }

    const out = distOut.get(nodeId);
    const incoming = distIn.get(nodeId);
    const fallback = signedDepthByNode.get(nodeId) ?? 1;
    let rank: number;

    if (typeof out === "number" && typeof incoming === "number") {
      if (out < incoming) {
        rank = Math.max(1, out);
      } else if (incoming < out) {
        rank = -Math.max(1, incoming);
      } else {
        rank = fallback < 0 ? -Math.max(1, incoming) : Math.max(1, out);
      }
    } else if (typeof out === "number") {
      rank = Math.max(1, out);
    } else if (typeof incoming === "number") {
      rank = -Math.max(1, incoming);
    } else {
      rank = fallback === 0 ? 1 : fallback;
    }

    xRank.set(nodeId, rank);
  }

  const yRank = new Map<string, number>(canonicalNodeIds.map((id) => [id, 0]));
  const hierarchyEdges = foldedEdges.filter(
    (edge) => edge.family === "hierarchy" && edge.source !== edge.target,
  );
  const pairSet = new Set(hierarchyEdges.map((edge) => `${edge.source}->${edge.target}`));
  const hierarchyConstraints = hierarchyEdges.filter((edge) => {
    const reverse = `${edge.target}->${edge.source}`;
    if (!pairSet.has(reverse)) {
      return true;
    }
    return edge.source.localeCompare(edge.target) < 0;
  });

  for (let pass = 0; pass < canonicalNodeIds.length; pass += 1) {
    let changed = false;
    for (const edge of hierarchyConstraints) {
      const parent = edge.target;
      const child = edge.source;
      const parentRank = yRank.get(parent) ?? 0;
      const childRank = yRank.get(child) ?? 0;
      const nextChild = Math.min(24, Math.max(childRank, parentRank + 1));
      if (nextChild !== childRank) {
        yRank.set(child, nextChild);
        changed = true;
      }
    }
    if (!changed) {
      break;
    }
  }

  const centerYRank = yRank.get(centerNodeId) ?? 0;
  for (const nodeId of canonicalNodeIds) {
    yRank.set(nodeId, (yRank.get(nodeId) ?? 0) - centerYRank);
  }
  applyVirtualRankHints(yRank, xRank, foldedEdges);

  const byXRank = new Map<number, string[]>();
  for (const nodeId of canonicalNodeIds) {
    const rank = xRank.get(nodeId) ?? 0;
    const list = byXRank.get(rank) ?? [];
    list.push(nodeId);
    byXRank.set(rank, list);
  }

  const sortedXRanks = [...byXRank.keys()].sort((left, right) => left - right);
  const layers = sortedXRanks.map((rank) =>
    [...(byXRank.get(rank) ?? [])].sort((left, right) => {
      const yCompare = (yRank.get(left) ?? 0) - (yRank.get(right) ?? 0);
      if (yCompare !== 0) {
        return yCompare;
      }
      return (labelsByNode.get(left) ?? left).localeCompare(labelsByNode.get(right) ?? right);
    }),
  );

  const neighborsByNode = new Map<string, Set<string>>();
  for (const edge of foldedEdges) {
    if (!neighborsByNode.has(edge.source)) {
      neighborsByNode.set(edge.source, new Set());
    }
    if (!neighborsByNode.has(edge.target)) {
      neighborsByNode.set(edge.target, new Set());
    }
    neighborsByNode.get(edge.source)?.add(edge.target);
    neighborsByNode.get(edge.target)?.add(edge.source);
  }

  for (let pass = 0; pass < 4; pass += 1) {
    for (let layerIdx = 1; layerIdx < layers.length; layerIdx += 1) {
      const layer = layers[layerIdx];
      const reference = layers[layerIdx - 1];
      if (!layer || !reference) {
        continue;
      }
      layers[layerIdx] = sortLayerByNeighborMedian(
        layer,
        nodeOrderIndex(reference),
        neighborsByNode,
        labelsByNode,
      );
    }

    for (let layerIdx = layers.length - 2; layerIdx >= 0; layerIdx -= 1) {
      const layer = layers[layerIdx];
      const reference = layers[layerIdx + 1];
      if (!layer || !reference) {
        continue;
      }
      layers[layerIdx] = sortLayerByNeighborMedian(
        layer,
        nodeOrderIndex(reference),
        neighborsByNode,
        labelsByNode,
      );
    }
  }

  const layerRelativeBounds = layers.map((layerNodeIds, xIdx) => {
    const rank = sortedXRanks[xIdx] ?? 0;
    const columnCount = columnCountForLayer(layerNodeIds.length, rank);
    const columns = bucketLayerNodes(layerNodeIds, columnCount, rank);
    const maxColumnWidth = layerNodeIds.reduce(
      (maxWidth, nodeId) => Math.max(maxWidth, estimatedWidthByNode.get(nodeId) ?? PILL_WIDTH_MIN),
      PILL_WIDTH_MIN,
    );
    const baseColumnSpacing = Math.max(
      DENSE_LAYER_COL_SPACING_MIN,
      maxColumnWidth + DENSE_LAYER_COL_GAP,
    );

    let minX = 0;
    let maxX = 0;
    const side = rank < 0 ? -1 : 1;
    const absRank = Math.abs(rank);
    const outwardDepthOffset =
      absRank >= 2
        ? side * (DEPTH_TWO_OUTWARD_OFFSET_X + (absRank - 2) * OUTER_LAYER_OUTWARD_STEP_X)
        : 0;

    for (let columnIdx = 0; columnIdx < columns.length; columnIdx += 1) {
      const columnOffset = side * columnIdx * baseColumnSpacing;
      const rankOneOffset = Math.abs(rank) === 1 ? side * RANK_ONE_CENTER_CLEARANCE_X : 0;
      const nodeX = columnOffset + rankOneOffset + outwardDepthOffset;
      minX = Math.min(minX, nodeX);
      maxX = Math.max(maxX, nodeX + maxColumnWidth);
    }

    return { minX, maxX, baseColumnSpacing, columns, rank };
  });

  const layerBaseX = Array.from<number>({ length: layers.length }).fill(0);
  if (layers.length > 0) {
    layerBaseX[0] = 150;
    for (let i = 1; i < layers.length; i++) {
      const prevLayer = layerRelativeBounds[i - 1]!;
      const thisLayer = layerRelativeBounds[i]!;
      const prevRightEdge = layerBaseX[i - 1]! + prevLayer.maxX;
      // GAP of 160 ensures trunk paths have ample horizontal room for routing branches
      layerBaseX[i] = prevRightEdge + 160 - thisLayer.minX;
    }
  }

  const positionsByNode = new Map<string, { x: number; y: number }>();
  layers.forEach((_, xIdx) => {
    const rank = sortedXRanks[xIdx] ?? 0;
    const layerBounds = layerRelativeBounds[xIdx]!;
    const columns = layerBounds.columns;
    const columnCenter = (columns.length - 1) / 2;
    const columnSpacing = layerBounds.baseColumnSpacing;
    const depthTwoSpacingBoost = Math.abs(rank) === 2 ? DEPTH_TWO_ROW_SPACING_BOOST : 0;
    const rowSpacing = Math.max(
      MIN_ROW_SPACING,
      LANE_JITTER_SPACING + (columns.length - 1) * 10 + depthTwoSpacingBoost,
    );
    const side = rank < 0 ? -1 : 1;
    const absRank = Math.abs(rank);
    const outwardDepthOffset =
      absRank >= 2
        ? side * (DEPTH_TWO_OUTWARD_OFFSET_X + (absRank - 2) * OUTER_LAYER_OUTWARD_STEP_X)
        : 0;

    for (let columnIdx = 0; columnIdx < columns.length; columnIdx += 1) {
      const columnNodes = columns[columnIdx];
      if (!columnNodes) {
        continue;
      }

      const columnOffset = side * columnIdx * columnSpacing;
      const columnStagger = (columnIdx - columnCenter) * DENSE_LAYER_STAGGER_Y;
      const rowCenter = (columnNodes.length - 1) / 2;
      const rankOneOffset = Math.abs(rank) === 1 ? side * RANK_ONE_CENTER_CLEARANCE_X : 0;

      for (let rowIdx = 0; rowIdx < columnNodes.length; rowIdx += 1) {
        const nodeId = columnNodes[rowIdx];
        if (!nodeId) {
          continue;
        }

        let anchorOffset = 17;
        const node = nodeById.get(nodeId);
        if (node && CARD_NODE_KINDS.has(node.kind)) {
          if (nodeId === centerNodeId) {
            const members = membersByHost.get(nodeId) ?? [];
            const mergedMembers = membersByCanonical.get(nodeId) ?? members;
            const publicMembers = mergedMembers.filter((m) => m.visibility === "public");
            const protectedMembers = mergedMembers.filter((m) => m.visibility === "protected");
            const privateMembers = mergedMembers.filter((m) => m.visibility === "private");
            const defaultMembers = mergedMembers.filter((m) => m.visibility === "default");
            const memberId = graph.center_id;

            const pIdx = publicMembers.findIndex((m) => m.id === memberId);
            if (pIdx >= 0) {
              anchorOffset = 74 + 28 + pIdx * 21 + 10;
            } else {
              const protIdx = protectedMembers.findIndex((m) => m.id === memberId);
              if (protIdx >= 0) {
                anchorOffset =
                  74 +
                  (publicMembers.length > 0 ? 28 + publicMembers.length * 21 : 0) +
                  28 +
                  protIdx * 21 +
                  10;
              } else {
                const prIdx = privateMembers.findIndex((m) => m.id === memberId);
                if (prIdx >= 0) {
                  anchorOffset =
                    74 +
                    (publicMembers.length > 0 ? 28 + publicMembers.length * 21 : 0) +
                    (protectedMembers.length > 0 ? 28 + protectedMembers.length * 21 : 0) +
                    28 +
                    prIdx * 21 +
                    10;
                } else {
                  const defaultIdx = defaultMembers.findIndex((m) => m.id === memberId);
                  if (defaultIdx >= 0) {
                    anchorOffset =
                      74 +
                      (publicMembers.length > 0 ? 28 + publicMembers.length * 21 : 0) +
                      (protectedMembers.length > 0 ? 28 + protectedMembers.length * 21 : 0) +
                      (privateMembers.length > 0 ? 28 + privateMembers.length * 21 : 0) +
                      28 +
                      defaultIdx * 21 +
                      10;
                  } else {
                    anchorOffset = 42;
                  }
                }
              }
            }
          } else {
            anchorOffset = 42;
          }
        }

        const hierarchyOffset = (yRank.get(nodeId) ?? 0) * HIER_ROW_SPACING;
        const compactOffset = (rowIdx - rowCenter) * rowSpacing;
        const anchorLineY = ROOT_TARGET_Y + hierarchyOffset + compactOffset + columnStagger;

        positionsByNode.set(nodeId, {
          x: layerBaseX[xIdx]! + columnOffset + rankOneOffset + outwardDepthOffset,
          y: anchorLineY - anchorOffset,
        });
      }
    }
  });

  compactRankPositions(positionsByNode, xRank, estimatedHeightByNode);
  smoothRankPositions(
    layers,
    sortedXRanks,
    positionsByNode,
    xRank,
    neighborsByNode,
    estimatedHeightByNode,
  );
  compactRankPositions(positionsByNode, xRank, estimatedHeightByNode);

  for (const [nodeId, position] of positionsByNode) {
    positionsByNode.set(nodeId, {
      x: snapToRaster(position.x),
      y: snapToRaster(position.y),
    });
  }

  const nodeOrder = layers.flat();
  const nodes = nodeOrder
    .map((nodeId) => {
      const node = nodeById.get(nodeId);
      const pos = positionsByNode.get(nodeId);
      if (!node || !pos) {
        return null;
      }

      const nodeStyle: "card" | "pill" = CARD_NODE_KINDS.has(node.kind) ? "card" : "pill";

      return {
        id: nodeId,
        kind: node.kind,
        label: node.label,
        center: nodeId === centerNodeId,
        nodeStyle,
        isNonIndexed: node.kind === "UNKNOWN" || node.kind === "BUILTIN_TYPE",
        duplicateCount: duplicateCountByCanonical.get(nodeId) ?? 1,
        mergedSymbolIds: (mergedIdsByCanonical.get(nodeId) ?? [nodeId]).slice(0, 6),
        memberCount: node.badge_visible_members ?? membersByCanonical.get(nodeId)?.length ?? 0,
        badgeVisibleMembers: node.badge_visible_members ?? undefined,
        badgeTotalMembers: node.badge_total_members ?? undefined,
        members: [...(membersByCanonical.get(nodeId) ?? [])].sort((left, right) =>
          left.label.localeCompare(right.label),
        ),
        xRank: xRank.get(nodeId) ?? 0,
        yRank: yRank.get(nodeId) ?? 0,
        x: pos.x,
        y: pos.y,
        width: estimatedWidthByNode.get(nodeId) ?? PILL_WIDTH_MIN,
        height: estimatedHeightByNode.get(nodeId) ?? PILL_HEIGHT,
        isVirtualBundle: false,
      };
    })
    .filter((node): node is NonNullable<typeof node> => node !== null);

  return {
    nodes,
    edges: toRoutedEdgeSpecs(foldedEdges),
    centerNodeId,
  };
}

export function buildFallbackLayout(graph: GraphResponse): LayoutElements {
  const { memberHostById, membersByHost, syntheticHosts } = extractMembers(graph);
  const allNodes = [...graph.nodes, ...syntheticHosts];
  const centerNodeId = memberHostById.get(graph.center_id) ?? graph.center_id;

  const nodes = allNodes
    .filter((node) => !memberHostById.has(node.id))
    .sort((left, right) => left.depth - right.depth || left.label.localeCompare(right.label))
    .map((node, index) => {
      const column = Math.max(0, node.depth);
      const row = index % 10;
      const nodeStyle: "card" | "pill" = CARD_NODE_KINDS.has(node.kind) ? "card" : "pill";
      const members = membersByHost.get(node.id) ?? [];
      const width = estimatedNodeWidth(node.kind, node.label, members);
      const height = estimatedNodeHeight(node.kind, members);
      return {
        id: node.id,
        kind: node.kind,
        label: node.label,
        center: node.id === centerNodeId,
        nodeStyle,
        isNonIndexed: node.kind === "UNKNOWN" || node.kind === "BUILTIN_TYPE",
        duplicateCount: 1,
        mergedSymbolIds: [node.id],
        memberCount: node.badge_visible_members ?? membersByHost.get(node.id)?.length ?? 0,
        badgeVisibleMembers: node.badge_visible_members ?? undefined,
        badgeTotalMembers: node.badge_total_members ?? undefined,
        members: [...members].sort((leftEntry, rightEntry) =>
          leftEntry.label.localeCompare(rightEntry.label),
        ),
        xRank: column,
        yRank: row,
        x: snapToRaster(120 + column * FLOW_COL_SPACING),
        y: snapToRaster(120 + row * LANE_JITTER_SPACING),
        width,
        height,
        isVirtualBundle: false,
      };
    });

  const canonicalIds = new Set(nodes.map((node) => node.id));
  const edges: RoutedEdgeSpec[] = graph.edges
    .filter((edge) => edge.kind !== "MEMBER")
    .map((edge) => {
      const source = memberHostById.get(edge.source) ?? edge.source;
      const target = memberHostById.get(edge.target) ?? edge.target;
      const family = edgeFamilyForKind(edge.kind);
      const routeKind: RoutedEdgeSpec["routeKind"] =
        family === "hierarchy" ? "hierarchy" : "direct";
      return {
        id: `${edge.kind}:${source}:${target}:${edge.id}`,
        sourceEdgeIds: [edge.id],
        source,
        target,
        sourceHandle: family === "hierarchy" ? "source-node-top" : "source-node",
        targetHandle: family === "hierarchy" ? "target-node-bottom" : "target-node",
        kind: edge.kind,
        certainty: edge.certainty,
        multiplicity: 1,
        family,
        routeKind,
        bundleCount: 1,
        routePoints: [],
      };
    })
    .filter((edge) => canonicalIds.has(edge.source) && canonicalIds.has(edge.target));

  return {
    nodes,
    edges,
    centerNodeId,
  };
}
