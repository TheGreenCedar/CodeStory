import type { Edge, Node } from "@xyflow/react";

import type { SemanticEdgeData } from "../layout/routing";
import type { FlowNodeData } from "../layout/types";

type FlowElementsState = {
  centerNodeId: string;
  nodes: Node<FlowNodeData>[];
  edges: Edge<SemanticEdgeData>[];
};

export function fitViewSettings(nodeCount: number): {
  denseGraph: boolean;
  fitPadding: number;
  fitMaxZoom: number;
} {
  const denseGraph = nodeCount > 64;
  return {
    denseGraph,
    fitPadding: denseGraph ? 0.12 : 0.06,
    fitMaxZoom: denseGraph ? 1.2 : 1.45,
  };
}

export function buildFitViewTargets(flowElements: FlowElementsState): Array<{ id: string }> {
  const focusNodeIds = new Set<string>([flowElements.centerNodeId]);
  const virtualBundleNodeIds = new Set(
    flowElements.nodes.filter((node) => node.data.isVirtualBundle).map((node) => node.id),
  );
  const adjacentBundleIds = new Set<string>();
  for (const edge of flowElements.edges) {
    if (edge.source === flowElements.centerNodeId) {
      focusNodeIds.add(edge.target);
      if (virtualBundleNodeIds.has(edge.target)) {
        adjacentBundleIds.add(edge.target);
      }
    } else if (edge.target === flowElements.centerNodeId) {
      focusNodeIds.add(edge.source);
      if (virtualBundleNodeIds.has(edge.source)) {
        adjacentBundleIds.add(edge.source);
      }
    }
  }
  if (adjacentBundleIds.size > 0) {
    const bundleQueue = [...adjacentBundleIds];
    const visitedBundles = new Set(bundleQueue);
    while (bundleQueue.length > 0) {
      const bundleId = bundleQueue.shift();
      if (!bundleId) {
        continue;
      }
      for (const edge of flowElements.edges) {
        let neighborId: string | null = null;
        if (edge.source === bundleId) {
          neighborId = edge.target;
        } else if (edge.target === bundleId) {
          neighborId = edge.source;
        }
        if (!neighborId) {
          continue;
        }
        if (virtualBundleNodeIds.has(neighborId)) {
          if (!visitedBundles.has(neighborId)) {
            visitedBundles.add(neighborId);
            bundleQueue.push(neighborId);
          }
          continue;
        }
        focusNodeIds.add(neighborId);
      }
    }
  }
  return flowElements.nodes
    .filter((node) => focusNodeIds.has(node.id))
    .map((node) => ({ id: node.id }));
}
