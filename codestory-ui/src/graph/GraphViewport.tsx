import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";
import mermaid from "mermaid";

import type { GraphArtifactDto, GraphResponse } from "../generated/api";

mermaid.initialize({
  startOnLoad: false,
  theme: "neutral",
  securityLevel: "loose",
});

type FlowNodeData = {
  kind: string;
  label: string;
  center: boolean;
  nodeStyle: "card" | "pill";
  memberCount: number;
  members: Array<{
    id: string;
    label: string;
    kind: string;
    visibility: "public" | "private";
  }>;
};

type EdgePalette = {
  stroke: string;
  width: number;
};

const MAX_ROWS_PER_COLUMN = 7;
const DEPTH_SPACING = 310;
const ROW_SPACING = 132;
const COLUMN_WRAP_SPACING = 220;
const ROOT_TARGET_Y = 260;
const MAX_VISIBLE_MEMBERS_PER_NODE = 6;

const STRUCTURAL_KINDS = new Set([
  "CLASS",
  "STRUCT",
  "INTERFACE",
  "UNION",
  "ENUM",
  "NAMESPACE",
  "MODULE",
  "PACKAGE",
]);

const CARD_NODE_KINDS = new Set([...STRUCTURAL_KINDS, "FILE"]);

const PRIVATE_MEMBER_KINDS = new Set([
  "FIELD",
  "VARIABLE",
  "GLOBAL_VARIABLE",
  "CONSTANT",
  "ENUM_CONSTANT",
]);

const PUBLIC_MEMBER_KINDS = new Set(["FUNCTION", "METHOD", "MACRO"]);

const EDGE_STYLE: Record<string, EdgePalette> = {
  CALL: { stroke: "#f0b429", width: 2.8 },
  USAGE: { stroke: "#4d9ac9", width: 2.4 },
  TYPE_USAGE: { stroke: "#878f98", width: 2.1 },
  MEMBER: { stroke: "#b8b8b8", width: 2.1 },
  INHERITANCE: { stroke: "#9d7aca", width: 2.2 },
  IMPORT: { stroke: "#a4b88c", width: 2.1 },
  INCLUDE: { stroke: "#a4b88c", width: 2.1 },
  MACRO_USAGE: { stroke: "#c88758", width: 2.1 },
};

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

function isCardNodeKind(kind: string): boolean {
  return CARD_NODE_KINDS.has(kind);
}

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

function GraphCardNode({ data, selected }: NodeProps<Node<FlowNodeData>>) {
  if (data.nodeStyle === "pill") {
    const pillClassName = [
      "graph-floating-pill",
      data.center ? "graph-floating-pill-center" : "",
      selected ? "graph-floating-pill-selected" : "",
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
        <span>{data.label}</span>
        <Handle
          id="source-node"
          className="graph-handle graph-handle-source"
          type="source"
          position={Position.Right}
        />
      </div>
    );
  }

  const visibleMembers = data.members.slice(0, MAX_VISIBLE_MEMBERS_PER_NODE);
  const hiddenMembers = data.members.length - visibleMembers.length;
  const publicMembers = visibleMembers.filter((member) => member.visibility === "public");
  const privateMembers = visibleMembers.filter((member) => member.visibility === "private");
  const className = [
    "graph-node",
    selected ? "graph-node-selected" : "",
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
            <div
              key={member.id}
              className={`graph-member-chip graph-member-chip-${visibility}`}
              title={formatKindLabel(member.kind)}
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
            </div>
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
      {data.kind === "FILE" ? <div className="graph-node-file-tab">{data.label}</div> : null}
      <div className="graph-node-title-row">
        <div className="graph-node-title">{data.label}</div>
        <div className="graph-node-count">{Math.max(1, data.memberCount)}</div>
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
      <Handle
        id="source-node"
        className="graph-handle graph-handle-source"
        type="source"
        position={Position.Right}
      />
    </div>
  );
}

const GRAPH_NODE_TYPES = {
  sourcetrail: GraphCardNode,
};

function toFlowElements(graph: GraphResponse): { nodes: Node<FlowNodeData>[]; edges: Edge[] } {
  const nodeById = new Map(graph.nodes.map((node) => [node.id, node]));
  const memberHostById = new Map<string, string>();
  const membersByHost = new Map<string, FlowNodeData["members"]>();

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

  const byDepth = new Map<number, typeof graph.nodes>();
  for (const node of graph.nodes) {
    if (memberHostById.has(node.id)) {
      continue;
    }
    const depthList = byDepth.get(node.depth) ?? [];
    depthList.push(node);
    byDepth.set(node.depth, depthList);
  }

  const sortedDepths = [...byDepth.keys()].sort((a, b) => a - b);
  const nodes: Node<FlowNodeData>[] = [];

  sortedDepths.forEach((depth, depthIndex) => {
    const depthNodes = [...(byDepth.get(depth) ?? [])].sort((a, b) =>
      a.label.localeCompare(b.label),
    );

    depthNodes.forEach((node, idx) => {
      const wrappedColumn = Math.floor(idx / MAX_ROWS_PER_COLUMN);
      const wrappedRow = idx % MAX_ROWS_PER_COLUMN;

      nodes.push({
        id: node.id,
        type: "sourcetrail",
        position: {
          x: 110 + depthIndex * DEPTH_SPACING + wrappedColumn * COLUMN_WRAP_SPACING,
          y: 90 + wrappedRow * ROW_SPACING,
        },
        sourcePosition: Position.Right,
        targetPosition: Position.Left,
        data: {
          kind: node.kind,
          label: node.label,
          center: node.id === graph.center_id,
          nodeStyle: isCardNodeKind(node.kind) ? "card" : "pill",
          memberCount: membersByHost.get(node.id)?.length ?? 0,
          members: [...(membersByHost.get(node.id) ?? [])].sort((a, b) =>
            a.label.localeCompare(b.label),
          ),
        },
      });
    });
  });

  const center = nodes.find((node) => node.id === graph.center_id);
  if (center) {
    const deltaY = ROOT_TARGET_Y - center.position.y;
    for (const node of nodes) {
      node.position = {
        x: node.position.x,
        y: node.position.y + deltaY,
      };
    }
  }

  const foldedEdges = new Map<
    string,
    (typeof graph.edges)[number] & { sourceHandle: string; targetHandle: string }
  >();
  for (const edge of graph.edges) {
    if (edge.kind === "MEMBER") {
      continue;
    }

    const sourceHost = memberHostById.get(edge.source);
    const targetHost = memberHostById.get(edge.target);
    const source = sourceHost ?? edge.source;
    const target = targetHost ?? edge.target;
    const sourceHandle = sourceHost ? `source-member-${edge.source}` : "source-node";
    const targetHandle = targetHost ? `target-member-${edge.target}` : "target-node";

    if (source === target && sourceHandle === targetHandle) {
      continue;
    }

    const callsiteKey =
      edge.kind === "CALL" ? edge.callsite_identity ?? edge.id : "";
    const key =
      edge.kind === "CALL"
        ? `${edge.kind}:${source}:${sourceHandle}:${target}:${targetHandle}:${callsiteKey}`
        : `${edge.kind}:${source}:${sourceHandle}:${target}:${targetHandle}`;
    if (!foldedEdges.has(key)) {
      foldedEdges.set(key, {
        ...edge,
        source,
        target,
        id: key,
        sourceHandle,
        targetHandle,
      });
    }
  }

  const edges: Edge[] = [...foldedEdges.values()].map((edge) => {
    const palette = EDGE_STYLE[edge.kind] ?? { stroke: "#8b8f96", width: 2.1 };
    const certainty = edge.certainty?.toLowerCase();
    const isUncertain = certainty === "uncertain";
    const isProbable = certainty === "probable";

    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
      type: "smoothstep",
      animated: false,
      pathOptions: {
        borderRadius: 18,
        offset: 20,
      },
      markerEnd: {
        type: MarkerType.ArrowClosed,
        width: 13,
        height: 13,
        color: palette.stroke,
      },
      style: {
        stroke: palette.stroke,
        strokeWidth: palette.width,
        strokeLinecap: "round",
        strokeDasharray: isUncertain ? "7 5" : undefined,
        opacity: isUncertain ? 0.72 : isProbable ? 0.88 : 1,
      },
      interactionWidth: 18,
    };
  });

  return { nodes, edges };
}

function MermaidGraph({ syntax }: { syntax: string }) {
  const [svg, setSvg] = useState<string>("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const renderId = `mermaid-${Math.random().toString(36).slice(2)}`;

    mermaid
      .render(renderId, syntax)
      .then(({ svg }) => {
        if (!disposed) {
          setSvg(svg);
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
  onSelectNode: (nodeId: string) => void;
};

export function GraphViewport({ graph, onSelectNode }: GraphViewportProps) {
  const [flow, setFlow] = useState<ReactFlowInstance | null>(null);
  const lastFittedGraphId = useRef<string | null>(null);

  const flowElements = useMemo(() => {
    if (graph === null || isMermaidGraph(graph)) {
      return null;
    }

    return toFlowElements(graph.graph);
  }, [graph]);

  useEffect(() => {
    if (!flow || !graph || isMermaidGraph(graph) || !flowElements) {
      return;
    }

    if (lastFittedGraphId.current === graph.id) {
      return;
    }

    const center = flow.getNode(graph.graph.center_id);
    window.requestAnimationFrame(() => {
      if (center) {
        void flow.fitView({
          nodes: [center],
          duration: 260,
          maxZoom: 1.02,
          minZoom: 0.35,
          padding: 2.2,
        });
      } else {
        void flow.fitView({
          duration: 260,
          maxZoom: 1.02,
          minZoom: 0.35,
          padding: 0.32,
        });
      }
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

  return (
    <ReactFlow
      onInit={setFlow}
      nodes={flowElements.nodes}
      edges={flowElements.edges}
      onNodeClick={(_, node) => onSelectNode(node.id)}
      nodeTypes={GRAPH_NODE_TYPES}
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
    </ReactFlow>
  );
}
