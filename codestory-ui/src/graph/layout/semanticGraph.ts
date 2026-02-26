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
  type SemanticNodePlacement,
  type SemanticEdgeFamily,
} from "./types";

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
const EDGE_BUNDLE_NODE_SIZE = 6;
const EDGE_BUNDLE_MIN_BRANCHES = 3;
const EDGE_BUNDLE_DEPTH_STEP = 0.42;
const EDGE_BUNDLE_ROW_STEP = 0.26;

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

type MemberExtraction = {
  memberHostById: Map<string, string>;
  membersByHost: Map<string, FlowMemberData[]>;
  syntheticHosts: GraphNodeLike[];
};

export type CanonicalLayoutOptions = {
  bundleFanOutEdges?: boolean;
};

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

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function textWidth(label: string): number {
  return label.length * APPROX_CHAR_WIDTH;
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
    foldedEdges: [...folded.values()].sort((left, right) => left.id.localeCompare(right.id)),
    canonicalNodeById,
    duplicateCountByCanonical,
    mergedIdsByCanonical,
  };
}

function toRoutedEdgeSpecs(edges: FoldedEdge[]): RoutedEdgeSpec[] {
  return edges.map((edge) => ({
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
    routeKind: edge.family === "hierarchy" ? "hierarchy" : "direct",
    routePoints: [],
  }));
}

function dedupeStringList(items: string[]): string[] {
  const seen = new Set<string>();
  const deduped: string[] = [];
  for (const item of items) {
    if (!seen.has(item)) {
      deduped.push(item);
      seen.add(item);
    }
  }
  return deduped;
}

function collectSourceEdgeIds(edges: RoutedEdgeSpec[]): string[] {
  return dedupeStringList(
    edges.flatMap((edge) => (edge.sourceEdgeIds.length > 0 ? edge.sourceEdgeIds : [edge.id])),
  );
}

function mergeEdgeCertainty(edges: RoutedEdgeSpec[]): string | null | undefined {
  return edges.reduce(
    (certainty, edge) => mergeCertainty(certainty, edge.certainty),
    null as string | null | undefined,
  );
}

function totalEdgeMultiplicity(edges: RoutedEdgeSpec[]): number {
  return edges.reduce((sum, edge) => sum + edge.multiplicity, 0);
}

function applyFanOutBundling(
  nodes: SemanticNodePlacement[],
  edges: RoutedEdgeSpec[],
  depthByNode: Map<string, number>,
): { nodes: SemanticNodePlacement[]; edges: RoutedEdgeSpec[] } {
  if (edges.length < EDGE_BUNDLE_MIN_BRANCHES) {
    return { nodes, edges };
  }

  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const groupsByKey = new Map<
    string,
    {
      key: string;
      kind: EdgeKind;
      source: string;
      sourceHandle: string;
      edges: RoutedEdgeSpec[];
    }
  >();

  for (const edge of edges) {
    if (edge.family !== "flow") {
      continue;
    }

    const sourceNode = nodeById.get(edge.source);
    const targetNode = nodeById.get(edge.target);
    if (!sourceNode || !targetNode || sourceNode.isVirtualBundle || targetNode.isVirtualBundle) {
      continue;
    }

    const key = `${edge.kind}:${edge.source}:${edge.sourceHandle}`;
    const group = groupsByKey.get(key) ?? {
      key,
      kind: edge.kind,
      source: edge.source,
      sourceHandle: edge.sourceHandle,
      edges: [],
    };
    group.edges.push(edge);
    groupsByKey.set(key, group);
  }

  const groups = [...groupsByKey.values()]
    .filter((group) => {
      if (group.edges.length < EDGE_BUNDLE_MIN_BRANCHES) {
        return false;
      }
      const distinctTargets = new Set(group.edges.map((edge) => edge.target));
      return distinctTargets.size > 1;
    })
    .sort((left, right) => left.key.localeCompare(right.key));

  if (groups.length === 0) {
    return { nodes, edges };
  }

  const groupsBySource = new Map<string, typeof groups>();
  for (const group of groups) {
    const sourceGroups = groupsBySource.get(group.source) ?? [];
    sourceGroups.push(group);
    groupsBySource.set(group.source, sourceGroups);
  }
  for (const sourceGroups of groupsBySource.values()) {
    sourceGroups.sort((left, right) => left.key.localeCompare(right.key));
  }

  const reroutedEdgesById = new Map<string, RoutedEdgeSpec>();
  const bundleTrunkEdges: RoutedEdgeSpec[] = [];
  const bundleNodes: SemanticNodePlacement[] = [];

  let bundleCounter = 0;
  for (const group of groups) {
    const sourceNode = nodeById.get(group.source);
    if (!sourceNode) {
      continue;
    }

    const edgeTargetRow = (edge: RoutedEdgeSpec): number =>
      nodeById.get(edge.target)?.yRank ?? sourceNode.yRank;
    const edgeTargetDepth = (edge: RoutedEdgeSpec): number =>
      depthByNode.get(edge.target) ?? nodeById.get(edge.target)?.xRank ?? sourceNode.xRank;
    const rowSortedEdges = [...group.edges].sort((left, right) => {
      const rowDiff = edgeTargetRow(left) - edgeTargetRow(right);
      if (rowDiff !== 0) {
        return rowDiff;
      }
      const depthDiff = edgeTargetDepth(left) - edgeTargetDepth(right);
      if (depthDiff !== 0) {
        return depthDiff;
      }
      return left.id.localeCompare(right.id);
    });

    if (rowSortedEdges.length < EDGE_BUNDLE_MIN_BRANCHES) {
      continue;
    }

    const sourceDepth = depthByNode.get(group.source) ?? sourceNode.xRank;

    const sourceGroups = groupsBySource.get(group.source) ?? [group];
    const sourceGroupIndex = sourceGroups.findIndex((candidate) => candidate.key === group.key);
    const rowOffset =
      sourceGroups.length <= 1
        ? 0
        : (sourceGroupIndex - (sourceGroups.length - 1) / 2) * EDGE_BUNDLE_ROW_STEP;

    const groupToken = `${bundleCounter}`;
    bundleCounter += 1;
    const upperLaneEdges = rowSortedEdges
      .filter((edge) => edgeTargetRow(edge) < sourceNode.yRank)
      .sort(
        (left, right) =>
          edgeTargetRow(right) - edgeTargetRow(left) || left.id.localeCompare(right.id),
      );
    const lowerLaneEdges = rowSortedEdges
      .filter((edge) => edgeTargetRow(edge) >= sourceNode.yRank)
      .sort(
        (left, right) =>
          edgeTargetRow(left) - edgeTargetRow(right) || left.id.localeCompare(right.id),
      );

    const lanes = [
      { laneId: "up", edges: upperLaneEdges, laneRowOffset: -0.24 },
      { laneId: "down", edges: lowerLaneEdges, laneRowOffset: 0.24 },
    ].filter((lane) => lane.edges.length > 0);

    if (lanes.length === 1) {
      lanes[0]!.laneRowOffset = 0;
    }

    for (const lane of lanes) {
      const laneEdges = lane.edges;
      if (laneEdges.length < 2) {
        continue;
      }

      const avgTargetDepth =
        laneEdges.reduce((sum, edge) => sum + edgeTargetDepth(edge), 0) / laneEdges.length;
      const depthDirection = avgTargetDepth >= sourceDepth ? 1 : -1;
      const directedDepthSpan = Math.max(
        EDGE_BUNDLE_DEPTH_STEP * 2.2,
        Math.abs(avgTargetDepth - sourceDepth),
      );
      const splitDepthStart =
        sourceDepth +
        depthDirection * Math.max(EDGE_BUNDLE_DEPTH_STEP * 2.8, directedDepthSpan * 0.72);
      const splitDepthEnd =
        sourceDepth +
        depthDirection * Math.max(EDGE_BUNDLE_DEPTH_STEP * 3.4, directedDepthSpan * 0.94);
      const maxSplitLevel = Math.max(1, Math.ceil(Math.log2(laneEdges.length)));

      const laneToken = `${groupToken}__${lane.laneId}`;
      const splitDepthFor = (level: number, subsetAvgDepth: number): number => {
        const levelRatio =
          maxSplitLevel <= 1 ? 1 : Math.min(1, level / Math.max(1, maxSplitLevel - 1));
        const plannedDepth = splitDepthStart + (splitDepthEnd - splitDepthStart) * levelRatio;
        const maxOffsetTowardSubset = Math.max(
          EDGE_BUNDLE_DEPTH_STEP * 2,
          Math.abs(subsetAvgDepth - sourceDepth) - EDGE_BUNDLE_DEPTH_STEP * 0.25,
        );
        const plannedOffset = Math.min(Math.abs(plannedDepth - sourceDepth), maxOffsetTowardSubset);
        return sourceDepth + depthDirection * plannedOffset;
      };

      const rerouteLeaf = (edge: RoutedEdgeSpec, bundleId: string) => {
        reroutedEdgesById.set(edge.id, {
          ...edge,
          source: bundleId,
          sourceHandle: "source-node-right",
        });
      };

      const buildBundleTree = (
        subset: RoutedEdgeSpec[],
        level: number,
        pathToken: string,
      ): string | null => {
        if (subset.length < 2) {
          return null;
        }

        const subsetAvgRow =
          subset.reduce((sum, edge) => sum + edgeTargetRow(edge), 0) / subset.length;
        const subsetAvgDepth =
          subset.reduce((sum, edge) => sum + edgeTargetDepth(edge), 0) / subset.length;
        const bundleId = `__fanout_bundle__${laneToken}__${pathToken}`;

        bundleNodes.push({
          id: bundleId,
          kind: "BUNDLE",
          label: "",
          center: false,
          nodeStyle: "bundle",
          isNonIndexed: false,
          duplicateCount: 1,
          mergedSymbolIds: [],
          memberCount: 0,
          members: [],
          xRank: splitDepthFor(level, subsetAvgDepth),
          yRank: subsetAvgRow + rowOffset + lane.laneRowOffset,
          x: 0,
          y: 0,
          width: EDGE_BUNDLE_NODE_SIZE,
          height: EDGE_BUNDLE_NODE_SIZE,
          isVirtualBundle: true,
        });

        if (subset.length === 2) {
          rerouteLeaf(subset[0]!, bundleId);
          rerouteLeaf(subset[1]!, bundleId);
          return bundleId;
        }

        const splitAt = Math.floor(subset.length / 2);
        const leftSubset = subset.slice(0, splitAt);
        const rightSubset = subset.slice(splitAt);
        const childSubsets = [leftSubset, rightSubset];

        for (const [childIndex, childSubset] of childSubsets.entries()) {
          if (childSubset.length === 0) {
            continue;
          }
          if (childSubset.length === 1) {
            rerouteLeaf(childSubset[0]!, bundleId);
            continue;
          }

          const childToken = `${pathToken}_${childIndex}`;
          const childBundleId = buildBundleTree(childSubset, level + 1, childToken);
          if (!childBundleId) {
            continue;
          }
          bundleTrunkEdges.push({
            id: `__fanout_trunk__${laneToken}__${pathToken}_${childIndex}`,
            sourceEdgeIds: collectSourceEdgeIds(childSubset),
            source: bundleId,
            target: childBundleId,
            sourceHandle: "source-node-right",
            targetHandle: "target-node-left",
            kind: group.kind,
            certainty: mergeEdgeCertainty(childSubset),
            multiplicity: totalEdgeMultiplicity(childSubset),
            family: "flow",
            routeKind: "direct",
            routePoints: [],
          });
        }

        return bundleId;
      };

      const rootBundleId = buildBundleTree(laneEdges, 0, "root");
      if (!rootBundleId) {
        continue;
      }
      bundleTrunkEdges.push({
        id: `__fanout_trunk__${laneToken}__source`,
        sourceEdgeIds: collectSourceEdgeIds(laneEdges),
        source: group.source,
        target: rootBundleId,
        sourceHandle: group.sourceHandle,
        targetHandle: "target-node-left",
        kind: group.kind,
        certainty: mergeEdgeCertainty(laneEdges),
        multiplicity: totalEdgeMultiplicity(laneEdges),
        family: "flow",
        routeKind: "direct",
        routePoints: [],
      });
    }
  }

  if (bundleNodes.length === 0) {
    return { nodes, edges };
  }

  const reroutedEdges = edges.map((edge) => reroutedEdgesById.get(edge.id) ?? edge);
  const bundledEdges = [...reroutedEdges, ...bundleTrunkEdges].sort((left, right) =>
    left.id.localeCompare(right.id),
  );

  return {
    nodes: [...nodes, ...bundleNodes],
    edges: bundledEdges,
  };
}

export function buildCanonicalLayout(
  graph: GraphResponse,
  options?: CanonicalLayoutOptions,
): LayoutElements {
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

  const depthByCanonical = new Map<string, number>();
  for (const nodeId of canonicalNodeIds) {
    const mergedIds = mergedIdsByCanonical.get(nodeId) ?? [nodeId];
    const depths = mergedIds.map((id) => signedDepthByNode.get(id) ?? 0);
    const depth =
      depths.length === 0
        ? 0
        : Math.round(depths.reduce((sum, value) => sum + value, 0) / depths.length);
    depthByCanonical.set(nodeId, depth);
  }

  const nodeOrder = [...canonicalNodeIds].sort((left, right) => {
    const depthDiff = (depthByCanonical.get(left) ?? 0) - (depthByCanonical.get(right) ?? 0);
    if (depthDiff !== 0) {
      return depthDiff;
    }
    return (labelsByNode.get(left) ?? left).localeCompare(labelsByNode.get(right) ?? right);
  });

  const rowByDepth = new Map<number, number>();
  const nodes = nodeOrder
    .map((nodeId) => {
      const node = nodeById.get(nodeId);
      if (!node) {
        return null;
      }

      const members = [...(membersByCanonical.get(nodeId) ?? [])].sort((left, right) =>
        left.label.localeCompare(right.label),
      );
      const depth = depthByCanonical.get(nodeId) ?? 0;
      const row = rowByDepth.get(depth) ?? 0;
      rowByDepth.set(depth, row + 1);
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
        memberCount: node.badge_visible_members ?? members.length,
        badgeVisibleMembers: node.badge_visible_members ?? undefined,
        badgeTotalMembers: node.badge_total_members ?? undefined,
        members,
        xRank: depth,
        yRank: row,
        x: 0,
        y: 0,
        width: estimatedNodeWidth(node.kind, node.label, members),
        height: estimatedNodeHeight(node.kind, members),
        isVirtualBundle: false,
      };
    })
    .filter((node): node is NonNullable<typeof node> => node !== null);

  const routedEdges = toRoutedEdgeSpecs(foldedEdges);
  const bundledLayout = options?.bundleFanOutEdges
    ? applyFanOutBundling(nodes, routedEdges, depthByCanonical)
    : { nodes, edges: routedEdges };

  return {
    nodes: bundledLayout.nodes,
    edges: bundledLayout.edges,
    centerNodeId,
  };
}
