import { describe, expect, it } from "vitest";

import {
  routeEdgesWithObstacles,
  routeIntersectsNonEndpointNode,
} from "../../src/graph/layout/obstacleRouting";
import type { LayoutElements, SemanticNodePlacement } from "../../src/graph/layout/types";

function makeNode(
  id: string,
  x: number,
  y: number,
  width = 140,
  height = 44,
  xRank = 0,
): SemanticNodePlacement {
  return {
    id,
    kind: "METHOD",
    label: id,
    center: id === "center",
    nodeStyle: "pill",
    duplicateCount: 1,
    mergedSymbolIds: [id],
    memberCount: 1,
    members: [],
    xRank,
    yRank: 0,
    x,
    y,
    width,
    height,
    isVirtualBundle: false,
  };
}

describe("routeEdgesWithObstacles", () => {
  it("keeps routes out of non-endpoint node rectangles", () => {
    const nodes = [
      makeNode("left", 40, 160, 140, 44, -1),
      makeNode("right-top", 560, 90, 140, 44, 1),
      makeNode("right-bottom", 560, 260, 140, 44, 1),
      makeNode("blocker", 260, 150, 180, 110, 0),
    ];

    const layout: LayoutElements = {
      nodes,
      edges: [
        {
          id: "e1",
          source: "left",
          target: "right-top",
          sourceHandle: "source-node",
          targetHandle: "target-node",
          kind: "CALL",
          certainty: null,
          multiplicity: 1,
          family: "flow",
          routeKind: "direct",
          bundleCount: 1,
          routePoints: [],
        },
        {
          id: "e2",
          source: "left",
          target: "right-bottom",
          sourceHandle: "source-node",
          targetHandle: "target-node",
          kind: "CALL",
          certainty: null,
          multiplicity: 1,
          family: "flow",
          routeKind: "direct",
          bundleCount: 1,
          routePoints: [],
        },
      ],
      centerNodeId: "left",
    };

    const routed = routeEdgesWithObstacles(layout);
    for (const edge of routed.edges) {
      expect(routeIntersectsNonEndpointNode(edge, routed.nodes), edge.id).toBe(false);
    }
  });

  it("prefers configured channel trunks for bundled edges", () => {
    const nodes = [
      makeNode("center", 120, 200, 180, 56, 0),
      makeNode("t1", 620, 120, 140, 44, 2),
      makeNode("t2", 620, 220, 140, 44, 2),
      makeNode("t3", 620, 320, 140, 44, 2),
    ];
    const trunkCoord = 392;
    const layout: LayoutElements = {
      nodes,
      edges: ["t1", "t2", "t3"].map((target, idx) => ({
        id: `trunk-${idx}`,
        source: "center",
        target,
        sourceHandle: "source-node",
        targetHandle: "target-node",
        kind: "CALL" as const,
        certainty: null,
        multiplicity: 1,
        family: "flow" as const,
        routeKind: "flow-trunk" as const,
        bundleCount: 3,
        trunkCoord,
        channelId: "channel:CALL:center:right:0",
        channelWeight: 3,
        routePoints: [],
      })),
      centerNodeId: "center",
    };

    const routed = routeEdgesWithObstacles(layout);
    for (const edge of routed.edges) {
      const interior = edge.routePoints.slice(1, -1);
      const closest = Math.min(...interior.map((point) => Math.abs(point.x - trunkCoord)));
      expect(closest).toBeLessThanOrEqual(24);
    }
  });

  it("returns a valid fallback path when direct corridor is blocked", () => {
    const layout: LayoutElements = {
      nodes: [
        makeNode("left", 40, 160, 140, 44, -1),
        makeNode("right", 560, 160, 140, 44, 1),
        makeNode("blocker", 250, 120, 220, 130, 0),
      ],
      edges: [
        {
          id: "blocked",
          source: "left",
          target: "right",
          sourceHandle: "source-node",
          targetHandle: "target-node",
          kind: "CALL",
          certainty: null,
          multiplicity: 1,
          family: "flow",
          routeKind: "direct",
          bundleCount: 1,
          routePoints: [],
        },
      ],
      centerNodeId: "left",
    };

    const routed = routeEdgesWithObstacles(layout);
    const edge = routed.edges[0];
    expect(edge).toBeDefined();
    expect((edge?.routePoints.length ?? 0) >= 4).toBe(true);
    expect(edge ? routeIntersectsNonEndpointNode(edge, routed.nodes) : true).toBe(false);
  });
});
