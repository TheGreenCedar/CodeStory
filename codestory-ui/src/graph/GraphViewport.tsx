import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BaseEdge,
  Controls,
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

import type { GraphArtifactDto } from "../generated/api";
import { applySharedTrunkBundling } from "./layout/bundling";
import { buildFallbackLayout, buildSemanticLayout } from "./layout/semanticLayout";
import { buildLegendRows, toReactFlowElements, type SemanticEdgeData } from "./layout/routing";
import type { TrailUiConfig } from "./trailConfig";
import type { FlowNodeData } from "./layout/types";

mermaid.initialize({
  startOnLoad: false,
  theme: "neutral",
  securityLevel: "loose",
});

const MAX_VISIBLE_MEMBERS_PER_NODE = 6;

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
  const routeKind = data?.routeKind ?? "direct";

  // Base 14px rounding radius for elbows
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
  const laneSpanY = Math.abs(targetY - sourceY);
  const laneOffset = laneSpanY > 20 ? laneOffsetFromEdgeId(edgeId, 8, 5) : 0;

  if (routeKind === "flow-trunk" || routeKind === "flow-branch") {
    // Disable laneOffset jitter for trunks so they perfectly merge into a single solid line
    const trunkCoord = data?.trunkCoord ?? (sourceX + targetX) / 2;
    // Gutter set to 42 to ensure: 14px for curve + ~16px for visible straight line + ~12px for arrowhead
    const elbowX = clampedElbowX(sourceX, targetX, trunkCoord, 42);

    const path = makeRoundedOrthogonal([
      { x: sourceX, y: sourceY },
      { x: elbowX, y: sourceY },
      { x: elbowX, y: targetY },
      { x: targetX, y: targetY },
    ]);

    return { path, labelX: elbowX, labelY: (sourceY + targetY) / 2 };
  }

  if (routeKind === "hierarchy") {
    const liftY = (sourceY + targetY) / 2 + laneOffset;
    const path = makeRoundedOrthogonal([
      { x: sourceX, y: sourceY },
      { x: sourceX, y: liftY },
      { x: targetX, y: liftY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: (sourceX + targetX) / 2, labelY: liftY };
  }

  // Default to a simple routed path instead of bezier to match the flowchart vibe
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
  const { path } = buildEdgePath(id, sourceX, sourceY, targetX, targetY, data);

  return <BaseEdge id={id} path={path} markerEnd={markerEnd} style={style} />;
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

const GRAPH_NODE_TYPES = {
  sourcetrail: GraphCardNode,
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
  trailConfig: TrailUiConfig;
};

type FlowElements = {
  nodes: Node<FlowNodeData>[];
  edges: Edge<SemanticEdgeData>[];
  centerNodeId: string;
};

function isLowSignalTraceLabel(label: string): boolean {
  const normalized = label.toLowerCase();
  return (
    normalized.includes("test") ||
    normalized.includes("bench") ||
    normalized.startsWith("run_") ||
    normalized.startsWith("index_")
  );
}

function traceNodeBudget(depth: number): number {
  if (depth <= 1) {
    return 16;
  }
  if (depth === 2) {
    return 20;
  }
  return 28;
}

function pruneTraceElements(elements: FlowElements, depth: number): FlowElements {
  if (elements.nodes.length <= 1) {
    return elements;
  }

  const bundleIds = new Set(
    elements.nodes.filter((node) => node.data.isVirtualBundle).map((node) => node.id),
  );
  const neighbors = new Map<string, Set<string>>();
  const weightedDegree = new Map<string, number>();
  for (const edge of elements.edges) {
    if (!neighbors.has(edge.source)) {
      neighbors.set(edge.source, new Set());
    }
    if (!neighbors.has(edge.target)) {
      neighbors.set(edge.target, new Set());
    }
    neighbors.get(edge.source)?.add(edge.target);
    neighbors.get(edge.target)?.add(edge.source);
    const weight = Math.max(1, Number(edge.data?.bundleCount ?? 1));
    weightedDegree.set(edge.source, (weightedDegree.get(edge.source) ?? 0) + weight);
    weightedDegree.set(edge.target, (weightedDegree.get(edge.target) ?? 0) + weight);
  }

  const dist = new Map<string, number>();
  const queue: Array<{ id: string; cost: number }> = [{ id: elements.centerNodeId, cost: 0 }];
  dist.set(elements.centerNodeId, 0);

  while (queue.length > 0) {
    queue.sort((left, right) => left.cost - right.cost);
    const current = queue.shift();
    if (!current) {
      continue;
    }
    const knownCost = dist.get(current.id);
    if (typeof knownCost === "number" && current.cost > knownCost) {
      continue;
    }
    for (const next of neighbors.get(current.id) ?? []) {
      const step = bundleIds.has(next) ? 0 : 1;
      const nextCost = current.cost + step;
      const prev = dist.get(next);
      if (typeof prev === "number" && prev <= nextCost) {
        continue;
      }
      dist.set(next, nextCost);
      queue.push({ id: next, cost: nextCost });
    }
  }

  const candidateNodes = elements.nodes
    .filter((node) => node.id !== elements.centerNodeId && !bundleIds.has(node.id))
    .map((node) => ({
      node,
      dist: dist.get(node.id) ?? Number.POSITIVE_INFINITY,
      score: weightedDegree.get(node.id) ?? 0,
      isCard: node.data.nodeStyle === "card",
      lowSignal: isLowSignalTraceLabel(node.data.label),
    }))
    .filter((entry) => Number.isFinite(entry.dist))
    .sort((left, right) => {
      if (left.dist !== right.dist) {
        return left.dist - right.dist;
      }
      if (left.isCard !== right.isCard) {
        return left.isCard ? -1 : 1;
      }
      if (left.lowSignal !== right.lowSignal) {
        return left.lowSignal ? 1 : -1;
      }
      if (left.score !== right.score) {
        return right.score - left.score;
      }
      return left.node.data.label.localeCompare(right.node.data.label);
    });

  const budget = traceNodeBudget(depth);
  const keptNodeIds = new Set<string>([elements.centerNodeId]);
  for (const entry of candidateNodes) {
    if (keptNodeIds.size - 1 >= budget) {
      break;
    }
    keptNodeIds.add(entry.node.id);
  }

  // Keep only bundle nodes that bridge at least two kept non-bundle nodes.
  const bundleSupport = new Map<string, Set<string>>();
  for (const edge of elements.edges) {
    const sourceBundle = bundleIds.has(edge.source);
    const targetBundle = bundleIds.has(edge.target);
    if (!sourceBundle && !targetBundle) {
      continue;
    }
    if (sourceBundle && keptNodeIds.has(edge.target)) {
      const support = bundleSupport.get(edge.source) ?? new Set<string>();
      support.add(edge.target);
      bundleSupport.set(edge.source, support);
    }
    if (targetBundle && keptNodeIds.has(edge.source)) {
      const support = bundleSupport.get(edge.target) ?? new Set<string>();
      support.add(edge.source);
      bundleSupport.set(edge.target, support);
    }
  }
  for (const [bundleId, support] of bundleSupport) {
    if (support.size >= 2 || support.has(elements.centerNodeId)) {
      keptNodeIds.add(bundleId);
    }
  }

  const nodeById = new Map(elements.nodes.map((node) => [node.id, node]));
  const keptEdges = elements.edges.filter((edge) => {
    if (!keptNodeIds.has(edge.source) || !keptNodeIds.has(edge.target)) {
      return false;
    }

    if (depth < 2) {
      return true;
    }

    const source = nodeById.get(edge.source);
    const target = nodeById.get(edge.target);
    const sourceCard = source?.data.nodeStyle === "card";
    const targetCard = target?.data.nodeStyle === "card";
    const touchesCenter =
      edge.source === elements.centerNodeId || edge.target === elements.centerNodeId;
    const isTrunk = edge.data?.routeKind === "flow-trunk";

    return touchesCenter || sourceCard || targetCard || isTrunk;
  });

  // Drop isolated virtual nodes after edge filtering.
  const usedNodeIds = new Set<string>([elements.centerNodeId]);
  for (const edge of keptEdges) {
    usedNodeIds.add(edge.source);
    usedNodeIds.add(edge.target);
  }

  const keptNodes = elements.nodes.filter((node) => usedNodeIds.has(node.id));
  if (keptNodes.length === elements.nodes.length && keptEdges.length === elements.edges.length) {
    return elements;
  }

  return {
    ...elements,
    nodes: keptNodes,
    edges: keptEdges,
  };
}

export function GraphViewport({ graph, onSelectNode, trailConfig }: GraphViewportProps) {
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [hoveredEdgeId, setHoveredEdgeId] = useState<string | null>(null);
  const lastFittedGraphId = useRef<string | null>(null);

  const flowElements = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
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
          isSelected: node.id === selectedNodeId,
          focusedMemberId,
          onSelectMember: (memberId: string, label: string) => {
            setSelectedNodeId(node.id);
            onSelectNode(memberId, label);
          },
        },
      };
    };

    try {
      const semantic = buildSemanticLayout(graph.graph);
      // Skip bundling — route edges directly from source to target
      // Run the semantic edge-grouping pass which now applies flow-trunk coords without
      // generating synthetic layout nodes
      const routed = applySharedTrunkBundling(semantic, trailConfig.bundlingMode);
      const flowLayout = toReactFlowElements(routed);
      const traceActive = trailConfig.bundlingMode === "trace";
      const scopedElements =
        traceActive && flowLayout.nodes.length > 36
          ? pruneTraceElements(flowLayout, trailConfig.depth)
          : flowLayout;
      const centerNodeId = scopedElements.centerNodeId;
      return {
        ...scopedElements,
        nodes: scopedElements.nodes.map(withInteractiveNodeData),
        edges: scopedElements.edges.map((edge) => {
          if (!traceActive) {
            return edge;
          }

          const touchesCenter = edge.source === centerNodeId || edge.target === centerNodeId;
          const isTrunk = edge.data?.routeKind === "flow-trunk";
          const baseOpacity = Number(edge.style?.opacity ?? 1);
          const baseStroke = Number(edge.style?.strokeWidth ?? 2);

          if (!hoveredEdgeId) {
            const deemphasized = !touchesCenter && !isTrunk;
            return {
              ...edge,
              style: {
                ...edge.style,
                opacity: deemphasized
                  ? Math.max(0.12, baseOpacity * 0.42)
                  : Math.max(0.82, baseOpacity),
                strokeWidth: deemphasized ? Math.max(1, baseStroke - 0.3) : baseStroke + 0.1,
              },
            };
          }

          const dimmed = edge.id !== hoveredEdgeId;
          return {
            ...edge,
            style: {
              ...edge.style,
              opacity: dimmed ? 0.18 : 1,
              strokeWidth: dimmed
                ? Math.max(1, Number(edge.style?.strokeWidth ?? 1) - 0.6)
                : Number(edge.style?.strokeWidth ?? 2) + 0.35,
            },
          };
        }),
      };
    } catch {
      const elements = toReactFlowElements(buildFallbackLayout(graph.graph));
      return {
        ...elements,
        nodes: elements.nodes.map(withInteractiveNodeData),
      };
    }
  }, [graph, hoveredEdgeId, onSelectNode, selectedNodeId, trailConfig.bundlingMode]);

  useEffect(() => {
    setSelectedNodeId(flowElements?.centerNodeId ?? null);
    setHoveredEdgeId(null);
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
        onSelectNode(node.id, nodeLabelFromData(node.data, node.id));
      }}
      onPaneClick={() => {
        setSelectedNodeId(null);
      }}
      onEdgeMouseEnter={(_, edge) => {
        if (trailConfig.bundlingMode !== "trace") {
          return;
        }
        setHoveredEdgeId(edge.id);
      }}
      onEdgeMouseLeave={() => {
        if (trailConfig.bundlingMode !== "trace") {
          return;
        }
        setHoveredEdgeId(null);
      }}
      nodeTypes={GRAPH_NODE_TYPES}
      edgeTypes={GRAPH_EDGE_TYPES}
      minZoom={0.18}
      maxZoom={2.1}
      proOptions={{ hideAttribution: true }}
      nodesDraggable={false}
      nodesConnectable={false}
      elementsSelectable
      onlyRenderVisibleElements={denseGraph}
      fitView={false}
      className="sourcetrail-flow"
    >
      <Controls position="top-left" showInteractive={false} />
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
      {trailConfig.showLegend && legendRows.length > 0 ? (
        <Panel position="bottom-right" className="graph-legend-panel">
          <div className="graph-legend-title">Legend</div>
          <div className="graph-legend-rows">
            {legendRows.map((row) => (
              <div key={row.kind} className="graph-legend-row">
                <span className="graph-legend-line" style={{ background: row.stroke }} />
                <span className="graph-legend-kind">{formatKindLabel(row.kind)}</span>
                <span className="graph-legend-count">{row.count}</span>
              </div>
            ))}
          </div>
          <div className="graph-legend-note">
            Trunk lines indicate bundled flow edges.
            {(hasUncertainEdges || hasProbableEdges) && " "}
            {hasUncertainEdges ? "Dashed edges = uncertain." : ""}
            {hasUncertainEdges && hasProbableEdges ? " " : ""}
            {hasProbableEdges ? "Lower opacity = probable." : ""}
          </div>
        </Panel>
      ) : null}
    </ReactFlow>
  );
}
