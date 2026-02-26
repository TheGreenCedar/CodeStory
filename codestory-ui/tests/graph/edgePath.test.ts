import { describe, expect, it } from "vitest";

import { buildEdgePath, decorateBundledTrunkJoins } from "../../src/graph/render/edgePath";
import type { SemanticEdgeData } from "../../src/graph/layout/routing";

function trunkData(overrides: Partial<SemanticEdgeData> = {}): SemanticEdgeData {
  return {
    edgeKind: "CALL",
    sourceEdgeIds: ["edge-1"],
    certainty: null,
    routeKind: "flow-trunk",
    family: "flow",
    bundleCount: 6,
    routePoints: [],
    trunkCoord: 100,
    layoutDirection: "Horizontal",
    ...overrides,
  };
}

describe("edgePath renderer", () => {
  it("decorates bundled trunk joins outward for rightward fan-out", () => {
    const points = [
      { x: 100, y: 0 },
      { x: 100, y: 100 },
      { x: 200, y: 100 },
      { x: 200, y: 140 },
    ];
    const decorated = decorateBundledTrunkJoins(points, trunkData());

    expect(decorated.length).toBeGreaterThan(points.length);
    expect(decorated.every((point) => point.x >= 100)).toBe(true);
  });

  it("decorates bundled trunk joins outward for leftward fan-out", () => {
    const points = [
      { x: 100, y: 0 },
      { x: 100, y: 100 },
      { x: 20, y: 100 },
      { x: 20, y: 140 },
    ];
    const decorated = decorateBundledTrunkJoins(points, trunkData());

    expect(decorated.length).toBeGreaterThan(points.length);
    expect(decorated.every((point) => point.x <= 100)).toBe(true);
  });

  it("decorates bundled trunk joins when trunkCoord is unsnapped", () => {
    const points = [
      { x: 104, y: 0 },
      { x: 104, y: 104 },
      { x: 200, y: 104 },
      { x: 200, y: 140 },
    ];
    const decorated = decorateBundledTrunkJoins(points, trunkData({ trunkCoord: 101 }));

    expect(decorated.length).toBeGreaterThan(points.length);
    expect(decorated.every((point) => point.x >= 104)).toBe(true);
  });

  it("rewrites routed endpoints to live handle coordinates", () => {
    const data = trunkData({
      routePoints: [
        { x: 120, y: 40 },
        { x: 100, y: 40 },
        { x: 100, y: 80 },
        { x: 220, y: 80 },
      ],
    });
    const result = buildEdgePath(16, 24, 232, 96, data);

    expect(result.path.startsWith("M 16 24")).toBe(true);
    expect(result.path.includes("232 96")).toBe(true);
    expect(Number.isFinite(result.labelX)).toBe(true);
    expect(Number.isFinite(result.labelY)).toBe(true);
  });
});
