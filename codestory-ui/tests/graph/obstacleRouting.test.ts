import { describe, expect, it } from "vitest";

import {
  routeEdgesWithObstacles,
  routeIntersectionDiagnostics,
  routeIntersectsNonEndpointNode,
} from "../../src/graph/layout/obstacleRouting";
import { PARITY_CONSTANTS } from "../../src/graph/layout/parityConstants";
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
    const sharedTrunkPoints = [
      { x: trunkCoord, y: 136 },
      { x: trunkCoord, y: 344 },
    ];
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
        channelId: "channel:CALL:center<->t:0",
        channelWeight: 3,
        sharedTrunkPoints,
        routePoints: [],
      })),
      centerNodeId: "center",
    };

    const routed = routeEdgesWithObstacles(layout);
    for (const edge of routed.edges) {
      const interior = edge.routePoints.slice(1, -1);
      const closest = Math.min(...interior.map((point) => Math.abs(point.x - trunkCoord)));
      expect(closest).toBeLessThanOrEqual(16);
      expect(
        interior.some(
          (point) => point.y >= sharedTrunkPoints[0]!.y && point.y <= sharedTrunkPoints[1]!.y,
        ),
      ).toBe(true);
    }
  });

  it("keeps bundled branch lanes ordered by source member order", () => {
    const nodes = [
      makeNode("center", 120, 220, 180, 56, 0),
      makeNode("t1", 620, 110, 140, 44, 2),
      makeNode("t2", 620, 220, 140, 44, 2),
      makeNode("t3", 620, 330, 140, 44, 2),
    ];
    const trunkCoord = 392;
    const layout: LayoutElements = {
      nodes,
      edges: ["t1", "t2", "t3"].map((target, idx) => ({
        id: `ordered-${idx}`,
        source: "center",
        target,
        sourceHandle: `source-member-center-${idx}`,
        targetHandle: `target-member-${target}`,
        kind: "CALL" as const,
        certainty: null,
        multiplicity: 1,
        family: "flow" as const,
        routeKind: "flow-trunk" as const,
        bundleCount: 3,
        trunkCoord,
        channelId: "channel:CALL:center<->ordered:0",
        channelWeight: 3,
        sourceMemberOrder: idx,
        targetMemberOrder: idx,
        sharedTrunkPoints: [
          { x: trunkCoord, y: 120 },
          { x: trunkCoord, y: 360 },
        ],
        routePoints: [],
      })),
      centerNodeId: "center",
    };

    const routed = routeEdgesWithObstacles(layout);
    const laneYs = [...routed.edges]
      .sort(
        (left, right) =>
          (left.sourceMemberOrder ?? Number.POSITIVE_INFINITY) -
            (right.sourceMemberOrder ?? Number.POSITIVE_INFINITY) ||
          left.id.localeCompare(right.id),
      )
      .map((edge) => {
        const interior = edge.routePoints.slice(1, -1);
        const nearest = interior.reduce(
          (best, point) => {
            const distance = Math.abs(point.x - trunkCoord);
            return distance < best.distance ? { distance, y: point.y } : best;
          },
          { distance: Number.POSITIVE_INFINITY, y: edge.routePoints[0]?.y ?? 0 },
        );
        return nearest.y;
      });

    for (let idx = 1; idx < laneYs.length; idx += 1) {
      expect((laneYs[idx] ?? 0) + PARITY_CONSTANTS.rasterStep).toBeGreaterThanOrEqual(
        laneYs[idx - 1] ?? 0,
      );
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

  it("applies styled vertical offset on the perpendicular axis", () => {
    const layout: LayoutElements = {
      nodes: [makeNode("src", 20, 20, 140, 44, -1), makeNode("dst", 420, 40, 140, 44, 1)],
      edges: [
        {
          id: "axis-style",
          source: "src",
          target: "dst",
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
      centerNodeId: "src",
    };

    const routed = routeEdgesWithObstacles(layout);
    const edge = routed.edges[0];
    expect(edge).toBeDefined();
    const source = edge?.routePoints[0];
    const firstBend = edge?.routePoints[1];
    expect(source).toBeDefined();
    expect(firstBend).toBeDefined();
    expect(firstBend?.x).not.toBe(source?.x);
    expect(firstBend?.y).not.toBe(source?.y);
  });

  it("returns structured non-endpoint intersection diagnostics", () => {
    const layout: LayoutElements = {
      nodes: [
        makeNode("left", 40, 160, 140, 44, -1),
        makeNode("right", 560, 160, 140, 44, 1),
        makeNode("blocker", 260, 130, 190, 120, 0),
      ],
      edges: [
        {
          id: "diagnostic",
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
          routePoints: [
            { x: 180, y: 184 },
            { x: 330, y: 184 },
            { x: 520, y: 184 },
          ],
        },
      ],
      centerNodeId: "left",
    };

    const report = routeIntersectionDiagnostics(layout.edges[0]!, layout.nodes);
    expect(report.edgeId).toBe("diagnostic");
    expect(report.collisionCount).toBeGreaterThan(0);
    expect(report.intersections[0]).toMatchObject({
      obstacleId: "blocker",
      segmentIndex: expect.any(Number),
      from: expect.any(Object),
      to: expect.any(Object),
    });
  });

  it("ignores horizontal segments that only graze obstacle boundaries", () => {
    const nodes = [
      makeNode("left", 40, 160, 140, 44, -1),
      makeNode("right", 560, 160, 140, 44, 1),
      makeNode("blocker", 260, 130, 190, 120, 0),
    ];
    const report = routeIntersectionDiagnostics(
      {
        id: "grazing-horizontal",
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
        routePoints: [
          { x: 180, y: 130 },
          { x: 520, y: 130 },
        ],
      },
      nodes,
    );

    expect(report.collisionCount).toBe(0);
    expect(report.intersections).toHaveLength(0);
  });

  it("ignores vertical segments that only graze obstacle boundaries", () => {
    const nodes = [
      makeNode("left", 40, 160, 140, 44, -1),
      makeNode("right", 560, 160, 140, 44, 1),
      makeNode("blocker", 260, 130, 190, 120, 0),
    ];
    const report = routeIntersectionDiagnostics(
      {
        id: "grazing-vertical",
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
        routePoints: [
          { x: 260, y: 80 },
          { x: 260, y: 300 },
        ],
      },
      nodes,
    );

    expect(report.collisionCount).toBe(0);
    expect(report.intersections).toHaveLength(0);
  });

  it("routes channels in vertical orientation with a shared horizontal trunk", () => {
    const nodes = [
      makeNode("top", 220, 60, 180, 56, 0),
      makeNode("b1", 120, 520, 140, 44, 2),
      makeNode("b2", 260, 520, 140, 44, 2),
      makeNode("b3", 400, 520, 140, 44, 2),
    ];
    const trunkCoord = 344;
    const layout: LayoutElements = {
      nodes,
      edges: ["b1", "b2", "b3"].map((target, idx) => ({
        id: `v-${idx}`,
        source: "top",
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
        channelId: "channel:CALL:top<->b:0",
        channelWeight: 3,
        sharedTrunkPoints: [
          { x: 110, y: trunkCoord },
          { x: 430, y: trunkCoord },
        ],
        routePoints: [],
      })),
      centerNodeId: "top",
    };

    const routed = routeEdgesWithObstacles(layout, "Vertical");
    for (const edge of routed.edges) {
      const interior = edge.routePoints.slice(1, -1);
      const closest = Math.min(...interior.map((point) => Math.abs(point.y - trunkCoord)));
      expect(closest).toBeLessThanOrEqual(24);
    }
  });
});
