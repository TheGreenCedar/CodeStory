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

export function buildCanonicalLayout(graph: GraphResponse): LayoutElements {
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

  return {
    nodes,
    edges: toRoutedEdgeSpecs(foldedEdges),
    centerNodeId,
  };
}
