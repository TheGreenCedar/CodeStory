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
    routeKind: kind === "INHERITANCE" ? "hierarchy" : "direct",
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

  it("uses bundled marker tier when multiplicity is greater than one", () => {
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("multi", "CALL", { multiplicity: 6 }),
      makeEdge("inheritance", "INHERITANCE"),
      makeEdge("template-spec", "TEMPLATE_SPECIALIZATION"),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));

    expect(byId.get("default")?.markerEnd).toMatchObject({ width: 10, height: 10 });
    expect(byId.get("multi")?.markerEnd).toMatchObject({ width: 12, height: 12 });
    expect(byId.get("inheritance")?.markerEnd).toMatchObject({ width: 18, height: 16 });
    expect(byId.get("template-spec")?.markerEnd).toMatchObject({ width: 14, height: 13 });
  });

  it("applies interaction width semantics for default and hierarchy edges", () => {
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("hierarchy", "INHERITANCE"),
      makeEdge("multi", "CALL", { multiplicity: 5 }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));
    const interactionWidth = PARITY_CONSTANTS.rendering.interactionWidth;

    expect(byId.get("default")?.interactionWidth).toBe(interactionWidth.default);
    expect(byId.get("hierarchy")?.interactionWidth).toBe(interactionWidth.hierarchy);
    expect(Number(byId.get("multi")?.interactionWidth ?? 0)).toBeGreaterThan(
      interactionWidth.default,
    );
  });

  it("applies stroke width semantics for default, hierarchy, and multiplicity", () => {
    const layout = baseLayout([
      makeEdge("default", "CALL"),
      makeEdge("hierarchy", "INHERITANCE"),
      makeEdge("multi", "CALL", { multiplicity: 7 }),
    ]);
    const { edges } = toReactFlowElements(layout);
    const byId = new Map(edges.map((edge) => [edge.id, edge]));
    const strokeAmplification = PARITY_CONSTANTS.rendering.strokeAmplification;

    const expectedDefault = EDGE_STYLE.CALL.width;
    const expectedHierarchy = EDGE_STYLE.INHERITANCE.width + strokeAmplification.hierarchyBoost;

    expect(Number(byId.get("default")?.style?.strokeWidth ?? 0)).toBeCloseTo(expectedDefault, 4);
    expect(Number(byId.get("hierarchy")?.style?.strokeWidth ?? 0)).toBeCloseTo(
      expectedHierarchy,
      4,
    );
    expect(Number(byId.get("multi")?.style?.strokeWidth ?? 0)).toBeGreaterThan(expectedDefault);
  });

  it("preserves orientation-aware metadata for vertical layouts", () => {
    const layout = baseLayout([
      makeEdge("vertical", "CALL", {
        multiplicity: 3,
        routePoints: [
          { x: 160, y: 210 },
          { x: 300, y: 280 },
        ],
      }),
    ]);
    const { nodes, edges } = toReactFlowElements(layout, "Vertical");
    const firstNode = nodes[0];
    const edge = edges[0];

    expect(firstNode?.sourcePosition).toBe(Position.Bottom);
    expect(firstNode?.targetPosition).toBe(Position.Top);
    expect(edge?.data?.layoutDirection).toBe("Vertical");
    expect(edge?.data?.routePoints).toEqual([
      { x: 160, y: 210 },
      { x: 300, y: 280 },
    ]);
  });

  it("keeps certainty dash and opacity semantics after routing simplification", () => {
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
