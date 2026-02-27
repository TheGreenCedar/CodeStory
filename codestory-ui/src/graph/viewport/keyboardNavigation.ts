import type { Edge, Node } from "@xyflow/react";

import type { SemanticEdgeData } from "../layout/routing";
import type { FlowNodeData } from "../layout/types";

export type NavDirection = "left" | "right" | "up" | "down";

export function isEditableTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

export function directionFromKey(key: string): NavDirection | null {
  const normalized = key.toLowerCase();
  if (normalized === "arrowleft" || normalized === "h" || normalized === "a") {
    return "left";
  }
  if (normalized === "arrowright" || normalized === "l" || normalized === "d") {
    return "right";
  }
  if (normalized === "arrowup" || normalized === "k" || normalized === "w") {
    return "up";
  }
  if (normalized === "arrowdown" || normalized === "j" || normalized === "s") {
    return "down";
  }
  return null;
}

export function nodeCenter(node: Node<FlowNodeData>): { x: number; y: number } {
  const width = typeof node.width === "number" ? node.width : 140;
  const height = typeof node.height === "number" ? node.height : 40;
  return {
    x: node.position.x + width / 2,
    y: node.position.y + height / 2,
  };
}

function edgeMidpoint(
  edge: Edge<SemanticEdgeData>,
  nodeById: Map<string, Node<FlowNodeData>>,
): { x: number; y: number } {
  const route = edge.data?.routePoints ?? [];
  if (route.length > 0) {
    const midpoint = route[Math.floor(route.length / 2)];
    if (midpoint) {
      return { x: midpoint.x, y: midpoint.y };
    }
  }
  const source = nodeById.get(edge.source);
  const target = nodeById.get(edge.target);
  if (source && target) {
    const sourceCenter = nodeCenter(source);
    const targetCenter = nodeCenter(target);
    return {
      x: (sourceCenter.x + targetCenter.x) / 2,
      y: (sourceCenter.y + targetCenter.y) / 2,
    };
  }
  return { x: 0, y: 0 };
}

function directionalScore(
  direction: NavDirection,
  from: { x: number; y: number },
  to: { x: number; y: number },
): number | null {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (direction === "left" && dx >= -0.5) {
    return null;
  }
  if (direction === "right" && dx <= 0.5) {
    return null;
  }
  if (direction === "up" && dy >= -0.5) {
    return null;
  }
  if (direction === "down" && dy <= 0.5) {
    return null;
  }

  const primary = direction === "left" || direction === "right" ? Math.abs(dx) : Math.abs(dy);
  const orthogonal = direction === "left" || direction === "right" ? Math.abs(dy) : Math.abs(dx);
  return primary + orthogonal * 0.6;
}

export function findNextNodeSelection(
  direction: NavDirection,
  graphNodes: Node<FlowNodeData>[],
  currentNodeId: string | null,
  nodeById: Map<string, Node<FlowNodeData>>,
): Node<FlowNodeData> | null {
  if (graphNodes.length === 0) {
    return null;
  }

  const current = currentNodeId ? nodeById.get(currentNodeId) : null;
  const fromPoint = current ? nodeCenter(current) : nodeCenter(graphNodes[0]!);
  let best: Node<FlowNodeData> | null = null;
  let bestScore = Number.POSITIVE_INFINITY;
  for (const candidate of graphNodes) {
    if (candidate.id === current?.id) {
      continue;
    }
    const score = directionalScore(direction, fromPoint, nodeCenter(candidate));
    if (score === null || score >= bestScore) {
      continue;
    }
    best = candidate;
    bestScore = score;
  }
  return best;
}

export function findNextEdgeSelection(
  direction: NavDirection,
  edges: Edge<SemanticEdgeData>[],
  selectedEdgeId: string | null,
  selectedNodeId: string | null,
  nodeById: Map<string, Node<FlowNodeData>>,
  edgeById: Map<string, Edge<SemanticEdgeData>>,
): Edge<SemanticEdgeData> | null {
  if (edges.length === 0) {
    return null;
  }
  const currentEdge = selectedEdgeId ? edgeById.get(selectedEdgeId) : null;
  const selectedNode = selectedNodeId ? (nodeById.get(selectedNodeId) ?? null) : null;
  const firstEdge = edges[0] ?? null;
  const firstNode =
    firstEdge === null
      ? null
      : (nodeById.get(firstEdge.source) ?? nodeById.get(firstEdge.target) ?? null);
  const fromPoint = currentEdge
    ? edgeMidpoint(currentEdge, nodeById)
    : selectedNode
      ? nodeCenter(selectedNode)
      : firstNode
        ? nodeCenter(firstNode)
        : { x: 0, y: 0 };

  let best: Edge<SemanticEdgeData> | null = null;
  let bestScore = Number.POSITIVE_INFINITY;
  for (const candidate of edges) {
    if (candidate.id === currentEdge?.id) {
      continue;
    }
    const score = directionalScore(direction, fromPoint, edgeMidpoint(candidate, nodeById));
    if (score === null || score >= bestScore) {
      continue;
    }
    best = candidate;
    bestScore = score;
  }
  return best;
}
