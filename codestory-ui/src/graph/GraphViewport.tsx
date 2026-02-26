import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
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
import mermaid from "mermaid";

import type { EdgeKind, GraphArtifactDto } from "../generated/api";
import { applyAdaptiveBundling } from "./layout/bundling";
import { routeEdgesWithObstacles } from "./layout/obstacleRouting";
import { buildFallbackLayout, buildSemanticLayout } from "./layout/semanticLayout";
import { buildLegendRows, toReactFlowElements, type SemanticEdgeData } from "./layout/routing";
import type { GroupingMode, TrailUiConfig } from "./trailConfig";
import type { FlowNodeData } from "./layout/types";

mermaid.initialize({
  startOnLoad: false,
  theme: "neutral",
  securityLevel: "loose",
});

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

function formatKindLabel(kind: string): string {
  return kind
    .toLowerCase()
    .split("_")
    .map((segment) => `${segment.slice(0, 1).toUpperCase()}${segment.slice(1)}`)
    .join(" ");
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

function measuredWidth(node: Node<FlowNodeData>): number {
  if (typeof node.width === "number") {
    return node.width;
  }
  if (node.data.nodeStyle === "card") {
    return 240;
  }
  return 132;
}

function measuredHeight(node: Node<FlowNodeData>): number {
  if (typeof node.height === "number") {
    return node.height;
  }
  if (node.data.nodeStyle === "card") {
    return 160;
  }
  return 42;
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

    groupedNodes.push({
      id: groupId,
      type: "sourcetrailGroup",
      position: { x: groupX, y: groupY },
      data: {
        kind: "GROUP",
        label: bucket.label,
        center: false,
        nodeStyle: "card",
        duplicateCount: 1,
        memberCount: 0,
        members: [],
        isVirtualBundle: true,
        groupMode: groupingMode,
        groupLabel: bucket.label,
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

function orthogonalPath(points: Array<{ x: number; y: number }>): string {
  if (points.length === 0) {
    return "";
  }
  const [first, ...rest] = points;
  if (!first) {
    return "";
  }

  let path = `M ${first.x} ${first.y}`;
  for (const point of rest) {
    path += ` L ${point.x} ${point.y}`;
  }
  return path;
}

function hash32(value: string): number {
  let hash = 2166136261;
  for (let idx = 0; idx < value.length; idx += 1) {
    hash ^= value.charCodeAt(idx);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function laneOffsetFromEdgeId(edgeId: string, step: number, radius: number): number {
  const slots = radius * 2 + 1;
  const slot = hash32(edgeId) % slots;
  return (slot - radius) * step;
}

function clampedElbowX(sourceX: number, targetX: number, desiredX: number, gutter = 10): number {
  if (targetX >= sourceX) {
    const min = sourceX + gutter;
    const max = Math.max(min, targetX - gutter);
    return Math.min(max, Math.max(min, desiredX));
  }

  const max = sourceX - gutter;
  const min = Math.min(max, targetX + gutter);
  return Math.max(min, Math.min(max, desiredX));
}

function buildEdgePath(
  edgeId: string,
  sourceX: number,
  sourceY: number,
  targetX: number,
  targetY: number,
  data: SemanticEdgeData | undefined,
): { path: string; labelX: number; labelY: number } {
  const r = 14;

  const makeRoundedOrthogonal = (points: Array<{ x: number; y: number }>) => {
    if (points.length < 3) return orthogonalPath(points);
    const firstPoint = points[0]!;
    let path = `M ${firstPoint.x} ${firstPoint.y}`;
    for (let i = 1; i < points.length - 1; i++) {
      const prev = points[i - 1]!;
      const curr = points[i]!;
      const next = points[i + 1]!;

      // Calculate the distances to check if we have enough room for the radius
      const dist1 = Math.sqrt((curr.x - prev.x) ** 2 + (curr.y - prev.y) ** 2);
      const dist2 = Math.sqrt((next.x - curr.x) ** 2 + (next.y - curr.y) ** 2);
      const radius = Math.min(r, dist1 / 2, dist2 / 2);

      // Directions
      const dir1X = Math.sign(curr.x - prev.x);
      const dir1Y = Math.sign(curr.y - prev.y);
      const dir2X = Math.sign(next.x - curr.x);
      const dir2Y = Math.sign(next.y - curr.y);

      // Arc start and end points
      const arcStartX = curr.x - dir1X * radius;
      const arcStartY = curr.y - dir1Y * radius;
      const arcEndX = curr.x + dir2X * radius;
      const arcEndY = curr.y + dir2Y * radius;

      // Determine sweep flag based on cross product
      const crossProduct = dir1X * dir2Y - dir1Y * dir2X;
      const sweepFlag = crossProduct > 0 ? 1 : 0;

      path += ` L ${arcStartX} ${arcStartY} A ${radius} ${radius} 0 0 ${sweepFlag} ${arcEndX} ${arcEndY}`;
    }
    const last = points[points.length - 1]!;
    path += ` L ${last.x} ${last.y}`;
    return path;
  };
  const routedPoints = data?.routePoints ?? [];
  if (routedPoints.length >= 2) {
    const points = routedPoints.map((point) => ({ ...point }));
    points[0] = { x: sourceX, y: sourceY };
    points[points.length - 1] = { x: targetX, y: targetY };
    const path = makeRoundedOrthogonal(points);
    const mid = points[Math.floor(points.length / 2)] ?? {
      x: (sourceX + targetX) / 2,
      y: (sourceY + targetY) / 2,
    };
    return { path, labelX: mid.x, labelY: mid.y };
  }

  const routeKind = data?.routeKind ?? "direct";
  const laneSpanY = Math.abs(targetY - sourceY);
  const laneOffset = laneSpanY > 20 ? laneOffsetFromEdgeId(edgeId, 8, 5) : 0;
  if (routeKind === "flow-trunk" || routeKind === "flow-branch") {
    const trunkCoord = data?.trunkCoord ?? (sourceX + targetX) / 2;
    const elbowX = clampedElbowX(sourceX, targetX, trunkCoord, 42);
    const path = makeRoundedOrthogonal([
      { x: sourceX, y: sourceY },
      { x: elbowX, y: sourceY },
      { x: elbowX, y: targetY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: elbowX, labelY: (sourceY + targetY) / 2 };
  }

  const midX = (sourceX + targetX) / 2 + laneOffset;
  const path = makeRoundedOrthogonal([
    { x: sourceX, y: sourceY },
    { x: midX, y: sourceY },
    { x: midX, y: targetY },
    { x: targetX, y: targetY },
  ]);
  return { path, labelX: midX, labelY: (sourceY + targetY) / 2 };
}

function SemanticEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  markerEnd,
  style,
  data,
}: EdgeProps<Edge<SemanticEdgeData>>) {
  const { path, labelX, labelY } = buildEdgePath(id, sourceX, sourceY, targetX, targetY, data);
  const bundleCount = data?.bundleCount ?? 1;

  return (
    <>
      <BaseEdge id={id} path={path} markerEnd={markerEnd} style={style} />
      {bundleCount > 1 ? (
        <EdgeLabelRenderer>
          <div
            className="graph-bundle-count"
            style={{
              transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
            }}
          >
            {bundleCount}
          </div>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
}

function GraphCardNode({ data, selected }: NodeProps<Node<FlowNodeData>>) {
  const [manuallyExpanded, setManuallyExpanded] = useState(false);
  const isSelected = selected || data.isSelected === true;

  if (data.nodeStyle === "bundle") {
    return (
      <div className="graph-bundle-node" aria-hidden>
        <Handle
          id="target-node-left"
          className="graph-handle graph-bundle-handle"
          type="target"
          position={Position.Left}
        />
        <Handle
          id="source-node-right"
          className="graph-handle graph-bundle-handle"
          type="source"
          position={Position.Right}
        />
        <Handle
          id="source-node-top"
          className="graph-handle graph-bundle-handle"
          type="source"
          position={Position.Top}
        />
        <Handle
          id="target-node-bottom"
          className="graph-handle graph-bundle-handle"
          type="target"
          position={Position.Bottom}
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
          position={Position.Left}
        />
        <Handle
          id="source-node"
          className="graph-handle graph-handle-source"
          type="source"
          position={Position.Right}
        />
        <Handle
          id="source-node-top"
          className="graph-handle graph-handle-top"
          type="source"
          position={Position.Top}
        />
        <Handle
          id="target-node-bottom"
          className="graph-handle graph-handle-bottom"
          type="target"
          position={Position.Bottom}
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
  const showAllMembers = data.center || isSelected || manuallyExpanded || !canToggleMembers;
  const visibleMembers = showAllMembers
    ? data.members
    : data.members.slice(0, MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMemberHandleMembers = showAllMembers
    ? []
    : data.members.slice(MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMembers = data.members.length - visibleMembers.length;
  const publicMembers = visibleMembers.filter((member) => member.visibility === "public");
  const privateMembers = visibleMembers.filter((member) => member.visibility === "private");
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
    visibility: "public" | "private",
    sectionLabel: string,
    members: FlowNodeData["members"],
  ): ReactNode => {
    if (members.length === 0) {
      return null;
    }

    return (
      <div className="graph-node-section">
        <div className="graph-node-section-header">
          <span className="graph-section-dot">{visibility === "public" ? "🌐" : "🏠"}</span>
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
                  position={Position.Left}
                />
                <span className="graph-member-name" title={member.label}>
                  {memberLabel}
                </span>
                <Handle
                  id={`source-member-${member.id}`}
                  className="graph-handle graph-member-handle graph-member-handle-source"
                  type="source"
                  position={Position.Right}
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
        position={Position.Left}
      />
      <Handle
        id="source-node"
        className="graph-handle graph-handle-source"
        type="source"
        position={Position.Right}
      />
      <Handle
        id="source-node-top"
        className="graph-handle graph-handle-top"
        type="source"
        position={Position.Top}
      />
      <Handle
        id="target-node-bottom"
        className="graph-handle graph-handle-bottom"
        type="target"
        position={Position.Bottom}
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
                position={Position.Left}
              />
              <Handle
                id={`source-member-${member.id}`}
                className="graph-handle graph-member-handle graph-member-handle-source graph-hidden-member-handle"
                type="source"
                position={Position.Right}
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
            disabled={isSelected}
            aria-label={
              showAllMembers ? "Collapse members" : `Expand members (${hiddenMembers} hidden)`
            }
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              if (isSelected) {
                return;
              }
              setManuallyExpanded((prev) => !prev);
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
        {renderSection("private", "PRIVATE", privateMembers)}
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
                  position={Position.Left}
                />
                <span className="graph-member-name">{data.label}</span>
                <Handle
                  id="source-node-chip"
                  className="graph-handle graph-member-handle graph-member-handle-source"
                  type="source"
                  position={Position.Right}
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
    <div
      className={[
        "graph-group-node",
        data.groupMode === "file" ? "graph-group-node-file" : "graph-group-node-namespace",
      ]
        .filter(Boolean)
        .join(" ")}
      aria-hidden
    >
      <div className="graph-group-label">{data.groupLabel}</div>
    </div>
  );
}

const GRAPH_NODE_TYPES = {
  sourcetrail: GraphCardNode,
  sourcetrailGroup: GraphGroupNode,
};

const GRAPH_EDGE_TYPES = {
  semantic: SemanticEdge,
};

function MermaidGraph({ syntax }: { syntax: string }) {
  const [svg, setSvg] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const renderId = `mermaid-${Math.random().toString(36).slice(2)}`;

    mermaid
      .render(renderId, syntax)
      .then(({ svg: renderedSvg }) => {
        if (!disposed) {
          setSvg(renderedSvg);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!disposed) {
          setError(err instanceof Error ? err.message : "Failed to render Mermaid diagram.");
          setSvg("");
        }
      });

    return () => {
      disposed = true;
    };
  }, [syntax]);

  if (error) {
    return <div className="graph-empty">{error}</div>;
  }
  if (svg.length === 0) {
    return <div className="graph-empty">Rendering diagram...</div>;
  }
  return <div className="mermaid-shell" dangerouslySetInnerHTML={{ __html: svg }} />;
}

type GraphViewportProps = {
  graph: GraphArtifactDto | null;
  onSelectNode: (nodeId: string, label: string) => void;
  onSelectEdge?: (selection: GraphEdgeSelection) => void;
  trailConfig: TrailUiConfig;
  onToggleLegend?: () => void;
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

function isDenseGraph(depth: number, nodeCount: number, edgeCount: number): boolean {
  if (nodeCount <= 48) {
    return false;
  }
  if (depth >= 4) {
    return nodeCount > 90 || edgeCount > 180;
  }
  if (depth >= 3) {
    return nodeCount > 120 || edgeCount > 240;
  }
  return nodeCount > 180 || edgeCount > 360;
}

export function GraphViewport({
  graph,
  onSelectNode,
  onSelectEdge,
  trailConfig,
  onToggleLegend,
}: GraphViewportProps) {
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [hoveredEdgeId, setHoveredEdgeId] = useState<string | null>(null);
  const [legendFilterKinds, setLegendFilterKinds] = useState<Set<EdgeKind> | null>(null);
  const [manualNodePositionsByGraph, setManualNodePositionsByGraph] = useState<
    Record<string, Record<string, { x: number; y: number }>>
  >({});
  const lastFittedGraphId = useRef<string | null>(null);
  const activeGraphNodePositions = useMemo(() => {
    if (!graph || isMermaidGraph(graph)) {
      return {};
    }
    return manualNodePositionsByGraph[graph.id] ?? {};
  }, [graph, manualNodePositionsByGraph]);

  const flowElements = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return null;
    }

    const nodeMetaById = new Map(
      graph.graph.nodes.map((node) => [
        node.id,
        {
          filePath: node.file_path ?? null,
          qualifiedName: node.qualified_name ?? null,
        },
      ]),
    );
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
          isSelected: node.id === selectedNodeId,
          focusedMemberId,
          onSelectMember: (memberId: string, label: string) => {
            setSelectedNodeId(node.id);
            setSelectedEdgeId(null);
            onSelectNode(memberId, label);
          },
        },
      };
    };

    try {
      const semantic = buildSemanticLayout(graph.graph);
      const bundled = applyAdaptiveBundling(
        semantic,
        trailConfig.depth,
        graph.graph.nodes.length,
        graph.graph.edges.length,
      );
      const routed = routeEdgesWithObstacles(bundled);
      const flowLayout = toReactFlowElements(routed);
      const centerNodeId = flowLayout.centerNodeId;
      const denseFocusActive = isDenseGraph(
        trailConfig.depth,
        flowLayout.nodes.length,
        flowLayout.edges.length,
      );
      const withManualNodePosition = (node: Node<FlowNodeData>): Node<FlowNodeData> => {
        const manualPosition = activeGraphNodePositions[node.id];
        if (!manualPosition) {
          return node;
        }
        return {
          ...node,
          position: manualPosition,
        };
      };
      return {
        ...flowLayout,
        nodes: applyGrouping(
          flowLayout.nodes.map(withInteractiveNodeData).map(withManualNodePosition),
          trailConfig.groupingMode,
          nodeMetaById,
        ),
        edges: flowLayout.edges.map((edge) => {
          const touchesCenter = edge.source === centerNodeId || edge.target === centerNodeId;
          const isTrunk = edge.data?.routeKind === "flow-trunk";
          const hasSelectedEdge = selectedEdgeId !== null;
          const isSelectedEdge = selectedEdgeId === edge.id;
          const isFilteredOut =
            legendFilterKinds !== null &&
            edge.data?.edgeKind !== undefined &&
            !legendFilterKinds.has(edge.data.edgeKind);
          const baseOpacity = Number(edge.style?.opacity ?? 1);
          const baseStroke = Number(edge.style?.strokeWidth ?? 2);

          if (!hoveredEdgeId) {
            const deemphasized = denseFocusActive && !touchesCenter && !isTrunk;
            return {
              ...edge,
              style: {
                ...edge.style,
                opacity: isFilteredOut
                  ? 0.09
                  : hasSelectedEdge
                    ? isSelectedEdge
                      ? 1
                      : 0.2
                    : deemphasized
                      ? Math.max(0.12, baseOpacity * 0.42)
                      : Math.max(0.82, baseOpacity),
                strokeWidth: isFilteredOut
                  ? Math.max(1, baseStroke - 0.8)
                  : hasSelectedEdge
                    ? isSelectedEdge
                      ? baseStroke + 0.45
                      : Math.max(1, baseStroke - 0.55)
                    : deemphasized
                      ? Math.max(1, baseStroke - 0.3)
                      : baseStroke + 0.1,
              },
            };
          }

          const dimmed = edge.id !== hoveredEdgeId;
          return {
            ...edge,
            style: {
              ...edge.style,
              opacity: isFilteredOut ? 0.08 : dimmed ? 0.18 : 1,
              strokeWidth: isFilteredOut
                ? Math.max(1, Number(edge.style?.strokeWidth ?? 1) - 0.8)
                : dimmed
                  ? Math.max(1, Number(edge.style?.strokeWidth ?? 1) - 0.6)
                  : Number(edge.style?.strokeWidth ?? 2) + 0.35,
            },
          };
        }),
      };
    } catch {
      const fallback = routeEdgesWithObstacles(buildFallbackLayout(graph.graph));
      const elements = toReactFlowElements(fallback);
      const withManualNodePosition = (node: Node<FlowNodeData>): Node<FlowNodeData> => {
        const manualPosition = activeGraphNodePositions[node.id];
        if (!manualPosition) {
          return node;
        }
        return {
          ...node,
          position: manualPosition,
        };
      };
      return {
        ...elements,
        nodes: applyGrouping(
          elements.nodes.map(withInteractiveNodeData).map(withManualNodePosition),
          trailConfig.groupingMode,
          nodeMetaById,
        ),
      };
    }
  }, [
    activeGraphNodePositions,
    graph,
    hoveredEdgeId,
    legendFilterKinds,
    onSelectNode,
    selectedEdgeId,
    selectedNodeId,
    trailConfig.depth,
    trailConfig.groupingMode,
  ]);

  useEffect(() => {
    setSelectedNodeId(flowElements?.centerNodeId ?? null);
    setSelectedEdgeId(null);
    setHoveredEdgeId(null);
    setLegendFilterKinds(null);
  }, [graph?.id, flowElements?.centerNodeId]);

  useEffect(() => {
    if (!flow || !graph || isMermaidGraph(graph) || !flowElements) {
      return;
    }

    if (lastFittedGraphId.current === graph.id) {
      return;
    }

    const denseGraph = flowElements.nodes.length > 64;
    const fitPadding = denseGraph ? 0.12 : 0.06;
    const fitMaxZoom = denseGraph ? 1.2 : 1.45;
    const focusNodeIds = new Set<string>([flowElements.centerNodeId]);
    for (const edge of flowElements.edges) {
      if (edge.source === flowElements.centerNodeId) {
        focusNodeIds.add(edge.target);
      } else if (edge.target === flowElements.centerNodeId) {
        focusNodeIds.add(edge.source);
      }
    }
    const focusNodes = flowElements.nodes
      .filter((node) => focusNodeIds.has(node.id))
      .map((node) => ({ id: node.id }));
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
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLSelectElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }

      if (event.key === "0") {
        event.preventDefault();
        resetZoom();
        return;
      }

      if (event.key === "+" || event.key === "=") {
        event.preventDefault();
        zoomIn();
        return;
      }

      if (event.key === "-" || event.key === "_") {
        event.preventDefault();
        zoomOut();
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [flow]);

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
    return <MermaidGraph syntax={graph.mermaid_syntax} />;
  }
  if (graph.graph.nodes.length === 0 || flowElements === null) {
    return <div className="graph-empty">No UML nodes were returned for this symbol yet.</div>;
  }

  const legendRows = buildLegendRows(graph.graph);
  const hasUncertainEdges = legendRows.some((row) => row.hasUncertain);
  const hasProbableEdges = legendRows.some((row) => row.hasProbable);
  const denseGraph = flowElements.nodes.length > 64;

  return (
    <ReactFlow
      key={graph.id}
      onInit={setFlow}
      nodes={flowElements.nodes}
      edges={flowElements.edges}
      onNodeClick={(_, node) => {
        const data = node.data as FlowNodeData | undefined;
        if (data?.isVirtualBundle) {
          return;
        }
        setSelectedNodeId(node.id);
        setSelectedEdgeId(null);
        onSelectNode(node.id, nodeLabelFromData(node.data, node.id));
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
      }}
      onEdgeClick={(_, edge) => {
        setSelectedEdgeId(edge.id);
        const sourceNode = flowElements.nodes.find((node) => node.id === edge.source);
        const targetNode = flowElements.nodes.find((node) => node.id === edge.target);
        const semanticEdgeData = edge.data as SemanticEdgeData | undefined;

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

        const centerNodeId = flowElements.centerNodeId;
        const preferredNodeId =
          edge.source === centerNodeId
            ? edge.target
            : edge.target === centerNodeId
              ? edge.source
              : edge.target;
        const fallbackNode =
          flowElements.nodes.find((node) => node.id === preferredNodeId) ??
          flowElements.nodes.find((node) => node.id === edge.target) ??
          flowElements.nodes.find((node) => node.id === edge.source);
        if (!fallbackNode) {
          return;
        }
        setSelectedNodeId(fallbackNode.id);
        onSelectNode(fallbackNode.id, nodeLabelFromData(fallbackNode.data, fallbackNode.id));
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
            Trunk lines indicate bundled flow edges.
            {(hasUncertainEdges || hasProbableEdges) && " "}
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
  );
}
