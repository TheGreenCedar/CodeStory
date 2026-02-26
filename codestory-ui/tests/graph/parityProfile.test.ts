import { describe, expect, it } from "vitest";

import { applyAdaptiveBundling } from "../../src/graph/layout/bundling";
import { routeEdgesWithObstacles } from "../../src/graph/layout/obstacleRouting";
import { PARITY_CONSTANTS } from "../../src/graph/layout/parityConstants";
import type { LayoutElements, SemanticNodePlacement } from "../../src/graph/layout/types";

function node(id: string, x: number, y: number, xRank: number): SemanticNodePlacement {
  return {
    id,
    kind: "METHOD",
    label: id,
    center: false,
    nodeStyle: "pill",
    isNonIndexed: false,
    duplicateCount: 1,
    mergedSymbolIds: [id],
    memberCount: 1,
    members: [],
    xRank,
    yRank: 0,
    x,
    y,
    width: 171,
    height: 47,
    isVirtualBundle: false,
  };
}

describe("PARITY_CONSTANTS integration", () => {
  it("drives bundling activation threshold through minEdgesForBundling", () => {
    const center = node("center", 120, 160, 0);
    const target = node("target", 640, 160, 1);
    const edgeCount = PARITY_CONSTANTS.bundling.minEdgesForBundling;

    const makeLayout = (edgeCount: number): LayoutElements => ({
      nodes: [center, target],
      edges: Array.from({ length: edgeCount }, (_, idx) => ({
        id: `edge-${idx}`,
        sourceEdgeIds: [`edge-${idx}`],
        source: "center",
        target: "target",
        sourceHandle: `source-member-center-${idx % 4}`,
        targetHandle: `target-member-target-${idx % 4}`,
        kind: "CALL" as const,
        certainty: null,
        multiplicity: 1,
        family: "flow" as const,
        routeKind: "direct" as const,
        bundleCount: 1,
        routePoints: [],
      })),
      centerNodeId: "center",
    });

    const belowThreshold = applyAdaptiveBundling(
      makeLayout(PARITY_CONSTANTS.bundling.minEdgesForBundling - 1),
      4,
      180,
      420,
    );
    const atThreshold = applyAdaptiveBundling(makeLayout(edgeCount), 4, 180, 420);

    expect(belowThreshold.edges.every((edge) => edge.routeKind !== "flow-trunk")).toBe(true);
    expect(atThreshold.edges.some((edge) => edge.routeKind === "flow-trunk")).toBe(true);
  });

  it("uses routing rasterStep when snapping routed path points", () => {
    const layout: LayoutElements = {
      nodes: [node("source", 37, 113, 0), node("target", 511, 297, 1)],
      edges: [
        {
          id: "raster-edge",
          sourceEdgeIds: ["raster-edge"],
          source: "source",
          target: "target",
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
      centerNodeId: "source",
    };

    const routed = routeEdgesWithObstacles(layout);
    const edge = routed.edges[0];
    const step = PARITY_CONSTANTS.rasterStep;
    expect(edge).toBeDefined();
    expect(
      edge?.routePoints.every(
        (point) => Number.isInteger(point.x / step) && Number.isInteger(point.y / step),
      ),
    ).toBe(true);
  });
});
