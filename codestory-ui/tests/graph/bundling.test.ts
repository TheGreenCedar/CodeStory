import { describe, expect, it } from "vitest";

import { applyAdaptiveBundling } from "../../src/graph/layout/bundling";
import { PARITY_CONSTANTS } from "../../src/graph/layout/parityConstants";
import type { LayoutElements, SemanticNodePlacement } from "../../src/graph/layout/types";

function compareDeterministicString(left: string, right: string): number {
  if (left === right) {
    return 0;
  }
  return left < right ? -1 : 1;
}

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

function pairLayout(edgeCount: number): LayoutElements {
  const center = makeNode("center", 320, 260, 0);
  const target = makeNode("target", 840, 300, 2);
  return {
    nodes: [center, target],
    edges: Array.from({ length: edgeCount }, (_, idx) => ({
      id: `pair-call-${idx}`,
      source: idx % 2 === 0 ? "center" : "target",
      target: idx % 2 === 0 ? "target" : "center",
      sourceHandle: `source-member-${idx % 4}`,
      targetHandle: `target-member-${idx % 4}`,
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
  it("keeps channel assignment deterministic across insertion order", () => {
    const layout = pairLayout(20);
    const reversed = {
      ...layout,
      edges: [...layout.edges].reverse(),
    };

    const first = applyAdaptiveBundling(layout, 4, 180, 420);
    const second = applyAdaptiveBundling(reversed, 4, 180, 420);

    const firstById = new Map(
      first.edges.map((edge) => [
        edge.id,
        {
          routeKind: edge.routeKind,
          channelId: edge.channelId,
          trunkCoord: edge.trunkCoord,
          sourceMemberOrder: edge.sourceMemberOrder,
          targetMemberOrder: edge.targetMemberOrder,
        },
      ]),
    );
    const secondById = new Map(
      second.edges.map((edge) => [
        edge.id,
        {
          routeKind: edge.routeKind,
          channelId: edge.channelId,
          trunkCoord: edge.trunkCoord,
          sourceMemberOrder: edge.sourceMemberOrder,
          targetMemberOrder: edge.targetMemberOrder,
        },
      ]),
    );

    expect(secondById).toEqual(firstById);
  });

  it("increases bundling compression as depth grows on fixed fixtures", () => {
    const layout = pairLayout(22);
    const shallow = applyAdaptiveBundling(layout, 1, 40, 70);
    const deep = applyAdaptiveBundling(layout, 5, 220, 520);

    const shallowBundled = shallow.edges.filter((edge) => edge.routeKind === "flow-trunk").length;
    const deepBundled = deep.edges.filter((edge) => edge.routeKind === "flow-trunk").length;
    expect(deepBundled).toBeGreaterThanOrEqual(shallowBundled);
    expect(deepBundled).toBeGreaterThan(0);
  });

  it("assigns stable channel metadata for trunked edges", () => {
    const layout = pairLayout(16);
    const bundled = applyAdaptiveBundling(layout, 4, 180, 420);
    const trunk = bundled.edges.find((edge) => edge.routeKind === "flow-trunk");
    const trunkEdges = bundled.edges
      .filter((edge) => edge.routeKind === "flow-trunk")
      .sort((left, right) => left.id.localeCompare(right.id));

    expect(trunk).toBeDefined();
    expect(trunk?.channelId?.startsWith("channel:CALL:center<->")).toBe(true);
    expect(trunk?.channelPairId).toContain("<->");
    expect((trunk?.channelWeight ?? 0) > 1).toBe(true);
    expect(typeof trunk?.trunkCoord).toBe("number");
    expect((trunk?.sharedTrunkPoints?.length ?? 0) >= 2).toBe(true);
    expect(typeof trunk?.sourceMemberOrder).toBe("number");
    expect(trunkEdges.every((edge) => typeof edge.sourceMemberOrder === "number")).toBe(true);
    expect(trunkEdges.every((edge) => typeof edge.targetMemberOrder === "number")).toBe(true);

    const bySourceHandle = new Map<string, number>();
    for (const edge of trunkEdges) {
      const order = edge.sourceMemberOrder ?? -1;
      const existing = bySourceHandle.get(edge.sourceHandle);
      if (typeof existing === "number") {
        expect(order).toBe(existing);
      } else {
        bySourceHandle.set(edge.sourceHandle, order);
      }
    }
  });

  it("uses deterministic code-point ordering for locale-sensitive ids and handles", () => {
    const umlautA = "\u00E4";
    const accentNodeId = `${umlautA}lpha`;
    const layout: LayoutElements = {
      nodes: [makeNode("zeta", 240, 220, 0), makeNode(accentNodeId, 760, 260, 2)],
      edges: [
        `edge-${umlautA}-03`,
        "edge-z-02",
        `edge-${umlautA}-01`,
        "edge-z-01",
        `edge-${umlautA}-02`,
        "edge-z-03",
        `edge-${umlautA}-00`,
        "edge-z-00",
        `edge-${umlautA}-07`,
        "edge-z-06",
        `edge-${umlautA}-05`,
        "edge-z-05",
        `edge-${umlautA}-06`,
        "edge-z-07",
        `edge-${umlautA}-04`,
        "edge-z-04",
      ].map((id) => {
        const zHandle = id.includes("-z-");
        return {
          id,
          source: "zeta",
          target: accentNodeId,
          sourceHandle: zHandle ? "source-member-z" : `source-member-${umlautA}`,
          targetHandle: zHandle ? "target-member-z" : `target-member-${umlautA}`,
          kind: "CALL" as const,
          certainty: null,
          multiplicity: 1,
          family: "flow" as const,
          routeKind: "direct" as const,
          bundleCount: 1,
          routePoints: [],
        };
      }),
      centerNodeId: "zeta",
    };

    const bundled = applyAdaptiveBundling(layout, 4, 180, 420);
    const trunk = bundled.edges.find((edge) => edge.routeKind === "flow-trunk");
    const zHandleEdge = bundled.edges.find((edge) => edge.sourceHandle === "source-member-z");
    const accentHandleEdge = bundled.edges.find(
      (edge) => edge.sourceHandle === `source-member-${umlautA}`,
    );

    expect(trunk?.channelPairId).toBe(`zeta<->${accentNodeId}`);
    expect(trunk?.channelId?.startsWith(`channel:CALL:zeta<->${accentNodeId}:`)).toBe(true);
    expect(zHandleEdge?.sourceMemberOrder).toBe(0);
    expect(accentHandleEdge?.sourceMemberOrder).toBe(1);
    expect(bundled.edges.map((edge) => edge.id)).toEqual(
      layout.edges.map((edge) => edge.id).sort(compareDeterministicString),
    );
  });

  it("pairs reverse-direction members into one canonical channel", () => {
    const center = makeNode("center", 240, 220, 0);
    const target = makeNode("target", 760, 260, 2);
    const edges = Array.from({ length: 8 }, (_, idx) => {
      const forward = idx % 2 === 0;
      return {
        id: `edge-${idx}`,
        source: forward ? "center" : "target",
        target: forward ? "target" : "center",
        sourceHandle: `source-member-${forward ? "center" : "target"}-${idx % 3}`,
        targetHandle: `target-member-${forward ? "target" : "center"}-${idx % 3}`,
        kind: "CALL" as const,
        certainty: null,
        multiplicity: 1,
        family: "flow" as const,
        routeKind: "direct" as const,
        bundleCount: 1,
        routePoints: [],
      };
    });
    const layout: LayoutElements = {
      nodes: [center, target],
      edges,
      centerNodeId: "center",
    };

    const bundled = applyAdaptiveBundling(layout, 4, 90, 240);
    const channelIds = new Set(
      bundled.edges
        .filter((edge) => edge.routeKind === "flow-trunk")
        .map((edge) => edge.channelId)
        .filter((value): value is string => typeof value === "string"),
    );

    expect(channelIds.size).toBe(1);
  });

  it("keeps hierarchy edges outside flow channels", () => {
    const layout = baseLayout(12);
    layout.edges.push({
      id: "inherit-1",
      source: "center",
      target: "target-0",
      sourceHandle: "source-node-top",
      targetHandle: "target-node-bottom",
      kind: "INHERITANCE",
      certainty: null,
      multiplicity: 1,
      family: "hierarchy",
      routeKind: "hierarchy",
      bundleCount: 1,
      routePoints: [],
    });

    const bundled = applyAdaptiveBundling(layout, 4, 180, 420);
    const inheritance = bundled.edges.find((edge) => edge.id === "inherit-1");
    expect(inheritance?.routeKind).toBe("hierarchy");
    expect(inheritance?.channelId).toBeUndefined();
  });

  it("passes through non-qualifying groups without mutating source identity fields", () => {
    const lowDensityEdgeCount = Math.max(1, PARITY_CONSTANTS.bundling.minEdgesForBundling - 1);
    const layout = baseLayout(lowDensityEdgeCount);
    const before = layout.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
      kind: edge.kind,
    }));

    const bundled = applyAdaptiveBundling(layout, 2, 20, lowDensityEdgeCount);
    expect(bundled.edges).toHaveLength(layout.edges.length);

    const after = bundled.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
      kind: edge.kind,
    }));

    expect(after).toEqual(before);
    expect(bundled.edges.every((edge) => edge.routeKind !== "flow-trunk")).toBe(true);
  });
});
