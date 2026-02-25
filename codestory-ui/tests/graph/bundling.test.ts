import { describe, expect, it } from "vitest";

import { applySharedTrunkBundling } from "../../src/graph/layout/bundling";
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
    isVirtualBundle: false,
  };
}

describe("applySharedTrunkBundling", () => {
  it("preserves pairwise edges in trace/off modes and bundles in overview mode", () => {
    const center = makeNode("center", 480, 320, 0);
    const rightNodes = Array.from({ length: 18 }, (_, idx) =>
      makeNode(`target-${idx}`, 860, 120 + idx * 28, 1),
    );

    const layout: LayoutElements = {
      nodes: [center, ...rightNodes],
      edges: rightNodes.map((node, idx) => ({
        id: `call-${idx}`,
        source: "center",
        target: node.id,
        sourceHandle: "source-node",
        targetHandle: "target-node",
        kind: "CALL",
        certainty: null,
        multiplicity: 1,
        family: "flow",
        routeKind: "direct",
        bundleCount: 1,
      })),
      centerNodeId: "center",
    };

    const trace = applySharedTrunkBundling(layout, "trace");
    const off = applySharedTrunkBundling(layout, "off");
    const overview = applySharedTrunkBundling(layout, "overview");

    expect(trace).toBe(layout);
    expect(off).toBe(layout);
    expect(overview.nodes.length).toBeGreaterThan(layout.nodes.length);
    expect(overview.nodes.some((node) => node.isVirtualBundle)).toBe(true);
    expect(overview.edges.some((edge) => edge.routeKind === "flow-trunk")).toBe(true);
  });

  it("keeps center-member handles on bundled trunk edges", () => {
    const center = makeNode("center", 480, 320, 0);
    const rightNodes = Array.from({ length: 12 }, (_, idx) =>
      makeNode(`target-${idx}`, 860, 120 + idx * 28, 1),
    );
    const leftNodes = Array.from({ length: 12 }, (_, idx) =>
      makeNode(`source-${idx}`, 120, 120 + idx * 28, -1),
    );

    const layout: LayoutElements = {
      nodes: [center, ...rightNodes, ...leftNodes],
      edges: [
        ...rightNodes.map((node, idx) => ({
          id: `call-out-${idx}`,
          source: "center",
          target: node.id,
          sourceHandle: "source-member-run_incremental",
          targetHandle: "target-node",
          kind: "CALL",
          certainty: null,
          multiplicity: 1,
          family: "flow",
          routeKind: "direct",
          bundleCount: 1,
        })),
        ...leftNodes.map((node, idx) => ({
          id: `call-in-${idx}`,
          source: node.id,
          target: "center",
          sourceHandle: "source-node",
          targetHandle: "target-member-run_incremental",
          kind: "CALL",
          certainty: null,
          multiplicity: 1,
          family: "flow",
          routeKind: "direct",
          bundleCount: 1,
        })),
      ],
      centerNodeId: "center",
    };

    const overview = applySharedTrunkBundling(layout, "overview");
    const centerOutgoingTrunk = overview.edges.find(
      (edge) =>
        edge.routeKind === "flow-trunk" &&
        edge.source === "center" &&
        edge.sourceHandle === "source-member-run_incremental",
    );
    const centerIncomingTrunk = overview.edges.find(
      (edge) =>
        edge.routeKind === "flow-trunk" &&
        edge.target === "center" &&
        edge.targetHandle === "target-member-run_incremental",
    );

    expect(centerOutgoingTrunk).toBeDefined();
    expect(centerIncomingTrunk).toBeDefined();
  });
});
