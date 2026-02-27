import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BaseEdge,
  Controls,
  EdgeLabelRenderer,
  Handle,
  MiniMap,
  Panel,
  Position,
  ReactFlow,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";

import { MermaidDiagram } from "../components/MermaidDiagram";
import type { EdgeKind, GraphArtifactDto } from "../generated/api";
import { buildDagreLayout } from "./layout/dagreLayout";
import { canonicalSeedFromGraphResponse } from "./layout/backendCanonical";
import { buildLegendRows, toReactFlowElements, type SemanticEdgeData } from "./layout/routing";
import type { GroupingMode, TrailUiConfig } from "./trailConfig";
import { STRUCTURAL_KINDS, type FlowNodeData } from "./layout/types";
import { ContextMenuPayload, ContextMenuState, contextMenuPosition } from "./viewport/contextMenu";
import {
  applyEdgeBusRouting,
  computeSemanticEdgePath,
  formatKindLabel,
  measuredHeight,
  measuredWidth,
  styleSemanticEdges,
} from "./viewport/edgeStyling";
import { useGraphExportActions } from "./viewport/exportActions";
import {
  directionFromKey,
  findNextEdgeSelection,
  findNextNodeSelection,
  isEditableTarget,
  type NavDirection,
} from "./viewport/keyboardNavigation";
import { buildFitViewTargets, fitViewSettings } from "./viewport/layoutState";

const MAX_VISIBLE_MEMBERS_PER_NODE = 6;
const GROUP_PADDING_X = 20;
const GROUP_PADDING_Y = 16;
const GROUP_HEADER_HEIGHT = 34;

const CODE_LANGUAGE_BY_EXT: Record<string, string> = {
  c: "c",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  cs: "csharp",
  go: "go",
  h: "cpp",
  hpp: "cpp",
  java: "java",
  js: "javascript",
  jsx: "javascript",
  kt: "kotlin",
  m: "objective-c",
  mm: "objective-cpp",
  php: "php",
  py: "python",
  rb: "ruby",
  rs: "rust",
  sh: "shell",
  sql: "sql",
  swift: "swift",
  ts: "typescript",
  tsx: "typescript",
  vue: "vue",
  xml: "xml",
  yaml: "yaml",
  yml: "yaml",
};

function isMermaidGraph(
  graph: GraphArtifactDto,
): graph is Extract<GraphArtifactDto, { kind: "mermaid" }> {
  return graph.kind === "mermaid";
}

const SIMPLE_TYPE_PILL_LABELS = new Set([
  "void",
  "bool",
  "byte",
  "char",
  "char8_t",
  "char16_t",
  "char32_t",
  "wchar_t",
  "short",
  "int",
  "long",
  "float",
  "double",
  "size_t",
  "ssize_t",
  "ptrdiff_t",
  "intptr_t",
  "uintptr_t",
  "u8",
  "u16",
  "u32",
  "u64",
  "u128",
  "i8",
  "i16",
  "i32",
  "i64",
  "i128",
  "f32",
  "f64",
  "string",
  "str",
  "object",
  "any",
  "unknown",
  "never",
  "null",
  "nil",
  "unit",
]);

function shortMemberDisplayLabel(label: string): string {
  const separatorIdx = label.lastIndexOf("::");
  if (separatorIdx < 0) {
    return label;
  }
  return label.slice(separatorIdx + 2);
}

function normalizeSimpleTypeLabel(label: string): string {
  const tailLabel = shortMemberDisplayLabel(label).toLowerCase();
  return tailLabel
    .replace(/\b(const|volatile|mut|signed|unsigned)\b/g, "")
    .replace(/[*&]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function isSimpleTypePillLabel(label: string): boolean {
  return SIMPLE_TYPE_PILL_LABELS.has(normalizeSimpleTypeLabel(label));
}

function minimapNodeColor(node: Node<FlowNodeData>): string {
  const data = node.data;
  if (data?.groupMode === "file") {
    return "#c6dfb4";
  }
  if (data?.groupMode === "namespace") {
    return "#efc7cb";
  }
  if (data?.isVirtualBundle) {
    return "#d4a63a";
  }
  if (data?.center) {
    return "#3d434b";
  }
  if (data?.kind === "FILE") {
    return "#9fc88e";
  }
  if (data?.nodeStyle === "card") {
    return "#c5cad2";
  }
  return "#d8dce3";
}

function minimapNodeStrokeColor(node: Node<FlowNodeData>): string {
  if (node.data?.groupMode === "file") {
    return "#96bb7e";
  }
  if (node.data?.groupMode === "namespace") {
    return "#dfb3b8";
  }
  return node.data?.center ? "#1f252c" : "#8f98a3";
}

export function languageForPath(path: string | null): string {
  if (!path) {
    return "plaintext";
  }
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return CODE_LANGUAGE_BY_EXT[ext] ?? "plaintext";
}

export function isTruncatedUmlGraph(graph: GraphArtifactDto | null): boolean {
  return graph !== null && !isMermaidGraph(graph) && graph.graph.truncated;
}

function nodeLabelFromData(data: unknown, fallback: string): string {
  if (typeof data !== "object" || data === null) {
    return fallback;
  }
  const candidate = (data as { label?: unknown }).label;
  return typeof candidate === "string" ? candidate : fallback;
}

function basename(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const segments = normalized.split("/").filter((segment) => segment.length > 0);
  return segments.at(-1) ?? normalized;
}

function namespaceLabelFromQualifiedName(qualifiedName: string | null | undefined): string | null {
  if (!qualifiedName) {
    return null;
  }
  const trimmed = qualifiedName.trim();
  if (trimmed.length === 0) {
    return null;
  }
  const idxCpp = trimmed.lastIndexOf("::");
  const idxDot = trimmed.lastIndexOf(".");
  const idx = Math.max(idxCpp, idxDot);
  if (idx <= 0) {
    return null;
  }
  return trimmed.slice(0, idx);
}

function groupLabelForNode(
  mode: GroupingMode,
  nodeMeta: Map<string, { filePath: string | null; qualifiedName: string | null }>,
  nodeId: string,
): string | null {
  const meta = nodeMeta.get(nodeId);
  if (!meta) {
    return null;
  }

  if (mode === "file") {
    const filePath = meta.filePath?.trim();
    if (!filePath) {
      return null;
    }
    return basename(filePath);
  }

  if (mode === "namespace") {
    return namespaceLabelFromQualifiedName(meta.qualifiedName);
  }

  return null;
}

function applyGrouping(
  nodes: Node<FlowNodeData>[],
  groupingMode: GroupingMode,
  nodeMeta: Map<string, { filePath: string | null; qualifiedName: string | null }>,
): Node<FlowNodeData>[] {
  if (groupingMode === "none") {
    return nodes;
  }

  const passthrough: Node<FlowNodeData>[] = [];
  const grouped = new Map<string, { label: string; nodes: Node<FlowNodeData>[] }>();
  for (const node of nodes) {
    if (node.data.isVirtualBundle || node.data.groupMode) {
      passthrough.push(node);
      continue;
    }

    const label = groupLabelForNode(groupingMode, nodeMeta, node.id);
    if (!label) {
      passthrough.push(node);
      continue;
    }

    const key = `${groupingMode}:${label}`;
    const bucket = grouped.get(key) ?? { label, nodes: [] };
    bucket.nodes.push(node);
    grouped.set(key, bucket);
  }

  if (grouped.size === 0) {
    return nodes;
  }

  const groupedNodes: Node<FlowNodeData>[] = [];
  const childNodes: Node<FlowNodeData>[] = [];

  for (const [key, bucket] of grouped) {
    let minX = Number.POSITIVE_INFINITY;
    let minY = Number.POSITIVE_INFINITY;
    let maxX = Number.NEGATIVE_INFINITY;
    let maxY = Number.NEGATIVE_INFINITY;

    for (const node of bucket.nodes) {
      const width = measuredWidth(node);
      const height = measuredHeight(node);
      minX = Math.min(minX, node.position.x);
      minY = Math.min(minY, node.position.y);
      maxX = Math.max(maxX, node.position.x + width);
      maxY = Math.max(maxY, node.position.y + height);
    }

    const groupX = minX - GROUP_PADDING_X;
    const groupY = minY - GROUP_HEADER_HEIGHT - GROUP_PADDING_Y;
    const groupId = `group:${key}`;
    const groupWidth = maxX - minX + GROUP_PADDING_X * 2;
    const groupHeight = maxY - minY + GROUP_PADDING_Y * 2 + GROUP_HEADER_HEIGHT;
    const layoutDirection = bucket.nodes[0]?.data.layoutDirection ?? "Horizontal";
    const preferredAnchor =
      bucket.nodes.find((node) => node.data.kind === "FILE") ??
      bucket.nodes.find((node) => STRUCTURAL_KINDS.has(node.data.kind)) ??
      bucket.nodes[0];
    const groupAnchorId = preferredAnchor?.id;

    groupedNodes.push({
      id: groupId,
      type: "sourcetrailGroup",
      position: { x: groupX, y: groupY },
      data: {
        kind: "GROUP",
        label: bucket.label,
        center: false,
        nodeStyle: "card",
        layoutDirection,
        duplicateCount: 1,
        memberCount: 0,
        members: [],
        isVirtualBundle: true,
        groupMode: groupingMode,
        groupLabel: bucket.label,
        groupAnchorId,
      },
      style: {
        width: groupWidth,
        height: groupHeight,
      },
      draggable: false,
      selectable: false,
      focusable: false,
      zIndex: -1,
    });

    for (const node of bucket.nodes) {
      childNodes.push({
        ...node,
        parentId: groupId,
        extent: "parent",
        position: {
          x: node.position.x - groupX,
          y: node.position.y - groupY,
        },
      });
    }
  }

  return [...groupedNodes, ...childNodes, ...passthrough];
}

function SemanticEdge({
  id: _id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  style,
  data,
}: EdgeProps<Edge<SemanticEdgeData>>) {
  const { path, labelX, labelY } = computeSemanticEdgePath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
    markerEnd,
    busTrunkX: data?.bundleTrunkX,
  });
  const showTooltip = Boolean(data?.tooltipLabel) && (data?.isHovered || data?.isFocused);

  return (
    <>
      <BaseEdge id={_id} path={path} markerEnd={markerEnd} style={style} />
      {showTooltip ? (
        <EdgeLabelRenderer>
          <div
            className="graph-edge-tooltip"
            style={{
              transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
            }}
          >
            {data?.tooltipLabel}
          </div>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
}

function GraphCardNode({ data, selected }: NodeProps<Node<FlowNodeData>>) {
  const isSelected = selected || data.isSelected === true;
  const horizontal = data.layoutDirection !== "Vertical";
  const targetNodePosition = horizontal ? Position.Left : Position.Top;
  const sourceNodePosition = horizontal ? Position.Right : Position.Bottom;
  const sourceSecondaryPosition = horizontal ? Position.Top : Position.Right;
  const targetSecondaryPosition = horizontal ? Position.Bottom : Position.Left;
  const targetMemberPosition = horizontal ? Position.Left : Position.Top;
  const sourceMemberPosition = horizontal ? Position.Right : Position.Bottom;

  if (data.nodeStyle === "bundle") {
    return (
      <div className="graph-bundle-node" aria-hidden>
        <Handle
          id="target-node-left"
          className="graph-handle graph-bundle-handle"
          type="target"
          position={targetNodePosition}
        />
        <Handle
          id="source-node-right"
          className="graph-handle graph-bundle-handle"
          type="source"
          position={sourceNodePosition}
        />
        <Handle
          id="source-node-top"
          className="graph-handle graph-bundle-handle"
          type="source"
          position={sourceSecondaryPosition}
        />
        <Handle
          id="target-node-bottom"
          className="graph-handle graph-bundle-handle"
          type="target"
          position={targetSecondaryPosition}
        />
      </div>
    );
  }

  if (data.nodeStyle === "pill") {
    const isSimpleTypePill = isSimpleTypePillLabel(data.label);
    const pillClassName = [
      "graph-floating-pill",
      data.center ? "graph-floating-pill-center" : "",
      isSimpleTypePill ? "graph-floating-pill-simple" : "",
      data.isNonIndexed ? "graph-node-unresolved" : "",
      isSelected ? "graph-floating-pill-selected" : "",
    ]
      .filter(Boolean)
      .join(" ");

    return (
      <div className={pillClassName}>
        <Handle
          id="target-node"
          className="graph-handle graph-handle-target"
          type="target"
          position={targetNodePosition}
        />
        <Handle
          id="source-node"
          className="graph-handle graph-handle-source"
          type="source"
          position={sourceNodePosition}
        />
        <Handle
          id="source-node-top"
          className="graph-handle graph-handle-top"
          type="source"
          position={sourceSecondaryPosition}
        />
        <Handle
          id="target-node-bottom"
          className="graph-handle graph-handle-bottom"
          type="target"
          position={targetSecondaryPosition}
        />
        <span>{data.label}</span>
        {data.duplicateCount > 1 ? (
          <span
            className="graph-pill-duplicate-count"
            title={`Merged symbols: ${(data.mergedSymbolIds ?? []).join(", ")}`}
          >
            x{data.duplicateCount}
          </span>
        ) : null}
      </div>
    );
  }

  const canToggleMembers = data.members.length > MAX_VISIBLE_MEMBERS_PER_NODE;
  const showAllMembers = data.center || isSelected || data.isExpanded === true || !canToggleMembers;
  const visibleMembers = showAllMembers
    ? data.members
    : data.members.slice(0, MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMemberHandleMembers = showAllMembers
    ? []
    : data.members.slice(MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMembers = data.members.length - visibleMembers.length;
  const publicMembers = visibleMembers.filter((member) => member.visibility === "public");
  const protectedMembers = visibleMembers.filter((member) => member.visibility === "protected");
  const privateMembers = visibleMembers.filter((member) => member.visibility === "private");
  const defaultMembers = visibleMembers.filter((member) => member.visibility === "default");
  const countTitle =
    typeof data.badgeVisibleMembers === "number" && typeof data.badgeTotalMembers === "number"
      ? `Visible members: ${data.badgeVisibleMembers} / total members: ${data.badgeTotalMembers}`
      : "Visible member count in current graph";
  const className = [
    "graph-node",
    isSelected ? "graph-node-selected" : "",
    data.center ? "graph-node-center" : "",
    data.kind === "FILE" ? "graph-node-file" : "",
    data.isNonIndexed ? "graph-node-unresolved" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const renderSection = (
    visibility: "public" | "protected" | "private" | "default",
    sectionLabel: string,
    members: FlowNodeData["members"],
  ): ReactNode => {
    if (members.length === 0) {
      return null;
    }

    return (
      <div className="graph-node-section">
        <div className="graph-node-section-header">
          <span className="graph-section-dot" aria-hidden>
            {visibility === "public"
              ? "○"
              : visibility === "protected"
                ? "◑"
                : visibility === "private"
                  ? "●"
                  : "◇"}
          </span>
          <span className="graph-node-section-title">{sectionLabel}</span>
          <span
            className={`graph-node-section-count graph-node-section-count-${visibility}`}
            title={`${members.length} ${sectionLabel.toLowerCase()} members`}
          >
            {members.length}
          </span>
        </div>
        <div className="graph-node-members">
          {members.map((member) => {
            const isFocusedMember = data.center && data.focusedMemberId === member.id;
            const memberLabel = shortMemberDisplayLabel(member.label);
            const chipTitle = `${member.label} (${formatKindLabel(member.kind)})`;
            return (
              <button
                type="button"
                key={member.id}
                className={[
                  "graph-member-chip",
                  "graph-member-chip-button",
                  `graph-member-chip-${visibility}`,
                  isFocusedMember ? "graph-member-chip-focused" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                title={chipTitle}
                aria-label={chipTitle}
                onClick={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  data.onSelectMember?.(member.id, member.label);
                }}
              >
                <Handle
                  id={`target-member-${member.id}`}
                  className="graph-handle graph-member-handle graph-member-handle-target"
                  type="target"
                  position={targetMemberPosition}
                />
                <span className="graph-member-name" title={member.label}>
                  {memberLabel}
                </span>
                <Handle
                  id={`source-member-${member.id}`}
                  className="graph-handle graph-member-handle graph-member-handle-source"
                  type="source"
                  position={sourceMemberPosition}
                />
              </button>
            );
          })}
        </div>
      </div>
    );
  };

  return (
    <div className={className}>
      <Handle
        id="target-node"
        className="graph-handle graph-handle-target"
        type="target"
        position={targetNodePosition}
      />
      <Handle
        id="source-node"
        className="graph-handle graph-handle-source"
        type="source"
        position={sourceNodePosition}
      />
      <Handle
        id="source-node-top"
        className="graph-handle graph-handle-top"
        type="source"
        position={sourceSecondaryPosition}
      />
      <Handle
        id="target-node-bottom"
        className="graph-handle graph-handle-bottom"
        type="target"
        position={targetSecondaryPosition}
      />
      {hiddenMemberHandleMembers.length > 0 ? (
        <div className="graph-hidden-member-handles" aria-hidden>
          {hiddenMemberHandleMembers.map((member) => (
            <div
              key={`hidden-member-handles-${member.id}`}
              className="graph-hidden-member-handle-pair"
            >
              <Handle
                id={`target-member-${member.id}`}
                className="graph-handle graph-member-handle graph-member-handle-target graph-hidden-member-handle"
                type="target"
                position={targetMemberPosition}
              />
              <Handle
                id={`source-member-${member.id}`}
                className="graph-handle graph-member-handle graph-member-handle-source graph-hidden-member-handle"
                type="source"
                position={sourceMemberPosition}
              />
            </div>
          ))}
        </div>
      ) : null}
      {data.kind === "FILE" ? <div className="graph-node-file-tab">{data.label}</div> : null}
      <div className="graph-node-title-row">
        <div className="graph-node-title" title={data.label}>
          {data.label}
        </div>
        {data.duplicateCount > 1 ? (
          <span
            className="graph-pill-duplicate-count"
            title={`Merged symbols: ${(data.mergedSymbolIds ?? []).join(", ")}`}
          >
            x{data.duplicateCount}
          </span>
        ) : null}
        {canToggleMembers ? (
          <button
            type="button"
            className="graph-node-toggle"
            aria-label={
              showAllMembers ? "Collapse members" : `Expand members (${hiddenMembers} hidden)`
            }
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              data.onToggleExpand?.();
            }}
          >
            <span className="graph-node-count" title={countTitle}>
              {Math.max(1, data.memberCount)}
            </span>
            <span className="graph-node-chevron">{showAllMembers ? "▾" : "▸"}</span>
          </button>
        ) : (
          <div className="graph-node-count" title={countTitle} aria-label={countTitle}>
            {Math.max(1, data.memberCount)}
          </div>
        )}
      </div>
      <div className="graph-node-body">
        {renderSection("public", "PUBLIC", publicMembers)}
        {renderSection("protected", "PROTECTED", protectedMembers)}
        {renderSection("private", "PRIVATE", privateMembers)}
        {renderSection("default", "DEFAULT", defaultMembers)}
        {visibleMembers.length === 0 ? (
          <div className="graph-node-section graph-node-section-empty">
            <div className="graph-node-section-header">
              <span className="graph-section-dot" />
              <span>{formatKindLabel(data.kind)}</span>
            </div>
            <div className="graph-node-members">
              <div className="graph-member-chip graph-member-chip-public">
                <Handle
                  id="target-node-chip"
                  className="graph-handle graph-member-handle graph-member-handle-target"
                  type="target"
                  position={targetMemberPosition}
                />
                <span className="graph-member-name">{data.label}</span>
                <Handle
                  id="source-node-chip"
                  className="graph-handle graph-member-handle graph-member-handle-source"
                  type="source"
                  position={sourceMemberPosition}
                />
              </div>
            </div>
          </div>
        ) : null}
        {hiddenMembers > 0 ? <div className="graph-member-more">+{hiddenMembers} more</div> : null}
      </div>
    </div>
  );
}

function GraphGroupNode({ data }: NodeProps<Node<FlowNodeData>>) {
  if (!data.groupMode || !data.groupLabel) {
    return null;
  }
  return (
    <button
      type="button"
      className={[
        "graph-group-node",
        data.groupMode === "file" ? "graph-group-node-file" : "graph-group-node-namespace",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        data.onSelectGroup?.();
      }}
      title={`Open ${data.groupLabel}`}
      aria-label={`Open ${data.groupLabel}`}
    >
      <div className="graph-group-label">{data.groupLabel}</div>
    </button>
  );
}

const GRAPH_NODE_TYPES = {
  sourcetrail: GraphCardNode,
  sourcetrailGroup: GraphGroupNode,
};

const GRAPH_EDGE_TYPES = {
  semantic: SemanticEdge,
};

type GraphViewportProps = {
  graph: GraphArtifactDto | null;
  onSelectNode: (nodeId: string, label: string) => void;
  onSelectEdge?: (selection: GraphEdgeSelection) => void;
  trailConfig: TrailUiConfig;
  onToggleLegend?: () => void;
  onOpenNodeInNewTab?: (nodeId: string, label: string) => void;
  onNavigateBack?: () => void;
  onNavigateForward?: () => void;
  onShowDefinitionInIde?: (nodeId: string) => void;
  onBookmarkNode?: (nodeId: string, label: string) => void;
  onOpenContainingFolder?: (path: string) => void;
  onRequestOpenTrailDialog?: () => void;
  onStatusMessage?: (message: string) => void;
};

export type GraphEdgeSelection = {
  id: string;
  edgeIds: string[];
  kind: EdgeKind;
  sourceNodeId: string;
  targetNodeId: string;
  sourceLabel: string;
  targetLabel: string;
};

export function GraphViewport({
  graph,
  onSelectNode,
  onSelectEdge,
  trailConfig,
  onToggleLegend,
  onOpenNodeInNewTab,
  onNavigateBack,
  onNavigateForward,
  onShowDefinitionInIde,
  onBookmarkNode,
  onOpenContainingFolder,
  onRequestOpenTrailDialog,
  onStatusMessage,
}: GraphViewportProps) {
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [hoveredEdgeId, setHoveredEdgeId] = useState<string | null>(null);
  const [legendFilterKinds, setLegendFilterKinds] = useState<Set<EdgeKind> | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [expandedNodeIdsByGraph, setExpandedNodeIdsByGraph] = useState<Record<string, string[]>>(
    {},
  );
  const [hiddenNodeIdsByGraph, setHiddenNodeIdsByGraph] = useState<Record<string, string[]>>({});
  const [hiddenEdgeIdsByGraph, setHiddenEdgeIdsByGraph] = useState<Record<string, string[]>>({});
  const [manualNodePositionsByGraph, setManualNodePositionsByGraph] = useState<
    Record<string, Record<string, { x: number; y: number }>>
  >({});
  const flowShellRef = useRef<HTMLDivElement | null>(null);
  const lastFittedGraphId = useRef<string | null>(null);
  const activeGraphNodePositions = useMemo(() => {
    if (!graph || isMermaidGraph(graph)) {
      return {};
    }
    return manualNodePositionsByGraph[graph.id] ?? {};
  }, [graph, manualNodePositionsByGraph]);
  const expandedNodeIds = useMemo(() => {
    if (!graph || isMermaidGraph(graph)) {
      return new Set<string>();
    }
    return new Set(expandedNodeIdsByGraph[graph.id] ?? []);
  }, [expandedNodeIdsByGraph, graph]);
  const hiddenNodeIds = useMemo(() => {
    if (!graph || isMermaidGraph(graph)) {
      return new Set<string>();
    }
    return new Set(hiddenNodeIdsByGraph[graph.id] ?? []);
  }, [graph, hiddenNodeIdsByGraph]);
  const hiddenEdgeIds = useMemo(() => {
    if (!graph || isMermaidGraph(graph)) {
      return new Set<string>();
    }
    return new Set(hiddenEdgeIdsByGraph[graph.id] ?? []);
  }, [graph, hiddenEdgeIdsByGraph]);
  const nodeMetaById = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return new Map<string, { filePath: string | null; qualifiedName: string | null }>();
    }
    return new Map(
      graph.graph.nodes.map((node) => [
        node.id,
        {
          filePath: node.file_path ?? null,
          qualifiedName: node.qualified_name ?? null,
        },
      ]),
    );
  }, [graph]);
  const nodeKindById = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return new Map<string, string>();
    }
    return new Map(graph.graph.nodes.map((node) => [node.id, node.kind]));
  }, [graph]);
  const toggleExpandedNode = useCallback(
    (nodeId: string) => {
      if (!graph || isMermaidGraph(graph)) {
        return;
      }
      setExpandedNodeIdsByGraph((previous) => {
        const current = new Set(previous[graph.id] ?? []);
        if (current.has(nodeId)) {
          current.delete(nodeId);
        } else {
          current.add(nodeId);
        }
        return {
          ...previous,
          [graph.id]: [...current],
        };
      });
    },
    [graph],
  );

  const canonicalSeedResult = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return { seed: null, error: null };
    }
    return canonicalSeedFromGraphResponse(graph.graph);
  }, [graph]);
  const dagreSeedMemo = canonicalSeedResult.seed;
  const canonicalLayoutError = canonicalSeedResult.error;
  const dagreLayoutMemo = useMemo(() => {
    if (!dagreSeedMemo) {
      return null;
    }
    return buildDagreLayout(dagreSeedMemo, trailConfig.layoutDirection);
  }, [dagreSeedMemo, trailConfig.layoutDirection]);
  const baseFlowLayoutMemo = useMemo(() => {
    if (!dagreLayoutMemo) {
      return null;
    }
    return toReactFlowElements(dagreLayoutMemo, trailConfig.layoutDirection);
  }, [dagreLayoutMemo, trailConfig.layoutDirection]);

  const flowElements = useMemo(() => {
    if (graph === null || isMermaidGraph(graph) || baseFlowLayoutMemo === null) {
      return null;
    }

    const centerMemberId = graph.graph.center_id;

    const withInteractiveNodeData = (node: Node<FlowNodeData>): Node<FlowNodeData> => {
      const focusedMemberId =
        node.data.center && node.data.members.some((member) => member.id === centerMemberId)
          ? centerMemberId
          : null;
      return {
        ...node,
        data: {
          ...node.data,
          layoutDirection: trailConfig.layoutDirection,
          isSelected: node.id === selectedNodeId,
          isExpanded: expandedNodeIds.has(node.id),
          focusedMemberId,
          onSelectMember: (memberId: string, label: string) => {
            setSelectedNodeId(node.id);
            setSelectedEdgeId(null);
            onSelectNode(memberId, label);
          },
          onToggleExpand: () => toggleExpandedNode(node.id),
        },
      };
    };

    const applyManualPosition = (node: Node<FlowNodeData>): Node<FlowNodeData> => {
      const manualPosition = activeGraphNodePositions[node.id];
      if (!manualPosition) {
        return node;
      }
      return {
        ...node,
        position: manualPosition,
      };
    };

    const hideUnknownByDefault =
      graph.id.startsWith("explore-") && !trailConfig.nodeFilter.includes("UNKNOWN");

    const groupedNodes = applyGrouping(
      baseFlowLayoutMemo.nodes
        .filter(
          (node) =>
            !(
              hideUnknownByDefault &&
              node.id !== baseFlowLayoutMemo.centerNodeId &&
              node.data.kind === "UNKNOWN"
            ),
        )
        .map(withInteractiveNodeData)
        .map(applyManualPosition),
      trailConfig.groupingMode,
      nodeMetaById,
    ).map((node) => {
      if (!node.data.groupMode || !node.data.groupAnchorId) {
        return node;
      }
      return {
        ...node,
        data: {
          ...node.data,
          layoutDirection: trailConfig.layoutDirection,
          onSelectGroup: () => {
            const anchorId = node.data.groupAnchorId;
            if (!anchorId) {
              return;
            }
            const anchorMeta = nodeMetaById.get(anchorId);
            onSelectNode(anchorId, anchorMeta?.qualifiedName ?? anchorId);
          },
        },
      };
    });
    const visibleNodes = groupedNodes.filter((node) => !hiddenNodeIds.has(node.id));
    const visibleNodeIds = new Set(visibleNodes.map((node) => node.id));
    const visibleEdges = baseFlowLayoutMemo.edges.filter(
      (edge) =>
        !hiddenEdgeIds.has(edge.id) &&
        visibleNodeIds.has(edge.source) &&
        visibleNodeIds.has(edge.target),
    );
    const routedEdges = trailConfig.bundleEdges
      ? applyEdgeBusRouting(visibleEdges, visibleNodes, trailConfig.layoutDirection)
      : visibleEdges;
    return {
      ...baseFlowLayoutMemo,
      nodes: visibleNodes,
      edges: styleSemanticEdges({
        edges: routedEdges,
        centerNodeId: baseFlowLayoutMemo.centerNodeId,
        nodeCount: visibleNodes.length,
        depth: trailConfig.depth,
        selectedEdgeId,
        hoveredEdgeId,
        legendFilterKinds,
      }),
    };
  }, [
    activeGraphNodePositions,
    baseFlowLayoutMemo,
    expandedNodeIds,
    graph,
    hiddenEdgeIds,
    hiddenNodeIds,
    hoveredEdgeId,
    legendFilterKinds,
    nodeMetaById,
    onSelectNode,
    selectedEdgeId,
    selectedNodeId,
    trailConfig.bundleEdges,
    trailConfig.depth,
    trailConfig.groupingMode,
    trailConfig.layoutDirection,
    trailConfig.nodeFilter,
    toggleExpandedNode,
  ]);
  const flowNodesById = useMemo(() => {
    return new Map(flowElements?.nodes.map((node) => [node.id, node]) ?? []);
  }, [flowElements?.nodes]);
  const flowEdgesById = useMemo(() => {
    return new Map(flowElements?.edges.map((edge) => [edge.id, edge]) ?? []);
  }, [flowElements?.edges]);
  const selectNodeById = useCallback(
    (nodeId: string) => {
      const node = flowNodesById.get(nodeId);
      if (!node) {
        return;
      }
      setSelectedNodeId(nodeId);
      setSelectedEdgeId(null);
      onSelectNode(nodeId, nodeLabelFromData(node.data, nodeId));
    },
    [flowNodesById, onSelectNode],
  );
  const activateEdge = useCallback(
    (edge: Edge<SemanticEdgeData>) => {
      setSelectedEdgeId(edge.id);
      const sourceNode = flowNodesById.get(edge.source);
      const targetNode = flowNodesById.get(edge.target);
      const semanticEdgeData = edge.data;

      if (onSelectEdge && semanticEdgeData?.edgeKind) {
        const sourceEdgeIds =
          semanticEdgeData.sourceEdgeIds.length > 0 ? semanticEdgeData.sourceEdgeIds : [edge.id];
        const primaryEdgeId = sourceEdgeIds[0] ?? edge.id;
        onSelectEdge({
          id: primaryEdgeId,
          edgeIds: sourceEdgeIds,
          kind: semanticEdgeData.edgeKind,
          sourceNodeId: edge.source,
          targetNodeId: edge.target,
          sourceLabel: sourceNode ? nodeLabelFromData(sourceNode.data, edge.source) : edge.source,
          targetLabel: targetNode ? nodeLabelFromData(targetNode.data, edge.target) : edge.target,
        });
        return;
      }

      const centerNodeId = flowElements?.centerNodeId;
      const preferredNodeId =
        edge.source === centerNodeId
          ? edge.target
          : edge.target === centerNodeId
            ? edge.source
            : edge.target;
      const fallbackNode =
        flowNodesById.get(preferredNodeId) ??
        flowNodesById.get(edge.target) ??
        flowNodesById.get(edge.source);
      if (!fallbackNode) {
        return;
      }
      setSelectedNodeId(fallbackNode.id);
      onSelectNode(fallbackNode.id, nodeLabelFromData(fallbackNode.data, fallbackNode.id));
    },
    [flowElements?.centerNodeId, flowNodesById, onSelectEdge, onSelectNode],
  );
  const closeContextMenu = useCallback(() => {
    setContextMenu(null);
  }, []);
  const openContextMenu = useCallback(
    (
      event: {
        clientX: number;
        clientY: number;
        preventDefault: () => void;
        stopPropagation: () => void;
      },
      state: ContextMenuPayload,
    ) => {
      event.preventDefault();
      event.stopPropagation();
      const shellRect = flowShellRef.current?.getBoundingClientRect();
      const { x, y } = contextMenuPosition(event.clientX, event.clientY, shellRect);
      setContextMenu({
        ...state,
        x,
        y,
      } as ContextMenuState);
    },
    [],
  );
  const hideNode = useCallback(
    (nodeId: string) => {
      if (!graph || isMermaidGraph(graph)) {
        return;
      }
      setHiddenNodeIdsByGraph((previous) => {
        const current = new Set(previous[graph.id] ?? []);
        current.add(nodeId);
        return {
          ...previous,
          [graph.id]: [...current],
        };
      });
      setSelectedNodeId((current) => (current === nodeId ? null : current));
      onStatusMessage?.("Node hidden. Use Reset Hidden in the context menu to restore.");
    },
    [graph, onStatusMessage],
  );
  const hideEdge = useCallback(
    (edgeId: string) => {
      if (!graph || isMermaidGraph(graph)) {
        return;
      }
      setHiddenEdgeIdsByGraph((previous) => {
        const current = new Set(previous[graph.id] ?? []);
        current.add(edgeId);
        return {
          ...previous,
          [graph.id]: [...current],
        };
      });
      setSelectedEdgeId((current) => (current === edgeId ? null : current));
      onStatusMessage?.("Edge hidden. Use Reset Hidden in the context menu to restore.");
    },
    [graph, onStatusMessage],
  );
  const resetHidden = useCallback(() => {
    if (!graph || isMermaidGraph(graph)) {
      return;
    }
    setHiddenNodeIdsByGraph((previous) => {
      if (!previous[graph.id]) {
        return previous;
      }
      const next = { ...previous };
      delete next[graph.id];
      return next;
    });
    setHiddenEdgeIdsByGraph((previous) => {
      if (!previous[graph.id]) {
        return previous;
      }
      const next = { ...previous };
      delete next[graph.id];
      return next;
    });
    onStatusMessage?.("Hidden graph elements restored.");
  }, [graph, onStatusMessage]);
  const { exportImage, exportToClipboard, copyText } = useGraphExportActions({
    graphTitle: graph?.title ?? "graph",
    flowShellRef,
    onStatusMessage,
  });
  const moveNodeSelection = useCallback(
    (direction: NavDirection) => {
      const graphNodes = flowElements?.nodes.filter((node) => !node.data.groupMode) ?? [];
      const nextNode = findNextNodeSelection(direction, graphNodes, selectedNodeId, flowNodesById);
      if (!nextNode) {
        return;
      }
      setSelectedNodeId(nextNode.id);
      setSelectedEdgeId(null);
    },
    [flowElements?.nodes, flowNodesById, selectedNodeId],
  );
  const moveEdgeSelection = useCallback(
    (direction: NavDirection) => {
      const edges = flowElements?.edges ?? [];
      const nextEdge = findNextEdgeSelection(
        direction,
        edges,
        selectedEdgeId,
        selectedNodeId,
        flowNodesById,
        flowEdgesById,
      );
      if (!nextEdge) {
        return;
      }
      setSelectedNodeId(null);
      setSelectedEdgeId(nextEdge.id);
    },
    [flowEdgesById, flowElements?.edges, flowNodesById, selectedEdgeId, selectedNodeId],
  );

  useEffect(() => {
    setSelectedNodeId(flowElements?.centerNodeId ?? null);
    setSelectedEdgeId(null);
    setHoveredEdgeId(null);
    setLegendFilterKinds(null);
    setContextMenu(null);
  }, [graph?.id, flowElements?.centerNodeId]);

  useEffect(() => {
    if (!flow || !graph || isMermaidGraph(graph) || !flowElements) {
      return;
    }

    if (lastFittedGraphId.current === graph.id) {
      return;
    }

    const { fitPadding, fitMaxZoom } = fitViewSettings(flowElements.nodes.length);
    const focusNodes = buildFitViewTargets(flowElements);
    window.requestAnimationFrame(() => {
      void flow.fitView({
        duration: 260,
        maxZoom: fitMaxZoom,
        minZoom: 0.28,
        padding: fitPadding,
        nodes: focusNodes.length > 0 ? focusNodes : undefined,
      });
    });

    lastFittedGraphId.current = graph.id;
  }, [flow, flowElements, graph]);

  const zoomIn = () => {
    if (!flow) {
      return;
    }
    void flow.zoomIn({ duration: 140 });
  };

  const zoomOut = () => {
    if (!flow) {
      return;
    }
    void flow.zoomOut({ duration: 140 });
  };

  const resetZoom = () => {
    if (!flow) {
      return;
    }
    const viewport = flow.getViewport();
    void flow.setViewport({ ...viewport, zoom: 1 }, { duration: 180 });
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditableTarget(event.target)) {
        return;
      }
      const key = event.key;
      const normalized = key.toLowerCase();

      if ((event.ctrlKey || event.metaKey) && normalized === "u") {
        event.preventDefault();
        onRequestOpenTrailDialog?.();
        return;
      }

      if (key === "?" || (event.shiftKey && key === "/")) {
        event.preventDefault();
        setLegendFilterKinds(null);
        onToggleLegend?.();
        return;
      }

      if (key === "0") {
        event.preventDefault();
        resetZoom();
        return;
      }

      if (key === "+" || key === "=") {
        event.preventDefault();
        zoomIn();
        return;
      }

      if (key === "-" || key === "_") {
        event.preventDefault();
        zoomOut();
        return;
      }

      const direction = directionFromKey(key);
      if (direction) {
        event.preventDefault();
        if (event.shiftKey) {
          moveEdgeSelection(direction);
        } else {
          moveNodeSelection(direction);
        }
        return;
      }

      if (key === "Enter" || normalized === "e") {
        event.preventDefault();
        if ((event.ctrlKey || event.metaKey) && event.shiftKey && selectedNodeId) {
          const node = flowNodesById.get(selectedNodeId);
          onOpenNodeInNewTab?.(
            selectedNodeId,
            node ? nodeLabelFromData(node.data, selectedNodeId) : selectedNodeId,
          );
          return;
        }

        if (event.shiftKey) {
          if (selectedNodeId) {
            toggleExpandedNode(selectedNodeId);
          }
          return;
        }

        if (selectedEdgeId) {
          const edge = flowEdgesById.get(selectedEdgeId);
          if (edge) {
            activateEdge(edge);
          }
          return;
        }
        if (selectedNodeId) {
          selectNodeById(selectedNodeId);
        }
        return;
      }

      if (key === "Escape") {
        closeContextMenu();
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [
    activateEdge,
    closeContextMenu,
    flowEdgesById,
    flowNodesById,
    moveEdgeSelection,
    moveNodeSelection,
    onOpenNodeInNewTab,
    onRequestOpenTrailDialog,
    onToggleLegend,
    resetZoom,
    selectNodeById,
    selectedEdgeId,
    selectedNodeId,
    toggleExpandedNode,
    zoomIn,
    zoomOut,
  ]);

  const toggleLegendKind = (kind: EdgeKind) => {
    setLegendFilterKinds((previous) => {
      if (previous === null) {
        return new Set([kind]);
      }

      const next = new Set(previous);
      if (next.has(kind)) {
        next.delete(kind);
      } else {
        next.add(kind);
      }

      return next.size > 0 ? next : null;
    });
  };

  if (graph === null) {
    return <div className="graph-empty">Pick a symbol or submit a prompt to render a graph.</div>;
  }
  if (isMermaidGraph(graph)) {
    return <MermaidDiagram syntax={graph.mermaid_syntax} />;
  }
  if (canonicalLayoutError !== null) {
    return <div className="graph-empty">Unable to render UML graph: {canonicalLayoutError}</div>;
  }
  if (graph.graph.nodes.length === 0) {
    return <div className="graph-empty">No UML nodes were returned for this symbol yet.</div>;
  }
  if (flowElements === null) {
    return <div className="graph-empty">Unable to render UML graph layout.</div>;
  }

  const legendRows = buildLegendRows(graph.graph);
  const hasUncertainEdges = legendRows.some((row) => row.hasUncertain);
  const hasProbableEdges = legendRows.some((row) => row.hasProbable);
  const denseGraph = flowElements.nodes.length > 64;
  const hasHiddenElements = hiddenNodeIds.size > 0 || hiddenEdgeIds.size > 0;
  const contextNode =
    contextMenu?.kind === "node" ? (flowNodesById.get(contextMenu.nodeId) ?? null) : null;
  const contextNodeIsExpandable =
    contextNode?.data.nodeStyle === "card" &&
    contextNode.data.members.length > MAX_VISIBLE_MEMBERS_PER_NODE;

  return (
    <div
      ref={flowShellRef}
      className="graph-flow-shell"
      onClick={() => {
        if (contextMenu) {
          closeContextMenu();
        }
      }}
    >
      <ReactFlow
        key={graph.id}
        onInit={setFlow}
        nodes={flowElements.nodes}
        edges={flowElements.edges}
        onNodeClick={(_, node) => {
          const data = node.data as FlowNodeData | undefined;
          if (data?.isVirtualBundle || data?.groupMode) {
            return;
          }
          setSelectedNodeId(node.id);
          setSelectedEdgeId(null);
          onSelectNode(node.id, nodeLabelFromData(node.data, node.id));
        }}
        onNodeContextMenu={(event, node) => {
          const data = node.data as FlowNodeData | undefined;
          const isGroup = Boolean(data?.groupMode);
          const anchorId = data?.groupAnchorId ?? null;
          const resolvedNodeId = isGroup && anchorId ? anchorId : node.id;
          const resolvedKind = isGroup ? nodeKindById.get(resolvedNodeId) : data?.kind;
          const resolvedLabel = isGroup
            ? (nodeMetaById.get(resolvedNodeId)?.qualifiedName ??
              nodeMetaById.get(resolvedNodeId)?.filePath ??
              nodeLabelFromData(node.data, node.id))
            : nodeLabelFromData(node.data, node.id);
          const resolvedPath = nodeMetaById.get(resolvedNodeId)?.filePath ?? null;
          if (!isGroup) {
            setSelectedNodeId(node.id);
            setSelectedEdgeId(null);
          }
          openContextMenu(event, {
            kind: "node",
            nodeId: resolvedNodeId,
            label: resolvedLabel,
            filePath: resolvedPath,
            isFile: resolvedKind === "FILE",
            isGroup,
            groupAnchorId: anchorId,
          });
        }}
        onNodeDragStop={(_, node) => {
          const data = node.data as FlowNodeData | undefined;
          if (data?.isVirtualBundle) {
            return;
          }
          const absolutePosition = (
            node as Node<FlowNodeData> & { positionAbsolute?: { x: number; y: number } }
          ).positionAbsolute;
          const persistedPosition = absolutePosition ?? node.position;
          setManualNodePositionsByGraph((previous) => ({
            ...previous,
            [graph.id]: {
              ...previous[graph.id],
              [node.id]: {
                x: persistedPosition.x,
                y: persistedPosition.y,
              },
            },
          }));
        }}
        onPaneClick={() => {
          setSelectedNodeId(null);
          setSelectedEdgeId(null);
          closeContextMenu();
        }}
        onPaneContextMenu={(event) => {
          openContextMenu(event, { kind: "pane" });
        }}
        onEdgeClick={(event, edge) => {
          const mouseEvent = event as { button?: number; altKey?: boolean };
          if (mouseEvent.button === 2) {
            return;
          }
          if (mouseEvent.altKey) {
            setSelectedNodeId(null);
            hideEdge(edge.id);
            closeContextMenu();
            return;
          }
          activateEdge(edge as Edge<SemanticEdgeData>);
        }}
        onEdgeContextMenu={(event, edge) => {
          setSelectedNodeId(null);
          setSelectedEdgeId(edge.id);
          openContextMenu(event, { kind: "edge", edgeId: edge.id });
        }}
        onEdgeMouseEnter={(_, edge) => {
          setHoveredEdgeId(edge.id);
        }}
        onEdgeMouseLeave={() => {
          setHoveredEdgeId(null);
        }}
        nodeTypes={GRAPH_NODE_TYPES}
        edgeTypes={GRAPH_EDGE_TYPES}
        minZoom={0.18}
        maxZoom={2.1}
        proOptions={{ hideAttribution: true }}
        nodesDraggable
        nodesConnectable={false}
        elementsSelectable
        onlyRenderVisibleElements={denseGraph}
        fitView={false}
        className="sourcetrail-flow"
      >
        <Controls position="top-left" showInteractive={false} />
        <Panel position="bottom-left" className="graph-zoom-panel">
          <button type="button" aria-label="Zoom in" onClick={zoomIn}>
            +
          </button>
          <button type="button" aria-label="Zoom out" onClick={zoomOut}>
            -
          </button>
          <button type="button" aria-label="Reset zoom to 100%" onClick={resetZoom}>
            0
          </button>
        </Panel>
        {trailConfig.showMiniMap ? (
          <MiniMap
            className="graph-minimap"
            pannable
            zoomable
            position="bottom-left"
            bgColor="rgb(251 251 249 / 0.92)"
            maskColor="rgb(39 44 52 / 0.16)"
            nodeColor={minimapNodeColor}
            nodeStrokeColor={minimapNodeStrokeColor}
            nodeBorderRadius={2}
          />
        ) : null}
        <Panel position="bottom-right" className="graph-legend-toggle-panel">
          <button
            type="button"
            aria-label={trailConfig.showLegend ? "Hide legend" : "Show legend"}
            onClick={() => {
              setLegendFilterKinds(null);
              onToggleLegend?.();
            }}
          >
            {trailConfig.showLegend ? "×" : "?"}
          </button>
        </Panel>
        {trailConfig.showLegend && legendRows.length > 0 ? (
          <Panel position="bottom-right" className="graph-legend-panel">
            <div className="graph-legend-title">Legend</div>
            <div className="graph-legend-rows">
              {legendRows.map((row) => (
                <button
                  key={row.kind}
                  type="button"
                  className={[
                    "graph-legend-row",
                    legendFilterKinds !== null && !legendFilterKinds.has(row.kind)
                      ? "graph-legend-row-muted"
                      : "graph-legend-row-active",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={() => toggleLegendKind(row.kind)}
                >
                  <span className="graph-legend-line" style={{ background: row.stroke }} />
                  <span className="graph-legend-kind">{formatKindLabel(row.kind)}</span>
                  <span className="graph-legend-count">{row.count}</span>
                </button>
              ))}
            </div>
            <div className="graph-legend-note">
              Edge thickness may indicate merged parallel relationships.
              {hasUncertainEdges || hasProbableEdges ? " " : ""}
              {hasUncertainEdges ? "Dashed edges = uncertain." : ""}
              {hasUncertainEdges && hasProbableEdges ? " " : ""}
              {hasProbableEdges ? "Lower opacity = probable." : ""}
              {legendFilterKinds !== null ? (
                <button
                  type="button"
                  className="graph-legend-reset"
                  onClick={() => setLegendFilterKinds(null)}
                >
                  Show all
                </button>
              ) : null}
            </div>
          </Panel>
        ) : null}
      </ReactFlow>
      {contextMenu ? (
        <div
          className="graph-context-menu"
          style={{ left: Math.max(8, contextMenu.x), top: Math.max(8, contextMenu.y) }}
          onContextMenu={(event) => event.preventDefault()}
        >
          {contextMenu.kind === "pane" ? (
            <>
              <button type="button" onClick={() => onNavigateBack?.()}>
                Back
              </button>
              <button type="button" onClick={() => onNavigateForward?.()}>
                Forward
              </button>
              <button type="button" onClick={() => void exportImage("png")}>
                Save Image (PNG)
              </button>
              <button type="button" onClick={() => void exportImage("jpeg")}>
                Save Image (JPEG)
              </button>
              <button type="button" onClick={() => void exportImage("svg")}>
                Save Image (SVG)
              </button>
              <button type="button" onClick={() => void exportToClipboard()}>
                Save To Clipboard (PNG)
              </button>
              {hasHiddenElements ? (
                <button type="button" onClick={resetHidden}>
                  Reset Hidden
                </button>
              ) : null}
            </>
          ) : null}
          {contextMenu.kind === "edge" ? (
            <>
              <button
                type="button"
                onClick={() => {
                  const edge = flowEdgesById.get(contextMenu.edgeId);
                  if (edge) {
                    activateEdge(edge);
                  }
                }}
              >
                Show Definition
              </button>
              <button type="button" onClick={() => hideEdge(contextMenu.edgeId)}>
                Hide Edge
              </button>
              <button type="button" onClick={() => void exportImage("png")}>
                Save Image (PNG)
              </button>
              <button type="button" onClick={() => void exportToClipboard()}>
                Save To Clipboard (PNG)
              </button>
            </>
          ) : null}
          {contextMenu.kind === "node" ? (
            <>
              <button type="button" onClick={() => selectNodeById(contextMenu.nodeId)}>
                Show Definition
              </button>
              <button
                type="button"
                onClick={() => onOpenNodeInNewTab?.(contextMenu.nodeId, contextMenu.label)}
              >
                Open In New Tab
              </button>
              <button type="button" onClick={() => onShowDefinitionInIde?.(contextMenu.nodeId)}>
                Show Definition In IDE
              </button>
              {contextNodeIsExpandable ? (
                <button type="button" onClick={() => toggleExpandedNode(contextMenu.nodeId)}>
                  {contextNode?.data.isExpanded ? "Collapse Node" : "Expand Node"}
                </button>
              ) : null}
              <button type="button" onClick={() => hideNode(contextMenu.nodeId)}>
                Hide Node
              </button>
              <button
                type="button"
                onClick={() => onBookmarkNode?.(contextMenu.nodeId, contextMenu.label)}
              >
                Bookmark Node
              </button>
              <button
                type="button"
                onClick={() => void copyText(contextMenu.label, "Name copied to clipboard.")}
              >
                Copy Name
              </button>
              {contextMenu.isFile && contextMenu.filePath ? (
                <>
                  <button
                    type="button"
                    onClick={() =>
                      void copyText(contextMenu.filePath ?? "", "Full path copied to clipboard.")
                    }
                  >
                    Copy Full Path
                  </button>
                  <button
                    type="button"
                    onClick={() => onOpenContainingFolder?.(contextMenu.filePath ?? "")}
                  >
                    Open Containing Folder
                  </button>
                </>
              ) : null}
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
