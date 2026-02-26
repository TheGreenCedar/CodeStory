import type { EdgeKind, LayoutDirection } from "../../generated/api";

export type MemberVisibility = "public" | "protected" | "private" | "default";

export type FlowMemberData = {
  id: string;
  label: string;
  kind: string;
  visibility: MemberVisibility;
};

export type FlowNodeStyle = "card" | "pill" | "bundle";

export type FlowNodeData = {
  kind: string;
  label: string;
  center: boolean;
  nodeStyle: FlowNodeStyle;
  layoutDirection: LayoutDirection;
  groupMode?: "namespace" | "file";
  groupLabel?: string;
  groupAnchorId?: string;
  isNonIndexed?: boolean;
  duplicateCount: number;
  mergedSymbolIds?: string[];
  memberCount: number;
  badgeVisibleMembers?: number;
  badgeTotalMembers?: number;
  members: FlowMemberData[];
  isVirtualBundle?: boolean;
  isSelected?: boolean;
  isExpanded?: boolean;
  focusedMemberId?: string | null;
  onSelectMember?: (memberId: string, label: string) => void;
  onToggleExpand?: () => void;
  onSelectGroup?: () => void;
};

export type SemanticEdgeFamily = "flow" | "hierarchy";

export type RouteKind = "direct" | "flow-trunk" | "flow-branch" | "hierarchy";

export type SemanticNodePlacement = {
  id: string;
  kind: string;
  label: string;
  center: boolean;
  nodeStyle: FlowNodeStyle;
  isNonIndexed: boolean;
  duplicateCount: number;
  mergedSymbolIds: string[];
  memberCount: number;
  badgeVisibleMembers?: number;
  badgeTotalMembers?: number;
  members: FlowMemberData[];
  xRank: number;
  yRank: number;
  x: number;
  y: number;
  width: number;
  height: number;
  isVirtualBundle: boolean;
};

export type RoutePoint = {
  x: number;
  y: number;
};

export type RoutedEdgeSpec = {
  id: string;
  sourceEdgeIds: string[];
  source: string;
  target: string;
  sourceHandle: string;
  targetHandle: string;
  kind: EdgeKind;
  certainty: string | null | undefined;
  multiplicity: number;
  family: SemanticEdgeFamily;
  routeKind: RouteKind;
  bundleCount: number;
  routePoints: RoutePoint[];
  trunkCoord?: number;
  channelId?: string;
  channelPairId?: string;
  channelWeight?: number;
  sharedTrunkPoints?: RoutePoint[];
  sourceMemberOrder?: number;
  targetMemberOrder?: number;
};

export type LayoutElements = {
  nodes: SemanticNodePlacement[];
  edges: RoutedEdgeSpec[];
  centerNodeId: string;
};

export type LegendRow = {
  kind: EdgeKind;
  stroke: string;
  count: number;
  hasUncertain: boolean;
  hasProbable: boolean;
};

export const STRUCTURAL_KINDS = new Set([
  "CLASS",
  "STRUCT",
  "INTERFACE",
  "UNION",
  "ENUM",
  "NAMESPACE",
  "MODULE",
  "PACKAGE",
]);

export const CARD_NODE_KINDS = new Set([...STRUCTURAL_KINDS, "FILE"]);

export const PRIVATE_MEMBER_KINDS = new Set([
  "FIELD",
  "VARIABLE",
  "GLOBAL_VARIABLE",
  "CONSTANT",
  "ENUM_CONSTANT",
]);

export const PUBLIC_MEMBER_KINDS = new Set(["FUNCTION", "METHOD", "MACRO"]);

export const FLOW_EDGE_KINDS = new Set<EdgeKind>([
  "CALL",
  "USAGE",
  "TYPE_USAGE",
  "IMPORT",
  "INCLUDE",
  "MACRO_USAGE",
  "ANNOTATION_USAGE",
  "UNKNOWN",
]);

export const HIERARCHY_EDGE_KINDS = new Set<EdgeKind>([
  "INHERITANCE",
  "OVERRIDE",
  "TYPE_ARGUMENT",
  "TEMPLATE_SPECIALIZATION",
]);

export function edgeFamilyForKind(kind: EdgeKind): SemanticEdgeFamily {
  if (HIERARCHY_EDGE_KINDS.has(kind)) {
    return "hierarchy";
  }
  return "flow";
}
