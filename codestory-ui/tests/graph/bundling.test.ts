import { describe, expect, it } from "vitest";

import { applyAdaptiveBundling } from "../../src/graph/layout/bundling";
import type { LayoutElements, SemanticNodePlacement } from "../../src/graph/layout/types";

function makeNode(id: string, x: number, y: number, xRank: number): SemanticNodePlacement {
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
    width: 172,
    height: 34,
    isVirtualBundle: false,
  };
}

function baseLayout(edgeCount: number): LayoutElements {
  const center = makeNode("center", 520, 300, 0);
  const rightNodes = Array.from({ length: edgeCount }, (_, idx) =>
    makeNode(`target-${idx}`, 960, 90 + idx * 20, 2),
  );
  return {
    nodes: [center, ...rightNodes],
    edges: rightNodes.map((node, idx) => ({
      id: `call-${idx}`,
      source: "center",
      target: node.id,
      sourceHandle: `source-member-center-${idx % 4}`,
      targetHandle: "target-node",
      kind: "CALL",
      certainty: null,
      multiplicity: 1,
      family: "flow",
      routeKind: "direct",
      bundleCount: 1,
      routePoints: [],
    })),
    centerNodeId: "center",
  };
}

describe("applyAdaptiveBundling", () => {
  it("increases bundling compression as depth grows on fixed fixtures", () => {
    const layout = baseLayout(22);
    const shallow = applyAdaptiveBundling(layout, 1, 40, 70);
    const deep = applyAdaptiveBundling(layout, 5, 220, 520);

    const shallowBundled = shallow.edges.filter((edge) => edge.routeKind === "flow-trunk").length;
    const deepBundled = deep.edges.filter((edge) => edge.routeKind === "flow-trunk").length;
    expect(deepBundled).toBeGreaterThanOrEqual(shallowBundled);
    expect(deepBundled).toBeGreaterThan(0);
  });

  it("assigns stable channel metadata for trunked edges", () => {
    const layout = baseLayout(16);
    const bundled = applyAdaptiveBundling(layout, 4, 180, 420);
    const trunk = bundled.edges.find((edge) => edge.routeKind === "flow-trunk");

    expect(trunk).toBeDefined();
    expect(trunk?.channelId?.startsWith("channel:CALL:center:right")).toBe(true);
    expect((trunk?.channelWeight ?? 0) > 1).toBe(true);
    expect(typeof trunk?.trunkCoord).toBe("number");
  });
});
