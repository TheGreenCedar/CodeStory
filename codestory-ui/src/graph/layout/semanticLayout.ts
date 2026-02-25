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

export const FLOW_COL_SPACING = 332;
export const HIER_ROW_SPACING = 112;
export const LANE_JITTER_SPACING = 58;

const ROOT_TARGET_Y = 320;
const DENSE_LAYER_ROW_TARGET = 9;
const DENSE_LAYER_MAX_COLUMNS = 4;
const DENSE_LAYER_COL_SPACING_MIN = 220;
const RANK_ONE_CENTER_CLEARANCE_X = 104;
const DENSE_LAYER_COL_GAP = 64;
const DENSE_LAYER_STAGGER_Y = 40;
const MIN_ROW_SPACING = 56;
const CARD_WIDTH_MIN = 228;
const CARD_WIDTH_MAX = 432;
const CARD_CHROME_WIDTH = 112;
const PILL_WIDTH_MIN = 96;
const PILL_WIDTH_MAX = 560;
const PILL_CHROME_WIDTH = 72;
const APPROX_CHAR_WIDTH = 7.25;

type FoldedEdge = {
  id: string;
  source: string;
  target: string;
  kind: EdgeKind;
  certainty: string | null | undefined;
  multiplicity: number;
  sourceHandle: string;
  targetHandle: string;
  family: SemanticEdgeFamily;
};

function inferMemberVisibility(kind: string, label: string): "public" | "private" {
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
  if (isCenter || CARD_NODE_KINDS.has(kind)) {
    return null;
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

  if (Math.abs(rank) === 1) {
    return 1;
  }

  if (layerSize <= DENSE_LAYER_ROW_TARGET) {
    return 1;
  }

  return Math.min(DENSE_LAYER_MAX_COLUMNS, Math.ceil(layerSize / DENSE_LAYER_ROW_TARGET));
}

function bucketLayerNodes(layerNodeIds: string[], columnCount: number): string[][] {
  const columns = Array.from({ length: columnCount }, () => [] as string[]);

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

type MemberExtraction = {
  memberHostById: Map<string, string>;
  membersByHost: Map<string, FlowMemberData[]>;
};

function extractMembers(graph: GraphResponse): MemberExtraction {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const memberHostById = new Map<string, string>();
  const membersByHost = new Map<string, FlowMemberData[]>();

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
      const memberLabel = nodeById.get(memberId)?.label ?? memberId;
      const memberKind = nodeById.get(memberId)?.kind ?? "UNKNOWN";
      hostMembers.push({
        id: memberId,
        label: memberLabel,
        kind: memberKind,
        visibility: inferMemberVisibility(memberKind, memberLabel),
      });
      membersByHost.set(hostId, hostMembers);
    }
  }

  return { memberHostById, membersByHost };
}

function foldEdges(
  graph: GraphResponse,
  centerHostNodeId: string,
  memberHostById: Map<string, string>,
  signedDepthByNode: Map<string, number>,
): {
  foldedEdges: FoldedEdge[];
  canonicalNodeById: Map<string, string>;
  duplicateCountByCanonical: Map<string, number>;
} {
  const canonicalNodeById = new Map<string, string>();
  const canonicalNodeByKey = new Map<string, string>();
  const duplicateCountByCanonical = new Map<string, number>();

  for (const node of graph.nodes) {
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
  }

  const folded = new Map<string, FoldedEdge>();
  for (const edge of graph.edges) {
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
    existing.certainty = mergeCertainty(existing.certainty, edge.certainty);
  }

  return {
    foldedEdges: [...folded.values()],
    canonicalNodeById,
    duplicateCountByCanonical,
  };
}

function computeSignedDepthByNode(
  graph: GraphResponse,
  centerHostNodeId: string,
): Map<string, number> {
  const directionBiasByNode = new Map<string, number>();
  for (const edge of graph.edges) {
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
  for (const node of graph.nodes) {
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

function toRoutedEdgeSpecs(edges: FoldedEdge[]): RoutedEdgeSpec[] {
  return edges
    .map((edge) => {
      const routeKind: RoutedEdgeSpec["routeKind"] =
        edge.family === "hierarchy" ? "hierarchy" : "direct";

      return {
        id: edge.id,
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
      };
    })
    .sort((left, right) => left.id.localeCompare(right.id));
}

export function buildSemanticLayout(graph: GraphResponse): LayoutElements {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const labelsByNode = new Map(graph.nodes.map((node) => [node.id, node.label]));
  const { memberHostById, membersByHost } = extractMembers(graph);
  const centerHostNodeId = memberHostById.get(graph.center_id) ?? graph.center_id;
  const signedDepthByNode = computeSignedDepthByNode(graph, centerHostNodeId);

  const { foldedEdges, canonicalNodeById, duplicateCountByCanonical } = foldEdges(
    graph,
    centerHostNodeId,
    memberHostById,
    signedDepthByNode,
  );

  const canonicalNodeIds = [...new Set(canonicalNodeById.values())];
  const centerNodeId = canonicalNodeById.get(centerHostNodeId) ?? centerHostNodeId;
  const estimatedWidthByNode = new Map<string, number>();
  for (const nodeId of canonicalNodeIds) {
    const node = nodeById.get(nodeId);
    if (!node) {
      continue;
    }
    estimatedWidthByNode.set(
      nodeId,
      estimatedNodeWidth(node.kind, node.label, membersByHost.get(nodeId) ?? []),
    );
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

  const positionsByNode = new Map<string, { x: number; y: number }>();
  layers.forEach((layerNodeIds, xIdx) => {
    const rank = sortedXRanks[xIdx] ?? 0;
    const columnCount = columnCountForLayer(layerNodeIds.length, rank);
    const columns = bucketLayerNodes(layerNodeIds, columnCount);
    const columnCenter = (columns.length - 1) / 2;
    const maxColumnWidth = layerNodeIds.reduce(
      (maxWidth, nodeId) => Math.max(maxWidth, estimatedWidthByNode.get(nodeId) ?? PILL_WIDTH_MIN),
      PILL_WIDTH_MIN,
    );
    const baseColumnSpacing = Math.max(
      DENSE_LAYER_COL_SPACING_MIN,
      maxColumnWidth + DENSE_LAYER_COL_GAP,
    );
    const columnSpacing = baseColumnSpacing;
    const rowSpacing = Math.max(MIN_ROW_SPACING, LANE_JITTER_SPACING + (columns.length - 1) * 10);
    const side = rank < 0 ? -1 : 1;

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

        const hierarchyOffset = (yRank.get(nodeId) ?? 0) * HIER_ROW_SPACING;
        const compactOffset = (rowIdx - rowCenter) * rowSpacing;
        positionsByNode.set(nodeId, {
          x: 150 + xIdx * FLOW_COL_SPACING + columnOffset + rankOneOffset,
          y: ROOT_TARGET_Y + hierarchyOffset + compactOffset + columnStagger,
        });
      }
    }
  });

  const centerPos = positionsByNode.get(centerNodeId);
  if (centerPos) {
    const deltaY = ROOT_TARGET_Y - centerPos.y;
    for (const [nodeId, position] of positionsByNode) {
      positionsByNode.set(nodeId, {
        x: position.x,
        y: position.y + deltaY,
      });
    }
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
        duplicateCount: duplicateCountByCanonical.get(nodeId) ?? 1,
        memberCount: membersByHost.get(nodeId)?.length ?? 0,
        members: [...(membersByHost.get(nodeId) ?? [])].sort((left, right) =>
          left.label.localeCompare(right.label),
        ),
        xRank: xRank.get(nodeId) ?? 0,
        yRank: yRank.get(nodeId) ?? 0,
        x: pos.x,
        y: pos.y,
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
  const { memberHostById, membersByHost } = extractMembers(graph);
  const centerNodeId = memberHostById.get(graph.center_id) ?? graph.center_id;

  const nodes = graph.nodes
    .filter((node) => !memberHostById.has(node.id))
    .sort((left, right) => left.depth - right.depth || left.label.localeCompare(right.label))
    .map((node, index) => {
      const column = Math.max(0, node.depth);
      const row = index % 10;
      const nodeStyle: "card" | "pill" = CARD_NODE_KINDS.has(node.kind) ? "card" : "pill";
      return {
        id: node.id,
        kind: node.kind,
        label: node.label,
        center: node.id === centerNodeId,
        nodeStyle,
        duplicateCount: 1,
        memberCount: membersByHost.get(node.id)?.length ?? 0,
        members: [...(membersByHost.get(node.id) ?? [])].sort((leftEntry, rightEntry) =>
          leftEntry.label.localeCompare(rightEntry.label),
        ),
        xRank: column,
        yRank: row,
        x: 120 + column * FLOW_COL_SPACING,
        y: 120 + row * LANE_JITTER_SPACING,
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
      };
    })
    .filter((edge) => canonicalIds.has(edge.source) && canonicalIds.has(edge.target));

  return {
    nodes,
    edges,
    centerNodeId,
  };
}
