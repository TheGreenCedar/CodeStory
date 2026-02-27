import type {
  CanonicalEdgeDto,
  CanonicalLayoutDto,
  CanonicalMemberDto,
  CanonicalNodeDto,
  GraphResponse,
} from "../../generated/api";
import type { LayoutElements, RoutedEdgeSpec, SemanticNodePlacement } from "./types";

const SUPPORTED_SCHEMA_VERSION = 1;

type CanonicalSeedResult = {
  seed: LayoutElements | null;
  error: string | null;
};

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function normalizeMembers(
  members: CanonicalMemberDto[] | undefined,
): SemanticNodePlacement["members"] {
  return (members ?? []).map((member) => ({
    id: member.id,
    label: member.label,
    kind: member.kind,
    visibility: member.visibility,
  }));
}

function normalizeNode(node: CanonicalNodeDto): SemanticNodePlacement | null {
  if (!isFiniteNumber(node.width) || !isFiniteNumber(node.height)) {
    return null;
  }
  if (!isFiniteNumber(node.x_rank) || !isFiniteNumber(node.y_rank)) {
    return null;
  }
  return {
    id: node.id,
    kind: node.kind,
    label: node.label,
    center: node.center,
    nodeStyle: node.node_style,
    isNonIndexed: node.is_non_indexed,
    duplicateCount: node.duplicate_count,
    mergedSymbolIds: node.merged_symbol_ids ?? [],
    memberCount: node.member_count,
    badgeVisibleMembers: node.badge_visible_members ?? undefined,
    badgeTotalMembers: node.badge_total_members ?? undefined,
    members: normalizeMembers(node.members),
    xRank: node.x_rank,
    yRank: node.y_rank,
    x: 0,
    y: 0,
    width: node.width,
    height: node.height,
    isVirtualBundle: node.is_virtual_bundle,
  };
}

function normalizeEdge(edge: CanonicalEdgeDto): RoutedEdgeSpec {
  return {
    id: edge.id,
    sourceEdgeIds: edge.source_edge_ids ?? [],
    source: edge.source,
    target: edge.target,
    sourceHandle: edge.source_handle,
    targetHandle: edge.target_handle,
    kind: edge.kind,
    certainty: edge.certainty,
    multiplicity: edge.multiplicity,
    family: edge.family,
    routeKind: edge.route_kind,
    routePoints: [],
  };
}

function normalizeCanonicalLayout(layout: CanonicalLayoutDto): CanonicalSeedResult {
  if (layout.schema_version !== SUPPORTED_SCHEMA_VERSION) {
    return {
      seed: null,
      error: `unsupported canonical schema ${layout.schema_version}`,
    };
  }

  const nodes = layout.nodes
    .map((node) => normalizeNode(node))
    .filter((node): node is NonNullable<typeof node> => node !== null);
  if (nodes.length !== layout.nodes.length) {
    return {
      seed: null,
      error: "canonical node payload contains invalid numeric values",
    };
  }

  const nodeIds = new Set(nodes.map((node) => node.id));
  if (!nodeIds.has(layout.center_node_id)) {
    return {
      seed: null,
      error: "canonical center node is missing from canonical nodes",
    };
  }

  const edges = layout.edges.map(normalizeEdge);
  if (edges.some((edge) => !nodeIds.has(edge.source) || !nodeIds.has(edge.target))) {
    return {
      seed: null,
      error: "canonical edges reference unknown canonical nodes",
    };
  }

  return {
    seed: {
      nodes,
      edges,
      centerNodeId: layout.center_node_id,
    },
    error: null,
  };
}

export function canonicalSeedFromGraphResponse(graph: GraphResponse): CanonicalSeedResult {
  if (!graph.canonical_layout) {
    return {
      seed: null,
      error: "missing canonical layout payload from backend",
    };
  }
  return normalizeCanonicalLayout(graph.canonical_layout);
}
