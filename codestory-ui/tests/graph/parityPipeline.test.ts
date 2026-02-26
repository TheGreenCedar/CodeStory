import { describe, expect, it, vi } from "vitest";

import { runDeterministicParityPipeline } from "../../src/graph/layout/parityPipeline";
import type { LayoutElements } from "../../src/graph/layout/types";
import denseHorizontal from "./fixtures/dense-horizontal.json";

describe("runDeterministicParityPipeline", () => {
  it("runs bundling then routing deterministically", () => {
    const fixture = denseHorizontal as LayoutElements;
    const first = runDeterministicParityPipeline({
      layout: fixture,
      depth: 5,
      nodeCount: fixture.nodes.length,
      edgeCount: fixture.edges.length,
      layoutDirection: "Horizontal",
    });
    const second = runDeterministicParityPipeline({
      layout: fixture,
      depth: 5,
      nodeCount: fixture.nodes.length,
      edgeCount: fixture.edges.length,
      layoutDirection: "Horizontal",
    });

    const firstRoutes = first.routed.edges.map((edge) => ({
      id: edge.id,
      channelId: edge.channelId,
      trunkCoord: edge.trunkCoord,
      routePoints: edge.routePoints,
    }));
    const secondRoutes = second.routed.edges.map((edge) => ({
      id: edge.id,
      channelId: edge.channelId,
      trunkCoord: edge.trunkCoord,
      routePoints: edge.routePoints,
    }));

    expect(secondRoutes).toEqual(firstRoutes);
  });

  it("emits channel and route diagnostics when debug toggles are enabled", () => {
    const fixture = denseHorizontal as LayoutElements;
    const log = vi.fn();

    runDeterministicParityPipeline({
      layout: fixture,
      depth: 5,
      nodeCount: fixture.nodes.length,
      edgeCount: fixture.edges.length,
      layoutDirection: "Horizontal",
      debugChannels: true,
      debugRoutes: true,
      log,
    });

    expect(log).toHaveBeenCalledWith(
      "[parity] channel diagnostics",
      expect.objectContaining({
        channelCount: expect.any(Number),
      }),
    );
    expect(log).toHaveBeenCalledWith(
      "[parity] route diagnostics",
      expect.objectContaining({
        collisionEdges: expect.any(Number),
      }),
    );
  });
});
