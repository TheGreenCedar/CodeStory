import { MarkerType, Position } from "@xyflow/react";
import { describe, expect, it } from "vitest";

import { PARITY_CONSTANTS } from "../../src/graph/layout/parityConstants";
import { EDGE_STYLE, toReactFlowElements } from "../../src/graph/layout/routing";
import type {
  LayoutElements,
  RoutedEdgeSpec,
  SemanticNodePlacement,
} from "../../src/graph/layout/types";

function makeNode(id: string, x: number, y: number, xRank: number): SemanticNodePlacement {
  return {
    id,
    kind: "METHOD",
    label: id,
    center: id === "center",
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
    width: 172,
    height: 34,
    isVirtualBundle: false,
  };
}

function makeEdge(
  id: string,
  kind: RoutedEdgeSpec["kind"],
  overrides: Partial<RoutedEdgeSpec> = {},
): RoutedEdgeSpec {
  return {
    id,
    source: "center",
    target: "target",
    sourceEdgeIds: [id],
    sourceHandle: "source-node",
    targetHandle: "target-node",
    kind,
    certainty: null,
    multiplicity: 1,
    family:
      kind === "INHERITANCE" || kind === "TYPE_ARGUMENT" || kind === "TEMPLATE_SPECIALIZATION"
        ? "hierarchy"
        : "flow",
    routeKind: "direct",
    bundleCount: 1,
    routePoints: [],
    ...overrides,
  };
}

function baseLayout(edges: RoutedEdgeSpec[]): LayoutElements {
  return {
    nodes: [makeNode("center", 80, 200, 0), makeNode("target", 480, 200, 2)],
    edges,
    centerNodeId: "center",
  };
}

describe("toReactFlowElements edge styling", () => {
  it("maps marker kind semantics to Sourcetrail parity", () => {
    const layout = baseLayout([
      makeEdge("call", "CALL"),
      makeEdge("override", "OVERRIDE"),
      makeEdge("inheritance", "INHERITANCE"),
      makeEdge("type-arg", "TYPE_ARGUMENT"),
      makeEdge("template-spec", "TEMPLATE_SPECIALIZATION"),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));

    expect(byId.get("call")?.markerEnd).toMatchObject({ type: MarkerType.Arrow });
    expect(byId.get("override")?.markerEnd).toMatchObject({ type: MarkerType.Arrow });
    expect(byId.get("inheritance")?.markerEnd).toMatchObject({ type: MarkerType.ArrowClosed });
    expect(byId.get("type-arg")?.markerEnd).toMatchObject({ type: MarkerType.ArrowClosed });
    expect(byId.get("template-spec")?.markerEnd).toMatchObject({ type: MarkerType.ArrowClosed });
  });

  it("applies marker size tiers for default, bundled, inheritance, and template specialization", () => {
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("bundled", "CALL", { routeKind: "flow-trunk", bundleCount: 9 }),
      makeEdge("inheritance", "INHERITANCE"),
      makeEdge("template-spec", "TEMPLATE_SPECIALIZATION"),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));

    expect(byId.get("default")?.markerEnd).toMatchObject({ width: 10, height: 10 });
    expect(byId.get("bundled")?.markerEnd).toMatchObject({ width: 12, height: 12 });
    expect(byId.get("inheritance")?.markerEnd).toMatchObject({ width: 18, height: 16 });
    expect(byId.get("template-spec")?.markerEnd).toMatchObject({ width: 14, height: 13 });
  });

  it("applies interaction width semantics for default, hierarchy, and bundled routes", () => {
    const bundledCount = 16;
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("hierarchy", "INHERITANCE"),
      makeEdge("bundled", "CALL", { routeKind: "flow-trunk", bundleCount: bundledCount }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));
    const interactionWidth = PARITY_CONSTANTS.rendering.interactionWidth;

    const expectedBundled =
      interactionWidth.bundledBase +
      Math.min(
        interactionWidth.bundledMaxExtra,
        Math.log2(bundledCount) * interactionWidth.bundledScale,
      );

    expect(byId.get("default")?.interactionWidth).toBe(interactionWidth.default);
    expect(byId.get("hierarchy")?.interactionWidth).toBe(interactionWidth.hierarchy);
    expect(byId.get("bundled")?.interactionWidth).toBeCloseTo(expectedBundled, 4);
  });

  it("applies stroke width semantics for default, hierarchy, and bundled routes", () => {
    const bundledCount = 16;
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("hierarchy", "INHERITANCE"),
      makeEdge("bundled", "CALL", { routeKind: "flow-trunk", bundleCount: bundledCount }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));
    const strokeAmplification = PARITY_CONSTANTS.rendering.strokeAmplification;

    const expectedDefault = EDGE_STYLE.CALL.width;
    const expectedHierarchy = EDGE_STYLE.INHERITANCE.width + strokeAmplification.hierarchyBoost;
    const expectedBundled =
      EDGE_STYLE.CALL.width +
      Math.min(
        strokeAmplification.bundledMaxBoost,
        Math.log2(bundledCount) * strokeAmplification.bundledLogMultiplier,
      );

    expect(Number(byId.get("default")?.style?.strokeWidth ?? 0)).toBeCloseTo(expectedDefault, 4);
    expect(Number(byId.get("hierarchy")?.style?.strokeWidth ?? 0)).toBeCloseTo(
      expectedHierarchy,
      4,
    );
    expect(Number(byId.get("bundled")?.style?.strokeWidth ?? 0)).toBeCloseTo(expectedBundled, 4);
  });

  it("thickens bundled trunk edges logarithmically with bundle count", () => {
    const layout = baseLayout([
      makeEdge("bundle-small", "CALL", { routeKind: "flow-trunk", bundleCount: 2 }),
      makeEdge("bundle-large", "CALL", { routeKind: "flow-trunk", bundleCount: 32 }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));
    const smallWidth = Number(byId.get("bundle-small")?.style?.strokeWidth ?? 0);
    const largeWidth = Number(byId.get("bundle-large")?.style?.strokeWidth ?? 0);

    expect(largeWidth).toBeGreaterThan(smallWidth);
  });

  it("preserves orientation-aware edge metadata for vertical layouts", () => {
    const layout = baseLayout([
      makeEdge("vertical-trunk", "CALL", {
        routeKind: "flow-trunk",
        bundleCount: 4,
        trunkCoord: 360,
        channelId: "channel:CALL:center<->target:0",
        channelPairId: "center<->target",
        channelWeight: 4,
        sharedTrunkPoints: [
          { x: 220, y: 360 },
          { x: 420, y: 360 },
        ],
      }),
    ]);
    const { nodes, edges } = toReactFlowElements(layout, "Vertical");
    const firstNode = nodes[0];
    const edge = edges[0];

    expect(firstNode?.sourcePosition).toBe(Position.Bottom);
    expect(firstNode?.targetPosition).toBe(Position.Top);
    expect(edge?.data?.layoutDirection).toBe("Vertical");
    expect(edge?.data?.channelPairId).toBe("center<->target");
    expect(edge?.data?.sharedTrunkPoints).toEqual([
      { x: 220, y: 360 },
      { x: 420, y: 360 },
    ]);
  });

  it("keeps certainty dash and opacity semantics after styling refactors", () => {
    const layout = baseLayout([
      makeEdge("uncertain-flow", "CALL", { certainty: "uncertain", family: "flow" }),
      makeEdge("probable-flow", "CALL", { certainty: "probable", family: "flow" }),
      makeEdge("uncertain-hierarchy", "INHERITANCE", {
        certainty: "uncertain",
        family: "hierarchy",
      }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));

    expect(byId.get("uncertain-flow")?.style?.strokeDasharray).toBe(
      PARITY_CONSTANTS.rendering.certainty.uncertainDash,
    );
    expect(byId.get("uncertain-flow")?.style?.opacity).toBeCloseTo(
      PARITY_CONSTANTS.rendering.certainty.uncertainOpacity,
      4,
    );
    expect(byId.get("probable-flow")?.style?.opacity).toBeCloseTo(
      PARITY_CONSTANTS.rendering.certainty.probableOpacity,
      4,
    );
    expect(byId.get("uncertain-hierarchy")?.style?.opacity).toBeCloseTo(
      Math.min(
        1,
        PARITY_CONSTANTS.rendering.certainty.uncertainOpacity +
          PARITY_CONSTANTS.rendering.certainty.hierarchyOpacityBias,
      ),
      4,
    );
  });
});
