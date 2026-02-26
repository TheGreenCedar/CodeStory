import type { LayoutDirection } from "../../generated/api";
import { applyAdaptiveBundling } from "./bundling";
import {
  routeEdgesWithObstacles,
  routeIntersectionDiagnostics,
  type RouteIntersectionDiagnostics,
} from "./obstacleRouting";
import type { LayoutElements } from "./types";

export type ParityPipelineDiagnostics = {
  bundledEdgeCount: number;
  channelCount: number;
  channels: Array<{
    channelId: string;
    edgeCount: number;
    weight: number;
  }>;
  routeDiagnostics: RouteIntersectionDiagnostics[];
};

export type RunParityPipelineParams = {
  layout: LayoutElements;
  depth: number;
  nodeCount: number;
  edgeCount: number;
  layoutDirection: LayoutDirection;
  debugChannels?: boolean;
  debugRoutes?: boolean;
  log?: (message: string, payload: unknown) => void;
};

export type RunParityPipelineResult = {
  bundled: LayoutElements;
  routed: LayoutElements;
  diagnostics: ParityPipelineDiagnostics;
};

function collectPipelineDiagnostics(routed: LayoutElements): ParityPipelineDiagnostics {
  const channelStats = new Map<string, { edgeCount: number; weight: number }>();
  for (const edge of routed.edges) {
    if (!edge.channelId) {
      continue;
    }
    const existing = channelStats.get(edge.channelId) ?? { edgeCount: 0, weight: 0 };
    existing.edgeCount += 1;
    existing.weight = Math.max(existing.weight, edge.channelWeight ?? edge.bundleCount ?? 1);
    channelStats.set(edge.channelId, existing);
  }

  const routeDiagnostics = routed.edges
    .map((edge) => routeIntersectionDiagnostics(edge, routed.nodes))
    .filter((report) => report.collisionCount > 0);

  return {
    bundledEdgeCount: routed.edges.filter((edge) => edge.routeKind === "flow-trunk").length,
    channelCount: channelStats.size,
    channels: [...channelStats.entries()]
      .map(([channelId, stats]) => ({ channelId, ...stats }))
      .sort((left, right) => left.channelId.localeCompare(right.channelId)),
    routeDiagnostics,
  };
}

export function runDeterministicParityPipeline({
  layout,
  depth,
  nodeCount,
  edgeCount,
  layoutDirection,
  debugChannels = false,
  debugRoutes = false,
  log = (message, payload) => {
    console.debug(message, payload);
  },
}: RunParityPipelineParams): RunParityPipelineResult {
  const bundled = applyAdaptiveBundling(layout, depth, nodeCount, edgeCount, layoutDirection);
  const routed = routeEdgesWithObstacles(bundled, layoutDirection);
  const diagnostics = collectPipelineDiagnostics(routed);

  if (debugChannels) {
    log("[parity] channel diagnostics", {
      bundledEdgeCount: diagnostics.bundledEdgeCount,
      channelCount: diagnostics.channelCount,
      channels: diagnostics.channels,
    });
  }
  if (debugRoutes) {
    log("[parity] route diagnostics", {
      collisionEdges: diagnostics.routeDiagnostics.length,
      routeDiagnostics: diagnostics.routeDiagnostics,
    });
  }

  return {
    bundled,
    routed,
    diagnostics,
  };
}
