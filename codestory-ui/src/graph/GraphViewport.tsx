import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BaseEdge,
  Controls,
  Handle,
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
  const trunkCoord = data?.trunkCoord ?? (sourceX + targetX) / 2;

  if (routeKind === "flow-trunk") {
    const elbowX = clampedElbowX(sourceX, targetX, trunkCoord, 0);
    const path = orthogonalPath([
      { x: sourceX, y: sourceY },
      { x: elbowX, y: sourceY },
      { x: elbowX, y: targetY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: elbowX, labelY: (sourceY + targetY) / 2 };
  }

  if (routeKind === "flow-branch") {
    const elbowX = clampedElbowX(sourceX, targetX, trunkCoord, 0);
    const path = orthogonalPath([
      { x: sourceX, y: sourceY },
      { x: elbowX, y: sourceY },
      { x: elbowX, y: targetY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: (sourceX + targetX) / 2, labelY: (sourceY + targetY) / 2 };
  }

  if (routeKind === "hierarchy") {
    const liftY = (sourceY + targetY) / 2;
    const path = orthogonalPath([
      { x: sourceX, y: sourceY },
      { x: sourceX, y: liftY },
      { x: targetX, y: liftY },
      { x: targetX, y: targetY },
    ]);
    return { path, labelX: (sourceX + targetX) / 2, labelY: liftY };
  }

  const direction = targetX >= sourceX ? 1 : -1;
  const spanX = Math.abs(targetX - sourceX);
  const curveX = Math.min(148, Math.max(44, spanX * 0.32));
  const laneOffset = laneOffsetFromEdgeId(edgeId, 10, 4);
  const sourceCtrlX = sourceX + direction * curveX;
  const targetCtrlX = targetX - direction * curveX;
  const sourceCtrlY = sourceY + laneOffset;
  const targetCtrlY = targetY - laneOffset;
  const path = `M ${sourceX} ${sourceY} C ${sourceCtrlX} ${sourceCtrlY}, ${targetCtrlX} ${targetCtrlY}, ${targetX} ${targetY}`;
  return { path, labelX: (sourceX + targetX) / 2, labelY: (sourceY + targetY) / 2 };
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
    const pillClassName = [
      "graph-floating-pill",
      data.center ? "graph-floating-pill-center" : "",
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
          <span className="graph-pill-duplicate-count">x{data.duplicateCount}</span>
        ) : null}
      </div>
    );
  }

  const canToggleMembers = data.members.length > MAX_VISIBLE_MEMBERS_PER_NODE;
  const showAllMembers = isSelected || manuallyExpanded || !canToggleMembers;
  const visibleMembers = showAllMembers
    ? data.members
    : data.members.slice(0, MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMemberHandleMembers = showAllMembers
    ? []
    : data.members.slice(MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMembers = data.members.length - visibleMembers.length;
  const publicMembers = visibleMembers.filter((member) => member.visibility === "public");
  const privateMembers = visibleMembers.filter((member) => member.visibility === "private");
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
          <span className={`graph-section-dot graph-section-dot-${visibility}`} />
          <span>{sectionLabel}</span>
        </div>
        <div className="graph-node-members">
          {members.map((member) => (
            <button
              type="button"
              key={member.id}
              className={`graph-member-chip graph-member-chip-button graph-member-chip-${visibility}`}
              title={formatKindLabel(member.kind)}
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
              <span className="graph-member-name">{member.label}</span>
              <Handle
                id={`source-member-${member.id}`}
                className="graph-handle graph-member-handle graph-member-handle-source"
                type="source"
                position={Position.Right}
              />
            </button>
          ))}
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
        <div className="graph-node-title">{data.label}</div>
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
            <span className="graph-node-count">{Math.max(1, data.memberCount)}</span>
            <span className="graph-node-chevron">{showAllMembers ? "▾" : "▸"}</span>
          </button>
        ) : (
          <div className="graph-node-count">{Math.max(1, data.memberCount)}</div>
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
};

export function GraphViewport({ graph, onSelectNode }: GraphViewportProps) {
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const lastFittedGraphId = useRef<string | null>(null);

  const flowElements = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return null;
    }

    try {
      const semantic = buildSemanticLayout(graph.graph);
      const bundled = applySharedTrunkBundling(semantic);
      const elements = toReactFlowElements(bundled);
      return {
        ...elements,
        nodes: elements.nodes.map((node) => ({
          ...node,
          data: {
            ...node.data,
            isSelected: node.id === selectedNodeId,
            onSelectMember: (memberId: string, label: string) => {
              setSelectedNodeId(node.id);
              onSelectNode(memberId, label);
            },
          },
        })),
      };
    } catch {
      const elements = toReactFlowElements(buildFallbackLayout(graph.graph));
      return {
        ...elements,
        nodes: elements.nodes.map((node) => ({
          ...node,
          data: {
            ...node.data,
            isSelected: node.id === selectedNodeId,
            onSelectMember: (memberId: string, label: string) => {
              setSelectedNodeId(node.id);
              onSelectNode(memberId, label);
            },
          },
        })),
      };
    }
  }, [graph, onSelectNode, selectedNodeId]);

  useEffect(() => {
    setSelectedNodeId(null);
  }, [graph?.id]);

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
    window.requestAnimationFrame(() => {
      void flow.fitView({
        duration: 260,
        maxZoom: fitMaxZoom,
        minZoom: 0.28,
        padding: fitPadding,
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

  return (
    <ReactFlow
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
      nodeTypes={GRAPH_NODE_TYPES}
      edgeTypes={GRAPH_EDGE_TYPES}
      minZoom={0.18}
      maxZoom={2.1}
      proOptions={{ hideAttribution: true }}
      nodesDraggable={false}
      nodesConnectable={false}
      elementsSelectable
      fitView={false}
      className="sourcetrail-flow"
    >
      <Controls position="top-left" showInteractive={false} />
      {legendRows.length > 0 ? (
        <Panel position="top-right" className="graph-legend-panel">
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
